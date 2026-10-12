//! Stable component identity and runtime-owned interaction state.

use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hash, Hasher},
    num::NonZeroU64,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use crate::{DismissReason, FocusReturn, OverlayAnchor, OverlayId};
use crate::{DocumentSelection, SelectionDocument, TextEditor};

/// The input family responsible for the most recent authoritative interaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InputModality {
    /// Keyboard navigation or text input.
    #[default]
    Keyboard,
    /// Mouse, pen, or touch-derived pointer input.
    Pointer,
    /// Game controller navigation.
    Controller,
    /// An accessibility adapter action.
    Accessibility,
}

/// Stable component identity used to retain state across declarative rebuilds.
#[derive(Clone)]
pub struct UiId(Arc<UiIdData>);

struct UiIdData {
    value: String,
    fingerprint: StableIdFingerprint,
    mounted: Option<MountedUiToken>,
}

#[derive(Clone, Copy)]
struct StableIdFingerprint(u64);

/// Process-local identity assigned only after a readable ID is admitted into a
/// retained mount. It is an equality fast path, never an authority or a
/// replacement for the collision-safe canonical path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MountedUiToken(NonZeroU64);

impl MountedUiToken {
    pub(crate) const fn new(mount: u32, identity: u32) -> Self {
        assert!(mount != 0, "mounted UI mount must be nonzero");
        assert!(identity != 0, "mounted UI identity must be nonzero");
        let packed = ((mount as u64) << 32) | identity as u64;
        Self(NonZeroU64::new(packed).expect("nonzero mount makes token nonzero"))
    }

    pub(crate) const fn mount(self) -> u32 {
        (self.0.get() >> 32) as u32
    }
}

fn ui_id_fingerprint(value: &str) -> StableIdFingerprint {
    static RANDOM_STATE: OnceLock<RandomState> = OnceLock::new();
    let mut hasher = RANDOM_STATE.get_or_init(RandomState::new).build_hasher();
    value.hash(&mut hasher);
    StableIdFingerprint(hasher.finish())
}

impl UiId {
    /// Creates an identity from an application-stable string.
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        crate::pipeline_trace::count_work!(4, 1);
        crate::pipeline_trace::count_work!(5, value.len() as u64);
        let fingerprint = ui_id_fingerprint(&value);
        Self(Arc::new(UiIdData {
            value,
            fingerprint,
            mounted: None,
        }))
    }

    /// Creates a descendant identity by appending a slash-separated segment.
    pub fn scoped(&self, child: impl AsRef<str>) -> Self {
        Self::new(format!("{}/{}", self.as_str(), child.as_ref()))
    }

    /// Returns the complete stable identity.
    pub fn as_str(&self) -> &str {
        &self.0.value
    }

    pub(crate) fn mounted_token(&self) -> Option<MountedUiToken> {
        self.0.mounted
    }

    pub(crate) fn with_mounted_token(self, token: MountedUiToken) -> Self {
        if self.0.mounted == Some(token) {
            return self;
        }
        let data = match Arc::try_unwrap(self.0) {
            Ok(mut data) => {
                data.mounted = Some(token);
                data
            }
            Err(data) => UiIdData {
                value: data.value.clone(),
                fingerprint: data.fingerprint,
                mounted: Some(token),
            },
        };
        Self(Arc::new(data))
    }

    pub(crate) fn allocation_identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    pub(crate) fn retained_allocation_bytes(&self) -> usize {
        2 * std::mem::size_of::<usize>() + std::mem::size_of::<UiIdData>() + self.0.value.capacity()
    }

    pub(crate) fn retained_allocation_count(&self) -> usize {
        1 + usize::from(self.0.value.capacity() != 0)
    }
}

impl std::fmt::Debug for UiId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("UiId").field(&self.as_str()).finish()
    }
}

impl PartialEq for UiId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.mounted.is_some() && self.0.mounted == other.0.mounted)
            || self.as_str() == other.as_str()
    }
}

impl Eq for UiId {}

impl PartialOrd for UiId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UiId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for UiId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.fingerprint.0);
    }
}

impl From<&str> for UiId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for UiId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

macro_rules! integer_ids {
    ($($integer:ty),* $(,)?) => {
        $(impl From<$integer> for UiId {
            fn from(value: $integer) -> Self {
                Self::new(value.to_string())
            }
        })*
    };
}

integer_ids!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

/// The amount of frame work required after a state transition.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Invalidation {
    /// No visual work is required.
    #[default]
    None,
    /// Existing geometry can be repainted.
    Paint,
    /// Layout and paint must be resolved again.
    Layout,
    /// A repaint is required after the supplied delay.
    Scheduled(Duration),
}

impl Invalidation {
    /// Combines two requests, retaining the strongest immediate work or earliest deadline.
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Layout, _) | (_, Self::Layout) => Self::Layout,
            (Self::Paint, _) | (_, Self::Paint) => Self::Paint,
            (Self::Scheduled(left), Self::Scheduled(right)) => Self::Scheduled(left.min(right)),
            (Self::Scheduled(delay), Self::None) | (Self::None, Self::Scheduled(delay)) => {
                Self::Scheduled(delay)
            }
            (Self::None, Self::None) => Self::None,
        }
    }
}

