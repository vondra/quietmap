"""Streaming source downloads preserve bbox rows without materializing whole strips."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import pyarrow as pa
import pyarrow.dataset as ds
import pyarrow.parquet as pq


SPEC = importlib.util.spec_from_file_location(
    "download_overture", Path(__file__).with_name("download-overture-tiles.py"))
assert SPEC is not None and SPEC.loader is not None
DOWNLOADER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DOWNLOADER)


class OvertureDownloadTests(unittest.TestCase):
    def test_sparse_resume_never_scans_the_gap_between_requested_tiles(self):
        tiles = ["N54E173", "N54E174", "N54E175", "N54W178",
                 "N35E087", "N35E150", "N35E151", "N35E152",
                 "N00W001", "N00E000", "N00W002", "N00E001"]
        strips = list(DOWNLOADER.strips(tiles))
        self.assertCountEqual([tile for strip in strips for tile in strip], tiles)
        for strip in strips:
            boxes = [DOWNLOADER.tile_bbox(tile) for tile in strip]
            self.assertLessEqual(len(strip), DOWNLOADER.BATCH_TILES)
            self.assertEqual(len({box[1] for box in boxes}), 1)
            self.assertEqual(max(box[2] for box in boxes) - min(box[0] for box in boxes),
                             len(strip), strip)

    def source(self, root):
        schema = pa.schema([
            pa.field("id", pa.string()), pa.field("geometry", pa.binary()),
            pa.field("bbox", pa.struct([pa.field(name, pa.float64())
                                        for name in ["xmin", "ymin", "xmax", "ymax"]])),
            pa.field("height", pa.float64()), pa.field("num_floors", pa.int32()),
            pa.field("class", pa.string()), pa.field("subtype", pa.string()),
            pa.field("is_underground", pa.bool_()),
        ], metadata={b"source": b"independent-fixture"})
        boxes = [(0.1, 0.1, 0.2, 0.2), (0.9, 0.1, 1.1, 0.2),
                 (1.1, 0.1, 1.2, 0.2), (-0.2, 0.1, 0.0, 0.2),
                 (2.0, 0.1, 2.2, 0.2), (0.1, 1.0, 0.2, 1.2),
                 (3.1, 0.1, 3.2, 0.2), None]
        rows = [{"id": str(index), "geometry": bytes([index]) * 100,
                 "bbox": dict(zip(["xmin", "ymin", "xmax", "ymax"], box)) if box else None,
                 "height": None if index % 2 else 12.5, "num_floors": index,
                 "class": "house", "subtype": "residential", "is_underground": False}
                for index, box in enumerate(boxes)]
        rows = [{**row, "id": str(index)} for index, row in enumerate(rows * 20_000)]
        table = pa.Table.from_pylist(rows, schema=schema)
        paths = [root / "first.parquet", root / "second.parquet"]
        for index, path in enumerate(paths):
            pq.write_table(table.slice(index * 80_000, 80_000), path, row_group_size=4096)
        return ds.dataset(paths, format="parquet")

    def test_streamed_multifragment_rows_schema_order_and_empty_tile_match_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self.source(root)
            strip = ["N00E000", "N00E001", "N00E002", "N00E003"]
            observed = []

            class StreamingOnlySource:
                def scanner(self, **kwargs):
                    observed.append(kwargs)
                    return source.scanner(**kwargs)

            self.assertEqual(DOWNLOADER.fetch_strip(StreamingOnlySource(), strip, root), 0)
            self.assertEqual(len(observed), 1)
            options = observed[0]
            self.assertEqual(options["batch_readahead"], 0)
            self.assertEqual(options["fragment_readahead"], 1)
            self.assertFalse(options["fragment_scan_options"].pre_buffer)
            self.assertTrue(options["fragment_scan_options"].use_buffered_stream)
            for tile in strip:
                expected = source.to_table(columns=DOWNLOADER.COLUMNS,
                                           filter=DOWNLOADER.bbox_filter(*DOWNLOADER.tile_bbox(tile)))
                actual = pq.read_table(root / f"{tile}.parquet")
                self.assertTrue(actual.equals(expected, check_metadata=True), tile)
                self.assertLess(expected.nbytes, DOWNLOADER.TILE_BUFFER_BYTES)
                self.assertEqual(pq.ParquetFile(root / f"{tile}.parquet").num_row_groups, 1)
            flushing = root / "flushing"
            flushing.mkdir()
            with patch.object(DOWNLOADER, "TILE_BUFFER_BYTES", 1):
                self.assertEqual(DOWNLOADER.fetch_strip(source, strip[:1], flushing), 0)
            flushed = flushing / f"{strip[0]}.parquet"
            self.assertGreater(pq.ParquetFile(flushed).num_row_groups, 1)
            self.assertTrue(pq.read_table(flushed).equals(
                pq.read_table(root / f"{strip[0]}.parquet"), check_metadata=True))
            empty = "N00E004"
            self.assertEqual(DOWNLOADER.fetch_strip(source, [empty], root), 0)
            self.assertEqual(pq.read_table(root / f"{empty}.parquet").num_rows, 0)
            paths = [root / f"{tile}.parquet" for tile in [*strip, empty]]
            before = [(path.stat().st_ino, path.stat().st_mtime_ns, path.read_bytes()) for path in paths]
            with tempfile.NamedTemporaryFile(mode="w", dir=root) as selected:
                selected.write("\n".join([*strip, empty]))
                selected.flush()
                with patch.object(DOWNLOADER.pafs, "S3FileSystem", side_effect=AssertionError("cached download")):
                    self.assertEqual(DOWNLOADER.main(selected.name, root), 0)
            self.assertEqual(before, [(path.stat().st_ino, path.stat().st_mtime_ns, path.read_bytes())
                                      for path in paths])

    def test_failed_scan_does_not_publish_a_partial_strip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self.source(root)

            class FailingSource:
                def scanner(self, **kwargs):
                    scanner = source.scanner(**kwargs)

                    class FailingScanner:
                        projected_schema = scanner.projected_schema

                        def to_batches(self):
                            yield next(scanner.to_batches())
                            raise OSError("injected source failure")

                    return FailingScanner()

            strip = ["N00E000", "N00E001"]
            self.assertEqual(DOWNLOADER.fetch_strip(FailingSource(), strip, root), len(strip))
            self.assertFalse(any((root / f"{tile}.parquet").exists() for tile in strip))
            parquet_writer = pq.ParquetWriter

            class FailingWriter:
                def __init__(self, *args):
                    self.writer = parquet_writer(*args)

                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    self.writer.close()

                def write_table(self, table):
                    self.writer.write_table(table)
                    raise OSError("injected write failure")

            with patch.object(DOWNLOADER.pq, "ParquetWriter", FailingWriter):
                self.assertEqual(DOWNLOADER.fetch_strip(source, strip, root), len(strip))
            self.assertFalse(any((root / f"{tile}.parquet").exists() for tile in strip))
            self.assertEqual(DOWNLOADER.fetch_strip(source, strip, root), 0)
            for tile in strip:
                expected = source.to_table(columns=DOWNLOADER.COLUMNS,
                                           filter=DOWNLOADER.bbox_filter(*DOWNLOADER.tile_bbox(tile)))
                self.assertTrue(pq.read_table(root / f"{tile}.parquet").equals(expected, check_metadata=True))


if __name__ == "__main__":
    unittest.main(verbosity=2)
