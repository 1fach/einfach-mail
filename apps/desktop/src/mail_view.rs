use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime},
};

use einfach_mail_application::{
    MailCache, MailSyncState, MailWorkflow, MailWorkflowError, MessageBodyForDisplay,
    OpenedMessage, SanitizedEmailHtml, allowed_external_message_url,
};
use einfach_mail_domain::{AccountProfile, FolderReference, MailFolder, MailMessage, MessageId};
use gpui_kit::base::v_virtual_list;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, AsyncApp, Context, EventEmitter, ImageSource, IntoElement, Render, Role,
    SharedString, Task, Window, base::TextSelectionLayer, component::text::TextView, div,
    prelude::*, px, size,
};

gpui_kit::actions!(
    mail_message_list,
    [
        SelectPreviousMessage,
        SelectNextMessage,
        SelectFirstMessage,
        SelectLastMessage,
    ]
);

const MESSAGE_LIST_CONTEXT: &str = "MailMessageList";

pub(crate) fn bind_message_navigation_keys(cx: &mut App) {
    let context = Some(MESSAGE_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousMessage, context),
        KeyBinding::new("down", SelectNextMessage, context),
        KeyBinding::new("home", SelectFirstMessage, context),
        KeyBinding::new("end", SelectLastMessage, context),
    ]);
}

pub(crate) enum MailViewEvent {
    BackToAccount,
}

pub(crate) struct MailView<P, C>
where
    P: einfach_mail_application::MailProvider + 'static,
    C: MailCache + 'static,
{
    profile: AccountProfile,
    workflow: Arc<MailWorkflow<P, C>>,
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    message_row_sizes: Rc<Vec<gpui_kit::Size<gpui_kit::Pixels>>>,
    message_list_focus: FocusHandle,
    message_list_scroll: VirtualListScrollHandle,
    sync_state: Option<MailSyncState>,
    sync_status: SyncStatus,
    sync_running: bool,
    sync_task: Option<Task<()>>,
    selected_message_id: Option<MessageId>,
    message_state: MessageState,
    message_task: Option<Task<()>>,
}

enum SyncStatus {
    LoadingCache,
    Syncing,
    UpToDate,
    Failed(MailWorkflowError),
}

enum MessageState {
    Empty,
    Loading,
    Opened(Box<OpenedMessage>),
    Failed(MailWorkflowError),
}

struct MailSnapshot {
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    sync_state: Option<MailSyncState>,
}

