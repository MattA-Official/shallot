//! The Vulkan backend: instance, adapter and device.

mod command;
mod conv;
mod resource;
mod surface;
pub mod vk;

pub use command::{CommandBuffer, CommandEncoder, Queue, RenderPassEncoder};
pub use resource::{
    Buffer, BufferView, BufferViewMut, MapFuture, PipelineLayout, PipelineLayoutDescriptor,
    RenderPipeline, ShaderModule, Texture, TextureView,
};
pub use surface::{Surface, SurfaceCapabilities, SurfaceTexture};

use crate::*;
use shallot_platform::Library;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) fn check(r: vk::Result, what: &str) -> Result<(), GpuError> {
    match r {
        vk::SUCCESS => Ok(()),
        vk::ERROR_OUT_OF_HOST_MEMORY | vk::ERROR_OUT_OF_DEVICE_MEMORY => Err(
            GpuError::OutOfMemory(format!("{what}: {}", vk::result_name(r))),
        ),
        _ => Err(GpuError::Internal(format!(
            "{what} failed: {}",
            vk::result_name(r)
        ))),
    }
}

// ------------------------------------------------------------------------------------------
// Instance

pub(crate) struct InstanceShared {
    _library: Library,
    pub raw: vk::Instance,
    pub fns: vk::InstanceFns,
    pub surface: Option<vk::SurfaceFns>,
    pub xlib_surface: Option<vk::XlibSurfaceFns>,
    pub api_version: u32,
}

unsafe impl Send for InstanceShared {}
unsafe impl Sync for InstanceShared {}

impl Drop for InstanceShared {
    fn drop(&mut self) {
        unsafe { (self.fns.destroy_instance)(self.raw, std::ptr::null()) };
    }
}

/// `navigator.gpu`: the entry point that finds adapters and creates surfaces.
#[derive(Clone)]
pub struct Instance {
    pub(crate) shared: Arc<InstanceShared>,
}

const LOADER_NAMES: &[&str] = if cfg!(windows) {
    &["vulkan-1.dll"]
} else if cfg!(target_os = "macos") {
    &["libvulkan.1.dylib", "libMoltenVK.dylib"]
} else {
    &["libvulkan.so.1", "libvulkan.so"]
};

