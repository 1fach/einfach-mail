use std::time::SystemTime;

use einfach_mail_domain::{
    AccountId, MailDraft, MailReplyDraft, MailSendOperationId, PendingMailSend,
};

use crate::shared::reservations::{AccountReservationError, OperationReservation};
use crate::{MailCache, MailCacheError, MailProvider, MailSendProviderError};

use super::{
    MailSendOutcome, MailSendResolutionError, MailSendWorkflowError, MailSubmission, MailWorkflow,
};

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Returns the account's unresolved send marker without contacting Graph.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account does not own the cache or the
    /// encrypted marker cannot be read.
    pub async fn load_pending_send(&self) -> Result<Option<PendingMailSend>, MailCacheError> {
        self.cache.load_pending_send(&self.account_id).await
    }

    /// Stages and submits one message without retrying ambiguous outcomes.
    ///
    /// The body remains in memory and is never written to the cache. A
    /// confirmed acceptance clears the marker; an unknown response leaves it
    /// for explicit user resolution.
    ///
    /// # Errors
    ///
    /// Returns a provider error for a definitive rejection, a cache error
    /// before dispatch, [`MailSendWorkflowError::PendingResolutionRequired`]
    /// when another send remains unresolved, or
    /// [`MailSendWorkflowError::OperationInProgress`] when the account is busy.
    pub async fn send_message(
        &self,
        draft: MailDraft,
        created_at: SystemTime,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        let _reservation = self.reserve_send(&self.account_id)?;
        self.ensure_no_pending_send().await?;
        let pending = draft.pending_send(
            self.account_id.clone(),
            MailSendOperationId::new(),
            created_at,
        );
        self.stage_send_marker(&pending).await?;
        self.submit_staged_send(MailSubmission::NewMessage(draft), pending)
            .await
    }

    /// Stages and submits one plain-text reply to a still-cached message.
    ///
    /// The reply body remains in memory. The encrypted marker records only the
    /// source message metadata; a reply request is never automatically retried.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendWorkflowError::ReplySourceNotCached`] when the source
    /// has left the cache, [`MailSendWorkflowError::Provider`] for an account
    /// mismatch or definitive provider rejection, a cache error before dispatch,
    /// or the same unresolved-operation errors as [`Self::send_message`].
    pub async fn reply_to_message(
        &self,
        draft: MailReplyDraft,
        created_at: SystemTime,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        if draft.source().account_id() != &self.account_id {
            return Err(MailSendProviderError::AccountMismatch.into());
        }
        let _reservation = self.reserve_send(&self.account_id)?;
        self.ensure_no_pending_send().await?;
        let pending = self.stage_reply_send(&draft, created_at).await?;
        self.submit_staged_send(MailSubmission::Reply(draft), pending)
            .await
    }

    /// Stores source metadata after confirming the reply source remains cached.
    async fn stage_reply_send(
        &self,
        draft: &MailReplyDraft,
        created_at: SystemTime,
    ) -> Result<PendingMailSend, MailSendWorkflowError> {
        let Some((source, _)) = self
            .cache
            .read_message(draft.source().folder(), draft.source().message_id())
            .await?
        else {
            return Err(MailSendWorkflowError::ReplySourceNotCached);
        };
        let pending = draft.pending_send(
            MailSendOperationId::new(),
            source.metadata().subject(),
            created_at,
        );
        self.stage_send_marker(&pending).await?;
        Ok(pending)
    }

    async fn ensure_no_pending_send(&self) -> Result<(), MailSendWorkflowError> {
        if let Some(pending) = self.cache.load_pending_send(&self.account_id).await? {
            return Err(MailSendWorkflowError::PendingResolutionRequired(pending));
        }
        Ok(())
    }

    async fn stage_send_marker(
        &self,
        pending: &PendingMailSend,
    ) -> Result<(), MailSendWorkflowError> {
        match self.cache.stage_pending_send(pending).await {
            Ok(()) => Ok(()),
            Err(MailCacheError::SendAlreadyPending) => {
                match self.cache.load_pending_send(pending.account_id()).await? {
                    Some(pending) => Err(MailSendWorkflowError::PendingResolutionRequired(pending)),
                    None => Err(MailCacheError::SendAlreadyPending.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn submit_staged_send(
        &self,
        submission: MailSubmission,
        pending: PendingMailSend,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        let provider_result = match submission {
            MailSubmission::NewMessage(draft) => {
                self.provider.send_mail(&self.account_id, draft).await
            }
            MailSubmission::Reply(draft) => {
                self.provider
                    .reply_to_message(&self.account_id, draft)
                    .await
            }
        };
        match provider_result {
            Ok(()) => self.finish_accepted_send(pending).await,
            Err(MailSendProviderError::OutcomeUnknown) => {
                Ok(MailSendOutcome::OutcomeUnknown(pending))
            }
            Err(error) => self.finish_rejected_send(pending, error).await,
        }
    }

    async fn finish_rejected_send(
        &self,
        pending: PendingMailSend,
        error: MailSendProviderError,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        match self
            .cache
            .clear_pending_send(&self.account_id, pending.operation_id())
            .await
        {
            Ok(()) => Err(MailSendWorkflowError::Provider(error)),
            Err(cache) => Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending: Box::new(pending),
                provider: error,
                cache,
            }),
        }
    }

    async fn finish_accepted_send(
        &self,
        pending: PendingMailSend,
    ) -> Result<MailSendOutcome, MailSendWorkflowError> {
        match self
            .cache
            .clear_pending_send(&self.account_id, pending.operation_id())
            .await
        {
            Ok(()) => Ok(MailSendOutcome::Accepted),
            Err(_) => Ok(MailSendOutcome::AcceptedNeedsResolution(pending)),
        }
    }

    /// Clears a matching unresolved marker after explicit user review.
    ///
    /// This method never sends or retries a message.
    ///
    /// # Errors
    ///
    /// Returns a cache error for an account mismatch or failed transaction,
    /// [`MailSendResolutionError::PendingResolutionRequired`] when another
    /// operation is pending, or [`MailSendResolutionError::OperationInProgress`]
    /// while another mail operation owns the account.
    pub async fn resolve_pending_send(
        &self,
        operation_id: MailSendOperationId,
    ) -> Result<(), MailSendResolutionError> {
        let _reservation = self.reserve_send(&self.account_id)?;
        let Some(pending) = self.cache.load_pending_send(&self.account_id).await? else {
            return Ok(());
        };
        if pending.operation_id() != operation_id {
            return Err(MailSendResolutionError::PendingResolutionRequired(pending));
        }
        self.cache
            .clear_pending_send(&self.account_id, operation_id)
            .await?;
        Ok(())
    }

    fn reserve_send(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, AccountReservationError> {
        self.reservations.reserve_account(account)
    }
}
