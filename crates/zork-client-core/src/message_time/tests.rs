use super::*;

/// Saturday 2026-09-26 14:30:00 in UTC+8.
fn now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-09-26T14:30:00+08:00").unwrap()
}
fn ago(duration: Duration) -> DateTime<Utc> {
    (now() - duration).with_timezone(&Utc)
}
fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc)
}
fn zh(at: DateTime<Utc>) -> String {
    format(at, now(), TimeLocale::ZhCn).label
}
fn en(at: DateTime<Utc>) -> String {
    format(at, now(), TimeLocale::En).label
}

#[test]
fn just_now_covers_the_first_minute() {
    assert_eq!(zh(ago(Duration::zero())), "刚刚");
    assert_eq!(zh(ago(Duration::seconds(59))), "刚刚");
    assert_eq!(en(ago(Duration::seconds(59))), "Just now");
    assert_eq!(zh(ago(Duration::seconds(60))), "1 分钟前");
}

#[test]
fn minutes_until_the_first_hour() {
    assert_eq!(zh(ago(Duration::seconds(119))), "1 分钟前");
    assert_eq!(zh(ago(Duration::minutes(2))), "2 分钟前");
    assert_eq!(zh(ago(Duration::seconds(3599))), "59 分钟前");
    assert_eq!(en(ago(Duration::minutes(5))), "5 min ago");
    assert_eq!(zh(ago(Duration::minutes(60))), "1 小时前");
}

#[test]
fn hours_only_within_the_same_calendar_day() {
    assert_eq!(zh(ago(Duration::minutes(119))), "1 小时前");
    assert_eq!(zh(ago(Duration::hours(2))), "2 小时前");
    assert_eq!(en(ago(Duration::hours(3))), "3 hr ago");
    // 00:00 today is still today: 14 hours 30 minutes ago.
    assert_eq!(zh(at("2026-09-26T00:00:00+08:00")), "14 小时前");
    // One second earlier is yesterday.
    assert_eq!(zh(at("2026-09-25T23:59:59+08:00")), "昨天 23:59");
}

#[test]
fn minutes_win_across_midnight() {
    let now = DateTime::parse_from_rfc3339("2026-09-26T00:10:00+08:00").unwrap();
    let label = |s: &str| format(at(s), now, TimeLocale::ZhCn).label;
    assert_eq!(label("2026-09-25T23:30:00+08:00"), "40 分钟前");
    assert_eq!(label("2026-09-25T23:10:00+08:00"), "昨天 23:10");
    assert_eq!(label("2026-09-25T22:00:00+08:00"), "昨天 22:00");
}

#[test]
fn yesterday_shows_the_clock() {
    assert_eq!(zh(at("2026-09-25T09:05:00+08:00")), "昨天 09:05");
    assert_eq!(en(at("2026-09-25T09:05:00+08:00")), "Yesterday 09:05");
    assert_eq!(zh(at("2026-09-25T00:00:00+08:00")), "昨天 00:00");
}

#[test]
fn weekday_within_the_week() {
    // Thursday, two calendar days ago.
    assert_eq!(zh(at("2026-09-24T23:59:00+08:00")), "周四 23:59");
    assert_eq!(en(at("2026-09-24T23:59:00+08:00")), "Thu 23:59");
    // Sunday, six calendar days ago.
    assert_eq!(zh(at("2026-09-20T08:00:00+08:00")), "周日 08:00");
    assert_eq!(en(at("2026-09-20T08:00:00+08:00")), "Sun 08:00");
}

#[test]
fn dates_beyond_a_week() {
    // Seven calendar days ago is the same weekday: show the date, not "周六".
    assert_eq!(zh(at("2026-09-19T18:00:00+08:00")), "9月19日 18:00");
    assert_eq!(en(at("2026-09-19T18:00:00+08:00")), "Sep 19 18:00");
    assert_eq!(zh(at("2026-08-26T10:15:00+08:00")), "8月26日 10:15");
    assert_eq!(zh(at("2026-01-01T00:00:00+08:00")), "1月1日 00:00");
    assert_eq!(zh(at("2025-12-30T21:45:00+08:00")), "2025年12月30日 21:45");
    assert_eq!(en(at("2025-12-30T21:45:00+08:00")), "Dec 30, 2025 21:45");
}

#[test]
fn future_within_skew_reads_just_now() {
    assert_eq!(zh(ago(-Duration::seconds(1))), "刚刚");
    assert_eq!(zh(ago(-Duration::minutes(5))), "刚刚");
    assert_eq!(en(ago(-Duration::minutes(5))), "Just now");
}