impl Instance {
    /// Opens the Vulkan loader and creates an instance with every surface extension available.
    pub fn new(desc: &InstanceDescriptor) -> Result<Instance, GpuError> {
        let library = Library::open(LOADER_NAMES).map_err(|e| {
            GpuError::Internal(format!(
                "{e}: install a Vulkan driver (on Linux, mesa-vulkan-drivers provides one)"
            ))
        })?;
        unsafe {
            let get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr = library
                .require("vkGetInstanceProcAddr")
                .map_err(|e| GpuError::Internal(e.to_string()))?;
            let entry = vk::EntryFns::load(|name| {
                get_instance_proc_addr(std::ptr::null_mut(), name.as_ptr())
            })
            .map_err(|name| GpuError::Internal(format!("Vulkan loader is missing {name}")))?;

            let mut api_version = vk::API_VERSION_1_1;
            let enumerate_version: Option<unsafe extern "system" fn(*mut u32) -> vk::Result> = {
                let p = get_instance_proc_addr(
                    std::ptr::null_mut(),
                    c"vkEnumerateInstanceVersion".as_ptr(),
                );
                (!p.is_null()).then(|| std::mem::transmute::<*const c_void, _>(p))
            };
            if let Some(f) = enumerate_version {
                let mut v = 0;
                if f(&mut v) == vk::SUCCESS {
                    api_version = v.min(vk::API_VERSION_1_3);
                }
            }
            if api_version < vk::API_VERSION_1_1 {
                return Err(GpuError::Internal("Vulkan 1.1 or newer is required".into()));
            }

            let available = vk::enumerate(
                |count, items| {
                    (entry.enumerate_instance_extension_properties)(std::ptr::null(), count, items)
                },
                std::mem::zeroed::<vk::ExtensionProperties>(),
            )
            .map_err(|r| {
                GpuError::Internal(format!(
                    "listing instance extensions: {}",
                    vk::result_name(r)
                ))
            })?;
            let has = |name: &CStr| available.iter().any(|e| e.name() == name);

            let mut extensions: Vec<&CStr> = Vec::new();
            let wanted: &[&CStr] = &[c"VK_KHR_surface", c"VK_KHR_xlib_surface"];
            for &name in wanted {
                if has(name) {
                    extensions.push(name);
                }
            }

            let validation =
                desc.validation || std::env::var_os("SHALLOT_GPU_VALIDATION").is_some();
            let mut layers: Vec<&CStr> = Vec::new();
            if validation {
                let available_layers = vk::enumerate(
                    |count, items| (entry.enumerate_instance_layer_properties)(count, items),
                    std::mem::zeroed::<vk::LayerProperties>(),
                )
                .unwrap_or_default();
                let name = c"VK_LAYER_KHRONOS_validation";
                if available_layers
                    .iter()
                    .any(|l| CStr::from_ptr(l.layer_name.as_ptr()) == name)
                {
                    layers.push(name);
                } else {
                    eprintln!("shallot-gpu: validation requested but VK_LAYER_KHRONOS_validation is not installed");
                }
            }

            let extension_ptrs: Vec<*const c_char> =
                extensions.iter().map(|e| e.as_ptr()).collect();
            let layer_ptrs: Vec<*const c_char> = layers.iter().map(|e| e.as_ptr()).collect();
            let app = vk::ApplicationInfo {
                s_type: vk::STRUCTURE_TYPE_APPLICATION_INFO,
                p_next: std::ptr::null(),
                p_application_name: c"shallot".as_ptr(),
                application_version: 0,
                p_engine_name: c"shallot".as_ptr(),
                engine_version: 0,
                api_version,
            };
            let info = vk::InstanceCreateInfo {
                s_type: vk::STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                p_application_info: &app,
                enabled_layer_count: layer_ptrs.len() as u32,
                pp_enabled_layer_names: layer_ptrs.as_ptr(),
                enabled_extension_count: extension_ptrs.len() as u32,
                pp_enabled_extension_names: extension_ptrs.as_ptr(),
            };
            let mut raw = std::ptr::null_mut();
            check(
                (entry.create_instance)(&info, std::ptr::null(), &mut raw),
                "vkCreateInstance",
            )?;
            let load = |name: &CStr| get_instance_proc_addr(raw, name.as_ptr());
            let fns = vk::InstanceFns::load(load)
                .map_err(|name| GpuError::Internal(format!("Vulkan instance is missing {name}")))?;
            let surface = if extensions.contains(&c"VK_KHR_surface") {
                vk::SurfaceFns::load(load).ok()
            } else {
                None
            };
            let xlib_surface = if extensions.contains(&c"VK_KHR_xlib_surface") {
                vk::XlibSurfaceFns::load(load).ok()
            } else {
                None
            };
            Ok(Instance {
                shared: Arc::new(InstanceShared {
                    _library: library,
                    raw,
                    fns,
                    surface,
                    xlib_surface,
                    api_version,
                }),
            })
        }
    }

    /// `navigator.gpu.requestAdapter`. Resolves to `None` when no device fits the options.
    pub fn request_adapter(
        &self,
        options: &RequestAdapterOptions<'_>,
    ) -> impl Future<Output = Option<Adapter>> {
        let adapter = self.pick_adapter(options);
        std::future::ready(adapter)
    }

    /// Every usable adapter, best first.
    pub fn enumerate_adapters(&self) -> Vec<Adapter> {
        let instance = &self.shared;
        let devices = unsafe {
            vk::enumerate(
                |count, items| {
                    (instance.fns.enumerate_physical_devices)(instance.raw, count, items)
                },
                std::ptr::null_mut(),
            )
        }
        .unwrap_or_default();
        let mut adapters: Vec<Adapter> = devices
            .into_iter()
            .filter_map(|pd| AdapterShared::probe(instance, pd))
            .collect();
        adapters.sort_by_key(|a| std::cmp::Reverse(a.shared.score(PowerPreference::None)));
        adapters
    }

    fn pick_adapter(&self, options: &RequestAdapterOptions<'_>) -> Option<Adapter> {
        let mut candidates = self.enumerate_adapters();
        candidates.retain(|a| {
            if options.force_fallback_adapter && !a.shared.info.is_fallback_adapter {
                return false;
            }
            match options.compatible_surface {
                Some(surface) => surface.supported_by(&a.shared),
                None => true,
            }
        });
        candidates
            .into_iter()
            .max_by_key(|a| a.shared.score(options.power_preference))
    }

    /// `navigator.gpu.getPreferredCanvasFormat()`.
    pub fn preferred_surface_format(&self) -> TextureFormat {
        TextureFormat::Bgra8Unorm
    }

    /// Creates a surface (the `GPUCanvasContext` analogue) for a window.
    ///
    /// # Safety
    /// The handles must stay valid until the surface is dropped.
    pub unsafe fn create_surface(&self, window: RawWindowHandle) -> Result<Surface, GpuError> {
        Surface::new(&self.shared, window)
    }
}

