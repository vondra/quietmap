"""Freeze world-build input identities and attach a complete native raster year."""

from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile

from prepared_manifest import file_identity, served_layer_files, square_directories

sys.path.insert(0, str(Path(__file__).parent / 'lib'))
import qmgrid
from worker_jobs import cpu_jobs, fit_jobs


def canonical_input(path):
    path = Path(path)
    resolved = path.resolve(strict=True)
    if 'pre2609' in path.parts or 'pre2609' in resolved.parts:
        raise ValueError(f'forbidden retired source: {path}')
    return resolved


def source_paths(config):
    required = {'planet', 'rasters', 'enrichment', 'boundaries', 'city_boundaries',
                'overture', 'regional_heights', 'official_barriers', 'measured_heights',
                'aircraft_primary', 'aircraft_secondary', 'ships', 'ships_gfw'}
    if set(config['sources']) != required:
        raise ValueError(f'sources must be exactly {sorted(required)}')
    return {name: canonical_input(path) for name, path in config['sources'].items()}


def source_family_roots(sources):
    """Source family -> the roots the pin and the freeze walk: whole trees, or exact raster and height files."""
    return {name: list(raster_inputs(path)) if name == 'rasters'
            else list(dict.fromkeys(height_inputs(path))) if name == 'regional_heights'
            else [path] for name, path in sources.items()}


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
    for channel in ('dem', 'canopy', 'forest', 'imd'):
        extension = '.u16le' if channel == 'dem' else '.u8'
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


