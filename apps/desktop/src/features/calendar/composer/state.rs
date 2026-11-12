use super::*;

pub(super) enum ComposerState {
    Create(CreateState),
    Edit(EditState),
}

pub(super) enum CreateState {
    Draft,
    DraftFailed(CalendarWorkflowError),
    InvalidDraft(DraftError),
    Pending {
        creation: CalendarEventCreation,
        status: PendingCreationStatus,
    },
    PendingResolutionRequired,
    PendingResolutionFailed(CalendarWorkflowError),
    Completed(CreationOutcome),
}

#[derive(Clone, Copy)]
pub(super) enum PendingCreationStatus {
    Unresolved,
    Creating,
    Checking,
    StillUnresolved,
    Failed(CalendarWorkflowError),
    ConfirmedButNotCached,
    RejectedPendingCleanup,
    RejectedCleanupFailed(CalendarWorkflowError),
    DiscardingRejected,
}

#[derive(Clone, Copy)]
pub(super) enum CreationOutcome {
    Confirmed,
    Rejected,
}

#[derive(Clone, Copy)]
pub(super) enum StatusStyle {
    Success,
    DestructiveStatus,
    Error,
}

pub(super) enum EditState {
    Loading,
    LoadFailed(CalendarWorkflowError),
    Ready {
        versioned_event: Box<VersionedCalendarEvent>,
        status: EditStatus,
    },
}

pub(super) struct EditableEventFormValues {
    pub(super) subject: String,
    pub(super) start: NaiveDateTime,
    pub(super) end: NaiveDateTime,
}

#[derive(Clone, Copy)]
pub(super) enum EditStatus {
    Editing,
    InvalidDraft(DraftError),
    Submitting,
    Failed(CalendarWorkflowError),
    Updated,
    UpdatedButNotCached,
    UpdateOutcomeUnknown,
    Deleting,
    Deleted,
    DeletedButNotCached,
    DeleteOutcomeUnknown,
    DeletionFailed(CalendarWorkflowError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DraftError {
    MissingSubject,
    MissingDate,
    InvalidTime,
    AmbiguousLocalTime,
    OutsideSelectedWeek,
    NoChanges,
}

impl CreateState {
    pub(super) fn from_pending_creation(
        pending_creation: Option<PendingCalendarEventCreation>,
    ) -> Self {
        match pending_creation {
            Some(PendingCalendarEventCreation::Unresolved(creation)) => Self::Pending {
                creation,
                status: PendingCreationStatus::Unresolved,
            },
            Some(PendingCalendarEventCreation::Rejected(creation)) => Self::Pending {
                creation,
                status: PendingCreationStatus::RejectedPendingCleanup,
            },
            None => Self::Draft,
        }
    }

    pub(super) fn submit_label(&self) -> Option<&'static str> {
        match self {
            Self::Draft | Self::DraftFailed(_) | Self::InvalidDraft(_) => Some("Create event"),
            Self::Pending { status, .. } => Some(status.submit_label()),
            Self::PendingResolutionRequired
            | Self::PendingResolutionFailed(_)
            | Self::Completed(_) => None,
        }
    }

    pub(super) fn requires_calendar_write_permission_review(&self) -> bool {
        matches!(
            self,
            Self::Pending {
                status: PendingCreationStatus::Failed(error),
                ..
            }
                | Self::DraftFailed(error) if is_access_denied(*error)
        )
    }
}

impl PendingCreationStatus {
    pub(super) fn submit_label(self) -> &'static str {
        match self {
            Self::DiscardingRejected => "Clearing draft…",
            Self::RejectedPendingCleanup | Self::RejectedCleanupFailed(_) => "Clear rejected draft",
            Self::Creating => "Creating event…",
            Self::Checking => "Checking event status…",
            Self::Unresolved
            | Self::StillUnresolved
            | Self::Failed(_)
            | Self::ConfirmedButNotCached => "Check event status",
        }
    }
}

impl EditStatus {
    pub(super) fn submit_label(self) -> &'static str {
        if matches!(self, Self::Submitting) {
            "Saving changes…"
        } else {
            "Save changes"
        }
    }
}

impl EditState {
    pub(super) fn submit_label(&self) -> Option<&'static str> {
        match self {
            Self::Ready { status, .. } => Some(status.submit_label()),
            Self::Loading | Self::LoadFailed(_) => None,
        }
    }

    pub(super) fn requires_calendar_write_permission_review(&self) -> bool {
        matches!(
            self,
            Self::LoadFailed(error)
                | Self::Ready {
                    status: EditStatus::Failed(error) | EditStatus::DeletionFailed(error),
                    ..
                } if is_access_denied(*error)
        )
    }
}

impl ComposerState {
    pub(super) fn pending_creation(
        &self,
    ) -> Option<(&CalendarEventCreation, PendingCreationStatus)> {
        match self {
            Self::Create(CreateState::Pending { creation, status }) => Some((creation, *status)),
            Self::Create(
                CreateState::Draft
                | CreateState::DraftFailed(_)
                | CreateState::InvalidDraft(_)
                | CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            )
            | Self::Edit(_) => None,
        }
    }

    pub(super) fn can_submit(&self) -> bool {
        match self {
            Self::Create(
                CreateState::Draft | CreateState::DraftFailed(_) | CreateState::InvalidDraft(_),
            ) => true,
            Self::Create(CreateState::Pending { status, .. }) => !matches!(
                status,
                PendingCreationStatus::Creating
                    | PendingCreationStatus::Checking
                    | PendingCreationStatus::DiscardingRejected
            ),
            Self::Create(
                CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            )
            | Self::Edit(EditState::Loading | EditState::LoadFailed(_)) => false,
            Self::Edit(EditState::Ready { status, .. }) => {
                !matches!(status, EditStatus::Submitting | EditStatus::Deleting)
                    && !is_terminal_edit_status(*status)
            }
        }
    }

