from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from comment_lint import entries


SCRIPT = Path(__file__).with_name("comment_lint.py").resolve()


class CommentLintTests(unittest.TestCase):
    def test_only_comment_tokens_are_reported(self):
        examples = [
            ("mod.rs", 'const X:&str=r###" // /* "###; /* a /* b */ c */ // d',
             ["/* a /* b */ c */", "// d"]),
            ("example.py", 'print("# dato")  # real\n', ["# real"]),
            ("Cargo.toml", 'url = "a#b" # real', ["# real"]),
            ("schema.sql", "/* a /* b */ SELECT [item--id], '--'; -- real", ["/* a /* b */", "-- real"]),
            ("Makefile", 'word := yes# real\nall:\n\techo "# dato" # receta\n', ["# real", "# receta"]),
            ("run.sh", "#!/bin/bash\nfoo=#dato\ncat <<'END' # real\n# dato\nEND\n#\n", ["# real", "#"]),
            (".github/workflows/test.yml", """env:
  run: 'display # dato'
jobs:
  lint:
    defaults:
      run:
        shell: python
    steps:
    - run: |
        for i in range(2):
            print(i)
    - shell: bash
      run: 'echo ok  # uno'
    - shell: sh
      run: >
        echo ok  # dos
    - shell: bash
      run: |
        # tres
        echo '# dato'
""",
             ["# uno", "# dos", "# tres"]),
        ]
        for path, source, expected in examples:
            with self.subTest(path=path):
                self.assertCountEqual([entry.text for entry in entries(Path(path), source)], expected)

    def test_ratchet_checks_committed_and_untracked_changes_per_occurrence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                subprocess.run(["git", *args], cwd=root, capture_output=True, check=True)

            def lint():
                return subprocess.run([sys.executable, SCRIPT, "--base", "base"], cwd=root,
                                      capture_output=True, text=True)

            git("init", "-q")
            code = root / "main.rs"
            code.write_text("fn main() { /* uno */ /* dos */ }")
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=t@example.test", "commit", "-qm", "base")
            git("tag", "base")
            code.write_text("fn main() { /* uno */ }")
            self.assertEqual(lint().returncode, 0)
            code.write_text("fn main() { /* uno */ /* uno */ }")
            git("-c", "user.name=Test", "-c", "user.email=t@example.test", "commit", "-qam", "duplicado")
            (root / "new.rs").write_text("// nuevo")
            result = lint()
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("main.rs:1:", result.stdout)
            self.assertIn("new.rs:1:", result.stdout)
