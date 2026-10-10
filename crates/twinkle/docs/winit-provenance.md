# Native window dependency

Twinkle uses the crates.io release of winit 0.31. Its upstream repository is `https://github.com/rust-windowing/winit`, and winit is licensed under Apache-2.0. The dependency is resolved normally by Cargo; Twinkle carries no vendored window-system source or downstream patch.

winit 0.31 provides the cross-platform data-transfer API used by Twinkle for native file dragging. Twinkle publishes URI-list payloads through `ActiveEventLoop::start_drag`, accepts incoming URI-list transfers, reports the negotiated Copy or Move action to the application, and converts received URIs into file paths. Wayland seat, serial, surface, and data-device ownership remain inside winit.

Older Nickel code patched winit 0.30.13 for this behavior and for multiple Windows event loops. The data-transfer patches became unnecessary with winit 0.31. Nickel removed the multiple-loop requirement by launching file windows in child processes and rendering context menus in their owning window.

The renamed-dependency fixture is a separate Cargo workspace, so its compilation verifies that the engine's window dependency resolves without root patch configuration. It also verifies the `ui!` and `id!` macros when the Cargo dependency is named `lights`.
