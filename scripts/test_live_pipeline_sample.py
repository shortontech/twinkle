import tempfile
import json
import subprocess
import sys
import unittest
import shutil
from unittest import mock
from pathlib import Path
from live_pipeline_sample import snapshot, summarize, system_cpu_times


class SamplerTests(unittest.TestCase):
    def process(self, root, pid, parent, pss, readable=True):
        entry = root / str(pid)
        entry.mkdir()
        fields = ['0'] * 22
        fields[0] = 'S'
        fields[1] = str(parent)
        fields[11] = '12'
        fields[12] = '3'
        fields[19] = str(pid * 10)
        (entry / 'stat').write_text(f'{pid} (name (with) parentheses) ' + ' '.join(fields))
        (entry / 'exe').symlink_to('/usr/bin/test')
        (entry / 'cmdline').write_bytes(b'test\0--flag\0')
        if readable:
            (entry / 'smaps_rollup').write_text(f'Rss: {pss * 2} kB\nPss: {pss} kB\n')

    def test_tree_includes_gpu_grandchildren_and_excludes_other_apps(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for pid, parent in [(10, 1), (11, 10), (12, 11), (99, 1)]:
                self.process(root, pid, parent, 100)
            members, missing = snapshot(10, root)
            self.assertEqual([p['pid'] for p in members], [10, 11, 12])
            self.assertEqual(missing, [])
            self.assertEqual(sum(p['pss_kib'] for p in members), 300)
            self.assertEqual(members[0]['ticks'], 15)
            self.assertEqual(members[0]['command'], 'test --flag')

    def test_unreadable_member_is_reported_not_counted_as_zero(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.process(root, 10, 1, 100)
            self.process(root, 11, 10, 100, readable=False)
            members, missing = snapshot(10, root)
            self.assertEqual(missing, [11])
            self.assertEqual(len(members), 1)

    def test_confirmed_child_exit_is_distinct_from_unreadable_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.process(root, 10, 1, 100)
            self.process(root, 11, 10, 100)
            original = Path.read_text
            def read(path, *args, **kwargs):
                if path == root / '11' / 'smaps_rollup':
                    shutil.rmtree(path.parent)
                    raise FileNotFoundError(path)
                return original(path, *args, **kwargs)
            with mock.patch.object(Path, 'read_text', read):
                members, missing = snapshot(10, root)
            self.assertEqual(missing, [])
            child = next(row for row in members if row['pid'] == 11)
            self.assertEqual(child['memory_status'], 'confirmed_exited_during_snapshot')
            self.assertEqual((child['rss_kib'], child['pss_kib']), (0, 0))
            self.assertEqual(child['ticks'], 15)

    def test_system_cpu_excludes_duplicate_guest_ticks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'stat').write_text('cpu 100 20 30 200 10 5 6 7 50 10\n')
            self.assertEqual(system_cpu_times(root), (378, 168))

    def test_early_exit_cannot_publish_a_complete_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'run'
            result = subprocess.run([sys.executable, str(Path(__file__).with_name('live_pipeline_sample.py')),
                '--output', str(output), '--label', 'early-exit-test', '--warmup', '0', '--seconds', '2',
                '--', sys.executable, '-c', 'import time; time.sleep(.1)'], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(json.loads((output / 'summary.json').read_text())['status'], 'incomplete')

    def test_distributions_exclude_incomplete_memory_samples(self):
        rows = [dict(rss_mib=v, pss_mib=v, cpu_percent_one_core=10) for v in [None, 2, 4]]
        result = summarize(rows)
        self.assertEqual(result['pss_mib']['samples'], 2)
        self.assertEqual(result['pss_mib']['p50'], 3)
        self.assertEqual(result['pss_mib']['p95'], 3.9)
        self.assertEqual(result['cpu_percent_one_core']['samples'], 3)


if __name__ == '__main__':
    unittest.main()
