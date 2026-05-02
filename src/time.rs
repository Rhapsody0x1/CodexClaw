use chrono::{DateTime, FixedOffset, Local, LocalResult, NaiveTime, TimeZone};

const BEIJING_OFFSET_SECS: i32 = 8 * 3600;

pub fn beijing_tz() -> FixedOffset {
    FixedOffset::east_opt(BEIJING_OFFSET_SECS).expect("valid beijing offset")
}

pub fn now_in_beijing() -> DateTime<FixedOffset> {
    Local::now().with_timezone(&beijing_tz())
}

pub fn server_local_time_for_beijing_hour(hour: u8) -> DateTime<Local> {
    let beijing_now = now_in_beijing();
    let naive = beijing_now
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(u32::from(hour), 0, 0).expect("valid hour"));
    let beijing_dt = match beijing_tz().from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt,
        _ => unreachable!("fixed offset timezone should not be ambiguous"),
    };
    beijing_dt.with_timezone(&Local)
}