#[test]
fn future_beyond_skew_is_absolute() {
    assert_eq!(zh(ago(-Duration::minutes(6))), "14:36");
    assert_eq!(en(ago(-Duration::hours(2))), "16:30");
    assert_eq!(zh(at("2026-09-27T09:00:00+08:00")), "9月27日 09:00");
    assert_eq!(zh(at("2027-01-02T09:00:00+08:00")), "2027年1月2日 09:00");
}

#[test]
fn calendar_follows_the_viewer_offset() {
    // 2026-09-25T20:00Z is 04:00 on the 26th in UTC+8: 10 hours ago, today.
    assert_eq!(zh(at("2026-09-25T20:00:00Z")), "10 小时前");
    // The same instant seen from UTC-7 at the same moment is yesterday 13:00.
    let pacific = now().with_timezone(&FixedOffset::west_opt(7 * 3600).unwrap());
    assert_eq!(
        format(at("2026-09-25T20:00:00Z"), pacific, TimeLocale::ZhCn).label,
        "10 小时前"
    );
    let pacific_morning = DateTime::parse_from_rfc3339("2026-09-26T08:00:00-07:00").unwrap();
    assert_eq!(
        format(
            at("2026-09-25T20:00:00Z"),
            pacific_morning,
            TimeLocale::ZhCn
        )
        .label,
        "昨天 13:00"
    );
}

#[test]
fn full_timestamp_has_date_weekday_and_seconds() {
    let time = format(at("2026-09-26T14:28:07+08:00"), now(), TimeLocale::ZhCn);
    assert_eq!(time.label, "1 分钟前");
    assert_eq!(time.full, "2026年9月26日 周六 14:28:07");
    let time = format(at("2026-09-03T09:04:05+08:00"), now(), TimeLocale::En);
    assert_eq!(time.full, "Thu, Sep 3, 2026 09:04:05");
}

#[test]
fn rfc3339_input_and_locale_tags() {
    let time = format_rfc3339("2026-09-26T06:25:00Z", now(), TimeLocale::from_tag("zh-CN"));
    assert_eq!(time.unwrap().label, "5 分钟前");
    assert_eq!(
        format_rfc3339("2026-09-26T06:25:00Z", now(), TimeLocale::from_tag("en-US"))
            .unwrap()
            .label,
        "5 min ago"
    );
    assert!(format_rfc3339("not a time", now(), TimeLocale::ZhCn).is_none());
    assert_eq!(TimeLocale::from_tag("ZH_hans"), TimeLocale::ZhCn);
    assert_eq!(TimeLocale::from_tag(""), TimeLocale::En);
}

#[test]
fn next_change_waits_for_the_label_to_move() {
    // Just now changes when the message turns one minute old.
    assert_eq!(
        next_change(ago(Duration::seconds(20)), now()),
        Some(Duration::seconds(40))
    );
    // 5 分钟前 becomes 6 分钟前 after the remainder of the minute.
    assert_eq!(
        next_change(ago(Duration::seconds(330)), now()),
        Some(Duration::seconds(30))
    );
    // 2 小时前 becomes 3 小时前 in 50 minutes.
    assert_eq!(
        next_change(ago(Duration::minutes(130)), now()),
        Some(Duration::minutes(50))
    );
    // Late in the day the hour label yields to midnight, when it becomes 昨天.
    let late = DateTime::parse_from_rfc3339("2026-09-26T23:50:00+08:00").unwrap();
    assert_eq!(
        next_change(at("2026-09-26T21:20:00+08:00"), late),
        Some(Duration::minutes(10))
    );
    // Yesterday changes at midnight, a weekday when it turns seven days
    // old; dates never do.
    assert_eq!(
        next_change(at("2026-09-25T10:00:00+08:00"), now()),
        Some(Duration::minutes(570))
    );
    assert_eq!(
        next_change(at("2026-09-24T10:00:00+08:00"), now()),
        Some(Duration::minutes(570) + Duration::days(4))
    );
    assert_eq!(next_change(at("2026-09-01T10:00:00+08:00"), now()), None);
    // A future instant beyond tolerance changes when it enters the window.
    assert_eq!(
        next_change(ago(-Duration::minutes(8)), now()),
        Some(Duration::minutes(3))
    );
    assert_eq!(
        next_change(ago(-Duration::minutes(2)), now()),
        Some(Duration::minutes(3))
    );
}

#[test]
fn every_label_changes_when_next_change_says() {
    // Walk a range of ages; the label just before the reported change equals
    // the current one and differs right after it.
    for seconds in (0..(9 * 86_400)).step_by(97) {
        let at = ago(Duration::seconds(seconds));
        let Some(wait) = next_change(at, now()) else {
            continue;
        };
        let before = format(at, now() + wait - Duration::seconds(1), TimeLocale::ZhCn).label;
        let after = format(at, now() + wait, TimeLocale::ZhCn).label;
        assert_eq!(before, zh(at), "age {seconds}s");
        assert_ne!(before, after, "age {seconds}s");
    }
}
