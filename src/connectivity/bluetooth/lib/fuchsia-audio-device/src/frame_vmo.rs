// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_audio::*;
use fuchsia_async as fasync;
use futures::FutureExt;
use futures::task::{Context, Poll, Waker};
use std::pin::Pin;

use crate::stream_config::frames_from_duration;
use crate::types::{AudioSampleFormat, Error, Result};

pub(crate) enum FrameReadResult {
    Frames { next_frame: usize, missed: usize },
    AudioDisabled,
}

/// A FrameVmo wraps a VMO with time tracking.  When a FrameVmo is started, it
/// assumes that audio frame data is being written to the VMO at the rate specific
/// in the format it is set to.  Frames that represent a time range can be
/// retrieved from the buffer.
pub(crate) struct FrameVmo {
    /// Ring Buffer VMO. Size zero until the ringbuffer is established.  Shared with
    /// the AudioFrameStream given back to the client.
    vmo: Option<zx::Vmo>,

    /// Cached size of the ringbuffer, in bytes.  Used to avoid zx_get_size() syscalls.
    size: usize,

    /// The time that streaming was started.
    /// Used to calculate the currently available frames.
    /// None if the stream is not started.
    start_time: Option<fasync::MonotonicInstant>,

    /// A waker to wake when enough frames are available after start, or after output is resumed.
    audio_waker: Option<Waker>,

    /// The channels that are currently active.
    /// If no channels are active, the cursor continues but we will not wake until channels are
    /// made active again.
    active_channels_mask: u64,

    /// The time that the active channels set was last changed.
    /// INFINITE_PAST if the channels have never been changed.
    active_channels_activity: fasync::MonotonicInstant,

    /// A waker to wake tasks waiting on active channels state changes.
    active_wakers: Vec<Waker>,

    /// The last notified state of active channels (true = active, false = inactive).
    active_channels_notified: Option<bool>,

    /// Whether AudioDisabled event has been notified to poll_read callers.
    stream_disabled_notified: bool,

    /// A timer which will fire when there are enough frames to return min_duration frames.
    timer: Option<Pin<Box<fasync::Timer>>>,

    /// The number of frames per second.
    frames_per_second: u32,

    /// The audio format of the frames.
    format: Option<AudioSampleFormat>,

    /// The number of channels.
    channels: Option<u16>,

    /// Number of bytes per frame, 0 if format is not set.
    bytes_per_frame: usize,

    /// Frames between notifications. Zero if position notifications aren't enabled
    frames_between_notifications: usize,

    /// A position responder that will be used to notify when the ringbuffer has been read.
    /// This will be notified the correct number of times based on the last read position using
    /// `get_frames`
    position_responder: Option<RingBufferWatchClockRecoveryPositionInfoResponder>,

    /// The next frame index we are due to notify the position.
    /// usize::MAX if position notifications are not enabled.
    next_notify_frame: usize,
}

impl FrameVmo {
    pub(crate) fn new() -> Result<FrameVmo> {
        Ok(FrameVmo {
            vmo: None,
            size: 0,
            start_time: None,
            audio_waker: None,
            frames_per_second: 0,
            active_channels_mask: u64::MAX, // by default all channels are active
            active_channels_activity: fasync::MonotonicInstant::INFINITE_PAST,
            active_wakers: Vec::with_capacity(1), // we expect only one interest in activity
            active_channels_notified: None,
            stream_disabled_notified: false,
            format: None,
            channels: None,
            timer: None,
            bytes_per_frame: 0,
            frames_between_notifications: 0,
            position_responder: None,
            next_notify_frame: usize::MAX,
        })
    }

    /// Set the format of this buffer.
    /// Can not be set if the buffer is already started.
    pub(crate) fn set_format(
        &mut self,
        frames_per_second: u32,
        format: AudioSampleFormat,
        channels: u16,
    ) -> Result<()> {
        if self.start_time.is_some() {
            return Err(Error::InvalidState);
        }
        self.channels = Some(channels);
        self.format = Some(format);
        self.frames_per_second = frames_per_second;
        Ok(())
    }

    /// Allocate the VMO, based on a minumum number of frames and the number
    /// of notifications per rotation of the buffer.
    /// Returns a handle representing the VMO.
    pub(crate) fn get_vmo(
        &mut self,
        frames: usize,
        notifications_per_ring: u32,
    ) -> Result<zx::Vmo> {
        if self.start_time.is_some() {
            return Err(Error::InvalidState);
        }
        let (Some(format), Some(channels)) = (self.format.as_ref(), self.channels) else {
            return Err(Error::InvalidState);
        };
        let bytes_per_frame = format.compute_frame_size(channels as usize)?;
        let new_size = bytes_per_frame * frames;
        let vmo = zx::Vmo::create(new_size as u64).map_err(|e| Error::IOError(e))?;
        self.vmo =
            Some(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(|e| Error::IOError(e))?);
        self.bytes_per_frame = bytes_per_frame;
        self.size = new_size;
        if channels > 64 {
            return Err(Error::InvalidArgs);
        }
        if notifications_per_ring > 0 {
            // TODO(https://fxbug.dev/42174677) : consider rounding this up to avoid delivering an extra
            // notification sometimes.
            // (and always align the frames notified to the beginning of the buffer)
            self.frames_between_notifications = frames / notifications_per_ring as usize;
        }
        Ok(vmo)
    }

