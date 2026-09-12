"""Worker count is an upper bound: memory must still fit."""

import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / 'lib'))
from worker_jobs import fit_jobs


class WorkerJobsTests(unittest.TestCase):
    def test_memory_caps_requested_workers_and_keeps_a_lower_manual_cap(self):
        worker = 4 << 30
        self.assertEqual(fit_jobs(20, worker, memory_bytes=60 << 30), 15)
        self.assertEqual(fit_jobs(8, worker, memory_bytes=60 << 30), 8)
        self.assertEqual(fit_jobs(20, worker, memory_bytes=3 << 30), 1)

    def test_rejects_a_non_positive_request(self):
        with self.assertRaisesRegex(ValueError, '--jobs must be >= 1'):
            fit_jobs(0, 4 << 30, memory_bytes=60 << 30)


if __name__ == '__main__':
    unittest.main()
