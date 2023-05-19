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
impl<K, E> From<E> for Error<K, E>
where
    K: Key,
    E: std::error::Error,
{
    fn from(err: E) -> Self {
        Self::FileOp(err)
    }
}
