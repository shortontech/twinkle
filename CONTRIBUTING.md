# Contributing to Twinkle

Bug reports, focused design discussions, documentation fixes, and code contributions are welcome.
For substantial API changes, open an issue before implementation so the compatibility and host
authority implications can be discussed.

## Development setup

Install Rust 1.96 or newer. On Ubuntu, install the same native packages used by CI:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libudev-dev libwayland-dev libxkbcommon-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

Before submitting a change, run:

```sh
cargo fmt --check --all
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo doc -p twinkle-ui --no-deps
./scripts/check-doc-coverage.sh
```

Keep changes focused, add tests for observable behavior, and update public documentation when an
API or supported workflow changes. Public APIs should explain invariants, failure modes, and include
a small example when usage is not obvious. Commit messages should describe the user-visible reason
for the change. The documentation coverage check forces the missing-doc lint through scoped legacy
allowances and prevents the known debt from growing; lower its bound whenever that debt is reduced.

## Specifications

Active design specifications live in `specs/`. Commit a specification when it is written. Once its
work is complete, move it to `specs/done/` in the completing change.

## Pull requests

Describe the problem, the chosen behavior, compatibility impact, and verification performed. Avoid
mixing unrelated refactors with behavioral changes. Contributions are accepted under the
repository's MIT OR Apache-2.0 dual license.
