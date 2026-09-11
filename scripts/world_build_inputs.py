"""Freeze world-build input identities and attach a complete native raster year."""

import os
from pathlib import Path
import struct
import sys

from prepared_manifest import file_identity, sha256, square_directories

sys.path.insert(0, str(Path(__file__).parent / 'lib'))
import qmgrid


def canonical_input(path):
    path = Path(path)
    resolved = path.resolve(strict=True)
    if 'pre2609' in path.parts or 'pre2609' in resolved.parts:
        raise ValueError(f'forbidden retired source: {path}')
    return resolved


def input_files(roots):
    seen_files = set()
    def walk(path, ancestors):
        path = Path(path).absolute()
        canonical_input(path)
        identity = path.stat()
        if path.is_file():
            if path not in seen_files:
                seen_files.add(path)
                yield path
        elif path.is_dir():
            key = identity.st_dev, identity.st_ino
            if key in ancestors:
                raise ValueError(f'cyclic source directory: {path}')
            for child in sorted(path.iterdir()):
                yield from walk(child, ancestors | {key})
        else:
            raise ValueError(f'not a source file or directory: {path}')
    for root in sorted(map(Path, roots)):
        yield from walk(root, set())


def raster_inputs(source):
    """Every channel file of every z9 square: window bytes or a 0-byte ocean file; missing is an error."""
    source = canonical_input(source)
    for channel in ('dem', 'forest', 'imd'):
        extension = '.i16be' if channel == 'dem' else '.u8'
        for x in range(qmgrid.Z9_AXIS):
            for y in range(qmgrid.Z9_AXIS):
                path = source / qmgrid.square_name(x, y) / (channel + extension)
                if not path.is_file():
                    raise ValueError(f'missing raster: {path}')
                yield path


def height_inputs(path, ancestors=frozenset()):
    path = Path(path).absolute()
    resolved = canonical_input(path)
    if resolved in ancestors:
        raise ValueError(f'cyclic height raster: {path}')
    if path.suffix.lower() == '.vrt':
        import xml.etree.ElementTree as xml
        for source in xml.parse(path).iter('SourceFilename'):
            dependency = Path(source.text)
            if source.get('relativeToVRT') == '1':
                dependency = path.parent / dependency
            yield from height_inputs(dependency, ancestors | {resolved})
    # VRT dependencies are checked before GDAL can open a retired path.
    import rasterio
    with rasterio.open(path) as raster:
        for filename in raster.files:
            canonical_input(filename)
            yield Path(filename).absolute()


def pin_inputs(database, roots):
    database.execute('CREATE TABLE inputs(path TEXT PRIMARY KEY, sha256 BLOB NOT NULL, '
                     'device INTEGER, inode INTEGER, bytes INTEGER, mtime_ns INTEGER, ctime_ns INTEGER)')
    for index, path in enumerate(input_files(roots), 1):
        before = file_identity(path)
        digest = sha256(path)
        if file_identity(path) != before:
            raise ValueError(f'source changed while hashing: {path}')
        database.execute('INSERT INTO inputs VALUES(?,?,?,?,?,?,?)', (str(path), digest, *before))
        if index % 1000 == 0:
            print(f'Pinned {index} input files', flush=True)
    database.commit()


def verify_inputs(database, roots):
    actual = set(map(str, input_files(roots)))
    count = 0
    for path, device, inode, size, mtime, ctime in database.execute(
            'SELECT path,device,inode,bytes,mtime_ns,ctime_ns FROM inputs'):
        if path not in actual or file_identity(Path(path)) != (device, inode, size, mtime, ctime):
            raise ValueError(f'frozen input changed during build: {path}')
        count += 1
    if len(actual) != count:
        raise ValueError('files added to frozen inputs during build')


def attach_rasters(source, prepared):
    source = canonical_input(source)
    for x in range(qmgrid.Z9_AXIS):
        for y in range(qmgrid.Z9_AXIS):
            (prepared / qmgrid.square_name(x, y)).mkdir(parents=True, exist_ok=True)
    for path in raster_inputs(source):
        attached, target = prepared / path.relative_to(source), canonical_input(path)
        # A resumed build finds its own links in place; anything else at the path is foreign.
        if attached.is_symlink() and os.readlink(attached) == str(target):
            continue
        if attached.exists() or attached.is_symlink():
            raise ValueError(f'prepared raster replaced: {attached}')
        attached.symlink_to(target)


def verify_prepared_raster_links(source, prepared):
    source = canonical_input(source)
    for original in raster_inputs(source):
        attached = prepared / original.relative_to(source)
        if canonical_input(attached) != canonical_input(original):
            raise ValueError(f'prepared raster replaced: {attached}')


def audit_world(prepared):
    import pyarrow as pa
    sys.path.insert(0, str(Path(__file__).parent / 'structures'))
    from structure_contract import CONTRACT_KEY, CONTRACT_VERSION
    sys.path.insert(0, str(Path(__file__).parent / 'square-country-city'))
    from build_square_country_city import expected_contract
    counts = {}
    squares = 0
    for square in square_directories(prepared):
        x, y = int(square.parent.name), int(square.name)
        record = (square / 'square-country-city.bin').read_bytes()
        if len(record) != 13 or struct.unpack('<Q', record[:8])[0] != qmgrid.square_id(x, y):
            raise ValueError(f'invalid square-country-city identity: {square}')
        if not (square / 'structures.arrow').is_file():
            raise ValueError(f'unfinished structures: {square}')
        # Pairing with structures.arrow is proven by build-world's zero-write rerun of
        # structures-finalize (a table rewritten after the step makes the rerun write).
        if not (square / 'structures.qoix').is_file():
            raise ValueError(f'unfinished obstacle index: {square}')
        for path in sorted(square.glob('*.arrow')):
            with pa.memory_map(str(path), 'r') as source:
                reader = pa.ipc.open_file(source)
                metadata = reader.schema.metadata or {}
                if path.stem == 'structures' and metadata.get(CONTRACT_KEY.encode()) != CONTRACT_VERSION.encode():
                    raise ValueError(f'invalid structure contract: {path}')
                if path.stem in ('roads', 'railways', 'industrial'):
                    key, value = expected_contract(path)
                    if metadata.get(key) != value:
                        raise ValueError(f'unbaked geography: {path}')
                rows = 0
                for index in range(reader.num_record_batches):
                    batch = reader.get_batch(index)
                    batch.validate(full=True)
                    rows += batch.num_rows
                # The merge's plain chunks carry no z14 envelope; the popup would read
                # the whole table. A 0-row table has nothing to prune and no key.
                if path.stem == 'structures' and rows and b'qm_blocks' not in metadata:
                    raise ValueError(f'unfinished structures blocks: {path}')
                counts[path.stem] = counts.get(path.stem, 0) + rows
        squares += 1
    if squares != qmgrid.Z9_AXIS ** 2:
        raise ValueError(f'incomplete world: {squares} structure squares')
    for layer in ('roads', 'railways', 'structures', 'industrial', 'airborne', 'cruise', 'airport_traffic'):
        if counts.get(layer, 0) == 0:
            raise ValueError(f'no world rows for {layer}')
    return counts
