"""Publication rejects missing native raster files, replaced links and cyclic height inputs."""

import os
from pathlib import Path
import sqlite3
import struct
import tempfile
import unittest
from unittest.mock import patch

import pyarrow as pa

import world_build_inputs as inputs


class WorldBuildInputsTest(unittest.TestCase):
    def test_every_square_needs_a_data_or_zero_byte_file_and_links_stay_pinned(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(inputs.qmgrid, 'Z9_AXIS', 2):
            root = Path(directory)
            source = root / 'source'
            raster = source / 'z9/0/0/dem.i16be'
            for x in range(2):
                for y in range(2):
                    square = source / inputs.qmgrid.square_name(x, y)
                    square.mkdir(parents=True)
                    for name in ('dem.i16be', 'forest.u8', 'imd.u8'):
                        (square / name).write_bytes(b'\x00\x25' if square / name == raster else b'')
            pins = sqlite3.connect(':memory:')
            selected = list(inputs.raster_inputs(source))
            self.assertEqual(len(selected), 12)
            self.assertIn(raster, selected)
            inputs.pin_inputs(pins, selected)
            output = root / 'valid'
            inputs.attach_rasters(source, output)
            inputs.attach_rasters(source, output)  # a resumed build attaches again
            self.assertEqual((output / 'z9/0/0/dem.i16be').read_bytes(), raster.read_bytes())
            self.assertEqual((output / 'z9/1/1/imd.u8').stat().st_size, 0)
            self.assertEqual(len(list(output.glob('z9/*/*/*'))), 12)
            inputs.verify_prepared_raster_links(source, output)
            attached = output / 'z9/0/0/dem.i16be'
            replacement = root / 'replacement.i16be'
            replacement.write_bytes(b'other generation')
            attached.unlink()
            attached.symlink_to(replacement)
            with self.assertRaisesRegex(ValueError, 'prepared raster replaced'):
                inputs.verify_prepared_raster_links(source, output)
            (source / 'z9/1/0/forest.u8').unlink()
            with self.assertRaisesRegex(ValueError, 'missing raster'):
                list(inputs.raster_inputs(source))
            with self.assertRaisesRegex(ValueError, 'missing raster'):
                inputs.attach_rasters(source, root / 'missing')
            pins.close()

    def test_height_alias_cycle_is_rejected_before_gdal_opens_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source.vrt'
            source.write_text('<VRTDataset><SourceFilename relativeToVRT="1">alias.vrt</SourceFilename></VRTDataset>')
            (root / 'alias.vrt').symlink_to(source)
            with self.assertRaisesRegex(ValueError, 'cyclic height'):
                list(inputs.height_inputs(source))

    def test_world_audit_requires_all_squares_and_all_seven_layers(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(inputs.qmgrid, 'Z9_AXIS', 2):
            root = Path(directory)
            for x in range(2):
                for y in range(2):
                    tile = root / inputs.qmgrid.square_name(x, y)
                    tile.mkdir(parents=True)
                    (tile / 'square-country-city.bin').write_bytes(struct.pack('<QBHH', inputs.qmgrid.square_id(x, y), 0, 0, 0))
                    # Import the producer-owned contracts used by the audit.
                    import sys
                    sys.path.insert(0, str(Path(__file__).parent / 'structures'))
                    from structure_contract import CONTRACT_KEY, CONTRACT_VERSION
                    schema = pa.schema([('value', pa.int32())], metadata={CONTRACT_KEY: CONTRACT_VERSION})
                    with pa.ipc.new_file(tile / 'structures.arrow', schema):
                        pass
                    (tile / 'structures.qoix').touch()
            # buildings.arrow is an input to structures.arrow, not a served
            # layer. A finalized generation does not retain that intermediate.
            for layer in ('roads', 'railways', 'industrial', 'airborne', 'cruise', 'airport_traffic'):
                import sys
                sys.path.insert(0, str(Path(__file__).parent / 'square-country-city'))
                from build_square_country_city import expected_contract
                path = root / 'z9/0/0' / f'{layer}.arrow'
                metadata = dict([expected_contract(path)]) if layer in ('roads', 'railways', 'industrial') else None
                table = pa.table({'value': [37]}).replace_schema_metadata(metadata)
                with pa.ipc.new_file(path, table.schema) as writer:
                    writer.write_table(table)
            with self.assertRaisesRegex(ValueError, 'no world rows for structures'):
                inputs.audit_world(root)
            path = root / 'z9/0/0/structures.arrow'
            table = pa.table({'value': [37]}).replace_schema_metadata({CONTRACT_KEY: CONTRACT_VERSION})
            with pa.ipc.new_file(path, table.schema) as writer:
                writer.write_table(table)
            # The merge's plain chunks are not final: only structures-finalize stamps qm_blocks.
            with self.assertRaisesRegex(ValueError, 'unfinished structures blocks'):
                inputs.audit_world(root)
            table = table.replace_schema_metadata({CONTRACT_KEY: CONTRACT_VERSION, 'qm_blocks': 'AQ=='})
            with pa.ipc.new_file(path, table.schema) as writer:
                writer.write_table(table)
            self.assertEqual(len(inputs.audit_world(root)), 7)
            structure = root / 'z9/1/1/structures.arrow'
            structure.unlink()
            with self.assertRaisesRegex(ValueError, 'unfinished structures'):
                inputs.audit_world(root)
            (root / 'z9/1/1/structures.qoix').unlink()
            with pa.ipc.new_file(structure, schema):
                pass
            with self.assertRaisesRegex(ValueError, 'unfinished obstacle index'):
                inputs.audit_world(root)
            (root / 'z9/1/1/structures.qoix').touch()
            with pa.ipc.new_file(structure, pa.schema([('value', pa.int32())])):
                pass
            with self.assertRaisesRegex(ValueError, 'invalid structure contract'):
                inputs.audit_world(root)


if __name__ == '__main__':
    unittest.main()
