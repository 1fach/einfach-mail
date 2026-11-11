use ews::server_version::ServerVersionInfo;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExchangeProductVersion {
    major: u32,
    minor: u32,
    major_build: u32,
    minor_build: u32,
}

impl ExchangeProductVersion {
    pub(crate) fn from_server_version_info(info: &ServerVersionInfo) -> Option<Self> {
        Some(Self {
            major: numeric_component(info.major_version.as_deref())?,
            minor: numeric_component(info.minor_version.as_deref())?,
            major_build: numeric_component(info.major_build_number.as_deref())?,
            minor_build: numeric_component(info.minor_build_number.as_deref())?,
        })
    }

    pub(crate) fn supports_read_receipt_suppression(self) -> bool {
        self >= Self {
            major: 15,
            minor: 0,
            major_build: 847,
            minor_build: 32,
        }
    }
}

fn numeric_component(component: Option<&str>) -> Option<u32> {
    let component = component?;
    if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    component.parse().ok()
}
#[cfg(test)]
mod tests;
