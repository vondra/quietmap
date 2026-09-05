"""Completeness failures retain source progress but can never publish a partial monthly TAR."""

from datetime import date, datetime, timezone
import gzip
import importlib.util
import json
from pathlib import Path
import sqlite3
import tarfile
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('adsbe', Path(__file__).with_name('download-adsbexchange.py'))
assert spec and spec.loader
adsbe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adsbe)
DAY = date(2026, 7, 1)
EPOCH = datetime(2026, 7, 1, 12, tzinfo=timezone.utc).timestamp()


def response(path):
    if path.startswith('readsb-hist/'):
        slot = datetime.strptime(path, 'readsb-hist/%Y/%m/%d/%H%M%SZ.json.gz').replace(tzinfo=timezone.utc).timestamp()
        body = {'now': slot - 0.999, 'aircraft': [{'hex': 'a00001', 'lat': 50},
            {'hex': 'a00002', 'lat': 50}, {'hex': '~a00003', 'lat': 50}, {'hex': 'a00004'}]}
        return 200, gzip.compress(json.dumps(body).encode()), 'snapshot-etag', 'modified'
    if path.endswith('a00002.json'):
        return 404, b'absent', None, None
    body = {'timestamp': EPOCH, 'trace': [[0, 50, 14, 1000, 100, 0, 0, 0], [1, 50, 14, 1000, 100, 0, 0, 0]]}
    return 200, json.dumps(body).encode(), 'trace-etag', 'modified'


