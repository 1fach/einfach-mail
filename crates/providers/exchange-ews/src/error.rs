use thiserror::Error;

use einfach_mail_application::{
    AccountOperationError, CalendarOperationError, MailOperationError, MailSendProviderError,
};

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum EwsError {
    #[error("Exchange connection details are invalid")]
    InvalidConnectionDetails,
    #[error("Exchange Autodiscover did not find a usable EWS endpoint")]
    AutodiscoverFailed,
    #[error("the Exchange credential service is unavailable")]
    CredentialStoreUnavailable,
    #[error("Exchange rejected the NTLMv2 credentials")]
    AccessDenied,
    #[error("Exchange returned invalid data")]
    InvalidResponse,
    #[error("the Exchange response exceeded its resource limits")]
    ResourceLimitExceeded,
    #[error("the Exchange service is unavailable")]
    ServiceUnavailable,
    #[error("the Exchange synchronization state expired")]
    SyncStateExpired,
}

impl From<EwsError> for AccountOperationError {
    fn from(error: EwsError) -> Self {
        match error {
            EwsError::InvalidConnectionDetails => Self::InvalidConnectionDetails,
            EwsError::AutodiscoverFailed => Self::AutodiscoverFailed,
            EwsError::CredentialStoreUnavailable => Self::CredentialStoreUnavailable,
            EwsError::AccessDenied => Self::AccessDenied,
            EwsError::InvalidResponse => Self::ProfileUnavailable,
            EwsError::ResourceLimitExceeded | EwsError::ServiceUnavailable => {
                Self::ServiceUnavailable
            }
            EwsError::SyncStateExpired => Self::ServiceUnavailable,
        }
    }
}

impl From<EwsError> for MailOperationError {
    fn from(error: EwsError) -> Self {
        match error {
            EwsError::AccessDenied => Self::AccessDenied,
            EwsError::CredentialStoreUnavailable => Self::CredentialStoreUnavailable,
            EwsError::InvalidResponse | EwsError::SyncStateExpired => Self::InvalidResponse,
            EwsError::ResourceLimitExceeded => Self::ResourceLimitExceeded,
            EwsError::InvalidConnectionDetails | EwsError::ServiceUnavailable => {
                Self::ServiceUnavailable
            }
            EwsError::AutodiscoverFailed => Self::ServiceUnavailable,
        }
    }
}

impl From<EwsError> for MailSendProviderError {
    fn from(error: EwsError) -> Self {
        match error {
            EwsError::AccessDenied => Self::PermissionRequired,
            EwsError::CredentialStoreUnavailable
            | EwsError::InvalidConnectionDetails
            | EwsError::InvalidResponse
            | EwsError::ResourceLimitExceeded
            | EwsError::ServiceUnavailable
            | EwsError::AutodiscoverFailed
            | EwsError::SyncStateExpired => Self::ServiceUnavailable,
        }
    }
}

impl From<EwsError> for CalendarOperationError {
    fn from(error: EwsError) -> Self {
        match error {
            EwsError::AccessDenied => Self::AccessDenied,
            EwsError::InvalidResponse => Self::InvalidResponse,
            EwsError::ResourceLimitExceeded => Self::ResourceLimitExceeded,
            EwsError::InvalidConnectionDetails
            | EwsError::CredentialStoreUnavailable
            | EwsError::ServiceUnavailable
            | EwsError::AutodiscoverFailed
            | EwsError::SyncStateExpired => Self::ServiceUnavailable,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
