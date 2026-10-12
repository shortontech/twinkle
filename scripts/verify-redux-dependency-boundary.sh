#!/usr/bin/env bash
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$workspace_root/Cargo.toml"
tree="$(cargo tree --manifest-path "$manifest" -p twinkle-redux --edges normal)"
no_default_tree="$(cargo tree --manifest-path "$manifest" -p twinkle-redux --no-default-features --edges normal)"
feature_tree="$(cargo tree --manifest-path "$manifest" -p twinkle-redux --all-features --edges features)"
native_binding_tree="$(cargo tree --manifest-path "$manifest" -p twinkle-ui --no-default-features --features redux --edges normal)"
adapter_tree="$(cargo tree --manifest-path "$manifest" -p twinkle-redux-jsx --edges normal)"

if grep -Eiq 'twinkle-jsx-runtime|(^|[-_])oxc|(^|[-_])v8|react|npm' <<<"$tree"; then
    printf '%s\n' "$tree" >&2
    echo "twinkle-redux native dependency boundary includes a forbidden runtime" >&2
    exit 1
fi

if [[ "$tree" != "twinkle-redux v"* ]]; then
    printf '%s\n' "$tree" >&2
    echo "twinkle-redux dependency tree did not identify the expected crate" >&2
    exit 1
fi

if [[ "$no_default_tree" != "$tree" || "$feature_tree" != "twinkle-redux v"* ]]; then
    printf '%s\n' "$no_default_tree" "$feature_tree" >&2
    echo "twinkle-redux feature variants changed the native dependency closure" >&2
    exit 1
fi

if ! grep -q '^twinkle-ui v' <<<"$native_binding_tree" || ! grep -q 'twinkle-redux v' <<<"$native_binding_tree"; then
    printf '%s\n' "$native_binding_tree" >&2
    echo "twinkle-ui native Redux binding tree is incomplete" >&2
    exit 1
fi
if grep -Eiq 'twinkle-jsx-runtime|(^|[-_])oxc|(^|[-_])v8|react|npm' <<<"$native_binding_tree"; then
    printf '%s\n' "$native_binding_tree" >&2
    echo "twinkle-ui native Redux binding pulled in a forbidden JSX/npm runtime" >&2
    exit 1
fi

if ! grep -q '^twinkle-redux-jsx v' <<<"$adapter_tree" || ! grep -q 'twinkle-jsx-runtime v' <<<"$adapter_tree"; then
    printf '%s\n' "$adapter_tree" >&2
    echo "twinkle-redux-jsx adapter tree is incomplete" >&2
    exit 1
fi

printf '%s\n' \
    '[core default]' "$tree" \
    '[core no-default-features]' "$no_default_tree" \
    '[core all-feature unification]' "$feature_tree" \
    '[native twinkle binding: no default UI features + redux]' "$native_binding_tree" \
    '[optional JSX adapter]' "$adapter_tree"
