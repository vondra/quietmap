"""Publisher identities, archive completeness and preserved-source acquisition regressions."""

from datetime import date
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('download_adsblol', Path(__file__).with_name('download-adsblol.py'))
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def release_fixture(day='2026-06-06', suffixes=('.aa', '.ab'), kind='staging'):
    tag = f'v{day.replace("-", ".")}-planes-readsb-{kind}-0'
    assets = []
    for suffix in suffixes:
        name = tag + '.tar' + suffix
        assets.append({'name': name, 'state': 'uploaded', 'size': 1024,
                       'digest': 'sha256:' + '1' * 64,
                       'browser_download_url': f'https://github.com/adsblol/globe_history_2026/releases/download/{tag}/{name}'})
    release = {'tag_name': tag, 'draft': False, 'assets': assets}
    urls = [asset['browser_download_url'] for asset in assets]
    return urls, {url: (release, asset) for url, asset in zip(urls, assets)}


def create_selected_catalog(output, days=('2026-06-06',), kind='staging', mlat_days=()):
    """Small valid native TARs and preserved publisher responses, with no network."""
    output = Path(output)
    exports = []
    for index, day in enumerate(days):
        urls, records = release_fixture(day=day, suffixes=('',), kind='mlatonly' if day in mlat_days else kind)
        release, asset = records[urls[0]]
        body = bytes(1024 + 512 * index)
        asset['size'] = len(body)
        asset['digest'] = 'sha256:' + hashlib.sha256(body).hexdigest()
        exports.append((day, urls, records, release, body))
    year = days[0][:4]
    responses = {
        MODULE.API + f'{year}/commits/main': json.dumps({'sha': 'a' * 40}).encode(),
        MODULE.RAW + f'{year}/{"a" * 40}/PREFERRED_RELEASES.txt': '\n'.join(row[1][0] for row in exports).encode(),
        MODULE.API + f'{year}/releases?per_page=100&page=1': json.dumps([row[3] for row in exports]).encode(),
    }
    with patch.object(MODULE, 'request', side_effect=lambda url: io.BytesIO(responses[url])):
        with MODULE.open_catalog(output, tuple(date.fromisoformat(day) for day in days)) as database:
            for day, urls, records, _, body in exports:
                chosen = MODULE.release_assets(day, urls, records)[0]
                source = MODULE.asset_target_path(chosen, output)
                source.parent.mkdir(parents=True)
                source.write_bytes(body)
                database.execute('INSERT INTO verified VALUES (?,?,?,?,?,?,?)',
                                 (str(source), chosen[4], *MODULE.identity(source)))
    return source


