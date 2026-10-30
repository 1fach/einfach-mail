use gpui_kit::*;

struct HeyMail;

impl Render for HeyMail {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child("Hey Mail")
            .child("Native desktop shell. Mail access is not connected yet.")
    }
}

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);

        open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| HeyMail))
            .expect("failed to open the Hey Mail window");
    });
}
