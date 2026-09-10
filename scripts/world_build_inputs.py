"""Freeze world-build input identities and attach a complete native raster generation."""

from pathlib import Path
import sqlite3
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
    source = canonical_input(source)
    yield source / 'rasters.sqlite'
    with sqlite3.connect(f'{(source / "rasters.sqlite").as_uri()}?mode=ro', uri=True) as catalog:
        for channel in ('dem', 'forest', 'imd'):
            populated = {row[0] for row in catalog.execute(
                'SELECT square FROM raster_squares WHERE channel=? AND sha256 IS NOT NULL', (channel,))}
            if not populated <= set(range(qmgrid.Z9_AXIS ** 2)):
                raise ValueError('invalid raster catalog square')
            extension = '.i16be' if channel == 'dem' else '.u8'
            for x in range(qmgrid.Z9_AXIS):
                for y in range(qmgrid.Z9_AXIS):
                    if qmgrid.square_id(x, y) in populated:
                        yield source / qmgrid.square_name(x, y) / (channel + extension)


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


def attach_rasters(source, prepared, database):
    source = canonical_input(source)
    # Native contracts remain owned by the Rust reader; import its literal pin.
    channel_source = Path(__file__).parents[1] / 'engine/raster-reader/src/channel.rs'
    import re
    contract = re.search(r'pub const CONTRACT: &str = "([^"]+)";', channel_source.read_text()).group(1)
    catalog = sqlite3.connect(f'{(source / "rasters.sqlite").as_uri()}?mode=ro', uri=True)
    channel_rows = catalog.execute('SELECT channel,contract,source_identity FROM raster_channels').fetchall()
    channels = {row[0]: row[1] for row in channel_rows}
    if (len(channel_rows) != 3 or channels != {channel: contract for channel in ('dem', 'forest', 'imd')}
            or any(not isinstance(identity, str) or not re.fullmatch(r'[0-9a-fA-F]{64}', identity)
                   for _, _, identity in channel_rows)):
        raise ValueError('native raster channel contract mismatch')
    for x in range(qmgrid.Z9_AXIS):
        for y in range(qmgrid.Z9_AXIS):
            (prepared / qmgrid.square_name(x, y)).mkdir(parents=True, exist_ok=True)
    for channel in channels:
        entries = catalog.execute('SELECT square,sha256 FROM raster_squares WHERE channel=?', (channel,)).fetchall()
        rows = dict(entries)
        if len(entries) != qmgrid.Z9_AXIS ** 2 or set(rows) != set(range(qmgrid.Z9_AXIS ** 2)):
            raise ValueError(f'incomplete world raster coverage: {channel}')
        filename = channel + ('.i16be' if channel == 'dem' else '.u8')
        for x in range(qmgrid.Z9_AXIS):
            for y in range(qmgrid.Z9_AXIS):
                relative = Path(qmgrid.square_name(x, y)) / filename
                path = source / relative
                digest = rows[qmgrid.square_id(x, y)]
                if digest is None:
                    if path.exists() or path.is_symlink():
                        raise ValueError(f'ocean declaration has a raster file: {path}')
                    continue
                pinned = database.execute('SELECT sha256 FROM inputs WHERE path=?',
                                          (str(path),)).fetchone()
                if not pinned or pinned[0] != digest:
                    raise ValueError(f'raster bytes differ from catalog: {path}')
                (prepared / relative).symlink_to(canonical_input(path))
    catalog.close()
    (prepared / 'rasters.sqlite').symlink_to(source / 'rasters.sqlite')



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
                for index in range(reader.num_record_batches):
                    batch = reader.get_batch(index)
                    batch.validate(full=True)
                    counts[path.stem] = counts.get(path.stem, 0) + batch.num_rows
        squares += 1
    if squares != qmgrid.Z9_AXIS ** 2:
        raise ValueError(f'incomplete world: {squares} structure squares')
    for layer in ('roads', 'railways', 'structures', 'industrial', 'airborne', 'cruise', 'airport_traffic'):
        if counts.get(layer, 0) == 0:
            raise ValueError(f'no world rows for {layer}')
    return counts
