use std::{fmt, str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use fs_extra::dir::CopyOptions;
use rand::Rng;
use rstest::{fixture, rstest};
use temp_dir::TempDir;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time,
};
use uuid::Uuid;

use crate::{error::Error, traits::AsyncFileRepr, FileBackedLfuCache, Path};

#[cfg(not(feature = "uuid-as-key"))]
impl crate::traits::Key for Uuid {
    fn new() -> Self {
        Uuid::new_v4()
    }

    fn as_filename(&self) -> String {
        self.to_string()
    }
}

/// A test struct that simply stores all lines of a file into a Vec<String>.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllLines(Vec<String>);
impl<'a> FromIterator<&'a str> for AllLines {
    fn from_iter<T: IntoIterator<Item = &'a str>>(it: T) -> Self {
        let lines = it.into_iter().map(ToOwned::to_owned).collect();
        Self(lines)
    }
}
#[async_trait]
impl AsyncFileRepr for AllLines {
    type Err = TestError;

    async fn load(path: impl AsRef<Path> + Send) -> Result<Self, Self::Err> {
        let path = path.as_ref();

        let mut lines = BufReader::new(tokio::fs::File::open(path).await?).lines();
        let mut v = vec![];
        while let Some(line) = lines.next_line().await? {
            v.push(line);
        }

        Ok(Self(v))
    }

    async fn flush(self: &Arc<Self>, path: impl AsRef<Path> + Send) -> Result<(), Self::Err> {
        let path = path.as_ref();

        let content = self.0.join("\n") + "\n";
        tokio::fs::write(path, content).await?;

        Ok(())
    }

    async fn delete(path: impl AsRef<Path> + Send) -> Result<(), Self::Err> {
        let path = path.as_ref();

        tokio::fs::remove_file(path).await?;

        Ok(())
    }
}
impl AllLines {
    fn random() -> Self {
        let mut rng = rand::thread_rng();

        let mut v = vec![];
        for _ in 0..10 {
            let mut s = String::new();
            for _ in 0..10 {
                s.push(rng.gen_range('a'..='z'));
            }
            v.push(s);
        }

        Self(v)
    }
}

/// A basic error.
#[derive(Debug, Clone, thiserror::Error)]
struct TestError(String);
impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl From<std::io::Error> for TestError {
    fn from(err: std::io::Error) -> Self {
        Self(err.to_string())
    }
}

// convenience type aliases
type Cache = FileBackedLfuCache<Uuid, AllLines>;
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The keys of the existing test files.
#[fixture]
fn keys() -> [Uuid; 2] {
    let mut v: Vec<_> = [
        "215a316d-4b3e-4bc3-9acb-2a3f992045d3",
        "b6174a01-334d-4924-ad4c-e044d7228560",
    ]
    .into_iter()
    .rev()
    .map(Uuid::from_str)
    .map(Result::unwrap)
    .collect();

    [v.pop().unwrap(), v.pop().unwrap()]
}

/// An empty cache based on an empty temporary directory.
///
/// Note that the returned `TempDir` is an RAII guard, which on drop will clean up
/// the temporary directory. Therefore it's important to keep it in scope
/// throughout the test. The easiest way to do this is to call `drop` on it
/// just before the test ends.
#[fixture]
fn empty_cache_setup(#[default(3)] capacity: usize) -> (Cache, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let temp_path: &Path = temp_dir.path().try_into().unwrap();

    let cache = FileBackedLfuCache::init(temp_path, capacity).unwrap();

    assert_eq!(std::fs::read_dir(temp_path).unwrap().count(), 0);
    assert_eq!(cache.loaded_count(), 0);

    (cache, temp_dir)
}

/// A cache based on a temporary directory that contains a copy of the test files.
/// The cache itself has nothing loaded.
///
/// Note that the returned `TempDir` is an RAII guard, which on drop will clean up
/// the temporary directory. Therefore it's important to keep it in scope
/// throughout the test. The easiest way to do this is to call `drop` on it
/// just before the test ends.
#[fixture]
fn unloaded_cache_setup(#[default(3)] capacity: usize, keys: [Uuid; 2]) -> (Cache, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let temp_path: &Path = temp_dir.path().try_into().unwrap();

    let res_dir = Path::new("test_res");
    let files: Vec<_> = keys
        .into_iter()
        .map(|k| res_dir.join(k.to_string()))
        .collect();
    fs_extra::copy_items(&files, temp_path, &CopyOptions::default()).unwrap();

    let cache = FileBackedLfuCache::init(temp_path, capacity).unwrap();

    assert_eq!(std::fs::read_dir(temp_path).unwrap().count(), keys.len());
    assert_eq!(cache.loaded_count(), 0);

    (cache, temp_dir)
}

