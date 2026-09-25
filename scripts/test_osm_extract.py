"""The launcher must admit all spill streams under a low inherited soft limit."""

import os
from pathlib import Path
import resource
import subprocess
import sys
import tempfile
import unittest


class DescriptorBudgetTests(unittest.TestCase):
    def test_low_soft_limit_can_open_ten_streams_per_bucket(self):
        with tempfile.TemporaryDirectory() as work:
            cargo = Path(work) / "cargo"
            cargo.write_text(
                f"#!{sys.executable}\n"
                "import os, resource, sys\n"
                "files = [os.open(os.devnull, os.O_RDONLY) for _ in range(2560)]\n"
                "print('opened 2560 streams', resource.getrlimit(resource.RLIMIT_NOFILE)[0])\n"
                "sys.exit(73)\n"
            )
            cargo.chmod(0o755)
            run = subprocess.run(
                ["bash", str(Path(__file__).with_name("osm-extract.sh"))],
                env={**os.environ, "PATH": work + os.pathsep + os.environ["PATH"],
                     "PBF_FILE": "unused", "OUTPUT_DIR": work, "NUM_BUCKETS": "256"},
                preexec_fn=lambda: resource.setrlimit(resource.RLIMIT_NOFILE, (1024, 2624)),
                capture_output=True, text=True, check=False,
            )
            # The fake build exits before the launcher could start an extraction.
            self.assertEqual(run.returncode, 73, run.stdout + run.stderr)
            self.assertIn("opened 2560 streams 2624", run.stdout)


if __name__ == "__main__":
    unittest.main()
