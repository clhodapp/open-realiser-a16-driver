// SPDX-License-Identifier: MIT
//! The part of libsamplerate the bridge uses: a sinc converter with a ratio set per call.

use std::ffi::{c_double, c_float, c_int, c_long, c_void};
use std::ptr::NonNull;

/// `SRC_SINC_FASTEST`.
const SINC_FASTEST: c_int = 2;

#[repr(C)]
struct SrcData {
    data_in: *const c_float,
    data_out: *mut c_float,
    input_frames: c_long,
    output_frames: c_long,
    input_frames_used: c_long,
    output_frames_gen: c_long,
    end_of_input: c_int,
    src_ratio: c_double,
}

unsafe extern "C" {
    fn src_new(converter_type: c_int, channels: c_int, error: *mut c_int) -> *mut c_void;
    fn src_delete(state: *mut c_void) -> *mut c_void;
    fn src_process(state: *mut c_void, data: *mut SrcData) -> c_int;
    fn src_reset(state: *mut c_void) -> c_int;
}

/// What one [`Resampler::process`] call used and made, in frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processed {
    pub input_used: usize,
    pub output_generated: usize,
}

/// A libsamplerate converter, `SRC_SINC_FASTEST`, for interleaved float frames.
pub struct Resampler {
    state: NonNull<c_void>,
    channels: usize,
}

// The converter state is plain memory that only its owner touches.
unsafe impl Send for Resampler {}

impl Resampler {
    /// A converter of `channels`-channel frames; None when libsamplerate refuses one.
    pub fn new(channels: usize) -> Option<Resampler> {
        let mut error = 0;
        // SAFETY: src_new only writes the error code
        let state = unsafe { src_new(SINC_FASTEST, channels as c_int, &mut error) };
        NonNull::new(state).map(|state| Resampler { state, channels })
    }

    /// Resample the frames of `input` into `output` at `ratio` (output rate over input rate).
    /// Err carries libsamplerate's error code.
    pub fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ratio: f64,
    ) -> Result<Processed, c_int> {
        let mut data = SrcData {
            data_in: input.as_ptr(),
            data_out: output.as_mut_ptr(),
            input_frames: (input.len() / self.channels) as c_long,
            output_frames: (output.len() / self.channels) as c_long,
            input_frames_used: 0,
            output_frames_gen: 0,
            end_of_input: 0,
            src_ratio: ratio,
        };
        // SAFETY: the buffers hold the frames `data` gives, and outlive the call
        let error = unsafe { src_process(self.state.as_ptr(), &mut data) };
        if error != 0 {
            return Err(error);
        }
        Ok(Processed {
            input_used: data.input_frames_used as usize,
            output_generated: data.output_frames_gen as usize,
        })
    }

    pub fn reset(&mut self) {
        // SAFETY: the state is live
        unsafe {
            src_reset(self.state.as_ptr());
        }
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: the state is live, and dropped once
        unsafe {
            src_delete(self.state.as_ptr());
        }
    }
}