    pub(crate) fn set_position_responder(
        &mut self,
        position_responder: RingBufferWatchClockRecoveryPositionInfoResponder,
    ) {
        self.position_responder = Some(position_responder);
    }

    /// Start the audio clock for the buffer at `time`
    /// Can't start if the format has not been set.
    pub(crate) fn start(&mut self, time: fasync::MonotonicInstant) -> Result<()> {
        if self.start_time.is_some() || self.vmo.is_none() {
            return Err(Error::InvalidState);
        }
        self.start_time = Some(time);
        if self.frames_between_notifications > 0 {
            self.next_notify_frame = 0;
        }
        let _ = self.audio_waker.take().map(Waker::wake);
        Ok(())
    }

    /// Stop the audio clock in the buffer.
    /// returns true if the streaming was stopped.
    pub(crate) fn stop(&mut self) -> Result<bool> {
        if self.vmo.is_none() {
            return Err(Error::InvalidState);
        }
        let start_time = self.start_time.take();
        Ok(start_time.is_some())
    }

    pub(crate) fn set_active_channels(
        &mut self,
        channel_mask: u64,
    ) -> Result<fasync::MonotonicInstant> {
        let Some(channels) = self.channels else {
            return Err(Error::InvalidState);
        };
        if channels > 64 {
            return Err(Error::InvalidState);
        }
        if (channel_mask >> channels) != 0 {
            return Err(Error::InvalidArgs);
        }
        // If there is no active channels change we return the last time
        // active channels has been changed, based on policy of
        // RingBuffer::SetActiveChannels
        if channel_mask == self.active_channels_mask {
            return Ok(self.active_channels_activity);
        }
        self.active_channels_mask = channel_mask;
        self.active_channels_activity = fasync::MonotonicInstant::now();
        self.stream_disabled_notified = false;
        let _ = self.audio_waker.take().map(Waker::wake);
        self.active_wakers.drain(..).for_each(Waker::wake);
        Ok(self.active_channels_activity)
    }