// ------------------------------------------------------------------------------------------
// Adapter

pub(crate) struct AdapterShared {
    pub instance: Arc<InstanceShared>,
    pub raw: vk::PhysicalDevice,
    pub properties: vk::PhysicalDeviceProperties,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub queue_family: u32,
    pub api_version: u32,
    pub info: AdapterInfo,
    pub features: Vec<Feature>,
    pub limits: Limits,
    pub vk_features: vk::PhysicalDeviceFeatures,
    pub extensions: Vec<CString>,
    pub depth24: i32,
    pub depth24_stencil8: i32,
}

unsafe impl Send for AdapterShared {}
unsafe impl Sync for AdapterShared {}

/// `GPUAdapter`.
#[derive(Clone)]
pub struct Adapter {
    pub(crate) shared: Arc<AdapterShared>,
}

// Indices into VkPhysicalDeviceFeatures.
const FEATURE_FULL_DRAW_INDEX_UINT32: usize = 1;
const FEATURE_IMAGE_CUBE_ARRAY: usize = 2;
const FEATURE_INDEPENDENT_BLEND: usize = 3;
const FEATURE_SAMPLE_RATE_SHADING: usize = 6;
const FEATURE_DRAW_INDIRECT_FIRST_INSTANCE: usize = 10;
const FEATURE_DEPTH_CLAMP: usize = 11;
const FEATURE_DEPTH_BIAS_CLAMP: usize = 12;
const FEATURE_VERTEX_PIPELINE_STORES_AND_ATOMICS: usize = 25;
const FEATURE_FRAGMENT_STORES_AND_ATOMICS: usize = 26;

