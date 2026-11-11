use super::*;

struct UnexpectedPermissionRequester;

impl MailPermissionRequester for UnexpectedPermissionRequester {
    fn request_send_permission<'a>(
        &'a self,
        _account_id: &'a AccountId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>> {
        Box::pin(async {
            panic!("test did not expect a send permission request");
        })
    }
}

fn unexpected_permission_requester() -> Arc<dyn MailPermissionRequester> {
    Arc::new(UnexpectedPermissionRequester)
}

pub(super) fn test_mail_view(
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    cx: &mut gpui_kit::Context<MailView>,
) -> MailView {
    MailView::new(profile, workflow, unexpected_permission_requester(), cx)
}

struct TestMailWorkspace {
    folder_navigation: Entity<MailFolderNavigation>,
    mail_view: Entity<MailView>,
}

impl gpui_kit::Render for TestMailWorkspace {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        gpui_kit::div()
            .size_full()
            .flex()
            .child(
                gpui_kit::div()
                    .w(gpui_kit::rems(16.))
                    .h_full()
                    .flex_shrink_0()
                    .child(self.folder_navigation.clone()),
            )
            .child(
                gpui_kit::div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.mail_view.clone()),
            )
    }
}

pub(super) fn test_mail_workspace(
    profile: AccountProfile,
    workflow: Arc<dyn MailSession>,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<gpui_kit::base::Root>,
) -> (gpui_kit::base::Root, Entity<MailView>) {
    let view = cx.new(|cx| test_mail_view(profile, workflow, cx));
    let navigation = cx.new(|cx| MailFolderNavigation::new(view.clone(), cx));
    let workspace = cx.new(|_| TestMailWorkspace {
        folder_navigation: navigation,
        mail_view: view.clone(),
    });
    (gpui_kit::base::Root::new(workspace, window, cx), view)
}

pub(super) fn mail_profile_with_capabilities(
    account: AccountId,
    mail: MailCapabilities,
) -> AccountProfile {
    AccountProfile::try_new_with_capabilities(
        account,
        Some("Ada Example".to_owned()),
        Some("ada@example.test".to_owned()),
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::new(mail, CalendarAccess::ReadOnly),
    )
    .expect("synthetic account profile has an identity")
}

pub(super) async fn wait_for_mail_sync_to_finish(
    context: &mut VisualTestContext,
    view: &Entity<MailView>,
) {
    wait_for_ui_condition(context, |_, app| {
        !view.read(app).sync_status.is_sync_active()
    })
    .await;
}

pub(super) fn draw_mail_view(context: &mut VisualTestContext) {
    context.update(|window, app| {
        let _ = window.draw(app);
    });
}

pub(super) fn message_list_width(context: &mut VisualTestContext) -> gpui_kit::Pixels {
    context.update(|window, _| window.find("mail-message-list").bounds().size.width)
}

pub(super) fn open_cached_html_message(
    cx: &mut TestAppContext,
    content: String,
) -> &mut VisualTestContext {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let (profile, cache, _) = fixture(account);
    cache.0.lock().expect("fixture cache mutex is healthy").body =
        Some(MessageBody::new(MessageBodyType::Html, content));
    let workflow = Arc::new(MailWorkflow::new(
        profile.id().clone(),
        OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        },
        cache,
    ));
    let (_, context) = cx.add_window_view(|_, cx| test_mail_view(profile, workflow, cx));
    open_first_cached_message(context);
    context
}

pub(super) fn open_first_cached_message(context: &mut VisualTestContext) {
    context.run_until_parked();
    draw_mail_view(context);
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
}

pub(super) fn accessible_alert_count(
    context: &mut VisualTestContext,
    expected_text: &str,
) -> usize {
    context.update(|window, _| {
        window
            .find_all_by_role(gpui_kit::Role::Alert)
            .iter()
            .filter(|alert| alert.label() == Some(expected_text))
            .count()
    })
}

