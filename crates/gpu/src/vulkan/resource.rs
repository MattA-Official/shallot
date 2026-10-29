//! Buffers, textures, shader modules and pipelines.

use super::{check, conv, vk, Device, DeviceShared};
use crate::*;
use std::ffi::{c_void, CString};
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

fn label(label: Option<&str>) -> String {
    label.unwrap_or("").to_string()
}

fn named(kind: &str, label: &str) -> String {
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

pub(crate) struct BufferShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::Buffer,
    memory: vk::DeviceMemory,
    /// Persistent mapping of host-visible memory, or null.
    mapped: *mut u8,
    pub size: u64,
    pub usage: BufferUsage,
    pub label: String,
    pub valid: bool,
    pub state: Mutex<BufferState>,
}

unsafe impl Send for BufferShared {}
unsafe impl Sync for BufferShared {}

impl BufferShared {
    pub fn describe(&self) -> String {
        named("buffer", &self.label)
    }

    /// Creates a buffer. `host_visible` forces mappable memory (staging and map usages).
    fn create(
        device: &Arc<DeviceShared>,
        size: u64,
        usage: BufferUsage,
        host_visible: bool,
        label: String,
    ) -> Result<BufferShared, GpuError> {
        let info = vk::BufferCreateInfo {
            s_type: vk::STRUCTURE_TYPE_BUFFER_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            size: size.max(4),
            usage: conv::buffer_usage(usage),
            sharing_mode: vk::SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
        };
        unsafe {
            let mut raw = 0;
            check(
                (device.fns.create_buffer)(device.raw, &info, std::ptr::null(), &mut raw),
                "vkCreateBuffer",
            )?;
            let mut requirements = vk::MemoryRequirements::default();
            (device.fns.get_buffer_memory_requirements)(device.raw, raw, &mut requirements);
            let (required, preferred) = if host_visible {
                let cached = if usage.contains(BufferUsage::MAP_READ) {
                    vk::MEMORY_PROPERTY_HOST_CACHED_BIT
                } else {
                    0
                };
                (
                    vk::MEMORY_PROPERTY_HOST_VISIBLE_BIT | vk::MEMORY_PROPERTY_HOST_COHERENT_BIT,
                    cached,
                )
            } else {
                (0, vk::MEMORY_PROPERTY_DEVICE_LOCAL_BIT)
            };
            let memory = match device.allocate(requirements, required, preferred) {
                Ok(m) => m,
                Err(e) => {
                    (device.fns.destroy_buffer)(device.raw, raw, std::ptr::null());
                    return Err(e);
                }
            };
            (device.fns.bind_buffer_memory)(device.raw, raw, memory, 0);
            let mut mapped = std::ptr::null_mut();
            if host_visible {
                let mut ptr: *mut c_void = std::ptr::null_mut();
                check(
                    (device.fns.map_memory)(device.raw, memory, 0, vk::WHOLE_SIZE, 0, &mut ptr),
                    "vkMapMemory",
                )?;
                mapped = ptr as *mut u8;
            }
            Ok(BufferShared {
                device: device.clone(),
                raw,
                memory,
                mapped,
                size,
                usage,
                label,
                valid: true,
                state: Mutex::new(BufferState {
                    map: MapState::Unmapped,
                    last_use: 0,
                    destroyed: false,
                    staging: None,
                    views: 0,
                }),
            })
        }
    }

    fn invalid(
        device: &Arc<DeviceShared>,
        size: u64,
        usage: BufferUsage,
        label: String,
    ) -> BufferShared {
        BufferShared {
            device: device.clone(),
            raw: 0,
            memory: 0,
            mapped: std::ptr::null_mut(),
            size,
            usage,
            label,
            valid: false,
            state: Mutex::new(BufferState {
                map: MapState::Unmapped,
                last_use: 0,
                destroyed: false,
                staging: None,
                views: 0,
            }),
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
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.mapped, data.len()) };
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

impl Drop for BufferShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_buffer)(self.device.raw, self.raw, std::ptr::null());
                (self.device.fns.free_memory)(self.device.raw, self.memory, std::ptr::null());
            }
        }
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
        let base = match &state.staging {
            Some(staging) => staging.mapped,
            None => shared.mapped,
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
    pub raw: vk::Image,
    /// Zero for swapchain images, which the swapchain owns.
    memory: vk::DeviceMemory,
    owned: bool,
    pub size: Extent3d,
    pub format: TextureFormat,
    pub usage: TextureUsage,
    pub dimension: TextureDimension,
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub label: String,
    pub valid: bool,
    pub destroyed: Mutex<bool>,
    /// Current layout of every subresource; transitions are whole-texture for now.
    pub layout: Mutex<i32>,
    /// For swapchain images: the semaphore the first submission using the image must wait on.
    pub acquire: Mutex<Option<vk::Semaphore>>,
}

