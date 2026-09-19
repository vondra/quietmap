"""Resume fingerprints bind every selected source, including explicit absence."""

from functools import lru_cache
import hashlib
import json
from pathlib import Path


def path_size_and_mtime(path):
    """Size and mtime only: a copy or a restore changes ctime while the bytes stay the same.
    Accepted blind spot: other bytes of the same size with the mtime restored read as unchanged."""
    path = Path(path).resolve()
    try:
        stat = path.stat()
    except FileNotFoundError:
        return (str(path), None)
    return (str(path), stat.st_size, stat.st_mtime_ns)


# The shared GHSL and regional files recur in every square and stay hot; the per-square
# files are asked for once, so a bound loses nothing over a world of squares.
@lru_cache(maxsize=1024)
def content_digest_of_path_size_and_mtime(identity):
    if identity[1] is None:
        return None
    try:
        with open(identity[0], "rb") as source:
            return hashlib.file_digest(source, "sha256").hexdigest()
    except FileNotFoundError:  # removed since its stat: absent, as path_size_and_mtime says
        return None


def structure_input_files(square_dir, overture_files, ghsl, regional):
    return {
        "osm": [str(Path(square_dir) / name) for name in ("buildings.arrow", "barriers.arrow")],
        "overture": list(overture_files),
        "ghsl": list(ghsl.input_files),
        "regional": list(regional.input_files) if regional is not None else None,
    }


def _digest_of_each_file(input_files, describe_file):
    described = {group: None if files is None else [describe_file(file) for file in files]
                 for group, files in input_files.items()}
    # A resume key, not a checksum claiming source completeness or authenticity.
    return hashlib.sha256(json.dumps(described, sort_keys=True).encode()).hexdigest()


def input_fingerprint(input_files):
    return _digest_of_each_file(input_files, path_size_and_mtime)


def input_content_digest(input_files):
    """Path-free, so a byte-identical copy at another path or time is still the same input."""
    return _digest_of_each_file(
        input_files, lambda file: content_digest_of_path_size_and_mtime(path_size_and_mtime(file)))

