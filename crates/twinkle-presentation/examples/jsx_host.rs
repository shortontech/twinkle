use serde_json::json;
use twinkle::{Application, View, ViewContext};
use twinkle_jsx_runtime::{JsxModuleGraph, JsxRuntime, ModuleSource, ScheduledPatch};
use twinkle_presentation::{
    components::{PluginImages, PluginMessage, RetainedPanelTree, render_retained_panel},
    css::StyleSheet,
};
use twinkle_protocol::SurfaceBounds;

struct JsxApp {
    runtime: JsxRuntime,
    accepted: RetainedPanelTree,
    bounds: SurfaceBounds,
    stylesheet: StyleSheet,
    images: PluginImages,
}

impl JsxApp {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let graph = JsxModuleGraph::new(
            "app.tsx",
            [
                ModuleSource {
                    path: "app.tsx",
                    source: include_str!("jsx_host.tsx"),
                },
                ModuleSource {
                    path: "jsx_host.css",
                    source: include_str!("jsx_host.css"),
                },
            ],
        )?;
        let stylesheet = StyleSheet::compile(&graph.stylesheet()?)?;
        let bounds = SurfaceBounds::from_json(
            r#"{"surfaces":[{"id":"main","kind":"window","width":640,"height":360}]}"#,
        )?;
        let mut runtime = JsxRuntime::new_modules(&graph, None)?;
        let accepted =
            render_retained_panel(&mut runtime, &bounds, Some("main"), "__twinkleRender()", 1)?;
        accepted.validate_candidate(accepted.source(), &bounds, Some("main"), &stylesheet)?;
        Ok(Self {
            runtime,
            accepted,
            bounds,
            stylesheet,
            images: Default::default(),
        })
    }

    fn dispatch(&mut self, action: usize) -> Result<Option<u64>, String> {
        self.runtime.begin_transaction()?;
        let result: Result<(Option<RetainedPanelTree>, Option<u64>), String> = (|| {
            let candidate = if let ScheduledPatch::Patched {
                patch,
                transport_bytes,
                ..
            } = self
                .runtime
                .dispatch_batch_patched(vec![json!([action, null])], false)?
            {
                let candidate = self.accepted.clone();
                let generation = candidate
                    .generation()
                    .checked_add(1)
                    .ok_or("render generation exhausted")?;
                let candidate = candidate
                    .apply_patch_owned(
                        &patch,
                        &self.bounds,
                        Some("main"),
                        &self.stylesheet,
                        generation,
                        transport_bytes,
                    )?
                    .0;
                self.runtime.finish_patch_render(true)?;
                Some(candidate)
            } else {
                None
            };

            let effects = self.runtime.take_effects()?;
            if effects.len() > 1 {
                return Err("this example accepts at most one effect per event".into());
            }
            let count = effects
                .first()
                .map(|effect| {
                    if effect.as_object().is_none_or(|object| object.len() != 2)
                        || effect["type"] != "set-count"
                    {
                        return Err::<u64, String>("host rejected an unknown operation".into());
                    }
                    effect["value"]
                        .as_u64()
                        .ok_or_else(|| String::from("host rejected an invalid count"))
                })
                .transpose()?;
            self.runtime.finish_event(true)?;
            Ok((candidate, count))
        })();
        self.runtime.finish_transaction(result.is_ok())?;
        let (candidate, count) = result?;
        if let Some(candidate) = candidate {
            self.accepted = candidate;
        }
        Ok(count)
    }
}

impl Application for JsxApp {
    type Message = PluginMessage;

    fn update(&mut self, message: PluginMessage) {
        let action = match message {
            PluginMessage::Click(action)
            | PluginMessage::Context(action)
            | PluginMessage::Text(action, _)
            | PluginMessage::Value(action, _) => Some(action),
            PluginMessage::Button { click, .. } => Some(click),
            PluginMessage::Scroll | PluginMessage::Drag(..) | PluginMessage::Drop(..) => None,
            PluginMessage::Chart(action, _) => Some(action),
            #[cfg(feature = "charts")]
            PluginMessage::ChartInspect(..) => None,
        };
        if let Some(action) = action {
            match self.dispatch(action) {
                Ok(Some(count)) => println!("Accepted count: {count}"),
                Ok(None) => {}
                Err(error) => eprintln!("JSX event rejected: {error}"),
            }
        }
    }

    fn view(&self, _context: ViewContext) -> impl View<PluginMessage> {
        self.accepted.node().view(&self.images, &self.stylesheet)
    }

    fn title(&self) -> &str {
        "Twinkle JSX host example"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (None, None) => twinkle::run(JsxApp::new()?),
        (Some("validate"), None) => {
            let mut app = JsxApp::new()?;
            let initial_view = app.accepted.node().view(&app.images, &app.stylesheet);
            let _initial_frame =
                twinkle::UiFrame::layout(initial_view, twinkle::Rect::new(0.0, 0.0, 640.0, 360.0));
            let action = app
                .accepted
                .node()
                .button_action("increment")
                .ok_or("example button was not admitted")?;
            let count = app
                .dispatch(action)?
                .ok_or("example button did not request the expected operation")?;
            if count != 1 {
                return Err("host returned an unexpected count".into());
            }
            let updated_view = app.accepted.node().view(&app.images, &app.stylesheet);
            let _updated_frame =
                twinkle::UiFrame::layout(updated_view, twinkle::Rect::new(0.0, 0.0, 640.0, 360.0));
            println!("TSX and CSS admitted; the host accepted count {count}.");
            Ok(())
        }
        _ => Err("usage: jsx_host [validate]".into()),
    }
}
