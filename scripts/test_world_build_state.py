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

    def test_unchanged_interrupted_chain_resumes_at_last_started_substep_only(self):
        roads = self.steps[1]
        receipt = dict(name='roads', exit=None, started=100.0, **state.step_identity(
            roads, self.config['build'], pin_digest(self.output / state.PIN_NAME)))
        state.record_steps(self.output, [receipt])
        log = self.output / 'roads.log'
        log.write_text('=== attempt 1970-01-01T00:01:40+00:00 ===\n'
                       '{"step":"roads-built-up","argv":["bake"]}\n'
                       '{"step":"roads-built-up","exit":0}\n'
                       '{"step":"roads-us","argv":["enrich"]}\n'
                       'progress interrupted without a final error message\n')
        before = (self.output / state.STATE_NAME).read_bytes()
        self.assertEqual(self.resume(dry_run=True), {'osm'})
        self.assertEqual((self.output / state.STATE_NAME).read_bytes(), before)
        self.assertEqual(self.steps[1].argv, roads.argv)
        self.assertEqual(self.resume(), {'osm'})
        self.assertEqual(self.steps[1].argv[-2:], ('--from', 'roads-us'))
        self.assertEqual(json.loads((self.output / state.STATE_NAME).read_text())['roads_from_step'], 'roads-us')
        # A later interruption moves forward rather than replaying earlier countries.
        receipt.update(started=200.0, **state.step_identity(self.steps[1], self.config['build'], pin_digest(self.output / state.PIN_NAME)))
        state.record_steps(self.output, [receipt])
        with log.open('a') as stream:
            stream.write('=== attempt 1970-01-01T00:03:20+00:00 ===\n'
                         '{"step":"roads-continuity","argv":["fill"]}\n')
        self.assertEqual(self.resume(), {'osm'})
        self.assertEqual(self.steps[1].argv[-2:], ('--from', 'roads-continuity'))
        self.assertEqual(self.steps[1].argv.count('--from'), 1)
        # A controller killed before opening its attempt cannot adopt an older log.
        receipt['started'] = 300.0
        self.assertIsNone(state.interrupted_chain_step(self.output, 'roads', receipt,
            roads, self.config['build'], pin_digest(self.output / state.PIN_NAME)))
        receipt['started'] = 200.0
        self.assertIsNone(state.interrupted_chain_step(self.output, 'roads', receipt,
            roads, self.config['build'], 'changed-code-pin'))

    def test_state_updates_remove_obsolete_note_and_preserve_rows(self):
        path = self.output / state.STATE_NAME
        counts = {'roads': 42, 'railways': 7}
        for status in ('running', 'failed', 'complete'):
            path.write_text(json.dumps({'remaining': 'World incomplete', 'rows': counts}))
            state.write_state(self.output, self.config, status)
            updated = json.loads(path.read_text())
            self.assertEqual(updated['status'], status)
            self.assertNotIn('remaining', updated)
            self.assertEqual(updated['rows'], counts)

    def test_source_change_refuses_every_retry_without_replacing_the_evidence(self):
        before = (self.output / state.PIN_NAME).read_bytes()
        self.source.write_text('changed source')
        for status in ('failed', 'complete'):
            state.write_state(self.output, self.config, status)
            original_state = (self.output / state.STATE_NAME).read_bytes()
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

    def test_complete_build_requires_reviewed_rebuild_and_preserves_unaffected_outputs(self):
        railways = self.step('railways', ('osm',))
        self.steps.extend([railways, self.step('railways-finalize', ('railways',)),
                           self.step('structures-finalize', ('osm',))])
        self.receipts([*self.steps[:2], self.step('railways', ('osm',),
                       argv=railways.argv + ('--from', 'railways-at')), *self.steps[3:]])
        state.write_state(self.output, self.config, 'complete', railways_from_step='railways-at')
        before = {name: (self.output / name).read_bytes()
                  for name in (state.STATE_NAME, state.STEPS_NAME, state.PIN_NAME)}
        self.assertEqual(self.resume(dry_run=True), {'osm', 'roads', 'railways', 'railways-finalize'})
        with self.assertRaisesRegex(ValueError, 'complete build requires'):
            self.resume()
        with self.assertRaisesRegex(ValueError, 'complete build requires'):
            self.resume(review=self.review(['osm', 'roads', 'railways', 'railways-finalize']))
        self.code.write_text('fixed rail routing; other producer output contracts unchanged')
        review = dict(self.review(['osm', 'roads']), railways_from_step='railways-cz')
        with self.assertRaisesRegex(ValueError, 'exact previous/current pins'):
            self.resume(review=dict(review, current_pin_sha256='stale review'))
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['osm', 'roads', 'railways']))
        with self.assertRaisesRegex(ValueError, 'retained upstream'):
            self.resume(review=dict(review, reuse=[]))
        self.assertEqual(self.resume(review=review, dry_run=True), {'osm', 'roads'})
        for name, content in before.items():
            self.assertEqual((self.output / name).read_bytes(), content)
        self.assertEqual(self.steps[2].argv, railways.argv)
        self.assertEqual(self.resume(review=review), {'osm', 'roads'})
        current = json.loads((self.output / state.STATE_NAME).read_text())
        self.assertEqual((current['status'], current['railways_from_step']), ('running', 'railways-cz'))
        self.assertEqual(current['resumes'][-1]['invalidated'], ['railways', 'railways-finalize'])
        self.assertEqual(self.steps[2].argv, railways.argv + ('--from', 'railways-cz'))
        receipts = state.latest_receipts(self.output / state.STEPS_NAME)
        for name in ('osm', 'roads'):
            self.assertEqual(receipts[name]['exit'], 0)
            self.assertEqual(receipts[name]['review'], review)
        for name in ('railways', 'railways-finalize'):
            self.assertIsNone(receipts[name]['exit'])

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
        for stage in ('stage2a', 'audit', '', None, 2):
            with self.subTest(stage=stage), self.assertRaisesRegex(ValueError, 'must be stage2b or stage2c'):
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

    def test_reviewed_cruise_only_replay_retains_other_aircraft_stages_and_resumes_its_window(self):
        aircraft = self.step('aircraft', ('osm',), environment=(('WORK_DIR', '/aircraft'),))
        self.steps.append(aircraft)
        previous = dict(name='aircraft', exit=0, **state.step_identity(
            self.step('aircraft', ('osm',), environment=(*aircraft.environment, ('FROM_STAGE', 'stage2c'))),
            self.config['build'], pin_digest(self.output / state.PIN_NAME)))
        state.record_steps(self.output, [previous])
        state.write_state(self.output, self.config, 'failed', aircraft_from_stage='stage2c')
        self.code.write_text('fixed cruise headings; ground and airborne contracts unchanged')
        review = dict(self.review(['osm', 'roads']), aircraft_from_stage='stage2b')
        for row in (dict(previous, exit=1), dict(previous, input_pin_sha256='another pin')):
            state.record_steps(self.output, [row])
            with self.assertRaisesRegex(ValueError, 'reviewed successful aircraft receipt'):
                self.resume(review=review)
        state.record_steps(self.output, [previous])
        before = {name: (self.output / name).read_bytes()
                  for name in (state.STATE_NAME, state.STEPS_NAME, state.PIN_NAME)}
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['osm', 'roads', 'aircraft']))
        self.assertEqual(self.resume(review=review, dry_run=True), {'osm', 'roads'})
        for name, content in before.items():
            self.assertEqual((self.output / name).read_bytes(), content)
        self.assertEqual(self.resume(review=review), {'osm', 'roads'})
        self.assertEqual(dict(self.steps[-1].environment),
                         {'WORK_DIR': '/aircraft', 'FROM_STAGE': 'stage2b', 'UNTIL_STAGE': 'stage2b'})
        self.assertIsNone(state.latest_receipts(self.output / state.STEPS_NAME)['aircraft']['exit'])
        self.steps[-1] = aircraft
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.assertEqual(dict(self.steps[-1].environment)['UNTIL_STAGE'], 'stage2b')
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

    def test_railway_resume_preserves_the_existing_road_boundary_and_requires_its_own_failed_receipt(self):
        self.steps[1] = self.step('roads', ('osm',), argv=('roads', '--from', 'roads-de'))
        railway = self.step('railways', ('osm',))
        self.steps.append(railway)
        state.write_state(self.output, self.config, 'failed', roads_from_step='roads-de')
        self.receipts(self.steps)
        review = dict(self.review(['osm', 'roads']), railways_from_step='railways-gtfs-us')
        with self.assertRaisesRegex(ValueError, 'unsuccessful railways attempt'):
            self.resume(review=review)
        receipt = dict(name='railways', exit=134, **state.step_identity(
            railway, self.config['build'], pin_digest(self.output / state.PIN_NAME)))
        state.record_steps(self.output, [dict(receipt, input_pin_sha256='another attempt')])
        with self.assertRaisesRegex(ValueError, 'unsuccessful railways attempt'):
            self.resume(review=review)
        state.record_steps(self.output, [receipt])
        self.code.write_text('fixed GTFS memory without changing completed railway countries')
        review = dict(self.review(['osm', 'roads']), railways_from_step='railways-gtfs-us')
        with self.assertRaisesRegex(ValueError, 'cannot adopt'):
            self.resume(review=dict(review, reuse=['osm', 'roads', 'railways']))
        with self.assertRaisesRegex(ValueError, 'retained upstream'):
            self.resume(review=dict(review, reuse=[]))
        self.assertEqual(self.resume(review=review, dry_run=True), {'osm', 'roads'})
        self.assertNotIn('--from', self.steps[-1].argv)
        self.assertEqual(self.resume(review=review), {'osm', 'roads'})
        persisted = json.loads((self.output / state.STATE_NAME).read_text())
        self.assertEqual((persisted['roads_from_step'], persisted['railways_from_step']),
                         ('roads-de', 'railways-gtfs-us'))
        self.steps[1] = self.step('roads', ('osm',))
        self.steps[-1] = railway
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.assertEqual(self.steps[1].argv[-2:], ('--from', 'roads-de'))
        self.assertEqual(self.steps[-1].argv[-2:], ('--from', 'railways-gtfs-us'))
        self.assertEqual(self.resume(), {'osm', 'roads'})
        self.assertEqual(self.steps[-1].argv.count('--from'), 1)

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
