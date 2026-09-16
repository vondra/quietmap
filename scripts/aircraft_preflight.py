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


def preflight_aircraft_sources(airline_cache, ga_cache, anchor, run=subprocess.run):
    """Admit the shared monthly window, GA publisher receipts and native airline days.

    Reuses the exact validators the producer runs later (`aircraft_window`
    selection, `download-adsblol.py validate` publisher authority, native
    `AdsbTarSource` day admission); nothing is fetched, written or rehashed.
    Returns the admitted counts for the build log.
    """
    airline_days, ga_days = sampling_days(resolve_anchor(anchor, datetime.now(timezone.utc).date()))
    ga_days_csv = ','.join(day.isoformat() for day in ga_days)
    airline_days_csv = ','.join(day.isoformat() for day in airline_days)
    ga = run([sys.executable, str(SCRIPTS / 'download-adsblol.py'), 'validate',
              '--source-root', str(ga_cache), '--days', ga_days_csv], capture_output=True)
    if ga.returncode:
        raise ValueError('GA source window incomplete under publisher receipts: '
                         + last_line(ga.stderr, f'exit {ga.returncode}'))
    accepted_ga_days = len(set(ga.stdout.split(b'\0')[:-1:2]))
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
    airline = run([str(ENGINE / 'target/release/aircraft-extract'), 'preflight-days',
                   '--adsb-cache', str(airline_cache), '--days', airline_days_csv],
                  capture_output=True)
    if airline.returncode:
        raise ValueError('airline source window incomplete: '
                         + last_line(airline.stderr, f'exit {airline.returncode}'))
    return {'anchor': anchor, 'airline_days': len(airline_days),
            'ga_window_days': len(ga_days), 'ga_accepted_days': accepted_ga_days}