impl<P, C> MailView<P, C>
where
    P: einfach_mail_application::MailProvider + 'static,
    C: MailCache + 'static,
{
    pub(crate) fn new(
        profile: AccountProfile,
        workflow: Arc<MailWorkflow<P, C>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            profile,
            workflow,
            folders: Vec::new(),
            selected_folder: None,
            messages: Vec::new(),
            message_row_sizes: Rc::new(Vec::new()),
            message_list_focus: cx.focus_handle().tab_stop(true),
            message_list_scroll: VirtualListScrollHandle::new(),
            sync_state: None,
            sync_status: SyncStatus::LoadingCache,
            sync_running: true,
            sync_task: None,
            selected_message_id: None,
            message_state: MessageState::Empty,
            message_task: None,
        };
        view.load_cache_and_sync(cx);
        view
    }

    fn load_cache_and_sync(&mut self, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.profile.id().clone();
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let initial_snapshot = load_snapshot(&workflow, &account_id, None).await;
            let has_snapshot = initial_snapshot.is_ok();
            let _ = this.update(cx, |view, cx| match initial_snapshot {
                Ok(snapshot) => {
                    view.apply_snapshot(snapshot);
                    view.sync_status = SyncStatus::Syncing;
                    cx.notify();
                }
                Err(error) => {
                    view.sync_running = false;
                    view.sync_status = SyncStatus::Failed(error);
                    cx.notify();
                }
            });
            if !has_snapshot {
                return;
            }

            let sync_result = workflow
                .synchronize_account(&account_id, SystemTime::now())
                .await;
            let Ok(selected_folder) = this.read_with(cx, |view, _| view.selected_folder.clone())
            else {
                return;
            };
            let snapshot = load_snapshot(&workflow, &account_id, selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_running = false;
                view.sync_task = None;
                view.finish_sync(sync_result.map(|_| ()), snapshot);
                cx.notify();
            });
        }));
    }

    fn apply_snapshot(&mut self, snapshot: MailSnapshot) {
        self.folders = snapshot.folders;
        self.selected_folder = snapshot.selected_folder;
        self.set_messages(snapshot.messages);
        self.sync_state = snapshot.sync_state;
        self.selected_message_id = None;
        self.message_state = MessageState::Empty;
        self.message_task = None;
    }

    fn set_messages(&mut self, messages: Vec<MailMessage>) {
        self.message_list_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.message_row_sizes = Rc::new(vec![size(px(1.), px(72.)); messages.len()]);
        self.messages = messages;
    }

    fn finish_sync(
        &mut self,
        sync_result: Result<(), MailWorkflowError>,
        snapshot: Result<MailSnapshot, MailWorkflowError>,
    ) {
        let (status, snapshot) = match (sync_result, snapshot) {
            (_, Err(error)) => (SyncStatus::Failed(error), None),
            (Err(error), Ok(snapshot)) => (SyncStatus::Failed(error), Some(snapshot)),
            (Ok(_), Ok(snapshot)) => (SyncStatus::UpToDate, Some(snapshot)),
        };
        if let Some(snapshot) = snapshot {
            self.apply_snapshot(snapshot);
        }
        self.sync_status = status;
    }

    pub(crate) fn stop_pending_work(&mut self) -> Arc<MailWorkflow<P, C>> {
        drop(self.sync_task.take());
        drop(self.message_task.take());
        self.sync_running = false;
        self.selected_folder = None;
        self.selected_message_id = None;
        self.sync_state = None;
        self.folders.clear();
        self.messages.clear();
        self.message_state = MessageState::Empty;
        Arc::clone(&self.workflow)
    }

    fn select_folder(&mut self, folder: FolderReference, cx: &mut Context<Self>) {
        if self.selected_folder.as_ref() == Some(&folder) {
            return;
        }
        self.selected_folder = Some(folder.clone());
        self.sync_state = None;
        self.selected_message_id = None;
        self.message_state = MessageState::Loading;
        self.set_messages(Vec::new());
        let workflow = Arc::clone(&self.workflow);
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let messages = workflow.list_cached_messages(&folder).await;
            let sync_state = workflow.cached_sync_state(&folder).await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_folder.as_ref() != Some(&folder) {
                    return;
                }
                match (messages, sync_state) {
                    (Ok(messages), Ok(sync_state)) => {
                        view.set_messages(messages);
                        view.sync_state = sync_state;
                        view.message_state = MessageState::Empty;
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        view.message_state = MessageState::Failed(error);
                    }
                }
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if !self
            .messages
            .iter()
            .any(|message| message.id() == &message_id)
        {
            return;
        }
        let Some(folder) = self.selected_folder.clone() else {
            return;
        };
        self.selected_message_id = Some(message_id.clone());
        self.message_state = MessageState::Loading;
        let workflow = Arc::clone(&self.workflow);
        let request_message_id = message_id.clone();
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            // Keep HTML sanitization off GPUI's event-loop executor.
            let opened = cx
                .background_executor()
                .spawn(async move { workflow.open_message(&folder, &request_message_id).await })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_message_id.as_ref() != Some(&message_id) {
                    return;
                }
                view.message_state = match opened {
                    Ok(message) => MessageState::Opened(Box::new(message)),
                    Err(error) => MessageState::Failed(error),
                };
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn navigate_message(&mut self, navigation: MessageNavigation, cx: &mut Context<Self>) {
        if self.sync_running || self.message_update_in_progress() {
            return;
        }
        let selected_index = self.selected_message_id.as_ref().and_then(|selected| {
            self.messages
                .iter()
                .position(|message| message.id() == selected)
        });
        let Some(target_index) = navigation.target_index(selected_index, self.messages.len())
        else {
            return;
        };
        let Some(message) = self.messages.get(target_index) else {
            return;
        };
        if self.selected_message_id.as_ref() == Some(message.id()) {
            return;
        }
        self.message_list_scroll
            .scroll_to_item(target_index, ScrollStrategy::Nearest);
        self.select_message(message.id().clone(), cx);
    }

    fn select_message_from_option(
        &mut self,
        message_id: MessageId,
        expected_folder: &FolderReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_folder.as_ref() != Some(expected_folder) {
            return;
        }
        self.message_list_focus.focus(window, cx);
        self.select_message(message_id, cx);
    }

    fn on_select_previous_message(
        &mut self,
        _: &SelectPreviousMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::Previous, cx);
    }

    fn on_select_next_message(
        &mut self,
        _: &SelectNextMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::Next, cx);
    }

    fn on_select_first_message(
        &mut self,
        _: &SelectFirstMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::First, cx);
    }

    fn on_select_last_message(
        &mut self,
        _: &SelectLastMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(MessageNavigation::Last, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.sync_running {
            return;
        }
        self.sync_running = true;
        self.sync_status = SyncStatus::Syncing;
        self.selected_message_id = None;
        self.message_state = MessageState::Empty;
        let workflow = Arc::clone(&self.workflow);
        let account_id = self.profile.id().clone();
        let selected_folder = self.selected_folder.clone();
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let sync_result = workflow
                .synchronize_account(&account_id, SystemTime::now())
                .await;
            let snapshot = load_snapshot(&workflow, &account_id, selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_running = false;
                view.sync_task = None;
                view.finish_sync(sync_result.map(|_| ()), snapshot);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.sync_status_label();
        let status_role = if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Role::Alert
        } else {
            Role::Status
        };

        div()
            .w_full()
            .h(px(58.))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(div().text_lg().font_bold().child("Mail"))
                    .child(
                        div()
                            .id("mail-sync-status")
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .role(status_role)
                            .aria_label(status.clone())
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("sync-mail")
                            .ghost()
                            .label("Sync")
                            .loading(self.sync_running)
                            .disabled(self.sync_running)
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    )
                    .child(
                        Button::new("account")
                            .ghost()
                            .label("Account")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(MailViewEvent::BackToAccount);
                            })),
                    ),
            )
    }

    fn sync_status_label(&self) -> String {
        match &self.sync_status {
            SyncStatus::LoadingCache => "Opening encrypted cache…".to_owned(),
            SyncStatus::Syncing => self.sync_age_label("Syncing"),
            SyncStatus::UpToDate => self.sync_age_label("Up to date"),
            SyncStatus::Failed(error) => {
                let cached_age = self.sync_state.as_ref().and_then(|state| {
                    state
                        .last_synchronized_at()
                        .map(|last_sync| format!(" Cached mail from {}.", elapsed_label(last_sync)))
                });
                format!(
                    "{}{}",
                    mail_error_message(*error),
                    cached_age.unwrap_or_default()
                )
            }
        }
    }

    fn sync_age_label(&self, prefix: &str) -> String {
        self.sync_state
            .as_ref()
            .and_then(MailSyncState::last_synchronized_at)
            .map_or_else(
                || prefix.to_owned(),
                |time| format!("{prefix} · {}", elapsed_label(time)),
            )
    }

    fn render_folder_sidebar(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut folders = div().flex().flex_col().gap_1().overflow_y_scrollbar();
        for folder in &self.folders {
            let reference = folder.reference().clone();
            folders = folders.child(
                Button::new(format!("folder:{}", reference.folder_id().as_str()))
                    .ghost()
                    .selected(self.selected_folder.as_ref() == Some(&reference))
                    .label(folder.display_name().to_owned())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_folder(reference.clone(), cx);
                    })),
            );
        }
        div()
            .w(px(216.))
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(colors.surface)
            .child(div().text_sm().font_bold().child("Folders"))
            .child(folders.flex_1().min_h_0())
    }

    fn render_message_list(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        if let MessageState::Failed(error) = &self.message_state {
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .id("mail-list-error")
                .role(Role::Alert)
                .child(mail_error_message(*error))
                .into_any_element();
        }
        let Some(folder) = self.selected_folder.as_ref() else {
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No visible mail folders were found.")
                .into_any_element();
        };
        if self.messages.is_empty() {
            let message = if matches!(self.message_state, MessageState::Loading) {
                "Loading messages…"
            } else {
                "No messages in this folder."
            };
            return div()
                .w(px(344.))
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child(message)
                .into_any_element();
        }

        let folder_reference = folder.clone();
        let sizes = Rc::clone(&self.message_row_sizes);
        div()
            .w(px(344.))
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .child(div().px_4().py_3().text_sm().font_bold().child("Messages"))
            .child(v_virtual_list(
                cx.entity(),
                "cached-message-list",
                sizes,
                move |view, range, _, cx| {
                    range
                        .filter_map(|index| {
                            let message = view.messages.get(index)?;
                            let message_id = message.id().clone();
                            let selected = view.selected_message_id.as_ref() == Some(&message_id);
                            let sender = message_sender(message);
                            let subject = message
                                .metadata()
                                .subject()
                                .unwrap_or("(No subject)")
                                .to_owned();
                            let accessible_label = format!("{sender}. {subject}");
                            let expected_folder = folder_reference.clone();
                            Some(
                                Button::new(format!("message:{}", message_id.as_str()))
                                    .ghost()
                                    .selected(selected)
                                    .accessibility_label(accessible_label)
                                    .debug_selector(|| {
                                        format!("message-row-{}", message_id.as_str())
                                    })
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div().w_full().text_sm().font_bold().child(sender),
                                            )
                                            .child(div().w_full().text_sm().child(subject)),
                                    )
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if view.selected_folder.as_ref() == Some(&expected_folder) {
                                            view.select_message(message_id.clone(), cx);
                                        }
                                    })),
                            )
                        })
                        .collect()
                },
            ))
            .into_any_element()
    }

    fn render_message_detail(&self, colors: ColorTokens) -> impl IntoElement {
        match &self.message_state {
            MessageState::Empty => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child("Select a message to read it.")
                .into_any_element(),
            MessageState::Loading => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child("Loading message…")
                .into_any_element(),
            MessageState::Failed(error) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .p_6()
                .id("mail-message-error")
                .role(Role::Alert)
                .child(mail_error_message(*error))
                .into_any_element(),
            MessageState::Opened(opened) => {
                let metadata = opened.message().metadata();
                let body = render_message_body(opened.body(), colors);
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_6()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .text_lg()
                                    .font_bold()
                                    .child(metadata.subject().unwrap_or("(No subject)").to_owned()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.muted_foreground)
                                    .child(message_sender(opened.message())),
                            ),
                    )
                    .child(body)
                    .into_any_element()
            }
        }
    }
}

