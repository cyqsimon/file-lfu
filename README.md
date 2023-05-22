# file-lfu

A least-frequently-used cache layered on a directory.

[crates.io](https://crates.io/crates/file-lfu) [docs.rs](https://docs.rs/file-lfu/latest/file_lfu/index.html)

## Quick start

1. Implement `AsyncFileRepr` for the type you intend to cache.
   - Optionally if you want to use your own type as cache key, implement
     `Key` for your cache key type.
2. Initialise the cache using `FileBackedLfuCache::init`.
3. Add new items to the cache using `FileBackedLfuCache::push`.
   - Items will not be flushed to disk until cache capacity is exceeded.
   - Use `FileBackedLfuCache::{flush, flush_all}` to trigger a flush manually.
4. Use `FileBackedLfuCache::{get, get_or_load}` to access items immutably.
5. Use `FileBackedLfuCache::{get_mut, get_or_load_mut}` to access items mutably.

## Example

```rust
use file_lfu::FileBackedLfuCache;
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
   let mut cache = FileBackedLfuCache<Uuid, Foo>::init("/path/to/data/dir", 2)?;

   // cache is not full, so no flushes or evictions
   let key0 = cache.push(Foo::new("foo0"))?;
   let key1 = cache.push(Foo::new("foo1"))?;

   for _ in 0..10 {
      // use `foo0` a bunch of times to boost its usage frequency
      let _foo0_arc = cache.get(&key0)?; // to use it immutably
      let _foo0_mut_ref = cache.get_mut(&key0)?; // to use it mutably

      // we use `get` and `get_mut` because we are sure `foo0` is in cache
      // if unsure, use `get_or_load` and `get_or_load_mut`
   }

   // cache is now full, so the least frequently used item (currently `foo1`)
   // will be evicted from cache and flushed to disk
   let _key2 = cache.push(Foo::new("foo2"))?;
   // now the cache contains `foo0` and `foo2`

   // when `foo1` is needed again, it will be loaded from disk
   // again, because the cache is still full, `foo2` will be evicted from cache
   // and flushed to disk
   let _foo1_arc = cache.get_or_load(&key1)?;
   let _foo1_mut_ref = cache.get_or_load_mut(&key1)?;

   // trigger a manual flush before program terminates
   cache.flush_all()?;

   Ok(())
}

```

## Features

#### `utf8-paths`

Use `Utf8Path` and `Utf8PathBuf` from [`camino`](https://crates.io/crates/camino)
for everything.

Using this feature adds [`camino`](https://crates.io/crates/camino) as a dependency.

This feature is disabled by default.

#### `uuid-as-key`

Implement the `Key` trait for [`uuid::Uuid`](https://docs.rs/uuid/latest/uuid/struct.Uuid.html),
so that you can use it as the key directly.

Using this feature adds [`uuid`](https://crates.io/crates/uuid) as a dependency.

This feature is enabled by default.

## Why not just rely on OS and/or filesystem level caching?

You should, when possible. It's free and requires no work on the part of the developer.

However, there are cases where you may wish for more granular control over
what things get cached and if they are cached. This is when this crate comes in handy.

Notably, this crate allows you to perform arbitrary serialisation and deserialisation
while loading and flushing, enabling you to cache an easier-to-work-with representation
of the underlying data in memory.
