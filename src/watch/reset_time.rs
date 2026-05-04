//! Parsers that turn a captured string from a watch rule's regex into
//! an `Instant` the engine can schedule against.
//!
//! All parsers are stateless and pure (modulo wall-clock reads). They
//! return `None` on any failure — the engine's caller surfaces that as
//! a fallback to the rule's backoff curve and a warning.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use regex::Regex;

use super::rule::WaitFormat;

/// Parse `captured` per `format` and translate the result into an
/// `Instant`. The `now_instant` and the wall-clock `now` (Utc) are
/// passed in so tests can drive deterministic results.
///
/// For wall-clock formats, the offset between the parsed wall time and
/// `now_wall` is added to `now_instant`. If the parsed time is in the
/// past, the result is `now_instant` (fire immediately).
pub fn parse(
    format: WaitFormat,
    captured: &str,
    now_instant: Instant,
    now_wall: DateTime<Utc>,
) -> Option<Instant> {
    let captured = captured.trim();
    match format {
        WaitFormat::UnixSeconds => {
            let secs: i64 = captured.parse().ok()?;
            let target = DateTime::<Utc>::from_timestamp(secs, 0)?;
            Some(offset_into_instant(target, now_wall, now_instant))
        }
        WaitFormat::UnixMillis => {
            let ms: i64 = captured.parse().ok()?;
            let target = DateTime::<Utc>::from_timestamp_millis(ms)?;
            Some(offset_into_instant(target, now_wall, now_instant))
        }
        WaitFormat::ClockLocal => {
            let target = parse_clock_local(captured, Local::now())?;
            let target_utc = target.with_timezone(&Utc);
            Some(offset_into_instant(target_utc, now_wall, now_instant))
        }
        WaitFormat::InSeconds => {
            let n: f64 = captured.parse().ok()?;
            if !n.is_finite() || n < 0.0 {
                return None;
            }
            Some(now_instant + Duration::from_secs_f64(n))
        }
        WaitFormat::InMinutes => {
            let n: f64 = captured.parse().ok()?;
            if !n.is_finite() || n < 0.0 {
                return None;
            }
            Some(now_instant + Duration::from_secs_f64(n * 60.0))
        }
        WaitFormat::InHours => {
            let n: f64 = captured.parse().ok()?;
            if !n.is_finite() || n < 0.0 {
                return None;
            }
            Some(now_instant + Duration::from_secs_f64(n * 3600.0))
        }
    }
}

/// Turn a wall-clock target (UTC) into an `Instant` derived from
/// `now_instant`, clamping past times to fire immediately.
fn offset_into_instant(
    target: DateTime<Utc>,
    now_wall: DateTime<Utc>,
    now_instant: Instant,
) -> Instant {
    let delta = target.signed_duration_since(now_wall);
    let secs = delta.num_seconds();
    if secs <= 0 {
        return now_instant;
    }
    now_instant + Duration::from_secs(secs as u64)
}

/// Cached regex for clock-time parsing. Compiled once per process via
/// `OnceLock` so the parser stays cheap to call.
fn clock_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches:
        //   "3pm", "3PM", "3 pm",
        //   "3:00pm", "3:30 pm",
        //   "15:00", "23:59"
        // The optional `(Timezone)` suffix is stripped before this regex
        // sees the input — see `parse_clock_local`.
        Regex::new(r"^\s*(\d{1,2})(?::(\d{2}))?\s*([AaPp][Mm])?\s*$").expect("clock regex compiles")
    })
}

