//! Tests public display messages and source chains for domain validation errors.

use std::error::Error;

use einfach_mail_domain::{
    EmptyIdentifier, EmptyMailboxAddress, FolderParentError, InvalidCalendarEventCreation,
    InvalidCalendarEventCreationId, InvalidCalendarEventDraft, InvalidCalendarEventTimeZone,
    InvalidCalendarEventUpdateDraft, InvalidCalendarEventVersion, InvalidCalendarOccurrenceTime,
    InvalidCalendarViewRange, InvalidEditableCalendarEvent, InvalidMailRecipient,
    InvalidMailSubject, MissingAccountLabel, NewCalendarOccurrenceError, NoCalendarEventChanges,
};

#[test]
fn domain_validation_errors_keep_their_display_messages() {
    let errors: [(&dyn Error, &str); 20] = [
        (
            &MissingAccountLabel,
            "account profile has no display name or address",
        ),
        (&EmptyIdentifier, "identifier must not be empty"),
        (&EmptyMailboxAddress, "mailbox address must not be empty"),
        (
            &InvalidMailRecipient,
            "recipient must be one valid bare mail address",
        ),
        (&InvalidMailSubject::Empty, "the message subject is empty"),
        (
            &InvalidMailSubject::Multiline,
            "the message subject contains a line break",
        ),
        (
            &FolderParentError::AccountMismatch,
            "parent folder must belong to the same account",
        ),
        (
            &FolderParentError::SelfParent,
            "a folder cannot be its own parent",
        ),
        (
            &InvalidCalendarViewRange,
            "calendar view bounds must be non-empty and ordered",
        ),
        (
            &InvalidCalendarOccurrenceTime,
            "calendar occurrence bounds must be non-empty and ordered",
        ),
        (
            &NewCalendarOccurrenceError::Incomplete,
            "a new calendar occurrence is missing required time fields",
        ),
        (
            &NewCalendarOccurrenceError::InvalidTime(InvalidCalendarOccurrenceTime),
            "calendar occurrence bounds must be non-empty and ordered",
        ),
        (
            &InvalidCalendarEventVersion,
            "calendar event version must be bounded and control-free",
        ),
        (
            &InvalidEditableCalendarEvent,
            "editable calendar event must use timed values",
        ),
        (
            &InvalidCalendarEventUpdateDraft,
            "calendar event update must contain a title and ordered UTC times",
        ),
        (
            &NoCalendarEventChanges,
            "calendar event update must change at least one value",
        ),
        (
            &InvalidCalendarEventCreationId,
            "calendar creation identifier must be a UUID",
        ),
        (
            &InvalidCalendarEventTimeZone,
            "calendar event time zone must be a non-empty identifier",
        ),
        (
            &InvalidCalendarEventCreation,
            "calendar event endpoints must fit the selected view",
        ),
        (
            &InvalidCalendarEventDraft,
            "calendar event draft must contain a title and ordered timed values",
        ),
    ];

    for (error, expected) in errors {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn invalid_new_occurrence_time_preserves_its_source() {
    let incomplete = NewCalendarOccurrenceError::Incomplete;
    assert!(Error::source(&incomplete).is_none());

    let invalid_time = NewCalendarOccurrenceError::InvalidTime(InvalidCalendarOccurrenceTime);
    let source = Error::source(&invalid_time).expect("invalid time remains the error source");

    assert!(source.is::<InvalidCalendarOccurrenceTime>());
    assert_eq!(
        source.to_string(),
        "calendar occurrence bounds must be non-empty and ordered"
    );
}
