use std::fmt;

/// Why a sync failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncError {
    /// No network, server errors, timeouts: retried silently.
    Transient(String),
    /// Something only the user can fix (e.g. the add-on is not installed).
    NeedsUser(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Transient(message) | SyncError::NeedsUser(message) => f.write_str(message),
        }
    }
}
