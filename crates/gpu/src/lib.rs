//! A WebGPU implementation in Rust.
//!
//! The types mirror the WebGPU IDL one for one with the `GPU` prefixes dropped and methods in
//! snake case: [`Instance`] is `navigator.gpu`, [`Surface`] is `GPUCanvasContext`, and so on.
//! WGSL shaders are compiled with `shallot-wgsl`. Nothing beyond `std` is used: the Vulkan
//! loader is opened at run time.
//!
//! The crate is split in two. `frontend` implements the WebGPU API itself: validation, error
//! scopes, mapping, submission tracking and frame pacing. `hal` is the backend underneath it,
//! which only creates native objects from validated descriptions and records validated
//! commands. Vulkan is the only backend for now.
//!
//! Implemented so far (see `docs/porting/gpu.md`): instance, adapter, device, queue, buffers,
//! textures and views, samplers, bind groups and their layouts (explicit and `"auto"`), shader
//! modules, render and compute pipelines, render and compute passes with indirect draws and
//! dispatches, copies, mapping, error scopes and presentation.
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
mod frontend;
mod hal;
mod types;

pub use error::{ErrorFilter, GpuError, MapError, RequestDeviceError, SurfaceError};
pub use executor::block_on;
pub use frontend::{
    Adapter, BindGroup, BindGroupLayout, Buffer, BufferView, BufferViewMut, CommandBuffer,
    CommandEncoder, ComputePassEncoder, ComputePipeline, Device, Instance, MapFuture,
    PipelineLayout, PollType, Queue, RenderPassEncoder, RenderPipeline, Sampler, ShaderModule,
    Surface, SurfaceCapabilities, SurfaceTexture, Texture, TextureView,
};
pub use shallot_platform::RawWindowHandle;
pub use types::*;
