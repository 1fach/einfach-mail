//! Std-blocking Microsoft Graph client: wraps a `Read + Write` stream
//! plus the bearer credential and runs the coroutines against
//! `graph.microsoft.com`. Gated behind the `client` feature.

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use core::time::Duration;
use core::{any::Any, error::Error, fmt};

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use std::io::{self, Read, Write};

use io_http::rfc6750::bearer::HttpAuthBearer;
/// TLS backend selection re-exported from pimalaya-stream, feeding
/// [`MsgraphClientStdConnectOptions::tls`].
#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
pub use pimalaya_stream::tls::*;
#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use pimalaya_stream::{
    proxy::Proxy,
    stream::{Stream, TcpConnectOptions, TlsConnectOptions},
};
#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use url::Url;

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use crate::v1::send::MSGRAPH_API_BASE;
use crate::{
    coroutine::*,
    v1::rest::batch::{MsgraphBatch, MsgraphBatchRequest, MsgraphBatchResponses},
    v1::rest::users::{
        MsgraphUser,
        calendars::{
            MsgraphCalendar,
            create::MsgraphCalendarCreate,
            delete::MsgraphCalendarDelete,
            get::MsgraphCalendarGet,
            list::{
                MsgraphCalendarsList, MsgraphCalendarsListParams, MsgraphCalendarsListResponse,
            },
            update::MsgraphCalendarUpdate,
        },
        contact_folders::{
            MsgraphContactFolder,
            child_folders::MsgraphContactChildFoldersList,
            create::MsgraphContactFolderCreate,
            delete::MsgraphContactFolderDelete,
            get::MsgraphContactFolderGet,
            list::{
                MsgraphContactFoldersList, MsgraphContactFoldersListParams,
                MsgraphContactFoldersListResponse,
            },
            update::MsgraphContactFolderUpdate,
        },
        contacts::{
            MsgraphContact,
            create::MsgraphContactCreate,
            delete::MsgraphContactDelete,
            delta::{MsgraphContactsDelta, MsgraphContactsDeltaResponse},
            get::MsgraphContactGet,
            list::{MsgraphContactsList, MsgraphContactsListParams, MsgraphContactsListResponse},
            update::MsgraphContactUpdate,
        },
        events::{
            MsgraphEvent,
            calendar_view::MsgraphCalendarView,
            create::MsgraphEventCreate,
            delete::MsgraphEventDelete,
            delta::{MsgraphEventsDelta, MsgraphEventsDeltaResponse},
            get::MsgraphEventGet,
            instances::MsgraphEventInstances,
            list::{MsgraphEventsList, MsgraphEventsListParams, MsgraphEventsListResponse},
            update::MsgraphEventUpdate,
        },
        get::MsgraphUserGet,
        mail_folders::{
            MsgraphMailFolder,
            child_folders::MsgraphMailChildFoldersList,
            copy::MsgraphMailFolderCopy,
            create::MsgraphMailFolderCreate,
            delete::MsgraphMailFolderDelete,
            get::MsgraphMailFolderGet,
            list::{
                MsgraphMailFoldersList, MsgraphMailFoldersListParams,
                MsgraphMailFoldersListResponse,
            },
            r#move::MsgraphMailFolderMove,
            update::MsgraphMailFolderUpdate,
        },
        messages::{
            MsgraphMessage,
            attachments::{
                MsgraphAttachment,
                create::MsgraphAttachmentCreate,
                delete::MsgraphAttachmentDelete,
                get_raw::MsgraphAttachmentGetRaw,
                list::{MsgraphAttachmentsList, MsgraphAttachmentsListResponse},
            },
            copy::MsgraphMessageCopy,
            create::MsgraphMessageCreate,
            create_mime::MsgraphMessageCreateMime,
            delete::MsgraphMessageDelete,
            delta::{
                MsgraphMessagesDelta, MsgraphMessagesDeltaParams, MsgraphMessagesDeltaResponse,
            },
            get::MsgraphMessageGet,
            get_raw::MsgraphMessageGetRaw,
            list::{MsgraphMessagesList, MsgraphMessagesListParams, MsgraphMessagesListResponse},
            r#move::MsgraphMessageMove,
            send::MsgraphMessageSend,
            update::MsgraphMessageUpdate,
        },
        send_mail::{MsgraphMailSend, MsgraphMailSendMime},
        todo::{
            lists::{
                MsgraphTodoList,
                create::MsgraphTodoListCreate,
                delete::MsgraphTodoListDelete,
                delta::{MsgraphTodoListsDelta, MsgraphTodoListsDeltaResponse},
                get::MsgraphTodoListGet,
                list::{
                    MsgraphTodoListsList, MsgraphTodoListsListParams, MsgraphTodoListsListResponse,
                },
                update::MsgraphTodoListUpdate,
            },
            tasks::{
                MsgraphTodoTask,
                checklist_items::list::{
                    MsgraphChecklistItemsList, MsgraphChecklistItemsListResponse,
                },
                create::MsgraphTodoTaskCreate,
                delete::MsgraphTodoTaskDelete,
                delta::{MsgraphTodoTasksDelta, MsgraphTodoTasksDeltaResponse},
                get::MsgraphTodoTaskGet,
                list::{
                    MsgraphTodoTasksList, MsgraphTodoTasksListParams, MsgraphTodoTasksListResponse,
                },
                update::MsgraphTodoTaskUpdate,
            },
        },
    },
    v1::send::{MsgraphNoResponse, MsgraphSendError, MsgraphSendOutput},
};