/// Per-component ephemeral values retained by [`UiStateStore`].
#[derive(Clone, Debug, PartialEq)]
pub struct TransientState {
    /// Horizontal scroll position in logical pixels.
    pub scroll_offset_x: f32,
    /// Vertical scroll position in logical pixels.
    pub scroll_offset: f32,
    /// Current vertical inertial velocity.
    pub scroll_velocity: f32,
    /// Whether content growth should keep this scroll region pinned to its end.
    pub scroll_at_end: bool,
    /// Whether a dropdown owned by the component is open.
    pub dropdown_open: bool,
    /// Frame generation in which the dropdown was opened.
    pub dropdown_open_generation: u64,
    /// Component-defined animation progress, normally from zero to one.
    pub animation_progress: f32,
    /// Retained editing model when the component owns a text editor.
    pub editor: Option<TextEditor>,
}

impl Default for TransientState {
    fn default() -> Self {
        Self {
            scroll_offset_x: 0.0,
            scroll_offset: 0.0,
            scroll_velocity: 0.0,
            scroll_at_end: true,
            dropdown_open: false,
            dropdown_open_generation: 0,
            animation_progress: 0.0,
            editor: None,
        }
    }
}

#[derive(Clone, Debug)]
struct StateEntry {
    state: TransientState,
    last_seen_frame: u64,
}

#[derive(Clone, Debug)]
struct DurableNodeState {
    entries: HashMap<UiId, StateEntry>,
    frame: u64,
    retention_frames: u64,
    initial_focus_claimed: bool,
}

#[derive(Clone, Debug)]
struct PointerModalityState {
    hovered: Option<UiId>,
    pressed: Option<UiId>,
    captured: Option<UiId>,
    scrollbar_grab_offset: Option<f32>,
    input_modality: InputModality,
    window_focused: bool,
    observed_window_focus: Option<bool>,
    position: Option<crate::Point>,
    hover_fraction: f32,
}

/// Focus and controller-navigation state shared by the resolved tree.
#[derive(Clone, Debug, Default)]
pub struct NavigationState {
    current_target: Option<UiId>,
    controller_projection_active: bool,
    controller_pane: Option<UiId>,
    controller_scope: Option<UiId>,
    target_mode: WidgetTargetMode,
    controller_retained_focus: HashMap<UiId, UiId>,
}

/// Operation currently owned by the authoritative focused widget.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WidgetTargetMode {
    /// Traversing between controls.
    #[default]
    Navigation,
    /// Editing text in the focused control.
    TextEditing,
    /// Adjusting a continuous value such as a slider.
    ValueAdjustment,
}

impl NavigationState {
    /// Returns the authoritative focused target.
    pub fn focused(&self) -> Option<&UiId> {
        self.current_target.as_ref()
    }
    /// Returns the focused target when controller projection is active.
    pub fn controller_selected(&self) -> Option<&UiId> {
        self.controller_projection_active
            .then_some(self.current_target.as_ref())
            .flatten()
    }
    pub(crate) fn controller_projection_active(&self) -> bool {
        self.controller_projection_active
    }
    pub(crate) fn set_controller_projection_active(&mut self, active: bool) -> Invalidation {
        if self.controller_projection_active == active || (active && self.current_target.is_none())
        {
            Invalidation::None
        } else {
            self.controller_projection_active = active;
            Invalidation::Paint
        }
    }
    /// Returns the active top-level controller pane.
    pub fn controller_pane(&self) -> Option<&UiId> {
        self.controller_pane.as_ref()
    }
    /// Returns the innermost active controller navigation scope.
    pub fn controller_scope(&self) -> Option<&UiId> {
        self.controller_scope.as_ref()
    }
    /// Reports whether a controller is adjusting a value.
    pub fn controller_editing(&self) -> bool {
        self.target_mode == WidgetTargetMode::ValueAdjustment
    }
    /// Returns the focused widget's current operation mode.
    pub fn target_mode(&self) -> WidgetTargetMode {
        self.target_mode
    }
    /// Returns the last controller-focused descendant retained for `scope`.
    pub fn retained_controller_focus(&self, scope: &UiId) -> Option<&UiId> {
        self.controller_retained_focus.get(scope)
    }
    pub(crate) fn retain_controller_focus(&mut self, scope: UiId, selected: UiId) {
        self.controller_retained_focus.insert(scope, selected);
    }
    pub(crate) fn forget_controller_focus(&mut self, scope: &UiId) {
        self.controller_retained_focus.remove(scope);
    }
    pub(crate) fn set_controller_selected(&mut self, id: Option<UiId>) -> Invalidation {
        let target_changed = id
            .as_ref()
            .is_some_and(|id| self.current_target.as_ref() != Some(id));
        let changed = self.controller_projection_active != id.is_some()
            || id
                .as_ref()
                .is_some_and(|id| self.current_target.as_ref() != Some(id));
        self.controller_projection_active = id.is_some();
        if let Some(id) = id {
            self.current_target = Some(id);
        }
        if target_changed {
            self.target_mode = WidgetTargetMode::Navigation;
        }
        if changed {
            Invalidation::Paint
        } else {
            Invalidation::None
        }
    }
    pub(crate) fn set_controller_pane(&mut self, id: Option<UiId>) -> Invalidation {
        replace_if_changed(&mut self.controller_pane, id, Invalidation::Paint)
    }
    pub(crate) fn set_controller_scope(&mut self, id: Option<UiId>) -> Invalidation {
        replace_if_changed(&mut self.controller_scope, id, Invalidation::Paint)
    }
    pub(crate) fn set_controller_editing(&mut self, editing: bool) -> Invalidation {
        let mode = if editing {
            WidgetTargetMode::ValueAdjustment
        } else {
            WidgetTargetMode::Navigation
        };
        if self.target_mode == mode {
            Invalidation::None
        } else {
            self.target_mode = mode;
            Invalidation::Paint
        }
    }

