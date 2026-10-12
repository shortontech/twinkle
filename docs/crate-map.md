# Workspace crate map

Twinkle is split by responsibility. Most applications need only `twinkle-ui`.

| Package | Intended audience | Published |
| --- | --- | --- |
| `twinkle-ui` | Application authors and native embedders; optional Redux owner adapter | Yes |
| `twinkle-input` | Backend and input-adapter authors | Yes |
| `twinkle-testkit` | Application tests and fixture tooling | Yes |
| `twinkle-theme` | Hosts sharing portable appearance values | Yes |
| `twinkle-protocol` | Native-tree transport implementers | Yes |
| `twinkle-render-assets` | Renderer implementers sharing CPU assets | Yes |
| `twinkle-presentation` | Protocol/native-presentation integrators | Yes |
| `twinkle-jsx-runtime` | Hosts deliberately embedding the V8/JSX runtime | Yes |
| `twinkle-redux` | Native transactional Redux-style stores and UI bindings | No |
| `twinkle-redux-jsx` | Optional `twinkle/redux` TSX adapter over native stores | Yes |
| `twinkle-macros` | Implementation dependency of `twinkle-ui` | Yes, not for direct use |
| `twinkle-workbench` | Repository fixture development | No |

Secondary crates are lower-level contracts, not a menu of dependencies every application should
add. Start from `twinkle-ui`; introduce another crate only when its stated boundary matches code
your application or host owns. The JSX runtime is intentionally separate and brings Oxc and V8.
