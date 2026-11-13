#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
#[cfg(test)]
use gpui_kit::{App, VisualContext as _, VisualTestContext, Window};

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn test_element_is_visible(window: &Window, id: &'static str) -> bool {
    window.try_find(id).is_some_and(|element| element.visible())
}

#[cfg(test)]
pub(crate) mod account;

pub(crate) mod calendar;
pub(crate) mod mail;

#[cfg(test)]
pub(crate) use account::{synthetic_account_id, synthetic_account_profile};
