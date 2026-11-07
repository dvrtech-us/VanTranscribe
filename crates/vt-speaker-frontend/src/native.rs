//! The only unsafe module. Rust owns our bridge handles, not sherpa C-API
//! representations. All C++ native class allocation/destruction stays in the
//! same bridge compiled from complete pinned upstream declarations.
use crate::{Error, Result};
use std::{
    ffi::{CStr, c_char},
    ptr::NonNull,
};
#[repr(C)]
struct BridgeHandle {
    _opaque: [u8; 0],
}
unsafe extern "C" {
    fn vt_reference_create(model: *const c_char, output: *mut *mut BridgeHandle) -> i32;
    fn vt_reference_destroy(handle: *mut BridgeHandle);
    fn vt_reference_extract(
        handle: *mut BridgeHandle,
        samples: *const f32,
        count: i32,
        output: *mut f32,
        capacity: i32,
    ) -> i32;
    #[cfg(any(test, feature = "feature-probe"))]
    fn vt_reference_features(
        samples: *const f32,
        count: i32,
        output: *mut f32,
        capacity: i32,
        frames: *mut i32,
        bins: *mut i32,
    ) -> i32;
}
pub(super) fn ensure_abi() -> Result<()> {
    // These official safe wrappers read native C version strings only. No C++
    // class with SDK-dependent layout is constructed until every value agrees.
    if sherpa_onnx::version() != "2.0.1"
        || sherpa_onnx::git_sha1() != "af2d8d81"
        || sherpa_onnx::onnxruntime_version() != "1.28.2"
    {
        return Err(Error::AbiMismatch);
    }
    Ok(())
}
fn status(code: i32) -> Result<()> {
    match code {
        0 => Ok(()),
        1 => Err(Error::InvalidAudio),
        3 => Err(Error::NotReady),
        _ => Err(Error::Native),
    }
}
pub(super) struct Handle {
    pointer: NonNull<BridgeHandle>,
}
impl Handle {
    pub(super) fn create(model: &CStr) -> Result<Self> {
        let mut output = std::ptr::null_mut();
        // SAFETY: model is a live NUL-terminated CStr. `output` is an initialized
        // writable pointer slot. The bridge copies the string and transfers one
        // custom heap handle only on success; it retains no Rust borrow.
        let result = unsafe { vt_reference_create(model.as_ptr(), &mut output) };
        status(result).map_err(|_| Error::Native)?;
        Ok(Self {
            pointer: NonNull::new(output).ok_or(Error::Native)?,
        })
    }
    pub(super) fn extract(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let count = i32::try_from(samples.len()).map_err(|_| Error::InvalidAudio)?;
        let mut output = vec![0.; 256];
        // SAFETY: self owns the live bridge handle until Drop. Public input
        // validation bounds the immutable samples; the initialized 256-element
        // output is exclusively borrowed for this synchronous call. C++ retains
        // neither buffer and catches every exception before returning a status.
        let result = unsafe {
            vt_reference_extract(
                self.pointer.as_ptr(),
                samples.as_ptr(),
                count,
                output.as_mut_ptr(),
                256,
            )
        };
        status(result)?;
        if output.iter().any(|v| !v.is_finite()) {
            return Err(Error::Native);
        }
        Ok(output)
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the pointer was allocated by the matching bridge constructor,
        // is owned exactly once, and is never exposed or cloned. Its C++ delete
        // uses the same complete types/runtime and has a noexcept C boundary.
        unsafe {
            vt_reference_destroy(self.pointer.as_ptr());
        }
    }
}
#[cfg(any(test, feature = "feature-probe"))]
pub(super) fn features(samples: &[f32]) -> Result<crate::FeatureMatrix> {
    let mut values = vec![0.; crate::MAX_FRAMES * crate::FEATURE_BINS];
    let mut frames = 0;
    let mut bins = 0;
    // SAFETY: samples are finite normalized, bounded and live for this call.
    // The output vector has the declared writable capacity; the two initialized
    // integer slots are exclusive. The bridge bounds/checks its returned matrix
    // and copies values without retaining any borrowed pointer.
    let code = unsafe {
        vt_reference_features(
            samples.as_ptr(),
            samples.len() as i32,
            values.as_mut_ptr(),
            values.len() as i32,
            &mut frames,
            &mut bins,
        )
    };
    status(code)?;
    let frames = usize::try_from(frames).map_err(|_| Error::Native)?;
    let bins = usize::try_from(bins).map_err(|_| Error::Native)?;
    if frames != 1 + (samples.len() - 400) / 160
        || frames > crate::MAX_FRAMES
        || bins != crate::FEATURE_BINS
    {
        return Err(Error::Native);
    }
    let total = frames.checked_mul(bins).ok_or(Error::Native)?;
    if total > values.len() || values[..total].iter().any(|v| !v.is_finite()) {
        return Err(Error::Native);
    }
    values.truncate(total);
    Ok(crate::FeatureMatrix {
        frames,
        bins,
        values,
    })
}
