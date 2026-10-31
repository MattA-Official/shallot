//! The Vulkan backend: Vulkan 1.1 with dynamic rendering and synchronization2, loaded at run
//! time.

mod command;
mod conv;
mod resource;
mod surface;
pub mod vk;

pub(crate) use resource::{
    BindGroup, BindGroupLayout, Buffer, ComputePipeline, PipelineLayout, RenderPipeline, Sampler,
    ShaderModule, Texture, TextureView,
};
pub(crate) use surface::Surface;

use crate::*;
use shallot_platform::Library;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

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

pub(crate) struct InstanceInner {
    _library: Library,
    pub raw: vk::Instance,
    pub fns: vk::InstanceFns,
    pub surface: Option<vk::SurfaceFns>,
    pub xlib_surface: Option<vk::XlibSurfaceFns>,
    pub win32_surface: Option<vk::Win32SurfaceFns>,
    pub metal_surface: Option<vk::MetalSurfaceFns>,
    pub api_version: u32,
}

unsafe impl Send for InstanceInner {}
unsafe impl Sync for InstanceInner {}

impl Drop for InstanceInner {
    fn drop(&mut self) {
        unsafe { (self.fns.destroy_instance)(self.raw, std::ptr::null()) };
    }
}

pub(crate) struct Instance {
    pub(crate) inner: Arc<InstanceInner>,
}

const LOADER_NAMES: &[&str] = if cfg!(windows) {
    &["vulkan-1.dll"]
} else if cfg!(target_os = "macos") {
    &[
        "libvulkan.1.dylib",
        "/opt/homebrew/lib/libvulkan.1.dylib",
        "/usr/local/lib/libvulkan.1.dylib",
        "libMoltenVK.dylib",
        "/opt/homebrew/lib/libMoltenVK.dylib",
        "/usr/local/lib/libMoltenVK.dylib",
    ]
} else {
    &["libvulkan.so.1", "libvulkan.so"]
};

impl Instance {
    /// Opens the Vulkan loader and creates an instance with every surface extension available.
    pub fn new(validation: bool) -> Result<Instance, GpuError> {
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
            let wanted: &[&CStr] = &[
                c"VK_KHR_surface",
                c"VK_KHR_xlib_surface",
                c"VK_KHR_win32_surface",
                c"VK_EXT_metal_surface",
                // MoltenVK is a portability implementation, which the loader lists only when
                // asked to.
                c"VK_KHR_portability_enumeration",
            ];
            for &name in wanted {
                if has(name) {
                    extensions.push(name);
                }
            }

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
            let flags = if extensions.contains(&c"VK_KHR_portability_enumeration") {
                vk::INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR
            } else {
                0
            };
            let info = vk::InstanceCreateInfo {
                s_type: vk::STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags,
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
            let win32_surface = if extensions.contains(&c"VK_KHR_win32_surface") {
                vk::Win32SurfaceFns::load(load).ok()
            } else {
                None
            };
            let metal_surface = if extensions.contains(&c"VK_EXT_metal_surface") {
                vk::MetalSurfaceFns::load(load).ok()
            } else {
                None
            };
            Ok(Instance {
                inner: Arc::new(InstanceInner {
                    _library: library,
                    raw,
                    fns,
                    surface,
                    xlib_surface,
                    win32_surface,
                    metal_surface,
                    api_version,
                }),
            })
        }
    }

    /// Every usable adapter, in no particular order.
    pub fn adapters(&self) -> Vec<Adapter> {
        let instance = &self.inner;
        let devices = unsafe {
            vk::enumerate(
                |count, items| {
                    (instance.fns.enumerate_physical_devices)(instance.raw, count, items)
                },
                std::ptr::null_mut(),
            )
        }
        .unwrap_or_default();
        devices
            .into_iter()
            .filter_map(|pd| Adapter::probe(instance, pd))
            .collect()
    }

    /// # Safety
    /// The handles must stay valid until the surface is dropped.
    pub unsafe fn create_surface(&self, window: RawWindowHandle) -> Result<Surface, GpuError> {
        Surface::new(&self.inner, window)
    }
}

// ------------------------------------------------------------------------------------------
// Adapter

pub(crate) struct AdapterInner {
    pub instance: Arc<InstanceInner>,
    pub raw: vk::PhysicalDevice,
    pub properties: vk::PhysicalDeviceProperties,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub queue_family: u32,
    pub api_version: u32,
    pub vk_features: vk::PhysicalDeviceFeatures,
    pub extensions: Vec<CString>,
    pub depth24: i32,
    pub depth24_stencil8: i32,
}

unsafe impl Send for AdapterInner {}
unsafe impl Sync for AdapterInner {}

