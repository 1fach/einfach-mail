use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use einfach_mail_application::{CalendarOperationError, CalendarSession, CalendarWorkflowError};
use einfach_mail_domain::{
    AccountProfile, CalendarOccurrence, CalendarOccurrenceTime, CalendarViewRange,
    CalendarViewReference, EventId,
};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, EventEmitter, FocusHandle, IntoElement, KeyBinding, Render,
    Role, ScrollHandle, Task, WeakEntity, Window, div, prelude::*,
};
use jiff::{Span, Timestamp, civil, tz::TimeZone};
use time::{Date, Month, OffsetDateTime};

gpui_kit::actions!(
    calendar_event_list,
    [
        SelectPreviousCalendarEvent,
        SelectNextCalendarEvent,
        SelectFirstCalendarEvent,
        SelectLastCalendarEvent,
    ]
);

const CALENDAR_EVENT_LIST_CONTEXT: &str = "CalendarEventList";

pub(crate) fn bind_calendar_event_navigation_keys(cx: &mut App) {
    let context = Some(CALENDAR_EVENT_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousCalendarEvent, context),
        KeyBinding::new("down", SelectNextCalendarEvent, context),
        KeyBinding::new("home", SelectFirstCalendarEvent, context),
        KeyBinding::new("end", SelectLastCalendarEvent, context),
    ]);
}

pub(crate) enum CalendarViewEvent {
    BackToAccount,
}

pub(crate) struct CalendarView<S: CalendarSession + ?Sized = dyn CalendarSession> {
    profile: AccountProfile,
    session: Arc<S>,
    time_zone: TimeZone,
    utc_fallback: bool,
    week_start: civil::Date,
    view: CalendarViewReference,
    events: Vec<CalendarOccurrence>,
    event_list_focus: FocusHandle,
    agenda_scroll: ScrollHandle,
    selected_agenda_event: Option<AgendaEventKey>,
    sync_state: Option<einfach_mail_application::CalendarSyncState>,
    status: CalendarStatus,
    sync_running: bool,
    request_id: u64,
    task: Option<Task<()>>,
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

#[derive(Clone, Eq, PartialEq)]
struct AgendaEventKey {
    date: civil::Date,
    event_id: EventId,
}

struct AgendaEvent<'a> {
    key: AgendaEventKey,
    display: DisplayEvent<'a>,
}

enum AgendaRow<'a> {
    DayHeading(civil::Date),
    EmptyDay,
    Event(AgendaEvent<'a>),
}

#[derive(Clone, Copy)]
enum CalendarEventNavigation {
    Previous,
    Next,
    First,
    Last,
}

