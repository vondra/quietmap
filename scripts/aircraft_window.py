"""One completed monthly anchor selects 12 airline samples and 365 aligned GA days."""

import argparse
from datetime import date, datetime, timedelta, timezone


def resolve_anchor(month: str | None, today: date) -> date:
    if month is None:
        anchor = today.replace(day=1)
        if anchor == today:
            anchor = (anchor - timedelta(days=1)).replace(day=1)
    else:
        try:
            anchor = date.fromisoformat(f"{month}-01")
        except ValueError as error:
            raise ValueError("anchor must be YYYY-MM") from error
    if anchor >= today:
        raise ValueError("anchor sample day must have finished in UTC")
    return anchor


def sampling_days(anchor: date) -> tuple[tuple[date, ...], tuple[date, ...]]:
    if anchor.day != 1:
        raise ValueError("aircraft anchor must be the first day of a month")
    month_index = anchor.year * 12 + anchor.month - 1
    airlines = tuple(
        date(index // 12, index % 12 + 1, 1)
        for index in range(month_index - 11, month_index + 1)
    )
    general_aviation = tuple(anchor - timedelta(days=offset) for offset in range(364, -1, -1))
    return airlines, general_aviation


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--anchor", help="YYYY-MM; defaults to the latest completed monthly sample")
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
