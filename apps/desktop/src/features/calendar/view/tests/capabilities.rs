use std::sync::Arc;

use einfach_mail_application::{CalendarWorkflow, CalendarWriteReadiness};
use einfach_mail_domain::{CalendarAction, CalendarCapabilities};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{AppContext as _, Modifiers, Role, TestAppContext};
use jiff::{civil, tz::TimeZone};

use super::super::super::composer::bind_event_composer_keys;
use super::super::{CalendarStatus, CalendarView, bind_calendar_event_navigation_keys};
use crate::test_support::calendar::{
    TestCache, TestProvider, draw_calendar_view, profile, profile_with_calendar_capabilities,
    select_default_timed_event, workflow_account_id,
};
use crate::test_support::{test_element_is_visible, wait_for_ui_condition};

#[gpui_kit::test]
async fn calendar_actions_follow_each_provider_capability(cx: &mut TestAppContext) {
    cx.update(|app| {
        gpui_kit::init(app);
        bind_calendar_event_navigation_keys(app);
        bind_event_composer_keys(app);
    });
    let provider = TestProvider::default();
    provider.allow_calendar_sync();
    let workflow = Arc::new(CalendarWorkflow::new(
        workflow_account_id(),
        provider.clone(),
        TestCache::default(),
    ));
    let week_start = civil::Date::new(2030, 1, 7).expect("synthetic week is valid");
    let mut calendar_view = None;
    let (_, context) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            CalendarView::new_for_week(
                profile(),
                workflow,
                super::support::system_calendar_week(week_start, TimeZone::UTC),
                cx,
            )
        });
        calendar_view = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let view = calendar_view.expect("calendar view is retained for capability checks");
    wait_for_ui_condition(context, |_, app| {
        view.read(app).write_readiness == CalendarWriteReadiness::Ready
            && matches!(view.read(app).status, CalendarStatus::UpToDate)
    })
    .await;
    select_default_timed_event(context);

    let profiles = [
        (
            "unavailable",
            CalendarCapabilities::unavailable(),
            false,
            None,
        ),
        ("view-only", CalendarCapabilities::view_only(), false, None),
        (
            "create-only",
            CalendarCapabilities::from_actions([CalendarAction::CreateEvent]),
            true,
            None,
        ),
        (
            "update-only",
            CalendarCapabilities::from_actions([CalendarAction::UpdateEvent]),
            false,
            Some("Edit"),
        ),
        (
            "delete-only",
            CalendarCapabilities::from_actions([CalendarAction::DeleteEvent]),
            false,
            Some("Details"),
        ),
        (
            "full",
            CalendarCapabilities::full_access(),
            true,
            Some("Edit"),
        ),
    ];

    for (name, capabilities, can_create, selected_action) in profiles {
        context.cx.update(|app| {
            view.update(app, |view, _| {
                view.profile = profile_with_calendar_capabilities(capabilities);
            });
        });
        draw_calendar_view(context);
        assert_eq!(
            context.debug_bounds("calendar-new-event").is_some(),
            can_create,
            "New event visibility for {name}"
        );
        assert_eq!(
            context.debug_bounds("calendar-event-open").is_some(),
            selected_action.is_some(),
            "selected event action visibility for {name}"
        );
        if let Some(expected_label) = selected_action {
            let labels = context.update(|window, _| {
                window
                    .find_all_by_role(Role::Button)
                    .iter()
                    .filter_map(|button| button.label().map(str::to_owned))
                    .collect::<Vec<_>>()
            });
            assert!(
                labels.iter().any(|label| label == expected_label),
                "{name} exposes the {expected_label} action: {labels:?}"
            );
        }
    }

    context.cx.update(|app| {
        view.update(app, |view, _| {
            view.profile =
                profile_with_calendar_capabilities(CalendarCapabilities::from_actions([
                    CalendarAction::UpdateEvent,
                ]));
        });
    });
    draw_calendar_view(context);
    context.simulate_keystrokes("enter");
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-delete").is_none());
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("update-only event sheet has an editable title");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_keystrokes("ctrl-a");
    context.simulate_input("Updated capability event");
    let save = context
        .debug_bounds("event-composer-submit")
        .expect("update-only event sheet has Save");
    context.simulate_click(save.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider
            .event_update_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            == 1
    })
    .await;
    wait_for_ui_condition(context, |_, app| {
        view.read(app)
            .events
            .iter()
            .any(|event| event.subject() == Some("Updated capability event"))
    })
    .await;
    let close = context
        .debug_bounds("event-composer-close")
        .expect("saved event sheet offers Close");
    context.simulate_click(close.center(), Modifiers::default());
    draw_calendar_view(context);
    assert!(!context.update(|window, app| window.has_active_sheet(app)));

    context.cx.update(|app| {
        view.update(app, |view, _| {
            view.profile =
                profile_with_calendar_capabilities(CalendarCapabilities::from_actions([
                    CalendarAction::DeleteEvent,
                ]));
        });
    });
    draw_calendar_view(context);
    let details = context
        .debug_bounds("calendar-event-open")
        .expect("delete-only provider exposes event details");
    context.simulate_click(details.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-details")
    })
    .await;
    assert!(context.debug_bounds("event-composer-fields").is_none());
    assert!(context.debug_bounds("event-composer-submit").is_none());
    let detail_labels = context.update(|window, _| {
        window
            .find_all_by_role(Role::Group)
            .iter()
            .filter_map(|group| group.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(
        detail_labels
            .iter()
            .any(|label| label == "Title: Updated capability event"),
        "read-only details include the loaded event title: {detail_labels:?}"
    );
    let mut delete_focused =
        context.update(|window, _| window.find_by_label("Delete…").focused() == Some(true));
    for _ in 0..4 {
        if delete_focused {
            break;
        }
        context.simulate_keystrokes("tab");
        delete_focused =
            context.update(|window, _| window.find_by_label("Delete…").focused() == Some(true));
    }
    assert!(delete_focused, "Tab reaches the delete-only action");
    let delete = context
        .debug_bounds("event-composer-delete")
        .expect("delete-only event details expose Delete");
    context.simulate_click(delete.center(), Modifiers::default());
    let confirm = context
        .debug_bounds("event-delete-dialog-confirm")
        .expect("Delete asks for confirmation");
    context.simulate_click(confirm.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider
            .event_deletion_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            == 1
    })
    .await;
    assert_eq!(
        provider
            .event_update_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let close = context
        .debug_bounds("event-composer-close")
        .expect("deleted event details offer Close");
    context.simulate_click(close.center(), Modifiers::default());
    draw_calendar_view(context);

    context.cx.update(|app| {
        view.update(app, |view, _| {
            view.profile =
                profile_with_calendar_capabilities(CalendarCapabilities::from_actions([
                    CalendarAction::CreateEvent,
                ]));
        });
    });
    draw_calendar_view(context);
    let new_event = context
        .debug_bounds("calendar-new-event")
        .expect("create-only provider exposes New event");
    context.simulate_click(new_event.center(), Modifiers::default());
    wait_for_ui_condition(context, |window, _| {
        test_element_is_visible(window, "event-composer-fields")
    })
    .await;
    assert!(context.debug_bounds("event-composer-delete").is_none());
    let title = context
        .debug_bounds("event-composer-subject-input")
        .expect("create-only event sheet has a title field");
    context.simulate_click(title.center(), Modifiers::default());
    context.simulate_input("Created capability event");
    let create = context
        .debug_bounds("event-composer-submit")
        .expect("create-only event sheet has Create");
    context.simulate_click(create.center(), Modifiers::default());
    wait_for_ui_condition(context, |_, _| {
        provider
            .event_creation_count
            .load(std::sync::atomic::Ordering::SeqCst)
            == 1
    })
    .await;
    assert_eq!(
        provider
            .event_deletion_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
