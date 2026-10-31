//! Buffers, textures, shader modules and pipelines.

use super::binding::{
    auto_layout, empty_layout, BindGroupLayout, BindGroupLayoutShared, PipelineLayoutShared,
    StageReflection,
};
use super::{Device, DeviceShared};
use crate::hal;
use crate::*;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

pub(crate) fn label(label: Option<&str>) -> String {
    label.unwrap_or("").to_string()
}

pub(crate) fn named(kind: &str, label: &str) -> String {
    if label.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} \"{label}\"")
    }
}

// ------------------------------------------------------------------------------------------
// Buffers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapState {
    Unmapped,
    Pending {
        mode: MapMode,
        offset: u64,
        size: u64,
    },
    Mapped {
        mode: MapMode,
        offset: u64,
        size: u64,
    },
}

pub(crate) struct BufferState {
    map: MapState,
    /// Index of the last submission that used the buffer; mapping waits for it.
    pub last_use: u64,
    pub destroyed: bool,
    /// Host-visible copy written while a device-local buffer is mapped at creation.
    staging: Option<Arc<BufferShared>>,
    views: usize,
}

impl BufferState {
    fn new() -> BufferState {
        BufferState {
            map: MapState::Unmapped,
            last_use: 0,
            destroyed: false,
            staging: None,
            views: 0,
        }
    }
}

pub(crate) struct BufferShared {
    pub device: Arc<DeviceShared>,
    /// `None` for an invalid buffer.
    pub raw: Option<hal::Buffer>,
    pub size: u64,
    pub usage: BufferUsage,
    pub label: String,
    pub valid: bool,
    pub state: Mutex<BufferState>,
}

impl BufferShared {
    pub fn describe(&self) -> String {
        named("buffer", &self.label)
    }

    /// The backend buffer of a buffer that has passed validation.
    pub fn hal(&self) -> &hal::Buffer {
        self.raw.as_ref().expect("a validated buffer")
    }

    /// Creates a buffer. `host_visible` forces mappable memory (staging and map usages).
    fn create(
        device: &Arc<DeviceShared>,
        size: u64,
        usage: BufferUsage,
        host_visible: bool,
        label: String,
    ) -> Result<BufferShared, GpuError> {
        let raw = device.raw.create_buffer(size, usage, host_visible)?;
        Ok(BufferShared {
            device: device.clone(),
            raw: Some(raw),
            size,
            usage,
            label,
            valid: true,
            state: Mutex::new(BufferState::new()),
        })
    }

    fn invalid(
        device: &Arc<DeviceShared>,
        size: u64,
        usage: BufferUsage,
        label: String,
    ) -> BufferShared {
        BufferShared {
            device: device.clone(),
            raw: None,
            size,
            usage,
            label,
            valid: false,
            state: Mutex::new(BufferState::new()),
        }
    }

    /// A host-visible upload buffer holding `data`.
    pub fn staging(device: &Arc<DeviceShared>, data: &[u8]) -> Result<Arc<BufferShared>, GpuError> {
        let buffer = BufferShared::create(
            device,
            data.len() as u64,
            BufferUsage::COPY_SRC,
            true,
            "staging".into(),
        )?;
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.hal().mapped(), data.len()) };
        Ok(Arc::new(buffer))
    }

    /// Whether the buffer can be used in a submission now.
    pub fn usable(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if !self.valid {
            return Err(format!("{} is invalid", self.describe()));
        }
        if state.destroyed {
            return Err(format!("{} is destroyed", self.describe()));
        }
        if state.map != MapState::Unmapped {
            return Err(format!("{} is mapped or pending map", self.describe()));
        }
        Ok(())
    }
}

/// `GPUBuffer`.
#[derive(Clone)]
pub struct Buffer {
    pub(crate) shared: Arc<BufferShared>,
}

impl Device {
    /// `createBuffer`. Invalid descriptors produce a validation error and an invalid buffer.
    pub fn create_buffer(&self, desc: &BufferDescriptor<'_>) -> Buffer {
        let device = &self.shared;
        let name = label(desc.label);
        let fail = |message: String| {
            device.validation(message);
            Buffer {
                shared: Arc::new(BufferShared::invalid(
                    device,
                    desc.size,
                    desc.usage,
                    label(desc.label),
                )),
            }
        };
        let usage = desc.usage;
        if usage.is_empty() {
            return fail(format!(
                "{}: usage must not be empty",
                named("buffer", &name)
            ));
        }
        if usage.contains(BufferUsage::MAP_READ)
            && !(BufferUsage::MAP_READ | BufferUsage::COPY_DST).contains(usage)
        {
            return fail(format!(
                "{}: MAP_READ may only be combined with COPY_DST",
                named("buffer", &name)
            ));
        }
        if usage.contains(BufferUsage::MAP_WRITE)
            && !(BufferUsage::MAP_WRITE | BufferUsage::COPY_SRC).contains(usage)
        {
            return fail(format!(
                "{}: MAP_WRITE may only be combined with COPY_SRC",
                named("buffer", &name)
            ));
        }
        if desc.mapped_at_creation && !desc.size.is_multiple_of(4) {
            return fail(format!(
                "{}: mappedAtCreation needs a size that is a multiple of 4",
                named("buffer", &name)
            ));
        }
        if desc.size > device.limits.max_buffer_size {
            return fail(format!(
                "{}: size {} exceeds maxBufferSize",
                named("buffer", &name),
                desc.size
            ));
        }
        let mappable = usage.intersects(BufferUsage::MAP_READ | BufferUsage::MAP_WRITE);
        let buffer = match BufferShared::create(device, desc.size, usage, mappable, name) {
            Ok(b) => b,
            Err(e) => {
                device.report(e);
                return Buffer {
                    shared: Arc::new(BufferShared::invalid(
                        device,
                        desc.size,
                        usage,
                        label(desc.label),
                    )),
                };
            }
        };
        if desc.mapped_at_creation {
            let mut state = buffer.state.lock().unwrap();
            if !mappable {
                match BufferShared::create(
                    device,
                    desc.size,
                    BufferUsage::COPY_SRC,
                    true,
                    "staging".into(),
                ) {
                    Ok(staging) => state.staging = Some(Arc::new(staging)),
                    Err(e) => device.report(e),
                }
            }
            state.map = MapState::Mapped {
                mode: MapMode::WRITE,
                offset: 0,
                size: desc.size,
            };
            drop(state);
        }
        Buffer {
            shared: Arc::new(buffer),
        }
    }
}

