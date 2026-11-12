use einfach_mail_application::{MailOperationError, MailWorkflowError};
use einfach_mail_domain::{FolderReference, MessageId};
use gpui_kit::Role;

pub(super) enum MessageWriteStatus<T: MessageWriteFeedbackText> {
    Idle,
    Pending {
        message_id: MessageId,
        requested: T,
    },
    Confirmed {
        message_id: MessageId,
        status: T,
    },
    ConfirmedNeedsSync {
        message_id: MessageId,
        status: T,
    },
    Failed {
        message_id: MessageId,
        requested: T,
        error: MailWorkflowError,
    },
}

pub(super) struct MessageWriteRequest<T: Copy> {
    pub(super) folder: FolderReference,
    pub(super) message_id: MessageId,
    pub(super) requested: T,
}

pub(super) struct MessageWriteFeedback {
    pub(super) message: String,
    pub(super) role: Role,
    pub(super) action: Option<MessageWriteFeedbackAction>,
}

pub(super) enum MessageWriteFeedbackAction {
    ReviewPermission,
}

impl MessageWriteFeedback {
    fn status(message: String) -> Self {
        Self {
            message,
            role: Role::Status,
            action: None,
        }
    }

    fn failure(message: String, error: MailWorkflowError) -> Self {
        Self {
            message,
            role: Role::Alert,
            action: matches!(
                error,
                MailWorkflowError::Provider(MailOperationError::AccessDenied)
            )
            .then_some(MessageWriteFeedbackAction::ReviewPermission),
        }
    }
}

pub(super) trait MessageWriteFeedbackText: Copy {
    fn progress_message(self) -> String;
    fn confirmed_message(self) -> &'static str;
    fn cache_sync_message(self) -> &'static str;
    fn failure_message(self, error: MailWorkflowError) -> String;
}

impl<T: MessageWriteFeedbackText> MessageWriteStatus<T> {
    pub(super) fn is_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            self,
            Self::Pending {
                message_id: pending_id,
                ..
            } if pending_id == message_id
        )
    }

    pub(super) fn feedback_for(&self, message_id: &MessageId) -> Option<MessageWriteFeedback> {
        if self.message_id()? != message_id {
            return None;
        }

        self.feedback()
    }

    fn feedback(&self) -> Option<MessageWriteFeedback> {
        match self {
            Self::Idle => None,
            Self::Pending { requested, .. } => {
                Some(MessageWriteFeedback::status(requested.progress_message()))
            }
            Self::Confirmed { status, .. } => Some(MessageWriteFeedback::status(
                status.confirmed_message().to_owned(),
            )),
            Self::ConfirmedNeedsSync { status, .. } => Some(MessageWriteFeedback::status(
                status.cache_sync_message().to_owned(),
            )),
            Self::Failed {
                requested, error, ..
            } => Some(MessageWriteFeedback::failure(
                requested.failure_message(*error),
                *error,
            )),
        }
    }

    fn message_id(&self) -> Option<&MessageId> {
        match self {
            Self::Idle => None,
            Self::Pending { message_id, .. }
            | Self::Confirmed { message_id, .. }
            | Self::ConfirmedNeedsSync { message_id, .. }
            | Self::Failed { message_id, .. } => Some(message_id),
        }
    }
}
