use super::*;

/// A `mail_messages` column that stores one field of [`MessageMetadata`].
pub(super) struct MessageColumn {
    name: &'static str,
    nullable: bool,
}

impl MessageColumn {
    const fn required(name: &'static str) -> Self {
        Self {
            name,
            nullable: false,
        }
    }

    const fn nullable(name: &'static str) -> Self {
        Self {
            name,
            nullable: true,
        }
    }

    #[cfg(test)]
    pub(super) fn name(&self) -> &'static str {
        self.name
    }
}

/// The metadata columns of `mail_messages`.
///
/// Every statement that reads, stores, or moves message metadata takes its column list from
/// this array. [`raw_message_from_row`] reads each column by name, and the store statement
/// binds each column by name, so the order of this array carries no meaning.
pub(super) const MESSAGE_COLUMNS: [MessageColumn; 10] = [
    MessageColumn::nullable("subject"),
    MessageColumn::nullable("from_display_name"),
    MessageColumn::nullable("from_address"),
    MessageColumn::nullable("sender_display_name"),
    MessageColumn::nullable("sender_address"),
    MessageColumn::required("received_seconds"),
    MessageColumn::required("received_nanoseconds"),
    MessageColumn::required("is_read"),
    MessageColumn::nullable("flag_status"),
    MessageColumn::nullable("preview"),
];

/// Returns `render(name)` for every metadata column, separated by commas.
pub(super) fn message_columns_sql(render: impl Fn(&str) -> String) -> String {
    join_columns(MESSAGE_COLUMNS.iter(), render)
}

/// Returns `render(name)` for every metadata column that can be NULL, separated by commas.
pub(super) fn nullable_message_columns_sql(render: impl Fn(&str) -> String) -> String {
    join_columns(
        MESSAGE_COLUMNS.iter().filter(|column| column.nullable),
        render,
    )
}

fn join_columns<'a>(
    columns: impl Iterator<Item = &'a MessageColumn>,
    render: impl Fn(&str) -> String,
) -> String {
    columns
        .map(|column| render(column.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Selects the identifier and metadata of one cached message.
pub(super) static READ_MESSAGE_METADATA_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT message_id, {columns}
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
        columns = message_columns_sql(str::to_owned),
    )
});

static LIST_MESSAGES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT message_id, {columns}
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2
         ORDER BY received_seconds DESC, received_nanoseconds DESC, message_id ASC",
        columns = message_columns_sql(str::to_owned),
    )
});

static READ_MESSAGE_WITH_BODY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT message_id, {columns}, body_type, body_content
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
        columns = message_columns_sql(str::to_owned),
    )
});

pub(super) fn read_messages(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<Vec<MailMessage>> {
    if !folder_exists(connection, folder)? {
        return Err(CacheFailure::InvalidData);
    }
    let mut statement = connection.prepare(&LIST_MESSAGES_SQL)?;
    let rows = statement.query_map(
        params![folder.account_id().as_str(), folder.folder_id().as_str()],
        raw_message_from_row,
    )?;
    rows.map(|row| {
        let raw = row?;
        decode_message(folder, raw)
    })
    .collect()
}

pub(super) fn read_cached_message(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<Option<(MailMessage, Option<MessageBody>)>> {
    let raw = connection
        .query_row(
            &READ_MESSAGE_WITH_BODY_SQL,
            params![
                folder.account_id().as_str(),
                folder.folder_id().as_str(),
                message_id.as_str()
            ],
            |row| {
                Ok((
                    raw_message_from_row(row)?,
                    row.get::<_, Option<String>>("body_type")?,
                    row.get::<_, Option<String>>("body_content")?,
                ))
            },
        )
        .optional()?
        .map(|(raw_message, body_type, body_content)| {
            let message = decode_message(folder, raw_message)?;
            let body = decode_body(body_type, body_content)?;
            Ok::<_, CacheFailure>((message, body))
        })
        .transpose()?;
    Ok(raw)
}

pub(super) fn raw_message_from_row(row: &async_sqlite::rusqlite::Row<'_>) -> SqlResult<RawMessage> {
    Ok(RawMessage {
        message_id: row.get("message_id")?,
        subject: row.get("subject")?,
        from_display_name: row.get("from_display_name")?,
        from_address: row.get("from_address")?,
        sender_display_name: row.get("sender_display_name")?,
        sender_address: row.get("sender_address")?,
        received_seconds: row.get("received_seconds")?,
        received_nanoseconds: row.get("received_nanoseconds")?,
        is_read: row.get("is_read")?,
        flag_status: row.get("flag_status")?,
        preview: row.get("preview")?,
    })
}

pub(super) struct RawMessage {
    message_id: String,
    subject: Option<String>,
    from_display_name: Option<String>,
    from_address: Option<String>,
    sender_display_name: Option<String>,
    sender_address: Option<String>,
    received_seconds: i64,
    received_nanoseconds: i64,
    is_read: bool,
    flag_status: Option<String>,
    preview: Option<String>,
}

pub(super) fn decode_message(
    folder: &FolderReference,
    raw: RawMessage,
) -> CacheResult<MailMessage> {
    let id = MessageId::try_from(raw.message_id).map_err(|_| CacheFailure::InvalidData)?;
    let received_at = system_time_from_parts(raw.received_seconds, raw.received_nanoseconds)?;
    let from = decode_mailbox(raw.from_display_name, raw.from_address)?;
    let sender = decode_mailbox(raw.sender_display_name, raw.sender_address)?;
    let read_state = if raw.is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    };
    let metadata = MessageMetadata::new(
        raw.subject,
        MessageOrigin::new(from, sender),
        received_at,
        read_state,
        raw.flag_status
            .as_deref()
            .map(follow_up_flag_status_from_name)
            .transpose()?,
    )
    .with_preview(
        raw.preview
            .as_deref()
            .and_then(MessagePreview::from_untrusted),
    );
    Ok(MailMessage::new(folder.clone(), id, metadata))
}

pub(super) fn read_state_from_bool(is_read: bool) -> ReadState {
    if is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    }
}

fn follow_up_flag_status_from_name(name: &str) -> CacheResult<FollowUpFlagStatus> {
    match name {
        "not_flagged" => Ok(FollowUpFlagStatus::NotFlagged),
        "flagged" => Ok(FollowUpFlagStatus::Flagged),
        "complete" => Ok(FollowUpFlagStatus::Complete),
        _ => Err(CacheFailure::InvalidData),
    }
}

pub(super) fn follow_up_flag_status_name(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => "not_flagged",
        FollowUpFlagStatus::Flagged => "flagged",
        FollowUpFlagStatus::Complete => "complete",
    }
}

fn decode_mailbox(
    display_name: Option<String>,
    address: Option<String>,
) -> CacheResult<Option<Mailbox>> {
    match (display_name, address) {
        (None, None) => Ok(None),
        (display_name, Some(address)) => Mailbox::try_new(address, display_name)
            .map(Some)
            .map_err(|_| CacheFailure::InvalidData),
        (Some(_), None) => Err(CacheFailure::InvalidData),
    }
}

fn decode_body(
    body_type: Option<String>,
    body_content: Option<String>,
) -> CacheResult<Option<MessageBody>> {
    match (body_type.as_deref(), body_content) {
        (None, None) => Ok(None),
        (Some("html"), Some(content)) => Ok(Some(MessageBody::new(MessageBodyType::Html, content))),
        (Some("text"), Some(content)) => Ok(Some(MessageBody::new(MessageBodyType::Text, content))),
        _ => Err(CacheFailure::InvalidData),
    }
}