    pub(crate) fn set_target_mode(&mut self, mode: WidgetTargetMode) -> Invalidation {
        if self.target_mode == mode {
            Invalidation::None
        } else {
            self.target_mode = mode;
            Invalidation::Paint
        }
    }
}

#[derive(Clone, Debug)]
struct TextSelectionState {
    caret_visible: bool,
    selection_owner: Option<UiId>,
    document_selections: HashMap<UiId, DocumentSelection>,
    selection_documents: HashMap<UiId, Arc<SelectionDocument>>,
}

#[derive(Clone, Debug, Default)]
struct OverlayState {
    stack: Vec<(OverlayId, Option<UiId>)>,
    last_focus_return: Option<FocusReturn>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextContextSession {
    pub editor: UiId,
    pub document_generation: u64,
    pub selection_generation: u64,
    pub secure: bool,
    pub editable: bool,
    pub anchor: OverlayAnchor,
}

/// Runtime-owned persistent interaction state for one independently presented viewport.
///
/// Applications normally let [`UiHost`](crate::UiHost) own this store. Headless/custom hosts must
/// call [`begin_frame`](Self::begin_frame), touch live IDs during resolution, and then call
/// [`end_frame`](Self::end_frame) so removed component state can be reclaimed.
#[derive(Clone, Debug)]
pub struct UiStateStore {
    pub(crate) virtual_boundary: Option<(UiId, u64, bool)>,
    pub(crate) virtual_focus: Option<(UiId, u64, usize)>,
    // Async editor effects lease focus transitions, not only a reusable UiId.
    focus_generation: u64,
    pub(crate) clipboard_text_limit: Option<usize>,
    pub(crate) clipboard_rejected: bool,
    durable: DurableNodeState,
    pointer: PointerModalityState,
    navigation: NavigationState,
    text: TextSelectionState,
    overlays: OverlayState,
    text_context: Option<TextContextSession>,
    clipboard_text: Option<String>,
    geometry_animations: HashMap<UiId, GeometryAnimation>,
    geometry_animation_active: bool,
    geometry_animation_frame: Instant,
}

#[derive(Clone, Copy, Debug)]
struct GeometryAnimation {
    value: f32,
    target: f32,
    updated_at: Instant,
}

impl Default for UiStateStore {
    fn default() -> Self {
        Self::with_retention_frames(2)
    }
}

impl UiStateStore {
    /// Creates a store that retains unseen component state for the given number of frames.
    pub fn with_retention_frames(retention_frames: u64) -> Self {
        let now = Instant::now();
        Self {
            focus_generation: 0,
            virtual_boundary: None,
            virtual_focus: None,
            clipboard_text_limit: None,
            clipboard_rejected: false,
            durable: DurableNodeState {
                entries: HashMap::new(),
                frame: 0,
                retention_frames,
                initial_focus_claimed: false,
            },
            pointer: PointerModalityState {
                hovered: None,
                pressed: None,
                captured: None,
                scrollbar_grab_offset: None,
                input_modality: InputModality::default(),
                window_focused: true,
                observed_window_focus: None,
                position: None,
                hover_fraction: 0.5,
            },
            navigation: NavigationState::default(),
            text: TextSelectionState {
                caret_visible: true,
                selection_owner: None,
                document_selections: HashMap::new(),
                selection_documents: HashMap::new(),
            },
            overlays: OverlayState::default(),
            text_context: None,
            clipboard_text: None,
            geometry_animations: HashMap::new(),
            geometry_animation_active: false,
            geometry_animation_frame: now,
        }
    }

    pub(crate) fn set_clipboard_offer(&mut self, text: Option<&str>) {
        self.clipboard_text = text.map(ToOwned::to_owned);
    }

    pub(crate) fn clipboard_text(&self) -> Option<&str> {
        self.clipboard_text.as_deref()
    }

    pub(crate) fn text_context(&self) -> Option<&TextContextSession> {
        self.text_context.as_ref()
    }

    pub(crate) fn open_text_context(
        &mut self,
        editor: UiId,
        document_generation: u64,
        selection_generation: u64,
        secure: bool,
        anchor: OverlayAnchor,
    ) -> Invalidation {
        let menu = OverlayId::new(editor.scoped("text-context-menu"));
        self.text_context = Some(TextContextSession {
            editor: editor.clone(),
            document_generation,
            selection_generation,
            secure,
            editable: true,
            anchor,
        });
        self.open_overlay(menu, editor)
    }

    pub(crate) fn open_read_only_text_context(
        &mut self,
        target: UiId,
        document_generation: u64,
        selection_generation: u64,
        anchor: OverlayAnchor,
    ) -> Invalidation {
        let menu = OverlayId::new(target.scoped("text-context-menu"));
        self.text_context = Some(TextContextSession {
            editor: target.clone(),
            document_generation,
            selection_generation,
            secure: false,
            editable: false,
            anchor,
        });
        self.open_overlay(menu, target)
    }

    pub(crate) fn clear_text_context(&mut self) {
        self.text_context = None;
    }

    pub(crate) fn open_overlay(&mut self, id: OverlayId, invocation_target: UiId) -> Invalidation {
        let focus_return = self
            .navigation
            .current_target
            .clone()
            .or(Some(invocation_target));
        if self
            .overlays
            .stack
            .last()
            .is_some_and(|(open, _)| open == &id)
        {
            return Invalidation::None;
        }
        self.overlays.stack.push((id, focus_return));
        Invalidation::Layout
    }

