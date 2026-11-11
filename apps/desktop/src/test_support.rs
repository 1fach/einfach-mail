use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{App, VisualContext as _, VisualTestContext, Window};

pub(crate) async fn wait_for_ui_condition(
    context: &mut VisualTestContext,
    condition: impl FnMut(&mut Window, &mut App) -> bool,
) {
    let window = context.window_handle();
    context.update(|_, app| app.set_reduce_motion(true));
    context
        .cx
        .wait_for(window, Duration::from_secs(1), condition)
        .await;
}

pub(crate) fn test_element_is_visible(window: &Window, id: &'static str) -> bool {
    window.try_find(id).is_some_and(|element| element.visible())
}

pub(crate) mod calendar;
