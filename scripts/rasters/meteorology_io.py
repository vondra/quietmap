"""ARCO chunk provenance, atomic checkpoints and the meteorology Arrow contract."""
import base64
import datetime as dt
import io
import hashlib
import json
import os
from pathlib import Path
import time
import urllib.request
from zoneinfo import TZPATH, ZoneInfo

import numcodecs
import numpy as np
import pyarrow as pa
import pyarrow.ipc as ipc

CONTRACT = json.loads((Path(__file__).resolve().parents[2] / 'engine/noise-compute/meteorology-contract.json').read_text())
VARIABLES = ('10m_u_component_of_wind', '10m_v_component_of_wind', '2m_temperature',
             '2m_dewpoint_temperature', 'total_cloud_cover', 'surface_pressure')
PERIODS = ('day', 'evening', 'night')
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

    def hour(self, timestamp, executor):
        hour = int((timestamp - self.epoch).total_seconds() / 3600)
        requests = [executor.submit(self.chunk, variable, f'{hour}.0.0') for variable in VARIABLES]
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


def write_arrow(path, state, indices, metadata):
    """Publish only complete rows, atomically; x/y are ERA5 node indices."""
    if np.any(state['counts'] == 0):
        raise ValueError('Cannot publish a cell with an unobserved period')
    means = state['means'].astype(np.float32)
    variance = (state['m2'] / state['counts'][:, :, None]).astype(np.float32)
    probabilities = np.rint(100 * state['favourable_counts'] / state['counts'][:, :, None]).astype(np.uint8)
    arrays = {'x': pa.array(indices % 1440, type=pa.uint16()),
              'y': pa.array(indices // 1440, type=pa.uint16())}
    for k, period in enumerate(PERIODS):
        arrays['p_' + period] = pa.FixedSizeListArray.from_arrays(pa.array(probabilities[:, k].ravel()), 16)
        arrays['p_max_' + period] = pa.array(probabilities[:, k].max(axis=1))
        for prefix, values in [('alpha_mean_', means), ('alpha_variance_', variance)]:
            arrays[prefix + period] = pa.FixedSizeListArray.from_arrays(pa.array(values[:, k].ravel()), 8)
    table = pa.table(arrays)
    # All columns and fixed-list children are present; readers validate every value.
    schema = pa.schema([pa.field(field.name, field.type, nullable=False) for field in table.schema],
                       metadata={**CONTRACT, **metadata})
    temporary = path.with_suffix('.arrow.tmp')
    with temporary.open('wb') as stream:
        with ipc.new_file(stream, schema) as writer:
            writer.write_table(table.cast(schema), max_chunksize=1440 * 16)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    sync_directory(path.parent)
    return sha256(path)
