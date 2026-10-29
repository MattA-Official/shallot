//! Surfaces and swapchains: the `GPUCanvasContext` analogue.

use super::resource::TextureShared;
use super::{check, conv, vk, Adapter, AdapterShared, Device, DeviceShared, InstanceShared};
use crate::*;
use std::sync::{Arc, Mutex};

/// Frames that may be queued for presentation at once (the browser's frame-latency limit).
const FRAMES_IN_FLIGHT: usize = 2;

struct Frame {
    acquire: vk::Semaphore,
    /// The present submission that last consumed `acquire`.
    submission: u64,
}

struct Swapchain {
    device: Arc<DeviceShared>,
    raw: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    config: SurfaceConfiguration,
    extent: vk::Extent2D,
    present_semaphores: Vec<vk::Semaphore>,
    frames: Vec<Frame>,
    next_frame: usize,
    outdated: bool,
    generation: u64,
}

pub(crate) struct SurfaceShared {
    instance: Arc<InstanceShared>,
    raw: vk::SurfaceKHR,
    swapchain: Mutex<Option<Swapchain>>,
    generations: Mutex<u64>,
}

unsafe impl Send for SurfaceShared {}
unsafe impl Sync for SurfaceShared {}

/// A window's presentable surface: `GPUCanvasContext`.
pub struct Surface {
    shared: Arc<SurfaceShared>,
}

/// What a surface supports on an adapter.
#[derive(Debug, Clone, Default)]
pub struct SurfaceCapabilities {
    pub formats: Vec<TextureFormat>,
    pub present_modes: Vec<PresentMode>,
    pub alpha_modes: Vec<CompositeAlphaMode>,
    pub usages: TextureUsage,
}

impl Surface {
    pub(crate) unsafe fn new(
        instance: &Arc<InstanceShared>,
        window: RawWindowHandle,
    ) -> Result<Surface, GpuError> {
        let mut raw = 0;
        match window {
            RawWindowHandle::Xlib { display, window } => {
                let Some(xlib) = &instance.xlib_surface else {
                    return Err(GpuError::Internal(
                        "the Vulkan driver does not support Xlib surfaces".into(),
                    ));
                };
                let info = vk::XlibSurfaceCreateInfoKHR {
                    s_type: vk::STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR,
                    p_next: std::ptr::null(),
                    flags: 0,
                    dpy: display,
                    window: window as std::ffi::c_ulong,
                };
                check(
                    (xlib.create_xlib_surface)(instance.raw, &info, std::ptr::null(), &mut raw),
                    "vkCreateXlibSurfaceKHR",
                )?;
            }
        }
        Ok(Surface {
            shared: Arc::new(SurfaceShared {
                instance: instance.clone(),
                raw,
                swapchain: Mutex::new(None),
                generations: Mutex::new(0),
            }),
        })
    }

    pub(crate) fn supported_by(&self, adapter: &AdapterShared) -> bool {
        let Some(fns) = &self.shared.instance.surface else {
            return false;
        };
        let mut supported = 0;
        let r = unsafe {
            (fns.get_physical_device_surface_support)(
                adapter.raw,
                adapter.queue_family,
                self.shared.raw,
                &mut supported,
            )
        };
        r == vk::SUCCESS && supported != 0 && adapter.has_extension(c"VK_KHR_swapchain")
    }

    fn surface_formats(&self, adapter: &AdapterShared) -> Vec<vk::SurfaceFormatKHR> {
        let Some(fns) = &self.shared.instance.surface else {
            return Vec::new();
        };
        unsafe {
            vk::enumerate(
                |count, items| {
                    (fns.get_physical_device_surface_formats)(
                        adapter.raw,
                        self.shared.raw,
                        count,
                        items,
                    )
                },
                vk::SurfaceFormatKHR::default(),
            )
        }
        .unwrap_or_default()
    }