impl AdapterShared {
    fn probe(instance: &Arc<InstanceShared>, raw: vk::PhysicalDevice) -> Option<Adapter> {
        unsafe {
            let fns = &instance.fns;
            let mut properties: vk::PhysicalDeviceProperties = std::mem::zeroed();
            (fns.get_physical_device_properties)(raw, &mut properties);
            let api_version = properties.api_version.min(instance.api_version);
            if api_version < vk::API_VERSION_1_1 {
                return None;
            }
            let families = vk::enumerate(
                |count, items| {
                    (fns.get_physical_device_queue_family_properties)(raw, count, items);
                    vk::SUCCESS
                },
                vk::QueueFamilyProperties::default(),
            )
            .ok()?;
            let wanted = vk::QUEUE_GRAPHICS_BIT | vk::QUEUE_COMPUTE_BIT;
            let queue_family = families
                .iter()
                .position(|f| f.queue_flags & wanted == wanted)?
                as u32;

            let extensions: Vec<CString> = vk::enumerate(
                |count, items| {
                    (fns.enumerate_device_extension_properties)(raw, std::ptr::null(), count, items)
                },
                std::mem::zeroed::<vk::ExtensionProperties>(),
            )
            .ok()?
            .iter()
            .map(|e| e.name().to_owned())
            .collect();
            let has = |name: &CStr| extensions.iter().any(|e| e.as_c_str() == name);

            // Dynamic rendering and synchronization2 are required: core in 1.3, extensions before.
            let mut sync2 = vk::PhysicalDeviceSynchronization2Features {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_SYNCHRONIZATION_2_FEATURES,
                p_next: std::ptr::null_mut(),
                synchronization2: 0,
            };
            let mut dynamic_rendering = vk::PhysicalDeviceDynamicRenderingFeatures {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_DYNAMIC_RENDERING_FEATURES,
                p_next: &mut sync2 as *mut _ as *mut c_void,
                dynamic_rendering: 0,
            };
            let mut features2 = vk::PhysicalDeviceFeatures2 {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2,
                p_next: &mut dynamic_rendering as *mut _ as *mut c_void,
                features: std::mem::zeroed(),
            };
            let core13 = api_version >= vk::API_VERSION_1_3;
            if core13 || (has(c"VK_KHR_dynamic_rendering") && has(c"VK_KHR_synchronization2")) {
                (fns.get_physical_device_features2)(raw, &mut features2);
            }
            if dynamic_rendering.dynamic_rendering == 0 || sync2.synchronization2 == 0 {
                return None;
            }

            let mut memory: vk::PhysicalDeviceMemoryProperties = std::mem::zeroed();
            (fns.get_physical_device_memory_properties)(raw, &mut memory);

            let format_features = |format: i32| {
                let mut p = vk::FormatProperties::default();
                (fns.get_physical_device_format_properties)(raw, format, &mut p);
                p.optimal_tiling_features
            };
            let depth_ok = |format: i32| {
                format_features(format) & vk::FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT != 0
            };
            let depth24 = if depth_ok(vk::FORMAT_X8_D24_UNORM_PACK32) {
                vk::FORMAT_X8_D24_UNORM_PACK32
            } else {
                vk::FORMAT_D32_SFLOAT
            };
            let depth24_stencil8 = if depth_ok(vk::FORMAT_D24_UNORM_S8_UINT) {
                vk::FORMAT_D24_UNORM_S8_UINT
            } else {
                vk::FORMAT_D32_SFLOAT_S8_UINT
            };

            let vk_features = features2.features;
            let mut features = Vec::new();
            if vk_features.flags[FEATURE_DRAW_INDIRECT_FIRST_INSTANCE] != 0 {
                features.push(Feature::IndirectFirstInstance);
            }
            if properties.limits.timestamp_compute_and_graphics != 0 {
                features.push(Feature::TimestampQuery);
            }
            if depth_ok(vk::FORMAT_D32_SFLOAT_S8_UINT) {
                features.push(Feature::Depth32FloatStencil8);
            }
            const FORMAT_FEATURE_STORAGE_IMAGE_BIT: vk::Flags = 0x2;
            if format_features(vk::FORMAT_B8G8R8A8_UNORM) & FORMAT_FEATURE_STORAGE_IMAGE_BIT != 0 {
                features.push(Feature::Bgra8UnormStorage);
            }

            let name = CStr::from_ptr(properties.device_name.as_ptr())
                .to_string_lossy()
                .into_owned();
            let info = AdapterInfo {
                vendor: vendor_name(properties.vendor_id).to_string(),
                architecture: String::new(),
                device: name.clone(),
                description: name,
                is_fallback_adapter: properties.device_type == vk::PHYSICAL_DEVICE_TYPE_CPU,
            };
            let l = &properties.limits;
            let defaults = Limits::default();
            let limits = Limits {
                max_texture_dimension_1d: l.max_image_dimension_1d,
                max_texture_dimension_2d: l.max_image_dimension_2d,
                max_texture_dimension_3d: l.max_image_dimension_3d,
                max_texture_array_layers: l.max_image_array_layers,
                max_bind_groups: l.max_bound_descriptor_sets.min(8),
                max_sampled_textures_per_shader_stage: l.max_per_stage_descriptor_sampled_images,
                max_samplers_per_shader_stage: l.max_per_stage_descriptor_samplers,
                max_storage_buffers_per_shader_stage: l.max_per_stage_descriptor_storage_buffers,
                max_storage_textures_per_shader_stage: l.max_per_stage_descriptor_storage_images,
                max_uniform_buffers_per_shader_stage: l.max_per_stage_descriptor_uniform_buffers,
                max_uniform_buffer_binding_size: l.max_uniform_buffer_range as u64,
                max_storage_buffer_binding_size: l.max_storage_buffer_range as u64,
                min_uniform_buffer_offset_alignment: l.min_uniform_buffer_offset_alignment.max(32)
                    as u32,
                min_storage_buffer_offset_alignment: l.min_storage_buffer_offset_alignment.max(32)
                    as u32,
                max_vertex_buffers: l.max_vertex_input_bindings,
                max_vertex_attributes: l.max_vertex_input_attributes,
                max_vertex_buffer_array_stride: l.max_vertex_input_binding_stride,
                max_color_attachments: l.max_color_attachments.min(8),
                max_compute_workgroup_storage_size: l.max_compute_shared_memory_size,
                max_compute_invocations_per_workgroup: l.max_compute_work_group_invocations,
                max_compute_workgroup_size_x: l.max_compute_work_group_size[0],
                max_compute_workgroup_size_y: l.max_compute_work_group_size[1],
                max_compute_workgroup_size_z: l.max_compute_work_group_size[2],
                max_compute_workgroups_per_dimension: l.max_compute_work_group_count[0]
                    .min(l.max_compute_work_group_count[1])
                    .min(l.max_compute_work_group_count[2]),
                ..defaults
            };
            Some(Adapter {
                shared: Arc::new(AdapterShared {
                    instance: instance.clone(),
                    raw,
                    properties,
                    memory,
                    queue_family,
                    api_version,
                    info,
                    features,
                    limits,
                    vk_features,
                    extensions,
                    depth24,
                    depth24_stencil8,
                }),
            })
        }
    }

