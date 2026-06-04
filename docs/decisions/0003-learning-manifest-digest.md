# Decision 0003: Deterministic Trainer Manifest Digesting

Status: Accepted

`sha2` is approved for `idiolect-trainerctl` as `sha2 = "=0.11.0"` using the SHA-256 algorithm.

Trainer manifest digests are computed from a locally serialized manifest input using:
- deterministic candidate filtering and ordering (approved candidates only, id ascending),
- canonical in-memory serde JSON serialization of that sorted manifest input,
- SHA-256 digest over the serialized bytes,
- lowercase hexadecimal encoding.

This keeps digesting deterministic and local-only for manifest construction and replay consistency.

This decision authorizes `idiolect-trainerctl` to use `sha2` privately for manifest digesting and does not change dependency usage in other crates.

`rusqlite` remains confined to `idiolect-adapter-sqlite`; no storage dependency leakage to the trainer control crate API.