    fn present_modes(&self, adapter: &AdapterShared) -> Vec<i32> {
        let Some(fns) = &self.shared.instance.surface else {
            return Vec::new();
        };
        unsafe {
            vk::enumerate(
                |count, items| {
                    (fns.get_physical_device_surface_present_modes)(
                        adapter.raw,
                        self.shared.raw,
                        count,
                        items,
                    )
                },
                0,
            )
        }
        .unwrap_or_default()
    }

    fn surface_capabilities(&self, adapter: &AdapterShared) -> Option<vk::SurfaceCapabilitiesKHR> {
        let fns = self.shared.instance.surface.as_ref()?;
        let mut caps = vk::SurfaceCapabilitiesKHR::default();
        let r = unsafe {
            (fns.get_physical_device_surface_capabilities)(adapter.raw, self.shared.raw, &mut caps)
        };
        (r == vk::SUCCESS).then_some(caps)
    }

    /// The formats, present modes and alpha modes this surface supports with `adapter`.
    pub fn capabilities(&self, adapter: &Adapter) -> SurfaceCapabilities {
        let adapter = &adapter.shared;
        if !self.supported_by(adapter) {
            return SurfaceCapabilities::default();
        }
        let mut formats: Vec<TextureFormat> = self
            .surface_formats(adapter)
            .iter()
            .filter(|f| f.color_space == vk::COLOR_SPACE_SRGB_NONLINEAR_KHR)
            .filter_map(|f| conv::surface_format(f.format))
            .collect();
        formats.dedup();
        let present_modes = self
            .present_modes(adapter)
            .into_iter()
            .filter_map(|m| match m {
                vk::PRESENT_MODE_FIFO_KHR => Some(PresentMode::Fifo),
                vk::PRESENT_MODE_MAILBOX_KHR => Some(PresentMode::Mailbox),
                vk::PRESENT_MODE_IMMEDIATE_KHR => Some(PresentMode::Immediate),
                _ => None,
            })
            .collect();
        let caps = self.surface_capabilities(adapter).unwrap_or_default();
        let mut alpha_modes = Vec::new();
        if caps.supported_composite_alpha
            & (vk::COMPOSITE_ALPHA_OPAQUE_BIT_KHR | vk::COMPOSITE_ALPHA_INHERIT_BIT_KHR)
            != 0
        {
            alpha_modes.push(CompositeAlphaMode::Opaque);
        }
        if caps.supported_composite_alpha & vk::COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR != 0 {
            alpha_modes.push(CompositeAlphaMode::Premultiplied);
        }
        let mut usages = TextureUsage::RENDER_ATTACHMENT;
        if caps.supported_usage_flags & vk::IMAGE_USAGE_TRANSFER_SRC_BIT != 0 {
            usages |= TextureUsage::COPY_SRC;
        }
        if caps.supported_usage_flags & vk::IMAGE_USAGE_TRANSFER_DST_BIT != 0 {
            usages |= TextureUsage::COPY_DST;
        }
        if caps.supported_usage_flags & vk::IMAGE_USAGE_SAMPLED_BIT != 0 {
            usages |= TextureUsage::TEXTURE_BINDING;
        }
        SurfaceCapabilities {
            formats,
            present_modes,
            alpha_modes,
            usages,
        }
    }

    /// `configure`. Call again after the window resizes.
    pub fn configure(&self, device: &Device, config: &SurfaceConfiguration) {
        let mut slot = self.shared.swapchain.lock().unwrap();
        let old = slot.take();
        if let Some(old) = &old {
            unsafe { (old.device.fns.device_wait_idle)(old.device.raw) };
        }
        match self.create_swapchain(&device.shared, config, old) {
            Ok(swapchain) => *slot = Some(swapchain),
            Err(e) => device.shared.report(e),
        }
    }

    /// `unconfigure`.
    pub fn unconfigure(&self) {
        if let Some(old) = self.shared.swapchain.lock().unwrap().take() {
            destroy_swapchain(old, true);
        }
    }

