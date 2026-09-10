"""Resolve a batch of GEM point coordinates through the existing strict CGAZ index."""

import json
import sys

from admin_at import AdminResolver


def main():
    points = json.load(sys.stdin)
    resolver = AdminResolver.from_file(sys.argv[1])
    result = resolver.resolve_land([p[0] for p in points], [p[1] for p in points])
    json.dump(result["country_iso"].tolist(), sys.stdout)


if __name__ == "__main__":
    main()
