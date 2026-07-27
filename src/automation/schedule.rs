//! RRULE subset evaluator + schedule presets for automations.

use super::schema::{Automation, AutomationKind};
use anyhow::{bail, Context, Result};
use chrono::{
    DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike, Utc, Weekday,
};
use std::collections::HashMap;

/// Preset schedules shown in the editor UI (Codex-style common cadences).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulePreset {
    Hourly,
    EverySixHours,
    DailyMorning,
    DailyEvening,
    WeekdaysMorning,
    WeeklyMonday,
    WeeklyFriday,
}

impl SchedulePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hourly => "Every hour",
            Self::EverySixHours => "Every 6 hours",
            Self::DailyMorning => "Daily at 9:00 AM",
            Self::DailyEvening => "Daily at 6:00 PM",
            Self::WeekdaysMorning => "Weekdays at 9:00 AM",
            Self::WeeklyMonday => "Mondays at 9:00 AM",
            Self::WeeklyFriday => "Fridays at 4:00 PM",
        }
    }

    pub fn rrule(self) -> &'static str {
        match self {
            Self::Hourly => "FREQ=HOURLY;INTERVAL=1",
            Self::EverySixHours => "FREQ=HOURLY;INTERVAL=6",
            Self::DailyMorning => "FREQ=DAILY;BYHOUR=9;BYMINUTE=0",
            Self::DailyEvening => "FREQ=DAILY;BYHOUR=18;BYMINUTE=0",
            Self::WeekdaysMorning => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0",
            Self::WeeklyMonday => "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=0",
            Self::WeeklyFriday => "FREQ=WEEKLY;BYDAY=FR;BYHOUR=16;BYMINUTE=0",
        }
    }

    pub fn all() -> &'static [SchedulePreset] {
        &[
            Self::Hourly,
            Self::EverySixHours,
            Self::DailyMorning,
            Self::DailyEvening,
            Self::WeekdaysMorning,
            Self::WeeklyMonday,
            Self::WeeklyFriday,
        ]
    }

    pub fn from_rrule(rrule: &str) -> Option<Self> {
        let normalized = rrule.trim().trim_start_matches("RRULE:").trim();
        for preset in Self::all() {
            if preset.rrule().eq_ignore_ascii_case(normalized) {
                return Some(*preset);
            }
        }
        None
    }
}

/// Human-readable schedule summary for list rows.
pub fn humanize_schedule(automation: &Automation) -> String {
    match automation.kind {
        AutomationKind::Heartbeat => {
            let m = automation.interval_minutes.max(1);
            if m == 1 {
                "Every minute".into()
            } else {
                format!("Every {m} minutes")
            }
        }
        AutomationKind::Cron => {
            if let Some(preset) = SchedulePreset::from_rrule(&automation.rrule) {
                return preset.label().to_string();
            }
            if automation.rrule.trim().is_empty() {
                "No schedule".into()
            } else {
                automation.rrule.clone()
            }
        }
    }
}

/// Deterministic jitter seconds in `[0, max_secs)` from salt + automation id.
pub fn jitter_seconds(salt: &str, automation_id: &str, max_secs: u32) -> u32 {
    if max_secs == 0 {
        return 0;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"\0");
    hasher.update(automation_id.as_bytes());
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    n % max_secs
}

/// Next fire time at or after `after` (UTC), including schedule jitter.
pub fn next_fire_after(
    automation: &Automation,
    after: DateTime<Utc>,
    salt: &str,
) -> Result<Option<DateTime<Utc>>> {
    if !automation.is_active() {
        return Ok(None);
    }
    let base = match automation.kind {
        AutomationKind::Heartbeat => {
            let mins = automation.interval_minutes.max(1) as i64;
            after + Duration::minutes(mins)
        }
        AutomationKind::Cron => {
            let rrule = automation.rrule.trim().trim_start_matches("RRULE:");
            if rrule.is_empty() {
                bail!("automation {} has empty rrule", automation.id);
            }
            next_rrule_occurrence(rrule, after)?
                .with_context(|| format!("no next occurrence for {}", automation.id))?
        }
    };
    let jitter = jitter_seconds(salt, &automation.id, 60);
    Ok(Some(base + Duration::seconds(jitter as i64)))
}