/// Error returned by [`MsgraphClientStd`] operations.
#[derive(Debug)]
pub enum MsgraphClientStdError {
    /// A coroutine completed with an error.
    Send(MsgraphSendError),
    /// Reading from or writing to the stream failed.
    Io(io::Error),
    /// Opening the TCP/TLS connection failed.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    Tls(anyhow::Error),
    /// The API base URL has no host to connect to.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    UrlMissingHost(String),
    /// The API base URL scheme is neither http nor https.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    UrlUnsupportedScheme {
        /// The rejected API base URL.
        url: String,
        /// The scheme of the rejected URL.
        scheme: String,
    },
}

impl fmt::Display for MsgraphClientStdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Send(err) => err.fmt(f),
            Self::Io(err) => err.fmt(f),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            Self::Tls(err) => err.fmt(f),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            Self::UrlMissingHost(url) => write!(f, "Microsoft Graph URL `{url}` has no host"),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            Self::UrlUnsupportedScheme { url, scheme } => write!(
                f,
                "Microsoft Graph URL `{url}` has unsupported scheme `{scheme}` (expected `http` or `https`)"
            ),
        }
    }
}

impl Error for MsgraphClientStdError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Send(err) => err.source(),
            Self::Io(err) => err.source(),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            Self::Tls(err) => err.source(),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            _ => None,
        }
    }
}

impl From<MsgraphSendError> for MsgraphClientStdError {
    fn from(err: MsgraphSendError) -> Self {
        Self::Send(err)
    }
}

impl From<io::Error> for MsgraphClientStdError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
impl From<anyhow::Error> for MsgraphClientStdError {
    fn from(err: anyhow::Error) -> Self {
        Self::Tls(err)
    }
}

/// Optional settings for [`MsgraphClientStd::connect`]; every field has a
/// default (the TLS backend default, and `me` as the mailbox owner).
pub struct MsgraphClientStdConnectOptions {
    /// The TLS backend configuration used to open the connection.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    pub tls: Tls,
    /// How the connection reaches the API: [`Proxy::System`] resolves it
    /// from the environment, [`Proxy::None`] connects directly.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    pub proxy: Proxy,
    /// The mailbox owner: `me`, a user id or a principal name.
    pub user_id: String,
}

impl Default for MsgraphClientStdConnectOptions {
    fn default() -> Self {
        Self {
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            tls: Tls::default(),
            #[cfg(any(
                feature = "rustls-aws",
                feature = "rustls-ring",
                feature = "native-tls"
            ))]
            proxy: Proxy::default(),
            user_id: String::from("me"),
        }
    }
}

const READ_BUFFER_SIZE: usize = 16 * 1024;

/// Std blocking Microsoft Graph client: a stream, the bearer
/// credential and the mailbox owner, with one method per operation.
pub struct MsgraphClientStd {
    /// The stream carrying the HTTPS connection to the Graph API.
    pub stream: Box<dyn MsgraphStream>,
    /// The bearer credential added to every request.
    pub auth: HttpAuthBearer,
    /// The mailbox owner: `me`, a user id or a principal name.
    pub user_id: String,
}

