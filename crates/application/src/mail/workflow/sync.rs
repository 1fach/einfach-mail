use std::{
    collections::HashSet,
    time::{Duration, SystemTime},
};

use einfach_mail_domain::{AccountId, FolderReference, MailFolder, MessageDeltaProgress};

use crate::shared::reservations::OperationReservation;
use crate::shared::sync_budget::{
    MAX_DELTA_PAGES_PER_ATTEMPT, MAX_DELTA_RESETS_PER_ATTEMPT, response_bytes_after,
};
use crate::{MailCache, MailProvider, MessageDeltaResponse};

use super::{MailSyncProgress, MailSyncReport, MailWorkflow, MailWorkflowError};

const INITIAL_SYNC_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

impl<P: MailProvider, C: MailCache> MailWorkflow<P, C> {
    /// Synchronizes every visible folder and commits each page with its checkpoint.
    ///
    /// `sync_started_at` supplies a deterministic clock value for the initial
    /// 90-day cutoff. Existing folder scopes retain their stored cutoff.
    /// One reservation covers folder enumeration, folder replacement, and every delta request.
    /// The handler receives `Rebuilding` after a replacement checkpoint is saved.
    ///
    /// # Panics
    ///
    /// Panics if `progress_handler` panics.
    ///
    /// # Errors
    ///
    /// Returns [`MailWorkflowError::AccountMismatch`] when the provider returns a folder for
    /// another account, or a provider, cache, synchronization-state, or resource-limit error. Returns
    /// [`MailWorkflowError::OperationInProgress`] when another operation owns this account.
    pub async fn synchronize_account(
        &self,
        sync_started_at: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<MailSyncReport, MailWorkflowError> {
        let _reservation = self.reserve_account(&self.account_id)?;
        self.recovery.mark_mail_sync_required();
        let initial_cutoff = sync_started_at
            .checked_sub(INITIAL_SYNC_WINDOW)
            .ok_or(MailWorkflowError::ClockUnavailable)?;
        let folders = self.provider.list_folders(&self.account_id).await?;
        if folders
            .iter()
            .any(|folder| folder.reference().account_id() != &self.account_id)
        {
            return Err(MailWorkflowError::AccountMismatch);
        }
        self.cache
            .ensure_account_sync_scope(&self.account_id, initial_cutoff)
            .await?;
        self.cache
            .replace_folders(&self.account_id, folders.clone())
            .await?;
        let page_count = self
            .sync_folders(&folders, initial_cutoff, progress_handler)
            .await?;
        self.recovery.mark_mail_sync_ready();
        Ok(MailSyncReport {
            folder_count: folders.len(),
            page_count,
        })
    }

    async fn sync_folders(
        &self,
        folders: &[MailFolder],
        initial_cutoff: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<usize, MailWorkflowError> {
        let mut page_count = 0;
        for folder in folders {
            page_count += self
                .sync_folder(folder.reference(), initial_cutoff, progress_handler)
                .await?;
        }
        Ok(page_count)
    }

    async fn sync_folder(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
        progress_handler: &(dyn Fn(MailSyncProgress) + Send + Sync),
    ) -> Result<usize, MailWorkflowError> {
        let mut state = self.cache.ensure_sync_state(folder, initial_cutoff).await?;
        let mut seen_requests = HashSet::new();
        let mut page_count = 0;
        let mut response_bytes = 0_usize;
        let mut reset_count = 0;
        loop {
            let expected_state = state.clone();
            let request = expected_state.request();
            let request_cursor = request_cursor(&request);
            if !seen_requests.insert(request_cursor) {
                return Err(MailWorkflowError::RepeatedCursor);
            }
            match self.provider.message_delta_page(folder, request).await? {
                MessageDeltaResponse::Reset(reset_link) => {
                    state = self
                        .cache
                        .begin_full_resync(folder, &expected_state, reset_link)
                        .await?;
                    progress_handler(MailSyncProgress::Rebuilding);
                    if reset_count == MAX_DELTA_RESETS_PER_ATTEMPT {
                        return Err(MailWorkflowError::RepeatedDeltaReset);
                    }
                    reset_count += 1;
                    seen_requests.clear();
                }
                MessageDeltaResponse::Page(response) => {
                    let (page, page_bytes) = response.into_parts();
                    response_bytes = response_bytes_after(response_bytes, page_bytes)
                        .ok_or(MailWorkflowError::ResponseLimitExceeded)?;
                    if let MessageDeltaProgress::Next(link) = page.progress()
                        && seen_requests.contains(&Some(link.clone()))
                    {
                        return Err(MailWorkflowError::RepeatedCursor);
                    }
                    state = self
                        .cache
                        .commit_delta_page(folder, &expected_state, page, SystemTime::now())
                        .await?;
                    page_count += 1;
                    if state.next_link().is_none() {
                        return Ok(page_count);
                    }
                    if page_count == MAX_DELTA_PAGES_PER_ATTEMPT {
                        return Err(MailWorkflowError::PageLimitExceeded);
                    }
                }
            }
        }
    }

    fn reserve_account(
        &self,
        account: &AccountId,
    ) -> Result<OperationReservation, MailWorkflowError> {
        self.reservations
            .reserve_account(account)
            .map_err(MailWorkflowError::from)
    }
}

fn request_cursor(
    request: &crate::MessageDeltaRequest,
) -> Option<einfach_mail_domain::MessageDeltaLink> {
    match request {
        crate::MessageDeltaRequest::Initial => None,
        crate::MessageDeltaRequest::Continue(link) => Some(link.clone()),
    }
}
