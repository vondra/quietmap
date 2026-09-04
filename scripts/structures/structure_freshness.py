"""Resume fingerprints bind every selected source, including explicit absence."""

from hashlib import sha256
import json
from pathlib import Path


def file_identity(path):
    path = Path(path).resolve()
    try:
        stat = path.stat()
    except FileNotFoundError:
        return (str(path), None)
    return (str(path), stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns)


def input_fingerprint(square_dir, overture_inputs, ghsl, regional):
    identities = {
        "osm": [file_identity(Path(square_dir) / name)
                for name in ("buildings.arrow", "barriers.arrow")],
        "overture": overture_inputs,
        "ghsl": ghsl.input_identity,
        "regional": regional.input_identity if regional is not None else None,
    }
    # A resume key, not a checksum claiming source completeness or authenticity.
    return sha256(json.dumps(identities, sort_keys=True).encode()).hexdigest()