    fn score(&self, preference: PowerPreference) -> i32 {
        let kind = self.properties.device_type;
        match (preference, kind) {
            (PowerPreference::LowPower, vk::PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU) => 5,
            (_, vk::PHYSICAL_DEVICE_TYPE_DISCRETE_GPU) => 4,
            (_, vk::PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU) => 3,
            (_, vk::PHYSICAL_DEVICE_TYPE_VIRTUAL_GPU) => 2,
            (_, vk::PHYSICAL_DEVICE_TYPE_CPU) => 1,
            _ => 0,
        }
    }

    pub fn has_extension(&self, name: &CStr) -> bool {
        self.extensions.iter().any(|e| e.as_c_str() == name)
    }

    pub fn vk_format(&self, format: TextureFormat) -> i32 {
        conv::format(format, self.depth24, self.depth24_stencil8)
    }
}

fn vendor_name(id: u32) -> &'static str {
    match id {
        0x1002 => "amd",
        0x10de => "nvidia",
        0x8086 => "intel",
        0x13b5 => "arm",
        0x5143 => "qualcomm",
        0x106b => "apple",
        0x1010 => "imgtec",
        0x10005 => "mesa",
        _ => "",
    }
}

impl Adapter {
    /// `GPUAdapter.info`.
    pub fn info(&self) -> AdapterInfo {
        self.shared.info.clone()
    }

    pub fn features(&self) -> Vec<Feature> {
        self.shared.features.clone()
    }

    pub fn limits(&self) -> Limits {
        self.shared.limits
    }

    /// `GPUAdapter.requestDevice`.
    pub fn request_device(
        &self,
        desc: &DeviceDescriptor<'_>,
    ) -> impl Future<Output = Result<Device, RequestDeviceError>> {
        std::future::ready(Device::new(&self.shared, desc))
    }
}

// ------------------------------------------------------------------------------------------
// Device

/// One `vkQueueSubmit` and what it keeps alive until its fence signals.
pub(crate) struct Submission {
    pub index: u64,
    pub fence: vk::Fence,
    pub command_buffer: vk::CommandBuffer,
    pub resources: Vec<Arc<dyn std::any::Any + Send + Sync>>,
}

pub(crate) struct DeviceState {
    pub command_pool: vk::CommandPool,
    pub in_flight: VecDeque<Submission>,
    /// Index the next submission gets; submission indices start at 1.
    pub next_index: u64,
    /// Every submission with an index at or below this has finished.
    pub completed: u64,
    /// `write_buffer` and `write_texture` copies, recorded at the start of the next submit.
    pub pending_writes: Vec<command::PendingWrite>,
    /// Futures waiting for a submission index to complete.
    pub wakers: Vec<(u64, std::task::Waker)>,
    pub lost: bool,
}

pub(crate) struct ErrorState {
    scopes: Vec<(ErrorFilter, Option<GpuError>)>,
    #[allow(clippy::type_complexity)]
    uncaptured: Option<Box<dyn Fn(GpuError) + Send + Sync>>,
}

pub(crate) struct DeviceShared {
    pub adapter: Arc<AdapterShared>,
    pub raw: vk::Device,
    pub fns: vk::DeviceFns,
    pub rendering: vk::RenderingFns,
    pub swapchain: Option<vk::SwapchainFns>,
    pub queue: vk::Queue,
    pub features: Vec<Feature>,
    pub limits: Limits,
    pub label: String,
    state: Mutex<DeviceState>,
    errors: Mutex<ErrorState>,
}

unsafe impl Send for DeviceShared {}
unsafe impl Sync for DeviceShared {}

/// `GPUDevice`.
#[derive(Clone)]
pub struct Device {
    pub(crate) shared: Arc<DeviceShared>,
}

/// How [`Device::poll`] waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollType {
    /// Retire finished work and return.
    Poll,
    /// Block until all submitted work has finished.
    Wait,
}

