use std::sync::{Arc, Mutex};

use einfach_mail_domain::{AccountId, CalendarViewReference, FolderReference};

#[derive(Clone, Copy)]
pub(crate) enum AccountReservationError {
    OperationInProgress,
    StateUnavailable,
}

#[derive(Default)]
pub(crate) struct ActiveOperations {
    accounts: std::collections::HashSet<AccountId>,
    folders: std::collections::HashSet<FolderReference>,
    calendar_views: std::collections::HashSet<CalendarViewReference>,
}

enum ReservationKey {
    Account(AccountId),
    Folders(std::collections::HashSet<FolderReference>),
    CalendarView(CalendarViewReference),
}

pub(crate) struct OperationReservation {
    reservations: Arc<Mutex<ActiveOperations>>,
    key: ReservationKey,
}

pub(crate) fn reserve_account(
    active: &Arc<Mutex<ActiveOperations>>,
    account: &AccountId,
) -> Result<OperationReservation, AccountReservationError> {
    let mut reservations = active
        .lock()
        .map_err(|_| AccountReservationError::StateUnavailable)?;
    if reservations.accounts.contains(account)
        || reservations
            .folders
            .iter()
            .any(|folder| folder.account_id() == account)
        || reservations
            .calendar_views
            .iter()
            .any(|view| view.account_id() == account)
    {
        return Err(AccountReservationError::OperationInProgress);
    }
    reservations.accounts.insert(account.clone());
    Ok(OperationReservation {
        reservations: Arc::clone(active),
        key: ReservationKey::Account(account.clone()),
    })
}

pub(crate) fn reserve_folders(
    active: &Arc<Mutex<ActiveOperations>>,
    folders: &[FolderReference],
) -> Result<OperationReservation, AccountReservationError> {
    let folders = folders
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut reservations = active
        .lock()
        .map_err(|_| AccountReservationError::StateUnavailable)?;
    if folders.iter().any(|folder| {
        reservations.accounts.contains(folder.account_id()) || reservations.folders.contains(folder)
    }) {
        return Err(AccountReservationError::OperationInProgress);
    }
    reservations.folders.extend(folders.iter().cloned());
    Ok(OperationReservation {
        reservations: Arc::clone(active),
        key: ReservationKey::Folders(folders),
    })
}

pub(crate) fn reserve_calendar_view(
    active: &Arc<Mutex<ActiveOperations>>,
    view: &CalendarViewReference,
) -> Result<OperationReservation, AccountReservationError> {
    let mut reservations = active
        .lock()
        .map_err(|_| AccountReservationError::StateUnavailable)?;
    if reservations.accounts.contains(view.account_id())
        || !reservations.calendar_views.insert(view.clone())
    {
        return Err(AccountReservationError::OperationInProgress);
    }
    Ok(OperationReservation {
        reservations: Arc::clone(active),
        key: ReservationKey::CalendarView(view.clone()),
    })
}

impl Drop for OperationReservation {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.reservations.lock() {
            match &self.key {
                ReservationKey::Account(account) => {
                    reservations.accounts.remove(account);
                }
                ReservationKey::Folders(folders) => {
                    for folder in folders {
                        reservations.folders.remove(folder);
                    }
                }
                ReservationKey::CalendarView(view) => {
                    reservations.calendar_views.remove(view);
                }
            }
        }
    }
}
