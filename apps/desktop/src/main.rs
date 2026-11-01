//! Native desktop entry point for Einfach Mail.

use gpui_kit::*;
use std::sync::mpsc;

mod account_view;
mod mail_view;

use account_view::{AccountView, account_workflow_from_environment};

fn main() -> Result<(), std::io::Error> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<std::io::Result<()>>();

    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);
        let account_services = account_workflow_from_environment();

        let window_open_attempt = open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| {
                AccountView::new(
                    account_services.workflow,
                    account_services.mail_provider,
                    cx,
                )
            })
        })
        .map(|_| ())
        .map_err(|error| {
            std::io::Error::other(format!("failed to open the Einfach Mail window: {error:#}"))
        });

        if window_open_attempt.is_err() {
            cx.quit();
        }

        window_open_attempt_sender
            .send(window_open_attempt)
            .expect("the main thread keeps the receiver alive until GPUI exits");
    });

    match window_open_attempt_receiver.try_recv() {
        Ok(window_open_attempt) => window_open_attempt,
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => Err(
            std::io::Error::other("GPUI exited before opening the Einfach Mail window"),
        ),
    }
}
