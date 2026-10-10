#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

# Package leaves before their consumers. `--no-verify` is intentional: package
# verification cannot resolve unpublished workspace packages from crates.io.
# The external fixture below performs a real compile against the local public API.
packages=(
  twinkle-input
  twinkle-macros
  twinkle-protocol
  twinkle-render-assets
  twinkle-theme
  twinkle
  twinkle-jsx-runtime
  twinkle-testkit
  twinkle-presentation
  twinkle-workbench
)

for package in "${packages[@]}"; do
  cargo package --locked --no-verify --package "$package"
done

cargo check \
  --manifest-path crates/twinkle/tests/fixtures/external-consumer/Cargo.toml
