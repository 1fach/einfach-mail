use einfach_mail_domain::{FolderReference, MailFolder, MailMessage, MessageBody, MessageId};

use crate::{MailCache, MailCacheError, MailProvider, prepare_message_body_for_display};

use super::{MailWorkflow, MailWorkflowError, OpenedMessage};

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Returns the account's folders currently stored in the cache.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account or encrypted database is unavailable.
    pub async fn list_cached_folders(&self) -> Result<Vec<MailFolder>, MailWorkflowError> {
        Ok(self.cache.list_folders(&self.account_id).await?)
    }

    /// Returns cached message metadata for one account-scoped folder.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs
    /// to another account, or a cache error when stored data is invalid.
    pub async fn list_cached_messages(
        &self,
        folder: &FolderReference,
    ) -> Result<Vec<MailMessage>, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        Ok(self.cache.list_messages(folder).await?)
    }

    /// Returns the saved sync cutoff and cursor state for one folder.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs
    /// to another account, or a cache error when stored state is invalid.
    pub async fn cached_sync_state(
        &self,
        folder: &FolderReference,
    ) -> Result<Option<crate::MailSyncState>, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        Ok(self.cache.load_sync_state(folder).await?)
    }

    /// Returns whether a folder recovery marker blocks message writes.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs
    /// to another account, or a cache error when the marker cannot be read.
    pub async fn folder_write_sync_required(
        &self,
        folder: &FolderReference,
    ) -> Result<bool, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        Ok(self.cache.folder_write_sync_required(folder).await?)
    }

    /// Opens a cached message and fetches its body only when it is not already cached.
    ///
    /// The returned body is safe for the native reader. Raw content remains in the cache.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the folder belongs
    /// to another account, or provider and cache errors when message data is unavailable.
    ///
    /// Returns [`MailWorkflowError::OperationInProgress`] when another operation owns the account
    /// or folder, or [`MailWorkflowError::StateUnavailable`] when the reservation state cannot be
    /// read.
    pub async fn open_message(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<OpenedMessage, MailWorkflowError> {
        self.ensure_folder_account(folder)?;
        let Some((message, cached_body)) = self.cache.read_message(folder, message_id).await?
        else {
            return Err(MailCacheError::MessageNotCached.into());
        };
        let (message, raw_body, loaded_from_cache) = match cached_body {
            Some(body) => (message, body, true),
            None => self.read_or_fetch_message_body(folder, message_id).await?,
        };
        let body = prepare_message_body_for_display(&raw_body);
        Ok(OpenedMessage {
            message,
            body,
            loaded_from_cache,
        })
    }

    async fn read_or_fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> Result<(MailMessage, MessageBody, bool), MailWorkflowError> {
        let _reservation = self.reserve_folders(std::slice::from_ref(folder))?;
        let Some((message, cached_body)) = self.cache.read_message(folder, message_id).await?
        else {
            return Err(MailCacheError::MessageNotCached.into());
        };
        if let Some(body) = cached_body {
            return Ok((message, body, true));
        }
        let body = self.provider.fetch_message_body(folder, message_id).await?;
        self.cache
            .store_message_body(folder, message_id, body.clone())
            .await?;
        Ok((message, body, false))
    }
}
