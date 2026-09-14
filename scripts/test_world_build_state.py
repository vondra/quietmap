"""Interrupted builds reuse completed work without discarding input-change evidence."""

from dataclasses import dataclass
import contextlib
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

from world_build_inputs import pin_digest, pin_inputs
import world_build_state as state


@dataclass(frozen=True)
class FixtureStep:
    name: str
    dependencies: tuple
    slots: int
    argv: tuple
    environment: tuple


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
    def step(name, dependencies=(), argv=None, environment=()):
        return FixtureStep(name=name, dependencies=dependencies, slots=1,
                               argv=argv or (name, '--jobs', '20'), environment=environment)

    def receipts(self, steps):
        rows = [dict(name=step.name, exit=0, **state.step_identity(
            step, self.config['build'], pin_digest(self.output / state.PIN_NAME))) for step in steps]
        (self.output / state.STEPS_NAME).write_text(''.join(json.dumps(row) + '\n' for row in rows))

    def resume(self, **options):
        with contextlib.redirect_stdout(io.StringIO()), patch.object(
                state.subprocess, 'run', return_value=SimpleNamespace(returncode=3)):
            return state.resume_steps(self.output, self.config, self.steps, self.roots, [self.source], **options)

    def test_source_change_refuses_every_retry_without_replacing_the_evidence(self):
        before = (self.output / state.PIN_NAME).read_bytes()
        original_state = (self.output / state.STATE_NAME).read_bytes()
        self.source.write_text('changed source')
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, 'frozen sources changed'):
                self.resume()
            self.assertEqual((self.output / state.PIN_NAME).read_bytes(), before)
            self.assertEqual((self.output / state.STATE_NAME).read_bytes(), original_state)

    def review(self, reuse):
        report = io.StringIO()
        with contextlib.redirect_stdout(report), patch.object(
                state.subprocess, 'run', return_value=SimpleNamespace(returncode=3)):
            state.resume_steps(self.output, self.config, self.steps, self.roots, [self.source], dry_run=True)
        return dict(json.loads(report.getvalue().splitlines()[-1])['review'],
                    reuse=reuse, reason='Reviewed producer changes and retained output contracts')

    def test_code_change_plan_preserves_evidence_and_requires_explicit_reuse(self):
        self.code.unlink()
        (self.repo / 'scripts/fixed.py').write_text('fixed code')
        before = {name: (self.output / name).read_bytes()
                  for name in (state.STATE_NAME, state.STEPS_NAME, state.PIN_NAME)}
        review = self.review(['osm'])
        for name, content in before.items():
            self.assertEqual((self.output / name).read_bytes(), content)
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, 'completed outputs need review'):
                self.resume()
        self.assertEqual(self.resume(review=review), {'osm'})
        self.assertEqual(self.resume(), {'osm'})
        receipts = state.latest_receipts(self.output / state.STEPS_NAME)
        self.assertEqual(receipts['osm']['review'], review)
        self.assertIsNone(receipts['roads']['exit'])
        receipt = json.loads((self.output / state.STATE_NAME).read_text())
        self.assertEqual(receipt['resumes'][0]['producer_inputs_changed'],
                         sorted([str(self.code), str(self.repo / 'scripts/fixed.py')]))
        state.write_state(self.output, self.config, 'complete')
        with self.assertRaisesRegex(ValueError, 'complete build'):
            self.resume()

    def test_runtime_upgrade_needs_exact_review_and_invalidates_completed_steps(self):
        runtime = self.root / 'node'
        runtime.write_text('old runtime')
        self.roots.append(runtime)
        pin_inputs(self.output / state.PIN_NAME, self.roots)
        self.receipts(self.steps)
        runtime.chmod(0o755)
        replacement = self.root / 'new-node'
        replacement.write_text('new runtime')
        self.roots.append(replacement)
        with self.assertRaisesRegex(ValueError, 'completed outputs need review'):
            self.resume()
        review = self.review([])
        self.assertEqual(self.resume(review=review), set())
        self.source.write_text('changed source')
        with self.assertRaisesRegex(ValueError, 'frozen sources changed'):
            self.resume()

    def test_legacy_successes_need_review_and_failed_attempts_cannot_be_adopted(self):
        path = self.output / state.STEPS_NAME
        rows = state.latest_receipts(path)
        for row in rows.values():
            del row['environment'], row['input_pin_sha256']
        path.write_text(''.join(json.dumps(row) + '\n' for row in rows.values()))
        with self.assertRaisesRegex(ValueError, 'completed outputs need review'):
            self.resume()
        self.assertEqual(self.resume(review=self.review(['osm', 'roads'])), {'osm', 'roads'})
        state.record_steps(self.output, [dict(rows['osm'], exit=None)])
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=self.review(['osm', 'roads']))

    def test_environment_change_invalidates_dependents_but_worker_and_storage_placement_do_not(self):
        original = {'PBF_FILE': '/planet', 'OUTPUT_DIR': '/prepared', 'MAX_THREADS': '20',
                    'SCRATCH_ROOT': '/old', 'NODE_CACHE': '/old/nodes', 'SPILL_DIR': '/old/spill'}
        self.steps[0] = self.step('osm', environment=tuple(original.items()))
        self.receipts(self.steps)
        moved = {key: value for key, value in original.items() if key != 'SCRATCH_ROOT'}
        moved.update(MAX_THREADS='8', NODE_CACHE='/new/nodes', SPILL_DIR='/other/spill')
        self.steps[0] = self.step('osm', environment=tuple(moved.items()))
        self.config['build'].update(osm_node_cache='/new/nodes', osm_spill_dir='/other/spill')
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.code.write_text('reviewed equivalent producer with different scratch placement')
        with self.assertRaisesRegex(ValueError, 'completed outputs need review'):
            self.resume()
        self.assertEqual(self.resume(review=self.review(['osm', 'roads'])), {'osm', 'roads'})
        for key in ('PBF_FILE', 'OUTPUT_DIR'):
            self.steps[0] = self.step('osm', environment=tuple(dict(moved, **{key: '/another'}).items()))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'cannot adopt'):
                self.resume(review=self.review(['osm', 'roads']))
        self.assertEqual(self.resume(review=self.review([])), set())
        # Once a producer is restarted, restoring its arguments cannot revive old consumers.
        self.steps[0] = self.step('osm', environment=tuple(moved.items()))
        state.record_steps(self.output, [dict(name='osm', exit=0, **state.step_identity(
            self.steps[0], self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.assertEqual(self.resume(), {'osm'})

    def test_review_is_bound_to_both_pins_and_rejects_missing_dependencies(self):
        self.code.write_text('reviewed code')
        review = self.review(['osm', 'roads'])
        self.code.write_text('unreviewed code')
        with self.assertRaisesRegex(ValueError, 'exact previous/current pins'):
            self.resume(review=review)
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=self.review(['roads']))
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=self.review(['unknown']))
        self.assertEqual(self.resume(review=self.review(['osm', 'roads'])), {'osm', 'roads'})

    def test_interruption_after_receipt_decisions_cannot_resurrect_invalidated_outputs(self):
        self.code.write_text('changed writer')
        review = self.review(['osm'])
        replace = state.os.replace
        def interrupt_pin(source, target):
            if Path(source).name == '.' + state.PIN_NAME + '.next':
                raise OSError('interrupted pin publish')
            return replace(source, target)
        with patch.object(state.os, 'replace', side_effect=interrupt_pin):
            with self.assertRaisesRegex(OSError, 'interrupted pin publish'):
                self.resume(review=review)
        self.assertEqual(self.resume(), {'osm'})

    def test_transport_scope_survives_interruption_and_keeps_dependency_invalidation(self):
        review = dict(self.review([]), osm_scope=['roads', 'railways'])
        before = (self.output / state.STATE_NAME).read_bytes()
        self.assertEqual(self.resume(review=review, dry_run=True), set())
        self.assertEqual((self.output / state.STATE_NAME).read_bytes(), before)
        self.assertNotIn('QM_OSM_ONLY', dict(self.steps[0].environment))
        with self.assertRaisesRegex(ValueError, 'exactly'):
            self.resume(review=dict(review, osm_scope=['roads']))
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['roads']))
        rows = state.latest_receipts(self.output / state.STEPS_NAME)
        del rows['osm']['environment']
        (self.output / state.STEPS_NAME).write_text(''.join(json.dumps(row) + '\n' for row in rows.values()))
        with self.assertRaisesRegex(ValueError, 'cannot adopt full osm'):
            self.resume(review=dict(review, reuse=['osm']))
        with patch.object(state, 'record_steps', side_effect=OSError('receipt interruption')):
            with self.assertRaisesRegex(OSError, 'receipt interruption'):
                self.resume(review=review)
        persisted = json.loads((self.output / state.STATE_NAME).read_text())
        self.assertEqual(persisted['osm_scope'], ['roads', 'railways'])
        self.assertEqual(self.resume(review=review), set())
        self.assertEqual(dict(self.steps[0].environment)['QM_OSM_ONLY'], 'roads,railways')
        self.steps[0] = self.step('osm')
        self.assertEqual(self.resume(), set())
        self.assertEqual(dict(self.steps[0].environment)['QM_OSM_ONLY'], 'roads,railways')
        state.record_steps(self.output, [dict(name='osm', exit=0, **state.step_identity(
            self.steps[0], self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.assertEqual(self.resume(), {'osm'})
        with self.assertRaisesRegex(ValueError, 'persisted'):
            self.resume(review=dict(self.review([]), osm_scope=[]))
        persisted.pop('osm_scope')
        (self.output / state.STATE_NAME).write_text(json.dumps(persisted))
        for exit_code in (None, 1, 0):
            state.record_steps(self.output, [dict(name='osm', exit=exit_code, environment={
                'QM_OSM_ONLY': 'roads,railways'})])
            with self.assertRaisesRegex(ValueError, 'successful full osm'):
                self.resume(review=review)

    def test_reviewed_aircraft_stage_preserves_successes_and_survives_interrupted_pin_publication(self):
        aircraft = self.step('aircraft', ('osm',), environment=(('WORK_DIR', '/aircraft'),))
        self.steps.append(aircraft)
        with self.assertRaisesRegex(ValueError, 'unsuccessful aircraft attempt'):
            self.resume(review=dict(self.review(['osm', 'roads']), aircraft_from_stage='stage2c'))
        state.record_steps(self.output, [dict(name='aircraft', exit=1, **state.step_identity(
            aircraft, self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.code.write_text('fixed Stage2C admission without changing completed A/B')
        review = dict(self.review(['osm', 'roads']), aircraft_from_stage='stage2c')
        before = {name: (self.output / name).read_bytes()
                  for name in (state.STATE_NAME, state.STEPS_NAME, state.PIN_NAME)}
        self.assertEqual(self.resume(review=review, dry_run=True), {'osm', 'roads'})
        self.assertNotIn('FROM_STAGE', dict(self.steps[-1].environment))
        for name, content in before.items():
            self.assertEqual((self.output / name).read_bytes(), content)
        for stage in ('stage2b', 'audit', '', None, 2):
            with self.subTest(stage=stage), self.assertRaisesRegex(ValueError, 'must be stage2c'):
                self.resume(review=dict(review, aircraft_from_stage=stage))
        with self.assertRaisesRegex(ValueError, 'retained upstream'):
            self.resume(review=dict(review, reuse=[]))
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['osm', 'roads', 'aircraft']))
        replace = state.os.replace
        def interrupt_pin(source, target):
            if Path(source).name == '.' + state.PIN_NAME + '.next':
                raise OSError('interrupted pin publish')
            return replace(source, target)
        with patch.object(state.os, 'replace', side_effect=interrupt_pin):
            with self.assertRaisesRegex(OSError, 'interrupted pin publish'):
                self.resume(review=review)
        self.assertEqual(json.loads((self.output / state.STATE_NAME).read_text())['aircraft_from_stage'], 'stage2c')
        self.assertEqual(self.resume(review=review), {'osm', 'roads'})
        self.assertEqual(dict(self.steps[-1].environment)['FROM_STAGE'], 'stage2c')
        self.steps[-1] = aircraft
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.assertEqual(dict(self.steps[-1].environment)['FROM_STAGE'], 'stage2c')
        state.record_steps(self.output, [dict(name='aircraft', exit=0, **state.step_identity(
            self.steps[-1], self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.assertEqual(self.resume(), {'osm', 'roads', 'aircraft'})
        self.source.write_text('changed source')
        with self.assertRaisesRegex(ValueError, 'frozen sources changed'):
            self.resume()

    def test_reviewed_road_boundary_uses_existing_chain_arguments_and_never_adopts_partial_success(self):
        roads = self.steps[1]
        with self.assertRaisesRegex(ValueError, 'unsuccessful roads attempt'):
            self.resume(review=dict(self.review(['osm']), roads_from_step='roads-de'))
        state.record_steps(self.output, [dict(name='roads', exit=None, **state.step_identity(
            roads, self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.code.write_text('reviewed code with unchanged completed road countries')
        review = dict(self.review(['osm']), roads_from_step='roads-de')
        for boundary in ('', ' ', None, 2, ['roads-de']):
            with self.subTest(boundary=boundary), self.assertRaisesRegex(ValueError, 'nonempty chain step'):
                self.resume(review=dict(review, roads_from_step=boundary))
        with self.assertRaisesRegex(ValueError, 'retained upstream'):
            self.resume(review=dict(review, reuse=[]))
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['osm', 'roads']))
        self.assertEqual(self.resume(review=review), {'osm'})
        self.assertEqual(self.steps[1].argv, roads.argv + ('--from', 'roads-de'))
        self.assertEqual(json.loads((self.output / state.STATE_NAME).read_text())['roads_from_step'], 'roads-de')
        self.steps[1] = roads
        self.assertEqual(self.resume(), {'osm'})
        self.assertEqual(self.steps[1].argv, roads.argv + ('--from', 'roads-de'))
        self.assertEqual(self.resume(), {'osm'})
        self.assertEqual(self.steps[1].argv.count('--from'), 1)
        with self.assertRaisesRegex(ValueError, 'unsuccessful roads attempt'):
            self.resume(review=dict(self.review([]), roads_from_step='roads-fr'))
        state.record_steps(self.output, [dict(name='roads', exit=0, **state.step_identity(
            self.steps[1], self.config['build'], pin_digest(self.output / state.PIN_NAME)))])
        self.assertEqual(self.resume(), {'osm', 'roads'})

    def test_data_arguments_invalidate_dependents_but_scheduling_does_not(self):
        old = self.step('osm', argv=('osm',))
        self.receipts([old, self.steps[1]])
        self.assertEqual(state.completed_steps(state.latest_receipts(self.output / state.STEPS_NAME), self.steps,
                                              self.config['build'], pin_digest(self.output / state.PIN_NAME)), {'osm', 'roads'})
        self.steps[0] = self.step('osm', argv=('osm', '--different-data'))
        self.assertEqual(state.completed_steps(state.latest_receipts(self.output / state.STEPS_NAME), self.steps,
                                              self.config['build'], pin_digest(self.output / state.PIN_NAME)), set())

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
                state.record_steps(self.output, [{'name': 'aircraft', 'exit': 0}])
        self.assertEqual(path.read_bytes(), previous)
        self.assertEqual(self.resume(), {'osm', 'roads'})
        state.record_steps(self.output, [{'name': 'aircraft', 'exit': 0}])
        self.assertEqual(json.loads(path.read_text().splitlines()[-1])['name'], 'aircraft')

    def test_live_producer_and_missing_pin_refuse_reuse(self):
        with patch.object(state.subprocess, 'run', return_value=SimpleNamespace(returncode=0)):
            with self.assertRaisesRegex(ValueError, 'producers are alive'):
                state.resume_steps(self.output, self.config, self.steps, self.roots, [self.source])
        (self.output / state.PIN_NAME).unlink()
        with self.assertRaisesRegex(ValueError, 'previous input pin'):
            self.resume()


if __name__ == '__main__':
    unittest.main()
