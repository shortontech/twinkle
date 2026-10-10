# Support and compatibility

Twinkle is pre-1.0 software. Version `0.2` is intended for evaluation and early adopters. Minor
releases may contain breaking public-API changes; pin an exact release or Git commit and review the
[changelog](CHANGELOG.md) when upgrading.

## Platforms

The project continuously builds and tests on Ubuntu Linux and Windows with Rust 1.96. Linux CI
exercises the enabled Wayland/X11 build surface; Windows CI uses the MSVC toolchain. macOS and other
Unix platforms may work through winit but are not currently tested by this project and should not
be treated as supported targets.

Rust 1.96 is the minimum supported Rust version declared by the workspace. A newer stable compiler
may be needed temporarily if a pre-release dependency raises its own requirement; report that as a
compatibility bug.

## Getting help and reporting bugs

Use [GitHub Issues](https://github.com/shortontech/twinkle/issues) for reproducible bugs, feature
requests, and usage questions. Include:

- the Twinkle version or Git commit;
- `rustc --version` and the target operating system;
- the smallest example that reproduces the behavior;
- relevant logs and whether the session uses Wayland or X11 on Linux.

The [troubleshooting guide](docs/troubleshooting.md) explains how to enable tracing and diagnose
common native build, display, clipboard, and controller problems.

Twinkle is maintained on a best-effort basis; no response-time or long-term-support guarantee is
currently offered. Do not disclose suspected security vulnerabilities in a public issue—follow the
[security policy](SECURITY.md).