    /// Returns the topmost open overlay identity.
    pub fn open_overlay_id(&self) -> Option<&OverlayId> {
        self.overlays.stack.last().map(|(id, _)| id)
    }

    /// Iterates open overlays from bottom to top and supports reverse traversal.
    pub fn overlay_stack(&self) -> impl DoubleEndedIterator<Item = &OverlayId> {
        self.overlays.stack.iter().map(|(id, _)| id)
    }

    pub(crate) fn dismiss_overlay(&mut self, reason: DismissReason) -> Invalidation {
        let Some((overlay, target)) = self.overlays.stack.pop() else {
            return Invalidation::None;
        };
        self.set_focus(target.clone());
        self.overlays.last_focus_return = Some(FocusReturn {
            overlay,
            target,
            reason,
        });
        self.text_context = None;
        Invalidation::Layout
    }

    /// Takes the pending focus restoration produced by an overlay dismissal.
    pub fn take_focus_return(&mut self) -> Option<FocusReturn> {
        self.overlays.last_focus_return.take()
    }

    pub(crate) fn claim_initial_focus(&mut self) -> bool {
        let first = !self.durable.initial_focus_claimed;
        self.durable.initial_focus_claimed = true;
        first
    }

    /// Advances the frame generation before resolving a new declaration.
    pub fn begin_frame(&mut self) {
        self.durable.frame = self.durable.frame.saturating_add(1);
    }

    /// Marks an identity live and returns its retained transient state.
    pub fn touch(&mut self, id: impl Into<UiId>) -> &mut TransientState {
        let frame = self.durable.frame;
        &mut self
            .durable
            .entries
            .entry(id.into())
            .and_modify(|entry| entry.last_seen_frame = frame)
            .or_insert_with(|| StateEntry {
                state: TransientState::default(),
                last_seen_frame: frame,
            })
            .state
    }

    /// Reclaims expired state and clears ownership referring to removed components.
    pub fn end_frame(&mut self) {
        let oldest = self
            .durable
            .frame
            .saturating_sub(self.durable.retention_frames);
        self.durable
            .entries
            .retain(|_, entry| entry.last_seen_frame >= oldest);
        self.clear_ownership_for_missing_entries();
    }

    /// Reconciles ephemeral interaction ownership against the current semantic
    /// topology. Cached per-component state may outlive a frame, but focus,
    /// capture, and controller ownership may not point at removed nodes.
    pub(crate) fn reconcile_live_targets(&mut self) {
        let live = self
            .durable
            .entries
            .iter()
            .filter(|(_, entry)| entry.last_seen_frame == self.durable.frame)
            .map(|(id, _)| id.clone())
            .collect::<std::collections::HashSet<_>>();
        for owner in [
            &mut self.pointer.hovered,
            &mut self.pointer.pressed,
            &mut self.pointer.captured,
            &mut self.navigation.controller_pane,
            &mut self.navigation.controller_scope,
            &mut self.text.selection_owner,
        ] {
            if owner.as_ref().is_some_and(|id| !live.contains(id)) {
                *owner = None;
            }
        }
        if self.current_target().is_some_and(|id| !live.contains(id)) {
            self.set_focus(None);
        }
        if self.navigation.current_target.is_none() {
            self.navigation.controller_projection_active = false;
            self.navigation.target_mode = WidgetTargetMode::Navigation;
        }
        self.navigation
            .controller_retained_focus
            .retain(|scope, target| live.contains(scope) && live.contains(target));
    }

    /// Reports whether the store currently has retained state for `id`.
    pub fn contains(&self, id: &UiId) -> bool {
        self.durable.entries.contains_key(id)
    }

    /// Returns retained state without marking it live in the current frame.
    pub fn state(&self, id: &UiId) -> Option<&TransientState> {
        self.durable.entries.get(id).map(|entry| &entry.state)
    }

    /// Marks an identity live and returns mutable retained state.
    pub fn state_mut(&mut self, id: impl Into<UiId>) -> &mut TransientState {
        self.touch(id)
    }

    /// Returns the retained editor for an identity, creating it from `initial` when absent.
    pub fn editor(&mut self, id: impl Into<UiId>, initial: impl Into<String>) -> &mut TextEditor {
        self.touch(id)
            .editor
            .get_or_insert_with(|| TextEditor::new(initial))
    }

    pub(crate) fn set_focus(&mut self, id: Option<UiId>) -> Invalidation {
        self.text.caret_visible = true;
        if self.navigation.current_target != id {
            self.focus_generation = self.focus_generation.wrapping_add(1);
            self.navigation.target_mode = WidgetTargetMode::Navigation;
        }
        if id.is_none() {
            self.navigation.controller_projection_active = false;
        }
        replace_if_changed(&mut self.navigation.current_target, id, Invalidation::Paint)
    }

    pub(crate) fn set_pointer_focus(&mut self, id: Option<UiId>) -> Invalidation {
        self.set_focus(id)
    }

    /// Monotonic generation advanced whenever focus ownership changes.
    pub fn focus_generation(&self) -> u64 {
        self.focus_generation
    }

    /// Updates the pointer-hover target and requests paint when it changes.
    pub fn set_hovered(&mut self, id: Option<UiId>) -> Invalidation {
        replace_if_changed(&mut self.pointer.hovered, id, Invalidation::Paint)
    }

