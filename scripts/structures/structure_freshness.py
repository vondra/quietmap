"""The resume stamp binds every selected input by its bytes, including explicit absence."""

from functools import lru_cache
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from prepared_manifest import file_identity


def path_file_identity(path):
    """Resolved path plus file_identity: a copy, restore or replacement changes
    device, inode or ctime while the bytes stay the same, so those re-digest and
    only a byte-identical same-file hit skips. Timestamps tick coarsely (200
    rewrites share 3-4 distinct stamps here), so a same-size in-place rewrite
    inside one tick stays invisible; shared sources are frozen inputs and the
    world-build pin fails a run whose inputs moved, so the pin and this cache
    share one identity and one residual."""
    path = Path(path).resolve()
    try:
        return (str(path), *file_identity(path))
    except FileNotFoundError:
        return (str(path), None)


# The shared source files recur in every square and stay hot; they are frozen
# inputs, never rewritten mid-run, so a digest per file identity stays true for
# the life of the process.
@lru_cache(maxsize=1024)
def content_digest_of_file_identity(identity):
    if identity[1] is None:
        return None
    try:
        with open(identity[0], "rb") as source:
            return hashlib.file_digest(source, "sha256").hexdigest()
    except FileNotFoundError:  # removed since its stat: absent, as path_file_identity says
        return None


def structure_input_files(square_dir, overture_files, regional,
                          official_files=None, measured_files=None):
    return {
        "osm": [str(Path(square_dir) / name) for name in ("buildings.arrow", "barriers.arrow")],
        "overture": list(overture_files),
        "regional": list(regional.input_files) if regional is not None else None,
        "official_barriers": None if official_files is None else [str(f) for f in official_files],
        "measured_heights": None if measured_files is None else [str(f) for f in measured_files],
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


def input_content_digest(input_files):
    """Path-free content digest of the inputs as they are now: a byte-identical copy elsewhere is
    the same input. Enrichers rewrite the square's buildings.arrow in place at the same size, and
    file mtimes move in 4 ms ticks (200 same-size rewrites share a handful of stamps), so the
    square's own files are digested every time; shared sources once per file identity."""
    def content(group, file):
        return content_digest_of_file(file) if group == "osm" else \
            content_digest_of_file_identity(path_file_identity(file))

    return _digest_of_each_file(input_files, content)
