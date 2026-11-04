use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use einfach_mail_application::{CalendarOperationError, CalendarSession, CalendarWorkflowError};
use einfach_mail_domain::{
    AccountProfile, CalendarOccurrence, CalendarOccurrenceTime, CalendarViewRange,
    CalendarViewReference, PendingCalendarEventCreation,
};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, AsyncApp, Context, Entity, EventEmitter, IntoElement, Render, Role, Subscription,
    Task, WeakEntity, Window, div, prelude::*,
};
use jiff::{Span, Timestamp, civil, tz::TimeZone};
use time::{Date, Month, OffsetDateTime};

use crate::event_composer::{
    EventComposer, EventComposerEvent, EventComposerIntent, EventComposerSetup,
};

pub(crate) enum CalendarViewEvent {
    BackToAccount,
    ReviewCalendarWritePermission,
}

pub(crate) struct CalendarView<S: CalendarSession + ?Sized = dyn CalendarSession> {
    profile: AccountProfile,
    session: Arc<S>,
    time_zone: TimeZone,
    utc_fallback: bool,
    week_start: civil::Date,
    view: CalendarViewReference,
    events: Vec<CalendarOccurrence>,
    pending_creation: Option<PendingCalendarEventCreation>,
    sync_state: Option<einfach_mail_application::CalendarSyncState>,
    status: CalendarStatus,
    sync_running: bool,
    request_id: u64,
    task: Option<Task<()>>,
    composer_subscription: Option<Subscription>,
}

#[derive(Clone, Copy)]
enum CalendarStatus {
    LoadingCache,
    Syncing,
    UpToDate,
    Failed(CalendarWorkflowError),
}

struct DisplayEvent<'a> {
    event: &'a CalendarOccurrence,
    sort_key: EventSortKey,
    time_label: String,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct EventSortKey {
    kind: u8,
    seconds: i64,
    nanoseconds: i32,
    original_index: usize,
}

struct CalendarWeekSelection {
    week_start: civil::Date,
    time_zone: TimeZone,
    utc_fallback: bool,
}

struct CalendarRefreshRequest<S: CalendarSession + ?Sized> {
    session: Arc<S>,
    view: CalendarViewReference,
    request_id: u64,
}

struct CalendarRefreshOutcome {
    sync_result: Result<einfach_mail_application::CalendarSyncReport, CalendarWorkflowError>,
    snapshot_result: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
}

impl CalendarWeekSelection {
    fn for_system() -> Self {
        let (mut time_zone, mut utc_fallback) = match TimeZone::try_system() {
            Ok(time_zone) if time_zone.iana_name().is_some() => (time_zone, false),
            Err(_) => (TimeZone::UTC, true),
            Ok(_) => (TimeZone::UTC, true),
        };
        let week_start = match current_week_start(&time_zone) {
            Some(week_start) => week_start,
            None => {
                time_zone = TimeZone::UTC;
                utc_fallback = true;
                fallback_week_start()
            }
        };
        Self {
            week_start,
            time_zone,
            utc_fallback,
        }
    }

    fn resolve(self, account_id: &einfach_mail_domain::AccountId) -> (CalendarViewReference, Self) {
        if let Some((view, week_start)) =
            view_for_week(account_id, self.week_start, &self.time_zone)
        {
            return (view, Self { week_start, ..self });
        }
        Self::utc_fallback(account_id)
    }

    fn utc_fallback(account_id: &einfach_mail_domain::AccountId) -> (CalendarViewReference, Self) {
        let time_zone = TimeZone::UTC;
        let week_start = fallback_week_start();
        let (view, week_start) = view_for_week(account_id, week_start, &time_zone)
            .expect("a fixed UTC week can be represented by the calendar domain");
        (
            view,
            Self {
                week_start,
                time_zone,
                utc_fallback: true,
            },
        )
    }
}

