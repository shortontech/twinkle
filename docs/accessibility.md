# Accessibility capabilities

Twinkle builds a backend-neutral semantic tree alongside layout and paint output. Components expose
roles, labels, values, selection, focus, disabled state, actions, and geometry through
`AccessibilityNode`. `UiHost::accessibility_nodes` exposes that tree, and hosts can send bounded
semantic actions back through `HostEvent::Accessibility`.

This is an accessibility *model*, not by itself a native screen-reader bridge. The default winit
runtime does not currently publish the tree to Windows UI Automation, macOS NSAccessibility, or
Linux AT-SPI, and those integrations are not tested in CI. Applications using the default runtime
should not claim screen-reader support solely because semantic nodes exist. An embedding host may
adapt the tree and action channel to a platform bridge, but owns synchronization, focus, lifecycle,
and platform conformance testing.

Keyboard navigation and controller navigation are separate input facilities. They are useful to
many users, but they do not replace assistive-technology integration.

When adding or reviewing a component:

- expose a stable semantic role and accessible name;
- keep visual disabled, selected, expanded, and error states synchronized with semantic state;
- ensure every pointer action has the appropriate keyboard and semantic action;
- exclude decorative images and label meaningful icons;
- test the semantic snapshot as well as pixels;
- document any platform bridge separately, including the versions and assistive tools tested.
