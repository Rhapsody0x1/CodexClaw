use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;

use super::store::CronKind;

pub fn next_after(kind: &CronKind, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    match kind {
        CronKind::OneShot { at } => Ok((*at > now).then_some(*at)),
        CronKind::Recurring { cron, tz } => {
            let tz = tz
                .parse::<Tz>()
                .with_context(|| format!("invalid timezone `{tz}`"))?;
            let schedule =
                Schedule::from_str(cron).with_context(|| format!("invalid cron `{cron}`"))?;
            let local_now = tz.from_utc_datetime(&now.naive_utc());
            Ok(schedule
                .after(&local_now)
                .next()
                .map(|value| value.with_timezone(&Utc)))
        }
    }
}

pub fn due_or_past(kind: &CronKind, now: DateTime<Utc>) -> bool {
    match kind {
        CronKind::OneShot { at } => *at <= now,
        CronKind::Recurring { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn one_shot_in_future_returns_at() {
        let at = Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap();
        let kind = CronKind::OneShot { at };
        assert_eq!(next_after(&kind, now).unwrap(), Some(at));
    }

    #[test]
    fn one_shot_in_past_returns_none() {
        let at = Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap();
        let kind = CronKind::OneShot { at };
        assert_eq!(next_after(&kind, now).unwrap(), None);
    }
}
