#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

# Force the lint through the temporary module-level legacy allowances. This is a ratchet, not a
# claim that the remaining warnings are acceptable documentation. Lower the bound whenever public
# items receive useful rustdoc; never raise it to admit a regression.
maximum_missing_docs=2515
report="$(mktemp)"
trap 'rm -f "$report"' EXIT

cargo rustdoc -p twinkle-ui --lib --locked -- --force-warn missing-docs 2>"$report"
missing_docs="$(grep -c '^warning: missing documentation' "$report" || true)"

if (( missing_docs > maximum_missing_docs )); then
  echo "public API documentation regressed: $missing_docs missing items (maximum $maximum_missing_docs)" >&2
  exit 1
fi

echo "public API documentation ratchet: $missing_docs missing items (maximum $maximum_missing_docs)"
