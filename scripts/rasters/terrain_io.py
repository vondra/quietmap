"""Bounded provider downloads and durable per-file provenance for terrain inputs."""
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request

REQUIRED_PROVENANCE = {'url', 'fetched_utc', 'sha256', 'bytes', 'licence',
                       'licence_url', 'terms_checked_utc'}
MAX_DOWNLOAD_BYTES = 1_500_000_000_000


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def provenance(path, verify_digest=True):
    path = Path(path)
    record = json.loads(Path(str(path) + '.provenance.json').read_text())
    if not REQUIRED_PROVENANCE <= record.keys() or any(not record[k] for k in REQUIRED_PROVENANCE):
        raise ValueError(f'incomplete provenance: {path}')
    if record['bytes'] != path.stat().st_size:
        raise ValueError(f'source size changed: {path}')
    if verify_digest and digest(path) != record['sha256']:
        raise ValueError(f'source checksum changed: {path}')
    return record


def publish_bytes(path, content):
    """Publish once; an identical rerun is allowed, a changed file is refused."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent) as staged:
        staged.write(content)
        staged.flush()
        os.fsync(staged.fileno())
        try:
            os.link(staged.name, path)
        except FileExistsError:
            if path.read_bytes() != content:
                raise ValueError(f'refusing to replace published file: {path}') from None
        with open_directory(path.parent) as directory:
            os.fsync(directory)


def open_directory(path):
    from contextlib import contextmanager
    @contextmanager
    def opened():
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        try:
            yield descriptor
        finally:
            os.close(descriptor)
    return opened()


def publish_json(path, value):
    publish_bytes(path, (json.dumps(value, sort_keys=True, indent=2) + '\n').encode())


def fetch(root, provider, name, url, licence, licence_url, terms_checked_utc,
          notes='', expected_sha256=None):
    """One shared ledger counts all retained bytes and refuses the total cap before downloading."""
    root = Path(root)
    root.mkdir(parents=True, exist_ok=True)
    if Path(provider).name != provider or Path(name).name != name:
        raise ValueError('provider and filename must be simple names')
    target = root / provider / name
    with (root / '.download.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        subprocess.run(['df', '-h', str(root)], check=True)
        if target.exists():
            record = provenance(target)
            if record['url'] != url or (expected_sha256 and record['sha256'] != expected_sha256):
                raise ValueError('retained source differs from the requested provider identity')
            return record
        used = sum(p.stat().st_size for p in root.rglob('*') if p.is_file())
        reserve = 2_000_000_000
        available = min(MAX_DOWNLOAD_BYTES - used, shutil.disk_usage(root).free - reserve)
        if available <= 0:
            raise ValueError('download budget or free-space reserve exhausted')
        target.parent.mkdir(parents=True, exist_ok=True)
        request = urllib.request.Request(url, headers={'User-Agent': 'QuietMap terrain producer'})
        with urllib.request.urlopen(request, timeout=180) as response, tempfile.NamedTemporaryFile(dir=target.parent) as staged:
            expected_bytes = response.headers.get('Content-Length')
            if expected_bytes and int(expected_bytes) > available:
                raise ValueError('provider file exceeds remaining budget')
            checksum, size = hashlib.sha256(), 0
            while block := response.read(1 << 20):
                size += len(block)
                if size > available:
                    raise ValueError('provider stream exceeds remaining budget')
                checksum.update(block)
                staged.write(block)
            if expected_bytes and size != int(expected_bytes):
                raise ValueError('incomplete provider response')
            if expected_sha256 and checksum.hexdigest() != expected_sha256:
                raise ValueError('provider checksum differs from official catalogue')
            staged.flush()
            os.fsync(staged.fileno())
            os.link(staged.name, target)
        record = dict(url=url, fetched_utc=utc_now(), sha256=checksum.hexdigest(), bytes=size,
                      licence=licence, licence_url=licence_url, terms_checked_utc=terms_checked_utc,
                      notes=notes, raw_bytes_retained=True)
        publish_json(str(target) + '.provenance.json', record)
        return record