    /// Updates the pressed pointer target and requests paint when it changes.
    pub fn set_pressed(&mut self, id: Option<UiId>) -> Invalidation {
        replace_if_changed(&mut self.pointer.pressed, id, Invalidation::Paint)
    }

    /// Updates pointer capture without itself requesting visual work.
    pub fn set_capture(&mut self, id: Option<UiId>) -> Invalidation {
        if id.is_none() {
            self.pointer.scrollbar_grab_offset = None;
        }
        replace_if_changed(&mut self.pointer.captured, id, Invalidation::None)
    }

    pub(crate) fn set_scrollbar_grab_offset(&mut self, offset: f32) {
        self.pointer.scrollbar_grab_offset = Some(offset);
    }

    pub(crate) fn scrollbar_grab_offset(&self) -> Option<f32> {
        self.pointer.scrollbar_grab_offset
    }

    /// Returns the read-only navigation state.
    pub fn navigation(&self) -> &NavigationState {
        &self.navigation
    }

    pub(crate) fn navigation_mut(&mut self) -> &mut NavigationState {
        &mut self.navigation
    }

    /// Returns the authoritative focused target.
    pub fn focused(&self) -> Option<&UiId> {
        self.navigation.current_target.as_ref()
    }

    /// The authoritative widget target. Keyboard and active controller
    /// projections are reconciled to this target by production transitions.
    pub fn current_target(&self) -> Option<&UiId> {
        self.focused()
    }

    /// Returns the current pointer-hover target.
    pub fn hovered(&self) -> Option<&UiId> {
        self.pointer.hovered.as_ref()
    }

    pub(crate) fn set_pointer_position(
        &mut self,
        position: crate::Point,
        hover_fraction: f32,
    ) -> Invalidation {
        let hover_fraction = hover_fraction.clamp(0.0, 1.0);
        if self.pointer.position == Some(position)
            && (self.pointer.hover_fraction - hover_fraction).abs() < f32::EPSILON
        {
            Invalidation::None
        } else {
            self.pointer.position = Some(position);
            self.pointer.hover_fraction = hover_fraction;
            Invalidation::Layout
        }
    }

    pub(crate) fn hover_fraction(&self) -> f32 {
        self.pointer.hover_fraction
    }

    pub(crate) fn animate_geometry_scale(
        &mut self,
        id: UiId,
        target: f32,
        duration_ms: f32,
    ) -> f32 {
        let animation = self
            .geometry_animations
            .entry(id)
            .or_insert(GeometryAnimation {
                value: 1.0,
                target,
                updated_at: self.geometry_animation_frame,
            });
        if duration_ms <= 0.0 {
            animation.value = target;
            animation.target = target;
            return target;
        }
        // Drive convergence from elapsed time, not an assumed frame cadence.
        // A moving pointer may retarget every layout as magnified geometry
        // shifts; retaining the previous timestamp prevents those small target
        // changes from restarting the transition indefinitely.
        let target_changed = (animation.target - target).abs() > f32::EPSILON;
        let was_settled = (animation.value - animation.target).abs() < 0.005;
        let elapsed_ms = if target_changed && was_settled {
            0.0
        } else {
            self.geometry_animation_frame
                .saturating_duration_since(animation.updated_at)
                .as_secs_f32()
                * 1_000.0
        };
        animation.updated_at = self.geometry_animation_frame;
        animation.target = target;
        let alpha = 1.0 - (-5.0 * elapsed_ms / duration_ms).exp();
        animation.value += (target - animation.value) * alpha.clamp(0.0, 1.0);
        if (animation.value - target).abs() < 0.005 {
            animation.value = target;
        } else {
            self.geometry_animation_active = true;
        }
        animation.value
    }

    pub(crate) fn begin_geometry_animation_frame(&mut self) {
        self.geometry_animation_active = false;
        self.geometry_animation_frame = Instant::now();
    }

    pub(crate) fn geometry_animation_active(&self) -> bool {
        self.geometry_animation_active
    }

    /// Returns the target on which a pointer button is currently held.
    pub fn pressed(&self) -> Option<&UiId> {
        self.pointer.pressed.as_ref()
    }

    /// Returns the target owning pointer capture.
    pub fn captured(&self) -> Option<&UiId> {
        self.pointer.captured.as_ref()
    }

    /// Returns the most recent authoritative input modality.
    pub fn input_modality(&self) -> InputModality {
        self.pointer.input_modality
    }

    pub(crate) fn set_input_modality(&mut self, modality: InputModality) -> Invalidation {
        if self.pointer.input_modality == modality {
            Invalidation::None
        } else {
            self.pointer.input_modality = modality;
            Invalidation::Paint
        }
    }

    /// Ignore duplicate lifecycle reports and an initial compositor focus loss
    /// before the window has ever received focus.
    pub(crate) fn observe_window_focus(&mut self, focused: bool) -> bool {
        let previous = self.pointer.observed_window_focus.replace(focused);
        match previous {
            Some(previous) => previous != focused,
            None => focused,
        }
    }

    /// Reports whether the native window currently has focus.
    pub fn window_focused(&self) -> bool {
        self.pointer.window_focused
    }

    pub(crate) fn set_window_focused(&mut self, focused: bool) -> Invalidation {
        let changed = self.pointer.window_focused != focused;
        self.pointer.window_focused = focused;
        let controller_changed = if focused {
            false
        } else {
            std::mem::take(&mut self.navigation.controller_projection_active)
                | self.navigation.controller_scope.take().is_some()
                | (std::mem::take(&mut self.navigation.target_mode) != WidgetTargetMode::Navigation)
        };
        if changed || controller_changed {
            Invalidation::Paint
        } else {
            Invalidation::None
        }
    }

