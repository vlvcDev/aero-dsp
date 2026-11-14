use std::cell::Cell;
use std::error::Error;
use std::ffi::{CStr, CString, c_char, c_double, c_float, c_int};
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;

pub mod alignment;

const STATUS_OK: c_int = 0;
const ERROR_CAPACITY: usize = 512;

#[repr(C)]
struct PedalNamModel {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pedal_nam_load(
        path: *const c_char,
        sample_rate: c_double,
        max_buffer_frames: c_int,
        full_size: c_int,
        output: *mut *mut PedalNamModel,
        error_message: *mut c_char,
        error_message_capacity: usize,
    ) -> c_int;
    fn pedal_nam_free(model: *mut PedalNamModel);
    fn pedal_nam_process(
        model: *mut PedalNamModel,
        input: *const c_float,
        output: *mut c_float,
        frames: c_int,
    ) -> c_int;
    fn pedal_nam_expected_sample_rate(model: *const PedalNamModel) -> c_double;
    fn pedal_nam_input_level(model: *const PedalNamModel) -> c_double;
    fn pedal_nam_output_level(model: *const PedalNamModel) -> c_double;
    fn pedal_nam_loudness(model: *const PedalNamModel) -> c_double;
    fn pedal_nam_supports_sizes(model: *const PedalNamModel) -> c_int;
}

/// Endpoint selections for compatible slimmable NAMs. Never changes sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NamQuality {
    Lite,
    #[default]
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelMetadata {
    pub input_level_dbu: Option<f64>,
    pub output_level_dbu: Option<f64>,
    pub loudness_db: Option<f64>,
    pub expected_sample_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamError {
    pub status: i32,
    pub message: String,
}

impl fmt::Display for NamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (NAM status {})", self.message, self.status)
    }
}

impl Error for NamError {}

pub struct NamModel {
    alignment: Option<alignment::AlignmentEstimate>,
    raw: NonNull<PedalNamModel>,
    max_buffer_frames: usize,
    input_workspace: Vec<f32>,
    faulted: bool,
    _not_sync: PhantomData<Cell<()>>,
}

// The model has exclusive ownership and is safe to move onto the audio thread.
unsafe impl Send for NamModel {}

impl NamModel {
    /// Prepare an analysis instance and discard it, then load a pristine model.
    /// Extra load/probe work is never performed on the audio callback.
    pub fn load_with_alignment(
        path: impl AsRef<Path>,
        sample_rate: f64,
        max_buffer_frames: usize,
        quality: NamQuality,
    ) -> Result<Self, NamError> {
        let path = path.as_ref();
        let estimate = {
            let mut probe = Self::load_with_quality(path, sample_rate, max_buffer_frames, quality)?;
            alignment::measure(sample_rate, max_buffer_frames, |samples| {
                probe.process(samples)
            })
        };
        let mut model = Self::load_with_quality(path, sample_rate, max_buffer_frames, quality)?;
        model.alignment = Some(estimate);
        Ok(model)
    }

    pub fn alignment(&self) -> Option<alignment::AlignmentEstimate> {
        self.alignment
    }
    /// Read on the control/loading thread. Missing metadata is never guessed.
    pub fn metadata(&self) -> ModelMetadata {
        let optional = |v: f64| v.is_finite().then_some(v);
        // SAFETY: the model is exclusively owned and alive throughout these calls.
        unsafe {
            ModelMetadata {
                input_level_dbu: optional(pedal_nam_input_level(self.raw.as_ptr())),
                output_level_dbu: optional(pedal_nam_output_level(self.raw.as_ptr())),
                loudness_db: optional(pedal_nam_loudness(self.raw.as_ptr())),
                expected_sample_rate: pedal_nam_expected_sample_rate(self.raw.as_ptr()),
            }
        }
    }
    pub fn load(
        path: impl AsRef<Path>,
        sample_rate: f64,
        max_buffer_frames: usize,
    ) -> Result<Self, NamError> {
        Self::load_with_quality(path, sample_rate, max_buffer_frames, NamQuality::Full)
    }

