//! Hardware presentation for standalone Twinkle windows.

use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hash, Hasher},
    sync::Arc,
};

use crate::backend::PaintCommand;
use crate::{DamageRegion, GradientAxis, Point, Rect, SoftwareRenderer};
use raw_window_handle::{DisplayHandle, WindowHandle};
use twinkle_render_assets::ProcessFontSystem;
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) shape: vec2<f32>,
};
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) shape: vec2<f32>,
};
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@vertex fn vs_main(input: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    out.half_size = input.half_size;
    out.shape = input.shape;
    return out;
}
@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, input.uv) * input.color;
}
@fragment fn fs_shape(input: VertexOut) -> @location(0) vec4<f32> {
    let radius = min(input.shape.x, min(input.half_size.x, input.half_size.y));
    let point = (input.uv * 2.0 - vec2<f32>(1.0)) * input.half_size;
    let q = abs(point) - (input.half_size - vec2<f32>(radius));
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let outer = 1.0 - smoothstep(-0.75, 0.75, distance);
    var coverage = outer;
    if input.shape.y > 0.0 {
        let inner = 1.0 - smoothstep(-0.75, 0.75, distance + input.shape.y);
        coverage = max(0.0, outer - inner);
    }
    return input.color * coverage;
}
"#;
const MAX_TEXTURE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRANSIENT_TEXTURE_BYTES: usize = 256 * 1024 * 1024;
const MAX_RETAINED_FRAMEBUFFER_BYTES: usize = 128 * 1024 * 1024;

fn retained_framebuffer_bytes(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
        .filter(|bytes| *bytes <= MAX_RETAINED_FRAMEBUFFER_BYTES)
}

fn retained_framebuffer_configuration(
    width: u32,
    height: u32,
    scale: f32,
    format: wgpu::TextureFormat,
) -> (u32, u32, u32, wgpu::TextureFormat) {
    (width, height, scale.to_bits(), format)
}

fn text_texture_dimensions(bounds: Rect, scale: f32, maximum_dimension: u32) -> Option<(u32, u32)> {
    if !scale.is_finite()
        || scale <= 0.0
        || !bounds.size.width.is_finite()
        || !bounds.size.height.is_finite()
        || bounds.size.width <= 0.0
        || bounds.size.height <= 0.0
    {
        return None;
    }
    let width = (bounds.size.width as f64 * scale as f64).ceil();
    let height = (bounds.size.height as f64 * scale as f64).ceil();
    if width < 1.0
        || height < 1.0
        || width > f64::from(maximum_dimension)
        || height > f64::from(maximum_dimension)
    {
        return None;
    }
    let width = width as u32;
    let height = height as u32;
    let bytes = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    (bytes <= MAX_TRANSIENT_TEXTURE_BYTES).then_some((width, height))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TextureKey {
    Text(TextTextureKey),
    Image(ImageTextureKey),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TextTextureKey {
    Plain {
        text: String,
        width_bits: u32,
        height_bits: u32,
        command_scale_bits: u32,
        color: u32,
        align: crate::TextAlign,
        bold: bool,
        wrap: bool,
        presentation_scale_bits: u32,
        font_generation: u64,
    },
    Styled {
        text: String,
        spans: Vec<crate::StyledTextSpan>,
        width_bits: u32,
        height_bits: u32,
        command_scale_bits: u32,
        font_size_bits: Option<u32>,
        color: u32,
        align: crate::TextAlign,
        presentation_scale_bits: u32,
        font_generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ImageTextureKey {
    id: u16,
    generation: u64,
    high_density: bool,
    width: u32,
    height: u32,
    allocation: usize,
}

enum BorrowedTextureKey<'a> {
    Text(BorrowedTextTextureKey<'a>),
    Image(ImageTextureKey),
}

enum BorrowedTextTextureKey<'a> {
    Plain {
        text: &'a str,
        width_bits: u32,
        height_bits: u32,
        command_scale_bits: u32,
        color: u32,
        align: crate::TextAlign,
        bold: bool,
        wrap: bool,
        presentation_scale_bits: u32,
        font_generation: u64,
    },
    Styled {
        text: &'a str,
        spans: &'a [crate::StyledTextSpan],
        width_bits: u32,
        height_bits: u32,
        command_scale_bits: u32,
        font_size_bits: Option<u32>,
        color: u32,
        align: crate::TextAlign,
        presentation_scale_bits: u32,
        font_generation: u64,
    },
}

impl BorrowedTextureKey<'_> {
    fn hash_with(&self, state: &RandomState) -> u64 {
        let mut hasher = state.build_hasher();
        match self {
            Self::Text(text) => {
                0u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            Self::Image(image) => {
                1u8.hash(&mut hasher);
                image.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn equals(&self, owned: &TextureKey) -> bool {
        match (self, owned) {
            (Self::Text(left), TextureKey::Text(right)) => left.equals(right),
            (Self::Image(left), TextureKey::Image(right)) => left == right,
            _ => false,
        }
    }

    fn to_owned(&self) -> TextureKey {
        match self {
            Self::Text(text) => TextureKey::Text(text.to_owned()),
            Self::Image(image) => TextureKey::Image(*image),
        }
    }

    fn compared_bytes(&self) -> usize {
        match self {
            Self::Text(BorrowedTextTextureKey::Plain { text, .. }) => {
                text.len() + std::mem::size_of::<TextTextureKey>()
            }
            Self::Text(BorrowedTextTextureKey::Styled { text, spans, .. }) => {
                text.len() + std::mem::size_of_val(*spans) + std::mem::size_of::<TextTextureKey>()
            }
            Self::Image(_) => std::mem::size_of::<ImageTextureKey>(),
        }
    }

    fn owned_dynamic_bytes(&self) -> usize {
        match self {
            Self::Text(BorrowedTextTextureKey::Plain { text, .. }) => text.len(),
            Self::Text(BorrowedTextTextureKey::Styled { text, spans, .. }) => {
                text.len() + std::mem::size_of_val(*spans)
            }
            Self::Image(_) => 0,
        }
    }
}

impl Hash for BorrowedTextTextureKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Plain {
                text,
                width_bits,
                height_bits,
                command_scale_bits,
                color,
                align,
                bold,
                wrap,
                presentation_scale_bits,
                font_generation,
            } => {
                0u8.hash(state);
                text.hash(state);
                width_bits.hash(state);
                height_bits.hash(state);
                command_scale_bits.hash(state);
                color.hash(state);
                align.hash(state);
                bold.hash(state);
                wrap.hash(state);
                presentation_scale_bits.hash(state);
                font_generation.hash(state);
            }
            Self::Styled {
                text,
                spans,
                width_bits,
                height_bits,
                command_scale_bits,
                font_size_bits,
                color,
                align,
                presentation_scale_bits,
                font_generation,
            } => {
                1u8.hash(state);
                text.hash(state);
                spans.hash(state);
                width_bits.hash(state);
                height_bits.hash(state);
                command_scale_bits.hash(state);
                font_size_bits.hash(state);
                color.hash(state);
                align.hash(state);
                presentation_scale_bits.hash(state);
                font_generation.hash(state);
            }
        }
    }
}

impl BorrowedTextTextureKey<'_> {
    fn equals(&self, owned: &TextTextureKey) -> bool {
        match (self, owned) {
            (
                Self::Plain {
                    text: a,
                    width_bits: b,
                    height_bits: c,
                    command_scale_bits: d,
                    color: e,
                    align: f,
                    bold: g,
                    wrap: h,
                    presentation_scale_bits: i,
                    font_generation: j,
                },
                TextTextureKey::Plain {
                    text: aa,
                    width_bits: bb,
                    height_bits: cc,
                    command_scale_bits: dd,
                    color: ee,
                    align: ff,
                    bold: gg,
                    wrap: hh,
                    presentation_scale_bits: ii,
                    font_generation: jj,
                },
            ) => {
                *a == aa
                    && b == bb
                    && c == cc
                    && d == dd
                    && e == ee
                    && f == ff
                    && g == gg
                    && h == hh
                    && i == ii
                    && j == jj
            }
            (
                Self::Styled {
                    text: a,
                    spans: b,
                    width_bits: c,
                    height_bits: d,
                    command_scale_bits: e,
                    font_size_bits: f,
                    color: g,
                    align: h,
                    presentation_scale_bits: i,
                    font_generation: j,
                },
                TextTextureKey::Styled {
                    text: aa,
                    spans: bb,
                    width_bits: cc,
                    height_bits: dd,
                    command_scale_bits: ee,
                    font_size_bits: ff,
                    color: gg,
                    align: hh,
                    presentation_scale_bits: ii,
                    font_generation: jj,
                },
            ) => {
                *a == aa
                    && *b == bb
                    && c == cc
                    && d == dd
                    && e == ee
                    && f == ff
                    && g == gg
                    && h == hh
                    && i == ii
                    && j == jj
            }
            _ => false,
        }
    }

    fn to_owned(&self) -> TextTextureKey {
        match self {
            Self::Plain {
                text,
                width_bits,
                height_bits,
                command_scale_bits,
                color,
                align,
                bold,
                wrap,
                presentation_scale_bits,
                font_generation,
            } => TextTextureKey::Plain {
                text: (*text).to_owned(),
                width_bits: *width_bits,
                height_bits: *height_bits,
                command_scale_bits: *command_scale_bits,
                color: *color,
                align: *align,
                bold: *bold,
                wrap: *wrap,
                presentation_scale_bits: *presentation_scale_bits,
                font_generation: *font_generation,
            },
            Self::Styled {
                text,
                spans,
                width_bits,
                height_bits,
                command_scale_bits,
                font_size_bits,
                color,
                align,
                presentation_scale_bits,
                font_generation,
            } => TextTextureKey::Styled {
                text: (*text).to_owned(),
                spans: (*spans).to_vec(),
                width_bits: *width_bits,
                height_bits: *height_bits,
                command_scale_bits: *command_scale_bits,
                font_size_bits: *font_size_bits,
                color: *color,
                align: *align,
                presentation_scale_bits: *presentation_scale_bits,
                font_generation: *font_generation,
            },
        }
    }
}

impl TextureKey {
    fn dynamic_bytes(&self) -> usize {
        match self {
            Self::Text(TextTextureKey::Plain { text, .. }) => text.capacity(),
            Self::Text(TextTextureKey::Styled { text, spans, .. }) => {
                text.capacity() + spans.capacity() * std::mem::size_of::<crate::StyledTextSpan>()
            }
            Self::Image(_) => 0,
        }
    }
}

struct TextureCacheEntry {
    key: TextureKey,
    texture: Arc<wgpu::BindGroup>,
    resident_bytes: usize,
    texture_bytes: usize,
    key_and_metadata_bytes: usize,
    last_used: u64,
}

struct TextureCache {
    hash_builder: RandomState,
    entries: HashMap<u64, Vec<TextureCacheEntry>>,
    bytes: usize,
    peak_bytes: usize,
    clock: u64,
    evictions: u64,
    eviction_scans: u64,
    oversized: u64,
    hits: u64,
    misses: u64,
    insertions: u64,
    bytes_hashed: u64,
    bytes_compared: u64,
    byte_capacity: usize,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self {
            hash_builder: RandomState::new(),
            entries: HashMap::new(),
            bytes: 0,
            peak_bytes: 0,
            clock: 0,
            evictions: 0,
            eviction_scans: 0,
            oversized: 0,
            hits: 0,
            misses: 0,
            insertions: 0,
            bytes_hashed: 0,
            bytes_compared: 0,
            byte_capacity: MAX_TEXTURE_CACHE_BYTES,
        }
    }
}

impl TextureCache {
    fn get(&mut self, key: &BorrowedTextureKey<'_>) -> (Option<Arc<wgpu::BindGroup>>, usize) {
        self.clock = self.clock.saturating_add(1);
        self.bytes_hashed = self
            .bytes_hashed
            .saturating_add(key.compared_bytes() as u64);
        let hash = key.hash_with(&self.hash_builder);
        let Some(bucket) = self.entries.get_mut(&hash) else {
            self.misses = self.misses.saturating_add(1);
            return (None, 0);
        };
        let mut compared = 0;
        for entry in bucket {
            compared += key.compared_bytes();
            if key.equals(&entry.key) {
                entry.last_used = self.clock;
                self.hits = self.hits.saturating_add(1);
                self.bytes_compared = self.bytes_compared.saturating_add(compared as u64);
                return (Some(entry.texture.clone()), compared);
            }
        }
        self.misses = self.misses.saturating_add(1);
        self.bytes_compared = self.bytes_compared.saturating_add(compared as u64);
        (None, compared)
    }

    fn insert(
        &mut self,
        key: BorrowedTextureKey<'_>,
        texture_bytes: usize,
        texture: Arc<wgpu::BindGroup>,
    ) -> (usize, usize) {
        // Check the borrowed payload first so an oversized uncached texture
        // does not pointlessly allocate an owned text key.
        let key_and_metadata_bytes = key
            .owned_dynamic_bytes()
            .saturating_add(std::mem::size_of::<TextureCacheEntry>())
            // Charge a full bucket vector header per entry. Collisions share
            // it, so this deliberately overcounts rather than hiding control
            // allocation outside the advertised resident bound.
            .saturating_add(std::mem::size_of::<u64>())
            .saturating_add(std::mem::size_of::<Vec<TextureCacheEntry>>())
            .saturating_add(1);
        let resident_bytes = texture_bytes.saturating_add(key_and_metadata_bytes);
        if resident_bytes > self.byte_capacity {
            self.oversized = self.oversized.saturating_add(1);
            return (0, 0);
        }
        let owned = key.to_owned();
        debug_assert_eq!(owned.dynamic_bytes(), key.owned_dynamic_bytes());
        let mut evicted_entries = 0;
        let mut evicted_bytes = 0;
        while self.bytes.saturating_add(resident_bytes) > self.byte_capacity {
            self.eviction_scans = self.eviction_scans.saturating_add(self.len() as u64);
            let oldest = self
                .entries
                .iter()
                .flat_map(|(hash, bucket)| {
                    bucket
                        .iter()
                        .enumerate()
                        .map(move |(index, entry)| (*hash, index, entry.last_used))
                })
                .min_by_key(|(_, _, last_used)| *last_used);
            let Some((hash, index, _)) = oldest else {
                break;
            };
            let bucket = self.entries.get_mut(&hash).expect("selected cache bucket");
            let removed = bucket.swap_remove(index);
            self.bytes = self.bytes.saturating_sub(removed.resident_bytes);
            evicted_entries += 1;
            evicted_bytes += removed.resident_bytes;
            self.evictions = self.evictions.saturating_add(1);
            if bucket.is_empty() {
                self.entries.remove(&hash);
            }
        }
        self.clock = self.clock.saturating_add(1);
        let hash = key.hash_with(&self.hash_builder);
        self.entries
            .entry(hash)
            .or_default()
            .push(TextureCacheEntry {
                key: owned,
                texture,
                resident_bytes,
                texture_bytes,
                key_and_metadata_bytes,
                last_used: self.clock,
            });
        self.bytes += resident_bytes;
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        self.insertions = self.insertions.saturating_add(1);
        (evicted_entries, evicted_bytes)
    }

    fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    fn texture_bytes(&self) -> usize {
        self.entries
            .values()
            .flatten()
            .map(|entry| entry.texture_bytes)
            .sum()
    }

    fn key_and_metadata_bytes(&self) -> usize {
        self.entries
            .values()
            .flatten()
            .map(|entry| entry.key_and_metadata_bytes)
            .sum()
    }
}

fn borrowed_text_texture_key(
    command: &PaintCommand,
    presentation_scale: f32,
    font_generation: u64,
) -> BorrowedTextureKey<'_> {
    match command {
        PaintCommand::Text {
            bounds,
            text,
            scale,
            color,
            align,
            bold,
            wrap,
        } => BorrowedTextureKey::Text(BorrowedTextTextureKey::Plain {
            text,
            width_bits: bounds.size.width.to_bits(),
            height_bits: bounds.size.height.to_bits(),
            command_scale_bits: scale.to_bits(),
            color: *color,
            align: *align,
            bold: *bold,
            wrap: *wrap,
            presentation_scale_bits: presentation_scale.to_bits(),
            font_generation,
        }),
        PaintCommand::StyledText {
            bounds,
            text,
            spans,
            scale,
            font_size,
            color,
            align,
        } => BorrowedTextureKey::Text(BorrowedTextTextureKey::Styled {
            text,
            spans,
            width_bits: bounds.size.width.to_bits(),
            height_bits: bounds.size.height.to_bits(),
            command_scale_bits: scale.to_bits(),
            font_size_bits: font_size.map(f32::to_bits),
            color: *color,
            align: *align,
            presentation_scale_bits: presentation_scale.to_bits(),
            font_generation,
        }),
        _ => unreachable!("text cache keys only accept text paint commands"),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    half_size: [f32; 2],
    shape: [f32; 2],
}

struct GpuGraphics {
    instance: wgpu::Instance,
    display: Option<raw_window_handle::RawDisplayHandle>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: Arc<wgpu::BindGroup>,
    textures: RefCell<TextureCache>,
    text_renderer: RefCell<SoftwareRenderer>,
}

impl GpuGraphics {
    unsafe fn new(display: DisplayHandle<'_>, window: WindowHandle<'_>) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        // SAFETY: the caller guarantees the native window and display outlive
        // this temporary compatibility surface and subsequent presenters.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(display.as_raw()),
                raw_window_handle: window.as_raw(),
            })
        }
        .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: true,
        }))
        .map_err(|error| error.to_string())?;
        crate::pipeline_trace::backend_identity("gpu_adapter", || {
            format!("{:?}", adapter.get_info())
        });
        crate::pipeline_trace::event!({"event": "gpu_adapter", "adapter": format!("{:?}", adapter.get_info())});
        Self::from_adapter(instance, Some(display.as_raw()), adapter)
    }

    fn from_adapter(
        instance: wgpu::Instance,
        display: Option<raw_window_handle::RawDisplayHandle>,
        adapter: wgpu::Adapter,
    ) -> Result<Self, String> {
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Twinkle standalone GPU"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|error| error.to_string())?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Nickel shell primitives"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Nickel shell texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Nickel shell texture sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = upload_texture(&device, &queue, &texture_layout, &sampler, 1, 1, &[255; 4]);
        Ok(Self {
            instance,
            display,
            adapter,
            device,
            queue,
            shader,
            texture_layout,
            sampler,
            white,
            textures: RefCell::new(TextureCache::default()),
            text_renderer: RefCell::new(SoftwareRenderer::new_pixel_buffer(1, 1, 1.0)),
        })
    }

    #[cfg(test)]
    fn new_headless() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: true,
        }))
        .map_err(|error| error.to_string())?;
        Self::from_adapter(instance, None, adapter)
    }

    fn upload_cached_texture(
        &self,
        key: BorrowedTextureKey<'_>,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Arc<wgpu::BindGroup> {
        let texture = upload_texture(
            &self.device,
            &self.queue,
            &self.texture_layout,
            &self.sampler,
            width,
            height,
            pixels,
        );
        let texture_bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        let mut cache = self.textures.borrow_mut();
        let (evicted_entries, _evicted_bytes) = cache.insert(key, texture_bytes, texture.clone());
        if evicted_entries > 0 {
            let _resident_entries = cache.len();
            crate::pipeline_trace::event!({"event": "texture_cache_eviction", "cause": "capacity",
                "removed_entries": evicted_entries, "removed_resident_bytes": _evicted_bytes,
                "resident_entries": _resident_entries, "resident_bytes": cache.bytes});
        }
        texture
    }

    fn text_texture(
        &self,
        command: &PaintCommand,
        bounds: Rect,
        scale: f32,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let (width, height) =
            text_texture_dimensions(bounds, scale, self.device.limits().max_texture_dimension_2d)?;
        let _key_started = crate::pipeline_trace::clock();
        let font_generation = ProcessFontSystem::new().generation();
        let key = borrowed_text_texture_key(command, scale, font_generation);
        let _key_build_us = crate::pipeline_trace::micros(_key_started);
        let (cached, _compared_bytes) = self.textures.borrow_mut().get(&key);
        if let Some(texture) = cached {
            crate::pipeline_trace::event!({"event": "text_lookup", "hit": true,
                "key_build_us": _key_build_us, "lookup_key_allocations": 0, "lookup_key_owned_bytes": 0,
                "bytes_hashed": key.compared_bytes(), "bytes_compared": _compared_bytes, "paint_command_clones": 0});
            return Some(texture);
        }
        crate::pipeline_trace::event!({"event": "text_lookup", "hit": false,
            "key_build_us": _key_build_us, "lookup_key_allocations": 0, "lookup_key_owned_bytes": 0,
            "bytes_hashed": key.compared_bytes(), "bytes_compared": _compared_bytes, "paint_command_clones": 0});
        let mut local = command.clone();
        match &mut local {
            PaintCommand::Text { bounds, .. } | PaintCommand::StyledText { bounds, .. } => {
                bounds.origin.x = 0.0;
                bounds.origin.y = 0.0;
            }
            _ => unreachable!(),
        }
        let _text_raster_started = crate::pipeline_trace::clock();
        let mut renderer = self.text_renderer.borrow_mut();
        renderer.resize(width, height, scale);
        renderer.invalidate();
        renderer.render(&[local]);
        let _text_raster_us = crate::pipeline_trace::micros(_text_raster_started);
        let _rgba_started = crate::pipeline_trace::clock();
        let mut bytes = Vec::with_capacity(renderer.pixels().len() * 4);
        for pixel in renderer.pixels() {
            bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b, pixel.a]);
        }
        crate::pipeline_trace::event!({"event": "text_raster",
            "raster_us": _text_raster_us, "rgba_conversion_us": crate::pipeline_trace::micros(_rgba_started),
            "rasterized_pixels": u64::from(width) * u64::from(height), "rgba_copy_bytes": bytes.len(),
            "scratch_pixel_capacity_bytes": renderer.pixel_capacity_bytes()});
        if renderer.pixel_capacity_bytes() > 8 * 1024 * 1024 {
            renderer.suspend();
        }
        drop(renderer);
        Some(self.upload_cached_texture(key, width, height, &bytes))
    }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Arc<wgpu::BindGroup> {
    let _texture_upload_started = crate::pipeline_trace::clock();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Twinkle standalone cached texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    crate::pipeline_trace::event!({"event": "texture_upload", "width": width, "height": height,
        "uploaded_bytes": pixels.len(), "create_and_write_call_us": crate::pipeline_trace::micros(_texture_upload_started),
        "gpu_completion_us": null});
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Twinkle standalone texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    }))
}

