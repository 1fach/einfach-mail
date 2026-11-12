use einfach_mail_application::{CalendarOperationError, MailOperationError};

use super::EwsError;

#[test]
fn calendar_errors_preserve_invalid_response_and_resource_limit_categories() {
    assert_eq!(
        CalendarOperationError::from(EwsError::InvalidResponse),
        CalendarOperationError::InvalidResponse
    );
    assert_eq!(
        CalendarOperationError::from(EwsError::ResourceLimitExceeded),
        CalendarOperationError::ResourceLimitExceeded
    );
}

#[test]
fn access_denied_remains_distinct_for_mail_and_calendar_operations() {
    assert_eq!(
        MailOperationError::from(EwsError::AccessDenied),
        MailOperationError::AccessDenied
    );
    assert_eq!(
        CalendarOperationError::from(EwsError::AccessDenied),
        CalendarOperationError::AccessDenied
    );
}
