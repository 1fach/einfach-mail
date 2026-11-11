use super::*;

pub(super) fn read_messages(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<Vec<MailMessage>> {
    if !folder_exists(connection, folder)? {
        return Err(CacheFailure::InvalidData);
    }
    let mut statement = connection.prepare(
        "SELECT message_id, subject, from_display_name, from_address,
                sender_display_name, sender_address, received_seconds,
                received_nanoseconds, is_read, flag_status
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2
         ORDER BY received_seconds DESC, received_nanoseconds DESC, message_id ASC",
    )?;
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
            "SELECT message_id, subject, from_display_name, from_address,
                    sender_display_name, sender_address, received_seconds,
                    received_nanoseconds, is_read, flag_status, body_type, body_content
             FROM mail_messages
             WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
            params![
                folder.account_id().as_str(),
                folder.folder_id().as_str(),
                message_id.as_str()
            ],
            |row| {
                Ok((
                    raw_message_from_row(row)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
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
        message_id: row.get(0)?,
        subject: row.get(1)?,
        from_display_name: row.get(2)?,
        from_address: row.get(3)?,
        sender_display_name: row.get(4)?,
        sender_address: row.get(5)?,
        received_seconds: row.get(6)?,
        received_nanoseconds: row.get(7)?,
        is_read: row.get(8)?,
        flag_status: row.get(9)?,
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
