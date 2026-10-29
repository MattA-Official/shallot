//! A WebGPU implementation in Rust, over Vulkan.
//!
//! The types mirror the WebGPU IDL one for one with the `GPU` prefixes dropped and methods in
//! snake case: [`Instance`] is `navigator.gpu`, [`Surface`] is `GPUCanvasContext`, and so on.
//! WGSL shaders are compiled with `shallot-wgsl`. Nothing beyond `std` is used: the Vulkan
//! loader is opened at run time.
//!
//! What is implemented so far is the path a triangle needs (see `docs/porting/gpu.md`,
//! milestone 1): instance, adapter, device, queue, buffers, textures and views, shader modules,
//! render pipelines, render passes, copies, mapping, error scopes and presentation.
//!
//! ```no_run
//! use shallot_gpu::*;
//!
//! let instance = Instance::new(&InstanceDescriptor::default()).unwrap();
//! let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default())).unwrap();
//! let device = block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
//! let buffer = device.create_buffer(&BufferDescriptor {
//!     label: Some("data"),
//!     size: 256,
//!     usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
//!     mapped_at_creation: false,
//! });
//! device.queue().write_buffer(&buffer, 0, &[1, 2, 3, 4]);
//! block_on(buffer.map_async(MapMode::READ, 0, None)).unwrap();
//! assert_eq!(&buffer.get_mapped_range(0, Some(4))[..], &[1, 2, 3, 4]);
//! ```

mod error;
mod executor;
mod types;
mod vulkan;

pub use error::{ErrorFilter, GpuError, MapError, RequestDeviceError, SurfaceError};
pub use executor::block_on;
pub use shallot_platform::RawWindowHandle;
pub use types::*;
pub use vulkan::{
    Adapter, Buffer, BufferView, BufferViewMut, CommandBuffer, CommandEncoder, Device, Instance,
    MapFuture, PipelineLayout, PipelineLayoutDescriptor, PollType, Queue, RenderPassEncoder,
    RenderPipeline, ShaderModule, Surface, SurfaceCapabilities, SurfaceTexture, Texture,
    TextureView,
};