    fn create_swapchain(
        &self,
        device: &Arc<DeviceShared>,
        config: &SurfaceConfiguration,
        old: Option<Swapchain>,
    ) -> Result<Swapchain, GpuError> {
        let adapter = &device.adapter;
        let fail = |message: String, old: Option<Swapchain>| {
            if let Some(old) = old {
                destroy_swapchain(old, true);
            }
            Err(GpuError::Validation(format!("configure: {message}")))
        };
        let Some(swapchain_fns) = &device.swapchain else {
            return fail(
                "the device was created without swapchain support".into(),
                old,
            );
        };
        if !self.supported_by(adapter) {
            return fail("the adapter cannot present to this surface".into(), old);
        }
        let caps = self.surface_capabilities(adapter).unwrap_or_default();
        let vk_format = adapter.vk_format(config.format);
        let formats = self.surface_formats(adapter);
        let Some(surface_format) = formats
            .iter()
            .find(|f| f.format == vk_format && f.color_space == vk::COLOR_SPACE_SRGB_NONLINEAR_KHR)
        else {
            return fail(
                format!(
                    "format {} is not supported by this surface",
                    config.format.name()
                ),
                old,
            );
        };
        let usage = conv::texture_usage(config.usage, config.format);
        if usage & !caps.supported_usage_flags != 0 {
            return fail(
                "a requested usage is not supported by this surface".into(),
                old,
            );
        }
        let composite_alpha = match config.alpha_mode {
            CompositeAlphaMode::Opaque
                if caps.supported_composite_alpha & vk::COMPOSITE_ALPHA_OPAQUE_BIT_KHR != 0 =>
            {
                vk::COMPOSITE_ALPHA_OPAQUE_BIT_KHR
            }
            CompositeAlphaMode::Opaque => vk::COMPOSITE_ALPHA_INHERIT_BIT_KHR,
            CompositeAlphaMode::Premultiplied
                if caps.supported_composite_alpha & vk::COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR
                    != 0 =>
            {
                vk::COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR
            }
            CompositeAlphaMode::Premultiplied => {
                return fail("premultiplied alpha is not supported".into(), old)
            }
        };
        let wanted_mode = conv::present_mode(config.present_mode);
        let present_mode = if self.present_modes(adapter).contains(&wanted_mode) {
            wanted_mode
        } else {
            vk::PRESENT_MODE_FIFO_KHR
        };
        let extent = if caps.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: config
                    .width
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: config
                    .height
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        } else {
            caps.current_extent
        };
        if extent.width == 0 || extent.height == 0 || config.width == 0 || config.height == 0 {
            return fail("the surface has zero size".into(), old);
        }
        let mut image_count = (caps.min_image_count + 1).max(3);
        if caps.max_image_count != 0 {
            image_count = image_count.min(caps.max_image_count);
        }
        let info = vk::SwapchainCreateInfoKHR {
            s_type: vk::STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR,
            p_next: std::ptr::null(),
            flags: 0,
            surface: self.shared.raw,
            min_image_count: image_count,
            image_format: surface_format.format,
            image_color_space: surface_format.color_space,
            image_extent: extent,
            image_array_layers: 1,
            image_usage: usage,
            image_sharing_mode: vk::SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
            pre_transform: if caps.supported_transforms & vk::SURFACE_TRANSFORM_IDENTITY_BIT_KHR
                != 0
            {
                vk::SURFACE_TRANSFORM_IDENTITY_BIT_KHR
            } else {
                caps.current_transform
            },
            composite_alpha,
            present_mode,
            clipped: vk::TRUE,
            old_swapchain: old.as_ref().map_or(0, |o| o.raw),
        };
        let mut raw = 0;
        let r = unsafe {
            (swapchain_fns.create_swapchain)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        let (frames, present_semaphores) = match old {
            Some(old) => {
                let kept = (old.frames, old.present_semaphores);
                unsafe {
                    (swapchain_fns.destroy_swapchain)(old.device.raw, old.raw, std::ptr::null())
                };
                kept
            }
            None => (Vec::new(), Vec::new()),
        };
        let cleanup = |frames: Vec<Frame>, semaphores: Vec<vk::Semaphore>| unsafe {
            for f in frames {
                (device.fns.destroy_semaphore)(device.raw, f.acquire, std::ptr::null());
            }
            for s in semaphores {
                (device.fns.destroy_semaphore)(device.raw, s, std::ptr::null());
            }
        };
        if let Err(e) = check(r, "vkCreateSwapchainKHR") {
            cleanup(frames, present_semaphores);
            return Err(e);
        }
        let images = unsafe {
            vk::enumerate(
                |count, items| (swapchain_fns.get_swapchain_images)(device.raw, raw, count, items),
                0,
            )
        };
        let images = match images {
            Ok(images) => images,
            Err(r) => {
                unsafe { (swapchain_fns.destroy_swapchain)(device.raw, raw, std::ptr::null()) };
                cleanup(frames, present_semaphores);
                return Err(check(r, "vkGetSwapchainImagesKHR").unwrap_err());
            }
        };
        let new_semaphore = || -> Result<vk::Semaphore, GpuError> {
            let info = vk::SemaphoreCreateInfo {
                s_type: vk::STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
            };
            let mut s = 0;
            unsafe {
                check(
                    (device.fns.create_semaphore)(device.raw, &info, std::ptr::null(), &mut s),
                    "vkCreateSemaphore",
                )?
            };
            Ok(s)
        };
        let mut frames = frames;
        while frames.len() < FRAMES_IN_FLIGHT {
            frames.push(Frame {
                acquire: new_semaphore()?,
                submission: 0,
            });
        }
        let mut present_semaphores = present_semaphores;
        while present_semaphores.len() < images.len() {
            present_semaphores.push(new_semaphore()?);
        }
        let generation = {
            let mut g = self.shared.generations.lock().unwrap();
            *g += 1;
            *g
        };
        Ok(Swapchain {
            device: device.clone(),
            raw,
            images,
            config: config.clone(),
            extent,
            present_semaphores,
            frames,
            next_frame: 0,
            outdated: false,
            generation,
        })
    }

