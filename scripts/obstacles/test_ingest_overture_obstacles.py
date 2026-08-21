#!/usr/bin/env python3
"""Regression tests for the published Overture envelope-class mapping."""

import importlib.util
from pathlib import Path


def load_ingest_module():
    path = Path(__file__).with_name("ingest-overture-obstacles.py")
    spec = importlib.util.spec_from_file_location("ingest_overture_obstacles", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


ingest = load_ingest_module()

# These are copied from FINDINGS-bldginterior-design.md §3, rather than
# derived from the implementation constants. The test must catch both an
# invented enum string and an accidentally dropped official class.
OFFICIAL_CLASSES = {
    "carport", "roof", "greenhouse", "glasshouse", "bridge_structure", "grandstand",
    "allotment_house", "apartments", "beach_hut", "boathouse", "bungalow", "cabin",
    "college", "detached", "dormitory", "dwelling_house", "ger", "hospital", "house",
    "houseboat", "hut", "kindergarten", "residential", "school", "semi",
    "semidetached_house", "static_caravan", "stilt_house", "terrace", "trullo",
    "university", "commercial", "hotel", "office", "retail", "supermarket",
    "agricultural", "barn", "cowshed", "digester", "factory", "farm", "farm_auxiliary",
    "hangar", "industrial", "manufacture", "shed", "silo", "slurry_tank", "stable",
    "storage_tank", "sty", "warehouse", "cathedral", "chapel", "church", "civic",
    "fire_station", "government", "library", "monastery", "mosque", "post_office",
    "presbytery", "public", "religious", "shrine", "synagogue", "temple",
    "wayside_shrine", "garage", "garages", "kiosk", "service", "parking", "stadium",
    "sports_centre", "sports_hall", "pavilion", "toilets", "bunker", "military",
    "transportation", "train_station", "transformer_tower", "outbuilding", "guardhouse",
}


def test_official_class_sets_are_exhaustive_and_valid():
    mapped = ingest.OUTDOOR | ingest.RESIDENTIAL | ingest.COMMERCIAL | ingest.INDUSTRIAL
    mapped |= ingest.HISTORIC | ingest.DEFAULT
    assert mapped <= OFFICIAL_CLASSES
    assert OFFICIAL_CLASSES <= mapped
    assert ingest.OFFICIAL_CLASSES == OFFICIAL_CLASSES


def test_class_precedes_subtype_and_unknown_class_falls_back_to_subtype():
    assert ingest.envelope_class("carport", "residential", False) == 0
    assert ingest.envelope_class("garage", "residential", False) == 5
    assert ingest.envelope_class("unrecognised_future_class", "residential", False) == 1
    assert ingest.envelope_class(None, "commercial", False) == 2
    assert ingest.envelope_class(None, "outbuilding", False) == 5
    assert ingest.envelope_class("house", "commercial", True) == 0


def test_every_official_class_has_a_stable_result():
    for building_class in sorted(OFFICIAL_CLASSES):
        result = ingest.envelope_class(building_class, "residential", False)
        assert result in range(6), (building_class, result)


if __name__ == "__main__":
    for test in (
        test_official_class_sets_are_exhaustive_and_valid,
        test_class_precedes_subtype_and_unknown_class_falls_back_to_subtype,
        test_every_official_class_has_a_stable_result,
    ):
        test()
    print("ingest envelope-class tests: PASS")
