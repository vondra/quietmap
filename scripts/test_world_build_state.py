"""Interrupted builds reuse completed work without discarding input-change evidence."""

import contextlib
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

from world_build_inputs import pin_inputs
import world_build_state as state


class WorldBuildStateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / 'repo'
        (self.repo / 'scripts').mkdir(parents=True)
        self.code = self.repo / 'scripts/writer.py'
        self.code.write_text('old code')
        self.source = self.root / 'planet.pbf'
        self.source.write_text('frozen source')
        self.output = self.root / 'output'
        self.output.mkdir()
        self.config = {'build': {'memory_gib': 80}}
        self.steps = [self.step('osm'), self.step('roads', ('osm',))]
        self.roots = [self.repo, self.source]
        state.write_state(self.output, self.config, 'failed')
        pin_inputs(self.output / state.PIN_NAME, self.roots)
        self.receipts(self.steps)

    @staticmethod
    def step(name, dependencies=(), argv=None):
        return SimpleNamespace(name=name, dependencies=dependencies, slots=1,
                               argv=argv or (name, '--jobs', '20'))

    def receipts(self, steps):
        rows = [{'name': step.name, 'command': state.producer_command(step, self.config['build']),
                 'exit': 0} for step in steps]
        (self.output / state.STEPS_NAME).write_text(''.join(json.dumps(row) + '\n' for row in rows))

    def resume(self):
        with contextlib.redirect_stdout(io.StringIO()), patch.object(
                state.subprocess, 'run', return_value=SimpleNamespace(returncode=3)):
            return state.resume_steps(self.output, self.config, self.steps, self.roots, self.repo)

    def test_source_change_refuses_every_retry_without_replacing_the_evidence(self):
        before = (self.output / state.PIN_NAME).read_bytes()
        original_state = (self.output / state.STATE_NAME).read_bytes()
        self.source.write_text('changed source')
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, 'frozen sources changed'):
                self.resume()
            self.assertEqual((self.output / state.PIN_NAME).read_bytes(), before)
            self.assertEqual((self.output / state.STATE_NAME).read_bytes(), original_state)

    def test_code_changes_and_deleted_code_are_recorded_while_completed_work_stays(self):
        self.code.unlink()
        (self.repo / 'scripts/fixed.py').write_text('fixed code')
        self.assertEqual(self.resume(), {'osm', 'roads'})
        state.write_state(self.output, self.config, 'complete')
        receipt = json.loads((self.output / state.STATE_NAME).read_text())
        self.assertEqual(receipt['resumes'][0]['code_changed'],
                         sorted([str(self.code), str(self.repo / 'scripts/fixed.py')]))
        with self.assertRaisesRegex(ValueError, 'complete build'):
            self.resume()

    def test_data_arguments_invalidate_dependents_but_scheduling_does_not(self):
        old = self.step('osm', argv=('osm',))
        self.receipts([old, self.steps[1]])
        self.assertEqual(state.completed_steps(self.output / state.STEPS_NAME, self.steps,
                                              self.config['build']), {'osm', 'roads'})
        self.steps[0] = self.step('osm', argv=('osm', '--different-data'))
        self.assertEqual(state.completed_steps(self.output / state.STEPS_NAME, self.steps,
                                              self.config['build']), set())

    def test_resource_change_keeps_completed_data_but_rechecks_derived_indexes(self):
        self.steps.append(self.step('structures-finalize', ('osm',)))
        self.receipts(self.steps)
        self.config['build'].update(threads=8, memory_gib=40)
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.config['build']['as_of_date'] = '20260911'
        with self.assertRaisesRegex(ValueError, 'another configuration'):
            self.resume()

    def test_interrupted_receipt_publish_preserves_completed_work(self):
        path = self.output / state.STEPS_NAME
        previous = path.read_bytes()
        with patch.object(state.os, 'replace', side_effect=OSError('interrupted publish')):
            with self.assertRaisesRegex(OSError, 'interrupted publish'):
                state.record_step(self.output, {'name': 'aircraft', 'exit': 0})
        self.assertEqual(path.read_bytes(), previous)
        self.assertEqual(self.resume(), {'osm', 'roads'})
        state.record_step(self.output, {'name': 'aircraft', 'exit': 0})
        self.assertEqual(json.loads(path.read_text().splitlines()[-1])['name'], 'aircraft')

    def test_live_producer_and_missing_pin_refuse_reuse(self):
        with patch.object(state.subprocess, 'run', return_value=SimpleNamespace(returncode=0)):
            with self.assertRaisesRegex(ValueError, 'producers are alive'):
                state.resume_steps(self.output, self.config, self.steps, self.roots, self.repo)
        (self.output / state.PIN_NAME).unlink()
        with self.assertRaisesRegex(ValueError, 'previous input pin'):
            self.resume()


if __name__ == '__main__':
    unittest.main()
