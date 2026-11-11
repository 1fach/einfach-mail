use std::time::{Duration, SystemTime};

pub(crate) fn relative_age_label(last_sync: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(last_sync)
        .unwrap_or(Duration::ZERO);
    match elapsed.as_secs() {
        seconds if seconds < 60 => "less than a minute ago".to_owned(),
        seconds if seconds < 3_600 => format!("{} min ago", seconds / 60),
        seconds if seconds < 86_400 => format!("{} hr ago", seconds / 3_600),
        seconds => format!("{} days ago", seconds / 86_400),
    }
}