class PublisherIntegrity(unittest.TestCase):
    def test_preferred_catalog_pins_commit_not_tree_and_preserves_year_authority(self):
        urls, releases = release_fixture(day='2025-12-30', suffixes=('',))
        release = releases[urls[0]][0]
        release['assets'][0]['browser_download_url'] = urls[0].replace('globe_history_2026', 'globe_history_2025')
        url = release['assets'][0]['browser_download_url']
        commit_sha, tree_sha = 'a' * 40, 'b' * 40
        responses = {
            MODULE.API + '2025/commits/main': json.dumps({'sha': commit_sha, 'commit': {'tree': {'sha': tree_sha}}}).encode(),
            MODULE.RAW + f'2025/{commit_sha}/PREFERRED_RELEASES.txt': url.encode(),
            MODULE.API + '2025/releases?per_page=100&page=1': json.dumps([release]).encode(),
        }
        with patch.object(MODULE, 'response', side_effect=lambda _, address: responses[address]):
            selected, records = MODULE.preferred_releases(None, {date(2025, 12, 30)})
        self.assertEqual(selected, {'2025-12-30': [url]})
        self.assertEqual(records[url][1]['size'], 1024)
        rollover_urls, rollover_records = release_fixture(day='2025-12-31', suffixes=('',))
        duplicate_urls, duplicate_records = release_fixture(day='2025-12-30', suffixes=('',))
        responses.update({
            MODULE.API + '2026/commits/main': json.dumps({'sha': 'c' * 40}).encode(),
            MODULE.RAW + f'2026/{"c" * 40}/PREFERRED_RELEASES.txt': (duplicate_urls[0] + '\n' + rollover_urls[0]).encode(),
            MODULE.API + '2026/releases?per_page=100&page=1': json.dumps([
                duplicate_records[duplicate_urls[0]][0], rollover_records[rollover_urls[0]][0]]).encode(),
        })
        both_days = {date(2025, 12, 30), date(2025, 12, 31)}
        with patch.object(MODULE, 'response', side_effect=lambda _, address: responses[address]):
            selected, _ = MODULE.preferred_releases(None, both_days)
        self.assertEqual(selected, {'2025-12-30': [url], '2025-12-31': rollover_urls})
        # If the original repository publishes the day, a missing preferred entry
        # must not silently select a different export from the next repository.
        responses[MODULE.API + '2025/releases?per_page=100&page=1'] = json.dumps([
            release, rollover_records[rollover_urls[0]][0]]).encode()
        with patch.object(MODULE, 'response', side_effect=lambda _, address: responses[address]):
            with self.assertRaisesRegex(ValueError, 'original repository'):
                MODULE.preferred_releases(None, both_days)

    def test_official_complete_whole_or_split_exports_and_malformed_identities(self):
        for suffixes in [('',), ('.aa', '.ab'), ('', '.aa', '.ab')]:
            with self.subTest(suffixes=suffixes):
                urls, records = release_fixture(suffixes=suffixes)
                self.assertEqual(len(MODULE.release_assets('2026-06-06', urls, records)), len(suffixes))
        for suffixes in [('.ab',), ('.aa', '.ac')]:
            urls, records = release_fixture(suffixes=suffixes)
            with self.assertRaisesRegex(ValueError, 'missing split'):
                MODULE.release_assets('2026-06-06', urls, records)
        for field, bad in [('digest', None), ('digest', 'sha256:bad'), ('size', 0), ('state', 'new'), ('name', '../escape.tar')]:
            with self.subTest(field=field):
                urls, records = release_fixture()
                records[urls[0]][1][field] = bad
                with self.assertRaises(ValueError):
                    MODULE.release_assets('2026-06-06', urls, records)
        urls, records = release_fixture()
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            MODULE.release_assets('2026-06-06', urls[:1], records)
        urls, records = release_fixture(day='2026-05-06', suffixes=('',), kind='mlatonly')
        self.assertIn('mlatonly', MODULE.release_assets('2026-05-06', urls, records)[0][5])

    def test_source_acquisition_is_atomic_hash_verified_and_never_overwrites_history(self):
        body = b'exact official archive bytes' * 100
        asset = ('2026-06-06', 'source.tar', 'https://github.com/asset', len(body), hashlib.sha256(body).hexdigest(), 'prod')
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch.object(MODULE, 'request', return_value=io.BytesIO(body)):
                result = MODULE.acquire(asset, output, 0)
            target = Path(result[0])
            self.assertEqual(target.read_bytes(), body)
            self.assertEqual(result[2:], MODULE.identity(target))
            with patch.object(MODULE, 'request', side_effect=AssertionError('must reuse valid complete bytes')):
                self.assertEqual(MODULE.acquire(asset, output, 0), result)
            target.write_bytes(b'preserved historical difference')
            with self.assertRaisesRegex(ValueError, 'retained unchanged'):
                MODULE.acquire(asset, output, 0)
            self.assertEqual(target.read_bytes(), b'preserved historical difference')
            second = tuple('other.tar' if i == 1 else value for i, value in enumerate(asset))
            with patch.object(MODULE, 'request', return_value=io.BytesIO(b'truncated')):
                with self.assertRaisesRegex(ValueError, 'SHA256 mismatch'):
                    MODULE.acquire(second, output, 0)
            self.assertFalse((target.parent / 'other.tar').exists())
            self.assertFalse(list(target.parent.glob('.download-*')))
            with patch.object(MODULE, 'request', return_value=io.BytesIO(body)):
                with self.assertRaisesRegex(ValueError, 'reserve'):
                    MODULE.acquire(second, output, 2**63)
            self.assertFalse((target.parent / 'other.tar').exists())

    def test_catalog_and_external_receipt_reject_changed_sources_before_reuse(self):
        days = (date(2026, 6, 6),)
        urls, releases = release_fixture(suffixes=('',))
        with tempfile.TemporaryDirectory() as directory, patch.object(MODULE, 'preferred_releases', return_value=({'2026-06-06': urls}, releases)):
            output = Path(directory)
            with MODULE.open_catalog(output, days) as database:
                source = output / 'verified-original'
                source.write_bytes(b'original')
                digest = hashlib.sha256(source.read_bytes()).hexdigest()
                record = (str(source), digest, *MODULE.identity(source))
                receipt = output / 'receipt.sqlite'
                with sqlite3.connect(receipt) as independent:
                    independent.execute('CREATE TABLE verified (path,sha256,dev,ino,size,mtime_ns,ctime_ns)')
                    independent.execute('INSERT INTO verified VALUES (?,?,?,?,?,?,?)', record)
                MODULE.import_verified(database, receipt)
                asset = ('2026-06-06', 'original.tar', 'unused', 8, digest, 'prod')
                self.assertEqual(MODULE.verified_asset(database, asset), str(source))
                source.write_bytes(b'changed!')
                with self.assertRaisesRegex(ValueError, 'changed verified'):
                    MODULE.verified_asset(database, asset)
                with self.assertRaisesRegex(ValueError, 'changed independently'):
                    MODULE.import_verified(database, receipt)
            with MODULE.open_catalog(output, days) as database:
                database.execute('UPDATE assets SET size=42')
            with self.assertRaisesRegex(ValueError, 'preserved official'):
                MODULE.open_catalog(output, days)
            with self.assertRaisesRegex(ValueError, 'different observation'):
                MODULE.open_catalog(output, (date(2026, 6, 7),))

    def test_failed_catalog_bootstrap_can_retry_without_losing_a_valid_catalog(self):
        urls, records = release_fixture(suffixes=('',))
        available = False

        def official_response(url):
            if url.endswith('/commits/main'):
                body = json.dumps({'sha': 'a' * 40}).encode()
            elif url.endswith('/PREFERRED_RELEASES.txt'):
                body = urls[0].encode() if available and 'globe_history_2026/' in url else b''
            else:
                body = json.dumps([records[urls[0]][0]] if available and 'globe_history_2026/' in url else []).encode()
            return io.BytesIO(body)

        with tempfile.TemporaryDirectory() as directory, patch.object(MODULE, 'request', side_effect=official_response):
            output, days = Path(directory), (date(2026, 6, 6),)
            with self.assertRaisesRegex(ValueError, 'official release missing'):
                MODULE.open_catalog(output, days)
            available = True
            with MODULE.open_catalog(output, days) as database:
                expected = database.execute('SELECT * FROM responses ORDER BY url').fetchall()
                self.assertEqual(database.execute('SELECT count(*) FROM assets').fetchone(), (1,))
            with patch.object(MODULE, 'request', side_effect=AssertionError('valid catalog must stay pinned')):
                with MODULE.open_catalog(output, days) as database:
                    self.assertEqual(database.execute('SELECT * FROM responses ORDER BY url').fetchall(), expected)

    def test_resume_published_archive_without_receipt_needs_no_additional_disk(self):
        urls, records = release_fixture(suffixes=('',))
        body = b'a' * 1024
        records[urls[0]][1]['digest'] = 'sha256:' + hashlib.sha256(body).hexdigest()
        with tempfile.TemporaryDirectory() as directory, patch.object(MODULE, 'preferred_releases', return_value=({'2026-06-06': urls}, records)):
            output = Path(directory)
            database = MODULE.open_catalog(output, (date(2026, 6, 6),))
            asset = MODULE.release_assets('2026-06-06', urls, records)[0]
            with patch.object(MODULE, 'request', return_value=io.BytesIO(body)):
                result = MODULE.acquire(asset, output, 0)
            before = MODULE.identity(Path(result[0]))
            with patch.object(MODULE, 'open_catalog', return_value=database), \
                    patch.object(MODULE.shutil, 'disk_usage', return_value=MODULE.shutil._ntuple_diskusage(100, 0, 100)), \
                    patch.object(MODULE, 'request', side_effect=AssertionError('published bytes need no download')), \
                    patch('sys.argv', ['download-adsblol.py', '--anchor', '2026-09', '--out', str(output), '--reserve-bytes', '100']):
                MODULE.main()
            self.assertEqual(MODULE.identity(Path(result[0])), before)
            self.assertEqual(database.execute('SELECT * FROM verified').fetchall(), [result])