    pub(super) fn can_delete(&self) -> bool {
        match self {
            Self::Edit(EditState::Ready { .. }) => self.can_submit(),
            Self::Create(_) | Self::Edit(EditState::Loading | EditState::LoadFailed(_)) => false,
        }
    }

    pub(super) fn edit_write_in_progress(&self) -> bool {
        matches!(
            self,
            Self::Edit(EditState::Ready {
                status: EditStatus::Submitting | EditStatus::Deleting,
                ..
            })
        )
    }

    pub(super) fn action_is_terminal(&self) -> bool {
        match self {
            Self::Create(
                CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            )
            | Self::Edit(EditState::Loading | EditState::LoadFailed(_)) => true,
            Self::Create(
                CreateState::Draft
                | CreateState::DraftFailed(_)
                | CreateState::InvalidDraft(_)
                | CreateState::Pending { .. },
            ) => false,
            Self::Edit(EditState::Ready { status, .. }) => is_terminal_edit_status(*status),
        }
    }

    pub(super) fn show_fields(&self) -> bool {
        match self {
            Self::Create(CreateState::Pending { .. }) => false,
            Self::Create(
                CreateState::Draft
                | CreateState::DraftFailed(_)
                | CreateState::InvalidDraft(_)
                | CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            ) => true,
            Self::Edit(EditState::Ready { status, .. }) => !is_terminal_edit_status(*status),
            Self::Edit(EditState::Loading | EditState::LoadFailed(_)) => false,
        }
    }

    pub(super) fn submit_is_loading(&self) -> bool {
        matches!(
            self,
            Self::Create(CreateState::Pending {
                status: PendingCreationStatus::Creating
                    | PendingCreationStatus::Checking
                    | PendingCreationStatus::DiscardingRejected,
                ..
            }) | Self::Edit(EditState::Ready {
                status: EditStatus::Submitting,
                ..
            })
        )
    }

    pub(super) fn submit_label(&self) -> Option<&'static str> {
        match self {
            Self::Create(state) => state.submit_label(),
            Self::Edit(state) => state.submit_label(),
        }
    }

    pub(super) fn validation_error(&self) -> Option<DraftError> {
        match self {
            Self::Create(CreateState::InvalidDraft(error))
            | Self::Edit(EditState::Ready {
                status: EditStatus::InvalidDraft(error),
                ..
            }) => Some(*error),
            Self::Create(CreateState::Draft)
            | Self::Create(CreateState::DraftFailed(_))
            | Self::Create(
                CreateState::Pending { .. }
                | CreateState::PendingResolutionRequired
                | CreateState::PendingResolutionFailed(_)
                | CreateState::Completed(_),
            )
            | Self::Edit(EditState::Loading | EditState::LoadFailed(_))
            | Self::Edit(EditState::Ready { .. }) => None,
        }
    }

    pub(super) fn status_text(&self) -> Option<&'static str> {
        match self {
            Self::Create(state) => super::copy::creation_status_text(state),
            Self::Edit(EditState::Loading) => None,
            Self::Edit(EditState::LoadFailed(error)) => {
                Some(super::copy::update_error_text(*error))
            }
            Self::Edit(EditState::Ready { status, .. }) => match status {
                EditStatus::Deleting
                | EditStatus::Deleted
                | EditStatus::DeletedButNotCached
                | EditStatus::DeleteOutcomeUnknown
                | EditStatus::DeletionFailed(_) => super::copy::deletion_status_text(*status),
                _ => super::copy::update_status_text(*status),
            },
        }
    }

    pub(super) fn requires_calendar_write_permission_review(&self) -> bool {
        match self {
            Self::Create(state) => state.requires_calendar_write_permission_review(),
            Self::Edit(state) => state.requires_calendar_write_permission_review(),
        }
    }

    pub(super) fn status_style(&self) -> StatusStyle {
        match self {
            Self::Create(CreateState::Completed(CreationOutcome::Confirmed))
            | Self::Edit(EditState::Ready {
                status: EditStatus::Updated,
                ..
            }) => StatusStyle::Success,
            Self::Edit(EditState::Ready {
                status: EditStatus::Deleted,
                ..
            }) => StatusStyle::DestructiveStatus,
            _ => StatusStyle::Error,
        }
    }
}

fn is_access_denied(error: CalendarWorkflowError) -> bool {
    matches!(
        error,
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied)
    )
}

fn is_terminal_edit_status(status: EditStatus) -> bool {
    matches!(
        status,
        EditStatus::Updated
            | EditStatus::UpdatedButNotCached
            | EditStatus::UpdateOutcomeUnknown
            | EditStatus::Deleted
            | EditStatus::DeletedButNotCached
            | EditStatus::DeleteOutcomeUnknown
            | EditStatus::DeletionFailed(
                CalendarWorkflowError::InvalidVersionedEvent
                    | CalendarWorkflowError::CalendarWriteSyncRequired
                    | CalendarWorkflowError::Provider(
                        CalendarOperationError::EventChanged
                            | CalendarOperationError::EventNotEditable
                            | CalendarOperationError::DeleteOutcomeUnknown
                    )
            )
            | EditStatus::Failed(
                CalendarWorkflowError::InvalidVersionedEvent
                    | CalendarWorkflowError::InvalidUpdateConfirmation
                    | CalendarWorkflowError::CalendarWriteSyncRequired
                    | CalendarWorkflowError::Provider(
                        CalendarOperationError::EventChanged
                            | CalendarOperationError::EventNotEditable
                            | CalendarOperationError::UpdateOutcomeUnknown
                    )
            )
    )
}
