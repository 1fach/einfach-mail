use ews::server_version::ServerVersionInfo;

use super::ExchangeProductVersion;

fn server_version_info(
    major: Option<&str>,
    minor: Option<&str>,
    major_build: Option<&str>,
    minor_build: Option<&str>,
    version: Option<&str>,
) -> ServerVersionInfo {
    ServerVersionInfo {
        major_version: major.map(str::to_owned),
        minor_version: minor.map(str::to_owned),
        major_build_number: major_build.map(str::to_owned),
        minor_build_number: minor_build.map(str::to_owned),
        version: version.map(str::to_owned),
    }
}

#[test]
fn recognizes_exchange_2013_sp1_and_later_product_builds() {
    for info in [
        server_version_info(Some("15"), Some("0"), Some("847"), Some("32"), None),
        server_version_info(Some("15"), Some("1"), Some("2507"), Some("10"), None),
    ] {
        assert!(
            ExchangeProductVersion::from_server_version_info(&info)
                .is_some_and(ExchangeProductVersion::supports_read_receipt_suppression)
        );
    }
}

#[test]
fn rejects_older_incomplete_malformed_and_overflowing_product_versions() {
    for info in [
        server_version_info(Some("15"), Some("0"), Some("516"), Some("32"), None),
        server_version_info(Some("15"), Some("0"), Some("847"), None, None),
        server_version_info(Some("fifteen"), Some("0"), Some("847"), Some("32"), None),
        server_version_info(Some("+15"), Some("0"), Some("847"), Some("32"), None),
        server_version_info(Some("4294967296"), Some("0"), Some("847"), Some("32"), None),
        server_version_info(None, None, None, None, Some("Exchange2013_SP1")),
    ] {
        assert!(
            ExchangeProductVersion::from_server_version_info(&info)
                .is_none_or(|version| !version.supports_read_receipt_suppression())
        );
    }
}