unsafe impl Send for TextureShared {}
unsafe impl Sync for TextureShared {}

impl TextureShared {
    pub fn describe(&self) -> String {
        named("texture", &self.label)
    }

    pub fn array_layers(&self) -> u32 {
        match self.dimension {
            TextureDimension::D3 => 1,
            _ => self.size.depth_or_array_layers,
        }
    }

    pub fn full_range(&self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: conv::aspect_mask(self.format, TextureAspect::All),
            base_mip_level: 0,
            level_count: self.mip_level_count,
            base_array_layer: 0,
            layer_count: self.array_layers(),
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
    #[allow(clippy::too_many_arguments)]
    pub fn swapchain_image(
        device: &Arc<DeviceShared>,
        raw: vk::Image,
        format: TextureFormat,
        usage: TextureUsage,
        width: u32,
        height: u32,
        acquire: vk::Semaphore,
    ) -> TextureShared {
        TextureShared {
            device: device.clone(),
            raw,
            memory: 0,
            owned: false,
            size: Extent3d::new(width, height, 1),
            format,
            usage,
            dimension: TextureDimension::D2,
            mip_level_count: 1,
            sample_count: 1,
            label: "surface texture".into(),
            valid: true,
            destroyed: Mutex::new(false),
            layout: Mutex::new(vk::IMAGE_LAYOUT_UNDEFINED),
            acquire: Mutex::new(Some(acquire)),
        }
    }
}

impl Drop for TextureShared {
    fn drop(&mut self) {
        if self.owned && self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_image)(self.device.raw, self.raw, std::ptr::null());
                (self.device.fns.free_memory)(self.device.raw, self.memory, std::ptr::null());
            }
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
            raw: 0,
            memory: 0,
            owned: false,
            size: desc.size,
            format: desc.format,
            usage: desc.usage,
            dimension: desc.dimension,
            mip_level_count: desc.mip_level_count,
            sample_count: desc.sample_count,
            label: name,
            valid: false,
            destroyed: Mutex::new(false),
            layout: Mutex::new(vk::IMAGE_LAYOUT_UNDEFINED),
            acquire: Mutex::new(None),
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

