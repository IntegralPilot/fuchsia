// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Format;
use fuchsia_runtime::vmar_root_self;

use thiserror::Error;

// Given coefficients for an affine function that transforms 'a' into 'b', apply it to 'a_value'.
// The affine function is:
//   b = ((a_value - a_offset) * numerator / denominator) + b_offset
//
// Generally we use these functions to translate between a running frame and a time. 'frame_number'
// is usually positive (and monotonic time should always be) but there are corner cases such as
// `presentation_frame()` where the result can be negative, so params and retval are signed. To
// retain full precision during the multiply-then-divide, we use i128. When available, we will use
// div_floor so we always round in the same direction (toward -INF) regardless of pos or neg values.
pub fn apply_affine_function(
    a_value: i64,
    numerator: u32,
    denominator: u32,
    a_offset: i64,
    b_offset: i64,
) -> i64 {
    let numerator_i128 = numerator as i128;
    let denominator_i128 = denominator as i128;

    // TODO: When `int_roundings` is released, do this instead:
    // let intermediate = (a_value - a_offset) as i128 * numerator_i128;
    // /Fractions should round NEGATIVELY (toward -INF, not toward ZERO as happens in int division).
    // intermediate.div_floor(denominator_i128) as i64 + b_offset

    // Fractions should round NEGATIVELY (toward -INF, not toward ZERO as happens in int division).
    let mut intermediate = (a_value - a_offset) as i128 * numerator_i128;
    // Check for negative; if so, subtract "almost-one" before the DIV so it rounds toward -INF.
    if intermediate < 0 {
        intermediate = intermediate - denominator_i128 + 1;
    }
    (intermediate / denominator_i128) as i64 + b_offset
}

/// A VMO-backed ring buffer that contains frames of audio.
pub struct VmoBuffer {
    /// VMO that contains `num_frames` of audio in `format`.
    vmo: zx::Vmo,

    /// Size of the VMO, in bytes.
    vmo_size_bytes: u64,

    /// Number of frames in the VMO.
    num_frames: u64,

    /// Format of each frame.
    format: Format,

    /// Base address of the memory-mapped `vmo`.
    base_address: usize,
}

impl VmoBuffer {
    pub fn new(vmo: zx::Vmo, num_frames: u64, format: Format) -> Result<Self, VmoBufferError> {
        // Ensure that the VMO is big enough to hold `num_frames` of audio in the given `format`.
        let data_size_bytes = num_frames * format.bytes_per_frame() as u64;
        let vmo_size_bytes = vmo.get_size().map_err(VmoBufferError::VmoGetSize)?;

        if data_size_bytes > vmo_size_bytes {
            return Err(VmoBufferError::VmoTooSmall { data_size_bytes, vmo_size_bytes });
        }

        let base_address = vmar_root_self()
            .map(
                0,
                &vmo,
                0,
                vmo_size_bytes as usize,
                // TODO(https://fxbug.dev/356700720): Don't map read-only VMOs with `PERM_WRITE`.
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            )
            .map_err(VmoBufferError::VmoMap)?;

        log::debug!(
            "format {:?} num frames {} data_size_bytes {}",
            format,
            num_frames,
            data_size_bytes
        );

        Ok(Self { vmo, vmo_size_bytes, num_frames, format, base_address })
    }

    /// Returns the size of the buffer in bytes.
    ///
    /// This may be less than the size of the backing VMO.
    pub fn data_size_bytes(&self) -> u64 {
        self.num_frames * self.format.bytes_per_frame() as u64
    }

