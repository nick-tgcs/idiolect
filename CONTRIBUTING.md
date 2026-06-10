# Contributing to Idiolect

## Test-Driven Development is mandatory

Every change to this repository is **test-driven**. No production code is written
or changed except to make a failing test pass. This is not a guideline — it is the
rule, and it applies to features, bug fixes, and refactors alike.

### The loop

1. **Red** — write a test that expresses the desired behaviour (or reproduces the
   bug) and watch it fail. A bug fix *starts* with a test that fails because of the
   bug. If you cannot write a failing test first, you do not yet understand the
   change well enough to make it.
2. **Green** — write the minimum production code to make that test pass.
3. **Refactor** — clean up with the tests green.

A pull request that fixes a bug without a test that fails on the old code and
passes on the new code is incomplete. Reviewers should reject it.

### Every behaviour gets three levels of coverage

For each behaviour, add tests at all three levels (skip a level only with an
explicit, written reason in the test file — e.g. a desktop/GUI boundary that has
no headless seam):

- **Unit** — the smallest pure piece, no I/O. Fast, deterministic, no display,
  daemon, or D-Bus. Lives in `#[cfg(test)] mod tests` next to the code, or in the
  crate's `tests/` for pure logic.
- **Integration** — real components wired across a seam (real socket, real SQLite,
  real IPC framing), but no GUI/desktop dependency. Lives in the crate's `tests/`.
  Templates: `crates/idiolectd/tests/run_loop_smoke.rs`,
  `crates/idiolect-adapters/desktop/ibus/tests/daemon_ipc_contract.rs`.
- **End-to-end** — the real binaries wired as in production, driven from the
  outside (D-Bus, the engine binary, the daemon process). **Prefer making the test
  self-contained: have it spawn the infra it needs** (e.g. its own `dbus-daemon`
  via `--print-address --nofork`, killed on drop) so it is a *normal* `#[test]`
  that runs in the standard flow with no `#[ignore]` and no external wrapper — just
  gated behind its feature, like the binary it drives.
  `engine_inserts_history_text_on_daemon_request` does exactly this.

  Reach for `#[ignore]` ONLY when the test genuinely cannot provision its world —
  e.g. it needs a StatusNotifier host for the tray, or real hardware. When you do,
  say *why* in the `#[ignore]` reason, and remember: **an ignored test that nothing
  runs is worthless.** If it can run anywhere (even with setup), wire it into CI.
  Template + CI wiring: `crates/idiolect-adapters/desktop/ibus/tests/ibus_engine_e2e.rs`
  run by `ci/scripts/test-ibus-e2e.sh` in the `e2e` job.

### Always-green gates

Run before AND after every change — never break the suite:

```sh
cargo test --workspace            # all unit + integration tests
cargo clippy --workspace --all-targets   # must be clean (warnings are denied)
```

Gated e2e (run when touching the IBus engine):

```sh
cargo test -p idiolect-ibus --features ibus-engine -- --ignored ibus_engine_e2e
```

### Worked example: the history "Insert" bug

The tray's "Insert" once did exactly what "Copy" did — it staged the clipboard and
never typed text at the cursor. The fix was driven by tests that fail on the old
behaviour and pass on the new one:

- unit (ipc): `InsertText` round-trips through the wire framing.
- unit (daemon): `insert_entry_via_ime` writes an `InsertText` for a real entry and
  nothing for a missing id — proving Insert routes through the IME, not the clipboard.
- integration (ibus): the engine's `DaemonReader` decodes a server-sent
  `InsertText` off a real socket.
- e2e (ibus): the real engine binary turns an `InsertText` into a `CommitText`
  D-Bus signal — the actual on-screen insertion.