impl Buffer {
    pub fn size(&self) -> u64 {
        self.shared.size
    }

    pub fn usage(&self) -> BufferUsage {
        self.shared.usage
    }

    pub fn label(&self) -> &str {
        &self.shared.label
    }

    /// `mapAsync`. Resolves once the GPU has finished with the buffer.
    pub fn map_async(&self, mode: MapMode, offset: u64, size: Option<u64>) -> MapFuture {
        let shared = &self.shared;
        let size = size.unwrap_or(shared.size.saturating_sub(offset));
        let error = {
            let state = shared.state.lock().unwrap();
            if !shared.valid {
                Some(format!("{} is invalid", shared.describe()))
            } else if state.destroyed {
                Some(format!("{} is destroyed", shared.describe()))
            } else if state.map != MapState::Unmapped {
                Some(format!(
                    "{} is already mapped or pending map",
                    shared.describe()
                ))
            } else if mode != MapMode::READ && mode != MapMode::WRITE {
                Some("map mode must be exactly one of READ or WRITE".into())
            } else if mode == MapMode::READ && !shared.usage.contains(BufferUsage::MAP_READ) {
                Some(format!("{} lacks MAP_READ usage", shared.describe()))
            } else if mode == MapMode::WRITE && !shared.usage.contains(BufferUsage::MAP_WRITE) {
                Some(format!("{} lacks MAP_WRITE usage", shared.describe()))
            } else if !offset.is_multiple_of(8) || !size.is_multiple_of(4) {
                Some("map offset must be a multiple of 8 and size a multiple of 4".into())
            } else if offset.checked_add(size).is_none_or(|end| end > shared.size) {
                Some("map range is out of bounds".into())
            } else {
                None
            }
        };
        if let Some(message) = error {
            shared.device.validation(message.clone());
            return MapFuture {
                buffer: shared.clone(),
                target: 0,
                error: Some(MapError::Validation(message)),
            };
        }
        // Queued writes to this buffer must land before it maps.
        shared.device.flush_pending_writes();
        let mut state = shared.state.lock().unwrap();
        state.map = MapState::Pending { mode, offset, size };
        let target = state.last_use;
        MapFuture {
            buffer: shared.clone(),
            target,
            error: None,
        }
    }

    /// `getMappedRange` for reading. `size` defaults to the rest of the buffer.
    pub fn get_mapped_range(&self, offset: u64, size: Option<u64>) -> BufferView<'_> {
        let (ptr, len) = self.mapped_range(offset, size, false);
        BufferView {
            buffer: self,
            ptr,
            len,
        }
    }

    /// `getMappedRange` for writing; needs a `WRITE` mapping or `mapped_at_creation`.
    pub fn get_mapped_range_mut(&self, offset: u64, size: Option<u64>) -> BufferViewMut<'_> {
        let (ptr, len) = self.mapped_range(offset, size, true);
        BufferViewMut {
            buffer: self,
            ptr,
            len,
        }
    }

    fn mapped_range(&self, offset: u64, size: Option<u64>, write: bool) -> (*mut u8, usize) {
        let shared = &self.shared;
        let mut state = shared.state.lock().unwrap();
        let MapState::Mapped {
            mode,
            offset: map_offset,
            size: map_size,
        } = state.map
        else {
            panic!(
                "getMappedRange on {}, which is not mapped",
                shared.describe()
            );
        };
        let size = size.unwrap_or((map_offset + map_size).saturating_sub(offset));
        assert!(
            offset >= map_offset && offset + size <= map_offset + map_size,
            "getMappedRange range {offset}..{} is outside the mapped range",
            offset + size
        );
        assert!(
            offset.is_multiple_of(8) && size.is_multiple_of(4),
            "getMappedRange offset must be a multiple of 8, size of 4"
        );
        assert!(
            !write || mode == MapMode::WRITE,
            "{} is mapped for reading only",
            shared.describe()
        );
        let base = match (&state.staging, &shared.raw) {
            (Some(staging), _) => staging.hal().mapped(),
            (None, Some(raw)) => raw.mapped(),
            (None, None) => std::ptr::null_mut(),
        };
        assert!(!base.is_null(), "{} has no mapping", shared.describe());
        state.views += 1;
        (unsafe { base.add(offset as usize) }, size as usize)
    }

    /// `unmap`. Mapped views must have been dropped.
    pub fn unmap(&self) {
        let shared = &self.shared;
        let mut state = shared.state.lock().unwrap();
        assert_eq!(
            state.views,
            0,
            "drop every mapped view of {} before unmapping it",
            shared.describe()
        );
        match state.map {
            MapState::Unmapped => {}
            MapState::Pending { .. } => state.map = MapState::Unmapped,
            MapState::Mapped { .. } => {
                state.map = MapState::Unmapped;
                if let Some(staging) = state.staging.take() {
                    drop(state);
                    let size = shared.size;
                    shared.device.lock().pending_writes.push(
                        super::command::PendingWrite::Buffer {
                            staging,
                            dst: shared.clone(),
                            dst_offset: 0,
                            size,
                        },
                    );
                }
            }
        }
    }

    /// `destroy`. Later use is a validation error; memory is freed once the GPU is done.
    pub fn destroy(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.destroyed = true;
        state.map = MapState::Unmapped;
        state.staging = None;
    }
}

