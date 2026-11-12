mod body;
mod composer;
mod view;

pub(crate) use composer::{MailPermissionRequester, bind_mail_composer_keys};
pub(crate) use view::{
    MailFolderNavigation, MailView, MailViewEvent, bind_message_navigation_keys,
};