/// A filled cache based on a temporary directory that contains a copy of the test files.
/// The cache has a capacity exactly equalling the number of test files,
/// and is completely filled.  
///
/// Note that the returned `TempDir` is an RAII guard, which on drop will clean up
/// the temporary directory. Therefore it's important to keep it in scope
/// throughout the test. The easiest way to do this is to call `drop` on it
/// just before the test ends.
#[fixture]
async fn filled_cache_setup(keys: [Uuid; 2]) -> (Cache, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let temp_path: &Path = temp_dir.path().try_into().unwrap();

    let res_dir = Path::new("test_res");
    let files: Vec<_> = keys
        .into_iter()
        .map(|k| res_dir.join(k.to_string()))
        .collect();
    fs_extra::copy_items(&files, temp_path, &CopyOptions::default()).unwrap();

    let mut cache = FileBackedLfuCache::init(temp_path, files.len()).unwrap();
    for key in keys.iter() {
        let _item = cache.get_or_load(key).await.unwrap();
    }

    assert_eq!(std::fs::read_dir(temp_path).unwrap().count(), keys.len());
    assert_eq!(cache.loaded_count(), keys.len());

    (cache, temp_dir)
}

#[rstest]
#[tokio::test]
async fn can_load_when_not_full(
    unloaded_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = unloaded_cache_setup;

    for key in keys.iter() {
        let _item = cache.get_or_load(key).await?;
    }
    assert_eq!(cache.loaded_count(), 2);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_load_when_full(
    #[with(1)] unloaded_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = unloaded_cache_setup;

    for key in keys.iter() {
        let _item = cache.get_or_load(key).await?;
        assert_eq!(cache.loaded_count(), 1);
        assert!(cache.has_loaded_key(key));
    }

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_get(#[future] filled_cache_setup: (Cache, TempDir), keys: [Uuid; 2]) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    for key in keys.iter() {
        let _item = cache.get(key)?;
    }

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_reject_random_key(empty_cache_setup: (Cache, TempDir)) {
    let (mut cache, temp_dir) = empty_cache_setup;

    let res = cache.get_or_load(Uuid::new_v4()).await;
    assert!(matches!(res, Err(Error::NotFound(_))));

    drop(temp_dir);
}

#[rstest]
#[tokio::test]
async fn can_load_mut_when_not_full(
    unloaded_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = unloaded_cache_setup;

    for key in keys.iter() {
        let _item = cache.get_or_load_mut(key).await?;
    }
    assert_eq!(cache.loaded_count(), 2);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_load_mut_when_full(
    #[with(1)] unloaded_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = unloaded_cache_setup;

    for key in keys.iter() {
        let _item = cache.get_or_load_mut(key).await?;
        assert_eq!(cache.loaded_count(), 1);
        assert!(cache.has_loaded_key(key));
    }

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_get_mut(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    for key in keys.iter() {
        let _item = cache.get_mut(key)?;
    }

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_reject_get_mut_when_shared(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [shared, unshared] = &keys;

    // keep an outstanding reference to one of the items
    let shared_item = cache.get(shared)?;

    let shared_res = cache.get_mut(shared);
    assert!(matches!(shared_res, Err(Error::Immutable(_))));
    let _mut_ref = cache.get_mut(unshared)?;

    // hold the outstanding reference for long enough
    drop(shared_item);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_mutate(empty_cache_setup: (Cache, TempDir)) -> TestResult {
    let (mut cache, temp_dir) = empty_cache_setup;

    let original = AllLines::random();
    let key = cache.push(original.clone()).await?;
    assert_eq!(cache.get(&key)?.as_ref(), &original);

    let new = AllLines::random();
    *cache.get_or_load_mut(&key).await? = new.clone();
    assert_eq!(cache.get(&key)?.as_ref(), &new);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_push_without_flush(empty_cache_setup: (Cache, TempDir)) -> TestResult {
    let (mut cache, temp_dir) = empty_cache_setup;

    let key = cache.push(AllLines::random()).await?;
    assert_eq!(cache.loaded_count(), 1);
    assert!(cache.has_loaded_key(&key));
    assert!(!cache.has_flushed_key(&key));

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_direct_flush(empty_cache_setup: (Cache, TempDir)) -> TestResult {
    let (cache, temp_dir) = empty_cache_setup;

    let key = cache.direct_flush(AllLines::random()).await?;
    assert_eq!(cache.loaded_count(), 0);
    assert!(!cache.has_loaded_key(&key));
    assert!(cache.has_flushed_key(&key));

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_evict_lfu(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [extra_access, should_evict] = &keys;

    // extra access on 1 item
    let _item = cache.get_or_load(extra_access).await?;
    let new = cache.push(AllLines::random()).await?;

    assert_eq!(cache.loaded_count(), 2);
    assert!(cache.has_loaded_key(extra_access));
    assert!(!cache.has_loaded_key(should_evict));
    assert!(cache.has_loaded_key(new));

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn will_flush_lfu_when_mutated(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [extra_access, should_evict] = &keys;

    // record modification time
    let old_mtime = tokio::fs::metadata(cache.get_path_for(should_evict))
        .await?
        .modified()?;

    // mutate 1 item
    // no actual mutation is necessary
    // simply obtaining a mutable reference should suffice
    let _mut_ref = cache.get_mut(should_evict)?;

    // extra accesses on 1 item
    for _ in 0..10 {
        let _item = cache.get_or_load(extra_access).await?;
    }

    // wait a bit before triggering LFU flush
    time::sleep(Duration::from_millis(100)).await;

    // trigger LFU flush
    let _new = cache.push(AllLines::random()).await?;
    assert!(!cache.has_loaded_key(should_evict));

    // compare modification times
    let new_mtime = tokio::fs::metadata(cache.get_path_for(should_evict))
        .await?
        .modified()?;
    assert!(old_mtime < new_mtime);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn wont_flush_lfu_when_not_mutated(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [extra_access, should_evict] = &keys;

    // record modification time
    let old_mtime = tokio::fs::metadata(cache.get_path_for(should_evict))
        .await?
        .modified()?;

    // no mutations on `should_evict`

    // extra accesses on 1 item
    for _ in 0..10 {
        let _item = cache.get_or_load(extra_access).await?;
    }

    // wait a bit before triggering LFU flush
    time::sleep(Duration::from_millis(100)).await;

    // trigger LFU flush
    let _new = cache.push(AllLines::random()).await?;
    assert!(!cache.has_loaded_key(should_evict));

    // compare modification times
    let new_mtime = tokio::fs::metadata(cache.get_path_for(should_evict))
        .await?
        .modified()?;
    assert!(old_mtime == new_mtime);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_manual_flush(empty_cache_setup: (Cache, TempDir)) -> TestResult {
    let (mut cache, temp_dir) = empty_cache_setup;

    let key = cache.push(AllLines::random()).await?;
    assert!(cache.has_loaded_key(&key));
    assert!(!cache.has_flushed_key(&key));

    cache.flush(&key).await?;
    assert!(cache.has_loaded_key(&key));
    assert!(cache.has_flushed_key(&key));

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_manual_flush_all(#[with(3)] empty_cache_setup: (Cache, TempDir)) -> TestResult {
    let (mut cache, temp_dir) = empty_cache_setup;

    // push 3 new items into cache
    let mut keys = vec![];
    for _ in 0..3 {
        let new_key = cache.push(AllLines::random()).await?;
        keys.push(new_key);
    }
    // nothing should be on disk at this point
    for k in keys.iter() {
        assert!(cache.has_loaded_key(k));
        assert!(!cache.has_flushed_key(k));
    }

    // flush all to disk
    cache.flush_all().await.unwrap();
    for k in keys.iter() {
        assert!(cache.has_loaded_key(k));
        assert!(cache.has_flushed_key(k));
    }

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn manual_flush_only_flushes_mutated(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [to_mutate, no_mutate] = &keys;

    // record modification times
    let to_mutate_old_mtime = tokio::fs::metadata(cache.get_path_for(to_mutate))
        .await?
        .modified()?;
    let no_mutate_old_mtime = tokio::fs::metadata(cache.get_path_for(no_mutate))
        .await?
        .modified()?;

    // mutate 1 item
    // no actual mutation is necessary
    // simply obtaining a mutable reference should suffice
    let _mut_ref = cache.get_mut(to_mutate)?;

    // wait a bit before flushing
    time::sleep(Duration::from_millis(100)).await;

    // flush
    cache.flush(to_mutate).await?;
    cache.flush(no_mutate).await?;

    // compare modification times
    let to_mutate_new_mtime = tokio::fs::metadata(cache.get_path_for(to_mutate))
        .await?
        .modified()?;
    let no_mutate_new_mtime = tokio::fs::metadata(cache.get_path_for(no_mutate))
        .await?
        .modified()?;
    assert!(to_mutate_old_mtime < to_mutate_new_mtime);
    assert!(no_mutate_old_mtime == no_mutate_new_mtime);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn manual_flush_all_only_flushes_mutated(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [to_mutate, no_mutate] = &keys;

    // record modification times
    let to_mutate_old_mtime = tokio::fs::metadata(cache.get_path_for(to_mutate))
        .await?
        .modified()?;
    let no_mutate_old_mtime = tokio::fs::metadata(cache.get_path_for(no_mutate))
        .await?
        .modified()?;

    // mutate 1 item
    // no actual mutation is necessary
    // simply obtaining a mutable reference should suffice
    let _mut_ref = cache.get_mut(to_mutate)?;

    // wait a bit before flushing
    time::sleep(Duration::from_millis(100)).await;

    // flush all
    let flush_result = cache.flush_all().await;
    assert!(flush_result.is_ok());

    // compare modification times
    let to_mutate_new_mtime = tokio::fs::metadata(cache.get_path_for(to_mutate))
        .await?
        .modified()?;
    let no_mutate_new_mtime = tokio::fs::metadata(cache.get_path_for(no_mutate))
        .await?
        .modified()?;
    assert!(to_mutate_old_mtime < to_mutate_new_mtime);
    assert!(no_mutate_old_mtime == no_mutate_new_mtime);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn mutation_persists_after_lfu_flush(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [to_mutate, extra_access] = &keys;

    // mutate
    let mutate_new = AllLines::random();
    *cache.get_mut(to_mutate)? = mutate_new.clone();

    // extras accesses to ensure lfu order
    let _access0 = cache.get(extra_access)?;
    let _access1 = cache.get(extra_access)?;
    assert_eq!(cache.cache.peek_lfu_key(), Some(to_mutate));

    // induce eviction
    let _new_item = cache.push(AllLines::random()).await?;
    assert!(!cache.has_loaded_key(to_mutate));

    // mutation should persist
    let mutated = cache.get_or_load(to_mutate).await?;
    assert_eq!(mutated.as_ref(), &mutate_new);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn mutation_persists_after_manual_flush(
    #[future] filled_cache_setup: (Cache, TempDir),
    keys: [Uuid; 2],
) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [to_mutate, _] = &keys;

    // mutate
    let mutate_new = AllLines::random();
    *cache.get_mut(to_mutate)? = mutate_new.clone();

    // flush and clear
    cache.clear_cache(true).await.unwrap();
    assert_eq!(cache.loaded_count(), 0);

    // mutation should persist
    let mutated = cache.get_or_load(to_mutate).await?;
    assert_eq!(mutated.as_ref(), &mutate_new);

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn can_delete(#[future] filled_cache_setup: (Cache, TempDir), keys: [Uuid; 2]) -> TestResult {
    let (mut cache, temp_dir) = filled_cache_setup.await;

    let [to_delete, should_stay] = &keys;

    cache.delete(to_delete).await?;

    assert!(!cache.has_key(to_delete));
    assert!(cache.has_key(should_stay));

    drop(temp_dir);
    Ok(())
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn can_clear(empty_cache_setup: (Cache, TempDir), #[case] with_flush: bool) -> TestResult {
    let (mut cache, temp_dir) = empty_cache_setup;

    let new = AllLines::random();
    let key = cache.push(new).await?;
    assert_eq!(cache.loaded_count(), 1);
    assert!(!cache.has_flushed_key(&key));

    cache.clear_cache(with_flush).await.unwrap();
    assert_eq!(cache.loaded_count(), 0);
    assert_eq!(cache.has_flushed_key(&key), with_flush);

    drop(temp_dir);
    Ok(())
}