def pin_inputs(path, roots):
    """Record device identity of every frozen input. Bytes are not hashed: verify re-stats."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f'.{path.name}.', dir=path.parent)
    try:
        with os.fdopen(descriptor, 'w', encoding='utf-8') as output:
            for index, source in enumerate(input_files(roots), 1):
                device, inode, size, mtime_ns, ctime_ns = file_identity(source)
                output.write(json.dumps({
                    'path': str(source), 'device': device, 'inode': inode,
                    'bytes': size, 'mtime_ns': mtime_ns, 'ctime_ns': ctime_ns,
                }, sort_keys=True) + '\n')
                if index % 1000 == 0:
                    print(f'Pinned {index} input files', flush=True)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def load_pin(path):
    rows = []
    with Path(path).open(encoding='utf-8') as source:
        for line in source:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def pin_digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


@contextmanager
def repin_inputs(path, roots, frozen_roots):
    """Prepare a candidate pin and name what changed; the caller publishes it after durable receipt invalidation.

    Yields the candidate path, every changed input and the subset under frozen source
    roots. A changed or added source is evidence like a code change: the reviewed pin
    decides which completed steps survive, so a source refreshed after completion (new
    counters, a new timetable) rebuilds only its consumers instead of blocking every resume.
    """
    previous = {row['path']: row for row in load_pin(path)} if path.exists() else {}
    if not previous:
        raise ValueError('cannot resume without the previous input pin; retained work needs inspection')
    candidate = path.with_name('.' + path.name + '.next')
    try:
        pin_inputs(candidate, roots)
        current = {row['path']: row for row in load_pin(candidate)}
        changed = sorted(name for name in previous.keys() | current.keys()
                         if previous.get(name) != current.get(name))
        frozen = {Path(root).absolute() for root in frozen_roots}
        sources = [name for name in changed
                   if any(parent in frozen for parent in (Path(name), *Path(name).parents))]
        yield candidate, changed, sources
    finally:
        candidate.unlink(missing_ok=True)


def verify_inputs(path, roots):
    actual = set(map(str, input_files(roots)))
    count = 0
    for row in load_pin(path):
        pinned = row['path']
        identity = (row['device'], row['inode'], row['bytes'], row['mtime_ns'], row['ctime_ns'])
        if pinned not in actual or file_identity(Path(pinned)) != identity:
            raise ValueError(f'frozen input changed during build: {pinned}')
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
        if attached.is_symlink() and attached.readlink() == target:
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


def stamps_the_point_query_expects():
    """layer -> {metadata key: value}. The point query answers another stamp by serving without
    the layer (structures: by refusing), so this audit is the gate that keeps such a file out of a
    release. Most stamps are read from the reader's own constants; the structures builder stamp is
    read from its producer, because the reader cannot see it — a builder change that keeps the
    schema keeps the contract, so only this audit tells a rebuilt square from a stale one."""
    reader = Path(__file__).resolve().parent.parent / 'engine/square-store/src'
    def constants(file):
        return {name: value.encode() for name, value in
                re.findall(r'pub const (\w+): &str = "([^"]*)";', (reader / file).read_text())}
    store, aircraft = constants('store.rs'), constants('aircraft_contract.rs')
    osm = constants('osm_contract.rs')
    grid = {b'grid': store['GRID_CONTRACT_Z30']}
    version = {b'schema_version': aircraft['SCHEMA_VERSION']}
    builder = re.search(r'BUILDER_VERSION = "([^"]+)"',
                        (Path(__file__).parent / 'structures/structure_merge.py').read_text())
    return {
        'structures': {b'builder_version': builder.group(1).encode(), **grid},
        'roads': {b'osm_roads_contract': osm['ROADS_CONTRACT'], **grid},
        'railways': {b'osm_railways_contract': osm['RAILWAYS_CONTRACT'], **grid},
        'industrial': {b'osm_industrial_contract': osm['INDUSTRIAL_CONTRACT'], **grid},
        'leisure': {b'leisure_contract': osm['LEISURE_CONTRACT_V5'], **grid},
        'ships': {b'ships_contract': store['SHIPS_CONTRACT_V1'], **grid},
        'airborne': {b'airborne_contract': aircraft['AIRBORNE_CONTRACT'], **version},
        'cruise': {b'cruise_contract': aircraft['CRUISE_CONTRACT'], **version},
        'airport_traffic': {b'airport_traffic_contract': aircraft['AIRPORT_TRAFFIC_CONTRACT'],
                            **version},
    }, aircraft['AIRPORT_SUMMARIES_KEY']


def audit_world(prepared, jobs=None):
    import pyarrow as pa
    sys.path.insert(0, str(Path(__file__).parent / 'structures'))
    from structure_contract import CONTRACT_KEY, CONTRACT_VERSION
    sys.path.insert(0, str(Path(__file__).parent / 'square-country-city'))
    from build_square_country_city import expected_contract
    expected_stamps, airport_summaries_key = stamps_the_point_query_expects()
    def audit_square(square):
        counts = {}
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
        for path in served_layer_files(square):
            with pa.memory_map(str(path), 'r') as source:
                reader = pa.ipc.open_file(source)
                metadata = reader.schema.metadata or {}
                if path.stem == 'structures' and metadata.get(CONTRACT_KEY.encode()) != CONTRACT_VERSION.encode():
                    raise ValueError(f'invalid structure contract: {path}')
                traffic_contract = {'roads': b'road_traffic_contract', 'railways': b'rail_traffic_contract'}.get(path.stem)
                if traffic_contract and metadata.get(traffic_contract) != b'1':
                    raise ValueError(f'unfinished {path.stem} traffic: {path}')
                if path.stem in ('roads', 'railways', 'industrial'):
                    key, value = expected_contract(path)
                    if metadata.get(key) != value:
                        raise ValueError(f'unbaked geography: {path}')
                for key, value in expected_stamps.get(path.stem, {}).items():
                    if metadata.get(key) != value:
                        raise ValueError(f'stale {path.stem} stamp {key.decode()}={metadata.get(key)}, the point query expects {value.decode()}: {path}')
                if path.stem in ('airborne', 'cruise', 'airport_traffic'):
                    days = [metadata.get(key, b'') for key in (b'baseline_days', b'increment_days')]
                    if not (all(count.isdigit() and int(count) < 1 << 16 for count in days) and int(days[0]) > 0):
                        raise ValueError(f'invalid sampling window baseline/increment days {days}: {path}')
                # Its agreement across cells is Stage 2C's own reduce; the stamp proves it ran.
                if path.stem == 'airport_traffic' and airport_summaries_key not in metadata:
                    raise ValueError(f'airport traffic without Stage 2C summaries: {path}')
                rows = 0
                for index in range(reader.num_record_batches):
                    batch = reader.get_batch(index)
                    batch.validate(full=True)
                    rows += batch.num_rows
                # The merge's plain chunks carry no z14 envelope; the popup would read
                # the whole table. A 0-row table has nothing to prune and no key.
                if path.stem in ('structures', 'roads', 'railways', 'ships') and rows and b'qm_blocks' not in metadata:
                    raise ValueError(f'unfinished {path.stem} blocks: {path}')
                counts[path.stem] = counts.get(path.stem, 0) + rows
        return counts

    counts, squares = {}, 0
    # Each worker holds one mapped batch; reserve 1 GiB for decompression and validation.
    jobs = fit_jobs(cpu_jobs() if jobs is None else jobs, 1 << 30)
    directories = iter(square_directories(prepared))
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        pending = {pool.submit(audit_square, square) for _, square in zip(range(jobs), directories)}
        while pending:
            done, pending = wait(pending, return_when=FIRST_COMPLETED)
            for future in done:
                for layer, rows in future.result().items():
                    counts[layer] = counts.get(layer, 0) + rows
                squares += 1
                square = next(directories, None)
                if square is not None:
                    pending.add(pool.submit(audit_square, square))
    if squares != qmgrid.Z9_AXIS ** 2:
        raise ValueError(f'incomplete world: {squares} structure squares')
    for layer in ('roads', 'railways', 'structures', 'industrial', 'airborne', 'cruise',
                  'airport_traffic', 'ships', 'leisure'):
        if counts.get(layer, 0) == 0:
            raise ValueError(f'no world rows for {layer}')
    return counts
