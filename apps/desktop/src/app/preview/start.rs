//! Startup choices for the developer-only UI preview.

use std::io;

use crate::features::AppearancePreference;

/// Environment variable that picks the view the preview opens on.
pub(crate) const DESTINATION_ENV: &str = "EINFACH_MAIL_UI_PREVIEW_DESTINATION";
/// Environment variable that picks the preview's saved appearance.
pub(crate) const APPEARANCE_ENV: &str = "EINFACH_MAIL_UI_PREVIEW_APPEARANCE";
/// Environment variable that picks the preview window size as `WIDTHxHEIGHT`.
pub(crate) const WINDOW_SIZE_ENV: &str = "EINFACH_MAIL_UI_PREVIEW_WINDOW_SIZE";

const MAX_WINDOW_EDGE: u32 = 16_384;

/// A view the preview can open on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PreviewDestination {
    #[default]
    Accounts,
    Mail,
    /// Mail with the first synthetic message open in the reader.
    MailReader,
    Calendar,
    Settings,
}

/// Window size in logical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreviewWindowSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Validated startup choices for one preview run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreviewStart {
    pub(crate) destination: PreviewDestination,
    /// `None` keeps the preview's default, which follows the system appearance.
    pub(crate) appearance: Option<AppearancePreference>,
    /// `None` keeps the platform's default window size.
    pub(crate) window_size: Option<PreviewWindowSize>,
}

impl PreviewStart {
    pub(crate) fn parse(
        destination: Option<&str>,
        appearance: Option<&str>,
        window_size: Option<&str>,
    ) -> io::Result<Self> {
        Ok(Self {
            destination: destination
                .map(parse_destination)
                .transpose()?
                .unwrap_or_default(),
            appearance: appearance.map(parse_appearance).transpose()?,
            window_size: window_size.map(parse_window_size).transpose()?,
        })
    }
}

fn parse_destination(value: &str) -> io::Result<PreviewDestination> {
    match value {
        "accounts" => Ok(PreviewDestination::Accounts),
        "mail" => Ok(PreviewDestination::Mail),
        "mail-reader" => Ok(PreviewDestination::MailReader),
        "calendar" => Ok(PreviewDestination::Calendar),
        "settings" => Ok(PreviewDestination::Settings),
        _ => Err(invalid_value(
            DESTINATION_ENV,
            value,
            "accounts, mail, mail-reader, calendar, or settings",
        )),
    }
}

fn parse_appearance(value: &str) -> io::Result<AppearancePreference> {
    match value {
        "light" => Ok(AppearancePreference::Light),
        "dark" => Ok(AppearancePreference::Dark),
        _ => Err(invalid_value(APPEARANCE_ENV, value, "light or dark")),
    }
}

fn parse_window_size(value: &str) -> io::Result<PreviewWindowSize> {
    let expected = "WIDTHxHEIGHT with each edge from 1 to 16384";
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| invalid_value(WINDOW_SIZE_ENV, value, expected))?;
    let edge = |text: &str| {
        text.parse::<u32>()
            .ok()
            .filter(|edge| (1..=MAX_WINDOW_EDGE).contains(edge))
            .ok_or_else(|| invalid_value(WINDOW_SIZE_ENV, value, expected))
    };
    Ok(PreviewWindowSize {
        width: edge(width)?,
        height: edge(height)?,
    })
}

fn invalid_value(variable: &str, value: &str, expected: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unsupported {variable} value {value:?}; use {expected}"),
    )
}

#[cfg(test)]
mod tests;
