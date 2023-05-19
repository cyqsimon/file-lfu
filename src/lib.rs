use std::{
    borrow::Borrow,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::Arc,
};

use lfu_cache::LfuCache;

use crate::{
    error::Error,
    traits::{AsyncFileRepr, Key},
};

pub mod error;
#[cfg(test)]
mod test;
pub mod traits;

/// A LFU (least frequently used) cache layered on top a file system,
/// where files can be accessed using their unique keys.
///
/// Files can be loaded from disk and stored in cache. When evicted from cache,
/// the file is automatically flushed to disk.
///
/// Note that if you are caching persistent data, you should call [`Self::flush_all`]
/// before dropping this cache. Otherwise new items and changes in the cache
/// will be lost.
#[derive(Debug)]
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

    /// Get the number of loaded items in cache.
    pub fn loaded_count(&self) -> usize {
        self.cache.len()
    }

    /// Get the backing directory of this cache on disk.
    pub fn get_backing_directory(&self) -> &Path {
        &self.directory
    }

    /// Get the file path in the backing directory for a key.
    ///
    /// Note that this function does not care whether the input key exists or not,
    /// and therefore makes no guarantee on the existence of this file.
    pub fn get_path_for(&self, key: impl Borrow<K>) -> PathBuf {
        self.directory.join(key.borrow().to_string())
    }

    /// Get whether a key already exists, whether in cache or on disk.
    pub fn has_key(&self, key: impl Borrow<K>) -> bool {
        let key = key.borrow();
        self.has_loaded_key(key) || self.has_flushed_key(key)
    }

    /// Get whether a key has been loaded in cache.
    pub fn has_loaded_key(&self, key: impl Borrow<K>) -> bool {
        self.cache.keys().any(|k| k == key.borrow())
    }

    /// Get whether a key has been flushed to disk.
    pub fn has_flushed_key(&self, key: impl Borrow<K>) -> bool {
        self.get_path_for(key).is_file()
    }

    /// Get an item from cache (if present) using its unique key, and increment
    /// its usage frequency.
    pub fn get(&mut self, key: impl Borrow<K>) -> Result<Arc<T>, Error<K, T::Err>> {
        let key = key.borrow();

        self.cache
            .get(key)
            .cloned()
            .ok_or(Error::NotInCache(key.clone()))
    }

    /// Get a mutable reference to an item from the cache using its unique key,
    /// and increment its usage frequency.
    ///
    /// If there exists other `Arc`s that point to this item, this function will error
    /// because it's not safe to mutate a shared value.
    pub fn get_mut(&mut self, key: impl Borrow<K>) -> Result<&mut T, Error<K, T::Err>> {
        let key = key.borrow();

        let Some(item) = self.cache.get_mut(key) else {
            Err(Error::NotInCache(key.clone()))?
        };

        Arc::get_mut(item).ok_or(Error::Immutable(key.clone()))
    }

    /// Using a unique key, get an item from cache, or if it is not found in cache,
    /// load it into cache first and then return it.
    ///
    /// Usage frequency is incremented in both cases. Eviction will happen if necessary.
    pub async fn get_or_load(&mut self, key: impl Borrow<K>) -> Result<Arc<T>, Error<K, T::Err>> {
        let key = key.borrow();

        // lookup cache, retrieve if loaded
        if let Some(item) = self.cache.get(key) {
            return Ok(Arc::clone(item));
        }

        // load from disk
        let item = Arc::new(self.read_from_disk(key, Error::NotFound).await?);

        // insert
        self.insert_and_handle_eviction(key.clone(), Arc::clone(&item))
            .await?;

        Ok(item)
    }

    /// Using a unique key, get a mutable reference to an item from cache,
    /// or if it not found in cache, load it into cache first and then return
    /// a mutable reference to it.
    ///
    /// Usage frequency is incremented in both cases. Eviction will happen if necessary.
    ///
    /// If there exists other `Arc`s that point to this item, this function will error
    /// because it's not safe to mutate a shared value.
    pub async fn get_or_load_mut(
        &mut self,
        key: impl Borrow<K>,
    ) -> Result<&mut T, Error<K, T::Err>> {
        let key = key.borrow();

        // lookup cache, load from disk if not found
        if !self.has_loaded_key(key) {
            let item = self.read_from_disk(key, Error::NotFound).await?;
            self.insert_and_handle_eviction(key.clone(), Arc::new(item))
                .await?;
        }

        // retrieve cache
        Arc::get_mut(
            self.cache
                .get_mut(key)
                .expect("something is wrong with Arc"), // item either exists in cache or was just loaded
        )
        .ok_or(Error::Immutable(key.clone()))
    }

    /// Push an item into cache, assign it a unique key, then return the key.
    ///
    /// Usage frequency is incremented. Eviction will happen if necessary.
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
    /// Neither frequency increment nor eviction will not occur. Hence this method
    /// does not require a mutable reference to self.
    pub async fn direct_flush(&self, item: T) -> Result<K, Error<K, T::Err>> {
        let key = K::new();
        let flush_path = self.get_path_for(&key);

        // flush
        Arc::new(item).flush(flush_path).await?;

        Ok(key)
    }

    /// Flush an item in cache to the backing directory on disk.
    ///
    /// The flushed item neither has its frequency incremented, nor will it be evicted.
    /// Hence this method does not require a mutable reference to self.
    pub async fn flush(&self, key: impl Borrow<K>) -> Result<(), Error<K, T::Err>> {
        let key = key.borrow();

        let item = self
            .cache
            .peek_iter()
            .find_map(|(k, v)| (k == key).then_some(v))
            .ok_or(Error::NotInCache(key.clone()))?;

        let flush_path = self.get_path_for(key);
        item.flush(flush_path).await?;

        Ok(())
    }

    /// Flush all items in cache to the backing directory on disk.
    ///
    /// The flushed items neither have their frequencies incremented, or are not evicted.
    /// Hence this method does not require a mutable reference to self.
    ///
    /// Note that this method does not fail fast. Instead it makes a flush attempt
    /// on all items in cache, then collects and returns all errors encountered (if any).
    /// Therefore a partial failure is possible (and is likely).
    pub async fn flush_all(&self) -> Result<(), Vec<Error<K, T::Err>>> {
        let mut errors = vec![];

        for (key, item) in self.cache.peek_iter() {
            let flush_path = self.get_path_for(key);
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

    /// Evict all items from cache, and optionally flushing all of them
    /// to the backing directory on disk.
    pub async fn clear_cache(&mut self, do_flush: bool) -> Result<(), Vec<Error<K, T::Err>>> {
        if do_flush {
            self.flush_all().await?;
        }
        self.cache.clear();

        Ok(())
    }

    /// Delete an item from both the cache and the backing directory on disk.
    pub async fn delete(&mut self, key: impl Borrow<K>) -> Result<(), Error<K, T::Err>> {
        let key = key.borrow();

        if !self.has_key(key) {
            Err(Error::NotFound(key.clone()))?
        }

        // remove from cache
        self.cache.remove(key);

        // remove from disk
        let path = self.get_path_for(key);
        if path.is_file() {
            T::delete(path).await?;
        }

        Ok(())
    }

    /// Helper function to read an item from the backing directory using its key.
    ///
    /// `not_found_variant` is a closure defining which semantic variant of "not found"
    /// this function should use. This allows the caller to customise the returned error
    /// according to the semantics of the call site.
    async fn read_from_disk<F>(
        &self,
        key: impl Borrow<K>,
        not_found_variant: F,
    ) -> Result<T, Error<K, T::Err>>
    where
        F: FnOnce(K) -> Error<K, T::Err>,
    {
        let key = key.borrow();

        let load_path = self.get_path_for(key);
        if !load_path.is_file() {
            Err(not_found_variant(key.clone()))?
        }
        let item = T::load(load_path).await?;

        Ok(item)
    }

    /// Helper function to insert an item, and handle the possible eviction it caused
    /// by flushing the evicted item to the backing directory on disk.
    ///
    /// Note that this function requires that the provided key is not yet loaded.
    /// If this key can already be found in cache, this function wil panic.
    async fn insert_and_handle_eviction(&mut self, key: K, item: Arc<T>) -> Result<(), T::Err> {
        assert!(!self.has_loaded_key(&key), "key already present in cache");

        // when peek_lfu_key() returns `Some`, it just means there is at least 1 item;
        // an eviction will not necessarily happen on the next insertion
        let flush_key = self.cache.peek_lfu_key().cloned();
        let evicted_item = self.cache.insert(key.clone(), item);

        match (flush_key, evicted_item) {
            (Some(key), Some(evicted)) => {
                let flush_path = self.get_path_for(key);
                evicted.flush(flush_path).await?;
            }
            (_, None) => {
                // nothing evicted
            }
            (None, Some(_)) => unreachable!("something is wrong with the LFU implementation"),
        };

        Ok(())
    }
}
