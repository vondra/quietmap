"""World publication must respect data dependencies, memory admission and immutable inputs."""

import os
from unittest.mock import patch
import importlib.util
from pathlib import Path
import sqlite3
import sys
import tempfile
import threading
import time
import unittest

from world_build_inputs import input_files, pin_inputs, verify_inputs

spec = importlib.util.spec_from_file_location('world_build', Path(__file__).with_name('build-world.py'))
world = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = world
spec.loader.exec_module(world)


class WorldBuildTest(unittest.TestCase):
    def test_changed_removed_added_and_cyclic_sources_cannot_validate_a_generation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            source.mkdir()
            path = source / 'measured.csv'
            path.write_text('count=37')
            alias = source / 'alias.csv'
            alias.symlink_to(path)
            database = sqlite3.connect(root / 'pins.sqlite')
            pin_inputs(database, [source])
            self.assertEqual(database.execute('SELECT count(*) FROM inputs').fetchone()[0], 2)
            verify_inputs(database, [source])
            other = source / 'second.csv'
            other.write_text('count=40')
            other_database = sqlite3.connect(':memory:')
            pin_inputs(other_database, [source])
            alias.unlink()
            alias.symlink_to(other)
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(other_database, [source])
            other_database.close()
            alias.unlink()
            alias.symlink_to(path)
            other.unlink()
            extra = source / 'new.csv'
            extra.write_text('new feed')
            with self.assertRaisesRegex(ValueError, 'files added'):
                verify_inputs(database, [source])
            extra.unlink()
            path.write_text('count=38')
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(database, [source])
            alias.unlink()
            path.unlink()
            with self.assertRaisesRegex(ValueError, 'changed'):
                verify_inputs(database, [source])
            (source / 'cycle').symlink_to(source, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'cyclic'):
                list(input_files([source]))
            database.close()

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

    def test_structures_rewritten_after_the_obstacle_index_step_fail_the_build(self):
        with tempfile.TemporaryDirectory() as directory:
            engine = Path(directory) / 'obstacle-index-build'
            engine.write_text('#!/bin/sh\necho "obstacle-index: 1/1 squares" >&2\n'
                              'echo "{\\"squares\\":1,\\"indexed\\":1,\\"written\\":$WRITTEN,\\"edges\\":4}"\n')
            engine.chmod(0o755)
            steps = [world.Step('obstacle-index', ('structures',), (str(engine), '/prepared/2026'))]
            world.require_obstacle_index_current(steps, {'WRITTEN': '0'})
            with self.assertRaisesRegex(ValueError, '1 structures.qoix rewritten by the obstacle-index rerun'):
                world.require_obstacle_index_current(steps, {'WRITTEN': '1'})

    def test_whole_plan_waits_for_building_attributes_and_structures_and_bounds_parallel_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = {name: str(root / name) for name in ('planet', 'rasters', 'enrichment', 'boundaries',
                       'city_boundaries', 'overture', 'ghsl', 'regional_heights', 'airline', 'general_aviation')}
            for path in sources.values():
                Path(path).touch()
            config = {'build': {'as_of_date': '20260909', 'aircraft_anchor': '2026-09',
                               'memory_gib': 80, 'threads': 4}, 'sources': sources}
            _, plan = world.build_plan(config, root / 'out', root / 'scratch')
            running, done = set(), set()
            lock = threading.Lock()
            peak = 0
            indexed = {step.name: step for step in plan}
            self.assertEqual(indexed['structures'].dependencies, ('buildings',))
            self.assertEqual(indexed['obstacle-index'].dependencies, ('structures',))
            self.assertTrue(indexed['obstacle-index'].argv[0].endswith('engine/target/release/obstacle-index-build'))
            self.assertEqual(set(indexed['roads'].dependencies), {'square-country-city', 'structures'})
            self.assertEqual(indexed['industrial'].dependencies, ('square-country-city',))
            self.assertEqual(indexed['railways'].dependencies, ('square-country-city',))
            self.assertNotIn('repaint', indexed)
            def execute(step):
                nonlocal peak
                with lock:
                    self.assertTrue(set(step.dependencies) <= done)
                    running.add(step.name)
                    occupied = sum(indexed[name].slots for name in running)
                    self.assertLessEqual(occupied, 4)
                    peak = max(peak, len(running))
                time.sleep(0.015)
                with lock:
                    running.remove(step.name)
                    done.add(step.name)
            self.assertEqual(world.run_plan(plan, execute), set(indexed))
            self.assertGreaterEqual(peak, 2)
            aircraft = dict(indexed['aircraft'].environment)
            self.assertEqual(aircraft['AIRCRAFT_ANCHOR'], '2026-09')
            self.assertEqual(aircraft['AIRLINE_FEED'], 'adsbexchange')
            self.assertEqual(aircraft['PREPARED_DIR'], aircraft['PREPARED_YEAR_DIR'])


if __name__ == '__main__':
    unittest.main()
