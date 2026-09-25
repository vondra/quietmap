"""One anchor month selects the exposure year [anchor − 1 year, anchor): every baseline day and the 12 increment month-firsts."""

import argparse
from datetime import date, datetime, timedelta, timezone
from typing import NamedTuple


class ExposureDays(NamedTuple):
    """Primary-provider baseline days and the secondary-provider increment days."""

    baseline: tuple[date, ...]
    increment: tuple[date, ...]


def resolve_anchor(month: str | None, today: date) -> date:
    """The first day after the window; the default is this month, so the window ends yesterday."""
    if month is None:
        anchor = today.replace(day=1)
    else:
        try:
            anchor = date.fromisoformat(f"{month}-01")
        except ValueError as error:
            raise ValueError("anchor must be YYYY-MM") from error
    if anchor > today:
        raise ValueError("the window's last day (the day before the anchor) must have finished in UTC")
    return anchor


def sampling_days(anchor: date) -> ExposureDays:
    """Every day of the half-open year before `anchor` (365 or 366) and its month-firsts."""
    if anchor.day != 1:
        raise ValueError("aircraft anchor must be the first day of a month")
    first_day = anchor.replace(year=anchor.year - 1)
    baseline = tuple(first_day + timedelta(days=offset) for offset in range((anchor - first_day).days))
    return ExposureDays(baseline, tuple(day for day in baseline if day.day == 1))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--anchor", help="YYYY-MM, the month after the window; defaults to the current UTC month")
    args = parser.parse_args()
    try:
        anchor = resolve_anchor(args.anchor, datetime.now(timezone.utc).date())
        windows = sampling_days(anchor)
    except ValueError as error:
        parser.error(str(error))
    for days in windows:
        print(",".join(day.isoformat() for day in days))


if __name__ == "__main__":
    main()
