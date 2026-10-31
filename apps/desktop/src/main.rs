//! Native desktop entry point for Einfach Mail.

use gpui_kit::*;
use std::sync::mpsc;

struct EinfachMail;

impl Render for EinfachMail {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child("Einfach Mail")
            .child("Native desktop shell. Mail access is not connected yet.")
    }
}

fn main() -> Result<(), std::io::Error> {
    let (window_open_attempt_sender, window_open_attempt_receiver) =
        mpsc::channel::<std::io::Result<()>>();

    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);

        let window_open_attempt = open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| EinfachMail)
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
