use std::future::Future;

use einfach_mail_domain::{FolderReference, FollowUpFlagStatus, MailFolder, MessageId, ReadState};

use crate::shared::recovery::AccountWorkflowRecovery;
use crate::shared::reservations::OperationReservation;
use crate::{MailCache, MailCacheError, MailOperationError, MailProvider, WellKnownMailFolder};

use super::{
    FollowUpFlagUpdateOutcome, MailWorkflow, MailWorkflowError, MessageMoveOutcome,
    ReadStateUpdateOutcome, WellKnownFolderMoveOutcome,
};

fn mail_folder_is_visible(folders: &[MailFolder], reference: &FolderReference) -> bool {
    folders.iter().any(|folder| folder.reference() == reference)
}

enum MissingMoveFolder {
    Source,
    Destination,
}

fn validate_visible_move_folders(
    folders: &[MailFolder],
    source: &FolderReference,
    destination: &FolderReference,
) -> Result<(), MissingMoveFolder> {
    if !mail_folder_is_visible(folders, source) {
        return Err(MissingMoveFolder::Source);
    }
    if !mail_folder_is_visible(folders, destination) {
        return Err(MissingMoveFolder::Destination);
    }
    Ok(())
}

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Sets a cached message's read state after Graph confirms the requested value.
    ///
    /// The operation commits a recovery marker before dispatching the provider request. A complete
    /// folder delta round clears the marker after an unknown outcome or process exit.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs to another account,
    /// a provider error when the request is rejected, a cache error when the message is
    /// absent or the marker cannot be committed, [`MailWorkflowError::FolderSyncRequired`] while
    /// an earlier update awaits reconciliation, or [`MailWorkflowError::OperationInProgress`]
    /// when another operation owns the account or folder.
    pub async fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> Result<ReadStateUpdateOutcome, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        let _reservation = self.prepare_message_write(folder, message_id).await?;

        let confirmed = self
            .confirm_message_update(
                folder,
                requested,
                self.provider
                    .set_message_read_state(folder, message_id, requested),
            )
            .await?;

        self.store_confirmed_read_state(folder, message_id, confirmed)
            .await
    }

    async fn store_confirmed_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        confirmed: ReadState,
    ) -> Result<ReadStateUpdateOutcome, MailWorkflowError> {
        match self
            .cache
            .set_message_read_state(folder, message_id, confirmed)
            .await
        {
            Ok(()) => Ok(ReadStateUpdateOutcome::ConfirmedAndCached(confirmed)),
            Err(cache_error) => Ok(ReadStateUpdateOutcome::ConfirmedNeedsSync {
                state: confirmed,
                cache_error,
            }),
        }
    }

    /// Sets a cached message's follow-up flag status after the provider confirms it.
    ///
    /// A SQLCipher recovery marker is committed before the provider request. A complete folder
    /// delta round removes the marker after an unknown outcome or process exit.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs to another account,
    /// a provider error when the request is rejected, a cache error when the message is
    /// absent or the marker cannot be committed, [`MailWorkflowError::FolderSyncRequired`] while
    /// an earlier update awaits reconciliation, or [`MailWorkflowError::OperationInProgress`]
    /// when another operation owns the account or folder.
    pub async fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: FollowUpFlagStatus,
    ) -> Result<FollowUpFlagUpdateOutcome, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        let _reservation = self.prepare_message_write(folder, message_id).await?;
        let confirmed = self
            .confirm_message_update(
                folder,
                requested,
                self.provider
                    .set_message_follow_up_flag_status(folder, message_id, requested),
            )
            .await?;

        match self
            .cache
            .set_message_follow_up_flag_status(folder, message_id, confirmed)
            .await
        {
            Ok(()) => Ok(FollowUpFlagUpdateOutcome::ConfirmedAndCached(confirmed)),
            Err(cache_error) => Ok(FollowUpFlagUpdateOutcome::ConfirmedNeedsSync {
                status: confirmed,
                cache_error,
            }),
        }
    }

    /// Moves a cached message between two visible folders in one account.
    ///
    /// The operation holds both folder reservations through provider dispatch
    /// and the cache transaction. A confirmed provider move remains successful
    /// when the cache transaction fails; the caller then synchronizes.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when either folder belongs
    /// to another account, [`MailWorkflowError::InvalidMoveDestination`] for identical,
    /// cross-account, or non-visible folders, a cache error when the folder
    /// tree or source message is unavailable, or a provider error when Graph
    /// does not confirm the move,
    /// or [`MailWorkflowError::OperationInProgress`] when an affected scope is busy.
    pub async fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> Result<MessageMoveOutcome, MailWorkflowError> {
        self.ensure_folder_account(source_folder)?;
        self.ensure_folder_account(destination_folder)?;
        validate_move_folder_pair(source_folder, destination_folder)?;
        let _reservation =
            self.reserve_folders(&[source_folder.clone(), destination_folder.clone()])?;
        self.ensure_account_sync_ready()?;
        let visible_folders = self.cache.list_folders(source_folder.account_id()).await?;
        validate_visible_move_folders(&visible_folders, source_folder, destination_folder)
            .map_err(|_| MailWorkflowError::InvalidMoveDestination)?;
        self.ensure_move_folders_writable(source_folder, destination_folder)
            .await?;
        self.ensure_cached_message(source_folder, message_id)
            .await?;
        self.confirm_and_cache_move(source_folder, message_id, destination_folder)
            .await
    }

    /// Moves a cached message into a well-known folder in the active account.
    ///
    /// The workflow reserves the source before resolving the destination, then
    /// holds both folder reservations through provider confirmation and cache reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the source belongs to
    /// another account, [`MailWorkflowError::AccountSyncRequired`] before a successful
    /// account sync, [`MailWorkflowError::AlreadyInWellKnownFolder`] when the
    /// source is already the requested folder,
    /// [`MailWorkflowError::WellKnownFolderUnavailable`] when the resolved
    /// destination is not visible in this account,
    /// [`MailWorkflowError::FolderSyncRequired`] when either folder needs
    /// reconciliation, a cache or provider error when the message cannot be
    /// moved, or [`MailWorkflowError::OperationInProgress`] when an affected
    /// scope is busy.
    pub async fn move_message_to_well_known_folder(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination: WellKnownMailFolder,
    ) -> Result<WellKnownFolderMoveOutcome, MailWorkflowError> {
        self.ensure_folder_account(source_folder)?;
        let _source_reservation = self.reserve_folders(std::slice::from_ref(source_folder))?;
        self.ensure_account_sync_ready()?;
        self.ensure_cached_message(source_folder, message_id)
            .await?;

        let destination_folder = self
            .resolve_well_known_folder(source_folder, destination)
            .await?;

        let _destination_reservation =
            self.reserve_folders(std::slice::from_ref(&destination_folder))?;
        let visible_folders = self.cache.list_folders(source_folder.account_id()).await?;
        validate_visible_move_folders(&visible_folders, source_folder, &destination_folder)
            .map_err(|missing| match missing {
                MissingMoveFolder::Source => MailWorkflowError::InvalidMoveDestination,
                MissingMoveFolder::Destination => {
                    MailWorkflowError::WellKnownFolderUnavailable(destination)
                }
            })?;
        self.ensure_move_folders_writable(source_folder, &destination_folder)
            .await?;
        let cache_outcome = self
            .confirm_and_cache_move(source_folder, message_id, &destination_folder)
            .await?;
        Ok(WellKnownFolderMoveOutcome {
            destination_folder,
            cache_outcome,
        })
    }

    async fn resolve_well_known_folder(
        &self,
        source_folder: &FolderReference,
        destination: WellKnownMailFolder,
    ) -> Result<FolderReference, MailWorkflowError> {
        let destination_folder = self
            .provider
            .well_known_folder(source_folder.account_id(), destination)
            .await?;
        if destination_folder.account_id() != source_folder.account_id() {
            return Err(MailWorkflowError::WellKnownFolderUnavailable(destination));
        }
        if source_folder == &destination_folder {
            return Err(MailWorkflowError::AlreadyInWellKnownFolder(destination));
        }
        Ok(destination_folder)
    }

    async fn ensure_cached_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<(), MailWorkflowError> {
        if self
            .cache
            .read_message(source_folder, message_id)
            .await?
            .is_some()
        {
            Ok(())
        } else {
            Err(MailCacheError::MessageNotCached.into())
        }
    }

    async fn ensure_move_folders_writable(
        &self,
        source_folder: &FolderReference,
        destination_folder: &FolderReference,
    ) -> Result<(), MailWorkflowError> {
        for folder in [source_folder, destination_folder] {
            if self.cache.folder_write_sync_required(folder).await? {
                return Err(MailWorkflowError::FolderSyncRequired);
            }
        }
        Ok(())
    }

    fn ensure_account_sync_ready(&self) -> Result<(), MailWorkflowError> {
        if self.recovery.mail_sync_ready() {
            Ok(())
        } else {
            Err(MailWorkflowError::AccountSyncRequired)
        }
    }

    async fn confirm_and_cache_move(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> Result<MessageMoveOutcome, MailWorkflowError> {
        let mut reconciliation = MoveReconciliationGuard::new(&self.recovery);
        if let Err(error) = self
            .provider
            .move_message(source_folder, message_id, destination_folder)
            .await
        {
            if error != MailOperationError::MoveOutcomeUnknown {
                reconciliation.complete();
            }
            return Err(error.into());
        }
        match self
            .cache
            .move_message(source_folder, destination_folder, message_id)
            .await
        {
            Ok(()) => {
                reconciliation.complete();
                Ok(MessageMoveOutcome::ConfirmedAndCached)
            }
            Err(_) => Ok(MessageMoveOutcome::ConfirmedNeedsSync),
        }
    }

    async fn prepare_message_write(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<OperationReservation, MailWorkflowError> {
        let reservation = self.reserve_folders(std::slice::from_ref(folder))?;
        if self.cache.folder_write_sync_required(folder).await? {
            return Err(MailWorkflowError::FolderSyncRequired);
        }
        if self
            .cache
            .message_read_state(folder, message_id)
            .await?
            .is_none()
        {
            return Err(MailCacheError::MessageNotCached.into());
        }
        self.cache.require_folder_write_sync(folder).await?;
        Ok(reservation)
    }

    async fn confirm_message_update<T, F>(
        &self,
        folder: &FolderReference,
        requested: T,
        provider_update: F,
    ) -> Result<T, MailWorkflowError>
    where
        T: Copy + Eq + Send,
        F: Future<Output = Result<T, MailOperationError>> + Send,
    {
        let confirmed = match provider_update.await {
            Ok(confirmed) => confirmed,
            Err(MailOperationError::MessageUpdateOutcomeUnknown) => {
                return Err(MailOperationError::MessageUpdateOutcomeUnknown.into());
            }
            Err(error) => {
                self.cache
                    .clear_folder_write_sync_requirement(folder)
                    .await?;
                return Err(error.into());
            }
        };
        (confirmed == requested)
            .then_some(confirmed)
            .ok_or_else(|| MailOperationError::MessageUpdateOutcomeUnknown.into())
    }
}

fn validate_move_folder_pair(
    source_folder: &FolderReference,
    destination_folder: &FolderReference,
) -> Result<(), MailWorkflowError> {
    if source_folder.account_id() == destination_folder.account_id()
        && source_folder != destination_folder
    {
        Ok(())
    } else {
        Err(MailWorkflowError::InvalidMoveDestination)
    }
}

/// Requires another successful account sync when a move ends without a definitive outcome.
#[must_use = "dropping an unresolved move guard blocks message moves until sync"]
struct MoveReconciliationGuard<'a> {
    recovery: &'a AccountWorkflowRecovery,
    complete: bool,
}

impl<'a> MoveReconciliationGuard<'a> {
    fn new(recovery: &'a AccountWorkflowRecovery) -> Self {
        Self {
            recovery,
            complete: false,
        }
    }

    fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for MoveReconciliationGuard<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.recovery.mark_mail_sync_required();
        }
    }
}
