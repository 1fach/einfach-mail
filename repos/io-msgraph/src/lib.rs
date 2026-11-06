#![no_std]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # io-msgraph
//!
//! I/O-free coroutines for the [Microsoft Graph API], built on
//! [io-http] (HTTP/1.1) and pumped by any stream the caller owns.
//!
//! [Microsoft Graph API]: https://learn.microsoft.com/en-us/graph/api/overview
//! [io-http]: https://docs.rs/io-http
//!
//! io-msgraph is the Microsoft Graph sibling of [io-gmail]: same shape
//! (JSON over HTTP), different vendor. Unlike io-gmail, which is scoped
//! to Gmail's mail API, io-msgraph represents the whole Microsoft Graph
//! API: the mail, contacts, calendar and To Do surfaces are covered
//! today, and sibling resources will be added under the same tree over
//! time.
//!
//! [io-gmail]: https://docs.rs/io-gmail
//!
//! ## Layers and features
//!
//! The crate has two of the three standard Pimalaya layers; there is no
//! CLI. The always-present no_std core holds the I/O-free coroutines,
//! the whole Microsoft Graph REST logic. The `client` feature adds
//! [`v1::client::MsgraphClientStd`], a std blocking client over any
//! stream, whose `connect` constructor opens the TCP/TLS connection
//! itself behind a TLS feature (`rustls-ring` by default, `rustls-aws`,
//! `native-tls`).
//!
//! ## Everything lives under v1
//!
//! The Microsoft Graph API is versioned (`/v1.0/`), so the crate is
//! too: the version-agnostic [`coroutine`] contract stays at the crate
//! root, everything else lives under [`v1`]. The day a breaking Graph
//! version ships, a sibling module slots in without breaking `v1`
//! consumers.
//!
//! ## The coroutine contract
//!
//! Every exchange implements [`coroutine::MsgraphCoroutine`]: `resume`
//! takes the bytes read since the last yield and either requests I/O
//! ([`coroutine::MsgraphYield`] `WantsRead` / `WantsWrite`) or
//! completes. The [`msgraph_try!`] macro is the coroutine equivalent
//! of `?`.
//!
//! A Graph call is a single HTTP request/response, so every REST
//! coroutine is a thin wrapper around one shared primitive,
//! [`v1::send::MsgraphSend`]: it builds the authorized request (bearer
//! token, JSON in and out) and parses either the 2xx body or Graph's
//! error envelope into [`v1::send::MsgraphSendError`]. Redirects are
//! never followed. The terminal [`v1::send::MsgraphSendOutput`] carries
//! the parsed response plus a keep-alive flag so pumps can reuse the
//! connection across the many small requests a Graph session makes.
//! Empty 2xx bodies (DELETE, sendMail, send draft) deserialize into the
//! [`v1::send::MsgraphNoResponse`] unit marker. The two `$value`
//! endpoints (raw message MIME and raw attachment content) return bytes
//! rather than JSON: they run the HTTP send directly and yield the
//! response body.
//!
//! ## Naming
//!
//! Public items follow `<Domain><Target><Verb><Ext>`: the domain is
//! `Msgraph`, the target-verb pair mirrors the REST operation
//! (`MsgraphMailFolderCreate` for creating a mail folder,
//! `MsgraphContactsList` for listing contacts) and the extension
//! distinguishes companions (`Params`, `Response`, `Error`, `Yield`).
//! Pure data resources omit the verb (`MsgraphMessage`,
//! `MsgraphContact`); the target is omitted when the verb applies to
//! the whole exchange ([`v1::send::MsgraphSend`]).
//!
//! ## Module layout
//!
//! [`v1::rest`] mirrors the Graph reference. The mail, contacts,
//! calendar and To Do surfaces hang off the users resource, so they live
//! under [`v1::rest::users`], with each sub-resource a directory and each
//! operation a file named after it: mail_folders (list, get, create,
//! update, delete, move, copy, child_folders), messages (list, get,
//! get_raw, create, create_mime, update, delete, move, copy, send,
//! delta, attachments), contact_folders (list, get, create, update,
//! delete, child_folders), contacts (list, get, create, update,
//! delete, delta), calendars (list, get, create, update, delete), events
//! (list, get, create, update, delete, instances, calendar_view, delta),
//! todo lists (list, get, create, update, delete, delta) and their tasks
//! (list, get, create, update, delete, delta, checklist_items), and the
//! sendMail action (JSON and MIME form). JSON batching lives at
//! the API root, under [`v1::rest::batch`]. A reader who knows the
//! reference knows where to look.
//!
//! Domain types mirror the Graph schema. Full-resource bodies double as
//! create and update bodies thanks to `skip_serializing_if` on every
//! optional field; contact fields use the [`v1::field::MsgraphField`]
//! tri-state to distinguish a field left out of a PATCH body from one
//! explicitly cleared. List operations take borrowed `*Params` structs
//! whose fields rename to the OData system query options (`$top`,
//! `$select`, `$filter` among others), flattened into query pairs by
//! [`v1::query::to_query_pairs`], a tiny no_std serde serializer.
//!
//! ## Authentication
//!
//! io-msgraph does no OAuth itself: the Graph API only accepts OAuth
//! 2.0 bearer tokens, so the credential is exactly a bare access token,
//! and minting or refreshing it is the caller's responsibility. The
//! base URL is fixed ([`v1::send::MSGRAPH_API_BASE`]); the mailbox
//! owner is addressed by [`v1::send::user_path`], which yields `me` for
//! the authenticated user or `users/{id}` for an explicit user id or
//! principal name (Graph rejects `users/me`).
//!
//! ## Logging
//!
//! Coroutines pair a `debug!` lifecycle line with one `trace!` per
//! input variable in `new()`, and a `debug!` plus `trace!("out: ...")`
//! when `resume` completes; the crate never logs above `debug!`.
//!
//! ## Example
//!
//! Running a coroutine against a caller-owned TLS stream:
//!
//! ```rust,no_run
//! use std::{
//!     io::{Read, Write},
//!     net::TcpStream,
//!     sync::Arc,
//! };
//!
//! use io_http::rfc6750::bearer::HttpAuthBearer;
//! use io_msgraph::{coroutine::*, v1::rest::users::get::MsgraphUserGet};
//! use rustls::{ClientConfig, ClientConnection, StreamOwned};
//! use rustls_platform_verifier::ConfigVerifierExt;
//!
//! let config = ClientConfig::with_platform_verifier().unwrap();
//! let server_name = "graph.microsoft.com".try_into().unwrap();
//! let conn = ClientConnection::new(Arc::new(config), server_name).unwrap();
//! let tcp = TcpStream::connect(("graph.microsoft.com", 443)).unwrap();
//! let mut stream = StreamOwned::new(conn, tcp);
//!
//! let auth = HttpAuthBearer::new("token");
//! let mut coroutine = MsgraphUserGet::new(&auth, "me").unwrap();
//!
//! let mut arg: Option<&[u8]> = None;
//! let mut buf = [0u8; 8192];
//! let mut read = Vec::new();
//!
//! let out = loop {
//!     match coroutine.resume(arg.take()) {
//!         MsgraphCoroutineState::Complete(Ok(out)) => break out,
//!         MsgraphCoroutineState::Complete(Err(err)) => panic!("{err}"),
//!         MsgraphCoroutineState::Yielded(MsgraphYield::WantsRead) => {
//!             let n = stream.read(&mut buf).unwrap();
//!             read.clear();
//!             read.extend_from_slice(&buf[..n]);
//!             arg = Some(&read);
//!         }
//!         MsgraphCoroutineState::Yielded(MsgraphYield::WantsWrite(bytes)) => {
//!             stream.write_all(&bytes).unwrap();
//!         }
//!     }
//! };
//!
//! println!("user principal name: {:?}", out.response.user_principal_name);
//! ```

extern crate alloc;
#[cfg(feature = "client")]
extern crate std;

pub mod coroutine;
pub mod v1;
