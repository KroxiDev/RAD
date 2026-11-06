#!/usr/bin/env python3
"""Conserva mtimes de fuentes idénticas junto con la caché de compilación."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import tempfile

SNAPSHOT = Path('target/release/.source-mtimes.json')


def safe_path(root, name):
    """Acepta únicamente rutas relativas existentes sin enlaces en ningún componente."""
    relative = PurePosixPath(name)
    if relative.is_absolute() or not relative.parts or any(part in ('.', '..') for part in relative.parts):
        return None
    path = root
    for part in relative.parts:
        path = path / part
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            return None
        if stat.S_ISLNK(mode):
            return None
    return path


def inspect(root):
    names = subprocess.check_output(['git', 'ls-files', '--cached', '-z'], cwd=root).split(b'\0')
    files, directories, parents = {}, {}, set()
    for raw in names:
        if not raw:
            continue
        name = os.fsdecode(raw)
        path = safe_path(root, name)
        if path is None or not path.is_file():
            continue
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as source:
            before = os.fstat(source.fileno())
            digest = hashlib.file_digest(source, 'sha256').hexdigest()
            after = os.fstat(source.fileno())
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise ValueError(f'Fuente modificada durante lectura: {name}')
        files[name] = {'sha256': digest, 'mode': stat.S_IMODE(after.st_mode), 'mtime_ns': after.st_mtime_ns}
        parents.update(str(parent) for parent in PurePosixPath(name).parents if str(parent) != '.')
    # Un directorio solo es reutilizable cuando TODO su inventario es conocido.
    # Las entradas nuevas/no rastreadas y los enlaces lo excluyen junto a sus padres.
    for name in sorted(parents, key=lambda item: (-len(PurePosixPath(item).parts), item)):
        path = safe_path(root, name)
        if path is None or not path.is_dir():
            continue
        children = []
        for child in path.iterdir():
            relative = child.relative_to(root).as_posix()
            kind = 'file' if relative in files else 'directory'
            record = files.get(relative) or directories.get(relative)
            if record is None or child.is_symlink():
                break
            children.append((child.name, kind, record['sha256'], record['mode']))
        else:
            metadata = path.stat()
            digest = hashlib.sha256(json.dumps(sorted(children), ensure_ascii=True).encode()).hexdigest()
            directories[name] = {'sha256': digest, 'mode': stat.S_IMODE(metadata.st_mode), 'mtime_ns': metadata.st_mtime_ns}
    return {'version': 1, 'files': files, 'directories': directories}


def record(root):
    snapshot = inspect(root)
    destination = root / SNAPSHOT
    for relative in ('target', 'target/release'):
        path = root / relative
        path.mkdir(exist_ok=True)
        if safe_path(root, relative) is None or not path.is_dir():
            raise ValueError('Directorio de caché inseguro')
    with tempfile.NamedTemporaryFile(mode='w', dir=destination.parent, prefix='.source-mtimes-', delete=False) as output:
        temporary = Path(output.name)
        json.dump(snapshot, output, sort_keys=True)
        output.write('\n')
    try:
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"Fechas registradas: {len(snapshot['files'])} archivos, {len(snapshot['directories'])} directorios")


def restore(root):
    path = safe_path(root, SNAPSHOT.as_posix())
    if path is None or not path.is_file():
        print('Sin estampa de fuentes reutilizable; Cargo decidirá qué compilar.')
        return
    try:
        snapshot = json.loads(path.read_text())
    finally:
        # Una corrida fallida puede guardar artefactos parciales, nunca esta estampa vieja.
        path.unlink()
    if snapshot.get('version') != 1:
        raise ValueError('Versión de estampa desconocida')
    current = inspect(root)
    restored = 0
    for kind in ('files', 'directories'):
        for name, metadata in current[kind].items():
            previous = snapshot.get(kind, {}).get(name, {})
            mtime = previous.get('mtime_ns')
            if (previous.get('sha256') != metadata['sha256'] or previous.get('mode') != metadata['mode']
                    or type(mtime) is not int or mtime < 0):
                continue
            path = safe_path(root, name)
            if path is None:
                continue
            state = path.stat()
            if state.st_mtime_ns != metadata['mtime_ns']:
                raise ValueError(f'Fuente modificada durante restauración: {name}')
            os.utime(path, ns=(state.st_atime_ns, mtime), follow_symlinks=False)
            restored += 1
    print(f'Fechas recuperadas por contenido idéntico: {restored}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('record', 'restore'))
    args = parser.parse_args()
    root = Path(subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], text=True).strip())
    (record if args.operation == 'record' else restore)(root)


if __name__ == '__main__':
    main()