        let vk_format = device.adapter.vk_format(desc.format);
        let array_layers = if desc.dimension == TextureDimension::D3 {
            1
        } else {
            size.depth_or_array_layers
        };
        let depth = if desc.dimension == TextureDimension::D3 {
            size.depth_or_array_layers
        } else {
            1
        };
        let cube_compatible = desc.dimension == TextureDimension::D2
            && array_layers >= 6
            && size.width == size.height;
        let info = vk::ImageCreateInfo {
            s_type: vk::STRUCTURE_TYPE_IMAGE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: if cube_compatible {
                vk::IMAGE_CREATE_CUBE_COMPATIBLE_BIT
            } else {
                0
            },
            image_type: match desc.dimension {
                TextureDimension::D1 => vk::IMAGE_TYPE_1D,
                TextureDimension::D2 => vk::IMAGE_TYPE_2D,
                TextureDimension::D3 => vk::IMAGE_TYPE_3D,
            },
            format: vk_format,
            extent: vk::Extent3D {
                width: size.width,
                height: size.height,
                depth,
            },
            mip_levels: desc.mip_level_count,
            array_layers,
            samples: desc.sample_count,
            tiling: vk::IMAGE_TILING_OPTIMAL,
            usage: conv::texture_usage(desc.usage, desc.format),
            sharing_mode: vk::SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
            initial_layout: vk::IMAGE_LAYOUT_UNDEFINED,
        };
        let result = unsafe {
            let mut raw = 0;
            check(
                (device.fns.create_image)(device.raw, &info, std::ptr::null(), &mut raw),
                "vkCreateImage",
            )
            .and_then(|_| {
                let mut requirements = vk::MemoryRequirements::default();
                (device.fns.get_image_memory_requirements)(device.raw, raw, &mut requirements);
                match device.allocate(requirements, 0, vk::MEMORY_PROPERTY_DEVICE_LOCAL_BIT) {
                    Ok(memory) => {
                        (device.fns.bind_image_memory)(device.raw, raw, memory, 0);
                        Ok((raw, memory))
                    }
                    Err(e) => {
                        (device.fns.destroy_image)(device.raw, raw, std::ptr::null());
                        Err(e)
                    }
                }
            })
        };
        match result {
            Ok((raw, memory)) => Texture {
                shared: Arc::new(TextureShared {
                    device: device.clone(),
                    raw,
                    memory,
                    owned: true,
                    size,
                    format: desc.format,
                    usage: desc.usage,
                    dimension: desc.dimension,
                    mip_level_count: desc.mip_level_count,
                    sample_count: desc.sample_count,
                    label: name,
                    valid: true,
                    destroyed: Mutex::new(false),
                    layout: Mutex::new(vk::IMAGE_LAYOUT_UNDEFINED),
                    acquire: Mutex::new(None),
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
        let view = |raw: vk::ImageView, valid: bool| TextureView {
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
            return view(0, false);
        }
        let info = vk::ImageViewCreateInfo {
            s_type: vk::STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            image: texture.raw,
            view_type: match dimension {
                TextureViewDimension::D1 => vk::IMAGE_VIEW_TYPE_1D,
                TextureViewDimension::D2 => vk::IMAGE_VIEW_TYPE_2D,
                TextureViewDimension::D2Array => vk::IMAGE_VIEW_TYPE_2D_ARRAY,
                TextureViewDimension::Cube => vk::IMAGE_VIEW_TYPE_CUBE,
                TextureViewDimension::CubeArray => vk::IMAGE_VIEW_TYPE_CUBE_ARRAY,
                TextureViewDimension::D3 => vk::IMAGE_VIEW_TYPE_3D,
            },
            format: device.adapter.vk_format(format),
            components: vk::ComponentMapping::default(),
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: conv::aspect_mask(format, desc.aspect),
                base_mip_level: desc.base_mip_level,
                level_count: mip_level_count,
                base_array_layer: desc.base_array_layer,
                layer_count: array_layer_count,
            },
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_image_view)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        match check(r, "vkCreateImageView") {
            Ok(()) => view(raw, true),
            Err(e) => {
                device.report(e);
                view(0, false)
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
    pub raw: vk::ImageView,
    pub format: TextureFormat,
    // Read by bind groups, which come next.
    #[allow(dead_code)]
    pub dimension: TextureViewDimension,
    #[allow(dead_code)]
    pub aspect: TextureAspect,
    pub base_mip_level: u32,
    pub mip_level_count: u32,
    #[allow(dead_code)]
    pub base_array_layer: u32,
    pub array_layer_count: u32,
    pub label: String,
    pub valid: bool,
}

unsafe impl Send for TextureViewShared {}
unsafe impl Sync for TextureViewShared {}

impl TextureViewShared {
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

impl Drop for TextureViewShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            let device = &self.texture.device;
            unsafe { (device.fns.destroy_image_view)(device.raw, self.raw, std::ptr::null()) };
        }
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
    pub device: Arc<DeviceShared>,
    pub module: Option<shallot_wgsl::Module>,
    pub raw: vk::ShaderModule,
    pub messages: Vec<CompilationMessage>,
}

unsafe impl Send for ShaderModuleShared {}
unsafe impl Sync for ShaderModuleShared {}

impl Drop for ShaderModuleShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_shader_module)(self.device.raw, self.raw, std::ptr::null())
            };
        }
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
                    device: device.clone(),
                    module: None,
                    raw: 0,
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
        let words = match module.to_spirv(&shallot_wgsl::SpirvOptions::default()) {
            Ok(w) => w,
            Err(e) => {
                let message = CompilationMessage {
                    message: e.message.clone(),
                    kind: CompilationMessageType::Error,
                    line_num: 0,
                    line_pos: 0,
                    offset: 0,
                    length: 0,
                };
                return fail(vec![message], e.message);
            }
        };
        let info = vk::ShaderModuleCreateInfo {
            s_type: vk::STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            code_size: words.len() * 4,
            p_code: words.as_ptr(),
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_shader_module)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        if let Err(e) = check(r, "vkCreateShaderModule") {
            device.report(e);
            raw = 0;
        }
        ShaderModule {
            shared: Arc::new(ShaderModuleShared {
                device: device.clone(),
                module: if raw != 0 { Some(module) } else { None },
                raw,
                messages: Vec::new(),
            }),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Pipeline layouts and render pipelines

pub(crate) struct PipelineLayoutShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::PipelineLayout,
}

unsafe impl Send for PipelineLayoutShared {}
unsafe impl Sync for PipelineLayoutShared {}

impl Drop for PipelineLayoutShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_pipeline_layout)(
                    self.device.raw,
                    self.raw,
                    std::ptr::null(),
                )
            };
        }
    }
}

