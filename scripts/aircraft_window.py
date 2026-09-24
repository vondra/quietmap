"""One anchor month selects the exposure year [anchor − 1 year, anchor): every GA day and 12 airline month-firsts."""

import argparse
from datetime import date, datetime, timedelta, timezone


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


def sampling_days(anchor: date) -> tuple[tuple[date, ...], tuple[date, ...]]:
    """Airline month-firsts and GA days of the half-open year before `anchor` (365 or 366 GA days)."""
    if anchor.day != 1:
        raise ValueError("aircraft anchor must be the first day of a month")
    first_day = anchor.replace(year=anchor.year - 1)
    general_aviation = tuple(first_day + timedelta(days=offset) for offset in range((anchor - first_day).days))
    airlines = tuple(day for day in general_aviation if day.day == 1)
    return airlines, general_aviation


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