impl<S> CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    pub(crate) fn new(profile: AccountProfile, session: Arc<S>, cx: &mut Context<Self>) -> Self {
        Self::new_for_week(profile, session, CalendarWeekSelection::for_system(), cx)
    }

    fn new_for_week(
        profile: AccountProfile,
        session: Arc<S>,
        selection: CalendarWeekSelection,
        cx: &mut Context<Self>,
    ) -> Self {
        let (view, selection) = selection.resolve(profile.id());
        let mut calendar_view = Self::loading_for_selection(profile, session, view, selection);
        calendar_view.refresh(cx);
        calendar_view
    }

    fn loading_for_selection(
        profile: AccountProfile,
        session: Arc<S>,
        view: CalendarViewReference,
        selection: CalendarWeekSelection,
    ) -> Self {
        Self {
            profile,
            session,
            time_zone: selection.time_zone,
            utc_fallback: selection.utc_fallback,
            week_start: selection.week_start,
            view,
            events: Vec::new(),
            pending_creation: None,
            sync_state: None,
            status: CalendarStatus::LoadingCache,
            sync_running: false,
            request_id: 0,
            task: None,
            composer_subscription: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.sync_running {
            return;
        }
        self.sync_running = true;
        self.status = CalendarStatus::LoadingCache;
        self.request_id = self.request_id.saturating_add(1);
        let request = CalendarRefreshRequest {
            session: Arc::clone(&self.session),
            view: self.view.clone(),
            request_id: self.request_id,
        };
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            Self::run_refresh(request, this, cx).await;
        }));
        cx.notify();
    }

    async fn run_refresh(
        request: CalendarRefreshRequest<S>,
        this: WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) {
        let cached = request.session.load_cached_view(&request.view).await;
        if !Self::show_cached_snapshot(&this, cx, request.request_id, cached) {
            return;
        }
        let outcome = Self::load_refresh_outcome(&request).await;
        Self::finish_refresh(&this, cx, request.request_id, outcome);
    }

    async fn load_refresh_outcome(request: &CalendarRefreshRequest<S>) -> CalendarRefreshOutcome {
        let sync_result = request.session.synchronize_view(&request.view).await;
        let snapshot_result = request.session.load_cached_view(&request.view).await;
        CalendarRefreshOutcome {
            sync_result,
            snapshot_result,
        }
    }

    fn show_cached_snapshot(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
    ) -> bool {
        match snapshot {
            Ok(snapshot) => Self::show_saved_events(this, cx, request_id, snapshot),
            Err(error) => Self::show_cache_failure(this, cx, request_id, error),
        }
    }

    fn show_saved_events(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: einfach_mail_application::CalendarSnapshot,
    ) -> bool {
        this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return false;
            }
            view.apply_snapshot(snapshot);
            view.status = CalendarStatus::Syncing;
            cx.notify();
            true
        })
        .unwrap_or_default()
    }

    fn show_cache_failure(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        error: CalendarWorkflowError,
    ) -> bool {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.sync_running = false;
            view.task = None;
            view.status = CalendarStatus::Failed(error);
            cx.notify();
        });
        false
    }

    fn finish_refresh(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        outcome: CalendarRefreshOutcome,
    ) {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.sync_running = false;
            view.task = None;
            view.apply_refresh_outcome(outcome);
            cx.notify();
        });
    }

    fn apply_refresh_outcome(&mut self, outcome: CalendarRefreshOutcome) {
        match outcome.snapshot_result {
            Ok(snapshot) => {
                self.apply_snapshot(snapshot);
                self.status = match outcome.sync_result {
                    Ok(_) => CalendarStatus::UpToDate,
                    Err(error) => CalendarStatus::Failed(error),
                };
            }
            Err(error) => self.status = CalendarStatus::Failed(error),
        }
    }

    fn apply_snapshot(&mut self, snapshot: einfach_mail_application::CalendarSnapshot) {
        self.events = snapshot.events().to_vec();
        self.pending_creation = snapshot.pending_creation().cloned();
        self.sync_state = Some(snapshot.sync_state().clone());
    }

    fn reload_cached_view(&mut self, cx: &mut Context<Self>) {
        if self.sync_running {
            return;
        }
        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        let session = Arc::clone(&self.session);
        let view = self.view.clone();
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let snapshot = session.load_cached_view(&view).await;
            Self::finish_cache_reload(&this, cx, request_id, snapshot);
        }));
    }

    fn finish_cache_reload(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        request_id: u64,
        snapshot: Result<einfach_mail_application::CalendarSnapshot, CalendarWorkflowError>,
    ) {
        let _ = this.update(cx, |view, cx| {
            if view.request_id != request_id {
                return;
            }
            view.task = None;
            match snapshot {
                Ok(snapshot) => view.apply_snapshot(snapshot),
                Err(error) => view.status = CalendarStatus::Failed(error),
            }
            cx.notify();
        });
    }

    fn open_event_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let intent = EventComposerIntent::Create {
            pending_creation: self.pending_creation.clone(),
        };
        let title = if self.pending_creation.is_some() {
            "Resolve pending event"
        } else {
            "New calendar event"
        };
        self.open_composer(intent, title, window, cx);
    }

    fn open_event_editor(
        &mut self,
        cached_event: CalendarOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(cached_event.time(), CalendarOccurrenceTime::Timed { .. }) {
            return;
        }
        self.open_composer(
            EventComposerIntent::Edit { cached_event },
            "Edit calendar event",
            window,
            cx,
        );
    }

    fn open_composer(
        &mut self,
        intent: EventComposerIntent,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sync_running || window.has_active_sheet(cx) {
            return;
        }
        let setup = self.event_composer_setup(intent);
        let composer = cx.new(|cx| EventComposer::new(setup, window, cx));
        self.composer_subscription = Some(cx.subscribe(
            &composer,
            |view, _, event: &EventComposerEvent, cx| match event {
                EventComposerEvent::CalendarChanged
                | EventComposerEvent::PendingOperationUpdated => view.reload_cached_view(cx),
                EventComposerEvent::ReviewCalendarWritePermission => {
                    cx.emit(CalendarViewEvent::ReviewCalendarWritePermission);
                }
            },
        ));
        Self::open_event_composer_sheet(title, composer, window, cx);
    }

    fn event_composer_setup(&self, intent: EventComposerIntent) -> EventComposerSetup<S> {
        let time_zone_label = if self.utc_fallback {
            "UTC".to_owned()
        } else {
            self.time_zone.iana_name().unwrap_or("UTC").to_owned()
        };
        EventComposerSetup {
            session: Arc::clone(&self.session),
            view: self.view.clone(),
            time_zone: self.time_zone.clone(),
            time_zone_label,
            intent,
            week_start: self.week_start,
        }
    }

    fn open_event_composer_sheet(
        title: &'static str,
        composer: Entity<EventComposer<S>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.open_sheet(cx, move |sheet, _, _| {
            sheet
                .title(div().text_base().font_bold().child(title))
                .child(composer.clone())
        });
    }

    fn change_week(&mut self, weeks: i64, cx: &mut Context<Self>) {
        let Some(week_start) = self.week_start.checked_add(Span::new().weeks(weeks)).ok() else {
            return;
        };
        let Some((view, selected_week)) =
            view_for_week(self.profile.id(), week_start, &self.time_zone)
        else {
            return;
        };
        drop(self.task.take());
        self.request_id = self.request_id.saturating_add(1);
        self.sync_running = false;
        self.week_start = selected_week;
        self.view = view;
        self.events.clear();
        self.sync_state = None;
        self.refresh(cx);
    }

    pub(crate) fn stop_pending_work(&mut self) {
        drop(self.task.take());
        self.request_id = self.request_id.saturating_add(1);
        self.sync_running = false;
        self.events.clear();
        self.pending_creation = None;
        self.sync_state = None;
    }

    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(colors.border)
            .child(self.render_title_bar(cx))
            .child(self.render_week_navigation(cx))
            .child(self.render_status(colors))
            .when(self.utc_fallback, |element| {
                element.child(render_utc_fallback(colors))
            })
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(Self::render_back_button(cx))
            .child(self.render_calendar_title())
            .child(self.render_event_button(cx))
            .child(self.render_sync_button(cx))
    }

    fn render_back_button(cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("calendar-back")
            .debug_selector(|| "calendar-back".to_owned())
            .ghost()
            .label("Accounts")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(CalendarViewEvent::BackToAccount);
            }))
    }

    fn render_calendar_title(&self) -> impl IntoElement {
        div()
            .flex_1()
            .text_lg()
            .font_bold()
            .child(format!("Calendar · {}", self.profile.label()))
    }

    fn render_sync_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("calendar-sync")
            .label("Sync now")
            .loading(self.sync_running)
            .disabled(self.sync_running)
            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx)))
    }

    fn render_event_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .debug_selector(|| "calendar-new-event".to_owned())
            .child(
                Button::new("calendar-new-event")
                    .label(if self.pending_creation.is_some() {
                        "Resolve pending event…"
                    } else {
                        "New event…"
                    })
                    .disabled(self.sync_running)
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_event_composer(window, cx);
                    })),
            )
    }

    fn render_week_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(Self::previous_week_button(cx))
            .child(
                div()
                    .flex_1()
                    .text_base()
                    .font_bold()
                    .child(week_label(self.week_start)),
            )
            .child(Self::next_week_button(cx))
    }

    fn previous_week_button(cx: &mut Context<Self>) -> impl IntoElement {
        week_navigation_button("calendar-previous-week", "Previous week", -1, cx)
    }

    fn next_week_button(cx: &mut Context<Self>) -> impl IntoElement {
        week_navigation_button("calendar-next-week", "Next week", 1, cx)
    }

    fn render_status(&self, colors: ColorTokens) -> AnyElement {
        if let CalendarStatus::Failed(error) = self.status {
            return self.render_sync_error(error, colors);
        }
        self.render_sync_progress(colors)
    }

    fn render_sync_error(&self, error: CalendarWorkflowError, colors: ColorTokens) -> AnyElement {
        div()
            .id("calendar-sync-error")
            .debug_selector(|| "calendar-sync-error".to_owned())
            .role(Role::Alert)
            .text_sm()
            .text_color(colors.destructive)
            .child(calendar_error_message(error, !self.events.is_empty()))
            .into_any_element()
    }

    fn render_sync_progress(&self, colors: ColorTokens) -> AnyElement {
        let (label, role) = match self.status {
            CalendarStatus::LoadingCache => {
                ("Loading saved calendar events…".to_owned(), Role::Status)
            }
            CalendarStatus::Syncing => ("Refreshing calendar events…".to_owned(), Role::Status),
            CalendarStatus::UpToDate => match self
                .sync_state
                .as_ref()
                .and_then(|state| state.last_synchronized_at())
            {
                Some(last_sync) => (elapsed_label(last_sync), Role::Status),
                None => ("Calendar is ready.".to_owned(), Role::Status),
            },
            CalendarStatus::Failed(error) => return self.render_sync_error(error, colors),
        };
        render_status_label(label, role, colors)
    }

    fn render_agenda(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let mut agenda = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(primary_calendar_heading());
        if self.events.is_empty() {
            return agenda
                .child(render_empty_agenda(self.status, colors))
                .into_any_element();
        }

        for offset in 0..7_i64 {
            let Some(date) = self.week_start.checked_add(Span::new().days(offset)).ok() else {
                continue;
            };
            agenda = agenda.child(self.render_day(date, colors, cx));
        }
        agenda.into_any_element()
    }

    fn render_day(
        &self,
        date: civil::Date,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let day_events = display_events(&self.events, date, &self.time_zone);
        div()
            .flex()
            .items_start()
            .gap_4()
            .p_3()
            .border_b_1()
            .border_color(colors.border)
            .child(render_day_label(date))
            .child(self.render_day_events(date, day_events, colors, cx))
    }

    fn render_day_events(
        &self,
        date: civil::Date,
        events: Vec<DisplayEvent<'_>>,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut event_list = div().flex().flex_col().gap_2().flex_1().min_w_0();
        if events.is_empty() {
            return event_list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("No events"),
            );
        }
        for display_event in events {
            let edit_action = match display_event.event.time() {
                CalendarOccurrenceTime::Timed { .. } => {
                    let cached_event = display_event.event.clone();
                    let button_id = format!(
                        "calendar-event-edit-{}-{}",
                        cached_event.id().as_str(),
                        date.strftime("%Y-%m-%d")
                    );
                    Some(
                        Button::new(button_id)
                            .debug_selector(|| "calendar-event-edit".to_owned())
                            .ghost()
                            .label("Edit")
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.open_event_editor(cached_event.clone(), window, cx);
                            }))
                            .into_any_element(),
                    )
                }
                CalendarOccurrenceTime::AllDay { .. } => None,
            };
            event_list = event_list.child(render_event(&display_event, colors, edit_action));
        }
        event_list
    }
}