impl Device {
    fn new(
        adapter: &Arc<AdapterShared>,
        desc: &DeviceDescriptor<'_>,
    ) -> Result<Device, RequestDeviceError> {
        for feature in desc.required_features {
            if !adapter.features.contains(feature) {
                return Err(RequestDeviceError(format!(
                    "feature {feature:?} is not supported"
                )));
            }
        }
        let limits = desc.required_limits.unwrap_or_default();
        if !limits_fit(&limits, &adapter.limits) {
            return Err(RequestDeviceError(
                "required limits exceed the adapter's".into(),
            ));
        }
        unsafe {
            let instance = &adapter.instance;
            let priority = 1.0f32;
            let queue_info = vk::DeviceQueueCreateInfo {
                s_type: vk::STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                queue_family_index: adapter.queue_family,
                queue_count: 1,
                p_queue_priorities: &priority,
            };
            let core13 = adapter.api_version >= vk::API_VERSION_1_3;
            let mut extensions: Vec<&CStr> = Vec::new();
            if adapter.has_extension(c"VK_KHR_swapchain") {
                extensions.push(c"VK_KHR_swapchain");
            }
            if !core13 {
                extensions.extend([c"VK_KHR_dynamic_rendering", c"VK_KHR_synchronization2"]);
                for dependency in [
                    c"VK_KHR_depth_stencil_resolve",
                    c"VK_KHR_create_renderpass2",
                    c"VK_KHR_multiview",
                    c"VK_KHR_maintenance2",
                ] {
                    if adapter.has_extension(dependency) {
                        extensions.push(dependency);
                    }
                }
            }
            if adapter.has_extension(c"VK_KHR_portability_subset") {
                extensions.push(c"VK_KHR_portability_subset");
            }
            let extension_ptrs: Vec<*const c_char> =
                extensions.iter().map(|e| e.as_ptr()).collect();

            // Enable the core features WebGPU relies on, where the device has them.
            let mut enabled: vk::PhysicalDeviceFeatures = std::mem::zeroed();
            for index in [
                FEATURE_FULL_DRAW_INDEX_UINT32,
                FEATURE_IMAGE_CUBE_ARRAY,
                FEATURE_INDEPENDENT_BLEND,
                FEATURE_SAMPLE_RATE_SHADING,
                FEATURE_DRAW_INDIRECT_FIRST_INSTANCE,
                FEATURE_DEPTH_CLAMP,
                FEATURE_DEPTH_BIAS_CLAMP,
                FEATURE_VERTEX_PIPELINE_STORES_AND_ATOMICS,
                FEATURE_FRAGMENT_STORES_AND_ATOMICS,
            ] {
                enabled.flags[index] = adapter.vk_features.flags[index];
            }
            let mut sync2 = vk::PhysicalDeviceSynchronization2Features {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_SYNCHRONIZATION_2_FEATURES,
                p_next: std::ptr::null_mut(),
                synchronization2: vk::TRUE,
            };
            let mut dynamic_rendering = vk::PhysicalDeviceDynamicRenderingFeatures {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_DYNAMIC_RENDERING_FEATURES,
                p_next: &mut sync2 as *mut _ as *mut c_void,
                dynamic_rendering: vk::TRUE,
            };
            let features2 = vk::PhysicalDeviceFeatures2 {
                s_type: vk::STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2,
                p_next: &mut dynamic_rendering as *mut _ as *mut c_void,
                features: enabled,
            };
            let info = vk::DeviceCreateInfo {
                s_type: vk::STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                p_next: &features2 as *const _ as *const c_void,
                flags: 0,
                queue_create_info_count: 1,
                p_queue_create_infos: &queue_info,
                enabled_layer_count: 0,
                pp_enabled_layer_names: std::ptr::null(),
                enabled_extension_count: extension_ptrs.len() as u32,
                pp_enabled_extension_names: extension_ptrs.as_ptr(),
                p_enabled_features: std::ptr::null(),
            };
            let mut raw = std::ptr::null_mut();
            let r = (instance.fns.create_device)(adapter.raw, &info, std::ptr::null(), &mut raw);
            if r != vk::SUCCESS {
                return Err(RequestDeviceError(format!(
                    "vkCreateDevice failed: {}",
                    vk::result_name(r)
                )));
            }
            let get = instance.fns.get_device_proc_addr;
            let load = |name: &CStr| get(raw, name.as_ptr());
            let fns = match vk::DeviceFns::load(load) {
                Ok(fns) => fns,
                Err(name) => return Err(RequestDeviceError(format!("device is missing {name}"))),
            };
            // Promoted entry points: the core name, or the KHR name on Vulkan 1.1 and 1.2.
            let rendering = vk::RenderingFns::load(|name| {
                let p = get(raw, name.as_ptr());
                if !p.is_null() {
                    return p;
                }
                let khr = CString::new(format!("{}KHR", name.to_str().unwrap())).unwrap();
                get(raw, khr.as_ptr())
            });
            let rendering = match rendering {
                Ok(r) => r,
                Err(name) => {
                    (fns.destroy_device)(raw, std::ptr::null());
                    return Err(RequestDeviceError(format!("device is missing {name}")));
                }
            };
            let swapchain = if extensions.contains(&c"VK_KHR_swapchain") {
                vk::SwapchainFns::load(load).ok()
            } else {
                None
            };
            let mut queue = std::ptr::null_mut();
            (fns.get_device_queue)(raw, adapter.queue_family, 0, &mut queue);

            let pool_info = vk::CommandPoolCreateInfo {
                s_type: vk::STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: vk::COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
                queue_family_index: adapter.queue_family,
            };
            let mut command_pool = 0;
            let r = (fns.create_command_pool)(raw, &pool_info, std::ptr::null(), &mut command_pool);
            if r != vk::SUCCESS {
                (fns.destroy_device)(raw, std::ptr::null());
                return Err(RequestDeviceError(format!(
                    "vkCreateCommandPool failed: {}",
                    vk::result_name(r)
                )));
            }

            Ok(Device {
                shared: Arc::new(DeviceShared {
                    adapter: adapter.clone(),
                    raw,
                    fns,
                    rendering,
                    swapchain,
                    queue,
                    features: desc.required_features.to_vec(),
                    limits,
                    label: desc.label.unwrap_or("").to_string(),
                    state: Mutex::new(DeviceState {
                        command_pool,
                        in_flight: VecDeque::new(),
                        next_index: 1,
                        completed: 0,
                        pending_writes: Vec::new(),
                        wakers: Vec::new(),
                        lost: false,
                    }),
                    errors: Mutex::new(ErrorState {
                        scopes: Vec::new(),
                        uncaptured: None,
                    }),
                }),
            })
        }
    }

