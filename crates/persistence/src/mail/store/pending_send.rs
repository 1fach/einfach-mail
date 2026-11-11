use super::*;

struct StoredPendingSend {
    operation_id: String,
    target_kind: String,
    target: StoredPendingSendTarget,
    created_seconds: i64,
    created_nanoseconds: i64,
}

struct StoredPendingSendTarget {
    recipient: Option<String>,
    subject: Option<String>,
    source_folder_id: Option<String>,
    source_message_id: Option<String>,
}

impl StoredPendingSend {
    fn read(connection: &Connection, account_id: &str) -> CacheResult<Option<Self>> {
        connection
            .query_row(
                "SELECT operation_id, target_kind, recipient, subject,
                        source_folder_id, source_message_id,
                        created_seconds, created_nanoseconds
                 FROM mail_pending_send WHERE account_id = ?1",
                [account_id],
                Self::from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn from_row(row: &async_sqlite::rusqlite::Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            operation_id: row.get(0)?,
            target_kind: row.get(1)?,
            target: StoredPendingSendTarget {
                recipient: row.get(2)?,
                subject: row.get(3)?,
                source_folder_id: row.get(4)?,
                source_message_id: row.get(5)?,
            },
            created_seconds: row.get(6)?,
            created_nanoseconds: row.get(7)?,
        })
    }

    fn into_domain(self, account_id: &str) -> CacheResult<PendingMailSend> {
        let Self {
            operation_id,
            target_kind,
            target,
            created_seconds,
            created_nanoseconds,
        } = self;
        let account_id =
            AccountId::try_from(account_id.to_owned()).map_err(|_| CacheFailure::InvalidData)?;
        let operation_id = Uuid::parse_str(&operation_id)
            .map(MailSendOperationId::from_uuid)
            .map_err(|_| CacheFailure::InvalidData)?;
        let created_at = system_time_from_parts(created_seconds, created_nanoseconds)?;
        match target_kind.as_str() {
            "new_message" => target.into_new_message(account_id, operation_id, created_at),
            "reply" => target.into_reply(account_id, operation_id, created_at),
            _ => Err(CacheFailure::InvalidData),
        }
    }
}

impl StoredPendingSendTarget {
    fn into_new_message(
        self,
        account_id: AccountId,
        operation_id: MailSendOperationId,
        created_at: SystemTime,
    ) -> CacheResult<PendingMailSend> {
        let (recipient, subject) = self.new_message_values()?;
        Ok(PendingMailSend::for_new_message(
            account_id,
            operation_id,
            recipient,
            subject,
            created_at,
        ))
    }

    fn new_message_values(self) -> CacheResult<(MailRecipient, MailSubject)> {
        if self.source_folder_id.is_some() || self.source_message_id.is_some() {
            return Err(CacheFailure::InvalidData);
        }
        let recipient = self
            .recipient
            .ok_or(CacheFailure::InvalidData)?
            .parse::<MailRecipient>()
            .map_err(|_| CacheFailure::InvalidData)?;
        let subject = self
            .subject
            .ok_or(CacheFailure::InvalidData)?
            .parse::<MailSubject>()
            .map_err(|_| CacheFailure::InvalidData)?;
        Ok((recipient, subject))
    }

    fn into_reply(
        self,
        account_id: AccountId,
        operation_id: MailSendOperationId,
        created_at: SystemTime,
    ) -> CacheResult<PendingMailSend> {
        let (source, subject) = self.reply_values(account_id)?;
        Ok(PendingMailSend::for_reply(
            source,
            operation_id,
            subject,
            created_at,
        ))
    }

    fn reply_values(
        self,
        account_id: AccountId,
    ) -> CacheResult<(MailReplySource, Option<Box<str>>)> {
        if self.recipient.is_some() {
            return Err(CacheFailure::InvalidData);
        }
        let folder_id = FolderId::try_from(self.source_folder_id.ok_or(CacheFailure::InvalidData)?)
            .map_err(|_| CacheFailure::InvalidData)?;
        let message_id =
            MessageId::try_from(self.source_message_id.ok_or(CacheFailure::InvalidData)?)
                .map_err(|_| CacheFailure::InvalidData)?;
        let source = MailReplySource::new(FolderReference::new(account_id, folder_id), message_id);
        Ok((source, self.subject.map(String::into_boxed_str)))
    }
}

pub(super) fn read_pending_send(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<Option<PendingMailSend>> {
    StoredPendingSend::read(connection, account_id)?
        .map(|pending| pending.into_domain(account_id))
        .transpose()
}