impl<S> EventEmitter<CalendarViewEvent> for CalendarView<S> where
    S: CalendarSession + ?Sized + 'static
{
}

impl<S> Render for CalendarView<S>
where
    S: CalendarSession + ?Sized + 'static,
{
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("calendar-view")
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
                    .overflow_y_scrollbar()
                    .child(self.render_agenda(colors, cx)),
            )
    }
}

fn week_navigation_button<S>(
    id: &'static str,
    label: &'static str,
    week_offset: i64,
    cx: &mut Context<CalendarView<S>>,
) -> impl IntoElement
where
    S: CalendarSession + ?Sized + 'static,
{
    div().flex().debug_selector(move || id.to_owned()).child(
        Button::new(id)
            .ghost()
            .label(label)
            .on_click(cx.listener(move |view, _, _, cx| view.change_week(week_offset, cx))),
    )
}

fn render_utc_fallback(colors: ColorTokens) -> impl IntoElement {
    div()
        .id("calendar-utc-fallback")
        .debug_selector(|| "calendar-utc-fallback".to_owned())
        .role(Role::Status)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("The system time zone is unavailable. Timed events are shown in UTC.")
}

fn primary_calendar_heading() -> impl IntoElement {
    div().text_sm().font_bold().child("Primary calendar")
}

fn render_empty_agenda(status: CalendarStatus, colors: ColorTokens) -> impl IntoElement {
    let message = match status {
        CalendarStatus::LoadingCache | CalendarStatus::Syncing => "Loading events for this week…",
        CalendarStatus::Failed(_) => "No cached events are available for this week.",
        CalendarStatus::UpToDate => "No events this week.",
    };
    div()
        .id("calendar-empty-state")
        .role(Role::Status)
        .p_4()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(message)
}

fn render_status_label(label: String, role: Role, colors: ColorTokens) -> AnyElement {
    div()
        .id("calendar-sync-status")
        .role(role)
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(label)
        .into_any_element()
}

fn render_day_label(date: civil::Date) -> impl IntoElement {
    div()
        .w_32()
        .flex()
        .flex_col()
        .text_sm()
        .font_bold()
        .child(date_label(date))
}

fn current_week_start(time_zone: &TimeZone) -> Option<civil::Date> {
    let now = Timestamp::try_from(SystemTime::now()).ok()?;
    monday_start(now.to_zoned(time_zone.clone()).date())
}

fn monday_start(date: civil::Date) -> Option<civil::Date> {
    date.checked_sub(Span::new().days(i64::from(date.weekday().to_monday_zero_offset())))
        .ok()
}

fn fallback_week_start() -> civil::Date {
    civil::Date::new(1970, 1, 5).expect("the Unix week baseline is a valid date")
}

fn view_for_week(
    account_id: &einfach_mail_domain::AccountId,
    week_start: civil::Date,
    time_zone: &TimeZone,
) -> Option<(CalendarViewReference, civil::Date)> {
    let week_end = week_start.checked_add(Span::new().weeks(1)).ok()?;
    let start_timestamp = week_start.to_zoned(time_zone.clone()).ok()?.timestamp();
    let end_timestamp = week_end.to_zoned(time_zone.clone()).ok()?.timestamp();
    let start = offset_date_time(start_timestamp)?;
    let end = offset_date_time(end_timestamp)?;
    let start_date = domain_date(week_start)?;
    let end_date = domain_date(week_end)?;
    let range = CalendarViewRange::new(start, end, start_date, end_date).ok()?;
    Some((
        CalendarViewReference::new(account_id.clone(), range),
        week_start,
    ))
}

fn domain_date(date: civil::Date) -> Option<Date> {
    let month = Month::try_from(u8::try_from(date.month()).ok()?).ok()?;
    Date::from_calendar_date(
        i32::from(date.year()),
        month,
        u8::try_from(date.day()).ok()?,
    )
    .ok()
}

fn offset_date_time(timestamp: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(timestamp.as_second())
        .ok()?
        .replace_nanosecond(u32::try_from(timestamp.subsec_nanosecond()).ok()?)
        .ok()
}

fn timestamp(date_time: OffsetDateTime) -> Option<Timestamp> {
    Timestamp::new(
        date_time.unix_timestamp(),
        i32::try_from(date_time.nanosecond()).ok()?,
    )
    .ok()
}

struct CalendarDayRange {
    date: Date,
    start: Timestamp,
    end: Timestamp,
    time_zone: TimeZone,
}