    /// `getCurrentTexture`. Present the result with [`SurfaceTexture::present`] (dropping it
    /// presents too, as a canvas does at the end of the frame).
    pub fn get_current_texture(&self) -> Result<SurfaceTexture, SurfaceError> {
        let mut slot = self.shared.swapchain.lock().unwrap();
        for attempt in 0..2 {
            let Some(swapchain) = slot.as_mut() else {
                return Err(SurfaceError::NotConfigured);
            };
            if swapchain.outdated {
                let config = swapchain.config.clone();
                let device = swapchain.device.clone();
                let old = slot.take();
                if let Some(old) = &old {
                    unsafe { (old.device.fns.device_wait_idle)(old.device.raw) };
                }
                match self.create_swapchain(&device, &config, old) {
                    Ok(new) => *slot = Some(new),
                    Err(_) => return Err(SurfaceError::Outdated),
                }
                continue;
            }
            let device = swapchain.device.clone();
            let frame = swapchain.next_frame % FRAMES_IN_FLIGHT;
            {
                let mut state = device.lock();
                device.wait_for(&mut state, swapchain.frames[frame].submission);
            }
            let fns = device.swapchain.as_ref().unwrap();
            let mut index = 0;
            let r = unsafe {
                (fns.acquire_next_image)(
                    device.raw,
                    swapchain.raw,
                    1_000_000_000,
                    swapchain.frames[frame].acquire,
                    0,
                    &mut index,
                )
            };
            match r {
                vk::SUCCESS | vk::SUBOPTIMAL_KHR => {
                    swapchain.next_frame += 1;
                    let config = &swapchain.config;
                    let texture = TextureShared::swapchain_image(
                        &device,
                        swapchain.images[index as usize],
                        config.format,
                        config.usage,
                        swapchain.extent.width,
                        swapchain.extent.height,
                        swapchain.frames[frame].acquire,
                    );
                    return Ok(SurfaceTexture {
                        texture: Texture {
                            shared: Arc::new(texture),
                        },
                        suboptimal: r == vk::SUBOPTIMAL_KHR,
                        surface: self.shared.clone(),
                        image_index: index,
                        frame,
                        generation: swapchain.generation,
                        presented: false,
                    });
                }
                vk::ERROR_OUT_OF_DATE_KHR if attempt == 0 => swapchain.outdated = true,
                vk::ERROR_OUT_OF_DATE_KHR => return Err(SurfaceError::Outdated),
                vk::TIMEOUT | vk::NOT_READY => return Err(SurfaceError::Timeout),
                vk::ERROR_SURFACE_LOST_KHR => return Err(SurfaceError::Lost),
                other => {
                    return Err(SurfaceError::Other(format!(
                        "vkAcquireNextImageKHR: {}",
                        vk::result_name(other)
                    )))
                }
            }
        }
        Err(SurfaceError::Outdated)
    }
}