class SelectedSourceReuse(unittest.TestCase):
    def test_read_only_selector_checks_only_requested_dates_and_never_fetches(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = create_selected_catalog(root)
            before = MODULE.identity(source)
            with patch.object(MODULE, 'request', side_effect=AssertionError('validator must not fetch')):
                selected = MODULE.validate_selected_sources(root, {'2026-06-06'})
                self.assertEqual(selected[0][1], str(source))
                with sqlite3.connect(root / 'catalog.sqlite') as db:
                    db.execute('DELETE FROM responses')
                with self.assertRaisesRegex(ValueError, 'missing preserved'):
                    MODULE.validate_selected_sources(root, {'2026-06-06'})
            self.assertEqual(before, MODULE.identity(source))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            create_selected_catalog(root, days=('2026-05-06',), kind='mlatonly')
            with self.assertRaisesRegex(ValueError, 'no complete source days.*MLAT-only'):
                MODULE.validate_selected_sources(root, {'2026-05-06'})

    def test_only_catalog_mlat_days_are_omitted_not_missing_full_source_assets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            days = ('2026-06-06', '2026-06-07', '2026-06-08')
            create_selected_catalog(root, days=days, mlat_days=(days[1],))
            selected = MODULE.validate_selected_sources(root, set(days))
            self.assertEqual([row[0][0] for row in selected], [days[0], days[2]])
            with sqlite3.connect(root / 'catalog.sqlite') as database:
                self.assertEqual(database.execute('SELECT COUNT(*) FROM window').fetchone()[0], 3)
            Path(selected[1][1]).unlink()
            with self.assertRaises(FileNotFoundError):
                MODULE.validate_selected_sources(root, set(days))
            with self.assertRaisesRegex(ValueError, 'outside the selected source catalog'):
                MODULE.validate_selected_sources(root, {'2026-06-09'})

    def test_empty_outputs_require_completed_matching_source_and_parent_receipts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = create_selected_catalog(root / 'source')
            selected = MODULE.validate_selected_sources(root / 'source', {'2026-06-06'})
            work = root / 'work'
            (work / 'flights').mkdir(parents=True)
            (work / 'segments').mkdir()
            flights = work / 'flights/2026-06-06.arrow'
            segments = work / 'segments/2026-06-06.arrow'
            flights.write_bytes(b'empty typed output tested by native IPC regression')
            def receipt(stage, action, chosen=selected, filter='ga'):
                return MODULE.source_receipt(work, chosen, stage, filter, action)
            with self.assertRaises(sqlite3.OperationalError):
                receipt('flights', 'check')
            receipt('flights', 'begin')
            with self.assertRaisesRegex(ValueError, 'completed Stage0 receipt'):
                receipt('flights', 'check')
            receipt('flights', 'complete')
            receipt('flights', 'check')
            with self.assertRaisesRegex(ValueError, 'feed/class differs'):
                receipt('flights', 'check', filter='non-ga')
            receipt('segments', 'begin')
            segments.write_bytes(b'new empty typed segment output')
            receipt('segments', 'complete')
            receipt('segments', 'check')
            other = create_selected_catalog(root / 'other', kind='prod')
            changed = MODULE.validate_selected_sources(root / 'other', {'2026-06-06'})
            with self.assertRaisesRegex(ValueError, 'source/feed/class differs'):
                receipt('segments', 'check', chosen=changed)
            self.assertTrue(other.exists())
            receipt('segments', 'begin')
            flights.write_bytes(b'changed parent')
            with self.assertRaisesRegex(ValueError, 'changed completed flights'):
                receipt('segments', 'complete')
            with self.assertRaisesRegex(ValueError, 'changed completed flights'):
                receipt('segments', 'check')
            receipt('flights', 'begin')
            source.write_bytes(bytes(2048))
            changed_stat = [(asset, path, MODULE.identity(Path(path))) for asset, path, _ in selected]
            with self.assertRaisesRegex(ValueError, 'changed since flights began'):
                receipt('flights', 'complete', chosen=changed_stat)


if __name__ == '__main__':
    unittest.main()