class DownloadTest(unittest.TestCase):
    def test_readsb_rounded_midnight_endpoint_survives_without_accepting_later_points(self):
        # Final sample pair from the official July 1 a2355d trace:
        # https://samples.adsbexchange.com/traces/2026/07/01/5d/trace_full_a2355d.json
        # Source SHA256: c74eaa1ed3d33aaff8c20ed0ef83293ef8a95cc8042d9ad722c3722b4e7cc374.
        trace = {'icao': 'a2355d', 't': 'E75L', 'timestamp': 1782864000.0, 'trace': [
            [86391.63, 37.585765, -122.296342, 1100, 127.1, 298.2, 0, -704],
            [86400.0, 37.587999, -122.301835, 1025, 127.5, 296.6, 0, -640],
        ]}
        path = 'traces/2026/07/01/5d/trace_full_a2355d.json'
        body = json.dumps(trace).encode()
        self.assertEqual(adsbe.source_json(path, 200, body, DAY), trace)
        self.assertEqual(adsbe.source_json(path, 200, gzip.compress(body), DAY), trace)
        for offset in [-0.01, 86400.01, float('nan'), float('inf')]:
            with self.subTest(offset=offset):
                trace['trace'][-1][0] = offset
                with self.assertRaisesRegex(ValueError, 'trace observation'):
                    adsbe.source_json(path, 200, json.dumps(trace).encode(), DAY)

    def test_snapshot_epoch_belongs_to_its_own_cadence_interval(self):
        path = 'readsb-hist/2026/07/01/000000Z.json.gz'
        slot = datetime(2026, 7, 1, tzinfo=timezone.utc).timestamp()
        for offset in [-0.999, -300, -86400, 1]:
            with self.subTest(offset=offset):
                body = json.dumps({'now': slot + offset, 'aircraft': []}).encode()
                if offset == -0.999:
                    self.assertEqual(adsbe.source_json(path, 200, body, DAY)['now'], slot + offset)
                else:
                    with self.assertRaisesRegex(ValueError, 'snapshot interval'):
                        adsbe.source_json(path, 200, body, DAY)

    def test_complete_union_packs_source_bytes_and_resumes_without_network(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(adsbe, 'fetch', side_effect=response) as fetch:
                adsbe.download_day(DAY, root, 4)
            self.assertEqual(fetch.call_count, 290)
            directory = root / '2026/2026-07-01'
            original = (directory / 'subset.tar').read_bytes()
            with tarfile.open(directory / 'subset.tar') as archive:
                self.assertEqual(archive.getnames(), ['traces/01/trace_full_a00001.json'])
                entry = archive.extractfile(archive.getmembers()[0])
                assert entry is not None
                self.assertEqual(gzip.decompress(entry.read()), response('traces/a00001.json')[1])
            with sqlite3.connect(directory / 'progress.sqlite') as database:
                self.assertEqual(database.execute('SELECT snapshots,traces,absent FROM publication').fetchone(), (288, 1, 1))
                self.assertEqual(database.execute('SELECT count(*) FROM responses').fetchone(), (290,))
            with patch.object(adsbe, 'fetch', side_effect=AssertionError('must use cached responses')):
                adsbe.download_day(DAY, root, 4)
            self.assertEqual((directory / 'subset.tar').read_bytes(), original)

    def test_gzipped_upstream_trace_is_exactly_one_gzip_layer_in_tar(self):
        def compressed(path):
            status, body, etag, modified = response(path)
            if path.startswith('traces/') and status == 200:
                body = gzip.compress(body)
            return status, body, etag, modified
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(adsbe, 'fetch', side_effect=compressed):
                adsbe.download_day(DAY, root, 4)
            with tarfile.open(root / '2026/2026-07-01/subset.tar') as archive:
                entry = archive.extractfile(archive.getmembers()[0])
                assert entry is not None
                payload = gzip.decompress(entry.read())
                self.assertEqual(payload, response('traces/a00001.json')[1])
                self.assertEqual(len(json.loads(payload)['trace']), 2)

    def test_any_snapshot_failure_or_wrong_source_window_blocks_publication(self):
        for failure in [404, 'transient', 'malformed', 'wrong-day', 'schema']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directory = root / '2026/2026-07-01'
                directory.mkdir(parents=True)
                target = directory / 'subset.tar'
                target.write_bytes(b'preserve prior output')
                def broken(path):
                    if path.endswith('000000Z.json.gz'):
                        if failure == 'transient':
                            raise RuntimeError('HTTP503 exhausted')
                        if failure == 404:
                            return 404, b'', None, None
                        body = b'not-json' if failure == 'malformed' else json.dumps({
                            'now': EPOCH - 86400 if failure == 'wrong-day' else EPOCH - 43200 - 0.999,
                            'aircraft': 'invalid' if failure == 'schema' else []}).encode()
                        return 200, body, None, None
                    return response(path)
                with patch.object(adsbe, 'fetch', side_effect=broken):
                    with self.assertRaises(RuntimeError):
                        adsbe.download_day(DAY, root, 4)
                self.assertEqual(target.read_bytes(), b'preserve prior output')
                with sqlite3.connect(directory / 'progress.sqlite') as database:
                    self.assertEqual(database.execute('SELECT count(*) FROM publication').fetchone(), (0,))

    def test_even_one_trace_error_blocks_publication_and_successful_sources_resume(self):
        for failure in ['transient', 'malformed', 'wrong-day']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                def broken(path):
                    if path.endswith('a00001.json'):
                        if failure == 'transient':
                            raise RuntimeError('HTTP503 exhausted')
                        body = b'broken' if failure == 'malformed' else json.dumps({
                            'timestamp': EPOCH, 'trace': [[86400, 0, 0, 0, 0, 0, 0, 0]]}).encode()
                        return 200, body, None, None
                    return response(path)
                with patch.object(adsbe, 'fetch', side_effect=broken):
                    with self.assertRaises(RuntimeError):
                        adsbe.download_day(DAY, root, 4)
                directory = root / '2026/2026-07-01'
                self.assertFalse((directory / 'subset.tar').exists())
                with patch.object(adsbe, 'fetch', side_effect=response) as fetch:
                    adsbe.download_day(DAY, root, 4)
                self.assertEqual(fetch.call_count, 1)

    def test_inflight_bound_and_atomic_failure_preserve_prior_tar(self):
        active = peak = 0
        lock = threading.Lock()
        def bounded(path):
            nonlocal active, peak
            with lock:
                active += 1
                peak = max(peak, active)
            time.sleep(0.001)
            result = response(path)
            with lock:
                active -= 1
            return result
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / '2026/2026-07-01/subset.tar'
            target.parent.mkdir(parents=True)
            target.write_bytes(b'prior')
            with patch.object(adsbe, 'fetch', side_effect=bounded), patch.object(adsbe.os, 'replace', side_effect=OSError('injected rename failure')):
                with self.assertRaises(OSError):
                    adsbe.download_day(DAY, root, 4)
            self.assertLessEqual(peak, 4)
            self.assertGreater(peak, 1)
            self.assertEqual(target.read_bytes(), b'prior')
            with patch.object(adsbe, 'fetch', side_effect=AssertionError('cached')):
                adsbe.download_day(DAY, root, 4)
            self.assertGreater(target.stat().st_size, 1024)

    def test_transient_http_is_never_a_definitive_absence(self):
        class Response:
            status, data, headers = 503, b'error', {}
            def release_conn(self):
                pass
        with patch.object(adsbe.HTTP, 'request', return_value=Response()) as request, patch.object(adsbe.time, 'sleep'):
            with self.assertRaises(RuntimeError):
                adsbe.fetch('traces/test.json')
        self.assertEqual(request.call_count, 5)


if __name__ == '__main__':
    unittest.main()