#[derive(Clone, Copy)]
struct PresentationGeometry {
    logical_width: u32,
    pixel_width: u32,
    pixel_height: u32,
}

pub(crate) struct WindowGpuPresenter {
    presenter: GpuPresenter,
    graphics: GpuGraphics,
}

impl WindowGpuPresenter {
    /// # Safety
    /// The display and window handles must remain valid until this presenter is dropped.
    pub(crate) unsafe fn new(
        display: DisplayHandle<'_>,
        window: WindowHandle<'_>,
    ) -> Result<Self, String> {
        let graphics = unsafe { GpuGraphics::new(display, window) }?;
        let presenter = unsafe { GpuPresenter::new(window, &graphics) }?;
        Ok(Self {
            presenter,
            graphics,
        })
    }

    pub(crate) fn present(
        &mut self,
        logical_width: u32,
        pixel_width: u32,
        pixel_height: u32,
        commands: &[PaintCommand],
        damage: Option<&[Rect]>,
    ) -> Result<DamageRegion, String> {
        self.presenter.present(
            PresentationGeometry {
                logical_width,
                pixel_width,
                pixel_height,
            },
            &self.graphics,
            commands,
            damage,
        )
    }
}

struct GpuPresenter {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    configured: bool,
    pipeline: wgpu::RenderPipeline,
    shape_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    retained: Option<RetainedFramebuffer>,
    retained_diagnostics: RetainedFramebufferDiagnostics,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: usize,
    auxiliary_vertex_buffer: wgpu::Buffer,
    reuse_preparation_bypass: bool,
    incremental_preparation_enabled: bool,
    prepared_scene: Option<PreparedScene>,
}

