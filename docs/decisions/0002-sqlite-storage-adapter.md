# Decision 0002: SQLite Storage Adapter Dependencies

Status: Accepted

The SQLite storage adapter uses the following exact dependency versions:

- `rusqlite = { version = "=0.40.0", default-features = false, features = ["bundled"] }`
- `sha2 = "=0.11.0"`

`rusqlite` and `sha2` are adapter-private dependencies only. SQLite and checksum crate types remain confined to `idiolect-adapter-sqlite` and do not appear in `idiolect-core`, `idiolect-ports`, or `idiolect-application` public APIs.
