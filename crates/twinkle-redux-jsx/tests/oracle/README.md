# Test-only upstream oracle

This isolated harness pins upstream Redux and Redux Toolkit solely to generate
behavioral reference traces. Neither package is a workspace dependency or an
application runtime dependency. `package-lock.json` records npm integrity
digests. Run `npm ci && npm run trace` in this directory when intentionally
refreshing `expected.json`; normal Rust builds never invoke npm.