struct RetainedFramebuffer {
    view: wgpu::TextureView,
    texture: Arc<wgpu::BindGroup>,
    configuration: (u32, u32, u32, wgpu::TextureFormat),
    valid: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetainedFramebufferDiagnostics {
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub creations: u64,
    pub releases: u64,
    pub budget_fallbacks: u64,
    pub composited_frames: u64,
    pub direct_full_frames: u64,
    pub full_initializations: u64,
    pub partial_redraws: u64,
    pub full_redraws: u64,
    pub unchanged_reuses: u64,
    pub prepared_scene_live_bytes: usize,
    pub prepared_scene_peak_bytes: usize,
    pub prepared_scene_budget_fallbacks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum OffscreenRedraw {
    Full,
    Partial(Rect),
    Reuse,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RedrawDecision {
    redraw: OffscreenRedraw,
    validation_visits: usize,
    fallback_reason: &'static str,
}

fn union_rect(left: Rect, right: Rect) -> Rect {
    let x = left.origin.x.min(right.origin.x);
    let y = left.origin.y.min(right.origin.y);
    let right_edge = (left.origin.x + left.size.width).max(right.origin.x + right.size.width);
    let bottom_edge = (left.origin.y + left.size.height).max(right.origin.y + right.size.height);
    Rect::new(x, y, right_edge - x, bottom_edge - y)
}

fn offscreen_redraw(
    commands: &[PaintCommand],
    damage: Option<&[Rect]>,
    valid: bool,
) -> RedrawDecision {
    if !valid {
        return RedrawDecision {
            redraw: OffscreenRedraw::Full,
            validation_visits: 0,
            fallback_reason: "retained-framebuffer-uninitialized",
        };
    }
    let Some(damage) = damage else {
        return RedrawDecision {
            redraw: OffscreenRedraw::Full,
            validation_visits: 0,
            fallback_reason: "damage-provenance-unknown",
        };
    };
    let Some(damage) = damage.iter().copied().reduce(union_rect) else {
        // An explicit empty authoritative damage set proves that the accepted
        // scene is unchanged. In particular, do not rescan the display list
        // for effects: no scene content will be prepared or redrawn.
        return RedrawDecision {
            redraw: OffscreenRedraw::Reuse,
            validation_visits: 0,
            fallback_reason: "none",
        };
    };
    let mut validation_visits = 0;
    for command in commands {
        validation_visits += 1;
        if matches!(command, PaintCommand::BackdropBlur { .. }) {
            return RedrawDecision {
                redraw: OffscreenRedraw::Full,
                validation_visits,
                fallback_reason: "backdrop-blur",
            };
        }
    }
    RedrawDecision {
        redraw: OffscreenRedraw::Partial(damage),
        validation_visits,
        fallback_reason: "none",
    }
}

fn physical_scissor(
    rect: Rect,
    width: u32,
    height: u32,
    scale: f32,
) -> Option<(u32, u32, u32, u32)> {
    let left = (rect.origin.x * scale).floor().clamp(0.0, width as f32) as u32;
    let top = (rect.origin.y * scale).floor().clamp(0.0, height as f32) as u32;
    let right = ((rect.origin.x + rect.size.width) * scale)
        .ceil()
        .clamp(0.0, width as f32) as u32;
    let bottom = ((rect.origin.y + rect.size.height) * scale)
        .ceil()
        .clamp(0.0, height as f32) as u32;
    (right > left && bottom > top).then(|| (left, top, right - left, bottom - top))
}

fn intersect_scissor(
    left: (u32, u32, u32, u32),
    right: (u32, u32, u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let x = left.0.max(right.0);
    let y = left.1.max(right.1);
    let right_edge = left
        .0
        .saturating_add(left.2)
        .min(right.0.saturating_add(right.2));
    let bottom = left
        .1
        .saturating_add(left.3)
        .min(right.1.saturating_add(right.3));
    (right_edge > x && bottom > y).then(|| (x, y, right_edge - x, bottom - y))
}

impl GpuPresenter {
    unsafe fn new(window: WindowHandle<'_>, graphics: &GpuGraphics) -> Result<Self, String> {
        // SAFETY: ShellSurface drops the presenter before the native window.
        let surface = unsafe {
            graphics
                .instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: graphics.display,
                    raw_window_handle: window.as_raw(),
                })
        }
        .map_err(|error| error.to_string())?;
        let mut config = surface
            .get_default_config(&graphics.adapter, 1, 1)
            .ok_or_else(|| "GPU adapter cannot present to this window".to_owned())?;
        let capabilities = surface.get_capabilities(&graphics.adapter);
        config.format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| *format == wgpu::TextureFormat::Bgra8Unorm)
            .unwrap_or(config.format);
        if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            config.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
        }
        crate::pipeline_trace::backend_identity("surface_synchronization", || {
            format!(
                "present_mode={:?}; desired_maximum_frame_latency={}; format={:?}; alpha_mode={:?}; physical_display_timing=unavailable",
                config.present_mode,
                config.desired_maximum_frame_latency,
                config.format,
                config.alpha_mode
            )
        });
        crate::pipeline_trace::event!({
            "event": "surface_synchronization",
            "present_mode": format!("{:?}", config.present_mode),
            "desired_maximum_frame_latency": config.desired_maximum_frame_latency,
            "physical_display_timing": null
        });
        let layout = graphics
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Twinkle standalone pipeline"),
                bind_group_layouts: &[Some(&graphics.texture_layout)],
                immediate_size: 0,
            });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2];
        let pipeline = graphics
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Twinkle standalone textured rectangles"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &graphics.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &graphics.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        let clear_pipeline =
            graphics
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Twinkle standalone damage clear"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &graphics.shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &graphics.shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: config.format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
        let shape_pipeline =
            graphics
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Twinkle standalone GPU shapes"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &graphics.shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &graphics.shader,
                        entry_point: Some("fs_shape"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: config.format,
                            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
        let mut auxiliary_vertices = Vec::with_capacity(12);
        auxiliary_vertices.extend_from_slice(&fullscreen_vertices([0.0; 4]));
        auxiliary_vertices.extend_from_slice(&fullscreen_vertices([1.0; 4]));
        let auxiliary_vertex_buffer =
            graphics
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Twinkle presenter auxiliary vertices"),
                    contents: bytemuck::cast_slice(&auxiliary_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
        Ok(Self {
            surface,
            config,
            configured: false,
            pipeline,
            shape_pipeline,
            clear_pipeline,
            retained: None,
            retained_diagnostics: RetainedFramebufferDiagnostics::default(),
            vertex_buffer: None,
            vertex_capacity: 0,
            auxiliary_vertex_buffer,
            reuse_preparation_bypass: std::env::var_os(
                "TWINKLE_GPU_FORCE_LEGACY_REUSE_PREPARATION",
            )
            .is_none(),
            incremental_preparation_enabled: std::env::var_os(
                "TWINKLE_GPU_DISABLE_INCREMENTAL_PREPARATION",
            )
            .is_none(),
            prepared_scene: None,
        })
    }

    fn ensure_retained_framebuffer(
        &mut self,
        graphics: &GpuGraphics,
        width: u32,
        height: u32,
        scale: f32,
    ) -> bool {
        let Some(bytes) = retained_framebuffer_bytes(width, height) else {
            self.release_retained_framebuffer();
            self.retained_diagnostics.budget_fallbacks =
                self.retained_diagnostics.budget_fallbacks.saturating_add(1);
            return false;
        };
        let configuration =
            retained_framebuffer_configuration(width, height, scale, self.config.format);
        if self
            .retained
            .as_ref()
            .is_some_and(|target| target.configuration == configuration)
        {
            return true;
        }
        self.release_retained_framebuffer();
        let texture = graphics.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Nickel retained shell framebuffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampled = Arc::new(
            graphics
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Nickel retained shell framebuffer"),
                    layout: &graphics.texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&graphics.sampler),
                        },
                    ],
                }),
        );
        self.retained = Some(RetainedFramebuffer {
            view,
            texture: sampled,
            configuration,
            valid: false,
        });
        self.retained_diagnostics.live_bytes = bytes;
        self.retained_diagnostics.peak_bytes = self.retained_diagnostics.peak_bytes.max(bytes);
        self.retained_diagnostics.creations = self.retained_diagnostics.creations.saturating_add(1);
        true
    }

    fn release_retained_framebuffer(&mut self) {
        if self.retained.take().is_some() {
            self.retained_diagnostics.releases =
                self.retained_diagnostics.releases.saturating_add(1);
        }
        self.retained_diagnostics.live_bytes = 0;
        self.discard_prepared_scene();
    }

    fn discard_prepared_scene(&mut self) {
        self.prepared_scene = None;
        self.retained_diagnostics.prepared_scene_live_bytes = 0;
    }

    fn present(
        &mut self,
        geometry: PresentationGeometry,
        graphics: &GpuGraphics,
        commands: &[PaintCommand],
        damage: Option<&[Rect]>,
    ) -> Result<DamageRegion, String> {
        let _presenter_started = crate::pipeline_trace::clock();
        let _allocation_started = crate::pipeline_trace::allocator_counts();
        if geometry.pixel_width == 0 || geometry.pixel_height == 0 {
            self.release_retained_framebuffer();
            self.configured = false;
            return Ok(DamageRegion::default());
        }
        let width = geometry.pixel_width.max(1);
        let height = geometry.pixel_height.max(1);
        let changed = self.config.width != width || self.config.height != height;
        if changed || !self.configured {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&graphics.device, &self.config);
            self.configured = true;
        }
        let scale = width as f32 / geometry.logical_width.max(1) as f32;
        let retained = self.ensure_retained_framebuffer(graphics, width, height, scale);
        let retained_valid = self.retained.as_ref().is_some_and(|target| target.valid);
        let decision = if retained {
            offscreen_redraw(commands, damage, retained_valid)
        } else {
            RedrawDecision {
                redraw: OffscreenRedraw::Full,
                validation_visits: 0,
                fallback_reason: "retained-framebuffer-budget",
            }
        };
        let mut redraw = decision.redraw;
        let mut damage_scissor = match redraw {
            OffscreenRedraw::Partial(damage) => physical_scissor(damage, width, height, scale),
            OffscreenRedraw::Full | OffscreenRedraw::Reuse => None,
        };
        if damage_scissor == Some((0, 0, width, height)) {
            redraw = OffscreenRedraw::Full;
            damage_scissor = None;
        } else if matches!(redraw, OffscreenRedraw::Partial(_)) && damage_scissor.is_none() {
            redraw = OffscreenRedraw::Reuse;
        }
        let _preparation_started = crate::pipeline_trace::clock();
        let mut frame = Frame::new(width, height, scale, graphics.white.clone());
        let configuration =
            retained_framebuffer_configuration(width, height, scale, self.config.format);
        let previous_prepared_scene = if matches!(redraw, OffscreenRedraw::Partial(_))
            && self.incremental_preparation_enabled
        {
            self.prepared_scene.take()
        } else {
            None
        };
        let mut prepared_update = if matches!(redraw, OffscreenRedraw::Partial(_))
            && self.incremental_preparation_enabled
        {
            prepare_incremental_scene(
                commands,
                configuration,
                width,
                height,
                scale,
                graphics,
                previous_prepared_scene,
            )
        } else {
            if matches!(redraw, OffscreenRedraw::Full) {
                self.prepared_scene = None;
            }
            None
        };
        if matches!(redraw, OffscreenRedraw::Partial(_))
            && self.incremental_preparation_enabled
            && prepared_update.is_none()
        {
            self.retained_diagnostics.prepared_scene_budget_fallbacks = self
                .retained_diagnostics
                .prepared_scene_budget_fallbacks
                .saturating_add(1);
            self.retained_diagnostics.prepared_scene_live_bytes = 0;
        }
        let prepare_scene = prepared_update.is_none()
            && (!matches!(redraw, OffscreenRedraw::Reuse) || !self.reuse_preparation_bypass);
        if prepare_scene {
            frame.prepare(commands, graphics);
        }
        let _preparation_us = crate::pipeline_trace::micros(_preparation_started);
        let _fragments_rebuilt = prepared_update.as_ref().map_or(0, |update| update.rebuilt);
        let _fragments_reused = prepared_update.as_ref().map_or(0, |update| update.reused);
        let _fragments_invalidated = prepared_update
            .as_ref()
            .map_or(0, |update| update.invalidated);
        let _scene_vertices_generated = prepared_update
            .as_ref()
            .map_or_else(|| frame.vertices.len(), |update| update.generated_vertices);
        let _commands_prepared = prepared_update.as_ref().map_or_else(
            || if prepare_scene { commands.len() } else { 0 },
            |update| update.rebuilt,
        );
        let prepared_update_metadata = prepared_update.take().map(|update| {
            self.retained_diagnostics.prepared_scene_live_bytes = update.scene.estimated_bytes;
            self.retained_diagnostics.prepared_scene_peak_bytes = self
                .retained_diagnostics
                .prepared_scene_peak_bytes
                .max(update.scene.estimated_bytes);
            self.prepared_scene = Some(update.scene);
            PreparedSceneUpdateMetadata {
                changed: update.changed,
                layout_stable: update.layout_stable,
            }
        });
        let prepared_active = matches!(redraw, OffscreenRedraw::Partial(_))
            && self.prepared_scene.is_some()
            && prepared_update_metadata.is_some();
        let scene_vertex_count = if prepared_active {
            self.prepared_scene
                .as_ref()
                .map_or(0, |scene| scene.vertex_count)
        } else {
            frame.vertices.len()
        };
        let preserved_scene_vertex_count = if matches!(redraw, OffscreenRedraw::Reuse) {
            self.prepared_scene
                .as_ref()
                .filter(|scene| scene.configuration == configuration)
                .map_or(0, |scene| scene.vertex_count)
        } else {
            scene_vertex_count
        };
        let _acquisition_started = crate::pipeline_trace::clock();
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output) => output,
            wgpu::CurrentSurfaceTexture::Suboptimal(output) => {
                self.configured = false;
                output
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&graphics.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(output)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => {
                        self.discard_prepared_scene();
                        return Ok(DamageRegion::default());
                    }
                    other => {
                        // Preparation may already have replaced the CPU-side cache. If the
                        // corresponding vertex writes cannot be submitted, that cache must not
                        // be trusted by a later sparse update.
                        self.release_retained_framebuffer();
                        return Err(format!(
                            "cannot acquire GPU surface after reconfigure: {other:?}"
                        ));
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.discard_prepared_scene();
                return Ok(DamageRegion::default());
            }
            other => {
                self.release_retained_framebuffer();
                return Err(format!("cannot acquire GPU surface: {other:?}"));
            }
        };
        let _acquisition_us = crate::pipeline_trace::micros(_acquisition_started);
        let _upload_started = crate::pipeline_trace::clock();
        let required = preserved_scene_vertex_count
            .saturating_mul(std::mem::size_of::<Vertex>())
            .max(std::mem::size_of::<Vertex>());
        let mut buffer_reallocated = false;
        if required > self.vertex_capacity {
            self.vertex_capacity = required.next_power_of_two();
            self.vertex_buffer = Some(graphics.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Nickel shell frame vertices"),
                size: self.vertex_capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            buffer_reallocated = true;
        }
        let vertex_buffer = self.vertex_buffer.as_ref().expect("frame buffer allocated");
        let mut _scene_vertex_upload_bytes = 0usize;
        let mut _vertex_upload_calls = 0usize;
        if prepared_active {
            let scene = self.prepared_scene.as_ref().expect("prepared scene");
            let update = prepared_update_metadata
                .as_ref()
                .expect("prepared update metadata");
            let upload_all = buffer_reallocated || !update.layout_stable;
            for (index, segment) in scene.segments.iter().enumerate() {
                if upload_all || update.changed[index] {
                    let bytes = bytemuck::cast_slice(&segment.vertices);
                    if !bytes.is_empty() {
                        graphics.queue.write_buffer(
                            vertex_buffer,
                            scene.vertex_offsets[index] as u64
                                * std::mem::size_of::<Vertex>() as u64,
                            bytes,
                        );
                        _scene_vertex_upload_bytes += bytes.len();
                        _vertex_upload_calls += 1;
                    }
                }
            }
        } else if prepare_scene {
            let bytes = bytemuck::cast_slice(&frame.vertices);
            if !bytes.is_empty() {
                graphics.queue.write_buffer(vertex_buffer, 0, bytes);
                _scene_vertex_upload_bytes = bytes.len();
                _vertex_upload_calls += 1;
            }
        }
        let _total_vertex_upload_bytes = _scene_vertex_upload_bytes;
        let _upload_us = crate::pipeline_trace::micros(_upload_started);
        let _draw_started = crate::pipeline_trace::clock();
        let _scene_encode_started = crate::pipeline_trace::clock();
        #[cfg(feature = "pipeline-attribution")]
        let mut _scene_draws_encoded = 0usize;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = graphics
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Nickel shell frame"),
            });
        if !matches!(redraw, OffscreenRedraw::Reuse) {
            let target = self
                .retained
                .as_ref()
                .filter(|_| retained)
                .map_or(&view, |retained| &retained.view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Nickel retained shell frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if matches!(redraw, OffscreenRedraw::Full) {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            if let Some(scissor) = damage_scissor {
                pass.set_pipeline(&self.clear_pipeline);
                pass.set_vertex_buffer(0, self.auxiliary_vertex_buffer.slice(..));
                pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                pass.set_bind_group(0, graphics.white.as_ref(), &[]);
                pass.draw(0..6, 0..1);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            }
            if prepared_active {
                for (segment, base) in self
                    .prepared_scene
                    .as_ref()
                    .expect("prepared scene")
                    .segments
                    .iter()
                    .zip(
                        self.prepared_scene
                            .as_ref()
                            .expect("prepared scene")
                            .vertex_offsets
                            .iter()
                            .copied(),
                    )
                {
                    for draw in &segment.draws {
                        let Some(scissor) = damage_scissor
                            .and_then(|damage| intersect_scissor(draw.scissor, damage))
                        else {
                            continue;
                        };
                        pass.set_pipeline(match draw.kind {
                            DrawKind::Textured => &self.pipeline,
                            DrawKind::Shape => &self.shape_pipeline,
                        });
                        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                        pass.set_bind_group(0, draw.texture.as_ref(), &[]);
                        pass.draw(base + draw.start..base + draw.end, 0..1);
                        #[cfg(feature = "pipeline-attribution")]
                        {
                            _scene_draws_encoded += 1;
                        }
                    }
                }
            } else {
                for draw in &frame.draws {
                    let scissor = match redraw {
                        OffscreenRedraw::Full => Some(draw.scissor),
                        OffscreenRedraw::Partial(_) => damage_scissor
                            .and_then(|damage| intersect_scissor(draw.scissor, damage)),
                        OffscreenRedraw::Reuse => None,
                    };
                    let Some(scissor) = scissor else { continue };
                    pass.set_pipeline(match draw.kind {
                        DrawKind::Textured => &self.pipeline,
                        DrawKind::Shape => &self.shape_pipeline,
                    });
                    pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                    pass.set_bind_group(0, draw.texture.as_ref(), &[]);
                    pass.draw(draw.start..draw.end, 0..1);
                    #[cfg(feature = "pipeline-attribution")]
                    {
                        _scene_draws_encoded += 1;
                    }
                }
            }
        }
        let _scene_encoding_us = crate::pipeline_trace::micros(_scene_encode_started);
        let _composite_started = crate::pipeline_trace::clock();
        #[cfg(feature = "pipeline-attribution")]
        let _composite_encoded = self.retained.is_some() && retained;
        if let Some(retained) = self.retained.as_ref().filter(|_| retained) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Nickel shell surface composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.auxiliary_vertex_buffer.slice(..));
            pass.set_scissor_rect(0, 0, width, height);
            pass.set_bind_group(0, retained.texture.as_ref(), &[]);
            pass.draw(6..12, 0..1);
            self.retained_diagnostics.composited_frames = self
                .retained_diagnostics
                .composited_frames
                .saturating_add(1);
        } else {
            self.retained_diagnostics.direct_full_frames = self
                .retained_diagnostics
                .direct_full_frames
                .saturating_add(1);
        }
        let _composite_encoding_us = crate::pipeline_trace::micros(_composite_started);
        let _submit_started = crate::pipeline_trace::clock();
        graphics.queue.submit(Some(encoder.finish()));
        let _submit_us = crate::pipeline_trace::micros(_submit_started);
        match redraw {
            OffscreenRedraw::Full => {
                self.retained_diagnostics.full_redraws =
                    self.retained_diagnostics.full_redraws.saturating_add(1);
            }
            OffscreenRedraw::Partial(_) => {
                self.retained_diagnostics.partial_redraws =
                    self.retained_diagnostics.partial_redraws.saturating_add(1);
            }
            OffscreenRedraw::Reuse => {
                self.retained_diagnostics.unchanged_reuses =
                    self.retained_diagnostics.unchanged_reuses.saturating_add(1);
            }
        }
        if let Some(retained) = self.retained.as_mut().filter(|_| retained)
            && !retained.valid
        {
            retained.valid = true;
            self.retained_diagnostics.full_initializations = self
                .retained_diagnostics
                .full_initializations
                .saturating_add(1);
        }
        let _encode_submit_us = crate::pipeline_trace::micros(_draw_started);
        let _present_started = crate::pipeline_trace::clock();
        graphics.queue.present(output);
        let _allocation_delta = _allocation_started
            .zip(crate::pipeline_trace::allocator_counts())
            .map(|(before, after)| {
                [
                    after[0].saturating_sub(before[0]),
                    after[1].saturating_sub(before[1]),
                    after[2].saturating_sub(before[2]),
                ]
            });
        let (
            _texture_cache_entries,
            _texture_cache_resident_bytes,
            _texture_cache_texture_bytes,
            _texture_cache_key_and_metadata_bytes,
            _texture_cache_peak_bytes,
            _texture_cache_hits,
            _texture_cache_misses,
            _texture_cache_insertions,
            _texture_cache_evictions,
            _texture_cache_eviction_scans,
            _texture_cache_oversized,
            _texture_cache_bytes_hashed,
            _texture_cache_bytes_compared,
        ) = {
            let cache = graphics.textures.borrow();
            (
                cache.len(),
                cache.bytes,
                cache.texture_bytes(),
                cache.key_and_metadata_bytes(),
                cache.peak_bytes,
                cache.hits,
                cache.misses,
                cache.insertions,
                cache.evictions,
                cache.eviction_scans,
                cache.oversized,
                cache.bytes_hashed,
                cache.bytes_compared,
            )
        };
        crate::pipeline_trace::event!({
            "event": "texture_cache", "entries": _texture_cache_entries,
            "resident_bytes": _texture_cache_resident_bytes,
            "texture_bytes": _texture_cache_texture_bytes,
            "key_and_metadata_bytes": _texture_cache_key_and_metadata_bytes,
            "peak_bytes": _texture_cache_peak_bytes, "budget_bytes": MAX_TEXTURE_CACHE_BYTES,
            "hits": _texture_cache_hits, "misses": _texture_cache_misses,
            "insertions": _texture_cache_insertions, "evictions": _texture_cache_evictions,
            "eviction_scan_entries": _texture_cache_eviction_scans,
            "oversized_uncached": _texture_cache_oversized, "whole_resets": 0,
            "bytes_hashed": _texture_cache_bytes_hashed, "bytes_compared": _texture_cache_bytes_compared,
            "externally_held_and_inflight_bytes": null
        });
        crate::pipeline_trace::event!({
            "event": "gpu_frame", "physical_width": width, "physical_height": height, "scale": scale,
            "provenance_validation_visits": decision.validation_visits,
            "fallback_reason": decision.fallback_reason,
            "reuse_preparation_bypass_enabled": self.reuse_preparation_bypass,
            "incremental_preparation_enabled": self.incremental_preparation_enabled,
            "commands_prepared": _commands_prepared,
            "scene_vertices": _scene_vertices_generated,
            "prepared_scene_vertices": scene_vertex_count,
            "scene_vertex_bytes": _scene_vertices_generated * std::mem::size_of::<Vertex>(),
            "scene_vertex_upload_bytes": _scene_vertex_upload_bytes,
            "auxiliary_vertex_bytes": 0,
            "static_auxiliary_vertex_bytes": 12 * std::mem::size_of::<Vertex>(),
            "total_vertex_upload_bytes": _total_vertex_upload_bytes,
            "vertex_upload_calls": _vertex_upload_calls,
            "redraw": format!("{redraw:?}"),
            "preparation_us": _preparation_us, "surface_acquisition_us": _acquisition_us,
            "vertex_upload_call_us": _upload_us, "encode_submit_us": _encode_submit_us,
            "scene_encoding_us": _scene_encoding_us, "scene_draws_encoded": _scene_draws_encoded,
            "composite_encoding_us": if _composite_encoded { _composite_encoding_us } else { Some(0u64) },
            "composite_skipped": !_composite_encoded, "command_finalize_and_submit_us": _submit_us,
            "effective_damage_scissor": damage_scissor,
            "scene_damage_coverage_pixels": match redraw { OffscreenRedraw::Full => u64::from(width)*u64::from(height), OffscreenRedraw::Partial(_) => damage_scissor.map_or(0, |(_,_,w,h)|u64::from(w)*u64::from(h)), OffscreenRedraw::Reuse => 0 },
            "present_call_us": crate::pipeline_trace::micros(_present_started),
            "framebuffer_live_bytes": self.retained_diagnostics.live_bytes,
            "framebuffer_peak_bytes": self.retained_diagnostics.peak_bytes,
            "texture_cache_resident_bytes": _texture_cache_resident_bytes,
            "texture_cache_peak_bytes": _texture_cache_peak_bytes,
            "externally_held_and_inflight_texture_bytes": null,
            "compositor_presentation_timestamp": null,
            "gpu_execution_us": null,
            "presenter_call_us": crate::pipeline_trace::micros(_presenter_started),
            "timing_boundary": "CPU calls only; preparation includes text cache/raster/texture uploads; GPU completion and compositor display unavailable"
        });
        crate::pipeline_trace::event!({
            "event": "gpu_damage",
            "damage_rectangles": damage.map_or(0, <[Rect]>::len),
            "damage_rectangle_pixels": damage.map_or(0, |rects| rects.iter().filter_map(|rect| physical_scissor(*rect, width, height, scale)).map(|(_,_,w,h)|u64::from(w)*u64::from(h)).sum::<u64>()),
            "damage_union_pixels": match redraw { OffscreenRedraw::Full => u64::from(width)*u64::from(height), OffscreenRedraw::Partial(_) => damage_scissor.map_or(0, |(_,_,w,h)|u64::from(w)*u64::from(h)), OffscreenRedraw::Reuse => 0 },
            "allocation_calls": _allocation_delta.map(|delta| delta[0]),
            "deallocation_calls": _allocation_delta.map(|delta| delta[1]),
            "allocation_requested_bytes": _allocation_delta.map(|delta| delta[2]),
            "draw_calls": _scene_draws_encoded + usize::from(damage_scissor.is_some()) + usize::from(_composite_encoded),
            "allocation_boundary": "Process-wide Rust allocator delta across presenter call; concurrent threads may overlap; excludes foreign driver allocations"
        });
        crate::pipeline_trace::event!({
            "event": "gpu_prepared_fragments",
            "rebuilt": _fragments_rebuilt,
            "reused": _fragments_reused,
            "invalidated": _fragments_invalidated,
            "live_bytes": self.retained_diagnostics.prepared_scene_live_bytes,
            "peak_bytes": self.retained_diagnostics.prepared_scene_peak_bytes,
            "budget_bytes": PREPARED_SCENE_BUDGET_BYTES,
            "vertex_buffer_capacity_bytes": self.vertex_capacity,
            "draw_records": if prepared_active { self.prepared_scene.as_ref().map_or(0, |scene| scene.segments.iter().map(|segment| segment.draws.len()).sum()) } else { frame.draws.len() },
            "budget_fallbacks": self.retained_diagnostics.prepared_scene_budget_fallbacks
        });
        Ok(DamageRegion {
            rects: vec![Rect::new(0.0, 0.0, width as f32, height as f32)].into(),
        })
    }
}

