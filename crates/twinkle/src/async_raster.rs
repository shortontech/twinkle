//! CPU raster work owned by a worker; native surfaces remain on their event loop.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

use crate::{DamageRegion, Pixel, Rect, SoftwareRenderer, backend::PaintCommand};

/// An owned display-list snapshot. The producer may immediately continue to
/// handle input and replace the pending frame for the same surface.
pub struct RasterRequest {
    pub surface: u64,
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub commands: Vec<PaintCommand>,
    pub damage: Option<Vec<Rect>>,
}

pub struct RasterResult {
    pub surface: u64,
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub damage: DamageRegion,
    pub pixels: Vec<Pixel>,
}

#[derive(Default)]
struct State {
    pending: HashMap<u64, RasterRequest>,
    order: VecDeque<u64>,
    retired: HashSet<u64>,
    completed: HashMap<u64, RasterResult>,
    acknowledged: HashMap<u64, u64>,
    stopped: bool,
    #[cfg(feature = "pipeline-attribution")]
    active: Option<(u64, u64)>,
}

#[cfg(feature = "pipeline-attribution")]
fn trace_state(event: &str, state: &State, surface: u64, revision: Option<u64>) {
    crate::pipeline_trace::event!({
        "event":"software_queue_lifetime","transition":event,"surface":surface,"revision":revision,
        "pending_generations":state.pending.values().map(|r| [r.surface,r.revision]).collect::<Vec<_>>(),
        "active_generation":state.active.map(|(s,r)| [s,r]),
        "completed_generations":state.completed.values().map(|r| [r.surface,r.revision]).collect::<Vec<_>>(),
        "pending_command_vector_capacity_bytes":state.pending.values().map(|r| r.commands.capacity()*std::mem::size_of::<PaintCommand>()).sum::<usize>(),
        "completed_pixel_capacity_bytes":state.completed.values().map(|r| r.pixels.capacity()*std::mem::size_of::<Pixel>()).sum::<usize>(),
        "stopped":state.stopped,
        "ownership":"Pending request vectors and completed result pixels are distinct owners. Active request commands and renderer framebuffer/cache are excluded here and reported in raster events. Nested command strings/images, map metadata and caller-owned taken results excluded. Capacities overlap allocator/process totals; never add to those totals. Snapshot while queue mutex held; opt-in tracing overhead only."
    });
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

/// One worker retains a renderer per surface, preserving text and image caches
/// without giving native window or compositor graphics handles to the worker.
pub struct AsyncRasterWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl AsyncRasterWorker {
    pub fn new(name: &str, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || worker_loop(worker_shared, wake))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Replace an unstarted frame for this surface. At most one frame per
    /// surface waits behind an in-progress raster job.
    pub fn submit(&self, request: RasterRequest) {
        let mut state = self.shared.state.lock().expect("raster queue poisoned");
        let surface = request.surface;
        state.retired.remove(&surface);
        if state.pending.insert(surface, request).is_none() {
            state.order.push_back(surface);
        }
        #[cfg(feature = "pipeline-attribution")]
        trace_state(
            "submit_or_replace",
            &state,
            surface,
            state.pending.get(&surface).map(|r| r.revision),
        );
        self.shared.ready.notify_one();
    }

    pub fn take_completed(&self, surface: u64) -> Option<RasterResult> {
        let mut state = self.shared.state.lock().expect("raster queue poisoned");
        let result = state.completed.remove(&surface);
        #[cfg(feature = "pipeline-attribution")]
        trace_state(
            "take_completed",
            &state,
            surface,
            result.as_ref().map(|r| r.revision),
        );
        result
    }

    /// Confirm the native presenter accepted a result. A skipped result makes
    /// the next partial damage relative to pixels that were never presented.
    pub fn acknowledge(&self, surface: u64, revision: u64) {
        let mut state = self.shared.state.lock().expect("raster queue poisoned");
        state.acknowledged.insert(surface, revision);
        #[cfg(feature = "pipeline-attribution")]
        trace_state("acknowledge", &state, surface, Some(revision));
    }

    /// Drop queued work and retained raster caches for a closed surface.
    pub fn retire(&self, surface: u64) {
        let mut state = self.shared.state.lock().expect("raster queue poisoned");
        state.pending.remove(&surface);
        state.completed.remove(&surface);
        state.acknowledged.remove(&surface);
        state.retired.insert(surface);
        #[cfg(feature = "pipeline-attribution")]
        trace_state("retire", &state, surface, None);
        self.shared.ready.notify_one();
    }
}

impl Drop for AsyncRasterWorker {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().expect("raster queue poisoned");
            state.stopped = true;
            state.pending.clear();
            state.order.clear();
            #[cfg(feature = "pipeline-attribution")]
            trace_state("stop", &state, 0, None);
            self.shared.ready.notify_one();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker_loop(shared: Arc<Shared>, wake: impl Fn()) {
    let mut renderers = HashMap::<u64, SoftwareRenderer>::new();
    let mut rendered_revisions = HashMap::<u64, u64>::new();
    loop {
        let request = {
            let mut state = shared.state.lock().expect("raster queue poisoned");
            while !state.stopped && state.order.is_empty() && state.retired.is_empty() {
                state = shared.ready.wait(state).expect("raster queue poisoned");
            }
            if state.stopped {
                crate::pipeline_trace::event!({"event":"software_worker_exit","retained_renderers_released":renderers.len(),"framebuffer_capacity_bytes_released":renderers.values().map(SoftwareRenderer::pixel_capacity_bytes).sum::<usize>(),"boundary":"Worker-local renderers drop on return; caches and driver/allocator retention excluded from framebuffer capacity. Queue-owned completed results drop later with shared state."});
                return;
            }
            for surface in state.retired.drain() {
                crate::pipeline_trace::event!({"event":"software_renderer_release","surface":surface,"framebuffer_capacity_bytes":renderers.get(&surface).map(SoftwareRenderer::pixel_capacity_bytes),"boundary":"Retired worker renderer/cache owner removed immediately after event; capacity is not process RSS freed."});
                renderers.remove(&surface);
                rendered_revisions.remove(&surface);
            }
            let next = state.order.pop_front().and_then(|id| {
                state.pending.remove(&id).map(|request| {
                    let acknowledged = state.acknowledged.get(&id).copied();
                    (request, acknowledged)
                })
            });
            #[cfg(feature = "pipeline-attribution")]
            if let Some((request, _)) = &next {
                state.active = Some((request.surface, request.revision));
                trace_state(
                    "start_raster",
                    &state,
                    request.surface,
                    Some(request.revision),
                );
            }
            next
        };
        let Some((request, acknowledged)) = request else {
            continue;
        };
        let renderer = renderers.entry(request.surface).or_insert_with(|| {
            SoftwareRenderer::new_pixel_buffer(request.width, request.height, request.scale)
        });
        renderer.resize(request.width, request.height, request.scale);
        if rendered_revisions.get(&request.surface).copied() != acknowledged {
            renderer.invalidate();
        }
        let _raster_started = crate::pipeline_trace::clock();
        let damage = renderer.render_frame_with_damage_hint(
            &request.commands,
            request.revision,
            request.damage.as_deref(),
        );
        let _raster_us = crate::pipeline_trace::micros(_raster_started);
        let _copy_started = crate::pipeline_trace::clock();
        let pixels = if damage.is_empty() {
            Vec::new()
        } else {
            renderer.pixels().to_vec()
        };
        crate::pipeline_trace::event!({
            "event": "software_raster", "surface": request.surface, "revision": request.revision,
            "commands_consumed": request.commands.len(), "raster_us": _raster_us,
            "result_copy_us": if pixels.is_empty() { Some(0u64) } else { crate::pipeline_trace::micros(_copy_started) },
            "result_pixel_copy_skipped": pixels.is_empty(),
            "pixel_copy_service_us": if pixels.is_empty() { Some(0u64) } else { crate::pipeline_trace::micros(_copy_started) },
            "result_copy_bytes": pixels.len() * std::mem::size_of::<Pixel>(),
            "framebuffer_capacity_bytes": renderer.pixel_capacity_bytes(),
            "software_cache": format!("{:?}", renderer.software_raster_diagnostics()),
            "damage_rects": damage.rects.len(),
            "effective_damage_rectangles": damage.rects.iter().map(|r| [r.origin.x,r.origin.y,r.size.width,r.size.height]).collect::<Vec<_>>(),
            "effective_damage_rectangle_area_sum": damage.rects.iter().map(|r| f64::from(r.size.width.max(0.0))*f64::from(r.size.height.max(0.0))).sum::<f64>(),
            "damage_boundary":"Physical framebuffer damage rectangles; area sum may overlap, not an exact pixel-operation count. Empty damage means zero copied pixels. Raster call includes signature/cache checks even when pixel work is skipped.",
            "timing_boundary": "raster includes renderer cache/key/raster work; result copy is separate"
        });
        let result = RasterResult {
            surface: request.surface,
            revision: request.revision,
            width: request.width,
            height: request.height,
            damage,
            pixels,
        };
        rendered_revisions.insert(request.surface, request.revision);
        let mut state = shared.state.lock().expect("raster queue poisoned");
        #[cfg(feature = "pipeline-attribution")]
        {
            state.active = None;
        }
        if !state.retired.contains(&request.surface) && !state.stopped {
            state.completed.insert(request.surface, result);
            #[cfg(feature = "pipeline-attribution")]
            trace_state(
                "complete_or_replace",
                &state,
                request.surface,
                Some(request.revision),
            );
            drop(state);
            wake();
        } else {
            #[cfg(feature = "pipeline-attribution")]
            trace_state(
                "discard_retired_or_stopped",
                &state,
                request.surface,
                Some(request.revision),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use crate::{Rect, backend::PaintCommand};

    use super::{AsyncRasterWorker, RasterRequest};

    #[test]
    fn acknowledged_frames_survive_repeated_surface_retirement() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let worker = AsyncRasterWorker::new("raster-lifetime-test", move || {
            wake_tx.send(()).unwrap();
        })
        .unwrap();
        for cycle in 0..100u64 {
            let surface = 100 + cycle;
            worker.submit(RasterRequest {
                surface,
                revision: 1,
                width: 8,
                height: 6,
                scale: 1.0,
                commands: vec![PaintCommand::Fill {
                    rect: Rect::new(0.0, 0.0, 8.0, 6.0),
                    color: 0x123456,
                }],
                damage: None,
            });
            wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let result = worker.take_completed(surface).unwrap();
            assert_eq!((result.surface, result.revision), (surface, 1));
            assert_eq!(result.pixels.len(), 48);
            assert!(!result.damage.is_empty());
            worker.acknowledge(surface, result.revision);
            drop(result);
            worker.retire(surface);
            assert!(worker.take_completed(surface).is_none());
        }
    }

    #[test]
    fn worker_rasters_owned_frames_and_retires_surface_state() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let worker = AsyncRasterWorker::new("raster-test", move || {
            wake_tx.send(()).unwrap();
        })
        .unwrap();
        worker.submit(RasterRequest {
            surface: 7,
            revision: 1,
            width: 8,
            height: 6,
            scale: 1.0,
            commands: vec![PaintCommand::Fill {
                rect: Rect::new(0.0, 0.0, 8.0, 6.0),
                color: 0x123456,
            }],
            damage: None,
        });
        wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = worker.take_completed(7).unwrap();
        assert_eq!((result.width, result.height, result.revision), (8, 6, 1));
        assert!(!result.damage.is_empty());
        assert_eq!((result.pixels[0].r, result.pixels[0].g), (0x12, 0x34));
        // The first result was consumed but never presented. The next frame
        // must repaint its entire buffer, even if its command delta is small.
        worker.submit(RasterRequest {
            surface: 7,
            revision: 2,
            width: 8,
            height: 6,
            scale: 1.0,
            commands: vec![
                PaintCommand::Fill {
                    rect: Rect::new(0.0, 0.0, 8.0, 6.0),
                    color: 0x123456,
                },
                PaintCommand::Fill {
                    rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                    color: 0x654321,
                },
            ],
            damage: None,
        });
        wake_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = worker.take_completed(7).unwrap();
        assert_eq!(
            result.damage.rects.as_slice(),
            &[Rect::new(0.0, 0.0, 8.0, 6.0)]
        );
        worker.retire(7);
        assert!(worker.take_completed(7).is_none());
    }
}
