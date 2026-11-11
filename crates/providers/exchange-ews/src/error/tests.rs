use einfach_mail_application::CalendarOperationError;

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
