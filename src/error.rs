use std::fmt;

use crate::{traits::Key, PathBuf};

/// Errors produced by `FileBackedLfuCache`.
#[derive(Debug, thiserror::Error)]
pub enum Error<K, E>
where
    K: Key,
    E: std::error::Error,
{
    /// Cannot initialise the given path as a backing directory.
    ///
    /// This can happen if the path does not resolve to a directory.
    Init(PathBuf),

    /// An item cannot be found with this key in cache.
    NotInCache(K),

    /// An item cannot be found with this key on disk.
    NotOnDisk(K),

    /// An item cannot be found with this key either in cache or on disk.
    NotFound(K),

    /// An error occurred when performing file operations.
    ///
    /// The inner type is the user-defined associated type of `AsyncFileRepr`.
    FileOp(E),

    /// An item with this key is temporarily immutable due to outstanding references.
    ///
    /// This can happen if you are holding a reference elsewhere, or if this item
    /// is in the process of being flushed to disk.
    Immutable(K),
}
impl<K, E> fmt::Display for Error<K, E>
where
    K: Key,
    E: std::error::Error,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;

        let repr = match self {
            Init(path) => format!("Cannot initialise {path:?} as a backing directory."),
            NotInCache(key) => format!("Cannot find an item with key {key:?} in cache"),
            NotOnDisk(key) => format!("Cannot find an item with key {key:?} on disk"),
            NotFound(key) => {
                format!("Cannot find an item with key {key:?} either in cache or on disk")
            }
            FileOp(error) => {
                format!("An error occurred when performing file operations: {error}")
            }
            Immutable(key) => format!(
                "An item with key {key:?} is temporarily immutable due to outstanding references"
            ),
        };

        write!(f, "{repr}")
    }
}
impl<K, E> From<E> for Error<K, E>
where
    K: Key,
    E: std::error::Error,
{
    fn from(err: E) -> Self {
        Self::FileOp(err)
    }
}