pub(crate) struct Adapter {
    pub(crate) inner: Arc<AdapterInner>,
    pub info: AdapterInfo,
    pub features: Vec<Feature>,
    pub limits: Limits,
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
const FEATURE_ROBUST_BUFFER_ACCESS: usize = 0;
pub(crate) const FEATURE_SAMPLER_ANISOTROPY: usize = 19;
const FEATURE_SHADER_IMAGE_GATHER_EXTENDED: usize = 28;
const FEATURE_SHADER_STORAGE_IMAGE_EXTENDED_FORMATS: usize = 29;
const FEATURE_SHADER_STORAGE_IMAGE_READ_WITHOUT_FORMAT: usize = 31;
const FEATURE_SHADER_STORAGE_IMAGE_WRITE_WITHOUT_FORMAT: usize = 32;

impl Adapter {
    fn probe(instance: &Arc<InstanceInner>, raw: vk::PhysicalDevice) -> Option<Adapter> {
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
                max_dynamic_uniform_buffers_per_pipeline_layout: l
                    .max_descriptor_set_uniform_buffers_dynamic,
                max_dynamic_storage_buffers_per_pipeline_layout: l
                    .max_descriptor_set_storage_buffers_dynamic,
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
                inner: Arc::new(AdapterInner {
                    instance: instance.clone(),
                    raw,
                    properties,
                    memory,
                    queue_family,
                    api_version,
                    vk_features,
                    extensions,
                    depth24,
                    depth24_stencil8,
                }),
                info,
                features,
                limits,
            })
        }
    }

    /// How well the adapter suits `preference`; higher is better.
    pub fn score(&self, preference: PowerPreference) -> i32 {
        let kind = self.inner.properties.device_type;
        match (preference, kind) {
            (PowerPreference::LowPower, vk::PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU) => 5,
            (_, vk::PHYSICAL_DEVICE_TYPE_DISCRETE_GPU) => 4,
            (_, vk::PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU) => 3,
            (_, vk::PHYSICAL_DEVICE_TYPE_VIRTUAL_GPU) => 2,
            (_, vk::PHYSICAL_DEVICE_TYPE_CPU) => 1,
            _ => 0,
        }
    }

    /// Creates the device with one graphics and compute queue.
    pub fn open(&self) -> Result<Device, String> {
        let adapter = &self.inner;
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
                FEATURE_ROBUST_BUFFER_ACCESS,
                FEATURE_SAMPLER_ANISOTROPY,
                FEATURE_SHADER_IMAGE_GATHER_EXTENDED,
                FEATURE_SHADER_STORAGE_IMAGE_EXTENDED_FORMATS,
                FEATURE_SHADER_STORAGE_IMAGE_READ_WITHOUT_FORMAT,
                FEATURE_SHADER_STORAGE_IMAGE_WRITE_WITHOUT_FORMAT,
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
                return Err(format!("vkCreateDevice failed: {}", vk::result_name(r)));
            }
            let get = instance.fns.get_device_proc_addr;
            let load = |name: &CStr| get(raw, name.as_ptr());
            let fns = match vk::DeviceFns::load(load) {
                Ok(fns) => fns,
                Err(name) => return Err(format!("device is missing {name}")),
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
                    return Err(format!("device is missing {name}"));
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
                return Err(format!(
                    "vkCreateCommandPool failed: {}",
                    vk::result_name(r)
                ));
            }
            Ok(Device {
                inner: Arc::new(DeviceInner {
                    adapter: adapter.clone(),
                    raw,
                    fns,
                    rendering,
                    swapchain,
                    queue,
                    command_pool: Mutex::new(command_pool),
                }),
            })
        }
    }
}

impl AdapterInner {
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

// ------------------------------------------------------------------------------------------
// Device

pub(crate) struct DeviceInner {
    pub adapter: Arc<AdapterInner>,
    pub raw: vk::Device,
    pub fns: vk::DeviceFns,
    pub rendering: vk::RenderingFns,
    pub swapchain: Option<vk::SwapchainFns>,
    pub queue: vk::Queue,
    /// Command buffers are allocated and freed under this lock, which also serialises queue
    /// submission.
    pub command_pool: Mutex<vk::CommandPool>,
}

unsafe impl Send for DeviceInner {}
unsafe impl Sync for DeviceInner {}

impl Drop for DeviceInner {
    fn drop(&mut self) {
        unsafe {
            (self.fns.device_wait_idle)(self.raw);
            let pool = *self.command_pool.get_mut().unwrap();
            (self.fns.destroy_command_pool)(self.raw, pool, std::ptr::null());
            (self.fns.destroy_device)(self.raw, std::ptr::null());
        }
    }
}

pub(crate) struct Device {
    pub(crate) inner: Arc<DeviceInner>,
}

impl Device {
    /// Blocks until the device has finished all submitted work.
    pub fn wait_idle(&self) {
        unsafe { (self.inner.fns.device_wait_idle)(self.inner.raw) };
    }
}

impl DeviceInner {
    /// Whether a `VkPhysicalDeviceFeatures` member, by index, is enabled. Every one the
    /// device creation list names is enabled when the adapter has it.
    pub fn has_feature_flag(&self, index: usize) -> bool {
        self.adapter.vk_features.flags[index] != 0
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
}

/// One `vkQueueSubmit`: its command buffer and the fence it signals.
pub(crate) struct Submission {
    device: Arc<DeviceInner>,
    fence: vk::Fence,
    command_buffer: vk::CommandBuffer,
}

unsafe impl Send for Submission {}
unsafe impl Sync for Submission {}

impl Submission {
    pub fn status(&self) -> hal::Status {
        match unsafe { (self.device.fns.get_fence_status)(self.device.raw, self.fence) } {
            vk::NOT_READY => hal::Status::Pending,
            vk::ERROR_DEVICE_LOST => hal::Status::Lost,
            _ => hal::Status::Done,
        }
    }

    /// Blocks until the work finishes. False if the device was lost.
    pub fn wait(&self) -> bool {
        let r = unsafe {
            (self.device.fns.wait_for_fences)(self.device.raw, 1, &self.fence, vk::TRUE, u64::MAX)
        };
        r != vk::ERROR_DEVICE_LOST
    }
}

impl Drop for Submission {
    fn drop(&mut self) {
        let device = &self.device;
        let pool = device.command_pool.lock().unwrap();
        unsafe {
            (device.fns.destroy_fence)(device.raw, self.fence, std::ptr::null());
            (device.fns.free_command_buffers)(device.raw, *pool, 1, &self.command_buffer);
        }
    }
}
