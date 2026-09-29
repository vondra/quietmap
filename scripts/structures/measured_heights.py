"""National measured building heights: cache contract and ladder rung 1 join."""

import pyarrow as pa
import shapely
from shapely import STRtree

from structure_inputs import footprint_in_longitude_frame

CONTRACT_KEY = "measured_heights_contract"
CONTRACT_VERSION = "measured_heights_v1"

SCHEMA = pa.schema([
    pa.field("geometry", pa.binary(), nullable=False),  # WKB Polygon lon/lat
    pa.field("height_m", pa.float32(), nullable=False),  # mean roof height
    pa.field("source", pa.utf8(), nullable=False),       # e.g. DE-NW-LoD1-2026
    pa.field("as_of", pa.utf8(), nullable=False),        # inventory vintage YYYY-MM-DD
])

# The OSM/Overture match predicate (structure_merge.IOU_MATCH_THRESHOLD):
# a measured footprint answers a candidate it covers or substantially overlaps.
IOU_MATCH_THRESHOLD = 0.5


def apply_measured_heights(candidates, measured_rows):
    """Override `regional_m` where a measured footprint answers the candidate.
    Returns the answered count. Vector heights win over the raster survey:
    they are per building, the raster is a zonal mean."""
    answered = 0
    if not measured_rows:
        return answered
    reference = float(shapely.get_coordinates(measured_rows[0]["geom"])[0][0])
    framed = [footprint_in_longitude_frame(row["geom"], reference)
              for row in measured_rows]
    tree = STRtree(framed)
    for candidate in candidates:
        geom = candidate["geom"]
        if geom is None or geom.is_empty:
            continue
        framed_candidate = footprint_in_longitude_frame(geom, reference)
        centroid = framed_candidate.centroid
        best, best_iou = None, -1.0
        for k in tree.query(framed_candidate, predicate="intersects"):
            target = framed[k]
            try:
                inter = framed_candidate.intersection(target).area
            except Exception:
                left = framed_candidate if framed_candidate.is_valid \
                    else framed_candidate.buffer(0)
                right = target if target.is_valid else target.buffer(0)
                inter = 0.0 if left.is_empty or right.is_empty \
                    else left.intersection(right).area
            iou = 0.0
            if inter > 0.0:
                union = framed_candidate.area + target.area - inter
                iou = inter / union if union > 0 else 0.0
            if (target.covers(centroid) or iou >= IOU_MATCH_THRESHOLD) and iou > best_iou:
                best, best_iou = measured_rows[k], iou
        if best is not None:
            candidate["regional_m"] = float(best["height_m"])
            answered += 1
    return answered