impl CalendarEventNavigation {
    fn target_index(self, selected_index: Option<usize>, event_count: usize) -> Option<usize> {
        if event_count == 0 {
            return None;
        }

        let last_index = event_count - 1;
        match self {
            Self::Previous => Some(
                selected_index.map_or(last_index, |index| index.min(last_index).saturating_sub(1)),
            ),
            Self::Next => Some(selected_index.map_or(0, |index| {
                index.min(last_index).saturating_add(1).min(last_index)
            })),
            Self::First => Some(0),
            Self::Last => Some(last_index),
        }
    }
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
            Ok(time_zone) => (time_zone, false),
            Err(_) => (TimeZone::UTC, true),
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
        let mut calendar_view = Self::loading_for_selection(profile, session, view, selection, cx);
        calendar_view.refresh(cx);
        calendar_view
    }

    fn loading_for_selection(
        profile: AccountProfile,
        session: Arc<S>,
        view: CalendarViewReference,
        selection: CalendarWeekSelection,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            profile,
            session,
            time_zone: selection.time_zone,
            utc_fallback: selection.utc_fallback,
            week_start: selection.week_start,
            view,
            events: Vec::new(),
            event_list_focus: cx.focus_handle().tab_stop(true),
            agenda_scroll: ScrollHandle::new(),
            selected_agenda_event: None,
            sync_state: None,
            status: CalendarStatus::LoadingCache,
            sync_running: false,
            request_id: 0,
            task: None,
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
        if let Some(selected) = self.selected_agenda_event.clone() {
            let selected_row = self
                .agenda_rows()
                .iter()
                .position(|row| matches!(row, AgendaRow::Event(event) if event.key == selected));
            if let Some(scroll_index) = selected_row {
                self.agenda_scroll.scroll_to_item(scroll_index);
            } else {
                self.selected_agenda_event = None;
            }
        }
        self.sync_state = Some(snapshot.sync_state().clone());
    }

    fn agenda_rows(&self) -> Vec<AgendaRow<'_>> {
        let mut rows = Vec::new();
        for offset in 0..7_i64 {
            let Some(date) = self.week_start.checked_add(Span::new().days(offset)).ok() else {
                continue;
            };
            rows.push(AgendaRow::DayHeading(date));
            let day_events = display_events(&self.events, date, &self.time_zone);
            if day_events.is_empty() {
                rows.push(AgendaRow::EmptyDay);
            } else {
                rows.extend(day_events.into_iter().map(|display| {
                    AgendaRow::Event(AgendaEvent {
                        key: AgendaEventKey {
                            date,
                            event_id: display.event.id().clone(),
                        },
                        display,
                    })
                }));
            }
        }
        rows
    }

    fn navigate_event(&mut self, navigation: CalendarEventNavigation, cx: &mut Context<Self>) {
        let rows = self.agenda_rows();
        let event_rows = rows
            .iter()
            .enumerate()
            .filter_map(|(row_index, row)| match row {
                AgendaRow::Event(event) => Some((row_index, &event.key)),
                AgendaRow::DayHeading(_) | AgendaRow::EmptyDay => None,
            })
            .collect::<Vec<_>>();
        let selected_index = self
            .selected_agenda_event
            .as_ref()
            .and_then(|selected| event_rows.iter().position(|(_, key)| *key == selected));
        let Some(target_index) = navigation.target_index(selected_index, event_rows.len()) else {
            return;
        };
        let Some((scroll_index, selected_event)) = event_rows.get(target_index) else {
            return;
        };
        let scroll_index = *scroll_index;
        let selected_event = (*selected_event).clone();
        self.agenda_scroll.scroll_to_item(scroll_index);
        self.selected_agenda_event = Some(selected_event);
        cx.notify();
    }

    fn select_event_from_option(
        &mut self,
        selected_event: AgendaEventKey,
        scroll_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .agenda_rows()
            .iter()
            .any(|row| matches!(row, AgendaRow::Event(event) if event.key == selected_event))
        {
            return;
        }
        self.agenda_scroll.scroll_to_item(scroll_index);
        self.event_list_focus.focus(window, cx);
        self.selected_agenda_event = Some(selected_event);
        cx.notify();
    }

    fn on_select_previous_calendar_event(
        &mut self,
        _: &SelectPreviousCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(CalendarEventNavigation::Previous, cx);
    }

    fn on_select_next_calendar_event(
        &mut self,
        _: &SelectNextCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(CalendarEventNavigation::Next, cx);
    }

    fn on_select_first_calendar_event(
        &mut self,
        _: &SelectFirstCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(CalendarEventNavigation::First, cx);
    }

    fn on_select_last_calendar_event(
        &mut self,
        _: &SelectLastCalendarEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_event(CalendarEventNavigation::Last, cx);
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
        self.agenda_scroll.scroll_to_item(0);
        self.selected_agenda_event = None;
        self.sync_state = None;
        self.refresh(cx);
    }

    pub(crate) fn stop_pending_work(&mut self) {
        drop(self.task.take());
        self.request_id = self.request_id.saturating_add(1);
        self.sync_running = false;
        self.events.clear();
        self.selected_agenda_event = None;
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
            .child(self.render_sync_button(cx))
    }

    fn render_back_button(cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("calendar-back")
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

    fn render_agenda(&self, colors: ColorTokens, cx: &mut Context<Self>) -> AnyElement {
        let agenda = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(primary_calendar_heading());
        let rows = self.agenda_rows();
        let event_count = rows
            .iter()
            .filter(|row| matches!(row, AgendaRow::Event(_)))
            .count();
        if event_count == 0 {
            return agenda
                .child(render_empty_agenda(self.status, colors))
                .into_any_element();
        }

        let mut event_position = 0;
        let mut event_list = div()
            .id("calendar-event-list")
            .debug_selector(|| "calendar-event-list".to_owned())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .track_scroll(&self.agenda_scroll)
            .track_focus(&self.event_list_focus)
            .key_context(CALENDAR_EVENT_LIST_CONTEXT)
            .role(Role::ListBox)
            .aria_label("Events this week")
            .on_action(cx.listener(Self::on_select_previous_calendar_event))
            .on_action(cx.listener(Self::on_select_next_calendar_event))
            .on_action(cx.listener(Self::on_select_first_calendar_event))
            .on_action(cx.listener(Self::on_select_last_calendar_event))
            .focus_visible(|style| style.border_1().border_color(colors.ring))
            .overflow_y_scroll();

        for (scroll_index, row) in rows.iter().enumerate() {
            event_list = match row {
                AgendaRow::DayHeading(date) => event_list.child(render_agenda_day_heading(*date)),
                AgendaRow::EmptyDay => event_list.child(render_empty_day(colors)),
                AgendaRow::Event(event) => {
                    event_position += 1;
                    event_list.child(render_agenda_event(
                        event,
                        AgendaOptionPosition {
                            scroll_index,
                            event_position,
                            event_count,
                            selected: self.selected_agenda_event.as_ref() == Some(&event.key),
                        },
                        colors,
                        cx,
                    ))
                }
            };
        }
        agenda
            .child(event_list.vertical_scrollbar(&self.agenda_scroll))
            .into_any_element()
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
            .child(self.render_agenda(colors, cx))
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

fn render_agenda_day_heading(date: civil::Date) -> impl IntoElement {
    div()
        .id(format!("calendar-day-{}", date.strftime("%Y-%m-%d")))
        .debug_selector(move || format!("calendar-day-{}", date.strftime("%Y-%m-%d")))
        .w_full()
        .pt_3()
        .pb_1()
        .text_sm()
        .font_bold()
        .child(date_label(date))
}

fn render_empty_day(colors: ColorTokens) -> impl IntoElement {
    div()
        .w_full()
        .px_3()
        .py_2()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child("No events")
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

#[derive(Clone, Copy)]
struct AgendaOptionPosition {
    scroll_index: usize,
    event_position: usize,
    event_count: usize,
    selected: bool,
}

fn render_agenda_event<S>(
    agenda_event: &AgendaEvent<'_>,
    position: AgendaOptionPosition,
    colors: ColorTokens,
    cx: &mut Context<CalendarView<S>>,
) -> impl IntoElement
where
    S: CalendarSession + ?Sized + 'static,
{
    let selected = position.selected;
    let event_key = agenda_event.key.clone();
    let date_id = event_key.date.strftime("%Y-%m-%d").to_string();
    let event_id = event_key.event_id.as_str().to_owned();
    let accessible_label = calendar_event_accessible_label(&agenda_event.display, event_key.date);
    let mut card = div()
        .id(format!("calendar-event-option-{date_id}-{event_id}"))
        .debug_selector(move || format!("calendar-event-row-{date_id}-{event_id}"))
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(if selected {
            colors.accent
        } else {
            colors.surface
        })
        .text_color(if selected {
            colors.accent_foreground
        } else {
            colors.foreground
        })
        .border_1()
        .border_color(if selected {
            colors.accent
        } else {
            colors.border
        })
        .rounded_md()
        .role(Role::ListBoxOption)
        .aria_label(accessible_label)
        .aria_position_in_set(position.event_position)
        .aria_size_of_set(position.event_count)
        .aria_selected(selected)
        .when(selected, |option| option.aria_active_descendant())
        .hover(|option| {
            option.bg(if selected {
                colors.accent
            } else {
                colors.muted
            })
        })
        .child(render_event_heading(&agenda_event.display));
    if let Some(location) = agenda_event.display.event.location() {
        card = card.child(render_event_location(location, colors, selected));
    }
    card.on_click(cx.listener(move |view, _, window, cx| {
        view.select_event_from_option(event_key.clone(), position.scroll_index, window, cx);
    }))
}

fn calendar_event_accessible_label(event: &DisplayEvent<'_>, date: civil::Date) -> String {
    let subject = event.event.subject().unwrap_or("(No title)");
    match event.event.location() {
        Some(location) => format!(
            "{}, {}, {}, {}",
            date_label(date),
            event.time_label,
            subject,
            location
        ),
        None => format!("{}, {}, {}", date_label(date), event.time_label, subject),
    }
}

fn render_event_heading(event: &DisplayEvent<'_>) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(render_event_time(event))
        .child(render_event_subject(event))
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

fn render_event_location(location: &str, colors: ColorTokens, selected: bool) -> impl IntoElement {
    div()
        .pl_32()
        .text_sm()
        .text_color(if selected {
            colors.accent_foreground
        } else {
            colors.muted_foreground
        })
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
        CalendarWorkflowError::Provider(CalendarOperationError::ServiceUnavailable) => {
            "The calendar service is unavailable. Check your connection and sync again."
        }
        _ => "The calendar could not be updated. Try syncing again.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{Future, ready},
        ops::Deref,
        sync::{Arc, Mutex},
    };

    use einfach_mail_application::{
        CalendarCache, CalendarCacheError, CalendarDeltaPageResponse, CalendarDeltaRequest,
        CalendarOperationError, CalendarProvider, CalendarSession, CalendarSyncState,
        CalendarWorkflow,
    };
    use einfach_mail_domain::{
        AccountId, AccountProfile, CalendarDeltaPage, CalendarOccurrence, CalendarOccurrencePatch,
        CalendarOccurrenceTime, CalendarOccurrenceTimePatch, CalendarViewRange,
        CalendarViewReference, EventId,
    };
    use gpui_kit::{AppContext, Modifiers, TestAppContext, VisualTestContext, px, size};
    use jiff::{Span, civil, tz::TimeZone};
    use time::{Date, Month, OffsetDateTime};

    use super::{CalendarEventNavigation, CalendarView, current_week_start, view_for_week};

    #[derive(Clone, Default)]
    struct TestProvider {
        requests: Arc<Mutex<Vec<(i64, i64)>>>,
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
    }

    #[derive(Clone, Default)]
    struct TestCache {
        events: Option<Vec<CalendarOccurrence>>,
    }

    impl TestCache {
        fn with_events(events: Vec<CalendarOccurrence>) -> Self {
            Self {
                events: Some(events),
            }
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
            if let Some(events) = &self.events {
                return ready(Ok(events.clone()));
            }
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
            ready(Ok(vec![event]))
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

    fn timed_event(
        event_id: &str,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> CalendarOccurrence {
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
        patch.set_subject(Some(format!("Synthetic event {event_id}")));
        let mut time = CalendarOccurrenceTimePatch::new();
        time.set_start(start);
        time.set_end(end);
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .into_new(
                CalendarViewReference::new(account, range),
                EventId::try_from(event_id.to_owned()).expect("synthetic event ID is valid"),
            )
            .expect("synthetic timed event is valid")
    }

    fn agenda_events(count: usize) -> Vec<CalendarOccurrence> {
        let date =
            Date::from_calendar_date(2030, Month::January, 7).expect("synthetic date is valid");
        (0..count)
            .map(|index| {
                let event_id = format!("synthetic-calendar-event-{}", index + 1);
                let start_minutes = 8 * 60 + i32::try_from(index).expect("event index fits") * 5;
                let start = date.midnight().assume_utc().replace_time(
                    time::Time::from_hms(
                        u8::try_from(start_minutes / 60).expect("hour fits"),
                        u8::try_from(start_minutes % 60).expect("minute fits"),
                        0,
                    )
                    .expect("synthetic event time is valid"),
                );
                timed_event(&event_id, start, start + time::Duration::minutes(30))
            })
            .collect()
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
            "synthetic-timed-event",
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
    async fn calendar_agenda_navigates_events_with_one_focus_stop_and_keeps_selection_visible(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_calendar_event_navigation_keys(cx);
        });
        let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
        let workflow = Arc::new(CalendarWorkflow::new(
            TestProvider::default(),
            TestCache::with_events(agenda_events(20)),
        ));
        let mut calendar_view = None;
        let window = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
            let view = cx.new(|cx| {
                CalendarView::new_for_week(
                    profile(),
                    workflow,
                    super::CalendarWeekSelection {
                        week_start,
                        time_zone: TimeZone::UTC,
                        utc_fallback: false,
                    },
                    cx,
                )
            });
            calendar_view = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        let context = VisualTestContext::from_window(*window.deref(), cx).into_mut();
        let view = calendar_view.expect("calendar view is created with the test window");

        for _ in 0..100 {
            context.run_until_parked();
            draw_calendar_view(context);
            if !context.cx.update(|cx| view.read(cx).sync_running) {
                break;
            }
            context
                .cx
                .executor()
                .timer(std::time::Duration::from_millis(10))
                .await;
        }
        assert!(context.debug_bounds("calendar-event-list").is_some());
        assert!(
            context
                .cx
                .update(|cx| view.read(cx).selected_agenda_event.is_none())
        );

        context.update(gpui_kit::Window::focus_next);
        let mut list_focused =
            context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
        for _ in 0..24 {
            if list_focused {
                break;
            }
            context.simulate_keystrokes("tab");
            list_focused =
                context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window));
            if list_focused {
                break;
            }
        }
        assert!(list_focused, "Tab reaches the calendar event list");

        context.simulate_keystrokes("up");
        assert_selected_event(context, &view, "synthetic-calendar-event-20");
        context.simulate_keystrokes("down");
        assert_selected_event(context, &view, "synthetic-calendar-event-20");

        context.simulate_keystrokes("home");
        assert_selected_event(context, &view, "synthetic-calendar-event-1");
        context.simulate_keystrokes("up");
        assert_selected_event(context, &view, "synthetic-calendar-event-1");
        context.simulate_keystrokes("down");
        assert_selected_event(context, &view, "synthetic-calendar-event-2");
        context.simulate_keystrokes("end");
        context.run_until_parked();
        draw_calendar_view(context);
        assert_selected_event(context, &view, "synthetic-calendar-event-20");

        let list_bounds = context
            .debug_bounds("calendar-event-list")
            .expect("event list is rendered");
        let selected_bounds = context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-20")
            .expect("End scrolls the last event into view");
        assert!(
            selected_bounds.top() >= list_bounds.top(),
            "selected event starts above the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
        );
        assert!(
            selected_bounds.bottom() <= list_bounds.bottom(),
            "selected event ends below the agenda viewport; list={list_bounds:?}, selected={selected_bounds:?}"
        );
        let (scroll_offset, max_scroll_offset) = context.cx.update(|cx| {
            let scroll = &view.read(cx).agenda_scroll;
            (scroll.offset(), scroll.max_offset())
        });
        assert!(
            max_scroll_offset.y > px(0.),
            "the short test viewport should require agenda scrolling; max={max_scroll_offset:?}, list={list_bounds:?}"
        );
        assert!(
            scroll_offset.y < px(0.),
            "End should scroll the agenda down; offset={scroll_offset:?}, max={max_scroll_offset:?}"
        );

        context.cx.update(|cx| {
            view.update(cx, |view, _| view.agenda_scroll.scroll_to_item(0));
        });
        context.run_until_parked();
        draw_calendar_view(context);
        context.simulate_keystrokes("tab");
        assert!(
            !context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)),
            "Tab leaves the event list as one focus stop"
        );
        let first_row = context
            .debug_bounds("calendar-event-row-2030-01-07-synthetic-calendar-event-1")
            .expect("the first event is visible after Home");
        context.simulate_click(first_row.center(), Modifiers::default());
        assert_selected_event(context, &view, "synthetic-calendar-event-1");
        assert!(context.update(|window, cx| view.read(cx).event_list_focus.is_focused(window)));
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
        assert!(
            context
                .debug_bounds("calendar-event-row-2030-01-07-synthetic-ui-event")
                .is_some()
        );
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

    #[test]
    fn calendar_event_navigation_handles_empty_lists_and_stops_at_boundaries() {
        let cases = [
            (CalendarEventNavigation::Previous, None, 0, None),
            (CalendarEventNavigation::Next, None, 0, None),
            (CalendarEventNavigation::First, None, 0, None),
            (CalendarEventNavigation::Last, None, 0, None),
            (CalendarEventNavigation::Previous, None, 3, Some(2)),
            (CalendarEventNavigation::Next, None, 3, Some(0)),
            (CalendarEventNavigation::Previous, Some(0), 3, Some(0)),
            (CalendarEventNavigation::Next, Some(2), 3, Some(2)),
            (CalendarEventNavigation::First, Some(1), 3, Some(0)),
            (CalendarEventNavigation::Last, Some(1), 3, Some(2)),
        ];

        for (navigation, selected_index, event_count, expected_index) in cases {
            assert_eq!(
                navigation.target_index(selected_index, event_count),
                expected_index
            );
        }
    }

    fn assert_selected_event<S>(
        context: &mut VisualTestContext,
        view: &gpui_kit::Entity<CalendarView<S>>,
        expected: &str,
    ) where
        S: CalendarSession + ?Sized + 'static,
    {
        let selected = context.cx.update(|cx| {
            view.read(cx)
                .selected_agenda_event
                .as_ref()
                .map(|key| key.event_id.as_str().to_owned())
        });
        assert_eq!(selected.as_deref(), Some(expected));
    }
}