impl MsgraphClientStd {
    /// Builds a client over a caller-managed stream.
    pub fn new<S: Read + Write + Send + 'static>(
        stream: S,
        token: impl ToString,
        options: MsgraphClientStdConnectOptions,
    ) -> Self {
        Self {
            stream: Box::new(stream),
            auth: HttpAuthBearer::new(token.to_string()),
            user_id: options.user_id,
        }
    }

    /// Builds a client by opening a TCP/TLS connection to the Graph
    /// API endpoint through pimalaya-stream.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    pub fn connect(
        token: impl ToString,
        options: MsgraphClientStdConnectOptions,
    ) -> Result<Self, MsgraphClientStdError> {
        let MsgraphClientStdConnectOptions {
            tls,
            proxy,
            user_id,
        } = options;

        let url = Url::parse(MSGRAPH_API_BASE).expect("Microsoft Graph API base URL is valid");
        let host = url
            .host_str()
            .ok_or_else(|| MsgraphClientStdError::UrlMissingHost(url.to_string()))?;

        let stream = match url.scheme() {
            "http" => {
                let port = url.port().unwrap_or(80);
                let opts = TcpConnectOptions {
                    proxy,
                    ..Default::default()
                };

                Stream::connect_tcp(host, port, opts)?
            }
            "https" => {
                let port = url.port().unwrap_or(443);
                let opts = TlsConnectOptions {
                    tls: tls.clone(),
                    proxy,
                    ..Default::default()
                };

                Stream::connect_tls(host, port, opts)?
            }
            scheme => {
                return Err(MsgraphClientStdError::UrlUnsupportedScheme {
                    url: url.to_string(),
                    scheme: scheme.to_string(),
                });
            }
        };

        stream.set_read_timeout(Some(Duration::from_secs(30)))?;

        Ok(Self {
            stream: Box::new(stream),
            auth: HttpAuthBearer::new(token.to_string()),
            user_id,
        })
    }

    /// Replaces the underlying stream (e.g. after a connection reset).
    pub fn set_stream<S: Read + Write + Send + 'static>(&mut self, stream: S) {
        self.stream = Box::new(stream);
    }

    /// Runs the given coroutine to completion against the stream,
    /// fulfilling its read and write requests.
    pub fn run<C, T>(
        &mut self,
        mut coroutine: C,
    ) -> Result<MsgraphSendOutput<T>, MsgraphClientStdError>
    where
        C: MsgraphCoroutine<
                Yield = MsgraphYield,
                Return = Result<MsgraphSendOutput<T>, MsgraphSendError>,
            >,
    {
        let mut buf = [0u8; READ_BUFFER_SIZE];
        let mut arg: Option<&[u8]> = None;

        loop {
            match coroutine.resume(arg.take()) {
                MsgraphCoroutineState::Complete(Ok(out)) => return Ok(out),
                MsgraphCoroutineState::Complete(Err(err)) => return Err(err.into()),
                MsgraphCoroutineState::Yielded(MsgraphYield::WantsRead) => {
                    let n = self.stream.read(&mut buf)?;
                    arg = Some(&buf[..n]);
                }
                MsgraphCoroutineState::Yielded(MsgraphYield::WantsWrite(bytes)) => {
                    self.stream.write_all(&bytes)?;
                    arg = None;
                }
            }
        }
    }

    /// Sends several requests in one HTTP call.
    pub fn batch(
        &mut self,
        requests: &[MsgraphBatchRequest],
    ) -> Result<MsgraphSendOutput<MsgraphBatchResponses>, MsgraphClientStdError> {
        let coroutine = MsgraphBatch::new(&self.auth, requests)?;
        self.run(coroutine)
    }

    /// Gets the profile of the mailbox owner.
    pub fn me(&mut self) -> Result<MsgraphSendOutput<MsgraphUser>, MsgraphClientStdError> {
        let coroutine = MsgraphUserGet::new(&self.auth, &self.user_id)?;
        self.run(coroutine)
    }

    /// Lists the mail folders of the mailbox.
    pub fn mail_folders_list(
        &mut self,
        params: &MsgraphMailFoldersListParams,
    ) -> Result<MsgraphSendOutput<MsgraphMailFoldersListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFoldersList::new(&self.auth, &self.user_id, params)?;
        self.run(coroutine)
    }

    /// Gets a mail folder by id.
    pub fn mail_folder_get(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderGet::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Creates a mail folder.
    pub fn mail_folder_create(
        &mut self,
        folder: &MsgraphMailFolder,
    ) -> Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderCreate::new(&self.auth, &self.user_id, folder)?;
        self.run(coroutine)
    }

    /// Updates a mail folder by id.
    pub fn mail_folder_update(
        &mut self,
        id: &str,
        folder: &MsgraphMailFolder,
    ) -> Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderUpdate::new(&self.auth, &self.user_id, id, folder)?;
        self.run(coroutine)
    }

    /// Deletes a mail folder by id.
    pub fn mail_folder_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Copies a mail folder into a destination folder.
    pub fn mail_folder_copy(
        &mut self,
        id: &str,
        destination: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderCopy::new(&self.auth, &self.user_id, id, destination)?;
        self.run(coroutine)
    }

    /// Moves a mail folder into a destination folder.
    pub fn mail_folder_move(
        &mut self,
        id: &str,
        destination: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphMailFolderMove::new(&self.auth, &self.user_id, id, destination)?;
        self.run(coroutine)
    }

    /// Lists the child folders of a mail folder.
    pub fn mail_child_folders_list(
        &mut self,
        id: &str,
        params: &MsgraphMailFoldersListParams,
    ) -> Result<MsgraphSendOutput<MsgraphMailFoldersListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMailChildFoldersList::new(&self.auth, &self.user_id, id, params)?;
        self.run(coroutine)
    }

    /// Lists the contact folders of the mailbox.
    pub fn contact_folders_list(
        &mut self,
        params: &MsgraphContactFoldersListParams,
    ) -> Result<MsgraphSendOutput<MsgraphContactFoldersListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactFoldersList::new(&self.auth, &self.user_id, params)?;
        self.run(coroutine)
    }

    /// Gets a contact folder by id.
    pub fn contact_folder_get(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphContactFolderGet::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Creates a contact folder.
    pub fn contact_folder_create(
        &mut self,
        folder: &MsgraphContactFolder,
    ) -> Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphContactFolderCreate::new(&self.auth, &self.user_id, folder)?;
        self.run(coroutine)
    }

    /// Updates a contact folder by id.
    pub fn contact_folder_update(
        &mut self,
        id: &str,
        folder: &MsgraphContactFolder,
    ) -> Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphClientStdError> {
        let coroutine = MsgraphContactFolderUpdate::new(&self.auth, &self.user_id, id, folder)?;
        self.run(coroutine)
    }

    /// Deletes a contact folder by id.
    pub fn contact_folder_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactFolderDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Lists the child folders of a contact folder.
    pub fn contact_child_folders_list(
        &mut self,
        id: &str,
        params: &MsgraphContactFoldersListParams,
    ) -> Result<MsgraphSendOutput<MsgraphContactFoldersListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactChildFoldersList::new(&self.auth, &self.user_id, id, params)?;
        self.run(coroutine)
    }

    /// Lists the contacts of the default Contacts folder, or of the
    /// given contact folder.
    pub fn contacts_list(
        &mut self,
        folder: Option<&str>,
        params: &MsgraphContactsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphContactsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactsList::new(&self.auth, &self.user_id, folder, params)?;
        self.run(coroutine)
    }

    /// Gets a contact by id, optionally expanding the given relations.
    pub fn contact_get(
        &mut self,
        id: &str,
        expand: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphContact>, MsgraphClientStdError> {
        let coroutine = MsgraphContactGet::new(&self.auth, &self.user_id, id, expand)?;
        self.run(coroutine)
    }

    /// Creates a contact in the default Contacts folder, or in the
    /// given contact folder.
    pub fn contact_create(
        &mut self,
        folder: Option<&str>,
        contact: &MsgraphContact,
    ) -> Result<MsgraphSendOutput<MsgraphContact>, MsgraphClientStdError> {
        let coroutine = MsgraphContactCreate::new(&self.auth, &self.user_id, folder, contact)?;
        self.run(coroutine)
    }

    /// Updates a contact by id.
    pub fn contact_update(
        &mut self,
        id: &str,
        contact: &MsgraphContact,
    ) -> Result<MsgraphSendOutput<MsgraphContact>, MsgraphClientStdError> {
        let coroutine = MsgraphContactUpdate::new(&self.auth, &self.user_id, id, contact)?;
        self.run(coroutine)
    }

    /// Deletes a contact by id.
    pub fn contact_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Starts a contacts delta round over the default Contacts folder,
    /// or over the given contact folder.
    pub fn contacts_delta(
        &mut self,
        folder: Option<&str>,
        select: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphContactsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactsDelta::new(&self.auth, &self.user_id, folder, select)?;
        self.run(coroutine)
    }

    /// Continues a contacts delta round from an `@odata.nextLink`, or
    /// starts the next round from a saved `@odata.deltaLink`.
    pub fn contacts_delta_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphContactsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphContactsDelta::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Starts a contacts delta round like
    /// [`contacts_delta`](Self::contacts_delta), asking for at most
    /// `max_page_size` contacts per page when given.
    pub fn contacts_delta_with_page_size(
        &mut self,
        folder: Option<&str>,
        select: Option<&str>,
        max_page_size: Option<u32>,
    ) -> Result<MsgraphSendOutput<MsgraphContactsDeltaResponse>, MsgraphClientStdError> {
        let mut coroutine = MsgraphContactsDelta::new(&self.auth, &self.user_id, folder, select)?;
        if let Some(size) = max_page_size {
            coroutine = coroutine.max_page_size(size);
        }
        self.run(coroutine)
    }

    /// Follows a contacts delta link like
    /// [`contacts_delta_from_link`](Self::contacts_delta_from_link),
    /// asking for at most `max_page_size` items per page when given.
    pub fn contacts_delta_from_link_with_page_size(
        &mut self,
        link: &str,
        max_page_size: Option<u32>,
    ) -> Result<MsgraphSendOutput<MsgraphContactsDeltaResponse>, MsgraphClientStdError> {
        let mut coroutine = MsgraphContactsDelta::from_link(&self.auth, link)?;
        if let Some(size) = max_page_size {
            coroutine = coroutine.max_page_size(size);
        }
        self.run(coroutine)
    }

    /// Lists the user's calendars.
    pub fn calendars_list(
        &mut self,
        params: &MsgraphCalendarsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphCalendarsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphCalendarsList::new(&self.auth, &self.user_id, params)?;
        self.run(coroutine)
    }

    /// Gets the calendar `id`.
    pub fn calendar_get(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphClientStdError> {
        let coroutine = MsgraphCalendarGet::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Creates a calendar.
    pub fn calendar_create(
        &mut self,
        calendar: &MsgraphCalendar,
    ) -> Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphClientStdError> {
        let coroutine = MsgraphCalendarCreate::new(&self.auth, &self.user_id, calendar)?;
        self.run(coroutine)
    }

    /// Patches the calendar `id`.
    pub fn calendar_update(
        &mut self,
        id: &str,
        calendar: &MsgraphCalendar,
    ) -> Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphClientStdError> {
        let coroutine = MsgraphCalendarUpdate::new(&self.auth, &self.user_id, id, calendar)?;
        self.run(coroutine)
    }

    /// Deletes the calendar `id`.
    pub fn calendar_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphCalendarDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Lists the lone events and series masters of a calendar, the default
    /// one when `calendar` is `None`.
    pub fn events_list(
        &mut self,
        calendar: Option<&str>,
        params: &MsgraphEventsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphEventsList::new(&self.auth, &self.user_id, calendar, params)?;
        self.run(coroutine)
    }

    /// Continues an events listing from an `@odata.nextLink`.
    pub fn events_list_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphEventsList::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Gets the event `id`, selecting `select` and expanding `expand` when
    /// given.
    pub fn event_get(
        &mut self,
        id: &str,
        select: Option<&str>,
        expand: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphEvent>, MsgraphClientStdError> {
        let coroutine = MsgraphEventGet::new(&self.auth, &self.user_id, id, select, expand)?;
        self.run(coroutine)
    }

    /// Creates an event in a calendar, the default one when `calendar` is
    /// `None`.
    pub fn event_create(
        &mut self,
        calendar: Option<&str>,
        event: &MsgraphEvent,
    ) -> Result<MsgraphSendOutput<MsgraphEvent>, MsgraphClientStdError> {
        let coroutine = MsgraphEventCreate::new(&self.auth, &self.user_id, calendar, event)?;
        self.run(coroutine)
    }

    /// Patches the event `id`.
    pub fn event_update(
        &mut self,
        id: &str,
        event: &MsgraphEvent,
    ) -> Result<MsgraphSendOutput<MsgraphEvent>, MsgraphClientStdError> {
        let coroutine = MsgraphEventUpdate::new(&self.auth, &self.user_id, id, event)?;
        self.run(coroutine)
    }

    /// Deletes the event `id`, the whole series for a series master.
    pub fn event_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphEventDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Lists the instances of the series master `id` within a window.
    pub fn event_instances(
        &mut self,
        id: &str,
        start: &str,
        end: &str,
        params: &MsgraphEventsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphEventInstances::new(&self.auth, &self.user_id, id, start, end, params)?;
        self.run(coroutine)
    }

    /// Lists a calendar view, every event expanded within a window.
    pub fn calendar_view(
        &mut self,
        calendar: Option<&str>,
        start: &str,
        end: &str,
        params: &MsgraphEventsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphCalendarView::new(&self.auth, &self.user_id, calendar, start, end, params)?;
        self.run(coroutine)
    }

    /// Starts a calendar view delta round over a window.
    pub fn events_delta(
        &mut self,
        calendar: Option<&str>,
        start: &str,
        end: &str,
    ) -> Result<MsgraphSendOutput<MsgraphEventsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphEventsDelta::new(&self.auth, &self.user_id, calendar, start, end)?;
        self.run(coroutine)
    }

    /// Continues or restarts a calendar view delta round from a link.
    pub fn events_delta_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphEventsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphEventsDelta::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Starts a calendar view delta round like
    /// [`events_delta`](Self::events_delta), asking for at most
    /// `max_page_size` events per page when given.
    pub fn events_delta_with_page_size(
        &mut self,
        calendar: Option<&str>,
        start: &str,
        end: &str,
        max_page_size: Option<u32>,
    ) -> Result<MsgraphSendOutput<MsgraphEventsDeltaResponse>, MsgraphClientStdError> {
        let mut coroutine =
            MsgraphEventsDelta::new(&self.auth, &self.user_id, calendar, start, end)?;
        if let Some(size) = max_page_size {
            coroutine = coroutine.max_page_size(size);
        }
        self.run(coroutine)
    }

    /// Follows a calendar view delta link like
    /// [`events_delta_from_link`](Self::events_delta_from_link),
    /// asking for at most `max_page_size` items per page when given.
    pub fn events_delta_from_link_with_page_size(
        &mut self,
        link: &str,
        max_page_size: Option<u32>,
    ) -> Result<MsgraphSendOutput<MsgraphEventsDeltaResponse>, MsgraphClientStdError> {
        let mut coroutine = MsgraphEventsDelta::from_link(&self.auth, link)?;
        if let Some(size) = max_page_size {
            coroutine = coroutine.max_page_size(size);
        }
        self.run(coroutine)
    }

    /// Lists the user's To Do task lists.
    pub fn todo_lists_list(
        &mut self,
        params: &MsgraphTodoListsListParams,
    ) -> Result<MsgraphSendOutput<MsgraphTodoListsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListsList::new(&self.auth, &self.user_id, params)?;
        self.run(coroutine)
    }

    /// Continues a task lists listing from an `@odata.nextLink`.
    pub fn todo_lists_list_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphTodoListsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListsList::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Gets the task list `id`.
    pub fn todo_list_get(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListGet::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Creates a task list.
    pub fn todo_list_create(
        &mut self,
        list: &MsgraphTodoList,
    ) -> Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListCreate::new(&self.auth, &self.user_id, list)?;
        self.run(coroutine)
    }

    /// Patches the task list `id`.
    pub fn todo_list_update(
        &mut self,
        id: &str,
        list: &MsgraphTodoList,
    ) -> Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListUpdate::new(&self.auth, &self.user_id, id, list)?;
        self.run(coroutine)
    }

    /// Deletes the task list `id` and its tasks.
    pub fn todo_list_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Starts a task lists delta round.
    pub fn todo_lists_delta(
        &mut self,
    ) -> Result<MsgraphSendOutput<MsgraphTodoListsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListsDelta::new(&self.auth, &self.user_id)?;
        self.run(coroutine)
    }

    /// Continues or restarts a task lists delta round from a link.
    pub fn todo_lists_delta_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphTodoListsDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoListsDelta::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Lists the tasks of the list `list_id`.
    pub fn todo_tasks_list(
        &mut self,
        list_id: &str,
        params: &MsgraphTodoTasksListParams,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTasksListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTasksList::new(&self.auth, &self.user_id, list_id, params)?;
        self.run(coroutine)
    }

    /// Continues a tasks listing from an `@odata.nextLink`.
    pub fn todo_tasks_list_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTasksListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTasksList::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Gets the task `id` of the list `list_id`, selecting `select` and
    /// expanding `expand` when given.
    pub fn todo_task_get(
        &mut self,
        list_id: &str,
        id: &str,
        select: Option<&str>,
        expand: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphClientStdError> {
        let coroutine =
            MsgraphTodoTaskGet::new(&self.auth, &self.user_id, list_id, id, select, expand)?;
        self.run(coroutine)
    }

    /// Creates a task in the list `list_id`.
    pub fn todo_task_create(
        &mut self,
        list_id: &str,
        task: &MsgraphTodoTask,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTaskCreate::new(&self.auth, &self.user_id, list_id, task)?;
        self.run(coroutine)
    }

    /// Patches the task `id` of the list `list_id`.
    pub fn todo_task_update(
        &mut self,
        list_id: &str,
        id: &str,
        task: &MsgraphTodoTask,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTaskUpdate::new(&self.auth, &self.user_id, list_id, id, task)?;
        self.run(coroutine)
    }

    /// Deletes the task `id` of the list `list_id`.
    pub fn todo_task_delete(
        &mut self,
        list_id: &str,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTaskDelete::new(&self.auth, &self.user_id, list_id, id)?;
        self.run(coroutine)
    }

    /// Starts a delta round over the tasks of the list `list_id`, trimmed
    /// to `select` when given.
    pub fn todo_tasks_delta(
        &mut self,
        list_id: &str,
        select: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTasksDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTasksDelta::new(&self.auth, &self.user_id, list_id, select)?;
        self.run(coroutine)
    }

    /// Continues or restarts a tasks delta round from a link.
    pub fn todo_tasks_delta_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphTodoTasksDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphTodoTasksDelta::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Lists the checklist items of the task `task_id` of the list
    /// `list_id`.
    pub fn checklist_items_list(
        &mut self,
        list_id: &str,
        task_id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphChecklistItemsListResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphChecklistItemsList::new(&self.auth, &self.user_id, list_id, task_id)?;
        self.run(coroutine)
    }

    /// Lists the messages of the whole mailbox, or of the given mail
    /// folder.
    pub fn messages_list(
        &mut self,
        folder: Option<&str>,
        params: &MsgraphMessagesListParams,
    ) -> Result<MsgraphSendOutput<MsgraphMessagesListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMessagesList::new(&self.auth, &self.user_id, folder, params)?;
        self.run(coroutine)
    }

    /// Gets a message by id.
    pub fn message_get(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageGet::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Gets the raw RFC 5322 MIME content of a message by id.
    pub fn message_get_raw(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<Vec<u8>>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageGetRaw::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Creates a draft message from JSON in the Drafts folder, or in
    /// the given mail folder.
    pub fn message_create(
        &mut self,
        folder: Option<&str>,
        message: &MsgraphMessage,
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageCreate::new(&self.auth, &self.user_id, folder, message)?;
        self.run(coroutine)
    }

    /// Creates a draft message from raw RFC 5322 MIME bytes in the
    /// Drafts folder, or in the given mail folder.
    pub fn message_create_mime(
        &mut self,
        folder: Option<&str>,
        raw: &[u8],
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageCreateMime::new(&self.auth, &self.user_id, folder, raw)?;
        self.run(coroutine)
    }

    /// Updates a message by id.
    pub fn message_update(
        &mut self,
        id: &str,
        message: &MsgraphMessage,
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageUpdate::new(&self.auth, &self.user_id, id, message)?;
        self.run(coroutine)
    }

    /// Deletes a message by id.
    pub fn message_delete(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageDelete::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Moves a message into a destination folder.
    pub fn message_move(
        &mut self,
        id: &str,
        destination: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageMove::new(&self.auth, &self.user_id, id, destination)?;
        self.run(coroutine)
    }

    /// Copies a message into a destination folder.
    pub fn message_copy(
        &mut self,
        id: &str,
        destination: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMessage>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageCopy::new(&self.auth, &self.user_id, id, destination)?;
        self.run(coroutine)
    }

    /// Starts a messages delta round over the whole mailbox, or over
    /// the given mail folder.
    pub fn messages_delta(
        &mut self,
        folder: Option<&str>,
        select: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphMessagesDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMessagesDelta::new(&self.auth, &self.user_id, folder, select)?;
        self.run(coroutine)
    }

    /// Continues a messages delta round from an `@odata.nextLink`, or
    /// starts the next round from a saved `@odata.deltaLink`.
    pub fn messages_delta_from_link(
        &mut self,
        link: &str,
    ) -> Result<MsgraphSendOutput<MsgraphMessagesDeltaResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMessagesDelta::from_link(&self.auth, link)?;
        self.run(coroutine)
    }

    /// Starts a messages delta round with the OData options and page
    /// size of `params`.
    pub fn messages_delta_with_params(
        &mut self,
        folder: Option<&str>,
        params: &MsgraphMessagesDeltaParams,
    ) -> Result<MsgraphSendOutput<MsgraphMessagesDeltaResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphMessagesDelta::with_params(&self.auth, &self.user_id, folder, params)?;
        self.run(coroutine)
    }

    /// Follows a messages delta link like
    /// [`messages_delta_from_link`](Self::messages_delta_from_link),
    /// asking for at most `max_page_size` messages per page when given.
    pub fn messages_delta_from_link_with_page_size(
        &mut self,
        link: &str,
        max_page_size: Option<u32>,
    ) -> Result<MsgraphSendOutput<MsgraphMessagesDeltaResponse>, MsgraphClientStdError> {
        let mut coroutine = MsgraphMessagesDelta::from_link(&self.auth, link)?;
        if let Some(size) = max_page_size {
            coroutine = coroutine.max_page_size(size);
        }
        self.run(coroutine)
    }

    /// Creates a file attachment on a message.
    pub fn attachment_create(
        &mut self,
        message_id: &str,
        name: &str,
        content: &[u8],
        content_type: Option<&str>,
    ) -> Result<MsgraphSendOutput<MsgraphAttachment>, MsgraphClientStdError> {
        let coroutine = MsgraphAttachmentCreate::new(
            &self.auth,
            &self.user_id,
            message_id,
            name,
            content,
            content_type,
        )?;
        self.run(coroutine)
    }

    /// Lists the attachments of a message.
    pub fn attachments_list(
        &mut self,
        message_id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphAttachmentsListResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphAttachmentsList::new(&self.auth, &self.user_id, message_id)?;
        self.run(coroutine)
    }

    /// Gets the raw content of an attachment.
    pub fn attachment_get_raw(
        &mut self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<MsgraphSendOutput<Vec<u8>>, MsgraphClientStdError> {
        let coroutine =
            MsgraphAttachmentGetRaw::new(&self.auth, &self.user_id, message_id, attachment_id)?;
        self.run(coroutine)
    }

    /// Deletes an attachment of a message.
    pub fn attachment_delete(
        &mut self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphAttachmentDelete::new(&self.auth, &self.user_id, message_id, attachment_id)?;
        self.run(coroutine)
    }

    /// Sends an existing draft message by id.
    pub fn message_send(
        &mut self,
        id: &str,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMessageSend::new(&self.auth, &self.user_id, id)?;
        self.run(coroutine)
    }

    /// Sends a message described as JSON through the sendMail action.
    pub fn mail_send(
        &mut self,
        message: &MsgraphMessage,
        save_to_sent_items: bool,
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine =
            MsgraphMailSend::new(&self.auth, &self.user_id, message, save_to_sent_items)?;
        self.run(coroutine)
    }

    /// Sends a message given as raw RFC 5322 MIME bytes through the
    /// sendMail action.
    pub fn mail_send_mime(
        &mut self,
        raw: &[u8],
    ) -> Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphClientStdError> {
        let coroutine = MsgraphMailSendMime::new(&self.auth, &self.user_id, raw)?;
        self.run(coroutine)
    }
}

impl fmt::Debug for MsgraphClientStd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MsgraphClientStd")
            .field("auth", &self.auth)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// Blocking stream the client runs over, downcastable through `Any`
/// (e.g. to recover a concrete TLS stream).
pub trait MsgraphStream: Read + Write + Send + Any {
    /// The stream as a mutable `Any`, ready for downcasting.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Read + Write + Send + Any> MsgraphStream for T {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
