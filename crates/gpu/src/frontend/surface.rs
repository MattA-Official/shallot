//! Surfaces: the `GPUCanvasContext` analogue. The backend owns the swapchain; this layer
//! paces frames and ties presentation into queue submission.

use super::resource::TextureShared;
use super::{Adapter, AdapterShared, Device, DeviceShared};
use crate::hal;
use crate::*;
use std::sync::{Arc, Mutex};

/// Frames that may be queued for presentation at once (the browser's frame-latency limit).
const FRAMES_IN_FLIGHT: usize = 2;

struct State {
    /// The device and configuration of the last successful `configure`.
    configured: Option<(Arc<DeviceShared>, SurfaceConfiguration)>,
    /// The present submission that last used each frame slot.
    frames: [u64; FRAMES_IN_FLIGHT],
    next_frame: usize,
}

pub(crate) struct SurfaceShared {
    raw: hal::Surface,
    state: Mutex<State>,
}

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
    pub(crate) fn new(raw: hal::Surface) -> Surface {
        Surface {
            shared: Arc::new(SurfaceShared {
                raw,
                state: Mutex::new(State {
                    configured: None,
                    frames: [0; FRAMES_IN_FLIGHT],
                    next_frame: 0,
                }),
            }),
        }
    }

    pub(crate) fn supported_by(&self, adapter: &AdapterShared) -> bool {
        self.shared.raw.supported(&adapter.raw)
    }

    /// The formats, present modes and alpha modes this surface supports with `adapter`.
    pub fn capabilities(&self, adapter: &Adapter) -> SurfaceCapabilities {
        self.shared.raw.capabilities(&adapter.shared.raw)
    }

    /// `configure`. Call again after the window resizes.
    pub fn configure(&self, device: &Device, config: &SurfaceConfiguration) {
        let device = &device.shared;
        let mut state = self.shared.state.lock().unwrap();
        let previous = state.configured.take();
        let result = self.shared.raw.configure(&device.raw, config);
        // Reconfiguring waits for the GPU, so earlier submissions can be retired now.
        if let Some((old, _)) = &previous {
            let mut old_state = old.lock();
            old.retire(&mut old_state);
        }
        match result {
            Ok(()) => state.configured = Some((device.clone(), config.clone())),
            Err(e) => device.report(e),
        }
    }

    /// `unconfigure`.
    pub fn unconfigure(&self) {
        let mut state = self.shared.state.lock().unwrap();
        self.shared.raw.unconfigure();
        if let Some((device, _)) = state.configured.take() {
            let mut device_state = device.lock();
            device.retire(&mut device_state);
        }
    }

    /// `getCurrentTexture`. Present the result with [`SurfaceTexture::present`] (dropping it
    /// presents too, as a canvas does at the end of the frame).
    pub fn get_current_texture(&self) -> Result<SurfaceTexture, SurfaceError> {
        let mut state = self.shared.state.lock().unwrap();
        let Some((device, config)) = state.configured.clone() else {
            return Err(SurfaceError::NotConfigured);
        };
        let frame = state.next_frame % FRAMES_IN_FLIGHT;
        {
            let mut device_state = device.lock();
            device.wait_for(&mut device_state, state.frames[frame]);
        }
        let acquired = self.shared.raw.acquire(frame).map_err(|e| match e {
            hal::AcquireError::Outdated => SurfaceError::Outdated,
            hal::AcquireError::Timeout => SurfaceError::Timeout,
            hal::AcquireError::Lost => SurfaceError::Lost,
            hal::AcquireError::Other(m) => SurfaceError::Other(m),
        })?;
        state.next_frame += 1;
        let (width, height) = self.shared.raw.extent();
        let texture =
            TextureShared::swapchain_image(&device, acquired.texture, config.usage, width, height);
        Ok(SurfaceTexture {
            texture: Texture {
                shared: Arc::new(texture),
            },
            suboptimal: acquired.suboptimal,
            surface: self.shared.clone(),
            image: acquired.image,
            frame,
            generation: acquired.generation,
            presented: false,
        })
    }
}

/// A texture to draw into this frame, from [`Surface::get_current_texture`].
pub struct SurfaceTexture {
    pub texture: Texture,
    /// The swapchain no longer matches the window exactly; reconfigure when convenient.
    pub suboptimal: bool,
    surface: Arc<SurfaceShared>,
    image: u32,
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
        let surface = &self.surface;
        let mut state = surface.state.lock().unwrap();
        let Some((device, _)) = state.configured.clone() else {
            return;
        };
        // A texture from a swapchain that has since been replaced is not presented.
        if surface.raw.generation() != self.generation {
            return;
        }
        let submitted = {
            let mut device_state = device.lock();
            device.submit_locked(
                &mut device_state,
                Vec::new(),
                Some(hal::Present {
                    texture: &self.texture.shared,
                    surface: &surface.raw,
                    image: self.image,
                }),
            )
        };
        match submitted {
            Ok(index) => {
                state.frames[self.frame] = index;
                surface.raw.present(self.image);
            }
            Err(e) => {
                drop(state);
                device.report(e);
            }
        }
    }
}

impl Drop for SurfaceTexture {
    fn drop(&mut self) {
        self.present_now();
    }
}
