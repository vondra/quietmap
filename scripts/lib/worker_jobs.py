"""Process-pool size: all CPUs, or fewer when the cgroup/host cannot hold them."""

from pathlib import Path
import os

_CGROUP_ROOT = Path('/sys/fs/cgroup')


def cpu_jobs():
    try:
        return len(os.sched_getaffinity(0)) or 1
    except (AttributeError, OSError):
        return os.cpu_count() or 1


def available_memory_bytes():
    """Cgroup `memory.max` when finite, otherwise host MemAvailable."""
    cgroup = _cgroup_memory_max()
    if cgroup is not None:
        return cgroup
    return _host_available_bytes()


def fit_jobs(requested, bytes_per_worker, memory_bytes=None):
    """Upper-bound `requested` workers by memory. `--jobs` is the request, not a floor."""
    if requested < 1:
        raise ValueError('--jobs must be >= 1')
    if bytes_per_worker < 1:
        raise ValueError('bytes_per_worker must be >= 1')
    available = available_memory_bytes() if memory_bytes is None else memory_bytes
    if available < 1:
        raise ValueError('memory limit must be >= 1')
    return max(1, min(requested, available // bytes_per_worker))


def _cgroup_memory_max():
    try:
        relative = None
        with open('/proc/self/cgroup', encoding='utf-8') as text:
            for line in text:
                line = line.strip()
                if line.startswith('0::'):
                    relative = line[3:].lstrip('/')
                    break
        if relative is None:
            return None
        path = _CGROUP_ROOT / relative
        while True:
            limit = path / 'memory.max'
            if limit.is_file():
                raw = limit.read_text(encoding='utf-8').strip()
                if raw != 'max':
                    return int(raw)
            if path == _CGROUP_ROOT:
                return None
            path = path.parent
    except (OSError, ValueError):
        return None


def _host_available_bytes():
    try:
        with open('/proc/meminfo', encoding='utf-8') as text:
            for line in text:
                if line.startswith('MemAvailable:'):
                    return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return os.sysconf('SC_PHYS_PAGES') * os.sysconf('SC_PAGE_SIZE')