#[derive(Clone)]
struct Draw {
    start: u32,
    end: u32,
    scissor: (u32, u32, u32, u32),
    texture: Arc<wgpu::BindGroup>,
    kind: DrawKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrawKind {
    Textured,
    Shape,
}

const PREPARED_SCENE_BUDGET_BYTES: usize = 32 * 1024 * 1024;

struct PreparedSegment {
    command: PaintCommand,
    entry_clips: Vec<Rect>,
    exit_clips: Vec<Rect>,
    vertices: Vec<Vertex>,
    draws: Vec<Draw>,
    estimated_bytes: usize,
}

struct PreparedScene {
    configuration: (u32, u32, u32, wgpu::TextureFormat),
    segments: Vec<Arc<PreparedSegment>>,
    vertex_offsets: Vec<u32>,
    vertex_count: usize,
    estimated_bytes: usize,
}

struct PreparedSceneUpdate {
    scene: PreparedScene,
    rebuilt: usize,
    reused: usize,
    invalidated: usize,
    generated_vertices: usize,
    changed: Vec<bool>,
    layout_stable: bool,
}

struct PreparedSceneUpdateMetadata {
    changed: Vec<bool>,
    layout_stable: bool,
}

fn paint_command_owned_bytes(command: &PaintCommand, scale: f32) -> usize {
    let texture_bytes = |bounds: Rect| {
        ((bounds.size.width * scale).ceil().max(1.0) as usize)
            .saturating_mul((bounds.size.height * scale).ceil().max(1.0) as usize)
            .saturating_mul(4)
    };
    match command {
        PaintCommand::Text { bounds, text, .. } => {
            text.capacity().saturating_add(texture_bytes(*bounds))
        }
        PaintCommand::StyledText {
            bounds,
            text,
            spans,
            ..
        } => text
            .capacity()
            .saturating_add(
                spans.capacity().saturating_mul(
                    spans
                        .first()
                        .map_or(0, |_| std::mem::size_of_val(&spans[0])),
                ),
            )
            .saturating_add(texture_bytes(*bounds)),
        PaintCommand::Image {
            image,
            high_density,
            ..
        } => {
            let cpu_bytes = image.as_raw().len().saturating_add(
                high_density
                    .as_ref()
                    .map_or(0, |image| image.as_raw().len()),
            );
            let selected = high_density
                .as_ref()
                .filter(|_| scale >= 1.5)
                .unwrap_or(image);
            cpu_bytes.saturating_add(selected.as_raw().len())
        }
        PaintCommand::Path { geometry, .. } => geometry.owned_bytes().saturating_add(
            geometry
                .triangles()
                .len()
                .saturating_mul(std::mem::size_of::<Vertex>()),
        ),
        PaintCommand::BackdropBlur { .. }
        | PaintCommand::Fill { .. }
        | PaintCommand::TopRoundedFill { .. }
        | PaintCommand::RoundedFill { .. }
        | PaintCommand::RoundedStroke { .. }
        | PaintCommand::Gradient { .. }
        | PaintCommand::Stroke { .. }
        | PaintCommand::OverlayFill { .. }
        | PaintCommand::OverlayStroke { .. }
        | PaintCommand::PushClip(_)
        | PaintCommand::PopClip => 0,
    }
}

fn prepare_segment(
    command: &PaintCommand,
    entry_clips: &[Rect],
    width: u32,
    height: u32,
    scale: f32,
    graphics: &GpuGraphics,
) -> Arc<PreparedSegment> {
    let mut frame = Frame::new(width, height, scale, graphics.white.clone());
    frame.clips.clear();
    frame.clips.extend_from_slice(entry_clips);
    frame.prepare(std::slice::from_ref(command), graphics);
    let estimated_bytes = std::mem::size_of::<PreparedSegment>()
        .saturating_add(std::mem::size_of::<PaintCommand>())
        .saturating_add(paint_command_owned_bytes(command, scale))
        .saturating_add(
            entry_clips
                .len()
                .saturating_add(frame.clips.len())
                .saturating_mul(std::mem::size_of::<Rect>()),
        )
        .saturating_add(
            frame
                .vertices
                .capacity()
                .saturating_mul(std::mem::size_of::<Vertex>()),
        )
        .saturating_add(
            frame
                .draws
                .capacity()
                .saturating_mul(std::mem::size_of::<Draw>()),
        );
    Arc::new(PreparedSegment {
        command: command.clone(),
        entry_clips: entry_clips.to_vec(),
        exit_clips: frame.clips,
        vertices: frame.vertices,
        draws: frame.draws,
        estimated_bytes,
    })
}

fn prepare_incremental_scene(
    commands: &[PaintCommand],
    configuration: (u32, u32, u32, wgpu::TextureFormat),
    width: u32,
    height: u32,
    scale: f32,
    graphics: &GpuGraphics,
    previous: Option<PreparedScene>,
) -> Option<PreparedSceneUpdate> {
    let previous_compatible = previous
        .as_ref()
        .is_some_and(|scene| scene.configuration == configuration);
    let initial_clip = Rect::new(0.0, 0.0, width as f32 / scale, height as f32 / scale);
    let mut clips = vec![initial_clip];
    let mut segments = Vec::with_capacity(commands.len());
    let mut offsets = Vec::with_capacity(commands.len());
    let mut changed = Vec::with_capacity(commands.len());
    let mut vertex_count = 0usize;
    let mut estimated_bytes =
        std::mem::size_of::<PreparedScene>().saturating_add(commands.len().saturating_mul(
            std::mem::size_of::<Arc<PreparedSegment>>() + std::mem::size_of::<u32>(),
        ));
    let mut rebuilt = 0usize;
    let mut reused = 0usize;
    let mut generated_vertices = 0usize;
    let mut layout_stable = previous_compatible
        && previous
            .as_ref()
            .is_some_and(|scene| scene.segments.len() == commands.len());

    for (index, command) in commands.iter().enumerate() {
        offsets.push(u32::try_from(vertex_count).ok()?);
        let prior = previous
            .as_ref()
            .filter(|_| previous_compatible)
            .and_then(|scene| scene.segments.get(index));
        let segment = if let Some(prior) = prior
            && prior.command == *command
            && prior.entry_clips == clips
        {
            reused += 1;
            changed.push(false);
            Arc::clone(prior)
        } else {
            let segment = prepare_segment(command, &clips, width, height, scale, graphics);
            generated_vertices = generated_vertices.saturating_add(segment.vertices.len());
            rebuilt += 1;
            changed.push(true);
            if prior.is_none_or(|prior| prior.vertices.len() != segment.vertices.len()) {
                layout_stable = false;
            }
            segment
        };
        clips.clone_from(&segment.exit_clips);
        vertex_count = vertex_count.checked_add(segment.vertices.len())?;
        estimated_bytes = estimated_bytes.checked_add(segment.estimated_bytes)?;
        if estimated_bytes > PREPARED_SCENE_BUDGET_BYTES {
            return None;
        }
        segments.push(segment);
    }
    let invalidated = previous
        .as_ref()
        .map_or(0, |scene| scene.segments.len().saturating_sub(reused));
    Some(PreparedSceneUpdate {
        scene: PreparedScene {
            configuration,
            segments,
            vertex_offsets: offsets,
            vertex_count,
            estimated_bytes,
        },
        rebuilt,
        reused,
        invalidated,
        generated_vertices,
        changed,
        layout_stable,
    })
}

struct Frame {
    width: u32,
    height: u32,
    scale: f32,
    white: Arc<wgpu::BindGroup>,
    vertices: Vec<Vertex>,
    draws: Vec<Draw>,
    clips: Vec<Rect>,
}

fn fullscreen_vertices(color: [f32; 4]) -> [Vertex; 6] {
    let top_left = Vertex {
        position: [-1.0, 1.0],
        uv: [0.0, 0.0],
        color,
        half_size: [0.0; 2],
        shape: [0.0; 2],
    };
    let top_right = Vertex {
        position: [1.0, 1.0],
        uv: [1.0, 0.0],
        color,
        half_size: [0.0; 2],
        shape: [0.0; 2],
    };
    let bottom_left = Vertex {
        position: [-1.0, -1.0],
        uv: [0.0, 1.0],
        color,
        half_size: [0.0; 2],
        shape: [0.0; 2],
    };
    let bottom_right = Vertex {
        position: [1.0, -1.0],
        uv: [1.0, 1.0],
        color,
        half_size: [0.0; 2],
        shape: [0.0; 2],
    };
    [
        top_left,
        bottom_left,
        top_right,
        top_right,
        bottom_left,
        bottom_right,
    ]
}

impl Frame {
    fn new(width: u32, height: u32, scale: f32, white: Arc<wgpu::BindGroup>) -> Self {
        Self {
            width,
            height,
            scale,
            white,
            vertices: Vec::new(),
            draws: Vec::new(),
            clips: vec![Rect::new(
                0.0,
                0.0,
                width as f32 / scale,
                height as f32 / scale,
            )],
        }
    }