    /// Loading, tier selection and prewarming are NOT real-time safe. Prepare a
    /// replacement model off-thread, then use the engine's faded model swap.
    pub fn load_with_quality(
        path: impl AsRef<Path>,
        sample_rate: f64,
        max_buffer_frames: usize,
        quality: NamQuality,
    ) -> Result<Self, NamError> {
        let path = path.as_ref();
        let path_string = path.to_str().ok_or_else(|| NamError {
            status: 1,
            message: "NAM path is not valid UTF-8".to_owned(),
        })?;
        let path = CString::new(path_string).map_err(|_| NamError {
            status: 1,
            message: "NAM path contains a null byte".to_owned(),
        })?;
        let max_buffer_frames = c_int::try_from(max_buffer_frames).map_err(|_| NamError {
            status: 1,
            message: "maximum buffer size exceeds the C ABI range".to_owned(),
        })?;
        let mut raw = std::ptr::null_mut();
        let mut error = [0 as c_char; ERROR_CAPACITY];
        // SAFETY: All pointers are valid for the duration of the call and the output starts null.
        let status = unsafe {
            pedal_nam_load(
                path.as_ptr(),
                sample_rate,
                max_buffer_frames,
                i32::from(quality == NamQuality::Full),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if status != STATUS_OK {
            // SAFETY: The C++ bridge always null-terminates this fixed-size error buffer.
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            return Err(NamError { status, message });
        }
        let raw = NonNull::new(raw).ok_or_else(|| NamError {
            status: 2,
            message: "NAM Core reported success without returning a model".to_owned(),
        })?;

        Ok(Self {
            alignment: None,
            raw,
            max_buffer_frames: max_buffer_frames as usize,
            input_workspace: vec![0.0; max_buffer_frames as usize],
            faulted: false,
            _not_sync: PhantomData,
        })
    }

    /// Allocation-free failure path for realtime graph adapters. Caller handles
    /// error reporting on the control thread and must silence failed output.
    /// Blocks longer than the prepared size are processed in prepared-size
    /// slices; NAM streams its state, so slicing is sample-exact.
    pub fn process_realtime(&mut self, samples: &mut [f32]) -> bool {
        if self.faulted || self.max_buffer_frames == 0 {
            self.faulted = true;
            return false;
        }
        for slice in samples.chunks_mut(self.max_buffer_frames) {
            if self.process_slice(slice) != STATUS_OK {
                self.faulted = true;
                return false;
            }
        }
        true
    }

    pub fn process(&mut self, samples: &mut [f32]) -> Result<(), NamError> {
        if self.max_buffer_frames == 0 {
            return Err(NamError {
                status: 1,
                message: "NAM model was prepared with a zero buffer size".to_owned(),
            });
        }
        for slice in samples.chunks_mut(self.max_buffer_frames) {
            let status = self.process_slice(slice);
            if status != STATUS_OK {
                self.faulted = true;
                return Err(NamError {
                    status,
                    message: "NAM processing failed".to_owned(),
                });
            }
        }
        Ok(())
    }

    /// `slice.len()` must not exceed `max_buffer_frames`.
    fn process_slice(&mut self, slice: &mut [f32]) -> c_int {
        debug_assert!(slice.len() <= self.max_buffer_frames);
        // NAM supports distinct buffers. This workspace was allocated during model loading.
        self.input_workspace[..slice.len()].copy_from_slice(slice);
        // SAFETY: The model is exclusively borrowed; buffers are valid and have `slice.len()` frames.
        unsafe {
            pedal_nam_process(
                self.raw.as_ptr(),
                self.input_workspace.as_ptr(),
                slice.as_mut_ptr(),
                slice.len() as c_int,
            )
        }
    }

    pub fn expected_sample_rate(&self) -> f64 {
        // SAFETY: `self.raw` stays valid until Drop.
        unsafe { pedal_nam_expected_sample_rate(self.raw.as_ptr()) }
    }

    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    pub fn supports_sizes(&self) -> bool {
        // SAFETY: immutable flag on this exclusively owned, live model.
        unsafe { pedal_nam_supports_sizes(self.raw.as_ptr()) != 0 }
    }
}

impl Drop for NamModel {
    fn drop(&mut self) {
        // SAFETY: This is the unique pointer returned by `pedal_nam_load` and is freed exactly once.
        unsafe { pedal_nam_free(self.raw.as_ptr()) };
    }
}
