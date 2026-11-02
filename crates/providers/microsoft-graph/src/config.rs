//! Valid callback port choices for the Linux system-browser flow.

use std::num::NonZeroU16;

use thiserror::Error;

/// A callback-port value could not be parsed as a TCP port.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("callback port must be an integer from 0 through 65535")]
pub struct InvalidCallbackPort;

/// Selects the IPv4 loopback port used for an authorization callback.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CallbackPort {
    /// Ask the operating system to assign an available port.
    #[default]
    Ephemeral,
    /// Bind this registered port and fail if another process already owns it.
    Fixed(NonZeroU16),
}

impl CallbackPort {
    /// Returns the port to bind, where `0` requests an operating-system port.
    #[must_use]
    pub const fn number(self) -> u16 {
        match self {
            Self::Ephemeral => 0,
            Self::Fixed(port) => port.get(),
        }
    }

    /// Parses the optional callback-port environment value.
    ///
    /// Missing or zero values select an operating-system-assigned port.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCallbackPort`] when `port_text` is not a decimal `u16`.
    pub fn parse_environment_value(port_text: Option<&str>) -> Result<Self, InvalidCallbackPort> {
        let Some(port_text) = port_text else {
            return Ok(Self::Ephemeral);
        };
        let number = port_text.parse::<u16>().map_err(|_| InvalidCallbackPort)?;
        match NonZeroU16::new(number) {
            Some(port) => Ok(Self::Fixed(port)),
            None => Ok(Self::Ephemeral),
        }
    }
}