fn render_message_body(body: &MessageBodyForDisplay, colors: ColorTokens) -> AnyElement {
    match body {
        MessageBodyForDisplay::PlainText(text) => render_plain_text_body(text),
        MessageBodyForDisplay::SanitizedHtml(html) => render_sanitized_html_body(html),
        MessageBodyForDisplay::PlainTextFallback(content) => {
            render_plain_text_fallback(content, colors)
        }
        MessageBodyForDisplay::HtmlTooLarge => render_oversized_html_fallback(colors),
    }
}

fn render_plain_text_body(text: &Arc<str>) -> AnyElement {
    div()
        .id("mail-message-plain-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(SharedString::from(Arc::clone(text)))
        .into_any_element()
}

fn render_plain_text_fallback(content: &SanitizedEmailHtml, colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .p_6()
        .child(render_plain_text_fallback_status(colors))
        .child(render_plain_text_fallback_content(content))
        .into_any_element()
}

fn render_plain_text_fallback_status(colors: ColorTokens) -> AnyElement {
    div()
        .id("mail-message-html-fallback-status")
        .debug_selector(|| "mail-message-html-fallback-status".into())
        .text_color(colors.muted_foreground)
        .role(Role::Status)
        .child(
            "Rich formatting was removed. Only the beginning of this message is shown as plain text.",
        )
        .into_any_element()
}

fn render_plain_text_fallback_content(content: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-fallback-body")
        .debug_selector(|| "mail-message-html-fallback-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .child(TextSelectionLayer)
        .child(
            TextView::html(
                "mail-message-html-fallback-content",
                content.shared_content(),
            )
            .selectable(true)
            .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
            .on_link_click(|_, _, _, _| {}),
        )
        .into_any_element()
}

fn render_sanitized_html_body(html: &SanitizedEmailHtml) -> AnyElement {
    div()
        .id("mail-message-html-body")
        .debug_selector(|| "mail-message-html-body".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .p_6()
        .child(TextSelectionLayer)
        .child(
            TextView::html("mail-message-html-content", html.shared_content())
                .selectable(true)
                .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
                .on_link_click(|href, event, _, cx| {
                    if event.is_secondary() {
                        return;
                    }
                    if let Some(url) = allowed_external_message_url(href.as_ref()) {
                        cx.open_url(url.as_str());
                    }
                }),
        )
        .into_any_element()
}

fn render_oversized_html_fallback(colors: ColorTokens) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p_6()
        .text_color(colors.muted_foreground)
        .id("mail-message-body-too-large")
        .debug_selector(|| "mail-message-body-too-large".into())
        .role(Role::Status)
        .child("This message is too large to display safely.")
        .into_any_element()
}

impl<P, C> EventEmitter<MailViewEvent> for MailView<P, C>
where
    P: einfach_mail_application::MailProvider + 'static,
    C: MailCache + 'static,
{
}

impl<P, C> Render for MailView<P, C>
where
    P: einfach_mail_application::MailProvider + 'static,
    C: MailCache + 'static,
{
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_folder_sidebar(colors, cx))
                    .child(self.render_message_list(colors, cx))
                    .child(self.render_message_detail(colors)),
            )
    }
}

