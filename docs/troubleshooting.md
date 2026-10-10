# Troubleshooting

## Enable diagnostics

Twinkle emits diagnostics through the `tracing` facade. An application must install a tracing
subscriber to see them; Twinkle does not choose a global logger. During development, add
`tracing-subscriber` and initialize it before `run`:

```rust,no_run
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```

Then run with `RUST_LOG=twinkle=debug,winit=info`. Do not enable verbose input logging in production
without reviewing it for user-entered text and paths.

## Linux build failures

An error mentioning `libudev`, Wayland, XKB, X11, or OpenGL usually means the development package
listed in [Getting started](getting-started.md) is absent. Controller-free applications can disable
the default `native-controller` feature to avoid the gilrs/libudev dependency.

## No window or immediate startup failure

Confirm that either `WAYLAND_DISPLAY` or `DISPLAY` is valid on Linux and that the process can reach
that session. Capture the complete error returned by `run`; do not discard the `Result` in `main`.
Remote, container, and service sessions often lack a native display even when compilation succeeds.

## Clipboard and drag-and-drop

Clipboard initialization is best-effort. On Linux, confirm the process is attached to the expected
desktop session and that the compositor supports the requested data-transfer protocol. Include the
session type (`echo $XDG_SESSION_TYPE`) in a bug report.

## Controller input

Native discovery is optional and failures are reported through tracing. On Linux, verify `/dev/input`
permissions and libudev availability. Hosted applications should use hosted controller delivery
instead of starting a second native reader.

## Reporting a reproducible problem

Include the exact Twinkle revision, `rustc --version`, OS and display-session type, enabled Cargo
features, the smallest reproducer, and relevant tracing output. Follow the private process in
[`SECURITY.md`](../SECURITY.md) for security issues.