    /// Returns the component owning the current document selection.
    pub fn selection_owner(&self) -> Option<&UiId> {
        self.text.selection_owner.as_ref()
    }

    /// Returns the retained selection for a selectable document.
    pub fn document_selection(&self, id: &UiId) -> Option<&DocumentSelection> {
        self.text.document_selections.get(id)
    }

    /// Returns or creates the retained selection for a selectable document.
    pub fn document_selection_mut(&mut self, id: impl Into<UiId>) -> &mut DocumentSelection {
        self.text.document_selections.entry(id.into()).or_default()
    }

    pub(crate) fn reconcile_document_selection(
        &mut self,
        id: UiId,
        document: Arc<SelectionDocument>,
    ) {
        let selection = self.text.document_selections.entry(id.clone()).or_default();
        if let Some(previous) = self.text.selection_documents.get(&id) {
            document.reconcile_from(previous, selection);
        } else {
            document.reconcile(selection);
        }
        self.text.selection_documents.insert(id, document);
    }

    /// Changes the component owning document selection.
    pub fn set_selection_owner(&mut self, id: Option<UiId>) -> Invalidation {
        if self.text.selection_owner == id {
            return Invalidation::None;
        }
        self.text.selection_owner = id;
        Invalidation::Paint
    }

    /// Clears every retained document selection and its owner.
    pub fn clear_document_selection(&mut self) -> Invalidation {
        let changed = self.text.selection_owner.take().is_some()
            || self
                .text
                .document_selections
                .values()
                .any(|selection| selection.anchor.is_some() || selection.focus.is_some());
        self.text.document_selections.clear();
        self.text.selection_documents.clear();
        if changed {
            Invalidation::Paint
        } else {
            Invalidation::None
        }
    }

    /// Reports whether the focused editor's blinking caret is visible.
    pub fn caret_visible(&self) -> bool {
        self.text.caret_visible
    }

    /// Makes the caret visible, requesting paint only when it changed.
    pub fn show_caret(&mut self) -> Invalidation {
        if self.text.caret_visible {
            Invalidation::None
        } else {
            self.text.caret_visible = true;
            Invalidation::Paint
        }
    }

    /// Toggles caret visibility when a target is focused.
    pub fn toggle_caret(&mut self) -> Invalidation {
        if self.navigation.current_target.is_none() {
            return Invalidation::None;
        }
        self.text.caret_visible = !self.text.caret_visible;
        Invalidation::Paint
    }

    /// Applies and clamps a vertical scroll delta for an identified region.
    pub fn scroll_by(&mut self, id: impl Into<UiId>, delta: f32, maximum: f32) -> Invalidation {
        let state = self.touch(id);
        let next = (state.scroll_offset + delta).clamp(0.0, maximum.max(0.0));
        state.scroll_at_end = next >= (maximum - 1.0).max(0.0);
        if next == state.scroll_offset {
            Invalidation::None
        } else {
            state.scroll_offset = next;
            Invalidation::Layout
        }
    }

    /// Applies and clamps a horizontal scroll delta for an identified region.
    pub fn scroll_by_x(&mut self, id: impl Into<UiId>, delta: f32, maximum: f32) -> Invalidation {
        let state = self.touch(id);
        let next = (state.scroll_offset_x + delta).clamp(0.0, maximum.max(0.0));
        if next == state.scroll_offset_x {
            Invalidation::None
        } else {
            state.scroll_offset_x = next;
            Invalidation::Layout
        }
    }

    pub(crate) fn set_dropdown_open(&mut self, id: impl Into<UiId>, open: bool) -> Invalidation {
        let state = self.touch(id);
        if state.dropdown_open == open {
            Invalidation::None
        } else {
            state.dropdown_open = open;
            Invalidation::Layout
        }
    }

    /// Creates a delayed repaint request.
    pub fn schedule_repaint(delay: Duration) -> Invalidation {
        Invalidation::Scheduled(delay)
    }

    /// Retires pointer gestures, text context, and overlays after window focus loss.
    pub fn focus_lost(&mut self) -> Invalidation {
        self.virtual_boundary = None;
        self.virtual_focus = None;
        self.focus_generation = self.focus_generation.wrapping_add(1);
        let pressed = self.pointer.pressed.take().is_some();
        let captured = self.pointer.captured.take().is_some();
        self.text_context = None;
        let overlay = self.dismiss_overlay(DismissReason::Cancel);
        let changed = pressed || captured;
        if changed {
            Invalidation::Paint.merge(overlay)
        } else {
            overlay
        }
    }

    /// Applies the same ownership retirement used for focus loss during suspension.
    pub fn suspended(&mut self) -> Invalidation {
        self.focus_lost()
    }

    /// Retires controller projection and active gestures after an input device disappears.
    pub fn device_removed(&mut self) -> Invalidation {
        let changed = std::mem::take(&mut self.navigation.controller_projection_active);
        self.focus_lost().merge(if changed {
            Invalidation::Paint
        } else {
            Invalidation::None
        })
    }

    /// Releases all retained component and interaction state for viewport destruction.
    pub fn destroy(&mut self) {
        self.virtual_boundary = None;
        self.virtual_focus = None;
        self.durable.entries.clear();
        self.set_focus(None);
        self.pointer.hovered = None;
        self.pointer.pressed = None;
        self.pointer.captured = None;
        self.navigation.current_target = None;
        self.navigation.controller_projection_active = false;
        self.navigation.controller_pane = None;
        self.navigation.controller_scope = None;
        self.navigation.target_mode = WidgetTargetMode::Navigation;
        self.pointer.input_modality = InputModality::default();
        self.navigation.controller_retained_focus.clear();
        self.text.caret_visible = true;
        self.text.selection_owner = None;
        self.text.document_selections.clear();
        self.text.selection_documents.clear();
    }