/// Parse a time-of-day string into a `DateTime<Local>` for today, rolled
/// to tomorrow if it would already be past `now_local`. Optional trailing
/// `(Timezone)` suffix is stripped and ignored.
fn parse_clock_local(s: &str, now_local: DateTime<Local>) -> Option<DateTime<Local>> {
    // Strip "(...)" suffix.
    let s = match s.find('(') {
        Some(idx) => s[..idx].trim(),
        None => s.trim(),
    };
    let caps = clock_regex().captures(s)?;
    let hour: u32 = caps.get(1)?.as_str().parse().ok()?;
    let minute: u32 = caps
        .get(2)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    let ampm = caps.get(3).map(|m| m.as_str().to_ascii_lowercase());

    let hour24 = match ampm.as_deref() {
        Some("am") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            if hour == 12 { 0 } else { hour }
        }
        Some("pm") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            if hour == 12 { 12 } else { hour + 12 }
        }
        // 24-hour format
        _ => {
            if hour > 23 {
                return None;
            }
            hour
        }
    };
    if minute > 59 {
        return None;
    }

    let today: NaiveDate = now_local.date_naive();
    let target_naive = today.and_hms_opt(hour24, minute, 0)?;
    let target = Local
        .from_local_datetime(&target_naive)
        .single()
        .or_else(|| Local.from_local_datetime(&target_naive).earliest())?;

    if target <= now_local {
        // Already past today — roll to tomorrow.
        let tomorrow = today.succ_opt()?;
        let tomorrow_naive = tomorrow.and_hms_opt(hour24, minute, 0)?;
        Local
            .from_local_datetime(&tomorrow_naive)
            .single()
            .or_else(|| Local.from_local_datetime(&tomorrow_naive).earliest())
    } else {
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    fn instant_offset_secs(target: Instant, base: Instant) -> i64 {
        if target >= base {
            target.duration_since(base).as_secs() as i64
        } else {
            -(base.duration_since(target).as_secs() as i64)
        }
    }

    #[test]
    fn unix_seconds_future_offset() {
        let now_instant = Instant::now();
        let now_wall = Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap();
        let target_ts = now_wall.timestamp() + 3600; // +1h
        let parsed = parse(
            WaitFormat::UnixSeconds,
            &target_ts.to_string(),
            now_instant,
            now_wall,
        )
        .expect("unix seconds parse");
        let off = instant_offset_secs(parsed, now_instant);
        assert!((3500..=3700).contains(&off), "expected ~3600s, got {off}");
    }

    #[test]
    fn unix_seconds_past_clamps_to_now() {
        let now_instant = Instant::now();
        let now_wall = Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap();
        let target_ts = now_wall.timestamp() - 3600; // 1h ago
        let parsed = parse(
            WaitFormat::UnixSeconds,
            &target_ts.to_string(),
            now_instant,
            now_wall,
        )
        .expect("past unix seconds parse");
        assert_eq!(parsed, now_instant, "past timestamps should clamp to now");
    }

    #[test]
    fn unix_millis_future_offset() {
        let now_instant = Instant::now();
        let now_wall = Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap();
        let target_ms = now_wall.timestamp_millis() + 60_000;
        let parsed = parse(
            WaitFormat::UnixMillis,
            &target_ms.to_string(),
            now_instant,
            now_wall,
        )
        .expect("unix millis parse");
        let off = instant_offset_secs(parsed, now_instant);
        assert!((50..=70).contains(&off));
    }

    #[test]
    fn in_hours_simple_case() {
        let now_instant = Instant::now();
        let now_wall = Utc::now();
        let parsed =
            parse(WaitFormat::InHours, "5", now_instant, now_wall).expect("in_hours parse");
        let off = instant_offset_secs(parsed, now_instant);
        assert!((5 * 3600 - 5..=5 * 3600 + 5).contains(&off));
    }

    #[test]
    fn in_minutes_fractional() {
        let now_instant = Instant::now();
        let now_wall = Utc::now();
        let parsed = parse(WaitFormat::InMinutes, "2.5", now_instant, now_wall).expect("parse");
        let off = instant_offset_secs(parsed, now_instant);
        assert!((150 - 2..=150 + 2).contains(&off));
    }

    #[test]
    fn in_seconds_negative_rejected() {
        let now_instant = Instant::now();
        let now_wall = Utc::now();
        assert!(parse(WaitFormat::InSeconds, "-5", now_instant, now_wall).is_none());
    }

    #[test]
    fn malformed_input_returns_none() {
        let now_instant = Instant::now();
        let now_wall = Utc::now();
        assert!(
            parse(
                WaitFormat::UnixSeconds,
                "not a number",
                now_instant,
                now_wall
            )
            .is_none()
        );
        assert!(parse(WaitFormat::InHours, "five", now_instant, now_wall).is_none());
        assert!(parse(WaitFormat::ClockLocal, "noon-ish", now_instant, now_wall).is_none());
    }

    #[test]
    fn clock_local_pm_format() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 13, 0, 0)
            .single()
            .unwrap();
        let parsed = parse_clock_local("3pm", now).expect("3pm parses");
        assert_eq!(parsed.hour(), 15);
        assert_eq!(parsed.minute(), 0);
        assert_eq!(parsed.day(), 4);
    }

    #[test]
    fn clock_local_with_minutes() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 13, 0, 0)
            .single()
            .unwrap();
        let parsed = parse_clock_local("3:30 pm", now).expect("3:30pm parses");
        assert_eq!(parsed.hour(), 15);
        assert_eq!(parsed.minute(), 30);
    }

    #[test]
    fn clock_local_24_hour() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 13, 0, 0)
            .single()
            .unwrap();
        let parsed = parse_clock_local("16:45", now).expect("16:45 parses");
        assert_eq!(parsed.hour(), 16);
        assert_eq!(parsed.minute(), 45);
    }

    #[test]
    fn clock_local_past_rolls_to_tomorrow() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 16, 0, 0)
            .single()
            .unwrap();
        let parsed = parse_clock_local("3pm", now).expect("3pm parses");
        // 3pm has already passed (now is 4pm) — should roll to May 5.
        assert_eq!(parsed.day(), 5);
        assert_eq!(parsed.hour(), 15);
    }

    #[test]
    fn clock_local_strips_timezone_suffix() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 12, 0, 0)
            .single()
            .unwrap();
        let parsed_tz = parse_clock_local("3pm (America/New_York)", now).expect("with TZ parses");
        let parsed_no_tz = parse_clock_local("3pm", now).expect("without TZ parses");
        // Same hour/minute regardless of suffix (we ignore it).
        assert_eq!(parsed_tz.hour(), parsed_no_tz.hour());
        assert_eq!(parsed_tz.minute(), parsed_no_tz.minute());
    }

    #[test]
    fn clock_local_12am_is_midnight() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 6, 0, 0)
            .single()
            .unwrap();
        // 12am today = midnight today, which is in the past relative to
        // 6am — should roll to tomorrow's midnight.
        let parsed = parse_clock_local("12am", now).expect("12am parses");
        assert_eq!(parsed.hour(), 0);
        assert_eq!(parsed.day(), 5);
    }

    #[test]
    fn clock_local_12pm_is_noon() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 4, 6, 0, 0)
            .single()
            .unwrap();
        let parsed = parse_clock_local("12pm", now).expect("12pm parses");
        assert_eq!(parsed.hour(), 12);
        assert_eq!(parsed.day(), 4);
    }

    #[test]
    fn clock_local_invalid_hour_returns_none() {
        let now = Local::now();
        assert!(parse_clock_local("25:00", now).is_none());
        assert!(parse_clock_local("13pm", now).is_none()); // pm only valid for 1-12
        assert!(parse_clock_local("0am", now).is_none()); // am only valid for 1-12
    }
}