struct TimedEventDisplayRange {
    sort_start: Timestamp,
    local_start: jiff::Zoned,
    local_end: jiff::Zoned,
    starts_before_day: bool,
    ends_after_day: bool,
}

impl CalendarDayRange {
    fn new(date: civil::Date, time_zone: &TimeZone) -> Option<Self> {
        let domain_date = domain_date(date)?;
        let next_date = date.checked_add(Span::new().days(1)).ok()?;
        let start = date.to_zoned(time_zone.clone()).ok()?.timestamp();
        let end = next_date.to_zoned(time_zone.clone()).ok()?.timestamp();
        Some(Self {
            date: domain_date,
            start,
            end,
            time_zone: time_zone.clone(),
        })
    }
}

impl TimedEventDisplayRange {
    fn new(start: OffsetDateTime, end: OffsetDateTime, day: &CalendarDayRange) -> Option<Self> {
        let start = timestamp(start)?;
        let end = timestamp(end)?;
        if start >= day.end || end <= day.start {
            return None;
        }
        Some(Self {
            sort_start: if start < day.start { day.start } else { start },
            local_start: start.to_zoned(day.time_zone.clone()),
            local_end: end.to_zoned(day.time_zone.clone()),
            starts_before_day: start < day.start,
            ends_after_day: end > day.end,
        })
    }
}

fn display_events<'a>(
    events: &'a [CalendarOccurrence],
    date: civil::Date,
    time_zone: &TimeZone,
) -> Vec<DisplayEvent<'a>> {
    let Some(day_range) = CalendarDayRange::new(date, time_zone) else {
        return Vec::new();
    };
    let mut display_events = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| display_event_for_day(event, index, &day_range))
        .collect::<Vec<_>>();
    display_events.sort_by_key(|display_event| display_event.sort_key);
    display_events
}

fn display_event_for_day<'a>(
    event: &'a CalendarOccurrence,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<DisplayEvent<'a>> {
    let (sort_key, time_label) = match event.time() {
        CalendarOccurrenceTime::AllDay {
            start,
            end_exclusive,
        } => all_day_event_display(*start, *end_exclusive, original_index, day)?,
        CalendarOccurrenceTime::Timed { start, end } => {
            timed_event_display(*start, *end, original_index, day)?
        }
    };
    Some(DisplayEvent {
        event,
        sort_key,
        time_label,
    })
}

fn all_day_event_display(
    start: Date,
    end_exclusive: Date,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, String)> {
    if start > day.date || day.date >= end_exclusive {
        return None;
    }
    Some((
        EventSortKey {
            kind: 0,
            seconds: 0,
            nanoseconds: 0,
            original_index,
        },
        "All day".to_owned(),
    ))
}

fn timed_event_display(
    start: OffsetDateTime,
    end: OffsetDateTime,
    original_index: usize,
    day: &CalendarDayRange,
) -> Option<(EventSortKey, String)> {
    let display_range = TimedEventDisplayRange::new(start, end, day)?;
    let time_label = timed_event_time_label(&display_range);
    Some((
        EventSortKey {
            kind: 1,
            seconds: display_range.sort_start.as_second(),
            nanoseconds: display_range.sort_start.subsec_nanosecond(),
            original_index,
        },
        time_label,
    ))
}

fn timed_event_time_label(display_range: &TimedEventDisplayRange) -> String {
    match (
        display_range.starts_before_day,
        display_range.ends_after_day,
    ) {
        (true, true) => "Continues".to_owned(),
        (true, false) => format!("Until {}", display_range.local_end.strftime("%H:%M %Z")),
        (false, true) => format!(
            "{} · continues",
            display_range.local_start.strftime("%H:%M %Z")
        ),
        (false, false) => format!(
            "{}–{}",
            display_range.local_start.strftime("%H:%M %Z"),
            display_range.local_end.strftime("%H:%M %Z")
        ),
    }
}

fn render_event(
    event: &DisplayEvent<'_>,
    colors: ColorTokens,
    edit_action: Option<AnyElement>,
) -> impl IntoElement {
    let mut card = div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .rounded_md()
        .debug_selector(|| "calendar-event".to_owned())
        .child(render_event_heading(event, edit_action));
    if let Some(location) = event.event.location() {
        card = card.child(render_event_location(location, colors));
    }
    card
}

fn render_event_heading(
    event: &DisplayEvent<'_>,
    edit_action: Option<AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(render_event_time(event))
        .child(render_event_subject(event))
        .when_some(edit_action, |element, action| element.child(action))
}

fn render_event_time(event: &DisplayEvent<'_>) -> impl IntoElement {
    div()
        .w_32()
        .text_sm()
        .font_medium()
        .child(event.time_label.clone())
}

fn render_event_subject(event: &DisplayEvent<'_>) -> impl IntoElement {
    let subject = event.event.subject().unwrap_or("(No title)");
    div()
        .flex_1()
        .min_w_0()
        .font_medium()
        .child(subject.to_owned())
}

fn render_event_location(location: &str, colors: ColorTokens) -> impl IntoElement {
    div()
        .pl_32()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(location.to_owned())
}

fn week_label(start: civil::Date) -> String {
    let Some(end) = start.checked_add(Span::new().days(6)).ok() else {
        return start.strftime("%B %-d, %Y").to_string();
    };
    format!(
        "{} – {}",
        start.strftime("%b %-d"),
        end.strftime("%b %-d, %Y")
    )
}

fn date_label(date: civil::Date) -> String {
    date.strftime("%A, %B %-d").to_string()
}

fn elapsed_label(last_sync: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(last_sync)
        .unwrap_or(Duration::ZERO);
    match elapsed.as_secs() {
        seconds if seconds < 60 => "Calendar updated less than a minute ago.".to_owned(),
        seconds if seconds < 3_600 => {
            format!("Calendar updated {} min ago.", seconds / 60)
        }
        seconds if seconds < 86_400 => {
            format!("Calendar updated {} hr ago.", seconds / 3_600)
        }
        seconds => format!("Calendar updated {} days ago.", seconds / 86_400),
    }
}

fn calendar_error_message(error: CalendarWorkflowError, has_cached_events: bool) -> String {
    let message = calendar_error_text(error);
    if has_cached_events {
        format!("Showing saved events. {message}")
    } else {
        message.to_owned()
    }
}

