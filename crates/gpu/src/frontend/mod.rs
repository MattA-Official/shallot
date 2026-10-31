//! The WebGPU front end: instance, adapter and device, with error scopes and submission
//! tracking. Validation lives here, shared by every backend in `crate::hal`.

mod binding;
mod command;
mod resource;
mod surface;

pub use binding::{BindGroup, BindGroupLayout, PipelineLayout, Sampler};
pub(crate) use binding::{BindGroupShared, Bound, PipelineLayoutShared};
pub(crate) use command::{copy_block_size, Command, PassRecord, PendingWrite};
pub use command::{CommandBuffer, CommandEncoder, ComputePassEncoder, Queue, RenderPassEncoder};
pub use resource::{
    Buffer, BufferView, BufferViewMut, ComputePipeline, MapFuture, RenderPipeline, ShaderModule,
    Texture, TextureView,
};
pub(crate) use resource::{TextureShared, TextureViewShared};
pub use surface::{Surface, SurfaceCapabilities, SurfaceTexture};

use crate::hal;
use crate::*;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

// ------------------------------------------------------------------------------------------
// Instance

pub(crate) struct InstanceShared {
    pub raw: hal::Instance,
}

/// `navigator.gpu`: the entry point that finds adapters and creates surfaces.
#[derive(Clone)]
pub struct Instance {
    pub(crate) shared: Arc<InstanceShared>,
}

impl Instance {
    /// Opens the platform's GPU API. Validation (`desc.validation`, or the
    /// `SHALLOT_GPU_VALIDATION` environment variable) turns on the backend's own checks where
    /// it has them, such as Vulkan's validation layers.
    pub fn new(desc: &InstanceDescriptor) -> Result<Instance, GpuError> {
        let validation = desc.validation || std::env::var_os("SHALLOT_GPU_VALIDATION").is_some();
        Ok(Instance {
            shared: Arc::new(InstanceShared {
                raw: hal::Instance::new(validation)?,
            }),
        })
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
        let mut adapters: Vec<Adapter> = self
            .shared
            .raw
            .adapters()
            .into_iter()
            .map(|raw| Adapter {
                shared: Arc::new(AdapterShared {
                    _instance: self.shared.clone(),
                    info: raw.info.clone(),
                    features: raw.features.clone(),
                    limits: raw.limits,
                    raw,
                }),
            })
            .collect();
        adapters.sort_by_key(|a| std::cmp::Reverse(a.shared.raw.score(PowerPreference::None)));
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
            .max_by_key(|a| a.shared.raw.score(options.power_preference))
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
        Ok(Surface::new(self.shared.raw.create_surface(window)?))
    }
}

// ------------------------------------------------------------------------------------------
// Adapter

pub(crate) struct AdapterShared {
    _instance: Arc<InstanceShared>,
    pub raw: hal::Adapter,
    pub info: AdapterInfo,
    pub features: Vec<Feature>,
    pub limits: Limits,
}

/// `GPUAdapter`.
#[derive(Clone)]
pub struct Adapter {
    pub(crate) shared: Arc<AdapterShared>,
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

/// One submission and what it keeps alive until it finishes.
pub(crate) struct Submission {
    pub index: u64,
    pub raw: hal::Submission,
    /// Only held, so that nothing the GPU may still read is freed.
    #[allow(dead_code)]
    pub resources: Vec<Arc<dyn std::any::Any + Send + Sync>>,
}

pub(crate) struct DeviceState {
    pub in_flight: VecDeque<Submission>,
    /// Index the next submission gets; submission indices start at 1.
    pub next_index: u64,
    /// Every submission with an index at or below this has finished.
    pub completed: u64,
    /// `write_buffer` and `write_texture` copies, recorded at the start of the next submit.
    pub pending_writes: Vec<PendingWrite>,
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
    pub raw: hal::Device,
    pub features: Vec<Feature>,
    pub limits: Limits,
    pub label: String,
    state: Mutex<DeviceState>,
    errors: Mutex<ErrorState>,
}

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
        let raw = adapter.raw.open().map_err(RequestDeviceError)?;
        Ok(Device {
            shared: Arc::new(DeviceShared {
                adapter: adapter.clone(),
                raw,
                features: desc.required_features.to_vec(),
                limits,
                label: desc.label.unwrap_or("").to_string(),
                state: Mutex::new(DeviceState {
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

    /// Frees resources of submissions that have finished.
    pub fn retire(&self, state: &mut DeviceState) {
        while let Some(front) = state.in_flight.front() {
            match front.raw.status() {
                hal::Status::Pending => break,
                hal::Status::Lost => state.lost = true,
                hal::Status::Done => {}
            }
            let done = state.in_flight.pop_front().unwrap();
            state.completed = state.completed.max(done.index);
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

    /// Blocks until submission `index` (and everything before it) has finished.
    pub fn wait_for(&self, state: &mut DeviceState, index: u64) {
        while state.completed < index {
            let Some(front) = state.in_flight.front() else {
                state.completed = state.next_index - 1;
                break;
            };
            if !front.raw.wait() {
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
        self.raw.wait_idle();
        let state = self.state.get_mut().unwrap();
        state.in_flight.clear();
        state.pending_writes.clear();
    }
}