impl Drop for SurfaceShared {
    fn drop(&mut self) {
        if let Some(swapchain) = self.swapchain.get_mut().unwrap().take() {
            destroy_swapchain(swapchain, true);
        }
        if let Some(fns) = &self.instance.surface {
            unsafe { (fns.destroy_surface)(self.instance.raw, self.raw, std::ptr::null()) };
        }
    }
}

fn destroy_swapchain(swapchain: Swapchain, wait: bool) {
    let device = &swapchain.device;
    unsafe {
        if wait {
            (device.fns.device_wait_idle)(device.raw);
        }
        let mut state = device.lock();
        device.retire(&mut state);
        drop(state);
        for frame in &swapchain.frames {
            (device.fns.destroy_semaphore)(device.raw, frame.acquire, std::ptr::null());
        }
        for &s in &swapchain.present_semaphores {
            (device.fns.destroy_semaphore)(device.raw, s, std::ptr::null());
        }
        if let Some(fns) = &device.swapchain {
            (fns.destroy_swapchain)(device.raw, swapchain.raw, std::ptr::null());
        }
    }
}

/// A texture to draw into this frame, from [`Surface::get_current_texture`].
pub struct SurfaceTexture {
    pub texture: Texture,
    /// The swapchain no longer matches the window exactly; reconfigure when convenient.
    pub suboptimal: bool,
    surface: Arc<SurfaceShared>,
    image_index: u32,
    frame: usize,
    generation: u64,
    presented: bool,
}

impl SurfaceTexture {
    /// Queues the texture for display.
    pub fn present(mut self) {
        self.present_now();
    }

    fn present_now(&mut self) {
        if std::mem::replace(&mut self.presented, true) {
            return;
        }
        let mut slot = self.surface.swapchain.lock().unwrap();
        let Some(swapchain) = slot.as_mut() else {
            return;
        };
        if swapchain.generation != self.generation {
            return;
        }
        let device = swapchain.device.clone();
        let semaphore = swapchain.present_semaphores[self.image_index as usize];
        let submitted = {
            let mut state = device.lock();
            device.submit_locked(
                &mut state,
                Vec::new(),
                Some((&self.texture.shared, semaphore)),
            )
        };
        let index = match submitted {
            Ok(index) => index,
            Err(e) => {
                drop(slot);
                device.report(e);
                return;
            }
        };
        swapchain.frames[self.frame].submission = index;
        let fns = device.swapchain.as_ref().unwrap();
        let info = vk::PresentInfoKHR {
            s_type: vk::STRUCTURE_TYPE_PRESENT_INFO_KHR,
            p_next: std::ptr::null(),
            wait_semaphore_count: 1,
            p_wait_semaphores: &semaphore,
            swapchain_count: 1,
            p_swapchains: &swapchain.raw,
            p_image_indices: &self.image_index,
            p_results: std::ptr::null_mut(),
        };
        let r = unsafe { (fns.queue_present)(device.queue, &info) };
        if r == vk::ERROR_OUT_OF_DATE_KHR || r == vk::SUBOPTIMAL_KHR {
            swapchain.outdated = true;
        }
    }
}

impl Drop for SurfaceTexture {
    fn drop(&mut self) {
        self.present_now();
    }
}
