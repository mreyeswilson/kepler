use std::fmt;

#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// Database was written by a newer Kepler; refusing to touch it.
    SchemaTooNew {
        found: i64,
        supported: i64,
    },
    /// Input rejected by a privacy rule (sensitive path, empty/oversized text...).
    Rejected(&'static str),
    /// The writer thread is gone or did not answer in time.
    WriterUnavailable,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::SchemaTooNew { found, supported } => {
                write!(
                    f,
                    "database schema v{found} is newer than supported v{supported}"
                )
            }
            Self::Rejected(why) => write!(f, "rejected by privacy rules: {why}"),
            Self::WriterUnavailable => f.write_str("storage writer unavailable"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
