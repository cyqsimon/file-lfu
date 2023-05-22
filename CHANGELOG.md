## Unreleased

- Added feature `utf8-paths`

## 1.3.0

- Added `Debug` trait bound on `Key`.
- Impl `Display` for `Error`.
- Added mutation tracking.
  - Items are now only flushed they are newly added, or when mutations occur.
  - This change should be transparent to the users of this crate.

## v1.2.1

Start of history.