/// The future `map_async` returns.
pub struct MapFuture {
    buffer: Arc<BufferShared>,
    target: u64,
    error: Option<MapError>,
}

impl Future for MapFuture {
    type Output = Result<(), MapError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(error) = self.error.take() {
            return Poll::Ready(Err(error));
        }
        let buffer = self.buffer.clone();
        if !matches!(buffer.state.lock().unwrap().map, MapState::Pending { .. }) {
            return Poll::Ready(Err(MapError::Aborted));
        }
        if !buffer.device.is_complete(self.target) {
            buffer
                .device
                .register_waker(self.target, cx.waker().clone());
            return Poll::Pending;
        }
        let mut state = buffer.state.lock().unwrap();
        match state.map {
            MapState::Pending { mode, offset, size } => {
                state.map = MapState::Mapped { mode, offset, size };
                Poll::Ready(Ok(()))
            }
            _ => Poll::Ready(Err(MapError::Aborted)),
        }
    }
}

/// A mapped range for reading, from [`Buffer::get_mapped_range`].
pub struct BufferView<'a> {
    buffer: &'a Buffer,
    ptr: *mut u8,
    len: usize,
}

impl Deref for BufferView<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for BufferView<'_> {
    fn drop(&mut self) {
        self.buffer.shared.state.lock().unwrap().views -= 1;
    }
}

/// A mapped range for writing, from [`Buffer::get_mapped_range_mut`].
pub struct BufferViewMut<'a> {
    buffer: &'a Buffer,
    ptr: *mut u8,
    len: usize,
}

impl Deref for BufferViewMut<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for BufferViewMut<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for BufferViewMut<'_> {
    fn drop(&mut self) {
        self.buffer.shared.state.lock().unwrap().views -= 1;
    }
}

// ------------------------------------------------------------------------------------------
// Textures

pub(crate) struct TextureShared {
    pub device: Arc<DeviceShared>,
    /// `None` for an invalid texture.
    pub raw: Option<hal::Texture>,
    pub size: Extent3d,
    pub format: TextureFormat,
    pub usage: TextureUsage,
    pub dimension: TextureDimension,
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub label: String,
    pub valid: bool,
    pub destroyed: Mutex<bool>,
}

impl TextureShared {
    pub fn describe(&self) -> String {
        named("texture", &self.label)
    }

    /// The backend texture of a texture that has passed validation.
    pub fn hal(&self) -> &hal::Texture {
        self.raw.as_ref().expect("a validated texture")
    }

    pub fn array_layers(&self) -> u32 {
        match self.dimension {
            TextureDimension::D3 => 1,
            _ => self.size.depth_or_array_layers,
        }
    }

    pub fn usable(&self) -> Result<(), String> {
        if !self.valid {
            return Err(format!("{} is invalid", self.describe()));
        }
        if *self.destroyed.lock().unwrap() {
            return Err(format!("{} is destroyed", self.describe()));
        }
        Ok(())
    }

    /// Wraps a swapchain image.
    pub fn swapchain_image(
        device: &Arc<DeviceShared>,
        raw: hal::Texture,
        usage: TextureUsage,
        width: u32,
        height: u32,
    ) -> TextureShared {
        TextureShared {
            device: device.clone(),
            format: raw.format,
            raw: Some(raw),
            size: Extent3d::new(width, height, 1),
            usage,
            dimension: TextureDimension::D2,
            mip_level_count: 1,
            sample_count: 1,
            label: "surface texture".into(),
            valid: true,
            destroyed: Mutex::new(false),
        }
    }
}

/// `GPUTexture`.
#[derive(Clone)]
pub struct Texture {
    pub(crate) shared: Arc<TextureShared>,
}

