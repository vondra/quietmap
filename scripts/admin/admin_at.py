"""CGAZ country, coastal ownership and metro lookup using dev1's geographic policy."""

import json
from pathlib import Path

import numpy as np
import shapely
from shapely import Polygon, STRtree

# dev1 AdminAt: a sea point is attributable only to exactly one coast within 2 km.
COASTAL_BUFFER_M = 2000.0
M_PER_DEG_LAT = 110_540.0
M_PER_DEG_LON_EQ = 111_320.0
GEOGRAPHY = json.loads(Path(__file__).with_name("geography.json").read_text())


def polar_coordinates(coordinates, south):
    coordinates = np.asarray(coordinates, dtype=float)
    radius = 90 + coordinates[..., 1] if south else 90 - coordinates[..., 1]
    angle = np.radians(coordinates[..., 0])
    return np.stack((radius * np.cos(angle), radius * np.sin(angle)), axis=-1)


class AdminResolver:
    def __init__(self, features, geography=GEOGRAPHY):
        self.countries = geography["countries"]
        self.metros = [(m, Polygon(m["polygon"])) for m in geography["metros"]]
        polygons, groups = [], []
        self.polar_parts = []
        self.coastal_rings = []
        self.coastal_index = None
        for feature in features:
            group = str(feature["properties"]["shapeGroup"])
            if group not in self.countries and not group.isdecimal():
                raise ValueError(f"Unmapped CGAZ country {group}; refusing an UNKNOWN bake")
            geometry = feature["geometry"]
            if geometry["type"] not in ("Polygon", "MultiPolygon"):
                raise ValueError(f"Unsupported CGAZ geometry {geometry['type']}")
            parts = geometry["coordinates"]
            if geometry["type"] == "Polygon":
                parts = [parts]
            for rings in parts:
                outer = np.asarray(rings[0], dtype=float)
                self.coastal_rings.append((group, outer))
                if np.any(np.abs(np.diff(outer[:, 0])) > 180):
                    # CGAZ's pole-enclosing Antarctica artifact needs dev1's polar PIP.
                    south = bool(outer[:, 1].mean() < 0)
                    polygon = Polygon(polar_coordinates(outer, south),
                                      [polar_coordinates(hole, south) for hole in rings[1:]])
                    self.polar_parts.append((group, polygon, south, outer[:, 1].min(), outer[:, 1].max()))
                else:
                    polygons.append(Polygon(outer, rings[1:]))
                    groups.append(group)
        if not polygons:
            raise ValueError("Empty CGAZ boundaries; refusing an UNKNOWN bake")
        self.land_index = STRtree(polygons)
        self.land_groups = np.asarray(groups, dtype=object)

    @classmethod
    def from_file(cls, path):
        resolver = cls(json.loads(Path(path).read_text())["features"])
        resolver.assert_alive()
        return resolver

    def assert_alive(self):
        # Independent on-land probes retained from dev1's bake-time liveness gate.
        probes = [(50.087, 14.421, "CZ"), (48.857, 2.352, "FR"),
                  (35.681, 139.767, "JP"), (6.524, 3.379, "NG"),
                  (-23.55, -46.633, "BR"), (-33.868, 151.209, "AU")]
        result = self.resolve([p[0] for p in probes], [p[1] for p in probes])
        for i, (lat, lon, expected) in enumerate(probes):
            if result["country_iso"][i] != int.from_bytes(expected.encode(), "little"):
                raise ValueError(f"CGAZ liveness failed at {lat},{lon}: expected {expected}")

    def _coast_groups(self, latitudes, longitudes):
        if self.coastal_index is None:
            segments, groups = [], []
            for group, ring in self.coastal_rings:
                edges = np.stack((ring[:-1], ring[1:]), axis=1)
                edges = edges[np.abs(edges[:, 1, 0] - edges[:, 0, 0]) <= 180]
                segments.extend(edges)
                groups.extend([group] * len(edges))
            self.coastal_segments = np.asarray(segments)
            self.coastal_groups = np.asarray(groups, dtype=object)
            self.coastal_index = STRtree(np.asarray(shapely.linestrings(self.coastal_segments), dtype=object))
        found = [set() for _ in latitudes]
        scale_x = M_PER_DEG_LON_EQ * np.cos(np.radians(latitudes))
        reach_x = COASTAL_BUFFER_M / np.maximum(np.abs(scale_x), 1.0)
        reach_y = COASTAL_BUFFER_M / M_PER_DEG_LAT
        # Shift search boxes at the seam; stored coast segments stay in [-180,180].
        for shift in (0, -360, 360):
            centers = longitudes + shift
            boxes = shapely.box(centers - reach_x, latitudes - reach_y,
                                centers + reach_x, latitudes + reach_y)
            point_indices, edge_indices = self.coastal_index.query(boxes)
            if not len(point_indices):
                continue
            a, b = self.coastal_segments[edge_indices].transpose(1, 0, 2)
            px = ((longitudes[point_indices] - a[:, 0] + 180) % 360 - 180) * scale_x[point_indices]
            py = (latitudes[point_indices] - a[:, 1]) * M_PER_DEG_LAT
            bx = ((b[:, 0] - a[:, 0] + 180) % 360 - 180) * scale_x[point_indices]
            by = (b[:, 1] - a[:, 1]) * M_PER_DEG_LAT
            lengths = bx * bx + by * by
            t = np.divide(px * bx + py * by, lengths,
                          out=np.zeros_like(lengths), where=lengths > 0).clip(0, 1)
            near = np.hypot(px - t * bx, py - t * by) <= COASTAL_BUFFER_M
            for point, edge in zip(point_indices[near], edge_indices[near]):
                found[point].add(self.coastal_groups[edge])
        return [next(iter(groups)) if len(groups) == 1 else "" for groups in found]

    def _land_groups(self, latitudes, longitudes):
        latitudes = np.asarray(latitudes, dtype=float)
        with np.errstate(invalid="ignore"):
            longitudes = (np.asarray(longitudes, dtype=float) + 180) % 360 - 180
        if latitudes.shape != longitudes.shape or latitudes.ndim != 1:
            raise ValueError("Expected aligned latitude/longitude vectors")
        valid = np.isfinite(latitudes) & np.isfinite(longitudes) & (np.abs(latitudes) <= 90)
        groups = np.full(len(latitudes), "", dtype=object)
        valid_indices = np.flatnonzero(valid)
        points = shapely.points(longitudes[valid], latitudes[valid])
        point_indices, part_indices = self.land_index.query(points, predicate="within")
        groups[valid_indices[point_indices]] = self.land_groups[part_indices]
        for group, polygon, south, min_lat, max_lat in self.polar_parts:
            candidates = np.flatnonzero(valid & (groups == "") & (latitudes >= min_lat) & (latitudes <= max_lat))
            coordinates = polar_coordinates(np.column_stack((longitudes[candidates], latitudes[candidates])), south)
            inside = shapely.contains_xy(polygon, coordinates[:, 0], coordinates[:, 1])
            groups[candidates[inside]] = group
        return latitudes, longitudes, valid, groups

    def resolve_land(self, latitudes, longitudes):
        """Strict original-centroid ownership; no coastal attribution."""
        latitudes, longitudes, _, groups = self._land_groups(latitudes, longitudes)
        # National industrial gates use the canonical ISO feature only; numeric
        # disputed features may have a broader road/admin attribution (e.g. 111).
        groups[np.fromiter((str(group).isdecimal() for group in groups), dtype=bool)] = ""
        return self._geography_at(latitudes, longitudes, groups)

    def resolve(self, latitudes, longitudes):
        latitudes, longitudes, valid, groups = self._land_groups(latitudes, longitudes)
        offshore = np.flatnonzero(valid & (groups == ""))
        if len(offshore):
            groups[offshore] = self._coast_groups(latitudes[offshore], longitudes[offshore])
        return self._geography_at(latitudes, longitudes, groups)

    def _geography_at(self, latitudes, longitudes, groups):
        result = {"country_iso": np.zeros(len(groups), dtype=np.uint16),
                  "city_id": np.zeros(len(groups), dtype=np.uint16),
                  "continent": np.zeros(len(groups), dtype=np.uint8)}
        for group in np.unique(groups):
            if group not in self.countries:
                continue  # Open sea or unmapped disputed land: dev1's explicit UNKNOWN.
            iso, continent = self.countries[group]
            mask = groups == group
            result["country_iso"][mask] = int.from_bytes(iso.encode(), "little")
            result["continent"][mask] = continent
        result["city_id"] = self.city_ids(latitudes, longitudes, result["country_iso"])
        return result

    def city_ids(self, latitudes, longitudes, country_codes):
        latitudes, longitudes = np.asarray(latitudes), np.asarray(longitudes)
        cities = np.zeros(len(latitudes), dtype=np.uint16)
        for metro, polygon in self.metros:
            candidates = np.flatnonzero((country_codes == int.from_bytes(metro["country"].encode(), "little"))
                                        & (cities == 0))
            inside = shapely.contains_xy(polygon, longitudes[candidates], latitudes[candidates])
            cities[candidates[inside]] = metro["id"]
        return cities
