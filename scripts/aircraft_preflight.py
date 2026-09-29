"""Read-only aircraft source preflight: fail a fresh world build before pinning and OSM extraction."""

import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from aircraft_window import resolve_anchor, sampling_days

SCRIPTS = Path(__file__).resolve().parent
ENGINE = SCRIPTS.parent / 'engine'


def last_line(output, fallback):
    lines = output.decode(errors='replace').strip().splitlines()
    return lines[-1] if lines else fallback


def preflight_aircraft_sources(primary_cache, secondary_cache, anchor, run=subprocess.run):
    """Admit the exposure year: primary publisher receipts and native secondary days.

    Reuses the exact validators the producer runs later (`aircraft_window`
    selection, `download-adsblol.py validate` publisher authority, native
    `AdsbTarSource` day admission); nothing is fetched, written or rehashed.
    Hourly content receipts are Stage 0's; this gate refuses an incomplete
    acquisition before any producer starts. Returns counts for the build log.
    """
    days = sampling_days(resolve_anchor(anchor, datetime.now(timezone.utc).date()))
    baseline_csv = ','.join(day.isoformat() for day in days.baseline)
    increment_csv = ','.join(day.isoformat() for day in days.increment)
    primary = run([sys.executable, str(SCRIPTS / 'download-adsblol.py'), 'validate',
                   '--source-root', str(primary_cache), '--days', baseline_csv], capture_output=True)
    if primary.returncode:
        raise ValueError('primary source window incomplete under publisher receipts: '
                         + last_line(primary.stderr, f'exit {primary.returncode}'))
    accepted_baseline_days = len(set(primary.stdout.split(b'\0')[:-1:2]))
    # The preflight must use the same selector the producer will, so build the
    # native binary first (a no-op when already current); producers reuse it.
    if shutil.which('cargo') is None:
        raise ValueError('cargo is unavailable: the aircraft preflight needs the native source selector')
    built = run(['cargo', 'build', '--release', '--manifest-path',
                 str(ENGINE / 'aircraft-extract/Cargo.toml'), '--bin', 'aircraft-extract'],
                cwd=str(ENGINE.parent), capture_output=True)
    if built.returncode:
        raise ValueError('aircraft-extract cannot be built for source preflight: '
                         + last_line(built.stderr, f'exit {built.returncode}'))
    secondary = run([str(ENGINE / 'target/release/aircraft-extract'), 'preflight-days',
                     '--adsb-cache', str(secondary_cache), '--days', increment_csv],
                    capture_output=True)
    if secondary.returncode:
        raise ValueError('secondary source window incomplete: '
                         + last_line(secondary.stderr, f'exit {secondary.returncode}'))
    return {'anchor': anchor, 'baseline_window_days': len(days.baseline),
            'baseline_accepted_days': accepted_baseline_days, 'increment_days': len(days.increment)}