    /// Writes all frames from `buf` to the ring buffer at position `running_frame`.
    pub fn write_to_frame(&self, running_frame: u64, buf: &[u8]) -> Result<(), VmoBufferError> {
        if !buf.len().is_multiple_of(self.format.bytes_per_frame() as usize) {
            return Err(VmoBufferError::BufferIncompleteFrames);
        }
        let frame_offset = running_frame % self.num_frames;
        let byte_offset = frame_offset as usize * self.format.bytes_per_frame() as usize;
        let num_frames_in_buf = buf.len() as u64 / self.format.bytes_per_frame() as u64;

        // Check whether the buffer can be written to contiguously or if the write needs to be
        // split into two: one until the end of the buffer and one starting from the beginning.
        if (frame_offset + num_frames_in_buf) <= self.num_frames {
            self.vmo.write(buf, byte_offset as u64).map_err(VmoBufferError::VmoWrite)?;
            // Flush cache so that hardware reads most recent write.
            self.flush_cache(byte_offset, buf.len()).map_err(VmoBufferError::VmoFlushCache)?;
            log::debug!(
                "At {} wrote {} frames at ring_buf pos {} (running frame {}-{})",
                zx::MonotonicInstant::get().into_nanos(),
                num_frames_in_buf,
                frame_offset,
                running_frame,
                running_frame + num_frames_in_buf
            );
            fuchsia_trace::instant!(
                c"audio-streaming",
                c"AudioLib::VmoBuffer::write_to_frame",
                fuchsia_trace::Scope::Process,
                "source bytes to write" => buf.len(),
                "source frames to write" => num_frames_in_buf,
                "frame size" => self.format.bytes_per_frame(),
                "dest RB size (frames)" => self.num_frames,
                "dest RB running frame" => running_frame,
                "dest RB frame position" => frame_offset,
                "vmo write start" => byte_offset,
                "vmo write len (bytes)" => buf.len()
            );
        } else {
            let frames_to_write_until_end = self.num_frames - frame_offset;
            let bytes_until_buffer_end =
                frames_to_write_until_end as usize * self.format.bytes_per_frame() as usize;

            self.vmo
                .write(&buf[..bytes_until_buffer_end], byte_offset as u64)
                .map_err(VmoBufferError::VmoWrite)?;
            // Flush cache so that hardware reads most recent write.
            self.flush_cache(byte_offset, bytes_until_buffer_end)
                .map_err(VmoBufferError::VmoFlushCache)?;
            log::debug!(
                "At {} first {} frames at ring_buf pos {} (running frame {}-{})",
                zx::MonotonicInstant::get().into_nanos(),
                frames_to_write_until_end,
                frame_offset,
                running_frame,
                running_frame + frames_to_write_until_end
            );

            if buf[bytes_until_buffer_end..].len() > self.vmo_size_bytes as usize {
                log::error!(
                    "Remainder of write buffer (size {}) is too big for the vmo (size {})",
                    buf[bytes_until_buffer_end..].len(),
                    self.vmo_size_bytes
                );
            }

            // Write what remains to the beginning of the buffer.
            self.vmo.write(&buf[bytes_until_buffer_end..], 0).map_err(VmoBufferError::VmoWrite)?;
            self.flush_cache(0, buf.len() - bytes_until_buffer_end)
                .map_err(VmoBufferError::VmoFlushCache)?;
            fuchsia_trace::instant!(
                c"audio-streaming",
                c"AudioLib::VmoBuffer::write_to_frame",
                fuchsia_trace::Scope::Process,
                "source bytes to write" => buf.len(),
                "source frames to write" => num_frames_in_buf,
                "frame size" => self.format.bytes_per_frame(),
                "dest RB size (frames)" => self.num_frames,
                "dest RB running frame" => running_frame,
                "dest RB frame position" => frame_offset,
                "first vmo write start" => byte_offset,
                "first vmo write len (bytes)" => bytes_until_buffer_end,
                "second vmo write start" => 0,
                "second vmo write len (bytes)" => buf.len() - bytes_until_buffer_end
            );
            log::debug!(
                "At {}, then {} frames at ring_buf frame 0 (running frame {}-{})",
                zx::MonotonicInstant::get().into_nanos(),
                num_frames_in_buf - frames_to_write_until_end,
                running_frame + frames_to_write_until_end,
                running_frame + num_frames_in_buf
            );
        }

        Ok(())
    }

