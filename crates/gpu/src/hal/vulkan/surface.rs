//! Vulkan surfaces and swapchains.

use super::resource::Texture;
use super::{check, conv, vk, Adapter, Device, DeviceInner, InstanceInner};
use crate::hal;
use crate::*;
use std::sync::{Arc, Mutex};

/// Frames that may be queued for presentation at once (the browser's frame-latency limit).
pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

struct Swapchain {
    device: Arc<DeviceInner>,
    raw: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    config: SurfaceConfiguration,
    extent: vk::Extent2D,
    present_semaphores: Vec<vk::Semaphore>,
    /// One acquire semaphore per frame in flight.
    acquire: Vec<vk::Semaphore>,
    outdated: bool,
    generation: u64,
}

pub(crate) struct Surface {
    instance: Arc<InstanceInner>,
    raw: vk::SurfaceKHR,
    swapchain: Mutex<Option<Swapchain>>,
    generations: Mutex<u64>,
}

unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    pub(super) unsafe fn new(
        instance: &Arc<InstanceInner>,
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
            RawWindowHandle::Win32 { hinstance, hwnd } => {
                let Some(win32) = &instance.win32_surface else {
                    return Err(GpuError::Internal(
                        "the Vulkan driver does not support Win32 surfaces".into(),
                    ));
                };
                let info = vk::Win32SurfaceCreateInfoKHR {
                    s_type: vk::STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR,
                    p_next: std::ptr::null(),
                    flags: 0,
                    hinstance,
                    hwnd,
                };
                check(
                    (win32.create_win32_surface)(instance.raw, &info, std::ptr::null(), &mut raw),
                    "vkCreateWin32SurfaceKHR",
                )?;
            }
            RawWindowHandle::AppKit { ca_metal_layer, .. } => {
                let Some(metal) = &instance.metal_surface else {
                    return Err(GpuError::Internal(
                        "the Vulkan driver does not support Metal surfaces".into(),
                    ));
                };
                let info = vk::MetalSurfaceCreateInfoEXT {
                    s_type: vk::STRUCTURE_TYPE_METAL_SURFACE_CREATE_INFO_EXT,
                    p_next: std::ptr::null(),
                    flags: 0,
                    p_layer: ca_metal_layer,
                };
                check(
                    (metal.create_metal_surface)(instance.raw, &info, std::ptr::null(), &mut raw),
                    "vkCreateMetalSurfaceEXT",
                )?;
            }
        }
        Ok(Surface {
            instance: instance.clone(),
            raw,
            swapchain: Mutex::new(None),
            generations: Mutex::new(0),
        })
    }

    /// Whether `adapter` can present to this surface.
    pub fn supported(&self, adapter: &Adapter) -> bool {
        let adapter = &adapter.inner;
        let Some(fns) = &self.instance.surface else {
            return false;
        };
        let mut supported = 0;
        let r = unsafe {
            (fns.get_physical_device_surface_support)(
                adapter.raw,
                adapter.queue_family,
                self.raw,
                &mut supported,
            )
        };
        r == vk::SUCCESS && supported != 0 && adapter.has_extension(c"VK_KHR_swapchain")
    }

    fn surface_formats(&self, adapter: &super::AdapterInner) -> Vec<vk::SurfaceFormatKHR> {
        let Some(fns) = &self.instance.surface else {
            return Vec::new();
        };
        unsafe {
            vk::enumerate(
                |count, items| {
                    (fns.get_physical_device_surface_formats)(adapter.raw, self.raw, count, items)
                },
                vk::SurfaceFormatKHR::default(),
            )
        }
        .unwrap_or_default()
    }

    fn present_modes(&self, adapter: &super::AdapterInner) -> Vec<i32> {
        let Some(fns) = &self.instance.surface else {
            return Vec::new();
        };
        unsafe {
            vk::enumerate(
                |count, items| {
                    (fns.get_physical_device_surface_present_modes)(
                        adapter.raw,
                        self.raw,
                        count,
                        items,
                    )
                },
                0,
            )
        }
        .unwrap_or_default()
    }

    fn surface_capabilities(
        &self,
        adapter: &super::AdapterInner,
    ) -> Option<vk::SurfaceCapabilitiesKHR> {
        let fns = self.instance.surface.as_ref()?;
        let mut caps = vk::SurfaceCapabilitiesKHR::default();
        let r = unsafe {
            (fns.get_physical_device_surface_capabilities)(adapter.raw, self.raw, &mut caps)
        };
        (r == vk::SUCCESS).then_some(caps)
    }

    /// The formats, present modes and alpha modes this surface supports with `adapter`.
    pub fn capabilities(&self, adapter: &Adapter) -> SurfaceCapabilities {
        if !self.supported(adapter) {
            return SurfaceCapabilities::default();
        }
        let adapter = &adapter.inner;
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

    /// (Re)creates the swapchain. Errors in the configuration are validation errors.
    pub fn configure(
        &self,
        device: &Device,
        config: &SurfaceConfiguration,
    ) -> Result<(), GpuError> {
        let mut slot = self.swapchain.lock().unwrap();
        let old = slot.take();
        if let Some(old) = &old {
            unsafe { (old.device.fns.device_wait_idle)(old.device.raw) };
        }
        *slot = Some(self.create_swapchain(&device.inner, config, old)?);
        Ok(())
    }

    pub fn unconfigure(&self) {
        if let Some(old) = self.swapchain.lock().unwrap().take() {
            destroy_swapchain(old);
        }
    }

    fn create_swapchain(
        &self,
        device: &Arc<DeviceInner>,
        config: &SurfaceConfiguration,
        old: Option<Swapchain>,
    ) -> Result<Swapchain, GpuError> {
        let adapter = &device.adapter;
        let fail = |message: String, old: Option<Swapchain>| {
            if let Some(old) = old {
                destroy_swapchain(old);
            }
            Err(GpuError::Validation(format!("configure: {message}")))
        };
        let Some(swapchain_fns) = &device.swapchain else {
            return fail(
                "the device was created without swapchain support".into(),
                old,
            );
        };
        let supported = {
            let fns = self.instance.surface.as_ref();
            let mut supported = 0;
            fns.is_some_and(|fns| unsafe {
                (fns.get_physical_device_surface_support)(
                    adapter.raw,
                    adapter.queue_family,
                    self.raw,
                    &mut supported,
                ) == vk::SUCCESS
                    && supported != 0
            })
        };
        if !supported {
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
            surface: self.raw,
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
        let (acquire, present_semaphores) = match old {
            Some(old) => {
                let kept = (old.acquire, old.present_semaphores);
                unsafe {
                    (swapchain_fns.destroy_swapchain)(old.device.raw, old.raw, std::ptr::null())
                };
                kept
            }
            None => (Vec::new(), Vec::new()),
        };
        let cleanup = |semaphores: Vec<vk::Semaphore>| unsafe {
            for s in semaphores {
                (device.fns.destroy_semaphore)(device.raw, s, std::ptr::null());
            }
        };
        if let Err(e) = check(r, "vkCreateSwapchainKHR") {
            cleanup(acquire);
            cleanup(present_semaphores);
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
                cleanup(acquire);
                cleanup(present_semaphores);
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
        let mut acquire = acquire;
        while acquire.len() < FRAMES_IN_FLIGHT {
            acquire.push(new_semaphore()?);
        }
        let mut present_semaphores = present_semaphores;
        while present_semaphores.len() < images.len() {
            present_semaphores.push(new_semaphore()?);
        }
        let generation = {
            let mut g = self.generations.lock().unwrap();
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
            acquire,
            outdated: false,
            generation,
        })
    }

    /// Acquires the next image, using frame slot `frame`'s semaphore. The caller has waited
    /// for the submission that last used that slot. An outdated swapchain is recreated once.
    pub fn acquire(&self, frame: usize) -> Result<hal::Acquired, hal::AcquireError> {
        let mut slot = self.swapchain.lock().unwrap();
        for attempt in 0..2 {
            let Some(swapchain) = slot.as_mut() else {
                return Err(hal::AcquireError::Other(
                    "the surface is not configured".into(),
                ));
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
                    Err(_) => return Err(hal::AcquireError::Outdated),
                }
                continue;
            }
            let device = &swapchain.device;
            let fns = device.swapchain.as_ref().unwrap();
            let semaphore = swapchain.acquire[frame % FRAMES_IN_FLIGHT];
            let mut index = 0;
            let r = unsafe {
                (fns.acquire_next_image)(
                    device.raw,
                    swapchain.raw,
                    1_000_000_000,
                    semaphore,
                    0,
                    &mut index,
                )
            };
            match r {
                vk::SUCCESS | vk::SUBOPTIMAL_KHR => {
                    let texture = Texture::swapchain_image(
                        device,
                        swapchain.images[index as usize],
                        swapchain.config.format,
                        semaphore,
                    );
                    return Ok(hal::Acquired {
                        texture,
                        image: index,
                        suboptimal: r == vk::SUBOPTIMAL_KHR,
                        generation: swapchain.generation,
                    });
                }
                vk::ERROR_OUT_OF_DATE_KHR if attempt == 0 => swapchain.outdated = true,
                vk::ERROR_OUT_OF_DATE_KHR => return Err(hal::AcquireError::Outdated),
                vk::TIMEOUT | vk::NOT_READY => return Err(hal::AcquireError::Timeout),
                vk::ERROR_SURFACE_LOST_KHR => return Err(hal::AcquireError::Lost),
                other => {
                    return Err(hal::AcquireError::Other(format!(
                        "vkAcquireNextImageKHR: {}",
                        vk::result_name(other)
                    )))
                }
            }
        }
        Err(hal::AcquireError::Outdated)
    }

    /// The size of the current swapchain's images.
    pub fn extent(&self) -> (u32, u32) {
        self.swapchain
            .lock()
            .unwrap()
            .as_ref()
            .map_or((0, 0), |s| (s.extent.width, s.extent.height))
    }

    /// The generation of the current swapchain, or 0 when there is none.
    pub fn generation(&self) -> u64 {
        self.swapchain
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |s| s.generation)
    }

    /// The semaphore a submission presenting `image` signals.
    pub(super) fn present_semaphore(&self, image: u32) -> vk::Semaphore {
        let slot = self.swapchain.lock().unwrap();
        slot.as_ref().unwrap().present_semaphores[image as usize]
    }

    /// Queues `image` for display, after the submission that signalled its semaphore.
    pub fn present(&self, image: u32) {
        let mut slot = self.swapchain.lock().unwrap();
        let Some(swapchain) = slot.as_mut() else {
            return;
        };
        let device = &swapchain.device;
        let fns = device.swapchain.as_ref().unwrap();
        let semaphore = swapchain.present_semaphores[image as usize];
        let info = vk::PresentInfoKHR {
            s_type: vk::STRUCTURE_TYPE_PRESENT_INFO_KHR,
            p_next: std::ptr::null(),
            wait_semaphore_count: 1,
            p_wait_semaphores: &semaphore,
            swapchain_count: 1,
            p_swapchains: &swapchain.raw,
            p_image_indices: &image,
            p_results: std::ptr::null_mut(),
        };
        let r = {
            let _queue = device.command_pool.lock().unwrap();
            unsafe { (fns.queue_present)(device.queue, &info) }
        };
        if r == vk::ERROR_OUT_OF_DATE_KHR || r == vk::SUBOPTIMAL_KHR {
            swapchain.outdated = true;
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(swapchain) = self.swapchain.get_mut().unwrap().take() {
            destroy_swapchain(swapchain);
        }
        if let Some(fns) = &self.instance.surface {
            unsafe { (fns.destroy_surface)(self.instance.raw, self.raw, std::ptr::null()) };
        }
    }
}

fn destroy_swapchain(swapchain: Swapchain) {
    let device = &swapchain.device;
    unsafe {
        (device.fns.device_wait_idle)(device.raw);
        for &s in swapchain
            .acquire
            .iter()
            .chain(&swapchain.present_semaphores)
        {
            (device.fns.destroy_semaphore)(device.raw, s, std::ptr::null());
        }
        if let Some(fns) = &device.swapchain {
            (fns.destroy_swapchain)(device.raw, swapchain.raw, std::ptr::null());
        }
    }
}
