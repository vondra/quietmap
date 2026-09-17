"""World publication must respect data dependencies, memory admission and immutable inputs."""

import contextlib
import io
import json
import os
from unittest.mock import patch
import importlib.util
from pathlib import Path
import sys
import subprocess
import tempfile
import threading
import time
import unittest

from world_build_inputs import input_files, load_pin, pin_inputs, verify_inputs

spec = importlib.util.spec_from_file_location('world_build', Path(__file__).with_name('build-world.py'))
world = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = world
spec.loader.exec_module(world)


class WorldBuildTest(unittest.TestCase):
    def test_resume_plan_never_starts_producers_or_rewrites_build_state(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            root = Path(directory)
            output, scratch = root / 'output', root / 'scratch'
            output.mkdir()
            scratch.mkdir()
            source = root / 'planet.pbf'
            source.write_text('frozen')
            config = root / 'build.toml'
            config.write_text('[build]\nas_of_date="20260909"\naircraft_anchor="2026-09"\n'
                              'memory_gib=80\nthreads=4\n[sources]\n')
            state_path = output / world.STATE_NAME
            state_path.write_text('retained build state')
            with patch.object(sys, 'argv', ['build-world.py', '--config', str(config),
                         '--output', str(output), '--scratch', str(scratch), '--resume-plan']), \
                    patch.object(world, 'source_paths', return_value={'planet': source}), \
                    patch.object(world, 'build_plan', return_value=(output / 'prepared/2026', [
                        world.Step('osm', (), (), environment=(('NODE_CACHE', str(scratch / 'nodes')),
                                                               ('SPILL_DIR', str(output / 'spill'))))])), \
                    patch.object(world, 'code_inputs', return_value=[]), \
                    patch.object(world, 'runtime_inputs', return_value=[]), \
                    patch.object(world, 'raster_inputs', return_value=[]), \
                    patch.object(world, 'height_inputs', return_value=[]), \
                    patch.object(world, 'resume_steps', return_value=set()) as resume, \
                    patch.object(world, 'preflight_aircraft_sources') as preflight, \
                    patch.object(world, 'attach_rasters') as attach, \
                    patch.object(world, 'pin_digest') as digest, \
                    patch.object(world.subprocess, 'run') as run, \
                    patch.dict(os.environ, {}, clear=True):
                # main changes cwd for producers; restore it even though this plan executes none.
                previous_cwd = Path.cwd()
                try:
                    # The source accessors are stubbed; source keys still describe the real CLI contract.
                    world.source_paths.return_value.update(rasters=source, ghsl=source, regional_heights=source)
                    world.main()
                finally:
                    os.chdir(previous_cwd)
                self.assertTrue(resume.call_args.kwargs['dry_run'])
                preflight.assert_not_called()
                run.assert_not_called()
                attach.assert_not_called()
                digest.assert_not_called()
            self.assertEqual(state_path.read_text(), 'retained build state')

    def test_fresh_build_rejects_a_missing_newest_sample_before_pinning_osm_or_any_producer(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            root = Path(directory)
            output, scratch = root / 'output', root / 'scratch'
            source = root / 'planet.pbf'
            source.write_text('frozen')
            airline, ga = root / 'adsbexchange', root / 'adsblol'
            airline.mkdir()
            ga.mkdir()
            config = root / 'build.toml'
            config.write_text('[build]\nas_of_date="20260910"\naircraft_anchor="2026-09"\n'
                              'memory_gib=80\nthreads=4\n[sources]\n')
            argv = ['build-world.py', '--config', str(config), '--output', str(output),
                    '--scratch', str(scratch)]
            with patch.object(sys, 'argv', argv), \
                    patch.object(world, 'source_paths', return_value={'planet': source}) as paths, \
                    patch.object(world, 'build_plan', return_value=(output / 'prepared/2026', [
                        world.Step('osm', (), ('osm-extract',))])), \
                    patch.object(world, 'preflight_aircraft_sources', side_effect=ValueError(
                        'airline source window incomplete: missing ADS-B day 2026-09-01')) as preflight, \
                    patch.object(world, 'pin_inputs') as pin, \
                    patch.object(world.subprocess, 'run') as run, \
                    patch.dict(os.environ, {}, clear=True):
                paths.return_value.update(airline=airline, general_aviation=ga)
                with self.assertRaisesRegex(ValueError, 'missing ADS-B day 2026-09-01'):
                    world.main()
                self.assertEqual(preflight.call_args.args, (airline, ga, '2026-09'))
                self.assertFalse(output.exists())
                pin.assert_not_called()
                run.assert_not_called()
                # --plan is a read-only nonproducer: it never builds or validates sources.
                with patch.object(sys, 'argv', [*argv, '--plan']):
                    world.main()
                preflight.assert_called_once()
                self.assertFalse(output.exists())

    def test_partial_transport_finalization_does_not_restart_enrichment_on_split_geometry(self):
        for layer in ('railways', 'roads'):
            with self.subTest(layer=layer):
                final = layer + '-finalize'
                steps = [world.Step(layer, (), ('enrich',)),
                         world.Step(final, (layer,), ('finalize',))]
                started = []
                def execute(step):
                    started.append(step.name)
                    self.assertEqual(step.slots, 4)
                self.assertEqual(world.run_plan(steps, execute, completed={layer}), {layer, final})
                self.assertEqual(started, [final])

    def test_last_running_producer_reclaims_memory_without_delaying_ready_work(self):
        for ready_consumer in (False, True):
            sibling_started, memory_expanded = threading.Event(), threading.Event()
            expanded, finished = [], set()
            steps = [world.Step('short', (), ()), world.Step('long', (), ())]
            if ready_consumer:
                steps.append(world.Step('consumer', ('short',), ()))
            def execute(step):
                if step.name == 'long':
                    sibling_started.set()
                    self.assertTrue(memory_expanded.wait(2))
                else:
                    self.assertTrue(sibling_started.wait(2))
                finished.add(step.name)
            def expand(step):
                self.assertIn('short', finished)
                if ready_consumer:
                    self.assertIn('consumer', finished)
                expanded.append(step.name)
                memory_expanded.set()
            self.assertEqual(world.run_plan(steps, execute, expand_memory=expand), {step.name for step in steps})
            self.assertEqual(expanded, ['long'])
        with patch.object(world.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            world.give_remaining_memory(world.Step('roads-finalize', (), ()), {'memory_gib': 80})
            self.assertEqual(run.call_args.args[0][-2:], ['world-build-roads-finalize.scope', f'MemoryMax={80 << 30}'])
        for active in (0, 3):
            with patch.object(world.subprocess, 'run', side_effect=[
                    subprocess.CompletedProcess([], 1, stderr='scope update unavailable'), subprocess.CompletedProcess([], active)]):
                world.give_remaining_memory(world.Step('retained', (), ()), {'memory_gib': 80})

    def test_changed_removed_added_and_cyclic_sources_cannot_validate_a_generation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            source.mkdir()
            path = source / 'measured.csv'
            path.write_text('count=37')
            alias = source / 'alias.csv'
            alias.symlink_to(path)
            pin = root / 'pins.jsonl'
            pin_inputs(pin, [source])
            self.assertEqual(len(load_pin(pin)), 2)
            verify_inputs(pin, [source])
            other = source / 'second.csv'
            other.write_text('count=40')
            other_pin = root / 'other.jsonl'
            pin_inputs(other_pin, [source])
            alias.unlink()
            alias.symlink_to(other)
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(other_pin, [source])
            alias.unlink()
            alias.symlink_to(path)
            other.unlink()
            extra = source / 'new.csv'
            extra.write_text('new feed')
            with self.assertRaisesRegex(ValueError, 'files added'):
                verify_inputs(pin, [source])
            extra.unlink()
            path.write_text('count=380')
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(pin, [source])
            alias.unlink()
            path.unlink()
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(pin, [source])
            (source / 'cycle').symlink_to(source, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'cyclic'):
                list(input_files([source]))

    def test_pin_and_producer_environment_preserves_local_datums_without_network_or_resume_overrides(self):
        with patch.dict(os.environ, {'PATH': '/bin', 'GDAL_DATA': '/datum/gdal',
                       'PROJ_DATA': '/datum/proj', 'FROM_STAGE': '2A', 'PROJ_NETWORK': 'ON',
                       'LD_LIBRARY_PATH': '/unpinned/native'}, clear=True):
            environment = world.producer_environment(4)
        self.assertEqual(environment['GDAL_DATA'], '/datum/gdal')
        self.assertEqual(environment['PROJ_DATA'], '/datum/proj')
        self.assertEqual(environment['PROJ_NETWORK'], 'OFF')
        self.assertNotIn('FROM_STAGE', environment)
        self.assertNotIn('LD_LIBRARY_PATH', environment)
        self.assertEqual(environment['RAYON_NUM_THREADS'], '4')
        self.assertEqual(environment['QM_ROAD_WORKERS'], '4')

    def test_spawned_worker_limits_gdal_cache_instead_of_using_host_memory(self):
        with patch.dict(os.environ, {'GDAL_CACHEMAX': '8192'}):
            environment = world.producer_environment(20)
        cache_bytes = subprocess.check_output([
            sys.executable, '-c',
            'from rasterio.env import get_gdal_config; print(get_gdal_config("GDAL_CACHEMAX"))',
        ], env=environment, text=True)
        self.assertEqual(int(cache_bytes), 256 << 20)

    def test_noncanonical_or_future_dates_fail_before_any_producer(self):
        with patch.object(world, 'source_paths', return_value={}):
            for as_of, anchor in [('202699', '2026-09'), ('20260909', '2026-9'),
                                  ('20260909', '2026-10')]:
                with self.subTest(as_of=as_of, anchor=anchor), self.assertRaises(ValueError):
                    world.build_plan({'build': {'as_of_date': as_of, 'aircraft_anchor': anchor}},
                                     Path('/unused/output'), Path('/unused/scratch'))

    def test_scheduler_does_not_start_dependents_after_failure_and_finishes_running_siblings(self):
        started, completed = set(), set()
        sibling_running = threading.Event()
        def execute(step):
            started.add(step.name)
            if step.name == 'rail':
                sibling_running.set()
                time.sleep(0.04)
            if step.name == 'industry':
                self.assertTrue(sibling_running.wait(2))
                raise RuntimeError('bad source')
            completed.add(step.name)
        steps = [world.Step('rail', (), ()), world.Step('industry', (), ()),
                 world.Step('publish', ('rail', 'industry'), ())]
        with self.assertRaisesRegex(RuntimeError, 'bad source'):
            world.run_plan(steps, execute)
        self.assertEqual(started, {'rail', 'industry'})
        self.assertEqual(completed, {'rail'})

    def test_structures_rewritten_after_the_finalize_step_fail_the_build(self):
        with tempfile.TemporaryDirectory() as directory:
            engine = Path(directory) / 'structures-finalize'
            engine.write_text('#!/bin/sh\necho "structures-finalize: 1/1 squares" >&2\n'
                              'echo "{\\"squares\\":1,\\"indexed\\":1,\\"blocked\\":$BLOCKED,\\"written\\":$WRITTEN,\\"edges\\":4}"\n')
            engine.chmod(0o755)
            steps = [world.Step('structures-finalize', ('structures',), (str(engine), '/prepared/2026'))]
            world.require_structures_final(steps, {'BLOCKED': '0', 'WRITTEN': '0'})
            with self.assertRaisesRegex(ValueError, '0 structures.arrow re-batched and 1 structures.qoix rewritten by the structures-finalize rerun'):
                world.require_structures_final(steps, {'BLOCKED': '0', 'WRITTEN': '1'})
            with self.assertRaisesRegex(ValueError, '1 structures.arrow re-batched and 1 structures.qoix rewritten'):
                world.require_structures_final(steps, {'BLOCKED': '1', 'WRITTEN': '1'})

    def test_whole_plan_waits_for_building_attributes_and_structures_and_bounds_parallel_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = {name: str(root / name) for name in ('planet', 'rasters', 'enrichment', 'boundaries',
                       'city_boundaries', 'overture', 'ghsl', 'regional_heights', 'airline', 'general_aviation', 'ships', 'ships_gfw')}
            for path in sources.values():
                Path(path).touch()
            config = {'build': {'as_of_date': '20260909', 'aircraft_anchor': '2026-09',
                               'memory_gib': 80, 'threads': 4}, 'sources': sources}
            _, plan = world.build_plan(config, root / 'out', root / 'scratch')
            running, done = {}, set()
            lock = threading.Lock()
            peak = 0
            indexed = {step.name: step for step in plan}
            osm = dict(indexed['osm'].environment)
            self.assertEqual(osm['NODE_CACHE'], str(root / 'scratch/osm/osm_nodes.cache'))
            self.assertEqual(osm['SPILL_DIR'], str(root / 'out/osm-spill'))
            overrides = dict(config['build'], osm_node_cache=str(root / 'cache/nodes'),
                             osm_spill_dir=str(root / 'spill/features'))
            _, overridden = world.build_plan(dict(config, build=overrides), root / 'out', root / 'scratch')
            self.assertEqual(dict(overridden[0].environment)['NODE_CACHE'], overrides['osm_node_cache'])
            self.assertEqual(dict(overridden[0].environment)['SPILL_DIR'], overrides['osm_spill_dir'])
            self.assertNotEqual(world.step_identity(indexed['osm'], config['build'], 'pin'),
                                world.step_identity(overridden[0], overrides, 'pin'))
            for key in ('osm_node_cache', 'osm_spill_dir'):
                for value in (False, 42, '', *sources.values(), str(root)):
                    with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                        world.build_plan(dict(config, build=dict(config['build'], **{key: value})),
                                         root / 'out', root / 'scratch')
            alias = root / 'source-alias'
            alias.symlink_to(root / 'enrichment')
            with self.assertRaisesRegex(ValueError, 'overlaps frozen source'):
                world.build_plan(dict(config, build=dict(config['build'], osm_spill_dir=str(alias))),
                                 root / 'out', root / 'scratch')
            with self.assertRaisesRegex(ValueError, 'overlaps frozen source'):
                world.validate_osm_storage([root / 'external-vrt'], [root / 'external-vrt/tile.tif'])
            self.assertEqual(indexed['structures'].dependencies, ('buildings',))
            self.assertEqual(indexed['structures'].argv[-2:], ('--jobs', '4'))
            self.assertEqual(indexed['structures-finalize'].dependencies, ('structures',))
            self.assertTrue(indexed['structures-finalize'].argv[0].endswith('engine/target/release/structures-finalize'))
            self.assertEqual(set(indexed['roads'].dependencies), {'square-country-city', 'structures'})
            self.assertEqual(indexed['roads'].argv[indexed['roads'].argv.index('--jobs') + 1], '4')
            self.assertEqual(indexed['roads-finalize'].dependencies, ('roads',))
            self.assertEqual(indexed['industrial'].dependencies, ('square-country-city',))
            self.assertEqual(indexed['railways'].dependencies, ('square-country-city',))
            self.assertEqual(indexed['railways-finalize'].dependencies, ('railways',))
            self.assertTrue(indexed['railways-finalize'].argv[0].endswith('engine/target/release/railways-finalize'))
            self.assertNotIn('repaint', indexed)
            geography_running, aircraft_running = threading.Event(), threading.Event()
            def execute(step):
                nonlocal peak
                with lock:
                    self.assertTrue(set(step.dependencies) <= done)
                    running[step.name] = step.slots
                    occupied = sum(running.values())
                    self.assertLessEqual(occupied, 4)
                    peak = max(peak, len(running))
                if step.name == 'square-country-city':
                    geography_running.set()
                    self.assertTrue(aircraft_running.wait(2), "aircraft waited for geography")
                elif step.name == 'aircraft':
                    self.assertTrue(geography_running.wait(2))
                    aircraft_running.set()
                time.sleep(0.015)
                with lock:
                    del running[step.name]
                    done.add(step.name)
            self.assertEqual(world.run_plan(plan, execute), set(indexed))
            self.assertGreaterEqual(peak, 2)
            aircraft = dict(indexed['aircraft'].environment)
            self.assertEqual(aircraft['AIRCRAFT_ANCHOR'], '2026-09')
            self.assertEqual(aircraft['AIRLINE_FEED'], 'adsbexchange')
            self.assertEqual(aircraft['PREPARED_DIR'], aircraft['PREPARED_YEAR_DIR'])


if __name__ == '__main__':
    unittest.main()