async fn load_snapshot<P, C>(
    workflow: &MailWorkflow<P, C>,
    account_id: &einfach_mail_domain::AccountId,
    preferred: Option<&FolderReference>,
) -> Result<MailSnapshot, MailWorkflowError>
where
    P: einfach_mail_application::MailProvider,
    C: MailCache,
{
    let folders = workflow.list_cached_folders(account_id).await?;
    let selected_folder = preferred
        .filter(|preferred| {
            folders
                .iter()
                .any(|folder| folder.reference() == *preferred)
        })
        .cloned()
        .or_else(|| preferred_folder(&folders));
    let (messages, sync_state) = if let Some(folder) = &selected_folder {
        (
            workflow.list_cached_messages(folder).await?,
            workflow.cached_sync_state(folder).await?,
        )
    } else {
        (Vec::new(), None)
    };
    Ok(MailSnapshot {
        folders,
        selected_folder,
        messages,
        sync_state,
    })
}

fn preferred_folder(folders: &[MailFolder]) -> Option<FolderReference> {
    folders
        .iter()
        .find(|folder| folder.display_name().eq_ignore_ascii_case("inbox"))
        .or_else(|| folders.first())
        .map(|folder| folder.reference().clone())
}

fn message_sender(message: &MailMessage) -> String {
    message
        .metadata()
        .origin()
        .from_mailbox()
        .or_else(|| message.metadata().origin().sender())
        .map_or_else(
            || "Unknown sender".to_owned(),
            |mailbox| match mailbox.display_name().unwrap_or_default().trim() {
                "" => mailbox.address().to_owned(),
                name => format!("{name} <{}>", mailbox.address()),
            },
        )
}

