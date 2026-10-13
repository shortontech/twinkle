//! Optional guest queue on winit's Wayland connection. It neither owns the display
//! nor dispatches winit's queue. Requests are attached before the present commit.
use crate::{NativeFrameCorrelation, NativeFramePresentation};
use std::{
    ffi::c_void,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    protocol::{wl_registry, wl_surface},
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Token {
    pub correlation: Option<NativeFrameCorrelation>,
    pub frame_generation: u64,
}
#[derive(Default)]
struct State {
    presentation: Option<wp_presentation::WpPresentation>,
    clock_id: Option<u32>,
    completed: Vec<NativeFramePresentation>,
    outstanding: usize,
}
pub(crate) struct Feedback {
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
}
impl Feedback {
    /// SAFETY: both pointers must be winit-owned live objects from the same
    /// display, and this guest queue must be dropped before either owner.
    pub unsafe fn new(display: *mut c_void, surface: *mut c_void) -> Result<Self, String> {
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);
        let id = unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface.cast()) }
            .map_err(|e| e.to_string())?;
        let surface = wl_surface::WlSurface::from_id(&connection, id).map_err(|e| e.to_string())?;
        let mut queue = connection.new_event_queue::<State>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        // Registry is no longer needed; wl_registry has no protocol destructor.
        drop(registry);
        Ok(Self {
            queue,
            state,
            surface,
        })
    }
    pub fn supported(&self) -> bool {
        self.state.presentation.is_some()
            && self.state.clock_id == Some(libc::CLOCK_MONOTONIC as u32)
    }
    pub fn request(&mut self, token: Token) -> Result<bool, String> {
        if !self.supported() {
            return Ok(false);
        }
        if self.state.outstanding >= 128 {
            return Err("Wayland presentation feedback exceeds 128 outstanding frames".into());
        }
        let presentation = self.state.presentation.as_ref().unwrap();
        presentation.feedback(&self.surface, &self.queue.handle(), token);
        self.state.outstanding += 1;
        self.queue.flush().map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub fn dispatch(&mut self) -> Result<Vec<NativeFramePresentation>, String> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| e.to_string())?;
        Ok(std::mem::take(&mut self.state.completed))
    }
}
impl Drop for Feedback {
    fn drop(&mut self) {
        if let Some(p) = self.state.presentation.take() {
            p.destroy();
        }
        let _ = self.queue.flush();
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wp_presentation" {
                state.presentation = Some(registry.bind(name, version.min(2), qh, ()));
            }
        }
    }
}
impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_id = Some(clk_id);
        }
    }
}
fn clock_timestamp(seconds: u64, nanos: u32) -> Option<Instant> {
    if nanos >= 1_000_000_000 {
        return None;
    }
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writable initialized timespec; no pointer escapes.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return None;
    }
    let now = Instant::now();
    let clock = Duration::new(
        u64::try_from(ts.tv_sec).ok()?,
        u32::try_from(ts.tv_nsec).ok()?,
    );
    map_timestamp(now, clock, Duration::new(seconds, nanos))
}
fn map_timestamp(now: Instant, clock: Duration, stamp: Duration) -> Option<Instant> {
    now.checked_sub(clock.checked_sub(stamp)?)
}
impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, Token> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        token: &Token,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let result = match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                ..
            } => {
                let seconds = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                Some(NativeFramePresentation {
                    correlation: token.correlation,
                    frame_generation: token.frame_generation,
                    presented_at: clock_timestamp(seconds, tv_nsec),
                    observed_at: Instant::now(),
                    discarded: false,
                    refresh_interval: Some(Duration::from_nanos(refresh as u64)),
                    sequence: Some(((seq_hi as u64) << 32) | seq_lo as u64),
                })
            }
            wp_presentation_feedback::Event::Discarded => Some(NativeFramePresentation {
                correlation: token.correlation,
                frame_generation: token.frame_generation,
                presented_at: None,
                observed_at: Instant::now(),
                discarded: true,
                refresh_interval: None,
                sequence: None,
            }),
            _ => None,
        };
        if let Some(result) = result {
            state.outstanding = state.outstanding.saturating_sub(1);
            state.completed.push(result);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_mapping_uses_reported_presentation_not_callback_time() {
        let now = Instant::now();
        let mapped =
            map_timestamp(now, Duration::from_secs(20), Duration::from_millis(19900)).unwrap();
        assert_eq!(now.duration_since(mapped), Duration::from_millis(100));
        assert!(map_timestamp(now, Duration::from_secs(20), Duration::from_secs(21)).is_none());
    }
}