    fn rect(
        &mut self,
        rect: Rect,
        color: [f32; 4],
        texture: Arc<wgpu::BindGroup>,
        kind: DrawKind,
        radius: f32,
        stroke: f32,
    ) {
        let clip = *self.clips.last().expect("frame clip");
        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            return;
        }
        if intersect(rect, clip).is_none() {
            return;
        }
        let x0 = (rect.origin.x * self.scale / self.width as f32) * 2.0 - 1.0;
        let y0 = 1.0 - (rect.origin.y * self.scale / self.height as f32) * 2.0;
        let x1 = ((rect.origin.x + rect.size.width) * self.scale / self.width as f32) * 2.0 - 1.0;
        let y1 = 1.0 - ((rect.origin.y + rect.size.height) * self.scale / self.height as f32) * 2.0;
        let half_size = [
            rect.size.width * self.scale * 0.5,
            rect.size.height * self.scale * 0.5,
        ];
        let shape = [radius * self.scale, stroke * self.scale];
        let corners = [
            Vertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                half_size,
                shape,
            },
            Vertex {
                position: [x1, y0],
                uv: [1.0, 0.0],
                color,
                half_size,
                shape,
            },
            Vertex {
                position: [x0, y1],
                uv: [0.0, 1.0],
                color,
                half_size,
                shape,
            },
            Vertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                half_size,
                shape,
            },
        ];
        let start = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&[
            corners[0], corners[2], corners[1], corners[1], corners[2], corners[3],
        ]);
        let sx = (clip.origin.x * self.scale)
            .floor()
            .clamp(0.0, self.width as f32) as u32;
        let sy = (clip.origin.y * self.scale)
            .floor()
            .clamp(0.0, self.height as f32) as u32;
        let ex = ((clip.origin.x + clip.size.width) * self.scale)
            .ceil()
            .clamp(0.0, self.width as f32) as u32;
        let ey = ((clip.origin.y + clip.size.height) * self.scale)
            .ceil()
            .clamp(0.0, self.height as f32) as u32;
        if ex > sx && ey > sy {
            let scissor = (sx, sy, ex - sx, ey - sy);
            if let Some(last) = self.draws.last_mut()
                && last.end == start
                && last.scissor == scissor
                && Arc::ptr_eq(&last.texture, &texture)
                && last.kind == kind
            {
                last.end += 6;
                return;
            }
            self.draws.push(Draw {
                start,
                end: start + 6,
                scissor,
                texture,
                kind,
            });
        }
    }

    fn solid(&mut self, rect: Rect, color: u32) {
        self.rect(
            rect,
            rgb(color),
            self.white.clone(),
            DrawKind::Textured,
            0.0,
            0.0,
        );
    }

    fn path(&mut self, points: &[Point], color: u32) {
        if points.is_empty() {
            return;
        }
        let clip = *self.clips.last().expect("frame clip");
        let start = self.vertices.len() as u32;
        self.vertices.extend(points.iter().map(|point| Vertex {
            position: [
                (point.x * self.scale / self.width as f32) * 2.0 - 1.0,
                1.0 - (point.y * self.scale / self.height as f32) * 2.0,
            ],
            uv: [0.0, 0.0],
            color: rgb(color),
            half_size: [0.0; 2],
            shape: [0.0; 2],
        }));
        let end = self.vertices.len() as u32;
        let sx = (clip.origin.x * self.scale)
            .floor()
            .clamp(0.0, self.width as f32) as u32;
        let sy = (clip.origin.y * self.scale)
            .floor()
            .clamp(0.0, self.height as f32) as u32;
        let ex = ((clip.origin.x + clip.size.width) * self.scale)
            .ceil()
            .clamp(0.0, self.width as f32) as u32;
        let ey = ((clip.origin.y + clip.size.height) * self.scale)
            .ceil()
            .clamp(0.0, self.height as f32) as u32;
        if ex > sx && ey > sy {
            self.draws.push(Draw {
                start,
                end,
                scissor: (sx, sy, ex - sx, ey - sy),
                texture: self.white.clone(),
                kind: DrawKind::Textured,
            });
        } else {
            self.vertices.truncate(start as usize);
        }
    }

    fn rounded(&mut self, rect: Rect, color: u32, radius: f32, top_only: bool) {
        if top_only {
            for (span, shaded) in
                crate::backend::rounded_coverage_spans(rect, color, radius, None, true, self.scale)
            {
                self.solid(span, shaded);
            }
        } else {
            self.rect(
                rect,
                rgb(color),
                self.white.clone(),
                DrawKind::Shape,
                radius,
                0.0,
            );
        }
    }

    fn prepare(&mut self, commands: &[PaintCommand], graphics: &GpuGraphics) {
        for command in commands {
            match command {
                PaintCommand::BackdropBlur { .. } => {}
                PaintCommand::Fill { rect, color } | PaintCommand::OverlayFill { rect, color } => {
                    self.solid(*rect, *color)
                }
                PaintCommand::RoundedFill {
                    rect,
                    color,
                    radius,
                } => self.rounded(*rect, *color, *radius, false),
                PaintCommand::TopRoundedFill {
                    rect,
                    color,
                    radius,
                } => self.rounded(*rect, *color, *radius, true),
                PaintCommand::RoundedStroke {
                    rect,
                    color,
                    width,
                    radius,
                } => {
                    self.rect(
                        *rect,
                        rgb(*color),
                        self.white.clone(),
                        DrawKind::Shape,
                        *radius,
                        *width,
                    );
                }
                PaintCommand::Gradient { rect, gradient } => {
                    let horizontal = gradient.axis == GradientAxis::Horizontal;
                    let extent = if horizontal {
                        rect.size.width
                    } else {
                        rect.size.height
                    };
                    for step in 0..extent.ceil().max(1.0) as u32 {
                        let at = step as f32;
                        let color = mix(
                            gradient.start,
                            gradient.end,
                            ((at + 0.5) / extent.max(1.0)).clamp(0.0, 1.0),
                        );
                        let strip = if horizontal {
                            Rect::new(
                                rect.origin.x + at,
                                rect.origin.y,
                                (extent - at).min(1.0),
                                rect.size.height,
                            )
                        } else {
                            Rect::new(
                                rect.origin.x,
                                rect.origin.y + at,
                                rect.size.width,
                                (extent - at).min(1.0),
                            )
                        };
                        self.solid(strip, color);
                    }
                }
                PaintCommand::Stroke { rect, color, width }
                | PaintCommand::OverlayStroke { rect, color, width } => {
                    let w = width.max(0.0).min(rect.size.width).min(rect.size.height);
                    self.solid(
                        Rect::new(rect.origin.x, rect.origin.y, rect.size.width, w),
                        *color,
                    );
                    self.solid(
                        Rect::new(
                            rect.origin.x,
                            rect.origin.y + rect.size.height - w,
                            rect.size.width,
                            w,
                        ),
                        *color,
                    );
                    self.solid(
                        Rect::new(
                            rect.origin.x,
                            rect.origin.y + w,
                            w,
                            (rect.size.height - 2.0 * w).max(0.0),
                        ),
                        *color,
                    );
                    self.solid(
                        Rect::new(
                            rect.origin.x + rect.size.width - w,
                            rect.origin.y + w,
                            w,
                            (rect.size.height - 2.0 * w).max(0.0),
                        ),
                        *color,
                    );
                }
                PaintCommand::Path { geometry, color } => {
                    self.path(geometry.triangles(), *color);
                }
                PaintCommand::Text { bounds, .. } | PaintCommand::StyledText { bounds, .. } => {
                    if bounds.size.width > 0.0
                        && bounds.size.height > 0.0
                        && intersect(*bounds, *self.clips.last().unwrap()).is_some()
                        && let Some(texture) = graphics.text_texture(command, *bounds, self.scale)
                    {
                        self.rect(*bounds, [1.0; 4], texture, DrawKind::Textured, 0.0, 0.0);
                    }
                }
                PaintCommand::Image {
                    bounds,
                    id,
                    generation,
                    image,
                    high_density,
                } => {
                    if bounds.size.width <= 0.0
                        || bounds.size.height <= 0.0
                        || intersect(*bounds, *self.clips.last().expect("frame clip")).is_none()
                    {
                        continue;
                    }
                    let selected = high_density
                        .as_ref()
                        .filter(|_| self.scale >= 1.5)
                        .unwrap_or(image);
                    if selected.width() == 0 || selected.height() == 0 {
                        continue;
                    }
                    let image_key = ImageTextureKey {
                        id: *id,
                        generation: *generation,
                        high_density: self.scale >= 1.5,
                        width: selected.width(),
                        height: selected.height(),
                        allocation: Arc::as_ptr(selected) as usize,
                    };
                    let key = BorrowedTextureKey::Image(image_key);
                    let cached = graphics.textures.borrow_mut().get(&key).0;
                    let texture = if let Some(cached) = cached {
                        cached
                    } else {
                        let mut bytes = selected.as_raw().clone();
                        for pixel in bytes.as_chunks_mut::<4>().0.iter_mut() {
                            let alpha = pixel[3] as u16;
                            for channel in &mut pixel[..3] {
                                *channel = ((*channel as u16 * alpha + 127) / 255) as u8;
                            }
                        }
                        graphics.upload_cached_texture(
                            key,
                            selected.width(),
                            selected.height(),
                            &bytes,
                        )
                    };
                    self.rect(*bounds, [1.0; 4], texture, DrawKind::Textured, 0.0, 0.0);
                }
                PaintCommand::PushClip(rect) => {
                    let clip = *self.clips.last().unwrap();
                    self.clips
                        .push(intersect(clip, *rect).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0)));
                }
                PaintCommand::PopClip => {
                    if self.clips.len() > 1 {
                        self.clips.pop();
                    }
                }
            }
        }
    }
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.origin.x.max(b.origin.x);
    let y = a.origin.y.max(b.origin.y);
    let right = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let bottom = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (right > x && bottom > y).then(|| Rect::new(x, y, right - x, bottom - y))
}