impl Device {
    /// `createTexture`.
    pub fn create_texture(&self, desc: &TextureDescriptor<'_>) -> Texture {
        let device = &self.shared;
        let name = label(desc.label);
        let invalid = |name: String| TextureShared {
            device: device.clone(),
            raw: None,
            size: desc.size,
            format: desc.format,
            usage: desc.usage,
            dimension: desc.dimension,
            mip_level_count: desc.mip_level_count,
            sample_count: desc.sample_count,
            label: name,
            valid: false,
            destroyed: Mutex::new(false),
        };
        let size = desc.size;
        let limits = &device.limits;
        let error = if desc.usage.is_empty() {
            Some("usage must not be empty".to_string())
        } else if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 {
            Some("size must not be zero".into())
        } else if desc.mip_level_count == 0 {
            Some("mipLevelCount must be at least 1".into())
        } else if !matches!(desc.sample_count, 1 | 4) {
            Some("sampleCount must be 1 or 4".into())
        } else if desc.sample_count > 1
            && (desc.dimension != TextureDimension::D2
                || desc.mip_level_count != 1
                || size.depth_or_array_layers != 1)
        {
            Some("multisampled textures must be 2D with one mip level and one layer".into())
        } else {
            let (max, layers_max) = match desc.dimension {
                TextureDimension::D1 => (limits.max_texture_dimension_1d, 1),
                TextureDimension::D2 => (
                    limits.max_texture_dimension_2d,
                    limits.max_texture_array_layers,
                ),
                TextureDimension::D3 => (
                    limits.max_texture_dimension_3d,
                    limits.max_texture_dimension_3d,
                ),
            };
            let largest = size.width.max(if desc.dimension == TextureDimension::D1 {
                1
            } else {
                size.height
            });
            let max_mips = 32
                - largest
                    .max(if desc.dimension == TextureDimension::D3 {
                        size.depth_or_array_layers
                    } else {
                        1
                    })
                    .leading_zeros();
            if size.width > max || size.height > max || size.depth_or_array_layers > layers_max {
                Some("size exceeds the device's texture limits".into())
            } else if desc.mip_level_count > max_mips {
                Some(format!(
                    "mipLevelCount {} exceeds the maximum {max_mips}",
                    desc.mip_level_count
                ))
            } else if desc.dimension == TextureDimension::D1
                && (size.height != 1 || desc.format.is_depth_stencil())
            {
                Some("1D textures have height 1 and colour formats".into())
            } else if desc.format == TextureFormat::Depth32FloatStencil8
                && !device.features.contains(&Feature::Depth32FloatStencil8)
            {
                Some("depth32float-stencil8 needs the depth32float-stencil8 feature".into())
            } else {
                None
            }
        };
        if let Some(message) = error {
            device.validation(format!("{}: {message}", named("texture", &name)));
            return Texture {
                shared: Arc::new(invalid(name)),
            };
        }

        let result = device.raw.create_texture(desc);
        match result {
            Ok(raw) => Texture {
                shared: Arc::new(TextureShared {
                    device: device.clone(),
                    raw: Some(raw),
                    size,
                    format: desc.format,
                    usage: desc.usage,
                    dimension: desc.dimension,
                    mip_level_count: desc.mip_level_count,
                    sample_count: desc.sample_count,
                    label: name,
                    valid: true,
                    destroyed: Mutex::new(false),
                }),
            },
            Err(e) => {
                device.report(e);
                Texture {
                    shared: Arc::new(invalid(name)),
                }
            }
        }
    }
}

impl Texture {
    pub fn width(&self) -> u32 {
        self.shared.size.width
    }

    pub fn height(&self) -> u32 {
        self.shared.size.height
    }

    pub fn depth_or_array_layers(&self) -> u32 {
        self.shared.size.depth_or_array_layers
    }

    pub fn size(&self) -> Extent3d {
        self.shared.size
    }

    pub fn format(&self) -> TextureFormat {
        self.shared.format
    }

    pub fn usage(&self) -> TextureUsage {
        self.shared.usage
    }

    pub fn dimension(&self) -> TextureDimension {
        self.shared.dimension
    }

    pub fn mip_level_count(&self) -> u32 {
        self.shared.mip_level_count
    }

    pub fn sample_count(&self) -> u32 {
        self.shared.sample_count
    }

    pub fn destroy(&self) {
        *self.shared.destroyed.lock().unwrap() = true;
    }

    /// `createView`.
    pub fn create_view(&self, desc: &TextureViewDescriptor<'_>) -> TextureView {
        let texture = &self.shared;
        let device = &texture.device;
        let name = label(desc.label);
        let dimension = desc.dimension.unwrap_or(match texture.dimension {
            TextureDimension::D1 => TextureViewDimension::D1,
            TextureDimension::D2
                if texture.size.depth_or_array_layers > 1 && desc.array_layer_count != Some(1) =>
            {
                TextureViewDimension::D2Array
            }
            TextureDimension::D2 => TextureViewDimension::D2,
            TextureDimension::D3 => TextureViewDimension::D3,
        });
        let format = desc.format.unwrap_or(texture.format);
        let mip_level_count = desc
            .mip_level_count
            .unwrap_or(texture.mip_level_count.saturating_sub(desc.base_mip_level));
        let array_layer_count = desc.array_layer_count.unwrap_or(match dimension {
            TextureViewDimension::D1 | TextureViewDimension::D2 | TextureViewDimension::D3 => 1,
            TextureViewDimension::Cube => 6,
            _ => texture.array_layers().saturating_sub(desc.base_array_layer),
        });
        let error = if let Err(e) = texture.usable() {
            Some(e)
        } else if format != texture.format && !format_compatible(format, texture.format) {
            Some(format!(
                "view format {} is not compatible with {}",
                format.name(),
                texture.format.name()
            ))
        } else if mip_level_count == 0
            || desc.base_mip_level + mip_level_count > texture.mip_level_count
        {
            Some("mip level range is out of bounds".into())
        } else if array_layer_count == 0
            || desc.base_array_layer + array_layer_count > texture.array_layers()
        {
            Some("array layer range is out of bounds".into())
        } else {
            let ok = match (texture.dimension, dimension) {
                (TextureDimension::D1, TextureViewDimension::D1) => true,
                (TextureDimension::D2, TextureViewDimension::D2) => array_layer_count == 1,
                (TextureDimension::D2, TextureViewDimension::D2Array) => true,
                (TextureDimension::D2, TextureViewDimension::Cube) => array_layer_count == 6,
                (TextureDimension::D2, TextureViewDimension::CubeArray) => {
                    array_layer_count.is_multiple_of(6)
                }
                (TextureDimension::D3, TextureViewDimension::D3) => true,
                _ => false,
            };
            (!ok).then(|| format!("{dimension:?} views are not allowed for this texture"))
        };
        let view = |raw: Option<hal::TextureView>, valid: bool| TextureView {
            shared: Arc::new(TextureViewShared {
                texture: texture.clone(),
                raw,
                format,
                dimension,
                aspect: desc.aspect,
                base_mip_level: desc.base_mip_level,
                mip_level_count,
                base_array_layer: desc.base_array_layer,
                array_layer_count,
                label: name.clone(),
                valid,
            }),
        };
        if let Some(message) = error {
            device.validation(format!("{}: {message}", named("texture view", &name)));
            return view(None, false);
        }
        let range = hal::ViewDesc {
            format,
            dimension,
            aspect: desc.aspect,
            base_mip_level: desc.base_mip_level,
            mip_level_count,
            base_array_layer: desc.base_array_layer,
            array_layer_count,
        };
        match device.raw.create_view(texture.hal(), &range) {
            Ok(raw) => view(Some(raw), true),
            Err(e) => {
                device.report(e);
                view(None, false)
            }
        }
    }
}