    pub fn features(&self) -> Vec<Feature> {
        self.shared.features.clone()
    }

    pub fn limits(&self) -> Limits {
        self.shared.limits
    }

    pub fn adapter_info(&self) -> AdapterInfo {
        self.shared.adapter.info.clone()
    }

    /// `GPUDevice.queue`.
    pub fn queue(&self) -> Queue {
        Queue {
            device: self.shared.clone(),
        }
    }

    /// `pushErrorScope`.
    pub fn push_error_scope(&self, filter: ErrorFilter) {
        self.shared
            .errors
            .lock()
            .unwrap()
            .scopes
            .push((filter, None));
    }

    /// `popErrorScope`: the first error the scope caught, if any.
    pub fn pop_error_scope(&self) -> impl Future<Output = Option<GpuError>> {
        let popped = self.shared.errors.lock().unwrap().scopes.pop();
        let result = match popped {
            Some((_, error)) => error,
            None => {
                self.shared.report(GpuError::Validation(
                    "popErrorScope with no scope pushed".into(),
                ));
                None
            }
        };
        std::future::ready(result)
    }

    /// Sets the `uncapturederror` handler. Without one, uncaptured errors are printed to stderr,
    /// as browsers print them to the console.
    pub fn on_uncaptured_error(&self, handler: impl Fn(GpuError) + Send + Sync + 'static) {
        self.shared.errors.lock().unwrap().uncaptured = Some(Box::new(handler));
    }

    /// Retires finished submissions, flushing queued writes first. With [`PollType::Wait`],
    /// blocks until everything submitted so far has finished.
    pub fn poll(&self, kind: PollType) {
        self.shared.flush_pending_writes();
        let mut state = self.shared.lock();
        let target = if kind == PollType::Wait {
            state.next_index - 1
        } else {
            0
        };
        self.shared.wait_for(&mut state, target);
        self.shared.retire(&mut state);
    }
}

fn limits_fit(required: &Limits, supported: &Limits) -> bool {
    required.max_texture_dimension_2d <= supported.max_texture_dimension_2d
        && required.max_bind_groups <= supported.max_bind_groups
        && required.max_storage_buffers_per_shader_stage
            <= supported.max_storage_buffers_per_shader_stage
        && required.max_uniform_buffer_binding_size <= supported.max_uniform_buffer_binding_size
        && required.max_storage_buffer_binding_size <= supported.max_storage_buffer_binding_size
        && required.max_color_attachments <= supported.max_color_attachments
        && required.max_vertex_buffers <= supported.max_vertex_buffers
        && required.max_compute_invocations_per_workgroup
            <= supported.max_compute_invocations_per_workgroup
        && required.min_uniform_buffer_offset_alignment
            >= supported.min_uniform_buffer_offset_alignment
        && required.min_storage_buffer_offset_alignment
            >= supported.min_storage_buffer_offset_alignment
}

