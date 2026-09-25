"""ARCO chunk provenance, atomic checkpoints and per-square meteorology windows."""
import base64
import datetime as dt
import io
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import time
import urllib.request
from zoneinfo import TZPATH, ZoneInfo

import numcodecs
import numpy as np

CONTRACT = json.loads((Path(__file__).resolve().parents[2] / 'engine/noise-compute/meteorology-contract.json').read_text())
VARIABLES = ('10m_u_component_of_wind', '10m_v_component_of_wind', '2m_temperature',
             '2m_dewpoint_temperature', 'total_cloud_cover', 'surface_pressure')
ERA5_NODES_PER_DEGREE = 4
ERA5_ROWS, ERA5_COLUMNS = 721, 1440
MET_NODE_BYTES = 240
numcodecs.blosc.set_nthreads(1)


def sha256(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def atomic_json(path, data):
    temporary = path.with_suffix(path.suffix + '.tmp')
    with temporary.open('w') as stream:
        json.dump(data, stream, sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    sync_directory(path.parent)


def timezone_rules(names, retained):
    """Snapshot the actual TZif rules, so resumed years cannot use an upgraded tzdb."""
    path = retained / 'timezone-rules.json'
    if not path.exists():
        rules = {}
        for name in names:
            if name.startswith('/') or '..' in name.split('/'):
                raise ValueError('Invalid timezone name')
            source = next((Path(root) / name for root in TZPATH if (Path(root) / name).is_file()), None)
            if source is None:
                raise ValueError(f'No installed TZif rules for {name}')
            rules[name] = base64.b64encode(source.read_bytes()).decode('ascii')
        atomic_json(path, rules)
    rules = json.loads(path.read_text())
    if set(rules) != set(names):
        raise ValueError('Timezone snapshot does not match node timezone names')
    return [ZoneInfo.from_file(io.BytesIO(base64.b64decode(rules[name])), key=name) for name in names], sha256(path)


def manifest_digest(path, length):
    """Verify the committed prefix without consuming an uncommitted append tail."""
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while length:
            block = stream.read(min(length, 1024 * 1024))
            if not block:
                raise ValueError('Committed chunk manifest was truncated')
            digest.update(block)
            length -= len(block)
    return digest.hexdigest()


def fetch(url):
    for attempt in range(6):
        try:
            with urllib.request.urlopen(url, timeout=120) as response:
                body = response.read()
                return body, dict(url=url, sha256=hashlib.sha256(body).hexdigest(), bytes=len(body),
                    generation=response.headers.get('x-goog-generation'),
                    fetched_utc=dt.datetime.now(dt.timezone.utc).isoformat(), licence='CC-BY-4.0')
        except (OSError, TimeoutError):
            if attempt == 5:
                raise
            time.sleep(min(30, 2 ** attempt))


class Arco:
    def __init__(self, base_url, retained, manifest):
        self.base = base_url.rstrip('/')
        self.manifest = manifest
        path = retained / 'arco-metadata.json'
        if not path.exists():
            body, receipt = fetch(self.base + '/.zmetadata')
            temporary = path.with_suffix('.tmp')
            with temporary.open('wb') as stream:
                stream.write(body)
                stream.flush()
                os.fsync(stream.fileno())
            atomic_json(retained / 'arco-metadata.provenance.json', receipt)
            os.replace(temporary, path)
            sync_directory(retained)
        receipt = json.loads((retained / 'arco-metadata.provenance.json').read_text())
        if receipt['url'] != self.base + '/.zmetadata' or sha256(path) != receipt['sha256']:
            raise ValueError('Retained ARCO metadata identity mismatch')
        self.metadata = json.loads(path.read_text())['metadata']
        for variable in VARIABLES:
            meta = self.metadata[variable + '/.zarray']
            if (meta['chunks'] != [1, 721, 1440] or meta['dtype'] != '<f4'
                    or meta['order'] != 'C' or meta['filters']):
                raise ValueError(f'Unsupported ARCO encoding for {variable}')
        latitude, receipt = self.chunk('latitude', '0')
        self.record(receipt)
        longitude, receipt = self.chunk('longitude', '0')
        self.record(receipt)
        np.testing.assert_array_equal(latitude, 90 - np.arange(721) * .25)
        np.testing.assert_array_equal(longitude, np.arange(1440) * .25)
        # The time coordinate, not the store name, anchors the hour index.
        times, receipt = self.chunk('time', '0')
        self.record(receipt)
        units = self.metadata['time/.zattrs']['units']
        if units != 'hours since 1900-01-01 00:00:00' or not np.all(np.diff(times) == 1):
            raise ValueError('Unsupported ARCO time coordinate')
        self.epoch = dt.datetime(1900, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(hours=int(times[0]))

    def record(self, receipt):
        self.manifest.write((json.dumps(receipt, sort_keys=True) + '\n').encode())

    def chunk(self, variable, key):
        body, receipt = fetch(f'{self.base}/{variable}/{key}')
        meta = self.metadata[variable + '/.zarray']
        decoded = numcodecs.get_codec(meta['compressor']).decode(body)
        data = np.frombuffer(decoded, dtype=meta['dtype'])
        if data.size != np.prod(meta['chunks']) or not np.isfinite(data).all():
            raise ValueError(f'Invalid chunk {variable}/{key}')
        return data, receipt

    def submit_hour(self, timestamp, executor):
        hour = int((timestamp - self.epoch).total_seconds() / 3600)
        return [executor.submit(self.chunk, variable, f'{hour}.0.0') for variable in VARIABLES]

    def collect_hour(self, requests):
        """Receipts enter the manifest only here, in step order, so a prefetched but unconsumed hour never
        reaches a checkpointed manifest."""
        arrays = []
        for request in requests:
            values, receipt = request.result()
            self.record(receipt)
            arrays.append(values)
        return np.array(arrays, dtype=np.float64)


class Checkpoint:
    def __init__(self, directory, identity):
        self.directory = directory
        self.identity = identity
        self.pointer = directory / 'checkpoint.json'

    def load(self):
        if not self.pointer.exists():
            return None, 0, 0
        record = json.loads(self.pointer.read_text())
        if record['identity'] != self.identity:
            raise ValueError('Checkpoint source, method, timezone or code identity changed')
        if manifest_digest(self.directory / 'chunks.jsonl', record['manifest_bytes']) != record['manifest_sha256']:
            raise ValueError('Committed chunk manifest checksum mismatch')
        path = self.directory / record['file']
        if sha256(path) != record['sha256']:
            raise ValueError('Checkpoint checksum mismatch')
        with np.load(path) as saved:
            state = {key: saved[key] for key in saved.files}
        return state, record['next_step'], record['manifest_bytes']

    def save(self, state, next_step, manifest):
        manifest.flush()
        os.fsync(manifest.fileno())
        previous = json.loads(self.pointer.read_text())['file'] if self.pointer.exists() else 'state-1.npz'
        name = 'state-1.npz' if previous == 'state-0.npz' else 'state-0.npz'
        path = self.directory / name
        with path.open('wb') as stream:
            np.savez(stream, **state)
            stream.flush()
            os.fsync(stream.fileno())
        atomic_json(self.pointer, dict(identity=self.identity, file=name, sha256=sha256(path),
                    next_step=next_step, manifest_bytes=manifest.tell(),
                    manifest_sha256=manifest_digest(self.directory / 'chunks.jsonl', manifest.tell())))


def meteorology_magic():
    """File magic naming the contract version, shared with the Rust reader."""
    version = CONTRACT['meteorology_contract']
    if len(version) != 1:
        raise ValueError('Unsupported meteorology contract version')
    return b'qm-met' + version.encode('ascii') + b'\n'


def era5_window(x, y):
    """A square's ERA5 node window as (west, north, rows, columns): the same floor/ceil edge
    bracketing as grid::raster::RasterWindow::for_square_with_density."""
    if not (0 <= x < 512 and 0 <= y < 512):
        raise ValueError('Square out of range')
    longitude_nodes = 360 * ERA5_NODES_PER_DEGREE
    pole_node = 90 * ERA5_NODES_PER_DEGREE
    west = x * longitude_nodes // 512 - longitude_nodes // 2
    east = ((x + 1) * longitude_nodes + 511) // 512 - longitude_nodes // 2
    mercator = math.pi * (1.0 - 2.0 * y / 512)
    edge = math.degrees(math.atan(math.sinh(mercator))) * ERA5_NODES_PER_DEGREE
    north = pole_node if y == 0 else math.ceil(edge)
    mercator = math.pi * (1.0 - 2.0 * (y + 1) / 512)
    edge = math.degrees(math.atan(math.sinh(mercator))) * ERA5_NODES_PER_DEGREE
    south = -pole_node if y == 511 else math.floor(edge)
    return west, north, north - south + 1, east - west + 1


def square_node_indices(x, y):
    """A square's window plus the global row-major ERA5 indices of its nodes, north to south."""
    west, north, rows, columns = era5_window(x, y)
    latitudes = 90 * ERA5_NODES_PER_DEGREE - (north - np.arange(rows)[:, None])
    indices = (latitudes * ERA5_COLUMNS + (west + np.arange(columns)) % ERA5_COLUMNS).ravel()
    return (west, north, rows, columns), indices


def final_nodes(state):
    """Published per-node values from accumulated statistics: u8 percents, f32 moments."""
    if np.any(state['counts'] == 0):
        raise ValueError('Cannot publish a cell with an unobserved period')
    probabilities = np.rint(100 * state['favourable_counts'] / state['counts'][:, :, None]).astype(np.uint8)
    means = state['means'].astype(np.float32)
    variance = (state['m2'] / state['counts'][:, :, None]).astype(np.float32)
    return probabilities, means, variance


def square_file_bytes(window, probabilities, means, variances):
    """A square's file bytes: 16-byte header, then row-major 240-byte node records."""
    west, north, rows, columns = window
    cells = rows * columns
    if (probabilities.shape != (cells, 3, 16) or means.shape != (cells, 3, 8)
            or variances.shape != (cells, 3, 8)):
        raise ValueError('Node arrays do not match the window')
    if probabilities.dtype != np.uint8 or means.dtype != np.float32 or variances.dtype != np.float32:
        raise ValueError('Node arrays must be u8 percents and f32 moments')
    records = np.empty((cells, MET_NODE_BYTES), dtype=np.uint8)
    records[:, :48] = probabilities.reshape(cells, 48)
    records[:, 48:144] = means.astype('<f4').reshape(cells, 24).view(np.uint8)
    records[:, 144:] = variances.astype('<f4').reshape(cells, 24).view(np.uint8)
    return meteorology_magic() + struct.pack('<2h2H', west, north, columns, rows) + records.tobytes()


def all_squares():
    """Every z9 square in hash order: x-major, y-minor."""
    return ((x, y) for x in range(512) for y in range(512))


def write_squares(root, probabilities, means, variances, squares=None):
    """Publish each square's window file; returns (files, bytes, sha256 over the payloads)."""
    digest = hashlib.sha256()
    files = total = 0
    for x, y in all_squares() if squares is None else squares:
        window, indices = square_node_indices(x, y)
        payload = square_file_bytes(window, probabilities[indices], means[indices], variances[indices])
        path = root / 'z9' / str(x) / str(y) / 'meteorology.bin'
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix('.bin.tmp')
        with temporary.open('wb') as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
        digest.update(payload)
        files += 1
        total += len(payload)
    return files, total, digest.hexdigest()


def verify_squares(root, squares=None):
    """Re-read every window file from disk, checking magic, dims and length; same triple."""
    digest = hashlib.sha256()
    files = total = 0
    for x, y in all_squares() if squares is None else squares:
        west, north, rows, columns = era5_window(x, y)
        payload = (root / 'z9' / str(x) / str(y) / 'meteorology.bin').read_bytes()
        if (payload[:8] != meteorology_magic()
                or struct.unpack('<2h2H', payload[8:16]) != (west, north, columns, rows)
                or len(payload) != 16 + rows * columns * MET_NODE_BYTES):
            raise ValueError(f'Window file failed verification: z9/{x}/{y}')
        digest.update(payload)
        files += 1
        total += len(payload)
    return files, total, digest.hexdigest()
