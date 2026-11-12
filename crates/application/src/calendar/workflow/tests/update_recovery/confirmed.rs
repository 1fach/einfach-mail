use einfach_mail_domain::{CalendarDeltaProgress, CalendarEventUpdate, CalendarEventUpdateDraft};
use time::Month;

use crate::test_support::{
    FixtureCache, FixtureProvider, creation, link, response, synchronize_empty_view, utc, view,
    workflow_account_id,
};
use crate::{
    CalendarCacheError, CalendarEventCreationOutcome, CalendarEventUpdateOutcome, CalendarWorkflow,
    CalendarWorkflowError, CalendarWriteReadiness,
};

#[test]
fn event_update_confirms_and_replaces_the_cached_occurrence() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "event-update-initial-sync").await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let versioned_event = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic timed event has a provider version");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update =
            CalendarEventUpdate::new(versioned_event, draft).expect("event values changed");

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("synthetic Graph update is confirmed");
        assert!(matches!(outcome, CalendarEventUpdateOutcome::Cached(_)));
        let captured_updates = provider
            .update_calls
            .lock()
            .expect("synthetic update log mutex is not poisoned")
            .clone();
        assert_eq!(captured_updates.len(), 1);
        assert_eq!(captured_updates[0].source().occurrence().id(), event.id());
        assert_eq!(
            captured_updates[0].source().version().as_str(),
            "synthetic-version-1"
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("updated event remains cached");
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(snapshot.events()[0].subject(), Some("Updated planning"));
        assert_eq!(snapshot.events()[0].time(), update.draft().time());
    });
}

#[test]
fn event_update_reports_remote_confirmation_when_cache_commit_fails() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_update_commit(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "event-update-cache-initial-sync",
        )
        .await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("a confirmed remote edit remains a successful outcome");
        assert!(matches!(
            outcome,
            CalendarEventUpdateOutcome::ConfirmedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
                ..
            }
        ));
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the failed cache commit leaves the view readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.load_versioned_event(&event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "a local cache failure must not repeat the remote update"
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("after-update-cache-failure")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a completed sync clears the update recovery gate");
        assert!(workflow.load_versioned_event(&event).await.is_ok());
    });
}
