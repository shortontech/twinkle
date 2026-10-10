#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

# Package leaves before their consumers. `--no-verify` is intentional: package
# verification cannot resolve unpublished workspace packages from crates.io.
# The external fixture below performs a real compile against the local public API.
leaf_packages=(
  twinkle-input
  twinkle-macros
  twinkle-protocol
  twinkle-render-assets
  twinkle-theme
)

dependent_packages=(
  twinkle-ui
  twinkle-jsx-runtime
  twinkle-testkit
  twinkle-presentation
)

for package in "${leaf_packages[@]}"; do
  cargo package --locked --no-verify --package "$package"
done

# Cargo insists that versioned dependencies already exist in the registry while
# assembling a package. Until the workspace's leaf crates have had their first
# release, listing the dependent packages is the strongest offline manifest and
# inclusion check available in CI. Release automation packages these crates in
# this order after publishing their dependencies.
for package in "${dependent_packages[@]}"; do
  cargo package --locked --list --package "$package" >/dev/null
done

cargo check \
  --manifest-path crates/twinkle/tests/fixtures/external-consumer/Cargo.toml

# Exercise Cargo's Git-source packaging and dependency resolution, not only the checkout's path
# dependency. CI runs from a committed revision, so this verifies the exact source users receive.
revision="$(git rev-parse HEAD)"
git_consumer="$(mktemp -d)"
trap 'rm -rf "$git_consumer"' EXIT
mkdir -p "$git_consumer/src"
cat >"$git_consumer/Cargo.toml" <<EOF
[package]
name = "twinkle-git-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
twinkle-ui = { git = "file://$repository_root", rev = "$revision", default-features = false }
EOF
cat >"$git_consumer/src/main.rs" <<'EOF'
use twinkle_ui::prelude::*;

fn main() {
    let _view = Column::<()>::new().child(Text::new("Git consumer"));
}
EOF
cargo check --manifest-path "$git_consumer/Cargo.toml"
