// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::Write;
use std::sync::Arc;

use anyhow::{format_err, Error};
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::{FutureExt, TryStreamExt};
use log::trace;

use crate::util;

/// The InputWorker is an implementation of an input virtual audio device.
///
/// When registered with the audio system, this device replaces the normal microphone.
///
/// It supports injecting an audio stream over a virtual microphone
/// to simulate audio input to the device.
#[derive(Debug, Default)]
pub(crate) struct InputWorker {
    inj_data: Vec<u8>,
    injecting: Arc<util::Condition>,

    vmo: Option<zx::Vmo>,

    // How much of the vmo's data we're actually using, in bytes.
    work_space: usize,

    // How many frames ahead of the position we want to be writing to, when writing.
    target_frames: usize,

    // How often, in frames, we want to be updated on the state of the injection ring buffer.
    frames_per_notification: usize,

    // How many bytes a frame is.
    frame_size: usize,

    // Next write pointer as a byte number.
    // This is not an offset into vmo (it does not wrap around).
    write_pointer: usize,

    // Next ring buffer pointer as a byte number.
    // This is not an offset into vmo (it does not wrap around).
    ring_pointer: usize,

    // Last relative ring buffer byte offset.
    // This is an offset into vmo (it wraps around).
    last_ring_offset: usize,
}

impl InputWorker {
    fn write_to_vmo(&self, data: &[u8]) -> Result<(), Error> {
        let vmo = if let Some(vmo) = &self.vmo { vmo } else { return Ok(()) };
        let start = self.write_pointer % self.work_space;
        let end = (self.write_pointer + data.len()) % self.work_space;
        let start_u64 = start.try_into()?;

        // Write in two chunks if we've wrapped around.
        if end < start {
            let split = self.work_space - start;
            vmo.write(&data[0..split], start_u64)?;
            vmo.write(&data[split..], 0)?;
        } else {
            vmo.write(&data, start_u64)?;
        }
        Ok(())
    }

    /// Clears the injected audio data.
    pub(crate) fn clear_injection_data(&mut self) {
        self.inj_data.clear();
        self.injecting.set(false);
    }

    /// Append data to the injected audio stream.
    ///
    /// The input data must be a WAV file with a 44 byte header.
    /// The header will be discarded, and it is assumed that the
    /// input data matches the channel and bitrate configuration this
    /// worker was started with.
    pub(crate) fn append_injection_data(&mut self, data: Vec<u8>) -> Result<(), Error> {
        if self.inj_data.len() > 0 {
            return Err(format_err!("Cannot inject new audio without flushing old audio"));
        }
        // This is a bad assumption, wav headers can be many different sizes.
        // 8 Bytes for riff header
        // 28 bytes for wave fmt block
        // 8 bytes for data header
        self.inj_data.write_all(&data[44..])?;
        self.injecting.set(true);
        trace!("AudioFacade::InputWorker: Injecting {:?} bytes", self.inj_data.len());
        Ok(())
    }

