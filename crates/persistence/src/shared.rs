/// A shared account-cache setup or teardown failure before application-level mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CacheLifecycleError {
    Unavailable,
    InUse,
    MissingEncryptionKey,
    InvalidData,
}

#[cfg(target_os = "linux")]
pub(crate) mod schema;
#[cfg(target_os = "linux")]
pub(crate) mod store;
