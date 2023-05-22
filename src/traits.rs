use std::{error::Error, fmt::Debug, hash::Hash, sync::Arc};

use async_trait::async_trait;

use crate::Path;

/// A datatype that can be used as the access key for cached items.
///
/// Note that this datatype when converted into a path, should not contain values
/// that can be misinterpreted by the OS (e.g. path separators). I recommend
/// UUIDv4 for most use cases.
pub trait Key
where
    Self: Clone + Debug + Eq + Hash,
{
    /// Generate a new, unique key.
    fn new() -> Self;

    /// Convert this key to a filename used for flushing to disk.
    fn as_filename(&self) -> String;
}
#[cfg(feature = "uuid-as-key")]
impl Key for uuid::Uuid {
    fn new() -> Self {
        uuid::Uuid::new_v4()
    }

    fn as_filename(&self) -> String {
        self.to_string()
    }
}

/// A data structure with a file representation which can be loaded from
/// and flushed to disk asynchronously.
#[async_trait]
pub trait AsyncFileRepr
where
    Self: Sized,
{
    type Err: Error;

    /// Load the data structure from disk asynchronously.
    ///
    /// If you wish to perform non-trivial deserialisation in this function,
    /// you should spawn a blocking task with your async runtime.
    async fn load(path: impl AsRef<Path> + Send) -> Result<Self, Self::Err>;

    /// Flush the data structure to disk asynchronously.
    ///
    /// If you wish to perform non-trivial serialisation in this function,
    /// you should spawn a blocking task with your async runtime.
    async fn flush(self: &Arc<Self>, path: impl AsRef<Path> + Send) -> Result<(), Self::Err>;

    /// Delete the data structure from disk asynchronously.
    async fn delete(path: impl AsRef<Path> + Send) -> Result<(), Self::Err>;
}