    pub(crate) fn poll_active_channels(&mut self, cx: &mut Context<'_>) -> Poll<bool> {
        let is_active = self.active_channels_mask != 0;
        if self.active_channels_notified != Some(is_active) {
            self.active_channels_notified = Some(is_active);
            Poll::Ready(is_active)
        } else {
            self.active_wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }

    /// Set the next-available-frame timer to fire `count` frames in the future from now.
    /// Replaces the currently active timer.
    /// Returns Poll::Pending if it was set to a time in the future, or Poll::Ready(()) if it would
    /// be ready now.
    fn reset_frame_timer(
        &mut self,
        deadline: fasync::MonotonicInstant,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        let mut timer = Box::pin(fasync::Timer::new(deadline));
        let Poll::Pending = timer.poll_unpin(cx) else {
            return Poll::Ready(());
        };
        self.timer = Some(timer);
        Poll::Pending
    }

    /// Calculate the number of bytes that `num` frames will occupy given the current set format.
    /// Returns None if no format has been set.
    pub(crate) fn bytecount_frames(&self, num: usize) -> Option<usize> {
        (self.bytes_per_frame != 0).then_some(self.bytes_per_frame * num)
    }

    fn poll_start(&mut self, cx: &mut Context<'_>) -> Poll<fasync::MonotonicInstant> {
        match self.start_time.as_ref() {
            None => {
                self.audio_waker = Some(cx.waker().clone());
                Poll::Pending
            }
            Some(time) => Poll::Ready(*time),
        }
    }

    /// The number of frames total in the VMO.
    /// Panics if the format has not been set.
    fn frames(&self) -> usize {
        self.size / self.bytes_per_frame
    }

    /// Index in the vmo of `frame_number`
    /// Panicks if the format has not been set.
    fn frame_idx(&self, frame_number: usize) -> usize {
        frame_number % self.frames()
    }

    /// Returns a pair (low, high) of the set of frames within the "safe window" where the
    /// client of the VMO is guaranteed to be able to read/write from, as of `time`.
    /// The safe frame window advances forward through time.
    /// TODO(https://fxbug.dev/42073419): currently this just returns the whole buffer
    /// it should only return the moving window we've reserved for ourselves.
    fn safe_frames_at(&self, time: fasync::MonotonicInstant) -> (usize, usize) {
        let youngest_frame_count = self.frames_before(time);
        let oldest_frame_count =
            youngest_frame_count.saturating_sub(self.size / self.bytes_per_frame);
        (oldest_frame_count, youngest_frame_count)
    }

    /// Notifies the position responder, if it's registered, that we have read or written past the
    /// next notification point. `frame` is the youngest frame that was touched.
    fn notify_position_responder(&mut self, frame: usize) {
        if frame > self.next_notify_frame && self.position_responder.is_some() {
            let start_time = self.start_time.unwrap();
            let end_frame_time = start_time + self.duration_from_frames(frame);
            let notify_frame_bytes = self.frame_idx(frame) * self.bytes_per_frame;
            let info = RingBufferPositionInfo {
                timestamp: end_frame_time.into_nanos(),
                position: notify_frame_bytes as u32,
            };
            if let Some(Ok(())) = self.position_responder.take().map(|t| t.send(&info)) {
                self.next_notify_frame += self.frames_between_notifications;
            }
        }
    }

    /// Gets the length of `buf` in frames. Returns Err(InvalidArgs) if the buffer is not an exact
    /// number of frames.
    fn len_in_frames(&self, buf: &[u8]) -> Result<usize> {
        match buf.len().checked_div(self.bytes_per_frame) {
            None => Err(Error::InvalidState),
            Some(0) => Err(Error::InvalidArgs),
            Some(_) if (buf.len() % self.bytes_per_frame) != 0 => Err(Error::InvalidArgs),
            Some(len) => Ok(len),
        }
    }

    /// Write audio frames starting at `next_frame`.
    /// `next_frame` is a frame index, starting at zero when the buffer timer was started.
    /// Reads from `buf` and writes into the VMO as whole frames.
    /// Returns the index of the next frame to write (can be passed to `next_frame` on next poll),
    /// and a count of frames that can never be written due to the frames being outside the write
    /// window (too late writing).
    /// Those frames provided in the `buf` are ignored, which can result in the entire buffer being
    /// ignored.
    /// If there is not enough space to write up to the end of the buffer, Poll::Pending is returned
    /// and the waker associated with `cx` will be woken at a time when the write pointer has
    /// advanced far enough to write the whole buffer.
    /// If called before start, the waker associated with `cx` will be woken when started.
    /// Returns Err(Error::InvalidArgs) if the buffer size is not a multiple of a positive number
    /// of frames, or the buffer is larger than the possible available frames in the buffer.
    /// See `bytecount_frames` to calculate buffer sizes for a count of frames.
    ///
    /// Calling poll_write with buffers of varying size is not expected, but should be supported.
    /// Calling poll_write and poll_read on the same FrameVmo is not expected to work correctly.
    pub(crate) fn poll_write(
        &mut self,
        mut next_frame: usize,
        mut buf: &[u8],
        cx: &mut Context<'_>,
    ) -> Poll<Result<(usize, usize)>> {
        let start_time = futures::ready!(self.poll_start(cx));
        let count = self.len_in_frames(buf)?;
        let total_vmo_frames = self.frames();
        if count > total_vmo_frames || count == 0 {
            return Poll::Ready(Err(Error::InvalidArgs));
        }

        let frame_until = next_frame + count;

        let now = fasync::MonotonicInstant::now();
        let (_oldest_frame_count, current_frame_count) = self.safe_frames_at(now);
        // We write _ahead_ of the read pointer.
        if (current_frame_count + total_vmo_frames) < frame_until {
            let deadline = start_time + self.duration_from_frames(frame_until - total_vmo_frames);
            let () = futures::ready!(self.reset_frame_timer(deadline, cx));
            // Somehow raced to a time where they are available, try again.
            return self.poll_write(next_frame, buf, cx);
        }

        let missing_frames = current_frame_count.saturating_sub(next_frame);

        if missing_frames >= count {
            // Missed all of them. boo underflows.
            // The oldest frame of the buf is considered to be "already written".
            self.notify_position_responder(frame_until);
            return Poll::Ready(Ok((frame_until, count)));
        } else if missing_frames > 0 {
            next_frame = current_frame_count;
            (_, buf) = buf.split_at(missing_frames * self.bytes_per_frame);
            assert_eq!(frame_until - next_frame, self.len_in_frames(buf).unwrap());
        }

        let mut frame_from_idx = self.frame_idx(next_frame);
        let frame_until_idx = self.frame_idx(frame_until);
        let mut ndx = 0;

        let Some(vmo) = self.vmo.as_mut() else {
            return Poll::Ready(Err(Error::InvalidState));
        };

        // If we wrap around, write to the end of the VMO, then set up to write the rest.
        if frame_from_idx >= frame_until_idx {
            let frames_to_write = total_vmo_frames - frame_from_idx;
            let bytes_to_write = frames_to_write * self.bytes_per_frame;
            let byte_start = frame_from_idx * self.bytes_per_frame;
            vmo.write(&buf[0..bytes_to_write], byte_start as u64).map_err(|e| Error::IOError(e))?;
            frame_from_idx = 0;
            ndx = bytes_to_write;
        }

        let byte_start = frame_from_idx * self.bytes_per_frame;

        vmo.write(&buf[ndx..], byte_start as u64).map_err(|e| Error::IOError(e))?;

        // We're writing frames from just after the `next_frame` up to `frame_until`
        // Notify if we have a position responder on the first write past the clock notification time.
        self.notify_position_responder(frame_until);
        Poll::Ready(Ok((frame_until, missing_frames)))
    }

    /// Retrieve audio frames available starting at `next_frame`.
    /// `next_frame` is a frame index, starting at zero when the buffer timer was started.
    /// Fills `buf` with as many frames as will fill the slice.
    /// Returns the index of the next frame to read (can be passed to `next_trame` on next poll), and a
    /// count of frames that were missed due to buffer overwrite (slow polling).
    /// If the buffer cannot be filled, Poll::Pending is returned and the waker
    /// associated with `cx` will be woken at a time when enough data is expected to be
    /// available.
    /// If polled before start, the waker associate with `cx` will be woken when started.
    /// Returns Err(Error::InvalidArgs) if the buffer size is not a multiple of a positive number
    /// of frames, or the buffer is larger than the possible available frames in the buffer.
    /// See `bytecount_frames` to calculate buffer sizes for a count of frames.
    ///
    /// Calling poll_read with buffers of varying size is not expected, but should be supported.
    /// Calling poll_write and poll_read on the same FrameVmo is not expected to work correctly.
    pub(crate) fn poll_read(
        &mut self,
        mut next_frame: usize,
        buf: &mut [u8],
        cx: &mut Context<'_>,
    ) -> Poll<Result<FrameReadResult>> {
        let _start_time = futures::ready!(self.poll_start(cx));
        if self.active_channels_mask == 0 {
            if !self.stream_disabled_notified {
                self.stream_disabled_notified = true;
                return Poll::Ready(Ok(FrameReadResult::AudioDisabled));
            }
            self.audio_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = self.len_in_frames(buf)?;
        let total_vmo_frames = self.frames();
        if count > total_vmo_frames || count == 0 {
            return Poll::Ready(Err(Error::InvalidArgs));
        }

        let now = fasync::MonotonicInstant::now();
        let (oldest_frame_count, current_frame_count) = self.safe_frames_at(now);

        let missing_frames = oldest_frame_count.saturating_sub(next_frame);

        if missing_frames > 0 {
            next_frame = oldest_frame_count;
        }

        let frames_available = current_frame_count.saturating_sub(next_frame);
        if frames_available < count {
            // Set the timer to wake when enough frames will be available
            let deadline = now + self.duration_from_frames(count - frames_available);
            let () = futures::ready!(self.reset_frame_timer(deadline, cx));
            // Somehow raced to a time where they are available, try again.
            return self.poll_read(next_frame, buf, cx);
        }

        let mut frame_from_idx = self.frame_idx(next_frame);
        let frame_until = next_frame + count;
        let frame_until_idx = self.frame_idx(frame_until);
        let mut ndx = 0;

        let Some(vmo) = self.vmo.as_mut() else {
            return Poll::Ready(Err(Error::InvalidState));
        };

        // If we wrap around, read to the end into the buffer, then set up to read the rest.
        if frame_from_idx >= frame_until_idx {
            let frames_to_read = total_vmo_frames - frame_from_idx;
            let bytes_to_read = frames_to_read * self.bytes_per_frame;
            let byte_start = frame_from_idx * self.bytes_per_frame;
            vmo.read(&mut buf[0..bytes_to_read], byte_start as u64)
                .map_err(|e| Error::IOError(e))?;
            frame_from_idx = 0;
            ndx = bytes_to_read;
        }

        let byte_start = frame_from_idx * self.bytes_per_frame;

        vmo.read(&mut buf[ndx..], byte_start as u64).map_err(|e| Error::IOError(e))?;

        // We're returning frames from just after the `next_frame` up to `frame_until`
        // Notify if we have a position responder and we read past the clock notification time.
        self.notify_position_responder(frame_until);
        Poll::Ready(Ok(FrameReadResult::Frames { next_frame: frame_until, missed: missing_frames }))
    }

    /// Count of the number of frames that have ended before `time`.
    fn frames_before(&self, time: fasync::MonotonicInstant) -> usize {
        match self.start_time.as_ref() {
            Some(start) if time > *start => self.frames_from_duration(time - *start) as usize,
            _ => 0,
        }
    }

    /// Frames from duration, based on the current frames per second of the ringbuffer.
    fn frames_from_duration(&self, duration: fasync::MonotonicDuration) -> usize {
        frames_from_duration(self.frames_per_second as usize, duration)
    }

    /// Return an amount of time that guarantees that `frames` frames has passed.
    /// This means that partial nanoseconds will be rounded up, so that
    /// [time, time + duration_from_frames(n)] is guaranteed to include n audio frames.
    /// Only defined for positive numbers of frames.
    fn duration_from_frames(&self, frames: usize) -> fasync::MonotonicDuration {
        let fps = self.frames_per_second as i64;
        let secs = frames as i64 / fps;
        let leftover_frames = frames as i64 % fps;
        let nanos = (leftover_frames * 1_000_000_000i64) / fps;
        let roundup_nano = if 0 != ((leftover_frames * 1_000_000_000i64) % fps) { 1 } else { 0 };
        zx::MonotonicDuration::from_seconds(secs)
            + zx::MonotonicDuration::from_nanos(nanos + roundup_nano)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_utils::PollExt;
    use fixture::fixture;

    // Convenience choice because one byte = one frame.
    const TEST_FORMAT: AudioSampleFormat = AudioSampleFormat::Eight { unsigned: false };
    const TEST_CHANNELS: u16 = 1;
    const TEST_FPS: u32 = 48000;
    const TEST_FRAMES: usize = TEST_FPS as usize / 2;
    // Duration of the whole VMO.
    const TEST_VMO_DURATION: fasync::MonotonicDuration =
        fasync::MonotonicDuration::from_millis(500);

    // At 48kHz, each frame is 20833 and 1/3 nanoseconds. We add one nanosecond
    // so that the one and two frames are completely within the time.
    const ONE_FRAME_NANOS: i64 = 20833 + 1;
    const TWO_FRAME_NANOS: i64 = 20833 * 2 + 1;
    const THREE_FRAME_NANOS: i64 = 20833 * 3 + 1;

    fn get_test_vmo(frames: usize) -> FrameVmo {
        let mut vmo = FrameVmo::new().expect("can't make a framevmo");
        let _ = vmo.set_format(TEST_FPS, TEST_FORMAT, TEST_CHANNELS).unwrap();
        let _handle = vmo.get_vmo(frames, 0).unwrap();
        vmo
    }

    fn with_test_vmo<F>(_name: &str, test: F)
    where
        F: FnOnce(FrameVmo) -> (),
    {
        test(get_test_vmo(TEST_FRAMES))
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn duration_from_frames(vmo: FrameVmo) {
        assert_eq!(zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS), vmo.duration_from_frames(1));
        assert_eq!(zx::MonotonicDuration::from_nanos(TWO_FRAME_NANOS), vmo.duration_from_frames(2));
        assert_eq!(
            zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS),
            vmo.duration_from_frames(3)
        );

        assert_eq!(
            zx::MonotonicDuration::from_seconds(1),
            vmo.duration_from_frames(TEST_FPS as usize)
        );

        assert_eq!(zx::MonotonicDuration::from_millis(1500), vmo.duration_from_frames(72000));
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn frames_from_duration(vmo: FrameVmo) {
        assert_eq!(0, vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(0)));

        assert_eq!(
            0,
            vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS - 1))
        );
        assert_eq!(1, vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS)));

        // Three frames is an exact number of nanoseconds, testing the edge.
        assert_eq!(
            2,
            vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS - 1))
        );
        assert_eq!(
            3,
            vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS))
        );
        assert_eq!(
            3,
            vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS + 1))
        );

        assert_eq!(
            TEST_FPS as usize,
            vmo.frames_from_duration(zx::MonotonicDuration::from_seconds(1))
        );
        assert_eq!(72000, vmo.frames_from_duration(zx::MonotonicDuration::from_millis(1500)));

        assert_eq!(10660, vmo.frames_from_duration(zx::MonotonicDuration::from_nanos(222084000)));
    }

    #[fuchsia::test]
    fn start_stop() {
        let _exec = fasync::TestExecutor::new();

        let mut vmo = FrameVmo::new().expect("can't make a framevmo");

        // Starting before set_format is an error.
        assert!(vmo.start(fasync::MonotonicInstant::now()).is_err());
        // Stopping before set_format is an error.
        assert!(vmo.stop().is_err());

        let _ = vmo.set_format(TEST_FPS, TEST_FORMAT, TEST_CHANNELS).unwrap();
        let _handle = vmo.get_vmo(TEST_FRAMES, 0).unwrap();

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();
        match vmo.start(start_time) {
            Err(Error::InvalidState) => {}
            x => panic!("Expected Err(InvalidState) from double start but got {:?}", x),
        };

        assert_eq!(true, vmo.stop().unwrap());
        // stop is idempotent, but will return false if it was already stopped
        assert_eq!(false, vmo.stop().unwrap());

        vmo.start(start_time).unwrap();
    }

    fn frames_before_exact(
        vmo: &mut FrameVmo,
        time_nanos: i64,
        duration: zx::MonotonicDuration,
        frames: usize,
    ) {
        let _ = vmo.stop();
        let start_time = fasync::MonotonicInstant::from_nanos(time_nanos);
        vmo.start(start_time).unwrap();
        assert_eq!(frames, vmo.frames_before(start_time + duration));
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn frames_before(mut vmo: FrameVmo) {
        let _exec = fasync::TestExecutor::new();

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        assert_eq!(0, vmo.frames_before(start_time));

        assert_eq!(
            1,
            vmo.frames_before(start_time + zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS))
        );
        assert_eq!(
            2,
            vmo.frames_before(
                start_time + zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS - 1)
            )
        );
        assert_eq!(
            3,
            vmo.frames_before(start_time + zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS))
        );

        assert_eq!(
            TEST_FPS as usize / 4,
            vmo.frames_before(start_time + zx::MonotonicDuration::from_millis(250))
        );

        let three_quarters_dur = zx::MonotonicDuration::from_millis(375);
        assert_eq!(3 * TEST_FPS as usize / 8, vmo.frames_before(start_time + three_quarters_dur));

        assert_eq!(
            10521,
            vmo.frames_before(start_time + zx::MonotonicDuration::from_nanos(219188000))
        );

        frames_before_exact(
            &mut vmo,
            273533747037,
            zx::MonotonicDuration::from_nanos(219188000),
            10521,
        );
        frames_before_exact(
            &mut vmo,
            714329925362,
            zx::MonotonicDuration::from_nanos(219292000),
            10526,
        );
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn poll_read(mut vmo: FrameVmo) {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        let half_dur = TEST_VMO_DURATION / 2;
        const QUART_FRAMES: usize = TEST_FRAMES / 4;
        let mut quart_frames_buf = [0; QUART_FRAMES];
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur));

        let mut no_wake_cx = Context::from_waker(futures_test::task::panic_waker_ref());

        let res = vmo.poll_read(0, &mut quart_frames_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: frame_idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        // index returned should be equal to the number of frames returned minus 1 - zero is the
        // first frame
        assert_eq!(frame_idx, QUART_FRAMES);

        // After another half_dur, we should be able to read the whole buffer from start to finish.
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur));

        let mut full_buf = [0; TEST_FRAMES];
        let res = vmo.poll_read(0, &mut full_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: frame_idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        // index returned should be equal to the number of frames returned minus 1 (zero indexing)
        assert_eq!(frame_idx, TEST_FRAMES);

        // Each `half_dur` period should pseudo-fill half the vmo.
        // After three halves, we should have the oldest frame half-way through the buffer (at
        // index 12000)
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur));

        // Should be able to get frames that span from the middle of the buffer to the middle of the
        // buffer (from index 12000 to index 11999).  This should be 24000 frames.
        // TODO(https://fxbug.dev/42171752): should mark the buffer somehow to confirm that the data is correct
        let res = vmo.poll_read(TEST_FRAMES / 2, &mut full_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: frame_idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        assert_eq!(frame_idx, TEST_FRAMES + TEST_FRAMES / 2);

        // Should be able to get a set of frames that is all located before the oldest point in the
        // VMO, which is currently in the middle of the VMO.
        // This should be from about a quarter in to halfway in (now)
        // Should be able to get exactly the min_duration amount of frames.
        let res = vmo.poll_read(frame_idx - QUART_FRAMES, &mut quart_frames_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: frame_idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        assert_eq!(frame_idx, TEST_FRAMES + TEST_FRAMES / 2);
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn poll_read_waking(mut vmo: FrameVmo) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));

        let half_dur = TEST_VMO_DURATION / 2;
        const QUART_FRAMES: usize = TEST_FRAMES / 4;
        let mut quart_frames_buf = [0; QUART_FRAMES];
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur));

        let (waker, count) = futures_test::task::new_count_waker();
        let mut counting_wake_cx = Context::from_waker(&waker);

        // Polled before start, should wake the waker when started.
        let res = vmo.poll_read(0, &mut quart_frames_buf[..], &mut counting_wake_cx);
        res.expect_pending("should be pending before start");

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        // Woken when we start.
        assert_eq!(count, 1);

        // Polling before frames are ready should return pending, and set a timer.
        let res = vmo.poll_read(0, &mut quart_frames_buf[..], &mut counting_wake_cx);
        res.expect_pending("should be pending before start");

        // Each `half_dur` period should pseudo-fill half the vmo.
        // After a half_dur, we should have been woken.
        let new_time = fasync::MonotonicInstant::after(half_dur);
        exec.set_fake_time(new_time);
        assert!(exec.wake_expired_timers());

        assert_eq!(count, 2);

        // Should be ready now, with half the duration in frames available.
        let res = vmo.poll_read(0, &mut quart_frames_buf[..], &mut counting_wake_cx);
        let FrameReadResult::Frames { next_frame: frame_idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };
        assert_eq!(0, missed);
        assert_eq!(frame_idx, QUART_FRAMES);

        // Polling again with more frames than we have should return pending because there
        // isn't enough data available.
        let mut half_frames_buf = [0; TEST_FRAMES / 2];
        let res = vmo.poll_read(frame_idx, &mut half_frames_buf, &mut counting_wake_cx);
        res.expect_pending("should be pending because not enough data");

        assert_eq!(count, 2);
    }

    #[fuchsia::test]
    fn multibyte_poll_read() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));
        let mut vmo = FrameVmo::new().expect("can't make a framevmo");
        let format = AudioSampleFormat::Sixteen { unsigned: false, invert_endian: false };
        let frames = TEST_FPS as usize / 2;

        // No byte count can be determined before the format is set.
        assert!(vmo.bytecount_frames(10).is_none());

        let _ = vmo.set_format(TEST_FPS, format, 2).unwrap();
        let _handle = vmo.get_vmo(frames, 0).unwrap();

        let half_dur = zx::MonotonicDuration::from_millis(250);

        // Start in the past so we can be sure the frames are in the past.
        let start_time = fasync::MonotonicInstant::now() - half_dur;
        vmo.start(start_time).unwrap();

        let bytecount =
            vmo.bytecount_frames(frames / 2).expect("a bytecount should exist after format");
        let mut half_frames_buf = vec![0; bytecount];

        let mut no_wake_cx = Context::from_waker(futures_test::task::panic_waker_ref());
        let res = vmo.poll_read(0, half_frames_buf.as_mut_slice(), &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: idx, missed } =
            res.expect("frames should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        // Still frames are in frame counts, not byte counts.
        assert_eq!(idx, frames / 2);
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn poll_read_boundaries(mut vmo: FrameVmo) {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));
        let mut no_wake_cx = Context::from_waker(futures_test::task::panic_waker_ref());

        let start_time = fasync::MonotonicInstant::now();

        vmo.start(start_time).unwrap();

        // Just before the frame finishes, we shouldn't be able to get it.
        exec.set_fake_time(
            start_time + zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS)
                - zx::MonotonicDuration::from_nanos(1),
        );
        let mut one_frame_buf = [0; 1];
        vmo.poll_read(2, &mut one_frame_buf, &mut no_wake_cx)
            .expect_pending("third frame shouldn't be ready");
        // Exactly when the frame finishes, should be able to get the frame.
        exec.set_fake_time(start_time + zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS));
        let res = vmo.poll_read(2, &mut one_frame_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: idx, missed } =
            res.expect("third frame should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };

        assert_eq!(0, missed);
        // index is the index of the next frame to write, which should be the frame index we requested + 1
        assert_eq!(3, idx);

        // a bunch of time has passed, let's get one frame again.
        let much_later_ns = 3999 * THREE_FRAME_NANOS;
        let next_frame_idx = 3998 * 3 + 2;
        exec.set_fake_time(start_time + zx::MonotonicDuration::from_nanos(much_later_ns));
        let res = vmo.poll_read(next_frame_idx, &mut one_frame_buf, &mut no_wake_cx);
        let FrameReadResult::Frames { next_frame: idx, missed } =
            res.expect("frame should be ready").expect("no error")
        else {
            panic!("expected Frames");
        };
        assert_eq!(0, missed);
        // index is the index of the next frame
        assert_eq!(next_frame_idx + 1, idx);

        let mut all_frames_len = 0;
        let mut total_duration = zx::MonotonicDuration::from_nanos(0);

        let moment_length = zx::MonotonicDuration::from_nanos(10_000);

        let mut moment_start = start_time;
        let mut moment_end = moment_start;
        let mut next_index = 0;

        let mut ten_frames_buf = [0; 10];

        while total_duration < zx::MonotonicDuration::from_millis(250) {
            moment_end += moment_length;
            exec.set_fake_time(moment_end);
            total_duration += moment_length;
            // the cx here will never be woken since we never wake timers
            let Poll::Ready(res) = vmo.poll_read(next_index, &mut ten_frames_buf, &mut no_wake_cx)
            else {
                continue;
            };
            let FrameReadResult::Frames { next_frame: last_idx, missed } = res.expect("no error")
            else {
                panic!("expected Frames");
            };
            assert_eq!(0, missed);
            all_frames_len += ten_frames_buf.len();
            assert_eq!(
                all_frames_len,
                vmo.frames_before(moment_end + zx::MonotonicDuration::from_nanos(1)),
                "frame miscount after {:?} - {:?} moment",
                moment_start,
                moment_end
            );
            moment_start = moment_end;
            next_index = last_idx;
        }

        assert_eq!(TEST_FRAMES / 2, all_frames_len, "should be a quarter second worth of frames");
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn poll_write(mut vmo: FrameVmo) {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        let half_dur = TEST_VMO_DURATION / 2;
        const QUART_FRAMES: usize = TEST_FRAMES / 4;
        let quart_frames_buf = [0; QUART_FRAMES];

        let mut no_wake_cx = Context::from_waker(futures_test::task::panic_waker_ref());

        // Immediately after starting we should be able to write.
        let res = vmo.poll_write(0, &quart_frames_buf, &mut no_wake_cx);
        let (frame_idx, missed) = res.expect("frames should be writable").expect("no error");

        assert_eq!(0, missed);
        // index returned should be equal to the number of frames written - zero indexed
        // frame numbers.
        assert_eq!(frame_idx, QUART_FRAMES);

        // After a quarter_duration, we should be able to write to the whole buffer
        // TODO(https://fxbug.dev/42073419): this should only be the safe space and not the whole
        // buffer, there is a no-go-space just past the read pointer.
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur / 2));

        let full_buf = [0; TEST_FRAMES];
        let res = vmo.poll_write(frame_idx, &full_buf, &mut no_wake_cx);
        let (frame_idx, missed) = res.expect("frames should be writable").expect("no error");

        assert_eq!(0, missed);
        // index returned should be equal to the number of frames returned minus 1 (zero indexing)
        assert_eq!(frame_idx, TEST_FRAMES + QUART_FRAMES);

        // Each `half_dur` period should "read past" half the vmo.
        // After five quarters duration, the read pointer should be at FRAMES + QUART_FRAMES.
        // (vmo index 12000)
        exec.set_fake_time(fasync::MonotonicInstant::after(TEST_VMO_DURATION));

        // Should be able to write frames frames that span from the quarter of the buffer to the
        // read pointer again (from index 6000 to index 5999).  This should be 24000 frames.
        // TODO(https://fxbug.dev/42171752): should mark the buffer somehow to confirm that the data is correct
        let res = vmo.poll_write(frame_idx, &full_buf, &mut no_wake_cx);
        let (frame_idx, missed) = res.expect("frames should be ready").expect("no error");

        assert_eq!(0, missed);
        assert_eq!(frame_idx, TEST_FRAMES * 2 + QUART_FRAMES);

        // Should be able to write set of frames that is all located before the youngest point in the
        // VMO, which is currently a quarter way through the vmo.
        // This is from zero to a quarter in.
        let res = vmo.poll_write(TEST_FRAMES + TEST_FRAMES, &quart_frames_buf, &mut no_wake_cx);
        let (frame_idx, missed) = res.expect("frames should be writable").expect("no error");

        assert_eq!(0, missed);
        assert_eq!(frame_idx, TEST_FRAMES + TEST_FRAMES + QUART_FRAMES);
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn poll_write_waking(mut vmo: FrameVmo) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));

        let half_dur = TEST_VMO_DURATION / 2;
        const QUART_FRAMES: usize = TEST_FRAMES / 4;
        let quart_frames_buf = [0; QUART_FRAMES];
        exec.set_fake_time(fasync::MonotonicInstant::after(half_dur));

        let (waker, count) = futures_test::task::new_count_waker();
        let mut counting_wake_cx = Context::from_waker(&waker);

        // Polled before start, should wake the waker when started.
        let res = vmo.poll_write(0, &quart_frames_buf, &mut counting_wake_cx);
        res.expect_pending("should be pending before start");

        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        // Woken when we start.
        assert_eq!(count, 1);

        // Should be able to write a whole buffer right after start.
        // TODO(https://fxbug.dev/42073419): this should only be the safe space and not the whole
        // buffer, there is a no-go-space just past the read pointer.
        let mut idx = 0;
        for _ in 0..4 {
            let res = vmo.poll_write(idx, &quart_frames_buf, &mut counting_wake_cx);
            let (new_idx, missed) = res.expect("should be writable just after start").expect("ok");
            assert_eq!(0, missed);
            assert!(new_idx > idx);
            idx = new_idx;
        }

        // After a full buffer write, we should be pending
        let res = vmo.poll_write(idx, &quart_frames_buf, &mut counting_wake_cx);
        res.expect_pending("should have to wait for time to go by");

        // Each `half_dur` period should pseudo-read half the vmo.
        // After a half_dur, we should have been woken.
        let new_time = fasync::MonotonicInstant::after(half_dur);
        exec.set_fake_time(new_time);
        assert!(exec.wake_expired_timers());

        assert_eq!(count, 2);

        // Should be ready now, and be able to write twice.
        for _ in 0..2 {
            let res = vmo.poll_write(idx, &quart_frames_buf, &mut counting_wake_cx);
            let (new_idx, missed) = res.expect("frames writable").expect("ok");
            assert_eq!(0, missed);
            assert!(new_idx >= idx);
            idx = new_idx;
        }

        // Polling again with more frames should return pending because there
        // isn't enough data available, even if we start in the "past"
        let half_frames_buf = [0; TEST_FRAMES / 2];
        let res = vmo.poll_write(idx - QUART_FRAMES, &half_frames_buf, &mut counting_wake_cx);
        res.expect_pending("should be pending because not enough space");

        assert_eq!(count, 2);
    }

    #[fixture(with_test_vmo)]
    #[fuchsia::test]
    fn active_channels_poll_read_and_watch(mut vmo: FrameVmo) {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let start_time = fasync::MonotonicInstant::now();
        vmo.start(start_time).unwrap();

        let (active_waker, active_waker_count) = futures_test::task::new_count_waker();
        let mut active_cx = Context::from_waker(&active_waker);

        // Initial poll_active_channels returns Ready(true) because channels are active by default.
        let res = vmo.poll_active_channels(&mut active_cx);
        assert_eq!(res, Poll::Ready(true));

        // Polling again without changes returns Pending.
        let res = vmo.poll_active_channels(&mut active_cx);
        assert_eq!(res, Poll::Pending);

        let (read_waker, read_waker_count) = futures_test::task::new_count_waker();
        let mut read_cx = Context::from_waker(&read_waker);

        // Advance time so frames are available.
        exec.set_fake_time(fasync::MonotonicInstant::after(TEST_VMO_DURATION / 2));
        let mut buf = [0; TEST_FRAMES / 4];
        let res = vmo.poll_read(0, &mut buf, &mut read_cx);
        assert!(res.is_ready());

        // Disable active channels.
        assert_eq!(active_waker_count, 0);
        assert_eq!(read_waker_count, 0);
        let _ = vmo.set_active_channels(0).unwrap();

        // Active waker should be woken. Read waker was not registered (poll_read was ready).
        assert_eq!(active_waker_count, 1);
        assert_eq!(read_waker_count, 0);

        // poll_active_channels should now return Ready(false).
        let res = vmo.poll_active_channels(&mut active_cx);
        assert_eq!(res, Poll::Ready(false));
        // Next poll returns Pending.
        assert_eq!(vmo.poll_active_channels(&mut active_cx), Poll::Pending);

        // poll_read should now return Ready with AudioDisabled event.
        let res = vmo.poll_read(0, &mut buf, &mut read_cx);
        assert!(matches!(res, Poll::Ready(Ok(FrameReadResult::AudioDisabled))));

        // Subsequent poll_read should return Pending because channels are disabled and event was consumed.
        let res = vmo.poll_read(0, &mut buf, &mut read_cx);
        res.expect_pending("should be pending because channels are disabled");

        // Enable active channels (test format has 1 channel).
        let _ = vmo.set_active_channels(1).unwrap();
        // Both active waker and read waker should be woken.
        assert_eq!(active_waker_count, 2);
        assert_eq!(read_waker_count, 1);

        // poll_active_channels returns Ready(true).
        let res = vmo.poll_active_channels(&mut active_cx);
        assert_eq!(res, Poll::Ready(true));

        // poll_read should now succeed with Frames.
        let res = vmo.poll_read(0, &mut buf, &mut read_cx);
        assert!(matches!(res, Poll::Ready(Ok(FrameReadResult::Frames { .. }))));
    }
}