/// sRGB and linear variants of a format may view each other (with `viewFormats`).
fn format_compatible(a: TextureFormat, b: TextureFormat) -> bool {
    use TextureFormat::*;
    matches!(
        (a, b),
        (Rgba8Unorm, Rgba8UnormSrgb)
            | (Rgba8UnormSrgb, Rgba8Unorm)
            | (Bgra8Unorm, Bgra8UnormSrgb)
            | (Bgra8UnormSrgb, Bgra8Unorm)
    )
}

pub(crate) struct TextureViewShared {
    pub texture: Arc<TextureShared>,
    /// `None` for an invalid view.
    pub raw: Option<hal::TextureView>,
    pub format: TextureFormat,
    pub dimension: TextureViewDimension,
    pub aspect: TextureAspect,
    pub base_mip_level: u32,
    pub mip_level_count: u32,
    // For finer layout tracking, which is per texture for now.
    #[allow(dead_code)]
    pub base_array_layer: u32,
    pub array_layer_count: u32,
    pub label: String,
    pub valid: bool,
}

impl TextureViewShared {
    /// The backend view of a view that has passed validation.
    pub fn hal(&self) -> &hal::TextureView {
        self.raw.as_ref().expect("a validated texture view")
    }

    /// Width and height of the view's base mip level.
    pub fn extent(&self) -> (u32, u32) {
        let size = self.texture.size;
        (
            (size.width >> self.base_mip_level).max(1),
            (size.height >> self.base_mip_level).max(1),
        )
    }

    pub fn describe(&self) -> String {
        named("texture view", &self.label)
    }
}

/// `GPUTextureView`.
#[derive(Clone)]
pub struct TextureView {
    pub(crate) shared: Arc<TextureViewShared>,
}

impl TextureView {
    pub fn texture(&self) -> Texture {
        Texture {
            shared: self.shared.texture.clone(),
        }
    }

    pub fn format(&self) -> TextureFormat {
        self.shared.format
    }
}

// ------------------------------------------------------------------------------------------
// Shader modules

pub(crate) struct ShaderModuleShared {
    pub module: Option<shallot_wgsl::Module>,
    /// `None` when compilation failed.
    pub raw: Option<hal::ShaderModule>,
    pub messages: Vec<CompilationMessage>,
}

impl ShaderModuleShared {
    /// The backend module of a module that compiled.
    pub fn hal(&self) -> &hal::ShaderModule {
        self.raw.as_ref().expect("a compiled shader module")
    }
}

/// `GPUShaderModule`.
#[derive(Clone)]
pub struct ShaderModule {
    pub(crate) shared: Arc<ShaderModuleShared>,
}

impl ShaderModule {
    /// `getCompilationInfo`: errors from compiling the WGSL, empty on success.
    pub fn get_compilation_info(&self) -> Vec<CompilationMessage> {
        self.shared.messages.clone()
    }

    /// The checked module, for reflection. `None` if compilation failed.
    pub fn reflection(&self) -> Option<&shallot_wgsl::Module> {
        self.shared.module.as_ref()
    }
}