fn message_option_element(
    presentation: &MessageOptionPresentation,
    colors: MessageRowColors,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let message_id = &presentation.message_id;
    let selected = presentation.selected;
    div()
        .id(format!("message-option-{}", message_id.as_str()))
        .debug_selector(|| format!("message-row-{}", message_id.as_str()))
        .w_full()
        .h(px(72.))
        .px_3()
        .py_3()
        .rounded_md()
        .text_color(colors.foreground)
        .role(Role::ListBoxOption)
        .aria_label(presentation.accessible_label.clone())
        .aria_position_in_set(presentation.position)
        .aria_size_of_set(presentation.list_size)
        .aria_selected(selected)
        .when(selected, |option| {
            option
                .aria_active_descendant()
                .bg(colors.selected_background)
        })
        .hover(|option| {
            option.bg(if selected {
                colors.selected_background
            } else {
                colors.hover_background
            })
        })
        .when(presentation.disabled, |option| option.opacity(0.6))
        .child(message_option_content(
            presentation.sender.clone(),
            presentation.subject.clone(),
        ))
}

fn message_option_content(sender: SharedString, subject: SharedString) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().w_full().text_sm().font_bold().child(sender))
        .child(div().w_full().text_sm().child(subject))
}

fn elapsed_label(last_sync: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(last_sync)
        .unwrap_or(Duration::ZERO);
    match elapsed.as_secs() {
        seconds if seconds < 60 => "less than a minute ago".to_owned(),
        seconds if seconds < 3_600 => format!("{} min ago", seconds / 60),
        seconds if seconds < 86_400 => format!("{} hr ago", seconds / 3_600),
        seconds => format!("{} days ago", seconds / 86_400),
    }
}