/// Whether `now` is at or past the next due time (catch-up: single fire, not N).
pub fn is_due(
    automation: &Automation,
    last_fired_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    salt: &str,
) -> Result<bool> {
    if !automation.is_active() {
        return Ok(false);
    }
    let after = last_fired_at.unwrap_or_else(|| now - Duration::days(3650));
    let Some(next) = next_fire_after(automation, after, salt)? else {
        return Ok(false);
    };
    // If never fired, only fire when next from "now-ish" is due — avoid ancient backfill.
    if last_fired_at.is_none() {
        // First schedule: due if next occurrence from (now - 1 minute) is <= now
        // i.e. we just passed a scheduled slot within a narrow window OR next is in the past.
        let recent = now - Duration::minutes(2);
        let Some(from_recent) = next_fire_after(automation, recent, salt)? else {
            return Ok(false);
        };
        return Ok(from_recent <= now);
    }
    Ok(next <= now)
}

#[derive(Debug, Clone)]
struct RRule {
    freq: Freq,
    interval: u32,
    by_hour: Option<u32>,
    by_minute: Option<u32>,
    by_second: Option<u32>,
    by_day: Option<Vec<Weekday>>,
    by_month_day: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freq {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

fn parse_rrule(raw: &str) -> Result<RRule> {
    let body = raw.trim().trim_start_matches("RRULE:").trim();
    let mut map = HashMap::new();
    for part in body.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part
            .split_once('=')
            .with_context(|| format!("invalid rrule part: {part}"))?;
        map.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
    }
    let freq = match map.get("FREQ").map(|s| s.to_ascii_uppercase()).as_deref() {
        Some("HOURLY") => Freq::Hourly,
        Some("DAILY") => Freq::Daily,
        Some("WEEKLY") => Freq::Weekly,
        Some("MONTHLY") => Freq::Monthly,
        other => bail!("unsupported FREQ: {other:?}"),
    };
    let interval = map
        .get("INTERVAL")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let by_hour = map.get("BYHOUR").and_then(|s| s.parse().ok());
    let by_minute = map.get("BYMINUTE").and_then(|s| s.parse().ok());
    let by_second = map.get("BYSECOND").and_then(|s| s.parse().ok());
    let by_month_day = map.get("BYMONTHDAY").and_then(|s| s.parse().ok());
    let by_day = map.get("BYDAY").map(|s| {
        s.split(',')
            .filter_map(|d| parse_weekday(d.trim()))
            .collect::<Vec<_>>()
    });
    Ok(RRule {
        freq,
        interval,
        by_hour,
        by_minute,
        by_second,
        by_day,
        by_month_day,
    })
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_ascii_uppercase().as_str() {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Next occurrence strictly after `after`, evaluated in system local wall clock then converted to UTC.
fn next_rrule_occurrence(rrule: &str, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    let rule = parse_rrule(rrule)?;
    let after_local = after.with_timezone(&Local);
    let hour = rule.by_hour.unwrap_or(after_local.hour());
    let minute = rule.by_minute.unwrap_or(0);
    let second = rule.by_second.unwrap_or(0);

    match rule.freq {
        Freq::Hourly => {
            let mut candidate = after_local
                .with_minute(minute)
                .and_then(|t| t.with_second(second))
                .and_then(|t| t.with_nanosecond(0))
                .unwrap_or(after_local);
            if candidate <= after_local {
                candidate += Duration::hours(rule.interval as i64);
                candidate = candidate
                    .with_minute(minute)
                    .and_then(|t| t.with_second(second))
                    .and_then(|t| t.with_nanosecond(0))
                    .unwrap_or(candidate);
            }
            // Align to interval from epoch-ish hours if INTERVAL > 1
            if rule.interval > 1 {
                for _ in 0..rule.interval + 2 {
                    if candidate > after_local {
                        break;
                    }
                    candidate += Duration::hours(rule.interval as i64);
                }
            }
            Ok(Some(candidate.with_timezone(&Utc)))
        }
        Freq::Daily => {
            let mut day = after_local.date_naive();
            for _ in 0..400 {
                if let Some(dt) = combine_local(day, hour, minute, second) {
                    if dt > after_local {
                        return Ok(Some(dt.with_timezone(&Utc)));
                    }
                }
                day += Duration::days(rule.interval as i64);
            }
            Ok(None)
        }
        Freq::Weekly => {
            let days = rule
                .by_day
                .clone()
                .unwrap_or_else(|| vec![after_local.weekday()]);
            let mut day = after_local.date_naive();
            for _ in 0..400 {
                if days.contains(&day.weekday()) {
                    if let Some(dt) = combine_local(day, hour, minute, second) {
                        if dt > after_local {
                            // For INTERVAL>1 weeks, snap to week multiples from after
                            if rule.interval == 1 {
                                return Ok(Some(dt.with_timezone(&Utc)));
                            }
                            let weeks =
                                (day - after_local.date_naive()).num_weeks().unsigned_abs() as u32;
                            if weeks % rule.interval == 0 || day == after_local.date_naive() {
                                // if same week as after but later today, allow
                                return Ok(Some(dt.with_timezone(&Utc)));
                            }
                        }
                    }
                }
                day += Duration::days(1);
            }
            Ok(None)
        }
        Freq::Monthly => {
            let mday = rule.by_month_day.unwrap_or(after_local.day());
            let mut year = after_local.year();
            let mut month = after_local.month();
            for _ in 0..120 {
                if let Some(date) =
                    NaiveDate::from_ymd_opt(year, month, mday.min(28)).or_else(|| {
                        // clamp to last day of month
                        NaiveDate::from_ymd_opt(year, month, 1)
                            .and_then(|d| d.with_day(mday.min(days_in_month(year, month))))
                    })
                {
                    if let Some(dt) = combine_local(date, hour, minute, second) {
                        if dt > after_local {
                            return Ok(Some(dt.with_timezone(&Utc)));
                        }
                    }
                }
                // advance by interval months
                let mut m = month as i32 + rule.interval as i32;
                while m > 12 {
                    m -= 12;
                    year += 1;
                }
                month = m as u32;
            }
            Ok(None)
        }
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
        .day()
}

fn combine_local(date: NaiveDate, hour: u32, minute: u32, second: u32) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::new(date, chrono::NaiveTime::from_hms_opt(hour, minute, second)?);
    naive.and_local_timezone(Local).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::schema::Automation;

    fn sample(rrule: &str) -> Automation {
        Automation::new(
            "test-auto".into(),
            "Test".into(),
            "do stuff".into(),
            "grok".into(),
            "grok-build".into(),
            rrule.into(),
            "/tmp".into(),
        )
    }

    #[test]
    fn jitter_is_stable() {
        let a = jitter_seconds("salt", "id-a", 60);
        let b = jitter_seconds("salt", "id-a", 60);
        assert_eq!(a, b);
        assert!(a < 60);
        let c = jitter_seconds("salt", "id-b", 60);
        // high probability different
        let _ = c;
    }

    #[test]
    fn daily_next_is_after() {
        let a = sample("FREQ=DAILY;BYHOUR=9;BYMINUTE=0");
        let after = Utc::now();
        let next = next_fire_after(&a, after, "salt").unwrap().unwrap();
        assert!(next > after);
    }

    #[test]
    fn weekly_weekday_next() {
        let a = sample("FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=0");
        let after = Utc::now();
        let next = next_fire_after(&a, after, "salt").unwrap().unwrap();
        assert!(next > after);
        let local = next.with_timezone(&Local);
        assert_eq!(local.weekday(), Weekday::Mon);
    }

    #[test]
    fn paused_not_due() {
        let mut a = sample("FREQ=HOURLY;INTERVAL=1");
        a.status = crate::automation::schema::AutomationStatus::Paused;
        assert!(!is_due(&a, None, Utc::now(), "salt").unwrap());
    }

    #[test]
    fn preset_round_trip() {
        for p in SchedulePreset::all() {
            assert_eq!(SchedulePreset::from_rrule(p.rrule()), Some(*p));
        }
    }

    #[test]
    fn humanize_preset() {
        let a = sample("FREQ=DAILY;BYHOUR=9;BYMINUTE=0");
        assert_eq!(humanize_schedule(&a), "Daily at 9:00 AM");
    }
}
