//! Relative message times shared by the desktop and Android transcripts.
//!
//! The label is a pure function of the message instant, an injected `now` and
//! the locale, so both clients format identically and tests pin the clock.
//! Calendar boundaries (yesterday, weekday) follow `now`'s UTC offset, which
//! the platform sets to the device's local time zone.
use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};

/// Locales with message time wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeLocale {
    ZhCn,
    En,
}
impl TimeLocale {
    /// `zh…` tags use Chinese; everything else falls back to English.
    pub fn from_tag(tag: &str) -> Self {
        if tag.to_ascii_lowercase().starts_with("zh") {
            Self::ZhCn
        } else {
            Self::En
        }
    }
}

/// A message time: the short label shown in the transcript and the full
/// timestamp revealed on hover (desktop) or long press (phone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageTime {
    pub label: String,
    pub full: String,
}

/// Clock skew between devices can place a just-sent message slightly in the
/// future; within this tolerance it still reads as "just now".
pub const SKEW_TOLERANCE: Duration = Duration::minutes(5);

/// Formats `at` relative to `now`:
///
/// - under a minute (or a future instant within [`SKEW_TOLERANCE`]): 刚刚 / Just now
/// - under an hour: N 分钟前 / N min ago
/// - earlier the same calendar day: N 小时前 / N hr ago
/// - the previous calendar day: 昨天 HH:MM / Yesterday HH:MM
/// - two to six calendar days ago: 周三 HH:MM / Wed HH:MM
/// - earlier this year: 9月3日 HH:MM / Sep 3 HH:MM
/// - an earlier year: 2025年12月30日 HH:MM / Dec 30, 2025 HH:MM
///
/// Instants further in the future than the tolerance are shown as absolute
/// times (HH:MM today, otherwise the dated form) rather than "in N minutes".
pub fn format(at: DateTime<Utc>, now: DateTime<FixedOffset>, locale: TimeLocale) -> MessageTime {
    let local = at.with_timezone(now.offset());
    MessageTime {
        label: label(local, now, locale),
        full: full(local, locale),
    }
}

/// Parses an RFC 3339 `created_at` and formats it; `None` when unparseable.
pub fn format_rfc3339(
    at: &str,
    now: DateTime<FixedOffset>,
    locale: TimeLocale,
) -> Option<MessageTime> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| format(at.with_timezone(&Utc), now, locale))
}

/// How long until the label for `at` changes, so a client repaints only then
/// instead of ticking every visible message. `None` once the label is stable
/// (dated forms never change except at the new-year boundary, which a normal
/// redraw catches).
pub fn next_change(at: DateTime<Utc>, now: DateTime<FixedOffset>) -> Option<Duration> {
    let local = at.with_timezone(now.offset());
    let elapsed = now.signed_duration_since(local);
    if elapsed < Duration::zero() {
        // Future within tolerance reads "just now" until it is a minute old;
        // beyond tolerance it is absolute until it falls inside the window.
        let until_tolerance = -elapsed - SKEW_TOLERANCE;
        return Some(if until_tolerance > Duration::zero() {
            until_tolerance
        } else {
            -elapsed + Duration::minutes(1)
        });
    }
    if elapsed < Duration::hours(1) {
        let minutes = elapsed.num_minutes();
        return Some(Duration::minutes(minutes + 1) - elapsed);
    }
    let days = calendar_days(local, now);
    let midnight = next_midnight(now);
    match days {
        0 => {
            let hours = elapsed.num_hours();
            Some((Duration::hours(hours + 1) - elapsed).min(midnight))
        }
        // Yesterday becomes a weekday at midnight; a weekday becomes a date
        // at the midnight that makes it seven days old.
        1 => Some(midnight),
        2..=6 => Some(midnight + Duration::days(6 - days)),
        _ => None,
    }
}

fn label(at: DateTime<FixedOffset>, now: DateTime<FixedOffset>, locale: TimeLocale) -> String {
    let zh = locale == TimeLocale::ZhCn;
    let elapsed = now.signed_duration_since(at);
    if elapsed < Duration::zero() {
        if -elapsed <= SKEW_TOLERANCE {
            return just_now(zh);
        }
        return if calendar_days(at, now) == 0 {
            clock(at)
        } else {
            dated(at, now, zh)
        };
    }
    if elapsed < Duration::minutes(1) {
        return just_now(zh);
    }
    if elapsed < Duration::hours(1) {
        let n = elapsed.num_minutes();
        return if zh {
            format!("{n} 分钟前")
        } else {
            format!("{n} min ago")
        };
    }
    match calendar_days(at, now) {
        0 => {
            let n = elapsed.num_hours();
            if zh {
                format!("{n} 小时前")
            } else {
                format!("{n} hr ago")
            }
        }
        1 => {
            if zh {
                format!("昨天 {}", clock(at))
            } else {
                format!("Yesterday {}", clock(at))
            }
        }
        2..=6 => format!("{} {}", weekday(at, zh), clock(at)),
        _ => dated(at, now, zh),
    }
}

fn just_now(zh: bool) -> String {
    if zh { "刚刚" } else { "Just now" }.into()
}

fn dated(at: DateTime<FixedOffset>, now: DateTime<FixedOffset>, zh: bool) -> String {
    let same_year = at.year() == now.year();
    match (zh, same_year) {
        (true, true) => format!("{}月{}日 {}", at.month(), at.day(), clock(at)),
        (true, false) => format!(
            "{}年{}月{}日 {}",
            at.year(),
            at.month(),
            at.day(),
            clock(at)
        ),
        (false, true) => format!("{} {} {}", month(at), at.day(), clock(at)),
        (false, false) => format!("{} {}, {} {}", month(at), at.day(), at.year(), clock(at)),
    }
}

fn full(at: DateTime<FixedOffset>, locale: TimeLocale) -> String {
    let seconds = format!("{}:{:02}", clock(at), at.second());
    match locale {
        TimeLocale::ZhCn => format!(
            "{}年{}月{}日 {} {seconds}",
            at.year(),
            at.month(),
            at.day(),
            weekday(at, true)
        ),
        TimeLocale::En => format!(
            "{}, {} {}, {} {seconds}",
            weekday(at, false),
            month(at),
            at.day(),
            at.year()
        ),
    }
}

fn clock(at: DateTime<FixedOffset>) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

fn weekday(at: DateTime<FixedOffset>, zh: bool) -> &'static str {
    let index = at.weekday().num_days_from_monday() as usize;
    if zh {
        ["周一", "周二", "周三", "周四", "周五", "周六", "周日"][index]
    } else {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][index]
    }
}

fn month(at: DateTime<FixedOffset>) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][at.month0() as usize]
}

/// Whole calendar days from `at` to `now`, in `now`'s offset.
fn calendar_days(at: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> i64 {
    (now.date_naive() - at.date_naive()).num_days()
}

fn next_midnight(now: DateTime<FixedOffset>) -> Duration {
    let tomorrow = now.date_naive() + Duration::days(1);
    let midnight = tomorrow.and_hms_opt(0, 0, 0).expect("midnight exists");
    midnight - now.naive_local()
}

#[cfg(test)]
mod tests;