    fn clear_ownership_for_missing_entries(&mut self) {
        if self
            .focused()
            .is_some_and(|id| !self.durable.entries.contains_key(id))
        {
            self.set_focus(None);
        }
        let entries = &self.durable.entries;
        for owner in [
            &mut self.pointer.hovered,
            &mut self.pointer.pressed,
            &mut self.pointer.captured,
            &mut self.navigation.controller_pane,
            &mut self.navigation.controller_scope,
            &mut self.text.selection_owner,
        ] {
            if owner.as_ref().is_some_and(|id| !entries.contains_key(id)) {
                *owner = None;
            }
        }
        if self
            .current_target()
            .is_some_and(|id| !entries.contains_key(id))
        {
            self.set_focus(None);
        }
        if self.navigation.current_target.is_none() {
            self.navigation.controller_projection_active = false;
        }
        self.text
            .document_selections
            .retain(|id, _| self.durable.entries.contains_key(id));
        self.text
            .selection_documents
            .retain(|id, _| self.durable.entries.contains_key(id));
        self.navigation
            .controller_retained_focus
            .retain(|scope, selected| {
                self.durable.entries.contains_key(scope)
                    && self.durable.entries.contains_key(selected)
            });
    }
}

fn replace_if_changed(
    target: &mut Option<UiId>,
    value: Option<UiId>,
    invalidation: Invalidation,
) -> Invalidation {
    if *target == value {
        Invalidation::None
    } else {
        *target = value;
        invalidation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_ui_ids_share_text_and_preserve_string_identity() {
        use std::hash::{Hash, Hasher};
        let id = UiId::new("files/toolbar/location");
        let cloned = id.clone();
        let equal = UiId::new(id.as_str());
        assert!(Arc::ptr_eq(&id.0, &cloned.0));
        assert_eq!(id, equal);
        assert_eq!(id.scoped("child").as_str(), "files/toolbar/location/child");
        assert!(UiId::new("a") < UiId::new("b"));
        let mut id_hash = std::hash::DefaultHasher::new();
        let mut equal_hash = std::hash::DefaultHasher::new();
        id.hash(&mut id_hash);
        equal.hash(&mut equal_hash);
        assert_eq!(id_hash.finish(), equal_hash.finish());
        assert_eq!(std::mem::size_of::<UiId>(), std::mem::size_of::<usize>());
        assert_eq!(std::mem::size_of::<MountedUiToken>(), 8);
        assert_eq!(std::mem::size_of::<Option<MountedUiToken>>(), 8);
    }

    #[test]
    fn colliding_cached_ui_id_fingerprints_still_use_exact_text_equality() {
        let fingerprint = StableIdFingerprint(7);
        let first = UiId(Arc::new(UiIdData {
            value: "root/first".into(),
            fingerprint,
            mounted: None,
        }));
        let second = UiId(Arc::new(UiIdData {
            value: "root/second".into(),
            fingerprint,
            mounted: None,
        }));
        assert_ne!(first, second);

        let mut values = HashMap::new();
        values.insert(first.clone(), 1);
        values.insert(second.clone(), 2);
        assert_eq!(values.len(), 2);
        assert_eq!(values.get(&first), Some(&1));
        assert_eq!(values.get(&second), Some(&2));
    }

    #[test]
    fn mounted_ui_ids_preserve_textual_identity_across_mounts() {
        use std::hash::{Hash, Hasher};

        let plain = UiId::new("root/board/cell-17");
        let first = plain.clone().with_mounted_token(MountedUiToken::new(7, 11));
        let same_token =
            UiId::new("root/board/cell-17").with_mounted_token(MountedUiToken::new(7, 11));
        let foreign_mount =
            UiId::new("root/board/cell-17").with_mounted_token(MountedUiToken::new(8, 11));
        let replacement_token =
            UiId::new("root/board/cell-17").with_mounted_token(MountedUiToken::new(7, 12));

        assert_eq!(first, same_token);
        assert_eq!(first, foreign_mount);
        assert_eq!(first, replacement_token);
        assert_eq!(first, plain);

        let hashes = [first, same_token, foreign_mount, replacement_token, plain].map(|id| {
            let mut hash = std::hash::DefaultHasher::new();
            id.hash(&mut hash);
            hash.finish()
        });
        assert!(hashes.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn controller_projection_never_retains_a_second_widget_target() {
        let mut store = UiStateStore::default();
        let a = UiId::from("a");
        let b = UiId::from("b");
        store.navigation_mut().set_controller_selected(Some(a));
        store.set_pointer_focus(Some(b.clone()));
        assert_eq!(store.current_target(), Some(&b));
        assert_eq!(store.navigation().controller_selected(), Some(&b));

        store.navigation_mut().set_controller_selected(None);
        assert_eq!(store.current_target(), Some(&b));
        assert!(store.navigation().controller_selected().is_none());
    }

    #[test]
    fn focus_generation_tracks_transfer_return_and_topology_retirement() {
        let mut store = UiStateStore::default();
        let first = UiId::from("first");
        let second = UiId::from("second");
        store.touch(first.clone());
        store.touch(second.clone());
        store.set_focus(Some(first.clone()));
        let original = store.focus_generation();
        store.set_focus(Some(first.clone()));
        store.set_hovered(Some(second.clone()));
        assert_eq!(store.focus_generation(), original);
        store.set_focus(Some(second));
        store.set_focus(Some(first));
        assert_eq!(store.focus_generation(), original + 2);
        store.begin_frame();
        store.reconcile_live_targets();
        assert!(store.focused().is_none());
        assert_eq!(store.focus_generation(), original + 3);
        store.focus_lost();
        assert_eq!(store.focus_generation(), original + 4);
    }

    #[test]
    fn keyed_state_survives_insertion_reordering_and_short_absence() {
        let mut store = UiStateStore::with_retention_frames(2);
        store.begin_frame();
        store.touch("list/a").scroll_offset = 42.0;
        store.touch("list/b");
        store.end_frame();

        store.begin_frame();
        store.touch("list/new");
        store.touch("list/b");
        store.touch("list/a");
        store.end_frame();
        assert_eq!(
            store.state(&UiId::from("list/a")).unwrap().scroll_offset,
            42.0
        );

        store.begin_frame();
        store.touch("list/b");
        store.end_frame();
        store.begin_frame();
        store.touch("list/a");
        store.end_frame();
        assert_eq!(
            store.state(&UiId::from("list/a")).unwrap().scroll_offset,
            42.0
        );
    }

    #[test]
    fn abandoned_state_is_reclaimed_after_bounded_generations() {
        let mut store = UiStateStore::with_retention_frames(1);
        store.begin_frame();
        store.touch("gone");
        store.end_frame();
        store.begin_frame();
        store.end_frame();
        assert!(store.contains(&UiId::from("gone")));
        store.begin_frame();
        store.end_frame();
        assert!(!store.contains(&UiId::from("gone")));
    }

    #[test]
    fn removed_targets_lose_interaction_ownership_before_cached_state_expires() {
        let mut store = UiStateStore::with_retention_frames(4);
        store.begin_frame();
        store.touch("removed");
        store.set_focus(Some(UiId::from("removed")));
        store
            .navigation_mut()
            .set_controller_selected(Some(UiId::from("removed")));
        store.end_frame();

        store.begin_frame();
        store.touch("retained");
        store.end_frame();
        store.reconcile_live_targets();

        assert!(
            store.contains(&UiId::from("removed")),
            "derived state remains cached"
        );
        assert!(store.focused().is_none());
        assert!(store.navigation().controller_selected().is_none());
    }

    #[test]
    fn focus_loss_device_removal_and_destroy_clear_input_ownership() {
        let mut store = UiStateStore::default();
        store.begin_frame();
        for id in ["field", "button", "controller"] {
            store.touch(id);
        }
        store.set_focus(Some(UiId::from("field")));
        store.set_pressed(Some(UiId::from("button")));
        store.set_capture(Some(UiId::from("button")));
        store
            .navigation_mut()
            .set_controller_selected(Some(UiId::from("controller")));
        store.open_overlay(OverlayId::new("context"), UiId::from("button"));
        assert_eq!(store.focus_lost(), Invalidation::Layout);
        assert!(store.pressed().is_none() && store.captured().is_none());
        assert!(store.open_overlay_id().is_none());
        assert_eq!(store.device_removed(), Invalidation::Paint);
        assert!(store.navigation().controller_selected().is_none());
        store.destroy();
        assert!(store.focused().is_none() && !store.contains(&UiId::from("field")));
    }

    #[test]
    fn transient_controls_persist_and_return_minimum_invalidations() {
        let mut store = UiStateStore::default();
        store.begin_frame();
        assert_eq!(
            store.set_hovered(Some(UiId::from("button"))),
            Invalidation::Paint
        );
        assert_eq!(
            store.set_hovered(Some(UiId::from("button"))),
            Invalidation::None
        );
        assert_eq!(store.scroll_by("list", 10.0, 100.0), Invalidation::Layout);
        assert_eq!(store.scroll_by("list", 0.0, 100.0), Invalidation::None);
        assert_eq!(store.set_dropdown_open("menu", true), Invalidation::Layout);
        assert_eq!(store.set_dropdown_open("menu", true), Invalidation::None);
        let editor = store.editor("query", "hello");
        editor.move_left(false);
        editor.set_preedit("世界", Some(0..3));
        let snapshot = editor.clone();
        store.end_frame();
        store.begin_frame();
        assert_eq!(store.editor("query", "ignored"), &snapshot);
    }

    #[test]
    fn nested_transients_cancel_from_the_top_and_restore_each_focus_owner() {
        let mut store = UiStateStore::default();
        store.set_focus(Some(UiId::from("page-button")));
        store.open_overlay(OverlayId::new("popover"), UiId::from("page-button"));
        store.set_focus(Some(UiId::from("popover-action")));
        store.open_overlay(OverlayId::new("dialog"), UiId::from("popover-action"));
        assert_eq!(
            store.overlay_stack().cloned().collect::<Vec<_>>(),
            [OverlayId::new("popover"), OverlayId::new("dialog")]
        );
        store.dismiss_overlay(DismissReason::Cancel);
        assert_eq!(store.open_overlay_id(), Some(&OverlayId::new("popover")));
        assert_eq!(store.focused(), Some(&UiId::from("popover-action")));
        store.dismiss_overlay(DismissReason::Cancel);
        assert!(store.open_overlay_id().is_none());
        assert_eq!(store.focused(), Some(&UiId::from("page-button")));
    }
}
