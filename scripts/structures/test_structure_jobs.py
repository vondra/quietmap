"""World structures builder accepts only a positive worker count."""

import sys
import tempfile
import unittest
from unittest.mock import patch

from test_structures_fixtures import BUILDER


class StructureJobsTests(unittest.TestCase):
    def test_jobs_must_be_at_least_one_when_set(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(sys, 'argv', [
                'build-structures.py',
                '--prepared-dir', directory,
                '--overture-parquet', directory,
                '--ghsl', directory,
                '--jobs', '0',
            ]):
                with self.assertRaisesRegex(ValueError, '--jobs must be >= 1'):
                    BUILDER.main()


if __name__ == '__main__':
    unittest.main()