impl DeviceShared {
    pub fn lock(&self) -> MutexGuard<'_, DeviceState> {
        self.state.lock().unwrap()
    }

    /// Routes an error to the innermost matching error scope, or the uncaptured handler.
    pub fn report(&self, error: GpuError) {
        let mut errors = self.errors.lock().unwrap();
        let filter = error.filter();
        if let Some(scope) = errors.scopes.iter_mut().rev().find(|(f, _)| *f == filter) {
            if scope.1.is_none() {
                scope.1 = Some(error);
            }
            return;
        }
        match &errors.uncaptured {
            Some(handler) => handler(error),
            None => {
                let label = if self.label.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", self.label)
                };
                eprintln!("shallot-gpu{label}: uncaptured {error}");
            }
        }
    }

    pub fn validation(&self, message: impl Into<String>) {
        self.report(GpuError::Validation(message.into()));
    }

    /// Picks a memory type allowed by `bits` with all of `required`, preferring `preferred`.
    pub fn memory_type(&self, bits: u32, required: vk::Flags, preferred: vk::Flags) -> Option<u32> {
        let memory = &self.adapter.memory;
        let types = &memory.memory_types[..memory.memory_type_count as usize];
        let fits = |i: usize, flags: vk::Flags| {
            bits & (1 << i) != 0 && types[i].property_flags & flags == flags
        };
        (0..types.len())
            .find(|&i| fits(i, required | preferred))
            .or_else(|| (0..types.len()).find(|&i| fits(i, required)))
            .map(|i| i as u32)
    }

    pub fn allocate(
        &self,
        requirements: vk::MemoryRequirements,
        required: vk::Flags,
        preferred: vk::Flags,
    ) -> Result<vk::DeviceMemory, GpuError> {
        let Some(memory_type) =
            self.memory_type(requirements.memory_type_bits, required, preferred)
        else {
            return Err(GpuError::OutOfMemory("no suitable memory type".into()));
        };
        // One allocation per resource for now; sub-allocation is planned (docs/porting/gpu.md).
        let info = vk::MemoryAllocateInfo {
            s_type: vk::STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            p_next: std::ptr::null(),
            allocation_size: requirements.size,
            memory_type_index: memory_type,
        };
        let mut memory = 0;
        unsafe {
            check(
                (self.fns.allocate_memory)(self.raw, &info, std::ptr::null(), &mut memory),
                "vkAllocateMemory",
            )?
        };
        Ok(memory)
    }

    /// Frees resources of submissions whose fences have signalled.
    pub fn retire(&self, state: &mut DeviceState) {
        while let Some(front) = state.in_flight.front() {
            let status = unsafe { (self.fns.get_fence_status)(self.raw, front.fence) };
            if status == vk::NOT_READY {
                break;
            }
            if status == vk::ERROR_DEVICE_LOST {
                state.lost = true;
            }
            let done = state.in_flight.pop_front().unwrap();
            state.completed = state.completed.max(done.index);
            self.free_submission(state.command_pool, done);
        }
        if state.in_flight.is_empty() {
            state.completed = state.next_index - 1;
        }
        let completed = state.completed;
        let (ready, waiting): (Vec<_>, Vec<_>) = state
            .wakers
            .drain(..)
            .partition(|(index, _)| *index <= completed);
        state.wakers = waiting;
        for (_, waker) in ready {
            waker.wake();
        }
    }

    fn free_submission(&self, pool: vk::CommandPool, submission: Submission) {
        unsafe {
            (self.fns.destroy_fence)(self.raw, submission.fence, std::ptr::null());
            (self.fns.free_command_buffers)(self.raw, pool, 1, &submission.command_buffer);
        }
        drop(submission.resources);
    }

    /// Blocks until submission `index` (and everything before it) has finished.
    pub fn wait_for(&self, state: &mut DeviceState, index: u64) {
        while state.completed < index {
            let Some(front) = state.in_flight.front() else {
                state.completed = state.next_index - 1;
                break;
            };
            let r = unsafe {
                (self.fns.wait_for_fences)(self.raw, 1, &front.fence, vk::TRUE, u64::MAX)
            };
            if r == vk::ERROR_DEVICE_LOST {
                state.lost = true;
            }
            self.retire(state);
        }
    }

    pub fn is_complete(&self, index: u64) -> bool {
        let mut state = self.lock();
        if state.completed >= index {
            return true;
        }
        self.retire(&mut state);
        state.completed >= index
    }
}

impl Drop for DeviceShared {
    fn drop(&mut self) {
        unsafe {
            (self.fns.device_wait_idle)(self.raw);
            let state = self.state.get_mut().unwrap();
            let pool = state.command_pool;
            let pending: Vec<Submission> = state.in_flight.drain(..).collect();
            let writes = std::mem::take(&mut state.pending_writes);
            drop(writes);
            for submission in pending {
                self.free_submission(pool, submission);
            }
            (self.fns.destroy_command_pool)(self.raw, pool, std::ptr::null());
            (self.fns.destroy_device)(self.raw, std::ptr::null());
        }
    }
}
