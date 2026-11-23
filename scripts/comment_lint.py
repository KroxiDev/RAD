import argparse
from bisect import bisect_right
from collections import Counter, defaultdict
from dataclasses import dataclass
from io import StringIO
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tokenize

from pygments.lexers import get_lexer_by_name
from pygments.token import Comment
import yaml


LANGUAGES = {
    ".rs": "rust", ".py": "python", ".sql": "sql", ".sh": "bash",
    ".toml": "toml", ".yaml": "yaml", ".yml": "yaml", ".xml": "xml",
    ".svg": "xml", ".policy": "xml", ".service": "systemd", ".desktop": "desktop",
    ".install": "bash", ".bash": "bash", ".mk": "make",
}
NAMES = {"PKGBUILD.in": "bash", "PKGBUILD": "bash", "Makefile": "make",
         ".gitignore": "git", ".gitattributes": "git"}
EXCLUDED = (".aws/", ".env/", ".codex/", "plugins/notices/", "packaging/notices/", "crates/rad-editor/assets/phosphor/")
RUST_TOKEN = re.compile(
    r'\b(?:br|cr|r)(?P<hashes>\#{0,255})".*?"(?P=hashes)'
    r'''|"(?:[^"\\]++|\\.)*"|'(?:\\(?:u\{[^}]*\}|x..|.)|[^'\\\n])' '''
    r'|(?P<line>//[^\n]*)|(?P<block>/\*)', re.DOTALL | re.VERBOSE)
BLOCK_DELIMITER = re.compile(r'/\*|\*/')


def git(*args):
    return subprocess.check_output(["git", *args])


def language(path):
    if str(path).startswith(EXCLUDED) or any(part == ".env" or part.startswith(".env.") for part in path.parts):
        return None
    return NAMES.get(path.name) or LANGUAGES.get(path.suffix)


def rust_spans(source):
    pos = 0
    while match := RUST_TOKEN.search(source, pos):
        pos = match.end()
        if match.lastgroup == "line":
            yield match.span()
        elif match.lastgroup == "block":
            depth = 1
            for delimiter in BLOCK_DELIMITER.finditer(source, pos):
                depth += 1 if delimiter[0] == "/*" else -1
                if depth == 0:
                    pos = delimiter.end()
                    break
            else:
                raise ValueError("comentario Rust sin cierre")
            yield match.start(), pos


def lexer_spans(dialect, source):
    lexer = get_lexer_by_name(dialect)
    for start, kind, value in lexer.get_tokens_unprocessed(source + "\n"):
        if kind in Comment and kind not in Comment.Preproc and kind not in Comment.Hashbang:
            end = min(len(source), start + len(value.rstrip("\n")))
            if end > start:
                yield start, end


SQL_TOKEN = re.compile(
    r"""'(?:''|[^'])*'|"(?:""|[^"])*"|`(?:``|[^`])*`|\[[^\]]*\]"""
    r"|(?P<comment>--[^\n]*|/\*.*?(?:\*/|\Z))", re.DOTALL)


def shell_spans(source):
    if "#" not in source:
        return
    shfmt = shutil.which("shfmt") or Path(__file__).resolve().parents[1] / "target/comment-tools/shfmt"
    if not Path(shfmt).is_file():
        raise SystemExit("Falta shfmt para analizar Bash. Consulta docs/testing.md.")
    ast = subprocess.check_output([shfmt, "-ln", "bash", "--to-json"], input=source.encode())
    lines = source.split("\n")
    starts = [0, *(m.end() for m in re.finditer("\n", source))]
    pending = [json.loads(ast)]
    while pending:
        node = pending.pop()
        if isinstance(node, list):
            pending.extend(node)
        elif isinstance(node, dict):
            if "Hash" in node and "End" in node:
                row = node["Hash"]["Line"] - 1
                col = node["Hash"]["Col"] - 1
                start = starts[row] + len(lines[row].encode()[:col].decode())
                yield start, starts[row] + len(lines[row])
            else:
                pending.extend(node.values())


def make_spans(source):
    recipes = {start: end for start, end in lexer_spans("make", source)}
    pos = 0
    for line in source.splitlines(keepends=True):
        if line.startswith("\t"):
            for start, end in recipes.items():
                if pos <= start < pos + len(line):
                    yield start, end
        elif match := re.search(r"(?:^|[^\\])(?:\\\\)*(?P<hash>#)", line):
            yield pos + match.start("hash"), pos + len(line.rstrip("\n"))
        pos += len(line)


def mapping(node):
    if isinstance(node, yaml.MappingNode):
        return {key.value: value for key, value in node.value}
    return {}


def default_shell(fields, fallback):
    run = mapping(mapping(fields.get("defaults")).get("run"))
    return run.get("shell", fallback)


def workflow_runs(root):
    fields = mapping(root)
    default = default_shell(fields, None)
    for job in mapping(fields.get("jobs")).values():
        fields = mapping(job)
        steps = fields.get("steps")
        if isinstance(steps, yaml.SequenceNode):
            for node in steps.value:
                step = mapping(node)
                run = step.get("run")
                shell = step.get("shell", default_shell(fields, default))
                command = shell.value.split() if isinstance(shell, yaml.ScalarNode) else ["bash"]
                if command and Path(command[0]).name in ("sh", "bash") and isinstance(run, yaml.ScalarNode):
                    yield run


@dataclass(frozen=True)
class Entry:
    line: int
    text: str
    standalone: bool