pub(super) fn open_cached_message_and_click_read_state(context: &mut VisualTestContext) {
    context.run_until_parked();
    draw_mail_view(context);
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let action = context
        .debug_bounds("message-read-state")
        .expect("opened message exposes its read-state action");
    context.simulate_click(action.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
}

pub(super) fn open_cached_message_and_click_follow_up_flag(context: &mut VisualTestContext) {
    context.run_until_parked();
    draw_mail_view(context);
    let message_row = context
        .debug_bounds("message-row-message-1")
        .expect("cached message row is rendered");
    context.simulate_click(message_row.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
    let action = context
        .debug_bounds("message-follow-up-flag")
        .expect("opened message exposes its follow-up flag action");
    context.simulate_click(action.center(), Modifiers::default());
    context.run_until_parked();
    draw_mail_view(context);
}

pub(super) fn click_composer_sheet_close(context: &mut VisualTestContext) {
    let composer = context
        .debug_bounds("mail-composer")
        .expect("composer exposes its test bounds");
    context.simulate_click(
        point(composer.right() - px(14.), composer.top() - px(16.)),
        Modifiers::default(),
    );
}

pub(super) fn append_synthetic_messages(
    cache: &FixtureCache,
    message_count: usize,
) -> Vec<MessageId> {
    let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
    let template = state
        .messages
        .first()
        .cloned()
        .expect("fixture cache has a synthetic message");
    let mut message_ids = vec![template.id().clone()];
    for index in 2..=message_count {
        let message_id =
            MessageId::try_from(format!("message-{index}")).expect("synthetic message ID is valid");
        let template_metadata = template.metadata();
        let metadata = einfach_mail_domain::MessageMetadata::new(
            Some(format!("Synthetic subject {index}")),
            template_metadata.origin().clone(),
            *template_metadata.received_at(),
            template_metadata.read_state(),
            template_metadata.follow_up_flag_status(),
        );
        state.messages.push(MailMessage::new(
            template.folder().clone(),
            message_id.clone(),
            metadata,
        ));
        message_ids.push(message_id);
    }
    message_ids
}

pub(super) fn press_window_key(context: &mut VisualTestContext, key: &str) {
    context.simulate_keystrokes(key);
}

pub(super) fn activate_focused_control(context: &mut VisualTestContext) {
    context.update(|window, cx| {
        window.key_down("enter", false, cx);
        window.key_up("enter", cx);
    });
}

pub(super) const SYNTHETIC_MESSAGE_LABEL_PREFIX: &str =
    "Ada Example <ada@example.test>. Synthetic subject";

pub(super) fn assert_accessible_message_list(context: &mut VisualTestContext) {
    let (lists, options) = context.update(|window, _| {
        (
            window.find_all_by_role(gpui_kit::Role::ListBox),
            window.find_all_by_role(gpui_kit::Role::ListBoxOption),
        )
    });
    assert_eq!(lists.len(), 1, "mail has one message listbox");
    assert_eq!(lists[0].label(), Some("Messages"));
    assert!(!options.is_empty(), "visible messages are listbox options");
    assert!(
        options.iter().all(|option| option
            .label()
            .is_some_and(|label| label.starts_with(SYNTHETIC_MESSAGE_LABEL_PREFIX))),
        "every visible message option exposes its sender and subject"
    );
    assert!(
        options
            .iter()
            .all(|option| option.selected() == Some(false))
    );
}

pub(super) fn accessible_message_list_focus(context: &mut VisualTestContext) -> Option<bool> {
    let lists = context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBox));
    assert_eq!(lists.len(), 1, "mail has one message listbox");
    lists[0].focused()
}

pub(super) fn assert_selected_message_and_accessible_label(
    context: &mut VisualTestContext,
    view: &gpui_kit::Entity<MailView>,
    expected_message_id: Option<&MessageId>,
    expected_label: &str,
) {
    let selected = context
        .cx
        .update(|cx| view.read(cx).selected_message_id().cloned());
    assert_eq!(selected.as_ref(), expected_message_id);

    let options =
        context.update(|window, _| window.find_all_by_role(gpui_kit::Role::ListBoxOption));
    let selected_option_labels: Vec<_> = options
        .iter()
        .filter(|option| option.selected() == Some(true))
        .map(|option| option.label())
        .collect();
    assert_eq!(
        selected_option_labels,
        [Some(expected_label)],
        "exactly one message option is exposed as selected"
    );
}
