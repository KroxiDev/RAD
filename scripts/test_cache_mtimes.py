"""La caché solo recupera fechas para las mismas fuentes, sin seguir enlaces."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('cache_mtimes.py')
SNAPSHOT = Path('target/release/.source-mtimes.json')


class CacheMtimesTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='rad-source-cache-')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        (self.root / 'src/locales').mkdir(parents=True)
        (self.root / 'src/main.rs').write_text('fn main() {}\n')
        (self.root / 'src/locales/en.json').write_text('{}\n')
        (self.root / '.gitignore').write_text('target/\n')
        subprocess.run(['git', 'init', '-q'], cwd=self.root, check=True)
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        self.old = 1_600_000_000_123_456_789
        self.new = 1_700_000_000_987_654_321
        for path in [self.root / 'src/main.rs', self.root / 'src/locales/en.json', self.root / 'src/locales', self.root / 'src']:
            os.utime(path, ns=(self.old, self.old))

    def run_cache(self, operation):
        return subprocess.run([sys.executable, '-B', str(SCRIPT), operation], cwd=self.root,
                              check=True, capture_output=True, text=True)

    def checkout_dates(self):
        for path in [self.root / 'src/main.rs', self.root / 'src/locales/en.json', self.root / 'src/locales', self.root / 'src']:
            os.utime(path, ns=(self.new, self.new))

    def test_identical_tracked_files_and_directories_reuse_dates_and_consume_snapshot(self):
        self.run_cache('record')
        self.checkout_dates()
        self.run_cache('restore')
        for name in ('src/main.rs', 'src/locales/en.json', 'src/locales', 'src'):
            self.assertEqual((self.root / name).stat().st_mtime_ns, self.old, name)
        self.assertFalse((self.root / SNAPSHOT).exists(), 'un build posterior fallido no conserva una estampa antigua')

    def test_changed_and_new_sources_keep_checkout_dates(self):
        self.run_cache('record')
        self.checkout_dates()
        changed = self.root / 'src/main.rs'
        changed.write_text('fn main() { println!("changed"); }\n')
        new = self.root / 'src/new.rs'
        new.write_text('pub fn added() {}\n')
        subprocess.run(['git', 'add', 'src/new.rs'], cwd=self.root, check=True)
        for path in (changed, new, self.root / 'src'):
            os.utime(path, ns=(self.new, self.new))
        self.run_cache('restore')
        for path in (changed, new, self.root / 'src'):
            self.assertEqual(path.stat().st_mtime_ns, self.new)
        self.assertEqual((self.root / 'src/locales/en.json').stat().st_mtime_ns, self.old)

    def test_new_untracked_directory_entry_prevents_directory_restoration(self):
        self.run_cache('record')
        self.checkout_dates()
        (self.root / 'src/locales/es.json').write_text('{}\n')
        os.utime(self.root / 'src/locales', ns=(self.new, self.new))
        self.run_cache('restore')
        self.assertEqual((self.root / 'src/locales').stat().st_mtime_ns, self.new)
        self.assertEqual((self.root / 'src').stat().st_mtime_ns, self.new)

    def test_removed_source_prevents_directory_restoration(self):
        self.run_cache('record')
        self.checkout_dates()
        (self.root / 'src/locales/en.json').unlink()
        os.utime(self.root / 'src/locales', ns=(self.new, self.new))
        self.run_cache('restore')
        self.assertEqual((self.root / 'src/locales').stat().st_mtime_ns, self.new)
        self.assertFalse((self.root / 'src/locales/en.json').exists())

    def test_symlink_file_or_parent_never_touches_outside_sources(self):
        self.run_cache('record')
        self.checkout_dates()
        outside = self.root / 'outside'
        outside.mkdir()
        source = outside / 'en.json'
        source.write_text('{}\n')
        os.utime(source, ns=(self.new, self.new))
        (self.root / 'src/main.rs').unlink()
        (self.root / 'src/main.rs').symlink_to(source)
        (self.root / 'src/locales/en.json').unlink()
        (self.root / 'src/locales').rmdir()
        (self.root / 'src/locales').symlink_to(outside, target_is_directory=True)
        self.run_cache('restore')
        self.assertEqual(source.stat().st_mtime_ns, self.new)
        self.assertTrue((self.root / 'src/main.rs').is_symlink())
        self.assertTrue((self.root / 'src/locales').is_symlink())

    def test_snapshot_cannot_target_untracked_or_outside_paths(self):
        self.run_cache('record')
        path = self.root / SNAPSHOT
        snapshot = json.loads(path.read_text())
        outsider = Path(self.scratch.name + '-outside')
        outsider.write_text('fn main() {}\n')
        self.addCleanup(outsider.unlink)
        os.utime(outsider, ns=(self.new, self.new))
        previous = snapshot['files']['src/main.rs']
        snapshot['files'][str(outsider)] = previous
        snapshot['files']['../' + outsider.name] = previous
        path.write_text(json.dumps(snapshot))
        self.run_cache('restore')
        self.assertEqual(outsider.stat().st_mtime_ns, self.new)


if __name__ == '__main__':
    unittest.main()