fn calendar_error_text(error: CalendarWorkflowError) -> &'static str {
    match error {
        CalendarWorkflowError::Provider(CalendarOperationError::AccessDenied) => {
            "Microsoft denied calendar access. Review the requested calendar permission or your organization's consent policy."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and connect again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::RateLimited { .. }) => {
            "Microsoft is limiting calendar requests. Wait a moment and sync again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ResourceLimitExceeded)
        | CalendarWorkflowError::PageLimitExceeded
        | CalendarWorkflowError::ResponseLimitExceeded => {
            "Calendar sync paused after reaching its request limit. Sync again to continue."
        }
        CalendarWorkflowError::RepeatedCursor => {
            "Calendar sync stopped because Microsoft repeated a page. Sync again to retry."
        }
        CalendarWorkflowError::OperationInProgress => {
            "Another mail or calendar operation is active. Wait for it to finish, then sync again."
        }
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection and sync again."
        }
        _ => "The calendar could not be updated. Try syncing again.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        future::{Future, ready},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use einfach_mail_application::{
        CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
        CalendarOperationError, CalendarProvider, CalendarSyncState, CalendarWorkflow,
    };
    use einfach_mail_domain::{
        AccountId, AccountProfile, CalendarDeltaPage, CalendarEventCreation, CalendarEventUpdate,
        CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
        CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
        EditableCalendarEvent, EventId, PendingCalendarEventCreation,
    };
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};
    use jiff::{Span, civil, tz::TimeZone};
    use time::{Date, Month, OffsetDateTime};
    use tokio::sync::oneshot;

    use super::{CalendarView, current_week_start, view_for_week};

    #[derive(Clone, Default)]
    struct TestProvider {
        requests: Arc<Mutex<Vec<(i64, i64)>>>,
        event_creation_gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
        event_creation_failure: Arc<Mutex<Option<CalendarOperationError>>>,
        event_creation_count: Arc<AtomicUsize>,
    }

    impl TestProvider {
        fn hold_event_creation_until(&self, gate: oneshot::Receiver<()>) {
            *self
                .event_creation_gate
                .lock()
                .expect("synthetic creation gate mutex is not poisoned") = Some(gate);
        }

        fn fail_next_event_creation(&self, failure: CalendarOperationError) {
            *self
                .event_creation_failure
                .lock()
                .expect("synthetic creation failure mutex is not poisoned") = Some(failure);
        }

        fn take_event_creation_gate(&self) -> Option<oneshot::Receiver<()>> {
            self.event_creation_gate
                .lock()
                .expect("synthetic creation gate mutex is not poisoned")
                .take()
        }

        fn take_event_creation_failure(&self) -> Option<CalendarOperationError> {
            self.event_creation_failure
                .lock()
                .expect("synthetic creation failure mutex is not poisoned")
                .take()
        }
    }

    impl CalendarProvider for TestProvider {
        fn calendar_delta_page(
            &self,
            view: &CalendarViewReference,
            _: CalendarDeltaRequest,
        ) -> impl Future<Output = Result<CalendarDeltaPageResponse, CalendarOperationError>> + Send + '_
        {
            self.requests
                .lock()
                .expect("synthetic calendar request log mutex is not poisoned")
                .push((
                    view.range().start().unix_timestamp(),
                    view.range().end().unix_timestamp(),
                ));
            ready(Err(CalendarOperationError::AccessDenied))
        }

        fn create_event(
            &self,
            creation: &CalendarEventCreation,
        ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_
        {
            let gate = self.take_event_creation_gate();
            let failure = self.take_event_creation_failure();
            let creation = creation.clone();
            let event_creation_count = Arc::clone(&self.event_creation_count);
            async move {
                event_creation_count.fetch_add(1, Ordering::SeqCst);
                create_event_after_wait(creation, gate, failure).await
            }
        }

        fn editable_event(
            &self,
            event: &CalendarOccurrence,
        ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_
        {
            let event = event.clone();
            async move {
                EditableCalendarEvent::new(
                    event,
                    CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                        .map_err(|_| CalendarOperationError::InvalidResponse)?,
                )
                .map_err(|_| CalendarOperationError::EventNotEditable)
            }
        }

        fn update_event(
            &self,
            update: &CalendarEventUpdate,
        ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_
        {
            let update = update.clone();
            async move {
                let mut event = update.source().occurrence().clone();
                let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
                    return Err(CalendarOperationError::InvalidResponse);
                };
                let mut patch = CalendarOccurrencePatch::new();
                patch.set_subject(Some(update.draft().subject().to_owned()));
                let mut time = CalendarOccurrenceTimePatch::new();
                time.set_start(*start);
                time.set_end(*end);
                time.set_is_all_day(false);
                patch.set_time(time);
                patch
                    .apply_to(&mut event)
                    .map_err(|_| CalendarOperationError::InvalidResponse)?;
                Ok(event)
            }
        }
    }

    /// Waits for the optional request gate before returning the configured result.
    async fn create_event_after_wait(
        creation: CalendarEventCreation,
        gate: Option<oneshot::Receiver<()>>,
        failure: Option<CalendarOperationError>,
    ) -> Result<CalendarOccurrence, CalendarOperationError> {
        if let Some(gate) = gate {
            gate.await
                .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
        }
        match failure {
            Some(failure) => Err(failure),
            None => created_event(&creation),
        }
    }

    fn created_event(
        creation: &CalendarEventCreation,
    ) -> Result<CalendarOccurrence, CalendarOperationError> {
        let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
            return Err(CalendarOperationError::InvalidResponse);
        };
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some(creation.draft().subject().to_owned()));
        let mut time = CalendarOccurrenceTimePatch::new();
        time.set_start(*start);
        time.set_end(*end);
        time.set_is_all_day(false);
        patch.set_time(time);
        let id = EventId::try_from(format!(
            "synthetic-created-event-{}",
            creation.id().as_uuid()
        ))
        .map_err(|_| CalendarOperationError::InvalidResponse)?;
        patch
            .into_new(creation.view().clone(), id)
            .map_err(|_| CalendarOperationError::InvalidResponse)
    }

    #[derive(Clone, Default)]
    struct TestCache {
        created_events:
            Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
        pending_creation: Arc<Mutex<Option<PendingCalendarEventCreation>>>,
        fail_next_rejected_cleanup: Arc<AtomicBool>,
        updated_events:
            Arc<Mutex<HashMap<CalendarViewReference, HashMap<String, CalendarOccurrence>>>>,
    }

    impl TestCache {
        fn fail_rejected_cleanup_once(&self) {
            self.fail_next_rejected_cleanup
                .store(true, Ordering::SeqCst);
        }
    }

    impl CalendarCache for TestCache {
        fn ensure_view(
            &self,
            _: &CalendarViewReference,
        ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_
        {
            ready(Ok(CalendarSyncState::initial()))
        }

        fn commit_delta_page(
            &self,
            _: &CalendarViewReference,
            _: CalendarDeltaPage,
            _: std::time::SystemTime,
        ) -> impl Future<Output = Result<CalendarSyncState, CalendarCacheError>> + Send + '_
        {
            ready(Err(CalendarCacheError::InvalidData))
        }

        fn list_events(
            &self,
            view: &CalendarViewReference,
        ) -> impl Future<Output = Result<Vec<CalendarOccurrence>, CalendarCacheError>> + Send + '_
        {
            let created_events = self
                .created_events
                .lock()
                .expect("synthetic event cache mutex is not poisoned")
                .get(view)
                .map(|events| events.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let updated_events = self
                .updated_events
                .lock()
                .expect("synthetic updated event mutex is not poisoned")
                .get(view)
                .map(|events| events.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let mut patch = CalendarOccurrencePatch::new();
            patch.set_subject(Some("Synthetic offline event".to_owned()));
            patch.set_location(Some("Synthetic room".to_owned()));
            let mut time = CalendarOccurrenceTimePatch::new();
            time.set_start(view.range().start_date().midnight().assume_utc());
            let end = view
                .range()
                .start_date()
                .checked_add(time::Duration::days(1))
                .expect("synthetic event end date is valid");
            time.set_end(end.midnight().assume_utc());
            time.set_is_all_day(true);
            patch.set_time(time);
            let event = patch
                .into_new(
                    view.clone(),
                    EventId::try_from("synthetic-ui-event".to_owned())
                        .expect("synthetic event ID is valid"),
                )
                .expect("synthetic cached event has a complete time range");
            let mut events = vec![
                event,
                timed_event(
                    Date::from_calendar_date(2030, Month::January, 9)
                        .expect("synthetic event start date is valid")
                        .with_time(time::Time::from_hms(10, 0, 0).expect("valid time"))
                        .assume_utc(),
                    Date::from_calendar_date(2030, Month::January, 9)
                        .expect("synthetic event end date is valid")
                        .with_time(time::Time::from_hms(11, 0, 0).expect("valid time"))
                        .assume_utc(),
                ),
            ];
            events.retain(|cached| {
                !updated_events
                    .iter()
                    .any(|updated| updated.id() == cached.id())
            });
            events.extend(updated_events);
            events.extend(created_events);
            ready(Ok(events))
        }

        fn stage_event_creation(
            &self,
            creation: &CalendarEventCreation,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            let pending_creation = Arc::clone(&self.pending_creation);
            let creation = creation.clone();
            async move {
                let mut pending = pending_creation
                    .lock()
                    .expect("synthetic pending event mutex is not poisoned");
                match pending.as_ref() {
                    Some(PendingCalendarEventCreation::Unresolved(existing))
                        if existing == &creation =>
                    {
                        Ok(())
                    }
                    None => {
                        *pending = Some(PendingCalendarEventCreation::Unresolved(creation));
                        Ok(())
                    }
                    Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
                }
            }
        }

        fn pending_event_creation(
            &self,
            account_id: &AccountId,
        ) -> impl Future<
            Output = Result<Option<PendingCalendarEventCreation>, CalendarCacheError>,
        > + Send
        + '_ {
            let pending_creation = Arc::clone(&self.pending_creation);
            let account_id = account_id.clone();
            async move {
                let pending = pending_creation
                    .lock()
                    .expect("synthetic pending event mutex is not poisoned")
                    .clone();
                if pending
                    .as_ref()
                    .is_some_and(|pending| pending.creation().view().account_id() != &account_id)
                {
                    Err(CalendarCacheError::AccountMismatch)
                } else {
                    Ok(pending)
                }
            }
        }

        fn mark_event_creation_rejected(
            &self,
            creation: &CalendarEventCreation,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            let pending_creation = Arc::clone(&self.pending_creation);
            let creation = creation.clone();
            async move {
                let mut pending = pending_creation
                    .lock()
                    .expect("synthetic pending event mutex is not poisoned");
                match pending.as_ref() {
                    Some(PendingCalendarEventCreation::Unresolved(existing))
                        if existing == &creation =>
                    {
                        *pending = Some(PendingCalendarEventCreation::Rejected(creation));
                        Ok(())
                    }
                    Some(PendingCalendarEventCreation::Rejected(existing))
                        if existing == &creation =>
                    {
                        Ok(())
                    }
                    None => Ok(()),
                    Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
                }
            }
        }

        fn discard_event_creation(
            &self,
            creation: &CalendarEventCreation,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            let pending_creation = Arc::clone(&self.pending_creation);
            let fail_cleanup = Arc::clone(&self.fail_next_rejected_cleanup);
            let creation = creation.clone();
            async move {
                if fail_cleanup.swap(false, Ordering::SeqCst) {
                    return Err(CalendarCacheError::Unavailable);
                }
                let mut pending = pending_creation
                    .lock()
                    .expect("synthetic pending event mutex is not poisoned");
                match pending.as_ref() {
                    Some(PendingCalendarEventCreation::Rejected(existing))
                        if existing == &creation =>
                    {
                        *pending = None;
                        Ok(())
                    }
                    None => Ok(()),
                    Some(_) => Err(CalendarCacheError::CreationAlreadyPending),
                }
            }
        }

        fn commit_event_creation(
            &self,
            creation: &CalendarEventCreation,
            event: &CalendarOccurrence,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            let created_events = Arc::clone(&self.created_events);
            let creation = creation.clone();
            let event = event.clone();
            let pending_creation = Arc::clone(&self.pending_creation);
            async move {
                if event.reference() != creation.view()
                    || event.subject() != Some(creation.draft().subject())
                    || event.time() != creation.draft().time()
                {
                    return Err(CalendarCacheError::InvalidData);
                }
                let mut pending = pending_creation
                    .lock()
                    .expect("synthetic pending event mutex is not poisoned");
                if pending.as_ref()
                    != Some(&PendingCalendarEventCreation::Unresolved(creation.clone()))
                {
                    return Err(CalendarCacheError::InvalidData);
                }
                created_events
                    .lock()
                    .expect("synthetic event cache mutex is not poisoned")
                    .entry(creation.view().clone())
                    .or_default()
                    .insert(event.id().as_str().to_owned(), event);
                *pending = None;
                Ok(())
            }
        }

        fn commit_event_update(
            &self,
            update: &CalendarEventUpdate,
            event: &CalendarOccurrence,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            let updated_events = Arc::clone(&self.updated_events);
            let update = update.clone();
            let event = event.clone();
            async move {
                if !update.confirms(&event) {
                    return Err(CalendarCacheError::InvalidData);
                }
                updated_events
                    .lock()
                    .expect("synthetic updated event mutex is not poisoned")
                    .entry(event.reference().clone())
                    .or_default()
                    .insert(event.id().as_str().to_owned(), event);
                Ok(())
            }
        }

        fn remove_account(
            &self,
            _: &AccountId,
        ) -> impl Future<Output = Result<(), CalendarCacheError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    fn profile() -> AccountProfile {
        AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid"),
            Some("Ada Example".to_owned()),
            Some("ada@example.test".to_owned()),
            None,
        )
        .expect("synthetic account profile is valid")
    }

    fn timed_event(start: OffsetDateTime, end: OffsetDateTime) -> CalendarOccurrence {
        let account = AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let start_date = Date::from_calendar_date(2030, Month::January, 7)
            .expect("synthetic view start date is valid");
        let end_date = Date::from_calendar_date(2030, Month::January, 14)
            .expect("synthetic view end date is valid");
        let range = CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar range is valid");
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Synthetic overnight event".to_owned()));
        let mut time = CalendarOccurrenceTimePatch::new();
        time.set_start(start);
        time.set_end(end);
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .into_new(
                CalendarViewReference::new(account, range),
                EventId::try_from("synthetic-timed-event".to_owned())
                    .expect("synthetic event ID is valid"),
            )
            .expect("synthetic timed event is valid")
    }

    fn draw_calendar_view(context: &mut VisualTestContext) {
        context.update(|window, app| {
            let _ = window.draw(app);
        });
    }

    #[test]
    fn calendar_view_bounds_follow_local_monday_across_a_dst_transition() {
        let time_zone = TimeZone::get("America/New_York")
            .expect("the synthetic IANA zone exists in Linux zoneinfo");
        let sunday = civil::Date::new(2030, 3, 10).expect("synthetic Sunday is valid");
        let start = super::monday_start(sunday).expect("Monday is in range");
        assert_eq!(start, civil::Date::new(2030, 3, 4).expect("valid date"));
        let account = einfach_mail_domain::AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (view, _) = view_for_week(&account, start, &time_zone)
            .expect("synthetic local week has UTC bounds");
        let range = view.range();
        assert_eq!(range.start_date().day(), 4);
        assert_eq!(range.end_date_exclusive().day(), 11);
        assert_eq!(
            range.end() - range.start(),
            time::Duration::days(7) - time::Duration::hours(1)
        );
    }

    #[test]
    fn unix_utc_fallback_uses_utc_week_and_labels_it_for_the_user() {
        let current = current_week_start(&TimeZone::UTC).expect("current timestamp fits Jiff");
        let account = einfach_mail_domain::AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is valid");
        let (view, week_start) =
            view_for_week(&account, current, &TimeZone::UTC).expect("synthetic UTC week is valid");

        assert_eq!(week_start.weekday(), jiff::civil::Weekday::Monday);
        assert_eq!(view.range().start().offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn timed_events_that_cross_midnight_appear_on_each_occupied_day() {
        let event = timed_event(
            Date::from_calendar_date(2030, Month::January, 6)
                .expect("synthetic date is valid")
                .with_time(time::Time::from_hms(23, 0, 0).expect("synthetic time is valid"))
                .assume_utc(),
            Date::from_calendar_date(2030, Month::January, 8)
                .expect("synthetic date is valid")
                .with_time(time::Time::from_hms(10, 0, 0).expect("synthetic time is valid"))
                .assume_utc(),
        );
        let time_zone = TimeZone::UTC;
        let first_day = civil::Date::new(2030, 1, 7).expect("synthetic date is valid");
        let last_day = civil::Date::new(2030, 1, 8).expect("synthetic date is valid");

        let first_day_events =
            super::display_events(std::slice::from_ref(&event), first_day, &time_zone);
        let last_day_events =
            super::display_events(std::slice::from_ref(&event), last_day, &time_zone);

        assert_eq!(first_day_events.len(), 1);
        assert_eq!(first_day_events[0].time_label, "Continues");
        assert_eq!(last_day_events.len(), 1);
        assert_eq!(last_day_events[0].time_label, "Until 10:00 UTC");
        assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
    }

    #[gpui_kit::test]
    async fn native_calendar_view_keeps_cached_events_and_navigates_fixed_weeks(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let time_zone = TimeZone::UTC;
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
        let provider = TestProvider::default();
        let requests = Arc::clone(&provider.requests);
        let workflow = Arc::new(CalendarWorkflow::new(provider, TestCache::default()));
        let (view, context) = cx.add_window_view(|_, cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::CalendarWeekSelection {
                    week_start,
                    time_zone,
                    utc_fallback: true,
                },
                cx,
            )
        });

        context.run_until_parked();
        let mut rendered_sync_error = false;
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("calendar-sync-error").is_some() {
                rendered_sync_error = true;
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        let (sync_running, status_failed) = context.cx.update(|cx| {
            let view = view.read(cx);
            (
                view.sync_running,
                matches!(view.status, super::CalendarStatus::Failed(_)),
            )
        });
        assert!(!sync_running && status_failed, "calendar refresh completes");
        assert!(rendered_sync_error, "calendar refresh reports its error");
        assert!(context.debug_bounds("calendar-sync-error").is_some());
        assert!(context.debug_bounds("calendar-utc-fallback").is_some());
        assert!(context.debug_bounds("calendar-event").is_some());
        assert!(context.debug_bounds("calendar-next-week").is_some());

        let next_week_button = context
            .debug_bounds("calendar-next-week")
            .expect("week navigation button is rendered");
        context.simulate_click(next_week_button.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);

        let (selected_week, range) = context.cx.update(|cx| {
            let view = view.read(cx);
            (view.week_start, view.view.range().clone())
        });
        assert_eq!(
            selected_week,
            week_start
                .checked_add(Span::new().weeks(1))
                .expect("following week is in range")
        );
        assert_eq!(
            range.start_date(),
            Date::from_calendar_date(2030, Month::January, 14).expect("valid date")
        );
        assert_eq!(
            requests
                .lock()
                .expect("synthetic calendar request log mutex is not poisoned")
                .len(),
            2
        );
    }

    #[gpui_kit::test]
    async fn native_event_composer_recovers_uncertain_and_rejected_creations(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let provider = TestProvider::default();
        let cache = TestCache::default();
        let workflow = Arc::new(CalendarWorkflow::new(provider.clone(), cache.clone()));
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
        let (_, context) = cx.add_window_view(|window, cx| {
            let calendar = cx.new(|cx| {
                CalendarView::new_for_week(
                    profile(),
                    Arc::clone(&workflow),
                    super::CalendarWeekSelection {
                        week_start,
                        time_zone: TimeZone::UTC,
                        utc_fallback: true,
                    },
                    cx,
                )
            });
            gpui_kit::base::Root::new(calendar, window, cx)
        });
        let mut sync_finished = false;
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("calendar-sync-error").is_some() {
                sync_finished = true;
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(sync_finished, "calendar sync reaches a visible error state");

        let new_event = context
            .debug_bounds("calendar-new-event")
            .expect("calendar offers event creation");
        context.simulate_click(new_event.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);
        assert!(context.update(|window, cx| window.has_active_sheet(cx)));
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-composer-fields").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-composer-fields").is_some());
        let create_event = context
            .debug_bounds("event-composer-submit")
            .expect("composer offers create action");
        context.simulate_click(create_event.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);
        assert!(context.debug_bounds("event-composer-validation").is_some());
        assert!(context.debug_bounds("event-creation-pending").is_none());

        let title = context
            .debug_bounds("event-composer-subject-input")
            .expect("composer exposes the title input");
        context.simulate_click(title.center(), Modifiers::default());
        context.simulate_input("Synthetic event");
        let (release, gate) = oneshot::channel();
        provider.hold_event_creation_until(gate);
        provider.fail_next_event_creation(CalendarOperationError::ServiceUnavailable);
        let create_event = context
            .debug_bounds("event-composer-submit")
            .expect("composer can submit the completed draft");
        context.simulate_click(create_event.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);
        assert!(context.debug_bounds("event-creation-submitting").is_some());
        release
            .send(())
            .expect("synthetic provider request is waiting on this test");

        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-creation-status").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-creation-pending").is_some());
        assert!(context.debug_bounds("event-creation-status").is_some());

        let retry = context
            .debug_bounds("event-composer-submit")
            .expect("the saved operation remains retryable");
        context.simulate_click(retry.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-composer-close").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-creation-pending").is_none());
        assert!(context.debug_bounds("event-creation-status").is_some());
        assert!(context.debug_bounds("event-composer-close").is_some());

        let close = context
            .debug_bounds("event-composer-close")
            .expect("confirmed composer can be closed");
        context.simulate_click(close.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);

        let new_event = context
            .debug_bounds("calendar-new-event")
            .expect("calendar offers another event creation");
        context.simulate_click(new_event.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);
        assert!(context.debug_bounds("event-composer-fields").is_some());
        let title = context
            .debug_bounds("event-composer-subject-input")
            .expect("composer exposes the title input");
        context.simulate_click(title.center(), Modifiers::default());
        context.simulate_input("Rejected synthetic event");
        provider.fail_next_event_creation(CalendarOperationError::EventCreationRejected);
        cache.fail_rejected_cleanup_once();

        let create_event = context
            .debug_bounds("event-composer-submit")
            .expect("composer offers create action");
        context.simulate_click(create_event.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-creation-status").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-creation-pending").is_some());
        assert!(context.debug_bounds("event-creation-status").is_some());
        let rejected_marker = workflow
            .load_cached_view(
                &view_for_week(profile().id(), week_start, &TimeZone::UTC)
                    .expect("synthetic calendar week is valid")
                    .0,
            )
            .await
            .expect("rejected operation remains available for cleanup");
        assert!(matches!(
            rejected_marker.pending_creation(),
            Some(PendingCalendarEventCreation::Rejected(_))
        ));
        assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 3);

        let clear_rejected = context
            .debug_bounds("event-composer-submit")
            .expect("rejected operation offers local cleanup");
        context.simulate_click(clear_rejected.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-composer-close").is_some()
                && context.debug_bounds("event-creation-pending").is_none()
            {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-creation-pending").is_none());
        assert!(context.debug_bounds("event-composer-close").is_some());
        assert_eq!(provider.event_creation_count.load(Ordering::SeqCst), 3);
        let cleared_marker = workflow
            .load_cached_view(
                &view_for_week(profile().id(), week_start, &TimeZone::UTC)
                    .expect("synthetic calendar week is valid")
                    .0,
            )
            .await
            .expect("calendar view can read its cache");
        assert!(cleared_marker.pending_creation().is_none());
    }

    #[gpui_kit::test]
    async fn denied_calendar_write_emits_permission_review_request(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let provider = TestProvider::default();
        provider.fail_next_event_creation(CalendarOperationError::AccessDenied);
        let workflow = Arc::new(CalendarWorkflow::new(
            provider.clone(),
            TestCache::default(),
        ));
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
        let permission_review_requested = Arc::new(AtomicBool::new(false));
        let requested = Arc::clone(&permission_review_requested);
        let mut subscription = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let calendar = cx.new(|cx| {
                CalendarView::new_for_week(
                    profile(),
                    workflow,
                    super::CalendarWeekSelection {
                        week_start,
                        time_zone: TimeZone::UTC,
                        utc_fallback: true,
                    },
                    cx,
                )
            });
            subscription = Some(cx.subscribe(
                &calendar,
                move |_, _, event: &super::CalendarViewEvent, _| {
                    if matches!(
                        event,
                        super::CalendarViewEvent::ReviewCalendarWritePermission
                    ) {
                        requested.store(true, Ordering::SeqCst);
                    }
                },
            ));
            gpui_kit::base::Root::new(calendar, window, cx)
        });
        let _subscription = subscription;

        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("calendar-sync-error").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        let new_event = context
            .debug_bounds("calendar-new-event")
            .expect("calendar offers event creation");
        context.simulate_click(new_event.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);

        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("event-composer-fields").is_some() {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        let title = context
            .debug_bounds("event-composer-subject-input")
            .expect("composer exposes the title input");
        context.simulate_click(title.center(), Modifiers::default());
        context.simulate_input("Synthetic event");
        let submit = context
            .debug_bounds("event-composer-submit")
            .expect("composer offers create action");
        context.simulate_click(submit.center(), Modifiers::default());

        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context
                .debug_bounds("review-calendar-write-permission")
                .is_some()
            {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        let review = context
            .debug_bounds("review-calendar-write-permission")
            .expect("denied write exposes an explicit permission review");
        context.simulate_click(review.center(), Modifiers::default());
        context.run_until_parked();

        assert!(permission_review_requested.load(Ordering::SeqCst));
    }

    #[gpui_kit::test]
    async fn native_event_editor_updates_a_cached_timed_appointment(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let workflow = Arc::new(CalendarWorkflow::new(
            TestProvider::default(),
            TestCache::default(),
        ));
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
        let mut calendar_handle = None;
        let (_, context) = cx.add_window_view(|window, cx| {
            let calendar = cx.new(|cx| {
                CalendarView::new_for_week(
                    profile(),
                    workflow,
                    super::CalendarWeekSelection {
                        week_start,
                        time_zone: TimeZone::UTC,
                        utc_fallback: true,
                    },
                    cx,
                )
            });
            calendar_handle = Some(calendar.clone());
            gpui_kit::base::Root::new(calendar, window, cx)
        });
        let calendar = calendar_handle.expect("calendar entity is retained for assertions");

        let mut sync_finished = false;
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if context.debug_bounds("calendar-sync-error").is_some() {
                sync_finished = true;
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(
            sync_finished,
            "calendar sync reaches its visible error state"
        );

        let edit = context
            .debug_bounds("calendar-event-edit")
            .expect("timed calendar event offers an edit action");
        context.simulate_click(edit.center(), Modifiers::default());
        context.run_until_parked();
        draw_calendar_view(context);
        for _ in 0..100 {
            if context.debug_bounds("event-composer-fields").is_some() {
                break;
            }
            context.run_until_parked();
            draw_calendar_view(context);
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("event-composer-fields").is_some());

        let title = context
            .debug_bounds("event-composer-subject-input")
            .expect("editor exposes the event title");
        context.simulate_click(title.center(), Modifiers::default());
        context.simulate_keystrokes("ctrl-a");
        context.simulate_input("Updated synthetic appointment");
        draw_calendar_view(context);

        let save = context
            .debug_bounds("event-composer-submit")
            .expect("editor offers a save action");
        context.simulate_click(save.center(), Modifiers::default());
        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            let updated = context.cx.update(|cx| {
                calendar
                    .read(cx)
                    .events
                    .iter()
                    .any(|event| event.subject() == Some("Updated synthetic appointment"))
            });
            if updated {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        let (updated, saved_status) = context.cx.update(|cx| {
            let view = calendar.read(cx);
            (
                view.events
                    .iter()
                    .any(|event| event.subject() == Some("Updated synthetic appointment")),
                view.events.len(),
            )
        });
        assert!(
            updated,
            "confirmed edit reloads the updated cached occurrence"
        );
        assert_eq!(
            saved_status, 2,
            "the calendar retains both synthetic events"
        );
        assert!(context.debug_bounds("event-creation-status").is_some());
        assert!(context.debug_bounds("event-composer-close").is_some());
    }
}
