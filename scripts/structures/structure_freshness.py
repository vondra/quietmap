"""Resume stamps bind every selected input, including explicit absence; prepared per-square
inputs by their bytes."""

from functools import lru_cache
import hashlib
import json
from pathlib import Path


def path_size_and_mtime(path):
    """Size and mtime only: a copy or a restore changes ctime while the bytes stay the same.
    Only retained source files, never rewritten in place, are identified this way."""
    path = Path(path).resolve()
    try:
        stat = path.stat()
    except FileNotFoundError:
        return (str(path), None)
    return (str(path), stat.st_size, stat.st_mtime_ns)


# The shared source files recur in every square and stay hot; they are never rewritten, so a
# digest per (path, size, mtime) stays true for the life of the process.
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


def content_digest_of_file(path):
    try:
        with open(path, "rb") as source:
            return hashlib.file_digest(source, "sha256").hexdigest()
    except FileNotFoundError:
        return None


def _digest_of_each_file(input_files, describe_file):
    described = {group: None if files is None else [describe_file(group, file) for file in files]
                 for group, files in input_files.items()}
    # A resume key, not a checksum claiming source completeness or authenticity.
    return hashlib.sha256(json.dumps(described, sort_keys=True).encode()).hexdigest()


def input_stamps(input_files):
    """(fingerprint, content digest) of the inputs as they are now. Enrichers rewrite the square's
    buildings.arrow in place at the same size, and file mtimes move in 4 ms ticks (2026-09-24:
    200 same-size rewrites left 14 distinct mtimes), so size and mtime cannot tell those bytes
    apart: both stamps identify them by content. The fingerprint binds sources by path, size and
    mtime; the content digest is path-free, so a byte-identical copy elsewhere is the same input."""
    square_files = {file: content_digest_of_file(file) for file in input_files["osm"]}

    def fingerprint(group, file):
        return square_files[file] if group == "osm" else path_size_and_mtime(file)

    def content(group, file):
        return square_files[file] if group == "osm" else \
            content_digest_of_path_size_and_mtime(path_size_and_mtime(file))

    return _digest_of_each_file(input_files, fingerprint), _digest_of_each_file(input_files, content)