impl PipelineLayoutShared {
    fn empty(device: &Arc<DeviceShared>) -> Result<Arc<PipelineLayoutShared>, GpuError> {
        let info = vk::PipelineLayoutCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            set_layout_count: 0,
            p_set_layouts: std::ptr::null(),
            push_constant_range_count: 0,
            p_push_constant_ranges: std::ptr::null(),
        };
        let mut raw = 0;
        unsafe {
            check(
                (device.fns.create_pipeline_layout)(device.raw, &info, std::ptr::null(), &mut raw),
                "vkCreatePipelineLayout",
            )?
        };
        Ok(Arc::new(PipelineLayoutShared {
            device: device.clone(),
            raw,
        }))
    }
}

/// `GPUPipelineLayoutDescriptor`. Bind group layouts arrive with bind groups.
#[derive(Debug, Clone, Default)]
pub struct PipelineLayoutDescriptor<'a> {
    pub label: Option<&'a str>,
}

/// `GPUPipelineLayout`.
#[derive(Clone)]
pub struct PipelineLayout {
    pub(crate) shared: Arc<PipelineLayoutShared>,
}

impl Device {
    /// `createPipelineLayout`. Only layouts without bind groups exist so far.
    pub fn create_pipeline_layout(&self, _desc: &PipelineLayoutDescriptor<'_>) -> PipelineLayout {
        match PipelineLayoutShared::empty(&self.shared) {
            Ok(shared) => PipelineLayout { shared },
            Err(e) => {
                self.shared.report(e);
                PipelineLayout {
                    shared: Arc::new(PipelineLayoutShared {
                        device: self.shared.clone(),
                        raw: 0,
                    }),
                }
            }
        }
    }
}

pub(crate) struct RenderPipelineShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::Pipeline,
    pub _layout: Option<Arc<PipelineLayoutShared>>,
    pub color_formats: Vec<Option<TextureFormat>>,
    pub depth_stencil_format: Option<TextureFormat>,
    pub sample_count: u32,
    /// Number of vertex buffer slots the pipeline reads.
    pub vertex_buffers: u32,
    pub label: String,
    pub valid: bool,
}

unsafe impl Send for RenderPipelineShared {}
unsafe impl Sync for RenderPipelineShared {}

impl Drop for RenderPipelineShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_pipeline)(self.device.raw, self.raw, std::ptr::null())
            };
        }
    }
}

impl RenderPipelineShared {
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
fn find_entry<'m>(
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
                    raw: 0,
                    _layout: None,
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

        let layout = match &desc.layout {
            PipelineLayoutChoice::Layout(layout) => layout.shared.clone(),
            PipelineLayoutChoice::Auto => {
                let mut uses = vertex_reflection.bindings_for(vertex_entry).count();
                if let Some((_, reflection, entry)) = &fragment {
                    uses += reflection.bindings_for(entry).count();
                }
                if uses > 0 {
                    return Err(v("shaders with resource bindings need bind groups, which are not implemented yet".into()));
                }
                PipelineLayoutShared::empty(device).map_err(PipelineError::Device)?
            }
        };

