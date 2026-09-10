"""Publication rejects incomplete native rasters, stale contracts and cyclic height inputs."""

import os
from pathlib import Path
import sqlite3
import struct
import tempfile
import unittest
from unittest.mock import patch

import pyarrow as pa

import world_build_inputs as inputs
from prepared_manifest import sha256


class WorldBuildInputsTest(unittest.TestCase):
    def test_catalog_pins_files_and_rejects_false_ocean_and_unknown_source(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(inputs.qmgrid, 'Z9_AXIS', 2):
            root = Path(directory)
            source = root / 'source'
            tile = source / 'z9/0/0'
            tile.mkdir(parents=True)
            raster = tile / 'dem.i16be'
            raster.write_bytes(b'\x00\x25')
            catalog = sqlite3.connect(source / 'rasters.sqlite')
            catalog.executescript('CREATE TABLE raster_channels(channel,contract,source_identity);'
                                  'CREATE TABLE raster_squares(channel,square,sha256);')
            # Match the reader-owned physical contract without another literal pin.
            import re
            channel_source = Path(__file__).parents[1] / 'engine/raster-reader/src/channel.rs'
            contract = re.search(r'pub const CONTRACT: &str = "([^"]+)";', channel_source.read_text()).group(1)
            for channel in ('dem', 'forest', 'imd'):
                catalog.execute('INSERT INTO raster_channels VALUES(?,?,?)', (channel, contract, 'a' * 64))
                for square in range(4):
                    digest = sha256(raster) if channel == 'dem' and square == 0 else None
                    catalog.execute('INSERT INTO raster_squares VALUES(?,?,?)', (channel, square, digest))
            catalog.commit()
            pins = sqlite3.connect(':memory:')
            selected = list(inputs.raster_inputs(source))
            self.assertEqual(set(selected), {source / 'rasters.sqlite', raster})
            inputs.pin_inputs(pins, selected)
            output = root / 'valid'
            inputs.attach_rasters(source, output, pins)
            self.assertEqual((output / 'z9/0/0/dem.i16be').read_bytes(), raster.read_bytes())
            self.assertEqual(len(list(output.glob('z9/*/*'))), 4)
            inputs.verify_prepared_raster_links(source, output)
            attached = output / 'z9/0/0/dem.i16be'
            replacement = root / 'replacement.i16be'
            replacement.write_bytes(b'other generation')
            attached.unlink()
            attached.symlink_to(replacement)
            with self.assertRaisesRegex(ValueError, 'prepared raster replaced'):
                inputs.verify_prepared_raster_links(source, output)
            attached.unlink()
            attached.symlink_to(raster)
            extra = tile / 'forest.u8'
            extra.write_bytes(b'\x00')
            with self.assertRaisesRegex(ValueError, 'ocean declaration'):
                inputs.attach_rasters(source, root / 'ocean', pins)
            extra.unlink()
            catalog.execute("UPDATE raster_squares SET sha256=? WHERE channel='dem' AND square=0", (b'x' * 32,))
            catalog.commit()
            with self.assertRaisesRegex(ValueError, 'differ from catalog'):
                inputs.attach_rasters(source, root / 'wrong-hash', pins)
            catalog.execute("UPDATE raster_channels SET source_identity='' WHERE channel='dem'")
            catalog.commit()
            with self.assertRaisesRegex(ValueError, 'contract mismatch'):
                inputs.attach_rasters(source, root / 'unknown-source', pins)
            catalog.close()
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
                    (tile / 'admin.bin').write_bytes(struct.pack('<QBHH', inputs.qmgrid.square_id(x, y), 0, 0, 0))
                    # Import the producer-owned contracts used by the audit.
                    import sys
                    sys.path.insert(0, str(Path(__file__).parent / 'structures'))
                    from structure_contract import CONTRACT_KEY, CONTRACT_VERSION
                    schema = pa.schema([('value', pa.int32())], metadata={CONTRACT_KEY: CONTRACT_VERSION})
                    with pa.ipc.new_file(tile / 'structures.arrow', schema):
                        pass
            # buildings.arrow is an input to structures.arrow, not a served
            # layer. A finalized generation does not retain that intermediate.
            for layer in ('roads', 'railways', 'industrial', 'airborne', 'cruise', 'airport_traffic'):
                import sys
                sys.path.insert(0, str(Path(__file__).parent / 'admin'))
                from build_admin import expected_contract
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
            self.assertEqual(len(inputs.audit_world(root)), 7)
            structure = root / 'z9/1/1/structures.arrow'
            structure.unlink()
            with self.assertRaisesRegex(ValueError, 'unfinished structures'):
                inputs.audit_world(root)
            with pa.ipc.new_file(structure, pa.schema([('value', pa.int32())])):
                pass
            with self.assertRaisesRegex(ValueError, 'invalid structure contract'):
                inputs.audit_world(root)


if __name__ == '__main__':
    unittest.main()
