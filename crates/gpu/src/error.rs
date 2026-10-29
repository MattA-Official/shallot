//! Errors, as WebGPU reports them.

use std::fmt;

/// `GPUError`: what an error scope or the uncaptured-error handler receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuError {
    /// `GPUValidationError`: the call broke a WebGPU validation rule.
    Validation(String),
    /// `GPUOutOfMemoryError`.
    OutOfMemory(String),
    /// `GPUInternalError`: the implementation failed for a reason the caller could not prevent.
    Internal(String),
}

impl GpuError {
    pub fn message(&self) -> &str {
        match self {
            GpuError::Validation(m) | GpuError::OutOfMemory(m) | GpuError::Internal(m) => m,
        }
    }

    pub fn filter(&self) -> ErrorFilter {
        match self {
            GpuError::Validation(_) => ErrorFilter::Validation,
            GpuError::OutOfMemory(_) => ErrorFilter::OutOfMemory,
            GpuError::Internal(_) => ErrorFilter::Internal,
        }
    }
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::Validation(m) => write!(f, "validation error: {m}"),
            GpuError::OutOfMemory(m) => write!(f, "out of memory: {m}"),
            GpuError::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// `GPUErrorFilter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorFilter {
    Validation,
    OutOfMemory,
    Internal,
}

/// Why `request_device` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestDeviceError(pub String);

impl fmt::Display for RequestDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not create a device: {}", self.0)
    }
}

impl std::error::Error for RequestDeviceError {}

/// Why `map_async` was rejected (the `OperationError` and `AbortError` of WebGPU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The map request was invalid; a validation error was also generated.
    Validation(String),
    /// The buffer was unmapped or destroyed before the mapping finished.
    Aborted,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Validation(m) => write!(f, "map failed: {m}"),
            MapError::Aborted => f.write_str("map aborted"),
        }
    }
}

impl std::error::Error for MapError {}

/// Why `Surface::get_current_texture` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceError {
    /// The surface has not been configured.
    NotConfigured,
    /// The window changed in a way that needs `configure` again, for example a resize.
    Outdated,
    /// The window is gone.
    Lost,
    /// Timed out waiting for the presentation engine.
    Timeout,
    Other(String),
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SurfaceError::NotConfigured => f.write_str("the surface is not configured"),
            SurfaceError::Outdated => f.write_str("the surface must be reconfigured"),
            SurfaceError::Lost => f.write_str("the surface was lost"),
            SurfaceError::Timeout => f.write_str("timed out acquiring a surface texture"),
            SurfaceError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SurfaceError {}