impl Device {
    /// `createShaderModule`. Compile errors are reported as a validation error and through
    /// [`ShaderModule::get_compilation_info`].
    pub fn create_shader_module(&self, desc: &ShaderModuleDescriptor<'_>) -> ShaderModule {
        let device = &self.shared;
        let name = label(desc.label);
        let fail = |messages: Vec<CompilationMessage>, summary: String| {
            device.validation(format!("{}: {summary}", named("shader module", &name)));
            ShaderModule {
                shared: Arc::new(ShaderModuleShared {
                    module: None,
                    raw: None,
                    messages,
                }),
            }
        };
        let source = desc.code;
        let module = match shallot_wgsl::compile(source) {
            Ok(m) => m,
            Err(e) => {
                let (line, column) = e.location(source);
                let message = CompilationMessage {
                    message: e.message.clone(),
                    kind: CompilationMessageType::Error,
                    line_num: line as u32,
                    line_pos: column as u32,
                    offset: e.span.start as u32,
                    length: (e.span.end - e.span.start) as u32,
                };
                return fail(vec![message], e.render(source));
            }
        };
        match device.raw.create_shader_module(&module) {
            Ok(Ok(raw)) => ShaderModule {
                shared: Arc::new(ShaderModuleShared {
                    module: Some(module),
                    raw: Some(raw),
                    messages: Vec::new(),
                }),
            },
            Ok(Err(e)) => {
                let message = CompilationMessage {
                    message: e.message.clone(),
                    kind: CompilationMessageType::Error,
                    line_num: 0,
                    line_pos: 0,
                    offset: 0,
                    length: 0,
                };
                fail(vec![message], e.message)
            }
            Err(e) => {
                device.report(e);
                ShaderModule {
                    shared: Arc::new(ShaderModuleShared {
                        module: None,
                        raw: None,
                        messages: Vec::new(),
                    }),
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Render and compute pipelines

pub(crate) struct RenderPipelineShared {
    pub device: Arc<DeviceShared>,
    /// `None` for an invalid pipeline.
    pub raw: Option<hal::RenderPipeline>,
    pub layout: Option<Arc<PipelineLayoutShared>>,
    pub color_formats: Vec<Option<TextureFormat>>,
    pub depth_stencil_format: Option<TextureFormat>,
    pub sample_count: u32,
    /// Number of vertex buffer slots the pipeline reads.
    pub vertex_buffers: u32,
    pub label: String,
    pub valid: bool,
}

impl RenderPipelineShared {
    /// The backend pipeline of a pipeline that has passed validation.
    pub fn hal(&self) -> &hal::RenderPipeline {
        self.raw.as_ref().expect("a validated render pipeline")
    }

    pub fn describe(&self) -> String {
        named("render pipeline", &self.label)
    }
}

/// `GPURenderPipeline`.
#[derive(Clone)]
pub struct RenderPipeline {
    pub(crate) shared: Arc<RenderPipelineShared>,
}

/// Finds the entry point for a stage: the named one, or the module's only one.
pub(crate) fn find_entry<'m>(
    module: &'m shallot_wgsl::Module,
    name: Option<&str>,
    stage: shallot_wgsl::Stage,
) -> Result<&'m shallot_wgsl::EntryPoint, String> {
    let mut candidates = module.entry_points.iter().filter(|e| e.stage == stage);
    match name {
        Some(name) => module
            .entry_points
            .iter()
            .find(|e| e.name == name && e.stage == stage)
            .ok_or_else(|| format!("no {stage:?} entry point named `{name}`")),
        None => {
            let first = candidates
                .next()
                .ok_or_else(|| format!("the module has no {stage:?} entry point"))?;
            if candidates.next().is_some() {
                return Err(format!(
                    "the module has several {stage:?} entry points; name one"
                ));
            }
            Ok(first)
        }
    }
}

impl Device {
    /// `createRenderPipeline`.
    pub fn create_render_pipeline(&self, desc: &RenderPipelineDescriptor<'_>) -> RenderPipeline {
        let device = &self.shared;
        let name = label(desc.label);
        let invalid = |message: Option<String>| {
            if let Some(message) = message {
                device.validation(format!("{}: {message}", named("render pipeline", &name)));
            }
            RenderPipeline {
                shared: Arc::new(RenderPipelineShared {
                    device: device.clone(),
                    raw: None,
                    layout: None,
                    color_formats: Vec::new(),
                    depth_stencil_format: None,
                    sample_count: desc.multisample.count,
                    vertex_buffers: desc.vertex.buffers.len() as u32,
                    label: name.clone(),
                    valid: false,
                }),
            }
        };
        match self.render_pipeline(desc, &name) {
            Ok(pipeline) => pipeline,
            Err(PipelineError::Validation(message)) => invalid(Some(message)),
            Err(PipelineError::Device(e)) => {
                device.report(e);
                invalid(None)
            }
        }
    }

    fn render_pipeline(
        &self,
        desc: &RenderPipelineDescriptor<'_>,
        name: &str,
    ) -> Result<RenderPipeline, PipelineError> {
        let device = &self.shared;
        let v = PipelineError::Validation;
        let vertex_module = &desc.vertex.module.shared;
        let Some(vertex_reflection) = vertex_module.module.as_ref() else {
            return Err(v("the vertex shader module is invalid".into()));
        };
        let vertex_entry = find_entry(
            vertex_reflection,
            desc.vertex.entry_point,
            shallot_wgsl::Stage::Vertex,
        )
        .map_err(v)?;
        let fragment = match &desc.fragment {
            Some(fragment) => {
                let Some(reflection) = fragment.module.shared.module.as_ref() else {
                    return Err(v("the fragment shader module is invalid".into()));
                };
                let entry = find_entry(
                    reflection,
                    fragment.entry_point,
                    shallot_wgsl::Stage::Fragment,
                )
                .map_err(v)?;
                Some((fragment, reflection, entry))
            }
            None => None,
        };

        let mut reflections = vec![StageReflection {
            module: vertex_reflection,
            entry: vertex_entry,
            stage: ShaderStages::VERTEX,
        }];
        if let Some((_, reflection, entry)) = &fragment {
            reflections.push(StageReflection {
                module: reflection,
                entry,
                stage: ShaderStages::FRAGMENT,
            });
        }
        let layout = pipeline_layout(device, &desc.layout, &reflections)?;

        // Vertex inputs.
        let mut locations: Vec<u32> = Vec::new();
        if desc.vertex.buffers.len() as u32 > device.limits.max_vertex_buffers {
            return Err(v("too many vertex buffers".into()));
        }
        for (slot, layout) in desc.vertex.buffers.iter().enumerate() {
            if layout.array_stride % 4 != 0
                || layout.array_stride > device.limits.max_vertex_buffer_array_stride as u64
            {
                return Err(v(format!(
                    "vertex buffer {slot} has an invalid arrayStride"
                )));
            }
            for attribute in layout.attributes {
                if layout.array_stride != 0
                    && attribute.offset + attribute.format.size() > layout.array_stride
                {
                    return Err(v(format!(
                        "vertex attribute {} overruns its stride",
                        attribute.shader_location
                    )));
                }
                if locations.contains(&attribute.shader_location) {
                    return Err(v(format!(
                        "vertex attribute location {} is used twice",
                        attribute.shader_location
                    )));
                }
                locations.push(attribute.shader_location);
            }
        }
        for input in &vertex_entry.inputs {
            if let shallot_wgsl::ir::IoBinding::Location { location, .. } = input.io {
                if !locations.contains(&location) {
                    return Err(v(format!(
                        "the vertex shader reads @location({location}) but no buffer provides it"
                    )));
                }
            }
        }

        // Colour targets.
        let targets: &[Option<ColorTargetState>] =
            fragment.as_ref().map(|(f, _, _)| f.targets).unwrap_or(&[]);
        if targets.len() as u32 > device.limits.max_color_attachments {
            return Err(v("too many colour targets".into()));
        }
        let mut color_formats = Vec::new();
        for (i, target) in targets.iter().enumerate() {
            let Some(target) = target else {
                color_formats.push(None);
                continue;
            };
            if target.format.is_depth_stencil() {
                return Err(v(format!(
                    "colour target {i} has a depth or stencil format"
                )));
            }
            let written = fragment.as_ref().is_some_and(|(_, _, entry)| {
                entry.outputs.iter().any(|o| matches!(o.io, shallot_wgsl::ir::IoBinding::Location { location, .. } if location == i as u32))
            });
            if !written && !target.write_mask.is_empty() {
                return Err(v(format!(
                    "the fragment shader does not write colour target {i}"
                )));
            }
            color_formats.push(Some(target.format));
        }
        if let Some(ds) = &desc.depth_stencil {
            if !ds.format.is_depth_stencil() {
                return Err(v(
                    "depthStencil.format must be a depth or stencil format".into()
                ));
            }
        }
        if color_formats.iter().all(Option::is_none) && desc.depth_stencil.is_none() {
            return Err(v(
                "a render pipeline needs at least one colour target or a depth-stencil state"
                    .into(),
            ));
        }
        if !matches!(desc.multisample.count, 1 | 4) {
            return Err(v("multisample.count must be 1 or 4".into()));
        }
        if desc.primitive.unclipped_depth {
            return Err(v(
                "unclippedDepth needs the depth-clip-control feature".into()
            ));
        }
        let strip = matches!(
            desc.primitive.topology,
            PrimitiveTopology::LineStrip | PrimitiveTopology::TriangleStrip
        );
        if desc.primitive.strip_index_format.is_some() && !strip {
            return Err(v(
                "stripIndexFormat is only allowed with strip topologies".into()
            ));
        }

        let groups: Vec<&[BindGroupLayoutEntry]> =
            layout.groups.iter().map(|g| g.entries.as_slice()).collect();
        let Some(hal_layout) = layout.raw.as_ref() else {
            return Err(v("the pipeline layout is invalid".into()));
        };
        let raw = device
            .raw
            .create_render_pipeline(&hal::RenderPipelineDesc {
                layout: hal_layout,
                groups: &groups,
                vertex: hal::Stage {
                    module: vertex_module.hal(),
                    reflection: vertex_reflection,
                    entry: vertex_entry,
                },
                buffers: desc.vertex.buffers,
                fragment: fragment.as_ref().map(|(f, reflection, entry)| hal::Stage {
                    module: f.module.shared.hal(),
                    reflection,
                    entry,
                }),
                targets,
                primitive: desc.primitive,
                depth_stencil: desc.depth_stencil,
                multisample: desc.multisample,
            })
            .map_err(PipelineError::Device)?;
        Ok(RenderPipeline {
            shared: Arc::new(RenderPipelineShared {
                device: device.clone(),
                raw: Some(raw),
                layout: Some(layout),
                color_formats,
                depth_stencil_format: desc.depth_stencil.map(|d| d.format),
                sample_count: desc.multisample.count,
                vertex_buffers: desc.vertex.buffers.len() as u32,
                label: name.to_string(),
                valid: true,
            }),
        })
    }
}

pub(crate) enum PipelineError {
    Validation(String),
    Device(GpuError),
}

/// The layout a pipeline uses: the explicit one, checked against the shaders, or one derived
/// from them.
fn pipeline_layout(
    device: &Arc<DeviceShared>,
    choice: &PipelineLayoutChoice<'_>,
    stages: &[StageReflection<'_>],
) -> Result<Arc<PipelineLayoutShared>, PipelineError> {
    let v = PipelineError::Validation;
    match choice {
        PipelineLayoutChoice::Layout(layout) => {
            let layout = &layout.shared;
            if layout.raw.is_none() {
                return Err(v("the pipeline layout is invalid".into()));
            }
            layout.check_stages(stages).map_err(v)?;
            Ok(layout.clone())
        }
        PipelineLayoutChoice::Auto => auto_layout(device, stages).map_err(v),
    }
}

/// `getBindGroupLayout`, shared by both pipeline kinds.
fn bind_group_layout(
    device: &Arc<DeviceShared>,
    layout: Option<&Arc<PipelineLayoutShared>>,
    what: String,
    index: u32,
) -> BindGroupLayout {
    if let Some(group) = layout.and_then(|l| l.groups.get(index as usize)) {
        return BindGroupLayout {
            shared: group.clone(),
        };
    }
    // An index the shaders leave unused, within the limit, is an empty layout in WebGPU.
    if layout.is_some() && index < device.limits.max_bind_groups {
        return BindGroupLayout {
            shared: empty_layout(device),
        };
    }
    device.validation(format!("{what}: no bind group layout at index {index}"));
    BindGroupLayout {
        shared: BindGroupLayoutShared::invalid(String::new()),
    }
}

impl RenderPipeline {
    /// `getBindGroupLayout`.
    pub fn get_bind_group_layout(&self, index: u32) -> BindGroupLayout {
        let p = &self.shared;
        bind_group_layout(&p.device, p.layout.as_ref(), p.describe(), index)
    }
}

pub(crate) struct ComputePipelineShared {
    pub device: Arc<DeviceShared>,
    /// `None` for an invalid pipeline.
    pub raw: Option<hal::ComputePipeline>,
    pub layout: Option<Arc<PipelineLayoutShared>>,
    pub label: String,
    pub valid: bool,
}

impl ComputePipelineShared {
    /// The backend pipeline of a pipeline that has passed validation.
    pub fn hal(&self) -> &hal::ComputePipeline {
        self.raw.as_ref().expect("a validated compute pipeline")
    }

    pub fn describe(&self) -> String {
        named("compute pipeline", &self.label)
    }
}

/// `GPUComputePipeline`.
#[derive(Clone)]
pub struct ComputePipeline {
    pub(crate) shared: Arc<ComputePipelineShared>,
}

impl ComputePipeline {
    /// `getBindGroupLayout`.
    pub fn get_bind_group_layout(&self, index: u32) -> BindGroupLayout {
        let p = &self.shared;
        bind_group_layout(&p.device, p.layout.as_ref(), p.describe(), index)
    }
}

impl Device {
    /// `createComputePipeline`.
    pub fn create_compute_pipeline(&self, desc: &ComputePipelineDescriptor<'_>) -> ComputePipeline {
        let device = &self.shared;
        let name = label(desc.label);
        match self.compute_pipeline(desc) {
            Ok((raw, layout)) => ComputePipeline {
                shared: Arc::new(ComputePipelineShared {
                    device: device.clone(),
                    raw: Some(raw),
                    layout: Some(layout),
                    label: name,
                    valid: true,
                }),
            },
            Err(e) => {
                match e {
                    PipelineError::Validation(message) => device
                        .validation(format!("{}: {message}", named("compute pipeline", &name))),
                    PipelineError::Device(e) => device.report(e),
                }
                ComputePipeline {
                    shared: Arc::new(ComputePipelineShared {
                        device: device.clone(),
                        raw: None,
                        layout: None,
                        label: name,
                        valid: false,
                    }),
                }
            }
        }
    }

    fn compute_pipeline(
        &self,
        desc: &ComputePipelineDescriptor<'_>,
    ) -> Result<(hal::ComputePipeline, Arc<PipelineLayoutShared>), PipelineError> {
        let device = &self.shared;
        let v = PipelineError::Validation;
        let module = &desc.compute.module.shared;
        let Some(reflection) = module.module.as_ref() else {
            return Err(v("the compute shader module is invalid".into()));
        };
        let entry = find_entry(
            reflection,
            desc.compute.entry_point,
            shallot_wgsl::Stage::Compute,
        )
        .map_err(v)?;
        let [x, y, z] = entry.workgroup_size;
        let limits = &device.limits;
        if x > limits.max_compute_workgroup_size_x
            || y > limits.max_compute_workgroup_size_y
            || z > limits.max_compute_workgroup_size_z
            || x as u64 * y as u64 * z as u64 > limits.max_compute_invocations_per_workgroup as u64
        {
            return Err(v(format!(
                "@workgroup_size({x}, {y}, {z}) exceeds the device's limits"
            )));
        }
        let stages = [StageReflection {
            module: reflection,
            entry,
            stage: ShaderStages::COMPUTE,
        }];
        let layout = pipeline_layout(device, &desc.layout, &stages)?;
        let groups: Vec<&[BindGroupLayoutEntry]> =
            layout.groups.iter().map(|g| g.entries.as_slice()).collect();
        let Some(hal_layout) = layout.raw.as_ref() else {
            return Err(v("the pipeline layout is invalid".into()));
        };
        let raw = device
            .raw
            .create_compute_pipeline(&hal::ComputePipelineDesc {
                layout: hal_layout,
                groups: &groups,
                stage: hal::Stage {
                    module: module.hal(),
                    reflection,
                    entry,
                },
            })
            .map_err(PipelineError::Device)?;
        Ok((raw, layout))
    }
}
