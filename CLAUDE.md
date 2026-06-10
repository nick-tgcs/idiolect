# Idiolect — repository rules

## Test-Driven Development is mandatory (read [CONTRIBUTING.md](CONTRIBUTING.md))

This repo is **strictly TDD**. The rule, in full:

> **No production code changes without a failing test first.** Something breaks →
> you fix it by writing a test that *fails* on the current code, then make it pass.
> Features work the same way: red → green → refactor.

Every behaviour must be covered at **all three levels** — **unit**, **integration**,
and **end-to-end** — unless a level is genuinely unreachable (e.g. a GUI/desktop
boundary with no headless seam), in which case state the reason in the test file.

Before AND after every change, both gates must be green — never break the suite:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

When touching the IBus engine, also run the gated e2e:

```sh
cargo test -p idiolect-ibus --features ibus-engine -- --ignored ibus_engine_e2e
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the loop, the three-level definitions,
test templates, and a worked example.