    /// Return a future that resolves when injection is finished.
    ///
    /// The future returns immediately if injection is not started.
    pub fn get_injection_end_future(&self) -> BoxFuture<'static, ()> {
        self.injecting.wait(false).boxed()
    }

    // Events from the Virtual Audio Device
    fn on_set_format(
        &mut self,
        frames_per_second: u32,
        sample_format: u32,
        num_channels: u32,
        _external_delay: i64,
    ) -> Result<(), Error> {
        let sample_size = crate::util::get_sample_size(sample_format)?;
        self.frame_size = usize::try_from(num_channels)? * usize::try_from(sample_size)?;

        let frames_per_millisecond = frames_per_second / 1000;

        trace!(
            "AudioFacade::InputWorker: configuring with {:?} fps, {:?} bpf",
            frames_per_second,
            self.frame_size
        );

        // We get notified every 50ms and write up to 250ms worth of data in the future.
        // The gap between frames_per_notification and target_frames gives us slack to
        // account for scheduling delays. Audio injection proceeds as follows:
        //
        //   1. When the buffer is created, the initial ring buffer position is "0ms".
        //      At that time, the ring buffer is zeroed and we pretend to write target_frames
        //      worth of silence to the ring buffer. This puts our write pointer ~250ms
        //      ahead of the ring buffer's safe read pointer.
        //
        //   2. We receive the first notification at time 50ms + scheduling delay. At this
        //      point we write data for the range 250ms - 300ms. As long as our scheduling
        //      delay is < 250ms, our writes will stay ahead of the ring buffer's safe read
        //      pointer. We assume that 250ms is more than enough time (this test framework
        //      should never be run in a debug or sanitizer build).
        //
        //   3. This continues ad infinitum.
        //
        // The sum of 250ms + 50ms must fit within the ring buffer. We currently use a 1s
        // ring buffer: see VirtualInput.start_input.
        //
        self.target_frames = (250 * frames_per_millisecond).try_into()?;
        self.frames_per_notification = (50 * frames_per_millisecond).try_into()?;
        Ok(())
    }

    async fn create_buffer_and_get_notification_frequency(
        &mut self,
        ring_buffer: zx::Vmo,
        num_ring_buffer_frames: u32,
        _notifications_per_ring: u32,
    ) -> Result<u32, Error> {
        // Ignore AudioCore's notification cadence (_notifications_per_ring); set up our own.
        let target_notifications_per_ring =
            num_ring_buffer_frames / u32::try_from(self.frames_per_notification)?;

        // The buffer starts zeroed and our write pointer starts target_frames in the future.
        self.work_space = usize::try_from(num_ring_buffer_frames)? * self.frame_size;
        self.write_pointer = self.target_frames * self.frame_size;
        self.last_ring_offset = 0;
        self.vmo = Some(ring_buffer);

        trace!(
            "AudioFacade::InputWorker: created buffer with {:?} frames, {:?} notifications, {:?} target frames per write",
            num_ring_buffer_frames,
            target_notifications_per_ring,
            self.target_frames
        );
        Ok(target_notifications_per_ring)
    }

    fn on_position_notify(&mut self, _monotonic_time: i64, ring_offset: u32) -> Result<(), Error> {
        let ring_offset = ring_offset.try_into()?;
        if ring_offset < self.last_ring_offset {
            self.ring_pointer += self.work_space - self.last_ring_offset + ring_offset;
        } else {
            self.ring_pointer += ring_offset - self.last_ring_offset;
        };

        let next_write_pointer = self.ring_pointer + self.target_frames * self.frame_size;
        let bytes_to_write = next_write_pointer - self.write_pointer;

        // Next segment of inj_data.
        if self.inj_data.len() > 0 {
            let data_end = std::cmp::min(self.inj_data.len(), bytes_to_write);
            trace!("Injecting {data_end} bytes, {} remaining", self.inj_data.len());
            self.write_to_vmo(&self.inj_data[0..data_end])?;
            self.inj_data.drain(0..data_end);
            self.write_pointer += data_end;
        }

        // Pad with zeroes.
        if self.write_pointer < next_write_pointer {
            let zeroes = vec![0; next_write_pointer - self.write_pointer];
            self.write_to_vmo(&zeroes)?;
            self.write_pointer = next_write_pointer;
        }

        if self.inj_data.is_empty() {
            if self.injecting.set(false) {
                trace!("Signalled injection completion");
            }
        }

        self.last_ring_offset = ring_offset;
        Ok(())
    }

    /// Asynchronously run the input audio device.
    pub(crate) async fn run(
        worker: Arc<Mutex<InputWorker>>,
        va_input: fidl_fuchsia_virtualaudio::DeviceProxy,
    ) -> Result<(), Error> {
        let mut input_events = va_input.take_event_stream();

        // Monotonic timestamp returned by the most-recent OnStart/OnPositionNotify/OnStop response.
        let mut last_timestamp = zx::MonotonicInstant::from_nanos(0);
        // Observed monotonic time that OnStart/OnPositionNotify/OnStop messages actually arrived.
        let mut last_event_time = zx::MonotonicInstant::from_nanos(0);

        while let Some(input_msg) = input_events.try_next().await? {
            match input_msg {
                fidl_fuchsia_virtualaudio::DeviceEvent::OnSetFormat {
                    frames_per_second,
                    sample_format,
                    num_channels,
                    external_delay,
                } => {
                    worker.lock().await.on_set_format(
                        frames_per_second,
                        sample_format,
                        num_channels,
                        external_delay,
                    )?;
                }
                fidl_fuchsia_virtualaudio::DeviceEvent::OnBufferCreated {
                    ring_buffer,
                    num_ring_buffer_frames,
                    notifications_per_ring,
                } => {
                    let target_notifications_per_ring = worker
                        .lock()
                        .await
                        .create_buffer_and_get_notification_frequency(
                            ring_buffer,
                            num_ring_buffer_frames,
                            notifications_per_ring,
                        )
                        .await?;
                    va_input
                        .set_notification_frequency(target_notifications_per_ring)
                        .await?
                        .map_err(|status| {
                            format_err!("SetNotificationFrequency returned error {:?}", status)
                        })?;
                }
                fidl_fuchsia_virtualaudio::DeviceEvent::OnStart { start_time } => {
                    if last_timestamp > zx::MonotonicInstant::from_nanos(0) {
                        trace!("AudioFacade::InputWorker: Injection OnPositionNotify received before OnStart");
                    }
                    last_timestamp = zx::MonotonicInstant::from_nanos(start_time);
                    last_event_time = zx::MonotonicInstant::get();
                }
                fidl_fuchsia_virtualaudio::DeviceEvent::OnStop {
                    stop_time: _,
                    ring_position: _,
                } => {
                    if last_timestamp == zx::MonotonicInstant::from_nanos(0) {
                        trace!("AudioFacade::InputWorker: Injection OnPositionNotify timestamp cleared before OnStop");
                    }
                    last_timestamp = zx::MonotonicInstant::from_nanos(0);
                    last_event_time = zx::MonotonicInstant::from_nanos(0);
                }
                fidl_fuchsia_virtualaudio::DeviceEvent::OnPositionNotify {
                    monotonic_time,
                    ring_position,
                } => {
                    let monotonic_zx_time = zx::MonotonicInstant::from_nanos(monotonic_time);
                    let now = zx::MonotonicInstant::get();

                    let mut worker = worker.lock().await;

                    // To minimize logspam, log glitches only when writing audio.
                    if worker.inj_data.len() > 0 {
                        if last_timestamp == zx::MonotonicInstant::from_nanos(0) {
                            trace!("AudioFacade::InputWorker: Injection OnStart not received before OnPositionNotify");
                        }

                        // Log if our timestamps had a gap of more than 100ms. This is highly
                        // abnormal and indicates possible glitching while receiving audio to
                        // be injected and/or providing it to the system.
                        let timestamp_interval = monotonic_zx_time - last_timestamp;

                        if timestamp_interval > zx::MonotonicDuration::from_millis(100) {
                            trace!("AudioFacade::InputWorker: Injection position timestamp jumped by more than 100ms ({:?}ms). Expect glitches.",
                                        timestamp_interval.into_millis());
                        }
                        if monotonic_zx_time < last_timestamp {
                            trace!("AudioFacade::InputWorker: Injection position timestamp moved backwards ({:?}ms). Expect glitches.",
                                        timestamp_interval.into_millis());
                        }

                        // Log if there was a gap in position notification arrivals of more
                        // than 150ms. This is highly abnormal and indicates possible glitching
                        // while receiving audio to be injected and/or providing it to the
                        // system.
                        let observed_interval = now - last_event_time;

                        if observed_interval > zx::MonotonicDuration::from_millis(150) {
                            trace!("AudioFacade::InputWorker: Injection position not updated for 150ms ({:?}ms). Expect glitches.",
                                        observed_interval.into_millis());
                        }
                    }

                    last_timestamp = monotonic_zx_time;
                    last_event_time = now;
                    worker.on_position_notify(monotonic_time, ring_position)?;
                }
                evt => {
                    trace!("Ignored event {evt:?}");
                }
            }
        }
        Ok(())
    }
}