def entries(path, source):
    dialect = language(path)
    lines = source.split("\n")
    starts = [0]
    starts.extend(match.end() for match in re.finditer("\n", source))
    if dialect == "rust":
        spans = rust_spans(source)
    elif dialect == "python":
        tokens = tokenize.generate_tokens(StringIO(source).readline)
        spans = ((starts[t.start[0]-1] + t.start[1], starts[t.end[0]-1] + t.end[1])
                 for t in tokens if t.type == tokenize.COMMENT)
    elif dialect == "sql":
        spans = (m.span() for m in SQL_TOKEN.finditer(source) if m.lastgroup == "comment")
    elif dialect == "bash":
        spans = shell_spans(source)
    elif dialect == "make":
        spans = make_spans(source)
    elif dialect == "git":
        spans = (m.span() for m in re.finditer(r"^#[^\n]*", source, re.MULTILINE))
    else:
        spans = lexer_spans(dialect, source)

    pieces = defaultdict(list)
    for start, end in spans:
        first, last = bisect_right(starts, start)-1, bisect_right(starts, end-1)-1
        for row in range(first, last+1):
            left = max(0, start-starts[row])
            right = min(len(lines[row]), end-starts[row])
            pieces[row].append((left, right))

    if dialect == "yaml" and path.parts[:2] == (".github", "workflows"):
        for run in workflow_runs(yaml.compose(source, Loader=yaml.BaseLoader)):
            body = re.sub(r"\$\{\{.*?\}\}", lambda m: "x" * len(m[0]), run.value)
            for a, b in sorted(shell_spans(body)):
                text = run.value[a:b]
                if run.style == "|":
                    row = run.start_mark.line + 1 + body.count("\n", 0, a)
                    left = lines[row].rindex(text)
                    pieces[row].append((left, left+len(text)))
                else:
                    yield Entry(run.start_mark.line+1, text, False)

    for row, segments in sorted(pieces.items()):
        line = lines[row]
        mask = list(line)
        for left, right in segments:
            mask[left:right] = " " * (right-left)
        for left, right in sorted(segments):
            text = line[left:right]
            if row == 0 and dialect in ("bash", "python") and re.match(r"^#![ \t]*/", line):
                continue
            if dialect == "python" and row < 2 and re.fullmatch(r"#[-* \t]*coding[:=]\s*[-\w.]+[ \t*-]*", line):
                continue
            if dialect == "bash" and re.fullmatch(r"# shellcheck source=[\w./-]+", text):
                continue
            yield Entry(row+1, text, not "".join(mask).strip())


def tracked_tree(ref):
    files = {}
    for item in git("ls-tree", "-rz", ref).split(b"\0"):
        if item:
            meta, name = item.split(b"\t", 1)
            mode, _, oid = meta.split()
            if mode in (b"100644", b"100755"):
                files[Path(os.fsdecode(name))] = oid.decode()
    return files


def paths(*args):
    return {Path(os.fsdecode(s)) for s in git(*args).split(b"\0") if s}


def changed_paths(base):
    fields = iter(git("diff", "--name-status", "-z", "--find-renames", base, "--").split(b"\0"))
    changes = {}
    for status in fields:
        if status:
            old = Path(os.fsdecode(next(fields)))
            path = Path(os.fsdecode(next(fields))) if status.startswith(b"R") else old
            changes[path] = old
    for path in paths("ls-files", "--others", "--exclude-standard", "-z"):
        changes.setdefault(path, path)
    return changes


def blob(oid):
    return git("cat-file", "blob", oid).decode().replace("\r\n", "\n")


def working_source(path):
    if path.absolute() == path.resolve() and path.is_file():
        return path.read_text()
    return ""


def main():
    parser = argparse.ArgumentParser(description="Detecta comentarios añadidos al código propio.")
    parser.add_argument("--base", help="revisión de comparación")
    parser.add_argument("--inventory", action="store_true", help="emitir las líneas comentadas como JSONL")
    parser.add_argument("--revision", help="inventariar una revisión en lugar del árbol de trabajo")
    args = parser.parse_args()
    if args.revision and not args.inventory:
        parser.error("--revision requiere --inventory")
    os.chdir(os.fsdecode(git("rev-parse", "--show-toplevel")).rstrip("\n"))
    if args.inventory:
        snapshot = tracked_tree(args.revision) if args.revision else {}
        files = snapshot.keys() if args.revision else paths("ls-files", "-coz", "--exclude-standard")
        for path in sorted(p for p in files if language(p)):
            source = blob(snapshot[path]) if args.revision else working_source(path)
            for entry in entries(path, source):
                print(json.dumps({"path": str(path), **vars(entry)}, ensure_ascii=False))
        return 0

    base = args.base or os.environ.get("RAD_COMMENT_BASE")
    if not base:
        base = git("merge-base", "HEAD", "origin/beta").decode().strip()
    previous = tracked_tree(base)
    changed = changed_paths(base)
    errors = 0
    for path in sorted(p for p in changed if language(p)):
        origin = changed[path]
        allowed = Counter()
        if origin in previous and language(origin):
            allowed.update(entry.text for entry in entries(origin, blob(previous[origin])))
        for entry in entries(path, working_source(path)):
            if allowed[entry.text] == 0:
                print(f"{path}:{entry.line}: comentario nuevo")
                errors += 1
            else:
                allowed[entry.text] -= 1
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