    /// Reads frames from the ring buffer into `buf` starting at position `running_frame`.
    pub fn read_from_frame(
        &self,
        running_frame: u64,
        buf: &mut [u8],
    ) -> Result<(), VmoBufferError> {
        if !buf.len().is_multiple_of(self.format.bytes_per_frame() as usize) {
            return Err(VmoBufferError::BufferIncompleteFrames);
        }
        let frame_offset = running_frame % self.num_frames;
        let byte_offset = frame_offset as usize * self.format.bytes_per_frame() as usize;
        let num_frames_in_buf = buf.len() as u64 / self.format.bytes_per_frame() as u64;

        // Check whether the buffer can be read from contiguously or if the read needs to be
        // split into two: one until the end of the buffer and one starting from the beginning.
        if (frame_offset + num_frames_in_buf) <= self.num_frames {
            // Flush and invalidate cache so we read the hardware's most recent write.
            log::debug!("frame {} reading starting from position {}", running_frame, byte_offset);
            self.flush_invalidate_cache(byte_offset, buf.len())
                .map_err(VmoBufferError::VmoFlushCache)?;
            self.vmo.read(buf, byte_offset as u64).map_err(VmoBufferError::VmoRead)?;
        } else {
            let frames_to_write_until_end = self.num_frames - frame_offset;
            let bytes_until_buffer_end =
                frames_to_write_until_end as usize * self.format.bytes_per_frame() as usize;

            log::debug!(
                "frame {} reading starting from position {}  (with looparound)",
                running_frame,
                byte_offset
            );
            // Flush and invalidate cache so we read the hardware's most recent write.
            self.flush_invalidate_cache(byte_offset, bytes_until_buffer_end)
                .map_err(VmoBufferError::VmoFlushCache)?;
            self.vmo
                .read(&mut buf[..bytes_until_buffer_end], byte_offset as u64)
                .map_err(VmoBufferError::VmoRead)?;

            if buf[bytes_until_buffer_end..].len() > self.vmo_size_bytes as usize {
                log::error!("Remainder of read buffer is too big for the vmo.");
            }

            self.flush_invalidate_cache(0, buf.len() - bytes_until_buffer_end)
                .map_err(VmoBufferError::VmoFlushCache)?;
            self.vmo
                .read(&mut buf[bytes_until_buffer_end..], 0)
                .map_err(VmoBufferError::VmoRead)?;
        }
        Ok(())
    }

    /// Flush the cache for a portion of the memory-mapped VMO.
    // TODO(https://fxbug.dev/328478694): Remove these methods once VMOs are created without caching
    fn flush_cache(&self, offset_bytes: usize, size_bytes: usize) -> Result<(), zx::Status> {
        assert!(offset_bytes + size_bytes <= self.vmo_size_bytes as usize);
        let status = unsafe {
            // SAFETY: The range was asserted above to be within the mapped region of the VMO.
            zx::sys::zx_cache_flush(
                (self.base_address + offset_bytes) as *mut u8,
                size_bytes,
                zx::sys::ZX_CACHE_FLUSH_DATA,
            )
        };
        zx::Status::ok(status)
    }

    /// Flush and invalidate cache for a portion of the memory-mapped VMO.
    // TODO(https://fxbug.dev/328478694): Remove these methods once VMOs are created without caching
    fn flush_invalidate_cache(
        &self,
        offset_bytes: usize,
        size_bytes: usize,
    ) -> Result<(), zx::Status> {
        assert!(offset_bytes + size_bytes <= self.vmo_size_bytes as usize);
        let status = unsafe {
            // SAFETY: The range was asserted above to be within the mapped region of the VMO.
            zx::sys::zx_cache_flush(
                (self.base_address + offset_bytes) as *mut u8,
                size_bytes,
                zx::sys::ZX_CACHE_FLUSH_DATA | zx::sys::ZX_CACHE_FLUSH_INVALIDATE,
            )
        };
        zx::Status::ok(status)
    }
}

