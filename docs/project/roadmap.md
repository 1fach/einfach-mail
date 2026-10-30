# Roadmap

Milestones describe outcomes. Keep individual tasks in issues once a Git remote is configured. Reorder work only after updating `docs/project/status.md` and recording any changed architectural choice.

## M0: Repository foundation

- [x] Create the Rust workspace and native GPUI Kit shell.
- [x] Define product scope, module ownership, agent workflow, and CI gates.
- [x] Research Microsoft Graph and record provider constraints.
- [x] Research GPUI Kit and record platform and optional WebView limits.
- [x] Establish a Rust dependency review process and research likely candidates.
- [x] Initialize Git metadata and review the resolved Cargo lock graph.
- [ ] Build and launch the shell on Windows and the current Linux development target.

## M1: Identity and local data design

- [ ] Prove system-browser sign-in with authorization code and PKCE for supported account types.
- [ ] Test MFA, Conditional Access, cancellation, token refresh, and sign-out.
- [ ] Choose the Rust identity library or platform broker and record the decision.
- [ ] Select local storage, schema migration, encryption-at-rest, and key recovery behavior.
- [ ] Verify that tokens never enter GPUI view state, application logs, or local mail data.

## M2: Read-only mail

- [ ] Connect one supported account and show its folders.
- [ ] Fetch and display a paginated inbox with sender, subject, date, and unread state.
- [ ] Read message content with remote content blocked by default.
- [ ] Choose and review maintained HTML parser, sanitizer, and rendering libraries before implementing rich message content.
- [ ] Synchronize changes incrementally and recover safely after restart or network failure.
- [ ] Add contract fixtures for Graph pagination, delta links, deleted messages, throttling, and invalid tokens.

## M3: Read-only calendar

- [ ] Show a date-range calendar view with correct time-zone handling.
- [ ] Handle recurring events and exceptions in the supported range.
- [ ] Synchronize a fixed calendar view incrementally and handle expired sync state.
- [ ] Add contract fixtures for daylight-saving transitions and recurring events.

## M4: Mail and calendar actions

- [ ] Mark, move, and delete messages with visible pending and confirmed states.
- [ ] Compose and send messages with attachment-size and failure handling.
- [ ] Create and update events with attendees, reminders, and time zones.
- [ ] Queue only actions with a defined retry and conflict policy.
- [ ] Request write permissions only when the user enables the matching feature.

## M5: Windows daily-use integration

- [ ] Add system notifications with user-controlled settings.
- [ ] Add taskbar unread state and calendar reminders if they improve the main workflow.
- [ ] Test installer upgrade, uninstall, and account removal.
- [ ] Complete accessibility and keyboard-flow review.

## M6: Broader release targets

- [ ] Evaluate macOS and Linux parity using their native UI backends and credential stores.
- [ ] Decide whether personal Microsoft accounts and multiple accounts are release requirements.
- [ ] Define support, telemetry, privacy, and release/update policies.
