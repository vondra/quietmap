#!/usr/bin/env python3
"""Regression fixture for materializer class preservation and fallback."""

import importlib.util
import tempfile
from pathlib import Path

import pyarrow as pa
import pyarrow.ipc as ipc


def load_materializer_module():
    path = Path(__file__).with_name("enrich-obstacle-heights.py")
    spec = importlib.util.spec_from_file_location("enrich_obstacle_heights", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


materializer = load_materializer_module()


def write_arrow(path, table):
    path.parent.mkdir(parents=True, exist_ok=True)
    with ipc.new_file(path, table.schema) as writer:
        writer.write_table(table)


class FixedGlobalPrior:
    def sample(self, lon, lat):
        return 12.0


def test_materializer_preserves_envelope_class_while_changing_height():
    cell = "841e309ffffffff"
    polygon = bytes.fromhex(
        "01030000000100000005000000"
        "00000000000000000000000000000000"
        "000000000000f03f0000000000000000"
        "000000000000f03f000000000000f03f"
        "0000000000000000000000000000f03f"
        "00000000000000000000000000000000"
    )
    with tempfile.TemporaryDirectory(prefix="qm-obstacle-materializer-") as root:
        root = Path(root)
        staging = root / "staging"
        prepared = root / "prepared"
        row = {
            "polygon_wkb": [polygon],
            "height_m": [8.0],
            "centroid_lat": [50.0],
            "centroid_lon": [14.0],
            "height_tier": [2],
            "envelope_class": [2],
        }
        staged = pa.table(row, schema=materializer.SCHEMA)
        write_arrow(staging / cell / "obstacles-N50E014.arrow", staged)
        write_arrow(prepared / cell / "obstacles.arrow", staged)

        materializer.enrich_cell(
            cell,
            str(prepared),
            str(staging),
            FixedGlobalPrior(),
            None,
        )
        output = ipc.open_file(prepared / cell / "obstacles.arrow").read_all()
        assert output.column("envelope_class").to_pylist() == [2]
        assert output.column("height_tier").to_pylist() == [4]
        assert output.column("height_m").to_pylist() == [12.0]


def test_materializer_falls_back_to_default_for_pre_class_staging():
    cell = "841e309ffffffff"
    polygon = bytes.fromhex(
        "01030000000100000005000000"
        "00000000000000000000000000000000"
        "000000000000f03f0000000000000000"
        "000000000000f03f000000000000f03f"
        "0000000000000000000000000000f03f"
        "00000000000000000000000000000000"
    )
    old_schema = pa.schema(
        [
            ("polygon_wkb", pa.binary()),
            ("height_m", pa.float32()),
            ("centroid_lat", pa.float64()),
            ("centroid_lon", pa.float64()),
            ("height_tier", pa.uint8()),
        ]
    )
    old = pa.table(
        {
            "polygon_wkb": [polygon],
            "height_m": [8.0],
            "centroid_lat": [50.0],
            "centroid_lon": [14.0],
            "height_tier": [2],
        },
        schema=old_schema,
    )
    with tempfile.TemporaryDirectory(prefix="qm-obstacle-materializer-old-") as root:
        root = Path(root)
        staging = root / "staging"
        prepared = root / "prepared"
        write_arrow(staging / cell / "obstacles-N50E014.arrow", old)
        write_arrow(prepared / cell / "obstacles.arrow", old)
        materializer.enrich_cell(cell, str(prepared), str(staging), FixedGlobalPrior(), None)
        output = ipc.open_file(prepared / cell / "obstacles.arrow").read_all()
        assert output.column("envelope_class").to_pylist() == [5]


if __name__ == "__main__":
    test_materializer_preserves_envelope_class_while_changing_height()
    test_materializer_falls_back_to_default_for_pre_class_staging()
    print("materializer envelope-class fixture: PASS")
