use super::*;

#[gpui_kit::test]
async fn native_event_composer_renders_date_pickers_and_recovers_create_outcomes(
    cx: &mut TestAppContext,
) {
    super::composer_tests::run_native_event_composer_renders_date_pickers_and_recovers_create_outcomes(cx).await;
}

#[gpui_kit::test]
async fn denied_calendar_write_emits_permission_review_request(cx: &mut TestAppContext) {
    super::composer_tests::run_denied_calendar_write_emits_permission_review_request(cx).await;
}

#[gpui_kit::test]
async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
    super::composer_tests::run_native_event_editor_updates_a_cached_timed_appointment(cx).await;
}

#[gpui_kit::test]
async fn native_event_editor_confirms_and_serializes_deletion(cx: &mut TestAppContext) {
    super::composer_tests::run_native_event_editor_confirms_and_serializes_deletion(cx).await;
}