fn mail_error_message(error: MailWorkflowError) -> &'static str {
    use einfach_mail_application::{MailCacheError, MailOperationError};

    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => {
            "Microsoft denied mail access. Review account permissions and try again."
        }
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        MailWorkflowError::Cache(MailCacheError::MissingEncryptionKey) => {
            "The encrypted mail cache key is missing. Remove the local cache and sync again."
        }
        MailWorkflowError::Cache(MailCacheError::Unavailable) => {
            "The encrypted mail cache is unavailable. Unlock your Linux keyring and try again."
        }
        MailWorkflowError::Cache(MailCacheError::MessageNotCached) => {
            "This message is no longer in the local cache. Sync the folder and try again."
        }
        MailWorkflowError::RepeatedCursor => {
            "Sync stopped because the service repeated a page. Try syncing again."
        }
        MailWorkflowError::PageLimitExceeded => {
            "Sync paused after many pages. Choose Sync to continue from its saved checkpoint."
        }
        MailWorkflowError::ResponseLimitExceeded => {
            "Sync paused after receiving a large amount of mail data. Choose Sync to continue from its saved checkpoint."
        }
        _ => "Couldn't complete this mail request. Check your connection and try again.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{Future, ready},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use einfach_mail_application::{
        MailCache, MailCacheError, MailOperationError, MailProvider, MailSyncState, MailWorkflow,
        MailWorkflowError, MessageDeltaRequest,
    };
    use einfach_mail_domain::{
        AccountId, AccountProfile, FolderId, FolderReference, MailFolder, MailMessage, Mailbox,
        MessageBody, MessageBodyType, MessageDeltaLink, MessageDeltaPage, MessageId,
    };
    use gpui_kit::base::TextSelection;
    use gpui_kit::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};

    use super::{MailView, MessageBodyForDisplay, MessageState, SyncStatus};

    #[derive(Clone)]
    struct OfflineProvider {
        body_fetches: Arc<AtomicUsize>,
    }

    impl MailProvider for OfflineProvider {
        fn list_folders(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn list_messages(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_
        {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn message_delta_page(
            &self,
            _: &FolderReference,
            _: MessageDeltaRequest,
        ) -> impl Future<
            Output = Result<einfach_mail_application::MessageDeltaPageResponse, MailOperationError>,
        > + Send
        + '_ {
            ready(Err(MailOperationError::ServiceUnavailable))
        }

        fn fetch_message_body(
            &self,
            _: &FolderReference,
            _: &MessageId,
        ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
            self.body_fetches.fetch_add(1, Ordering::Relaxed);
            ready(Err(MailOperationError::ServiceUnavailable))
        }
    }

    #[derive(Clone, Default)]
    struct FixtureCache(Arc<Mutex<FixtureData>>);

    #[derive(Default)]
    struct FixtureData {
        account_id: Option<AccountId>,
        folders: Vec<MailFolder>,
        messages: Vec<MailMessage>,
        body: Option<MessageBody>,
        sync_state: Option<MailSyncState>,
    }

    impl MailCache for FixtureCache {
        fn replace_folders(
            &self,
            account_id: &AccountId,
            folders: Vec<MailFolder>,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            let account_id = account_id.clone();
            async move {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                data.account_id = Some(account_id);
                data.folders = folders;
                Ok(())
            }
        }

        fn ensure_account_sync_scope(
            &self,
            _: &AccountId,
            _: SystemTime,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            ready(Ok(()))
        }

        fn ensure_sync_state(
            &self,
            _: &FolderReference,
            initial_cutoff: SystemTime,
        ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            ready(Ok(data
                .sync_state
                .get_or_insert_with(|| MailSyncState::initial(initial_cutoff))
                .clone()))
        }

        fn load_sync_state(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_
        {
            ready(Ok(self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .sync_state
                .clone()))
        }

        fn commit_delta_page(
            &self,
            _: &FolderReference,
            _: MessageDeltaPage,
            _: SystemTime,
        ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
            ready(Err(MailCacheError::InvalidData))
        }

        fn list_folders(
            &self,
            account_id: &AccountId,
        ) -> impl Future<Output = Result<Vec<MailFolder>, MailCacheError>> + Send + '_ {
            let account_id = account_id.clone();
            async move {
                let data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                Ok(data.folders.clone())
            }
        }

        fn list_messages(
            &self,
            _: &FolderReference,
        ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_ {
            ready(Ok(self
                .0
                .lock()
                .expect("fixture cache mutex is healthy")
                .messages
                .clone()))
        }

        fn read_message(
            &self,
            _: &FolderReference,
            message_id: &MessageId,
        ) -> impl Future<
            Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>,
        > + Send
        + '_ {
            let message_id = message_id.clone();
            async move {
                let data = self.0.lock().expect("fixture cache mutex is healthy");
                Ok(data
                    .messages
                    .iter()
                    .find(|message| message.id() == &message_id)
                    .cloned()
                    .map(|message| (message, data.body.clone())))
            }
        }

        fn store_message_body(
            &self,
            _: &FolderReference,
            _: &MessageId,
            body: MessageBody,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            self.0.lock().expect("fixture cache mutex is healthy").body = Some(body);
            ready(Ok(()))
        }

        fn remove_account(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    fn fixture(profile_account: AccountId) -> (AccountProfile, FixtureCache, MessageId) {
        let folder_reference = FolderReference::new(
            profile_account.clone(),
            FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
        );
        let folder = MailFolder::new(folder_reference.clone(), None, "Inbox".to_owned())
            .expect("synthetic folder is valid");
        let message_id =
            MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is valid");
        let sender = Mailbox::try_new(
            "ada@example.test".to_owned(),
            Some("Ada Example".to_owned()),
        )
        .expect("synthetic mailbox is valid");
        let message = MailMessage::new(
            folder_reference,
            message_id.clone(),
            einfach_mail_domain::MessageMetadata::new(
                Some("Synthetic subject".to_owned()),
                einfach_mail_domain::MessageOrigin::new(Some(sender), None),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                einfach_mail_domain::ReadState::Unread,
            ),
        );
        let cache = FixtureCache::default();
        {
            let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
            data.account_id = Some(profile_account.clone());
            data.folders.push(folder);
            data.messages.push(message);
            data.body = Some(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic offline body".to_owned(),
            ));
            data.sync_state = Some(
                MailSyncState::from_checkpoint(
                    UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                    Some(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/delta?token=synthetic".to_owned(),
                    )),
                    None,
                    Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                )
                .expect("synthetic sync checkpoint is valid"),
            );
        }
        let profile = AccountProfile::try_new(
            profile_account,
            Some("Ada Example".to_owned()),
            Some("ada@example.test".to_owned()),
            Some("ada@tenant.example.test".to_owned()),
        )
        .expect("synthetic account profile is valid");
        (profile, cache, message_id)
    }

    fn draw_mail_view(context: &mut VisualTestContext) {
        context.update(|window, app| {
            let _ = window.draw(app);
        });
    }

    fn append_synthetic_messages(cache: &FixtureCache, message_count: usize) -> Vec<MessageId> {
        let mut state = cache.0.lock().expect("fixture cache mutex is healthy");
        let template = state
            .messages
            .first()
            .cloned()
            .expect("fixture cache has a synthetic message");
        let mut message_ids = vec![template.id().clone()];
        for index in 2..=message_count {
            let message_id = MessageId::try_from(format!("message-{index}"))
                .expect("synthetic message ID is valid");
            state.messages.push(MailMessage::new(
                template.folder().clone(),
                message_id.clone(),
                template.metadata().clone(),
            ));
            message_ids.push(message_id);
        }
        message_ids
    }

    fn press_window_key(context: &mut VisualTestContext, key: &str) {
        context.simulate_keystrokes(key);
    }

    fn assert_selected_message(
        context: &mut VisualTestContext,
        view: &gpui_kit::Entity<MailView<OfflineProvider, FixtureCache>>,
        expected: Option<&MessageId>,
    ) {
        let selected = context
            .cx
            .update(|cx| view.read(cx).selected_message_id.clone());
        assert_eq!(selected.as_ref(), expected);
    }

    #[gpui_kit::test]
    async fn cached_message_list_navigates_by_keyboard_and_keeps_one_tab_stop(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_message_navigation_keys(cx);
        });
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let message_ids = append_synthetic_messages(&cache, 20);
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let mut mail_view = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MailView::new(profile, workflow, None, cx));
            mail_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mail_view.expect("mail view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_mail_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context.cx.executor().timer(Duration::from_millis(10)).await;
        }

        context.update(gpui_kit::Window::focus_next);
        let mut list_focused = false;
        for _ in 0..20 {
            press_window_key(context, "tab");
            list_focused =
                context.update(|window, cx| view.read(cx).message_list_focus.is_focused(window));
            if list_focused {
                break;
            }
        }
        assert!(list_focused, "Tab reaches the cached message list");

        press_window_key(context, "up");
        context.run_until_parked();
        assert_selected_message(context, &view, message_ids.last());
        press_window_key(context, "down");
        assert_selected_message(context, &view, message_ids.last());

        press_window_key(context, "home");
        assert_selected_message(context, &view, message_ids.first());
        press_window_key(context, "up");
        assert_selected_message(context, &view, message_ids.first());
        press_window_key(context, "down");
        assert_selected_message(context, &view, message_ids.get(1));
        press_window_key(context, "end");
        context.run_until_parked();
        draw_mail_view(context);
        assert_selected_message(context, &view, message_ids.last());
        assert!(
            context
                .cx
                .update(|cx| matches!(&view.read(cx).message_state, MessageState::Opened(_)))
        );
        let row = context
            .debug_bounds("message-row-message-20")
            .expect("End scrolls the selected message into view");
        let scroll_offset = context
            .cx
            .update(|cx| view.read(cx).message_list_scroll.offset());
        assert!(scroll_offset.y < px(0.));
        context.simulate_click(row.center(), Modifiers::default());
        context.run_until_parked();
        assert!(
            context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) })
        );

        press_window_key(context, "shift-tab");
        context.update(|window, cx| {
            assert!(!view.read(cx).message_list_focus.is_focused(window));
        });
        press_window_key(context, "up");
        assert_selected_message(context, &view, message_ids.last());
        press_window_key(context, "tab");
        assert!(
            context.update(|window, cx| { view.read(cx).message_list_focus.is_focused(window) })
        );
        press_window_key(context, "tab");
        context.update(|window, cx| {
            assert!(!view.read(cx).message_list_focus.is_focused(window));
        });
        press_window_key(context, "up");
        assert_selected_message(context, &view, message_ids.last());
    }

    #[gpui_kit::test]
    fn offline_mail_view_opens_cached_message_without_provider_body_request(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, message_id) = fixture(account);
        let provider = OfflineProvider {
            body_fetches: Arc::new(AtomicUsize::new(0)),
        };
        let body_fetches = Arc::clone(&provider.body_fetches);
        let workflow = Arc::new(MailWorkflow::new(provider, cache));
        let (view, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let row_bounds = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(row_bounds.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let opened_offline = context.cx.update(|cx| {
            matches!(
                &view.read(cx).message_state,
                MessageState::Opened(opened)
                    if opened.message().id() == &message_id
                        && opened.loaded_from_cache()
                        && matches!(
                            opened.body(),
                            MessageBodyForDisplay::PlainText(text)
                                if text.as_ref() == "Synthetic offline body"
                        )
            )
        });
        assert!(opened_offline);
        assert_eq!(body_fetches.load(Ordering::Relaxed), 0);
    }

    #[gpui_kit::test]
    fn cached_html_mail_view_sanitizes_content_and_opens_links_on_click(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            concat!(
                "<p><a href=\"https://mail.example.test/thread\">Open safe link</a></p>",
                "<p>Readable synthetic message content.</p>",
                "<img src=\"https://tracker.example.test/pixel\">",
                "<script>hidden script marker</script>"
            )
            .to_owned(),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let safe_html = context.cx.update(|cx| {
            let state = &view.read(cx).message_state;
            let MessageState::Opened(opened) = state else {
                return false;
            };
            let MessageBodyForDisplay::SanitizedHtml(html) = opened.body() else {
                return false;
            };
            html.as_str().contains("Open safe link")
                && !html.as_str().contains("tracker.example.test")
                && !html.as_str().contains("hidden script marker")
        });
        assert!(safe_html);

        let body_bounds = context
            .debug_bounds("mail-message-html-body")
            .expect("sanitized HTML body has a layout");
        let link_position = point(body_bounds.left() + px(36.), body_bounds.top() + px(36.));
        context.simulate_mouse_down(link_position, MouseButton::Right, Modifiers::default());
        context.simulate_mouse_up(link_position, MouseButton::Right, Modifiers::default());
        assert_eq!(context.opened_url(), None);
        context.simulate_click(link_position, Modifiers::default());
        assert_eq!(
            context.opened_url(),
            Some("https://mail.example.test/thread".to_owned())
        );
    }

    #[gpui_kit::test]
    fn cached_html_mail_view_supports_selection_and_copy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            "<p>Readable synthetic content for selection and copy.</p>".to_owned(),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (_, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let body_bounds = context
            .debug_bounds("mail-message-html-body")
            .expect("sanitized HTML body has a layout");
        let start = point(body_bounds.left() + px(25.), body_bounds.top() + px(34.));
        let end = point(body_bounds.left() + px(285.), body_bounds.top() + px(34.));
        context.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        draw_mail_view(context);
        context.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        draw_mail_view(context);
        context.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        draw_mail_view(context);

        let selected_text = context.update(TextSelection::selected_text);
        assert!(selected_text.contains("Readable synthetic content"));
        context.simulate_keystrokes("ctrl-c");
        let copied_text = context.update(|_, app| {
            app.read_from_clipboard()
                .and_then(|clipboard| clipboard.text())
        });
        assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
    }

    #[gpui_kit::test]
    fn oversized_sanitized_html_shows_selectable_noninteractive_text(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let body_content = format!(
            "<p>Readable fallback content. <a href=\"https://mail.example.test/path\">link destination</a> {}</p>",
            ">".repeat(8 * 1024 * 1024 / 4)
        );
        cache.0.lock().expect("fixture cache mutex is healthy").body =
            Some(MessageBody::new(MessageBodyType::Html, body_content));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) =
            cx.add_window_view(|_, cx| MailView::new(profile, workflow, None, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let status = context
            .debug_bounds("mail-message-html-fallback-status")
            .expect("plain-text fallback explains why formatting was removed");
        assert!(status.size.width > px(0.));
        let body_bounds = context
            .debug_bounds("mail-message-html-fallback-body")
            .expect("plain-text fallback has a selectable content region");

        let contains_fallback_text = context.cx.update(|cx| {
            matches!(
                &view.read(cx).message_state,
                MessageState::Opened(opened)
                    if matches!(
                        opened.body(),
                        MessageBodyForDisplay::PlainTextFallback(content)
                            if content.as_str().contains("Readable&#32;fallback&#32;content")
                                && content.as_str().contains("link&#32;destination")
                    )
            )
        });
        assert!(contains_fallback_text);

        context.simulate_click(
            point(body_bounds.left() + px(280.), body_bounds.top() + px(10.)),
            Modifiers::default(),
        );
        assert_eq!(context.opened_url(), None);
        context.simulate_keystrokes("ctrl-a");

        let selected_text = context.update(TextSelection::selected_text);
        assert!(selected_text.contains("Readable fallback content"));
        context.simulate_keystrokes("ctrl-c");
        let copied_text = context.update(|_, app| {
            app.read_from_clipboard()
                .and_then(|clipboard| clipboard.text())
        });
        assert_eq!(copied_text, Some(selected_text.trim_end().to_owned()));
    }

    #[gpui_kit::test]
    fn oversized_cached_html_shows_a_safe_reader_fallback(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        cache.0.lock().expect("fixture cache mutex is healthy").body = Some(MessageBody::new(
            MessageBodyType::Html,
            "x".repeat(8 * 1024 * 1024 + 1),
        ));
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, cx));

        context.run_until_parked();
        draw_mail_view(context);
        let message_row = context
            .debug_bounds("message-row-message-1")
            .expect("cached message row is rendered");
        context.simulate_click(message_row.center(), Modifiers::default());
        context.run_until_parked();
        draw_mail_view(context);

        let is_over_limit = context.cx.update(|cx| {
            matches!(
                view.read(cx).message_state,
                MessageState::Opened(ref opened)
                    if opened.body() == &MessageBodyForDisplay::HtmlTooLarge
            )
        });
        assert!(is_over_limit);
        assert!(
            context
                .debug_bounds("mail-message-body-too-large")
                .is_some()
        );
    }

    #[gpui_kit::test]
    fn cache_snapshot_failure_survives_a_successful_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (profile, cache, _) = fixture(account);
        let workflow = Arc::new(MailWorkflow::new(
            OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
            cache,
        ));
        let (view, context) = cx.add_window_view(|_, cx| MailView::new(profile, workflow, cx));
        context.run_until_parked();

        context.cx.update(|cx| {
            view.update(cx, |view, _| {
                view.finish_sync(
                    Ok(()),
                    Err(MailWorkflowError::Cache(MailCacheError::Unavailable)),
                );
            });
        });
        let preserved_error = context.cx.update(|cx| {
            matches!(
                &view.read(cx).sync_status,
                SyncStatus::Failed(MailWorkflowError::Cache(MailCacheError::Unavailable))
            )
        });

        assert!(preserved_error);
    }
}
