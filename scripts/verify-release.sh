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
  twinkle
  twinkle-jsx-runtime
  twinkle-testkit
  twinkle-presentation
  twinkle-workbench
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
