use std::{borrow::Borrow, sync::Arc};

use lfu_cache::LfuCache;

use crate::{
    error::Error,
    traits::{AsyncFileRepr, Key},
};

pub mod error;
#[cfg(test)]
mod test;
pub mod traits;

#[cfg(feature = "utf8-paths")]
pub type Path = camino::Utf8Path;
#[cfg(feature = "utf8-paths")]
pub type PathBuf = camino::Utf8PathBuf;
#[cfg(not(feature = "utf8-paths"))]
pub type Path = std::path::Path;
#[cfg(not(feature = "utf8-paths"))]
pub type PathBuf = std::path::PathBuf;

/// A LFU (least frequently used) cache layered on top a file system,
/// where files can be accessed using their unique keys.
///
/// Files can be loaded from disk and stored in cache. When evicted from cache,
/// the file is automatically flushed to disk.
///
/// Note that [`Self::flush_all`] should be called before dropping this cache,
/// otherwise new items and changes in the cache will be lost.
pub struct FileBackedLfuCache<K, T>
where
    K: Key,
    T: AsyncFileRepr,
{
    /// The storage directory for this cache.
    directory: PathBuf,

    /// The cache.
    cache: LfuCache<K, Arc<T>>,
}
impl<K, T> FileBackedLfuCache<K, T>
where
    K: Key,
    T: AsyncFileRepr,
{
    /// Initialise a cache with a specific size, using the given path as
    /// the backing directory.
    ///
    /// The provided path must exist and resolve to a directory. Otherwise
    /// an error will be returned.
    pub fn init(path: impl AsRef<Path>, size: usize) -> Result<Self, Error<K, T::Err>> {
        let path = path.as_ref().to_owned();
        if !(path.is_dir() || path.canonicalize().map(|p| p.is_dir()).unwrap_or(false)) {
            return Err(Error::Init(path));
        }

        let cache = LfuCache::with_capacity(size);

        Ok(Self { directory: path, cache })
    }

    /// Get an item from cache using its unique key.
    ///
    /// If the key is not found in cache, a lookup using the key will be performed
    /// on the backing directory. The matching file will be loaded into the cache
    /// and returned. Eviction will happen if necessary.
    pub async fn get_or_load(&mut self, key: impl Borrow<K>) -> Result<Arc<T>, Error<K, T::Err>> {
        let key = key.borrow();

        // lookup cache, retrieve if loaded
        if let Some(item) = self.cache.get(key) {
            return Ok(Arc::clone(item));
        }

        // load from disk
        let item = Arc::new(self.read_from_disk(key).await?);

        // insert
        self.insert_and_handle_eviction(key.clone(), Arc::clone(&item))
            .await?;

        Ok(item)
    }

    /// Get a mutable reference to an item from cache using its unique key.
    ///
    /// If the key is not found in cache, a lookup using the key will be performed
    /// on the backing directory. The matching file will be loaded into the cache
    /// and have its mutable reference returned. Eviction will happen if necessary.
    ///
    /// If there exists other `Arc`s that point to this item, this function will error
    /// because it's not safe to mutate a shared value.
    pub async fn get_or_load_mut(
        &mut self,
        key: impl Borrow<K>,
    ) -> Result<&mut T, Error<K, T::Err>> {
        let key = key.borrow();

        // lookup cache, load from disk if not found
        if !self.cache.keys().any(|k| k == key) {
            let item = self.read_from_disk(key).await?;
            self.insert_and_handle_eviction(key.clone(), Arc::new(item))
                .await?;
        }

        // retrieve cache
        Arc::get_mut(
            self.cache
                .get_mut(&key)
                .expect("something is wrong with Arc"), // item either exists in cache or was just loaded
        )
        .ok_or(Error::Immutable(key.clone()))
    }

    /// Push an item into cache and assign it a unique key. Eviction will happen
    /// if necessary. Returns the assigned key.
    ///
    /// Note that the newly added item will not be immediately flushed
    /// to the backing directory on disk.
    pub async fn push(&mut self, item: T) -> Result<K, Error<K, T::Err>> {
        let key = K::new();

        // insert
        self.insert_and_handle_eviction(key.clone(), Arc::new(item))
            .await?;

        Ok(key)
    }

    /// Directly flush an item into the backing directory on disk without
    /// touching the cache. Returns the assigned key.
    ///
    /// Eviction will not occur, hence this method does not require mutable reference
    /// to self.
    pub async fn direct_flush(&self, item: T) -> Result<K, Error<K, T::Err>> {
        let key = K::new();
        let flush_path = self.get_file_path(&key);

        // flush
        Arc::new(item).flush(flush_path).await?;

        Ok(key)
    }

    /// Flush all items in cache to the backing directory on disk.
    ///
    /// The flushed items are not evicted, hence this method does not require
    /// mutable reference to self.
    ///
    /// Note that this method does not fail fast. Instead it makes a flush attempt
    /// on all items in cache, then collects and returns all errors encountered (if any).
    /// Therefore a partial failure is possible (and is likely).
    pub async fn flush_all(&self) -> Result<(), Vec<Error<K, T::Err>>> {
        let mut errors = vec![];

        for (key, item) in self.cache.peek_iter() {
            let flush_path = self.get_file_path(key);
            if let Err(err) = item.flush(flush_path).await {
                errors.push(err.into());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Delete an item from both the cache and the backing directory on disk.
    pub async fn delete(&mut self, key: impl Borrow<K>) -> Result<(), Error<K, T::Err>> {
        let key = key.borrow();

        // remove from cache
        self.cache.remove(key);

        // remove from disk
        let path = self.get_file_path(key);
        T::delete(path).await?;

        Ok(())
    }

    /// Helper function to get the file path in the backing directory for a key.
    pub(crate) fn get_file_path(&self, key: impl Borrow<K>) -> PathBuf {
        self.directory.join(key.borrow().to_string())
    }

    /// Helper function to read an item from the backing directory using its key.
    async fn read_from_disk(&self, key: impl Borrow<K>) -> Result<T, Error<K, T::Err>> {
        let key = key.borrow();

        let load_path = self.get_file_path(key);
        if !load_path.is_file() {
            Err(Error::NotFound(key.clone()))?
        }
        let item = T::load(load_path).await?;

        Ok(item)
    }

    /// Helper function to insert an item, and handle the possible eviction it caused
    /// by flushing the evicted item to the backing directory on disk.
    async fn insert_and_handle_eviction(&mut self, key: K, item: Arc<T>) -> Result<(), T::Err> {
        let maybe_evicted_key = self.cache.peek_lfu_key().cloned();
        let maybe_evicted_item = self.cache.insert(key.clone(), item);

        match (maybe_evicted_key, maybe_evicted_item) {
            (Some(key), Some(evicted)) => {
                let flush_path = self.get_file_path(key);
                evicted.flush(flush_path).await?;
            }
            (None, None) => {
                // noting evicted
            }
            _ => unreachable!("something is wrong with the LFU implementation"),
        };

        Ok(())
    }
}