fn rgb(color: u32) -> [f32; 4] {
    let alpha = if color <= 0x00ff_ffff {
        1.0
    } else {
        ((color >> 24) & 255) as f32 / 255.0
    };
    [
        ((color >> 16) & 255) as f32 / 255.0 * alpha,
        ((color >> 8) & 255) as f32 / 255.0 * alpha,
        (color & 255) as f32 / 255.0 * alpha,
        alpha,
    ]
}

fn mix(start: u32, end: u32, at: f32) -> u32 {
    let channel = |shift: u32| {
        let a = if shift == 24 && start <= 0x00ff_ffff {
            255
        } else {
            (start >> shift) & 255
        };
        let b = if shift == 24 && end <= 0x00ff_ffff {
            255
        } else {
            (end >> shift) & 255
        };
        (a as f32 + (b as f32 - a as f32) * at).round() as u32
    };
    channel(24) << 24 | channel(16) << 16 | channel(8) << 8 | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_cache_key(id: u16) -> ImageTextureKey {
        ImageTextureKey {
            id,
            generation: 1,
            high_density: false,
            width: 4,
            height: 4,
            allocation: id as usize,
        }
    }

    fn plain_text_command() -> PaintCommand {
        PaintCommand::Text {
            bounds: Rect::new(11.0, 13.0, 180.0, 42.0),
            text: "Cache me".to_owned(),
            scale: 1.25,
            color: 0xff336699,
            align: crate::TextAlign::Center,
            bold: false,
            wrap: true,
        }
    }

    fn assert_text_key_changed(base: &TextureKey, command: &PaintCommand) {
        assert_ne!(borrowed_text_texture_key(command, 2.0, 7).to_owned(), *base);
    }

    #[test]
    fn plain_text_key_covers_every_raster_dependency_but_translation() {
        let command = plain_text_command();
        let base = borrowed_text_texture_key(&command, 2.0, 7).to_owned();
        let mut translated = command.clone();
        if let PaintCommand::Text { bounds, .. } = &mut translated {
            bounds.origin.x += 900.0;
            bounds.origin.y -= 400.0;
        }
        assert_eq!(
            borrowed_text_texture_key(&translated, 2.0, 7).to_owned(),
            base
        );

        for mutation in 0..8 {
            let mut changed = command.clone();
            if let PaintCommand::Text {
                bounds,
                text,
                scale,
                color,
                align,
                bold,
                wrap,
            } = &mut changed
            {
                match mutation {
                    0 => text.push('!'),
                    1 => bounds.size.width += 1.0,
                    2 => bounds.size.height += 1.0,
                    3 => *scale += 0.25,
                    4 => *color ^= 1,
                    5 => *align = crate::TextAlign::End,
                    6 => *bold = !*bold,
                    7 => *wrap = !*wrap,
                    _ => unreachable!(),
                }
            }
            assert_text_key_changed(&base, &changed);
        }
        assert_ne!(borrowed_text_texture_key(&command, 1.5, 7).to_owned(), base);
        assert_ne!(borrowed_text_texture_key(&command, 2.0, 8).to_owned(), base);
    }

    #[test]
    fn text_texture_dimensions_reject_invalid_and_unbounded_inputs() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0);
        assert_eq!(text_texture_dimensions(bounds, 2.0, 8192), Some((200, 40)));
        assert_eq!(text_texture_dimensions(bounds, f32::NAN, 8192), None);
        assert_eq!(text_texture_dimensions(bounds, f32::INFINITY, 8192), None);
        assert_eq!(text_texture_dimensions(bounds, 0.0, 8192), None);
        assert_eq!(
            text_texture_dimensions(Rect::new(0.0, 0.0, f32::NAN, 20.0), 1.0, 8192),
            None
        );
        assert_eq!(
            text_texture_dimensions(Rect::new(0.0, 0.0, 9000.0, 20.0), 1.0, 8192),
            None
        );
        assert_eq!(
            text_texture_dimensions(Rect::new(0.0, 0.0, 9000.0, 9000.0), 1.0, 16384),
            None
        );
    }

    #[test]
    fn styled_text_key_covers_spans_font_size_and_float_bits() {
        let span = crate::StyledTextSpan {
            range: 0..5,
            bold: true,
            italic: false,
            monospace: false,
            font_family: Some(Arc::from("serif")),
            strikethrough: false,
            underline: crate::TextUnderlineStyle::Single,
            color: Some(0xff112233),
            background: None,
        };
        let command = PaintCommand::StyledText {
            bounds: Rect::new(3.0, 4.0, 140.0, 60.0),
            text: "styled".to_owned(),
            spans: vec![span],
            scale: 1.0,
            font_size: Some(17.0),
            color: 0xffabcdef,
            align: crate::TextAlign::Start,
        };
        let base = borrowed_text_texture_key(&command, 1.0, 3).to_owned();
        for mutation in 0..7 {
            let mut changed = command.clone();
            if let PaintCommand::StyledText {
                bounds,
                text,
                spans,
                scale,
                font_size,
                color,
                align,
            } = &mut changed
            {
                match mutation {
                    0 => text.push('!'),
                    1 => spans[0].italic = true,
                    2 => bounds.size.width += 1.0,
                    3 => bounds.size.height += 1.0,
                    4 => *scale = f32::from_bits(scale.to_bits() + 1),
                    5 => *font_size = Some(f32::from_bits(17.0f32.to_bits() + 1)),
                    6 => {
                        *color ^= 1;
                        *align = crate::TextAlign::End;
                    }
                    _ => unreachable!(),
                }
            }
            assert_ne!(borrowed_text_texture_key(&changed, 1.0, 3).to_owned(), base);
        }
        assert_ne!(
            borrowed_text_texture_key(&command, f32::NAN, 3).to_owned(),
            base
        );
        assert_ne!(borrowed_text_texture_key(&command, 1.0, 4).to_owned(), base);
        assert_ne!(
            borrowed_text_texture_key(&plain_text_command(), 1.0, 3).to_owned(),
            base
        );
    }

    #[test]
    fn texture_cache_evicts_only_the_least_recently_used_entries() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let mut cache = TextureCache {
            byte_capacity: usize::MAX,
            ..TextureCache::default()
        };
        cache.insert(
            BorrowedTextureKey::Image(image_cache_key(1)),
            64,
            graphics.white.clone(),
        );
        let one_entry_bytes = cache.bytes;
        cache.insert(
            BorrowedTextureKey::Image(image_cache_key(2)),
            64,
            graphics.white.clone(),
        );
        assert_eq!(cache.bytes, one_entry_bytes * 2);
        cache.byte_capacity = one_entry_bytes * 2;
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(1)))
                .0
                .is_some()
        );
        let (evicted, _) = cache.insert(
            BorrowedTextureKey::Image(image_cache_key(3)),
            64,
            graphics.white.clone(),
        );
        assert_eq!(evicted, 1);
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(1)))
                .0
                .is_some()
        );
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(2)))
                .0
                .is_none()
        );
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(3)))
                .0
                .is_some()
        );
        assert!(cache.bytes <= cache.byte_capacity);
    }

    #[test]
    fn texture_cache_uses_exact_equality_inside_forced_hash_collision() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let mut cache = TextureCache::default();
        let requested = image_cache_key(7);
        let hash = BorrowedTextureKey::Image(requested).hash_with(&cache.hash_builder);
        cache.entries.insert(
            hash,
            vec![
                TextureCacheEntry {
                    key: TextureKey::Image(image_cache_key(8)),
                    texture: graphics.white.clone(),
                    resident_bytes: 1,
                    texture_bytes: 0,
                    key_and_metadata_bytes: 1,
                    last_used: 0,
                },
                TextureCacheEntry {
                    key: TextureKey::Image(requested),
                    texture: graphics.white.clone(),
                    resident_bytes: 1,
                    texture_bytes: 0,
                    key_and_metadata_bytes: 1,
                    last_used: 0,
                },
            ],
        );
        let (found, compared) = cache.get(&BorrowedTextureKey::Image(requested));
        assert!(found.is_some());
        assert_eq!(compared, std::mem::size_of::<ImageTextureKey>() * 2);
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(9)))
                .0
                .is_none()
        );
    }

    #[test]
    fn oversized_texture_is_returned_without_disturbing_residents() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let mut cache = TextureCache {
            byte_capacity: 256,
            ..TextureCache::default()
        };
        cache.insert(
            BorrowedTextureKey::Image(image_cache_key(1)),
            32,
            graphics.white.clone(),
        );
        let before = cache.bytes;
        let (evicted, _) = cache.insert(
            BorrowedTextureKey::Image(image_cache_key(2)),
            cache.byte_capacity,
            graphics.white.clone(),
        );
        assert_eq!(evicted, 0);
        assert_eq!(cache.bytes, before);
        assert_eq!(cache.oversized, 1);
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(1)))
                .0
                .is_some()
        );
    }

    #[test]
    fn lru_trace_preserves_hot_subset_better_than_clear_all() {
        fn replay(clear_all: bool) -> (usize, usize) {
            let mut resident = Vec::<u16>::new();
            let mut misses = 0;
            let mut evictions = 0;
            let trace = (0..40).flat_map(|cold| [1, 2, 1, 2, 100 + cold]);
            for key in trace {
                if let Some(index) = resident.iter().position(|entry| *entry == key) {
                    let hit = resident.remove(index);
                    resident.push(hit);
                    continue;
                }
                misses += 1;
                if resident.len() == 4 {
                    if clear_all {
                        evictions += resident.len();
                        resident.clear();
                    } else {
                        resident.remove(0);
                        evictions += 1;
                    }
                }
                resident.push(key);
            }
            (misses, evictions)
        }
        let selective = replay(false);
        let legacy = replay(true);
        assert!(
            selective.0 < legacy.0,
            "selective={selective:?} clear-all={legacy:?}"
        );
        assert!(
            selective.1 < legacy.1,
            "selective={selective:?} clear-all={legacy:?}"
        );
    }

    #[test]
    fn translated_text_reuses_texture_and_changed_text_does_not() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let command = plain_text_command();
        let bounds = match &command {
            PaintCommand::Text { bounds, .. } => *bounds,
            _ => unreachable!(),
        };
        let first = graphics.text_texture(&command, bounds, 1.0).unwrap();
        let mut translated = command.clone();
        let translated_bounds = match &mut translated {
            PaintCommand::Text { bounds, .. } => {
                bounds.origin.x += 37.0;
                bounds.origin.y += 19.0;
                *bounds
            }
            _ => unreachable!(),
        };
        let reused = graphics
            .text_texture(&translated, translated_bounds, 1.0)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &reused));

        let mut changed = translated;
        if let PaintCommand::Text { text, .. } = &mut changed {
            text.push('!');
        }
        let changed_texture = graphics
            .text_texture(&changed, translated_bounds, 1.0)
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &changed_texture));
        let cache = graphics.textures.borrow();
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.misses, 2);
        assert_eq!(cache.insertions, 2);
    }

    #[test]
    fn eviction_drops_only_cache_ownership_not_in_flight_handles() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let mut cache = TextureCache {
            byte_capacity: usize::MAX,
            ..TextureCache::default()
        };
        cache.insert(
            BorrowedTextureKey::Image(image_cache_key(1)),
            64,
            graphics.white.clone(),
        );
        let held = cache
            .get(&BorrowedTextureKey::Image(image_cache_key(1)))
            .0
            .expect("resident handle");
        cache.byte_capacity = cache.bytes + "mixed".len();
        cache.insert(
            BorrowedTextureKey::Text(BorrowedTextTextureKey::Plain {
                text: "mixed",
                width_bits: 10.0f32.to_bits(),
                height_bits: 10.0f32.to_bits(),
                command_scale_bits: 1.0f32.to_bits(),
                color: 0xffffffff,
                align: crate::TextAlign::Start,
                bold: false,
                wrap: false,
                presentation_scale_bits: 1.0f32.to_bits(),
                font_generation: 1,
            }),
            64,
            graphics.white.clone(),
        );
        assert!(
            cache
                .get(&BorrowedTextureKey::Image(image_cache_key(1)))
                .0
                .is_none()
        );
        assert!(Arc::strong_count(&held) >= 1);
        drop(cache);
        assert!(Arc::strong_count(&held) >= 1);
    }

    #[test]
    #[ignore = "release-profile typed text-cache hit microbenchmark"]
    fn typed_text_cache_hit_benchmark() {
        use std::hint::black_box;
        use std::time::Instant;

        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let command = plain_text_command();
        let scale = 2.0;
        let generation = ProcessFontSystem::new().generation();
        let mut cache = TextureCache::default();
        cache.insert(
            borrowed_text_texture_key(&command, scale, generation),
            64,
            graphics.white.clone(),
        );

        let mut local = command.clone();
        if let PaintCommand::Text { bounds, .. } = &mut local {
            bounds.origin.x = 0.0;
            bounds.origin.y = 0.0;
        }
        let legacy_key = format!("text:{scale:?}:{local:?}");
        let legacy = HashMap::from([(legacy_key, graphics.white.clone())]);
        let iterations = 50_000u32;
        let mut pairs = Vec::new();
        for pair in 0..7 {
            let legacy_started = Instant::now();
            for _ in 0..iterations {
                let mut local = black_box(&command).clone();
                if let PaintCommand::Text { bounds, .. } = &mut local {
                    bounds.origin.x = 0.0;
                    bounds.origin.y = 0.0;
                }
                let key = format!("text:{scale:?}:{local:?}");
                black_box(legacy.get(&key).expect("legacy resident hit"));
            }
            let legacy_ns = legacy_started.elapsed().as_nanos();
            let typed_started = Instant::now();
            for _ in 0..iterations {
                let key = borrowed_text_texture_key(black_box(&command), scale, generation);
                black_box(cache.get(&key).0.expect("typed resident hit"));
            }
            let typed_ns = typed_started.elapsed().as_nanos();
            pairs.push((legacy_ns, typed_ns));
            black_box(pair);
        }
        let legacy_ns = pairs.iter().map(|pair| pair.0).collect::<Vec<_>>();
        let typed_ns = pairs.iter().map(|pair| pair.1).collect::<Vec<_>>();
        println!(
            "typed_text_cache_hit_benchmark pairs={} iterations_per_arm={} legacy_ns={legacy_ns:?} typed_ns={typed_ns:?} adapter={:?} scope=CPU-construction-plus-resident-lookup",
            pairs.len(),
            iterations,
            graphics.adapter.get_info()
        );
    }

    type DrawStreamEntry = (Vertex, (u32, u32, u32, u32), DrawKind, usize);

    fn prepared_draw_stream(scene: &PreparedScene) -> Vec<DrawStreamEntry> {
        let mut stream = Vec::new();
        let mut expected_base = 0u32;
        for (segment, base) in scene
            .segments
            .iter()
            .zip(scene.vertex_offsets.iter().copied())
        {
            assert_eq!(base, expected_base);
            for draw in &segment.draws {
                for index in draw.start..draw.end {
                    stream.push((
                        segment.vertices[index as usize],
                        draw.scissor,
                        draw.kind,
                        Arc::as_ptr(&draw.texture) as usize,
                    ));
                }
                assert!(draw.end as usize <= segment.vertices.len());
            }
            expected_base += segment.vertices.len() as u32;
        }
        stream
    }

    fn frame_draw_stream(frame: &Frame) -> Vec<DrawStreamEntry> {
        let mut stream = Vec::new();
        for draw in &frame.draws {
            for index in draw.start..draw.end {
                stream.push((
                    frame.vertices[index as usize],
                    draw.scissor,
                    draw.kind,
                    Arc::as_ptr(&draw.texture) as usize,
                ));
            }
        }
        stream
    }

    fn assert_matches_full(
        commands: &[PaintCommand],
        graphics: &GpuGraphics,
        scene: &PreparedScene,
    ) {
        let mut full = Frame::new(320, 240, 1.0, graphics.white.clone());
        full.prepare(commands, graphics);
        assert_eq!(scene.vertex_count, full.vertices.len());
        assert_eq!(prepared_draw_stream(scene), frame_draw_stream(&full));
    }

    #[test]
    fn retained_framebuffer_obeys_memory_budget() {
        assert_eq!(retained_framebuffer_bytes(1920, 1080), Some(8_294_400));
        assert_eq!(retained_framebuffer_bytes(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn retained_damage_selects_full_partial_and_reuse() {
        let commands = [PaintCommand::Fill {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: 0xffffff,
        }];
        assert_eq!(
            offscreen_redraw(&commands, None, true).redraw,
            OffscreenRedraw::Full
        );
        assert_eq!(
            offscreen_redraw(&commands, Some(&[]), true),
            RedrawDecision {
                redraw: OffscreenRedraw::Reuse,
                validation_visits: 0,
                fallback_reason: "none",
            }
        );
        assert_eq!(
            offscreen_redraw(&commands, Some(&[]), true).redraw,
            OffscreenRedraw::Reuse
        );
        assert_eq!(
            offscreen_redraw(&commands, Some(&[Rect::new(2.0, 3.0, 4.0, 5.0)]), true).redraw,
            OffscreenRedraw::Partial(Rect::new(2.0, 3.0, 4.0, 5.0))
        );
    }

    #[test]
    fn physical_scissor_clips_to_surface() {
        assert_eq!(
            physical_scissor(Rect::new(-2.0, 3.0, 8.0, 5.0), 20, 20, 2.0),
            Some((0, 6, 12, 10))
        );
        assert_eq!(
            physical_scissor(Rect::new(30.0, 30.0, 2.0, 2.0), 20, 20, 1.0),
            None
        );
    }

    #[test]
    fn blur_forces_full_without_weakening_explicit_empty_reuse() {
        let commands = [
            PaintCommand::Fill {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                color: 0xff00ff,
            },
            PaintCommand::BackdropBlur {
                rect: Rect::new(2.0, 2.0, 4.0, 4.0),
                radius: 2.0,
                blur: 3.0,
            },
        ];
        let partial = offscreen_redraw(&commands, Some(&[Rect::new(1.0, 1.0, 2.0, 2.0)]), true);
        assert_eq!(partial.redraw, OffscreenRedraw::Full);
        assert_eq!(partial.validation_visits, 2);
        assert_eq!(partial.fallback_reason, "backdrop-blur");
        assert_eq!(
            offscreen_redraw(&commands, Some(&[]), true).redraw,
            OffscreenRedraw::Reuse
        );
    }

    #[test]
    fn prepared_fragments_preserve_order_clips_resources_and_sparse_reuse() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let image = Arc::new(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([255, 0, 0, 128]),
        ));
        let mut commands = vec![
            PaintCommand::Fill {
                rect: Rect::new(0.0, 0.0, 320.0, 240.0),
                color: 0xff101820,
            },
            PaintCommand::PushClip(Rect::new(10.0, 12.0, 180.0, 100.0)),
            PaintCommand::Fill {
                rect: Rect::new(4.0, 5.0, 90.0, 80.0),
                color: 0x80336699,
            },
            PaintCommand::PushClip(Rect::new(20.0, 20.0, 50.0, 40.0)),
            PaintCommand::Image {
                bounds: Rect::new(15.0, 16.0, 64.0, 48.0),
                id: 7,
                generation: 1,
                image: image.clone(),
                high_density: None,
            },
            PaintCommand::PopClip,
            PaintCommand::RoundedStroke {
                rect: Rect::new(30.0, 30.0, 100.0, 50.0),
                color: 0xc0ffffff,
                width: 2.0,
                radius: 8.0,
            },
            PaintCommand::PopClip,
            PaintCommand::OverlayFill {
                rect: Rect::new(40.0, 35.0, 120.0, 70.0),
                color: 0x6044cc88,
            },
        ];
        let configuration = (320, 240, 1.0f32.to_bits(), wgpu::TextureFormat::Rgba8Unorm);
        let initial =
            prepare_incremental_scene(&commands, configuration, 320, 240, 1.0, &graphics, None)
                .expect("initial prepared scene");
        assert_eq!(initial.rebuilt, commands.len());
        assert_matches_full(&commands, &graphics, &initial.scene);

        let unchanged = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(initial.scene),
        )
        .expect("unchanged prepared scene");
        assert_eq!(unchanged.rebuilt, 0);
        assert_eq!(unchanged.reused, commands.len());

        commands[2] = PaintCommand::Fill {
            rect: Rect::new(4.0, 5.0, 90.0, 80.0),
            color: 0x80aa2299,
        };
        let sparse = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(unchanged.scene),
        )
        .expect("sparse prepared scene");
        assert_eq!(sparse.rebuilt, 1);
        assert_eq!(sparse.reused, commands.len() - 1);
        assert_eq!(sparse.invalidated, 1);
        assert_matches_full(&commands, &graphics, &sparse.scene);

        commands[1] = PaintCommand::PushClip(Rect::new(11.0, 12.0, 170.0, 90.0));
        let clip_changed = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(sparse.scene),
        )
        .expect("clip-changed prepared scene");
        assert!(clip_changed.rebuilt > 1);
        assert!(clip_changed.reused > 0);
        assert_matches_full(&commands, &graphics, &clip_changed.scene);

        if let PaintCommand::Image { generation, .. } = &mut commands[4] {
            *generation += 1;
        }
        let resource_changed = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(clip_changed.scene),
        )
        .expect("resource-changed prepared scene");
        assert_eq!(resource_changed.rebuilt, 1);
        assert_matches_full(&commands, &graphics, &resource_changed.scene);

        commands[0] = PaintCommand::Fill {
            rect: Rect::new(0.0, 0.0, 320.0, 240.0),
            color: 0xff17242d,
        };
        commands[8] = PaintCommand::OverlayFill {
            rect: Rect::new(42.0, 37.0, 116.0, 68.0),
            color: 0x7044cc88,
        };
        let separated = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(resource_changed.scene),
        )
        .expect("separated changes prepared scene");
        assert_eq!(separated.rebuilt, 2);
        assert_eq!(separated.reused, commands.len() - 2);
        assert_matches_full(&commands, &graphics, &separated.scene);

        commands.swap(6, 8);
        commands.remove(7);
        let reordered = prepare_incremental_scene(
            &commands,
            configuration,
            320,
            240,
            1.0,
            &graphics,
            Some(separated.scene),
        )
        .expect("reordered prepared scene");
        assert!(reordered.rebuilt > 0);
        assert!(reordered.invalidated > 0);
        assert_matches_full(&commands, &graphics, &reordered.scene);

        let resized = prepare_incremental_scene(
            &commands,
            (321, 241, 1.0f32.to_bits(), wgpu::TextureFormat::Rgba8Unorm),
            321,
            241,
            1.0,
            &graphics,
            Some(reordered.scene),
        )
        .expect("resize-invalidated prepared scene");
        assert_eq!(resized.rebuilt, commands.len());
        assert_eq!(resized.reused, 0);

        let mut scene = resized.scene;
        for cycle in 0..32u32 {
            if cycle.is_multiple_of(2) {
                commands.push(PaintCommand::Fill {
                    rect: Rect::new(250.0, 180.0, 20.0, 20.0),
                    color: 0xff58d6c5,
                });
            } else {
                commands.pop();
            }
            let update = prepare_incremental_scene(
                &commands,
                (321, 241, 1.0f32.to_bits(), wgpu::TextureFormat::Rgba8Unorm),
                321,
                241,
                1.0,
                &graphics,
                Some(scene),
            )
            .expect("bounded remove/reinsert cycle");
            assert!(update.scene.estimated_bytes <= PREPARED_SCENE_BUDGET_BYTES);
            assert_eq!(update.scene.segments.len(), commands.len());
            scene = update.scene;
        }
    }

    #[test]
    fn prepared_fragment_budget_counts_cpu_and_gpu_resource_holds() {
        let image = Arc::new(image::RgbaImage::new(16, 8));
        let command = PaintCommand::Image {
            bounds: Rect::new(0.0, 0.0, 16.0, 8.0),
            id: 1,
            generation: 1,
            image,
            high_density: None,
        };
        // One CPU image plus one selected GPU texture payload.
        assert_eq!(paint_command_owned_bytes(&command, 1.0), 16 * 8 * 4 * 2);
    }

    #[test]
    fn oversized_prepared_scene_falls_back_instead_of_retaining_resources() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let command = PaintCommand::Fill {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: 0xff123456,
        };
        let clips = vec![Rect::new(0.0, 0.0, 320.0, 240.0)];
        let segment = Arc::new(PreparedSegment {
            command: command.clone(),
            entry_clips: clips.clone(),
            exit_clips: clips,
            vertices: Vec::new(),
            draws: Vec::new(),
            estimated_bytes: PREPARED_SCENE_BUDGET_BYTES + 1,
        });
        let configuration = (320, 240, 1.0f32.to_bits(), wgpu::TextureFormat::Rgba8Unorm);
        let previous = PreparedScene {
            configuration,
            segments: vec![segment],
            vertex_offsets: vec![0],
            vertex_count: 0,
            estimated_bytes: PREPARED_SCENE_BUDGET_BYTES + 1,
        };
        assert!(
            prepare_incremental_scene(
                &[command],
                configuration,
                320,
                240,
                1.0,
                &graphics,
                Some(previous),
            )
            .is_none()
        );
    }

    #[test]
    fn randomized_sparse_sequences_match_full_images_and_full_preparation() {
        let graphics = GpuGraphics::new_headless().expect("headless test GPU adapter");
        let configuration = (320, 240, 1.0f32.to_bits(), wgpu::TextureFormat::Rgba8Unorm);
        let mut commands = Vec::new();
        for index in 0..64u32 {
            let x = (index % 8) as f32 * 36.0 + 8.0;
            let y = (index / 8) as f32 * 27.0 + 8.0;
            commands.push(PaintCommand::RoundedFill {
                rect: Rect::new(x, y, 30.0, 21.0),
                color: 0x80204060 + index * 0x000101,
                radius: 3.0,
            });
        }
        // Box shadows enter the paint list as ordered, expanding translucent
        // rounded-fill layers; keep representative effect extents in the oracle.
        for layer in 0..3u32 {
            commands.push(PaintCommand::RoundedFill {
                rect: Rect::new(
                    106.0 - layer as f32,
                    76.0 - layer as f32,
                    88.0 + layer as f32 * 2.0,
                    48.0 + layer as f32 * 2.0,
                ),
                color: 0x18000000u32.saturating_sub(layer << 24),
                radius: 8.0 + layer as f32,
            });
        }
        // Overlapping translucent contributors make partial repaint order observable.
        commands.push(PaintCommand::OverlayFill {
            rect: Rect::new(30.0, 30.0, 220.0, 150.0),
            color: 0x3044cc88,
        });

        let initial =
            prepare_incremental_scene(&commands, configuration, 320, 240, 1.0, &graphics, None)
                .expect("random sequence initial scene");
        let mut prepared = initial.scene;
        let mut retained = SoftwareRenderer::new(320, 240, 1.0);
        retained.render(&commands);
        let mut state = 0x4d595df4d0f33173u64;
        for step in 0..96u32 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let index = (state as usize) % 64;
            if let PaintCommand::RoundedFill { color, rect, .. } = &mut commands[index] {
                *color ^= 0x0001_0305;
                rect.origin.x += if step.is_multiple_of(2) { 1.0 } else { -1.0 };
            }
            if step % 17 == 0
                && let PaintCommand::OverlayFill { color, .. } = commands.last_mut().unwrap()
            {
                *color ^= 0x0010_2010;
            }

            let update = prepare_incremental_scene(
                &commands,
                configuration,
                320,
                240,
                1.0,
                &graphics,
                Some(prepared),
            )
            .expect("random sparse prepared scene");
            assert!(update.rebuilt <= 2);
            assert_matches_full(&commands, &graphics, &update.scene);
            prepared = update.scene;

            let before = retained.pixels().to_vec();
            let damage = retained.render(&commands);
            let mut full = SoftwareRenderer::new(320, 240, 1.0);
            full.render(&commands);
            assert_eq!(retained.pixels(), full.pixels(), "step {step}");
            for (pixel_index, _) in before
                .iter()
                .zip(retained.pixels())
                .enumerate()
                .filter(|(_, (old, new))| old != new)
            {
                let x = (pixel_index % 320) as f32;
                let y = (pixel_index / 320) as f32;
                assert!(
                    damage.rects.iter().any(|rect| {
                        x >= rect.origin.x
                            && x < rect.origin.x + rect.size.width
                            && y >= rect.origin.y
                            && y < rect.origin.y + rect.size.height
                    }),
                    "changed pixel ({x}, {y}) escaped damage at step {step}: {damage:?}"
                );
            }
        }
    }
}