impl Drop for VmoBuffer {
    fn drop(&mut self) {
        // SAFETY: `base_address` and `vmo_size_bytes` are private to self,
        // so no other code can observe that this mapping has been removed.
        unsafe {
            vmar_root_self().unmap(self.base_address, self.vmo_size_bytes as usize).unwrap();
        }
    }
}

#[derive(Error, Debug)]
pub enum VmoBufferError {
    #[error(
        "VMO is too small ({vmo_size_bytes} bytes) to hold ring buffer data ({data_size_bytes} bytes)"
    )]
    VmoTooSmall { data_size_bytes: u64, vmo_size_bytes: u64 },

    #[error("Buffer size is invalid; contains incomplete frames")]
    BufferIncompleteFrames,

    #[error("Failed to memory map VMO: {}", .0)]
    VmoMap(#[source] zx::Status),

    #[error("Failed to get VMO size: {}", .0)]
    VmoGetSize(#[source] zx::Status),

    #[error("Failed to flush VMO memory cache: {}", .0)]
    VmoFlushCache(#[source] zx::Status),

    #[error("Failed to read from VMO: {}", .0)]
    VmoRead(#[source] zx::Status),

    #[error("Failed to write to VMO: {}", .0)]
    VmoWrite(#[source] zx::Status),
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::format::SampleType;
    use assert_matches::assert_matches;

    // For a given start time, validate that one second translates to a second of frames.
    #[test]
    fn affine_function_time_to_frames() {
        let start_time = zx::MonotonicInstant::from_nanos(1_234_567_890);
        let current_time = start_time + zx::MonotonicDuration::from_seconds(1);

        let frames_per_second: u32 = 48000;
        let nanos_per_second: u32 = 1_000_000_000;

        let start_frame: i64 = 1_234_567;
        let expected_frame = start_frame + (frames_per_second as i64);

        let current_frame = apply_affine_function(
            current_time.into_nanos(),
            frames_per_second,
            nanos_per_second,
            start_time.into_nanos(),
            start_frame,
        );

        assert_eq!(current_frame, expected_frame);
    }

    // For a given start frame, validate that a second of frames translates to one second.
    #[test]
    fn affine_function_frames_to_time() {
        let nanos_per_second: u32 = 1_000_000_000;
        let frames_per_second: u32 = 48000;

        let start_frame: i64 = 1_234_567;
        let current_frame: i64 = start_frame + (frames_per_second as i64);

        let start_time = zx::MonotonicInstant::from_nanos(1_234_567_890);
        let expected_time = start_time + zx::MonotonicDuration::from_seconds(1);

        let time_for_frame = apply_affine_function(
            current_frame,
            nanos_per_second,
            frames_per_second,
            start_frame,
            start_time.into_nanos(),
        );

        assert_eq!(time_for_frame, expected_time.into_nanos());
    }

    // Validate that 0.999_999_999 second leads to a frame value that is NOT rounded up -- this
    // should result in 47999 frames, not 48000.
    #[test]
    fn affine_function_rounds_down() {
        let start_time = zx::MonotonicInstant::from_nanos(1_234_567_890);
        let current_time = start_time + zx::MonotonicDuration::from_seconds(1)
            - zx::MonotonicDuration::from_nanos(1);

        let frames_per_second: u32 = 48000;
        let nanos_per_second: u32 = 1_000_000_000;

        let start_frame: i64 = 1_234_567;
        let expected_frame = start_frame + (frames_per_second as i64) - 1;

        let current_frame = apply_affine_function(
            current_time.into_nanos(),
            frames_per_second,
            nanos_per_second,
            start_time.into_nanos(),
            start_frame,
        );

        assert_eq!(current_frame, expected_frame);
    }

    // Affine functions should always round the same direction.
    // Thus they must round negatively (toward -INF), rather than rounding toward zero.
    // Validate that -1.000_000_001 second leads to a frame value that is NOT rounded up -- this
    // should result in -48001 frames, not -48000.
    #[test]
    fn affine_function_rounds_negatively() {
        let start_time = zx::MonotonicInstant::from_nanos(1_234_567_890);
        let current_time = start_time
            - zx::MonotonicDuration::from_seconds(1)
            - zx::MonotonicDuration::from_nanos(1);

        let frames_per_second: u32 = 48000;
        let nanos_per_second: u32 = 1_000_000_000;

        let start_frame: i64 = 1_234_567;
        let expected_frame = start_frame - (frames_per_second as i64) - 1;

        let current_frame = apply_affine_function(
            current_time.into_nanos(),
            frames_per_second,
            nanos_per_second,
            start_time.into_nanos(),
            start_frame,
        );

        assert_eq!(current_frame, expected_frame);
    }

    #[test]
    fn vmobuffer_vmo_too_small() {
        let format =
            Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 1 };

        // VMO size is rounded up to the system page size.
        let page_size = zx::system_get_page_size() as u64;
        let num_frames = page_size + 1;
        let vmo = zx::Vmo::create(page_size).unwrap();

        assert_matches!(
            VmoBuffer::new(vmo, num_frames, format).err(),
            Some(VmoBufferError::VmoTooSmall { .. })
        )
    }

    #[test]
    fn vmobuffer_read_write() {
        let format =
            Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 2 };
        const NUM_FRAMES_VMO: u64 = 10;
        const NUM_FRAMES_BUF: u64 = 5;
        const SAMPLE: u8 = 42;

        let vmo_size = format.bytes_per_frame() as u64 * NUM_FRAMES_VMO;
        let buf_size = format.bytes_per_frame() as u64 * NUM_FRAMES_BUF;

        let vmo = zx::Vmo::create(vmo_size).unwrap();

        // Buffer used to read from the VmoBuffer.
        let mut in_buf = vec![0; buf_size as usize];
        // Buffer used to write to from the VmoBuffer.
        let out_buf = vec![SAMPLE; buf_size as usize];

        let vmo_buffer = VmoBuffer::new(vmo, NUM_FRAMES_VMO, format).unwrap();

        // Write the buffer to the VmoBuffer, starting on the second frame (zero based).
        vmo_buffer.write_to_frame(1, &out_buf).unwrap();

        // Read back from the VmoBuffer.
        vmo_buffer.read_from_frame(1, &mut in_buf).unwrap();

        assert_eq!(in_buf, out_buf);
    }

    #[test]
    fn vmobuffer_read_write_wrapping() {
        let format =
            Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 2 };
        let page_size = zx::system_get_page_size() as u64;
        let num_frames_vmo: u64 = page_size;
        let num_frames_buf: u64 = page_size / 2;
        const SAMPLE: u8 = 42;

        let vmo_size = format.bytes_per_frame() as u64 * num_frames_vmo;
        let buf_size = format.bytes_per_frame() as u64 * num_frames_buf;

        let vmo = zx::Vmo::create(vmo_size).unwrap();

        // Buffer used to read from the VmoBuffer.
        let mut in_buf = vec![0; buf_size as usize];
        // Buffer used to write to from the VmoBuffer.
        let out_buf = vec![SAMPLE; buf_size as usize];

        let vmo_buffer = VmoBuffer::new(vmo, num_frames_vmo, format).unwrap();

        // Write and read at the last frame to ensure the operations wrap to the beginning.
        let frame = num_frames_vmo - 1;
        assert!(frame + num_frames_buf > num_frames_vmo);

        // Write the buffer to the VmoBuffer, starting on the last frame.
        vmo_buffer.write_to_frame(frame, &out_buf).unwrap();

        // Read back from the VmoBuffer.
        vmo_buffer.read_from_frame(frame, &mut in_buf).unwrap();

        assert_eq!(in_buf, out_buf);
    }
}
