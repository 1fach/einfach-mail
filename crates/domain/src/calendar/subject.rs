pub(crate) fn normalized_event_subject(raw_subject: &str) -> Option<&str> {
    let subject = raw_subject.trim();
    (!subject.is_empty() && !subject.chars().any(char::is_control)).then_some(subject)
}
