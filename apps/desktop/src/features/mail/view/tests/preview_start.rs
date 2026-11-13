use super::*;

#[gpui_kit::test]
fn preview_opens_the_first_cached_message_in_the_reader(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let account = synthetic_account_id();
    let (profile, cache, message_id) = fixture(account);
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (view, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));

    context.run_until_parked();
    assert!(
        context
            .cx
            .update(|cx| view.update(cx, |view, cx| view.open_first_message_for_preview(cx)))
    );
    context.run_until_parked();

    assert!(context.cx.update(|cx| {
        matches!(
            &view.read(cx).message_detail_state,
            MessageDetailState::Opened(opened) if opened.message().id() == &message_id
        ) && view.read(cx).compact_pane == CompactMailPane::MessageReader
    }));
}
