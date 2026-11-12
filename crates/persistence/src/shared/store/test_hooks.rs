use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_sqlite::Client;
use einfach_mail_application::MailCacheError;

type TestOperationGate = (smol::channel::Sender<()>, smol::channel::Receiver<()>);

#[derive(Clone, Copy)]
pub(crate) enum Gate {
    RemovalAfterMarkerCheck,
    PendingSendRead,
}

#[derive(Default)]
pub(crate) struct TestHooks {
    fail_next_pending_send_clear: AtomicBool,
    removal_gate: Mutex<Option<TestOperationGate>>,
    pending_send_read_gate: Mutex<Option<TestOperationGate>>,
}

impl TestHooks {
    pub(super) fn hold_at(
        &self,
        gate: Gate,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        let lock_message = gate.lock_message();
        *self.gate_storage(gate).lock().expect(lock_message) = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) async fn wait_at(&self, gate: Gate) {
        let Some((started, release)) = self.take_gate(gate) else {
            return;
        };
        let started_message = gate.started_message();
        started.send(()).await.expect(started_message);
        let release_message = gate.release_message();
        release.recv().await.expect(release_message);
    }

    pub(super) fn fail_next_pending_send_clear(&self) {
        self.fail_next_pending_send_clear
            .store(true, Ordering::Relaxed);
    }

    pub(crate) fn ensure_pending_send_clear_succeeds(&self) -> Result<(), MailCacheError> {
        if self
            .fail_next_pending_send_clear
            .swap(false, Ordering::Relaxed)
        {
            Err(MailCacheError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(super) async fn set_message_move_failure(client: &Client, enabled: bool) {
        let statement = if enabled {
            "CREATE TRIGGER fail_message_move BEFORE DELETE ON mail_messages BEGIN SELECT RAISE(ABORT, 'synthetic move failure'); END;"
        } else {
            "DROP TRIGGER IF EXISTS fail_message_move;"
        };
        client
            .conn_and_then(|connection| -> Result<(), async_sqlite::Error> {
                connection.execute_batch(statement)?;
                Ok(())
            })
            .await
            .expect("synthetic message move trigger can be updated");
    }

    fn take_gate(&self, gate: Gate) -> Option<TestOperationGate> {
        let lock_message = gate.lock_message();
        self.gate_storage(gate).lock().expect(lock_message).take()
    }

    fn gate_storage(&self, gate: Gate) -> &Mutex<Option<TestOperationGate>> {
        match gate {
            Gate::RemovalAfterMarkerCheck => &self.removal_gate,
            Gate::PendingSendRead => &self.pending_send_read_gate,
        }
    }
}

impl Gate {
    fn lock_message(self) -> &'static str {
        match self {
            Self::RemovalAfterMarkerCheck => "synthetic account-removal gate mutex is healthy",
            Self::PendingSendRead => "synthetic pending-send read gate mutex is healthy",
        }
    }

    fn started_message(self) -> &'static str {
        match self {
            Self::RemovalAfterMarkerCheck => "the account-removal test is waiting at the gate",
            Self::PendingSendRead => "the pending-send test is waiting at the gate",
        }
    }

    fn release_message(self) -> &'static str {
        match self {
            Self::RemovalAfterMarkerCheck => "the account-removal test releases the gate",
            Self::PendingSendRead => "the pending-send test releases the gate",
        }
    }
}
