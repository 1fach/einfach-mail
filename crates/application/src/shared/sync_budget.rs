pub(crate) const MAX_DELTA_PAGES_PER_ATTEMPT: usize = 1_000;
pub(crate) const MAX_DELTA_RESETS_PER_ATTEMPT: usize = 1;
pub(crate) const MAX_SYNC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn response_bytes_after(current_bytes: usize, page_bytes: usize) -> Option<usize> {
    current_bytes
        .checked_add(page_bytes)
        .filter(|total| *total <= MAX_SYNC_RESPONSE_BYTES)
}
