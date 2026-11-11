use einfach_mail_application::{MailCacheError, MailSendProviderError};
use einfach_mail_domain::PendingMailSend;

#[derive(Clone, Copy)]
pub(super) enum PendingSendCertainty {
    Accepted,
    Rejected(MailSendProviderError),
    Unknown,
}

pub(super) enum PendingSendState {
    Available(Option<PendingSendMarker>),
    Unavailable(Option<PendingSendMarker>),
}

#[derive(Clone)]
pub(super) struct PendingSendMarker {
    pub(super) pending: PendingMailSend,
    pub(super) certainty: PendingSendCertainty,
}

impl PendingSendState {
    pub(super) fn from_result(pending: Result<Option<PendingMailSend>, MailCacheError>) -> Self {
        match pending {
            Ok(pending) => Self::Available(pending.map(|pending| PendingSendMarker {
                pending,
                certainty: PendingSendCertainty::Unknown,
            })),
            Err(_) => Self::Unavailable(None),
        }
    }

    pub(super) fn marker(&self) -> Option<&PendingSendMarker> {
        match self {
            Self::Available(marker) | Self::Unavailable(marker) => marker.as_ref(),
        }
    }

    pub(super) fn pending(&self) -> Option<&PendingMailSend> {
        self.marker().map(|marker| &marker.pending)
    }

    pub(super) fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    pub(super) fn set_marker(&mut self, pending: PendingMailSend, certainty: PendingSendCertainty) {
        let marker = Some(PendingSendMarker { pending, certainty });
        *self = match self {
            Self::Available(_) => Self::Available(marker),
            Self::Unavailable(_) => Self::Unavailable(marker),
        };
    }

    pub(super) fn clear_marker(&mut self) {
        *self = match self {
            Self::Available(_) => Self::Available(None),
            Self::Unavailable(_) => Self::Unavailable(None),
        };
    }

    pub(super) fn apply_snapshot(
        &mut self,
        pending_snapshot: Result<Option<PendingMailSend>, MailCacheError>,
    ) {
        match pending_snapshot {
            Ok(pending) => self.apply_available_snapshot(pending),
            Err(_) => self.retain_marker_as_unknown(),
        }
    }

    fn apply_available_snapshot(&mut self, pending: Option<PendingMailSend>) {
        let marker = pending.map(|pending| {
            let certainty = self
                .marker()
                .filter(|marker| marker.pending.operation_id() == pending.operation_id())
                .map_or(PendingSendCertainty::Unknown, |marker| marker.certainty);
            PendingSendMarker { pending, certainty }
        });
        *self = Self::Available(marker);
    }

    fn retain_marker_as_unknown(&mut self) {
        let marker = self.marker().cloned().map(|mut marker| {
            marker.certainty = PendingSendCertainty::Unknown;
            marker
        });
        *self = Self::Unavailable(marker);
    }
}