        // Vertex inputs.
        let mut bindings = Vec::new();
        let mut attributes = Vec::new();
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
            bindings.push(vk::VertexInputBindingDescription {
                binding: slot as u32,
                stride: layout.array_stride as u32,
                input_rate: match layout.step_mode {
                    VertexStepMode::Vertex => vk::VERTEX_INPUT_RATE_VERTEX,
                    VertexStepMode::Instance => vk::VERTEX_INPUT_RATE_INSTANCE,
                },
            });
            for attribute in layout.attributes {
                if layout.array_stride != 0
                    && attribute.offset + attribute.format.size() > layout.array_stride
                {
                    return Err(v(format!(
                        "vertex attribute {} overruns its stride",
                        attribute.shader_location
                    )));
                }
                if attributes
                    .iter()
                    .any(|a: &vk::VertexInputAttributeDescription| {
                        a.location == attribute.shader_location
                    })
                {
                    return Err(v(format!(
                        "vertex attribute location {} is used twice",
                        attribute.shader_location
                    )));
                }
                attributes.push(vk::VertexInputAttributeDescription {
                    location: attribute.shader_location,
                    binding: slot as u32,
                    format: conv::vertex_format(attribute.format),
                    offset: attribute.offset as u32,
                });
            }
        }
        for input in &vertex_entry.inputs {
            if let shallot_wgsl::ir::IoBinding::Location { location, .. } = input.io {
                if !attributes.iter().any(|a| a.location == location) {
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
        let mut vk_color_formats = Vec::new();
        let mut blends = Vec::new();
        for (i, target) in targets.iter().enumerate() {
            let Some(target) = target else {
                color_formats.push(None);
                vk_color_formats.push(vk::FORMAT_UNDEFINED);
                blends.push(vk::PipelineColorBlendAttachmentState::default());
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
            vk_color_formats.push(device.adapter.vk_format(target.format));
            let blend = target.blend.unwrap_or_default();
            blends.push(vk::PipelineColorBlendAttachmentState {
                blend_enable: target.blend.is_some() as u32,
                src_color_blend_factor: conv::blend_factor(blend.color.src_factor),
                dst_color_blend_factor: conv::blend_factor(blend.color.dst_factor),
                color_blend_op: conv::blend_op(blend.color.operation),
                src_alpha_blend_factor: conv::blend_factor(blend.alpha.src_factor),
                dst_alpha_blend_factor: conv::blend_factor(blend.alpha.dst_factor),
                alpha_blend_op: conv::blend_op(blend.alpha.operation),
                color_write_mask: target.write_mask.0,
            });
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

        // Vulkan structures.
        let entry_names: Vec<CString> = std::iter::once(&vertex_entry.name)
            .chain(fragment.as_ref().map(|(_, _, e)| &e.name))
            .map(|n| CString::new(n.as_str()).unwrap())
            .collect();
        let mut stages = vec![vk::PipelineShaderStageCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            stage: vk::SHADER_STAGE_VERTEX_BIT,
            module: vertex_module.raw,
            p_name: entry_names[0].as_ptr(),
            p_specialization_info: std::ptr::null(),
        }];
        if let Some((f, _, _)) = &fragment {
            stages.push(vk::PipelineShaderStageCreateInfo {
                s_type: vk::STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                stage: vk::SHADER_STAGE_FRAGMENT_BIT,
                module: f.module.shared.raw,
                p_name: entry_names[1].as_ptr(),
                p_specialization_info: std::ptr::null(),
            });
        }
        let vertex_input = vk::PipelineVertexInputStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            vertex_binding_description_count: bindings.len() as u32,
            p_vertex_binding_descriptions: bindings.as_ptr(),
            vertex_attribute_description_count: attributes.len() as u32,
            p_vertex_attribute_descriptions: attributes.as_ptr(),
        };
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            topology: conv::topology(desc.primitive.topology),
            primitive_restart_enable: (strip && desc.primitive.strip_index_format.is_some()) as u32,
        };
        let viewport = vk::PipelineViewportStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            viewport_count: 1,
            p_viewports: std::ptr::null(),
            scissor_count: 1,
            p_scissors: std::ptr::null(),
        };
        let depth_bias = desc
            .depth_stencil
            .map(|d| (d.depth_bias, d.depth_bias_slope_scale, d.depth_bias_clamp));
        let (bias, slope, clamp) = depth_bias.unwrap_or((0, 0.0, 0.0));
        let rasterization = vk::PipelineRasterizationStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            depth_clamp_enable: vk::FALSE,
            rasterizer_discard_enable: vk::FALSE,
            polygon_mode: vk::POLYGON_MODE_FILL,
            cull_mode: match desc.primitive.cull_mode {
                CullMode::None => vk::CULL_MODE_NONE,
                CullMode::Front => vk::CULL_MODE_FRONT_BIT,
                CullMode::Back => vk::CULL_MODE_BACK_BIT,
            },
            // The flipped viewport keeps WebGPU's winding, so this maps directly.
            front_face: match desc.primitive.front_face {
                FrontFace::Ccw => vk::FRONT_FACE_COUNTER_CLOCKWISE,
                FrontFace::Cw => vk::FRONT_FACE_CLOCKWISE,
            },
            depth_bias_enable: (bias != 0 || slope != 0.0) as u32,
            depth_bias_constant_factor: bias as f32,
            depth_bias_clamp: clamp,
            depth_bias_slope_factor: slope,
            line_width: 1.0,
        };
        let sample_mask = desc.multisample.mask;
        let multisample = vk::PipelineMultisampleStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            rasterization_samples: desc.multisample.count,
            sample_shading_enable: vk::FALSE,
            min_sample_shading: 0.0,
            p_sample_mask: &sample_mask,
            alpha_to_coverage_enable: desc.multisample.alpha_to_coverage_enabled as u32,
            alpha_to_one_enable: vk::FALSE,
        };
        let depth_stencil = desc
            .depth_stencil
            .map(|ds| vk::PipelineDepthStencilStateCreateInfo {
                s_type: vk::STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                depth_test_enable: ds.format.has_depth() as u32,
                depth_write_enable: ds.depth_write_enabled as u32,
                depth_compare_op: conv::compare(ds.depth_compare),
                depth_bounds_test_enable: vk::FALSE,
                stencil_test_enable: ds.format.has_stencil() as u32,
                front: conv::stencil_face(
                    &ds.stencil_front,
                    ds.stencil_read_mask,
                    ds.stencil_write_mask,
                ),
                back: conv::stencil_face(
                    &ds.stencil_back,
                    ds.stencil_read_mask,
                    ds.stencil_write_mask,
                ),
                min_depth_bounds: 0.0,
                max_depth_bounds: 1.0,
            });
        let color_blend = vk::PipelineColorBlendStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            logic_op_enable: vk::FALSE,
            logic_op: 0,
            attachment_count: blends.len() as u32,
            p_attachments: blends.as_ptr(),
            blend_constants: [0.0; 4],
        };
        let dynamic_states = [
            vk::DYNAMIC_STATE_VIEWPORT,
            vk::DYNAMIC_STATE_SCISSOR,
            vk::DYNAMIC_STATE_BLEND_CONSTANTS,
            vk::DYNAMIC_STATE_STENCIL_REFERENCE,
        ];
        let dynamic = vk::PipelineDynamicStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            dynamic_state_count: dynamic_states.len() as u32,
            p_dynamic_states: dynamic_states.as_ptr(),
        };
        let ds_format = desc.depth_stencil.map(|d| d.format);
        let vk_ds_format = ds_format
            .map(|f| device.adapter.vk_format(f))
            .unwrap_or(vk::FORMAT_UNDEFINED);
        let rendering = vk::PipelineRenderingCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_RENDERING_CREATE_INFO,
            p_next: std::ptr::null(),
            view_mask: 0,
            color_attachment_count: vk_color_formats.len() as u32,
            p_color_attachment_formats: vk_color_formats.as_ptr(),
            depth_attachment_format: if ds_format.is_some_and(|f| f.has_depth()) {
                vk_ds_format
            } else {
                vk::FORMAT_UNDEFINED
            },
            stencil_attachment_format: if ds_format.is_some_and(|f| f.has_stencil()) {
                vk_ds_format
            } else {
                vk::FORMAT_UNDEFINED
            },
        };
        let info = vk::GraphicsPipelineCreateInfo {
            s_type: vk::STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
            p_next: &rendering as *const _ as *const c_void,
            flags: 0,
            stage_count: stages.len() as u32,
            p_stages: stages.as_ptr(),
            p_vertex_input_state: &vertex_input,
            p_input_assembly_state: &input_assembly,
            p_tessellation_state: std::ptr::null(),
            p_viewport_state: &viewport,
            p_rasterization_state: &rasterization,
            p_multisample_state: &multisample,
            p_depth_stencil_state: depth_stencil
                .as_ref()
                .map_or(std::ptr::null(), |d| d as *const _),
            p_color_blend_state: &color_blend,
            p_dynamic_state: &dynamic,
            layout: layout.raw,
            render_pass: 0,
            subpass: 0,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_graphics_pipelines)(
                device.raw,
                0,
                1,
                &info,
                std::ptr::null(),
                &mut raw,
            )
        };
        check(r, "vkCreateGraphicsPipelines").map_err(PipelineError::Device)?;
        Ok(RenderPipeline {
            shared: Arc::new(RenderPipelineShared {
                device: device.clone(),
                raw,
                _layout: Some(layout),
                color_formats,
                depth_stencil_format: ds_format,
                sample_count: desc.multisample.count,
                vertex_buffers: desc.vertex.buffers.len() as u32,
                label: name.to_string(),
                valid: true,
            }),
        })
    }
}

enum PipelineError {
    Validation(String),
    Device(GpuError),
}
