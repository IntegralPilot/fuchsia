// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use async_helpers::maybe_stream::MaybeStream;
use fidl::endpoints::ClientEnd;
use fidl::prelude::*;
use fidl_fuchsia_hardware_audio::*;
use fuchsia_inspect_derive::{IValue, Inspect};
use fuchsia_sync::Mutex;

use fuchsia_async as fasync;
use fuchsia_inspect as inspect;
use futures::{StreamExt, select};
use log::{info, warn};
use std::sync::Arc;

use crate::audio_frame_sink::AudioFrameSink;
use crate::audio_frame_stream::AudioFrameStream;
use crate::frame_vmo;
use crate::types::{AudioSampleFormat, Error, Result};

pub(crate) enum StreamConfigOrTask {
    StreamConfig(Box<SoftStreamConfig>),
    Task(fasync::Task<Result<()>>),
    Complete,
}

impl StreamConfigOrTask {
    /// Start the task if it's not running.
    /// Does nothing if the task is running or completed.
    pub(crate) fn start(&mut self) {
        *self = match std::mem::replace(self, StreamConfigOrTask::Complete) {
            StreamConfigOrTask::StreamConfig(st) => {
                StreamConfigOrTask::Task(fasync::Task::spawn(st.process_requests()))
            }
            x => x,
        }
    }
}

/// Number of frames within the duration.  This includes frames that end at exactly the duration.
pub(crate) fn frames_from_duration(
    frames_per_second: usize,
    duration: fasync::MonotonicDuration,
) -> usize {
    assert!(
        duration >= zx::MonotonicDuration::from_nanos(0),
        "frames_from_duration is not defined for negative durations"
    );
    let mut frames = duration.into_seconds() * frames_per_second as i64;
    let frames_partial =
        ((duration.into_nanos() % 1_000_000_000) as f64 / 1e9) * frames_per_second as f64;
    frames += frames_partial as i64;
    frames as usize
}

/// A software fuchsia audio output, which implements Audio Driver Streaming Interface
/// as defined in //docs/concepts/drivers/driver_interfaces/audio_streaming.md
#[derive(Inspect)]
pub struct SoftStreamConfig {
    /// The Stream channel handles format negotiation, plug detection, and gain
    stream_config_stream: StreamConfigRequestStream,

    /// The Unique ID that this stream will present to the system
    unique_id: [u8; 16],
    /// The manufacturer of the hardware for this stream
    manufacturer: String,
    /// A product description for the hardware for the stream
    product: String,
    /// The clock domain that this stream will present to the system
    clock_domain: u32,

    /// True when this represents an output
    is_output: bool,

    /// The supported format of this output.
    /// Currently only support one format per output is supported.
    supported_formats: PcmSupportedFormats,

    /// The number of audio frames per packet from the frame stream.
    /// Used to calculate audio buffer sizes.
    /// If an input, this is the amount of space we reserve for audio frames.
    packet_frames: usize,

    /// The size of a frame.
    /// Used to report the driver transfer size.
    frame_bytes: usize,

    /// The request stream for the ringbuffer.
    ring_buffer_stream: MaybeStream<RingBufferRequestStream>,

    /// A pointer to the ring buffer for this stream
    frame_vmo: Arc<Mutex<frame_vmo::FrameVmo>>,

    /// The current delay that has been communicated exists after the audio is retrieved.
    external_delay: zx::MonotonicDuration,

    /// Replied to plugged state watch.
    plug_state_replied: bool,

    /// Replied to gain state watch.
    gain_state_replied: bool,

    /// Replied to delay info watch.
    delay_info_replied: bool,

    /// Inspect node
    #[inspect(forward)]
    inspect: SoftStreamConfigInspect,
}

#[derive(Default, Inspect)]
struct SoftStreamConfigInspect {
    inspect_node: inspect::Node,
    ring_buffer_format: IValue<Option<String>>,
    frame_vmo_status: IValue<Option<String>>,
}

impl SoftStreamConfigInspect {
    fn record_current_format(&mut self, current: &(u32, AudioSampleFormat, u16)) {
        self.ring_buffer_format
            .iset(Some(format!("{} rate: {} channels: {}", current.1, current.0, current.2)));
    }

    fn record_vmo_status(&mut self, new: &str) {
        self.frame_vmo_status.iset(Some(new.to_owned()));
    }
}

impl SoftStreamConfig {
    /// Create a new software audio device, returning a client channel which can be supplied
    /// to the AudioCore and will act correctly as an audio output driver channel which can
    /// render audio in the `pcm_format` format, and an AudioFrameStream which produces the
    /// audio frames delivered to the audio output.
    /// Spawns a task to handle messages from the Audio Core and setup of internal VMO buffers
    /// required for audio output.  See AudioFrameStream for more information on timing
    /// requirements for audio output.
    /// `packet_duration`: desired duration of an audio packet returned by the stream. Rounded down to
    /// end on a audio frame boundary.
    /// `initial_external_delay`: delay that is added after packets have been returned from the stream
    pub fn create_output(
        unique_id: &[u8; 16],
        manufacturer: &str,
        product: &str,
        clock_domain: u32,
        pcm_format: fidl_fuchsia_media::PcmFormat,
        packet_duration: zx::MonotonicDuration,
        initial_external_delay: zx::MonotonicDuration,
    ) -> Result<(ClientEnd<StreamConfigMarker>, AudioFrameStream)> {
        let (client, soft_stream_config) = SoftStreamConfig::build(
            unique_id,
            manufacturer,
            product,
            clock_domain,
            true,
            pcm_format,
            packet_duration,
            initial_external_delay,
        )?;
        Ok((client, AudioFrameStream::new(soft_stream_config)))
    }

    pub fn create_input(
        unique_id: &[u8; 16],
        manufacturer: &str,
        product: &str,
        clock_domain: u32,
        pcm_format: fidl_fuchsia_media::PcmFormat,
        buffer: zx::MonotonicDuration,
    ) -> Result<(ClientEnd<StreamConfigMarker>, AudioFrameSink)> {
        let (client, soft_stream_config) = SoftStreamConfig::build(
            unique_id,
            manufacturer,
            product,
            clock_domain,
            false,
            pcm_format,
            buffer,
            zx::MonotonicDuration::from_nanos(0),
        )?;
        Ok((client, AudioFrameSink::new(soft_stream_config)))
    }

    fn build(
        unique_id: &[u8; 16],
        manufacturer: &str,
        product: &str,
        clock_domain: u32,
        is_output: bool,
        pcm_format: fidl_fuchsia_media::PcmFormat,
        packet_duration: zx::MonotonicDuration,
        initial_external_delay: zx::MonotonicDuration,
    ) -> Result<(ClientEnd<StreamConfigMarker>, SoftStreamConfig)> {
        if pcm_format.bits_per_sample % 8 != 0 {
            // Non-byte-aligned format not allowed.
            return Err(Error::InvalidArgs);
        }
        let (client, request_stream) =
            fidl::endpoints::create_request_stream::<StreamConfigMarker>();

        let number_of_channels = pcm_format.channel_map.len();
        let attributes = vec![ChannelAttributes::default(); number_of_channels];
        let channel_set = ChannelSet { attributes: Some(attributes), ..Default::default() };
        let supported_formats = PcmSupportedFormats {
            channel_sets: Some(vec![channel_set]),
            sample_formats: Some(vec![SampleFormat::PcmSigned]),
            bytes_per_sample: Some(vec![(pcm_format.bits_per_sample / 8) as u8]),
            valid_bits_per_sample: Some(vec![pcm_format.bits_per_sample as u8]),
            frame_rates: Some(vec![pcm_format.frames_per_second]),
            ..Default::default()
        };

        let packet_frames =
            frames_from_duration(pcm_format.frames_per_second as usize, packet_duration);

        let soft_stream_config = SoftStreamConfig {
            stream_config_stream: request_stream,
            unique_id: unique_id.clone(),
            manufacturer: manufacturer.to_string(),
            product: product.to_string(),
            is_output,
            clock_domain,
            supported_formats,
            packet_frames,
            frame_bytes: (pcm_format.bits_per_sample / 8) as usize,
            ring_buffer_stream: Default::default(),
            frame_vmo: Arc::new(Mutex::new(frame_vmo::FrameVmo::new()?)),
            external_delay: initial_external_delay,
            plug_state_replied: false,
            gain_state_replied: false,
            delay_info_replied: false,
            inspect: Default::default(),
        };
        Ok((client, soft_stream_config))
    }

    pub(crate) fn frame_vmo(&self) -> Arc<Mutex<frame_vmo::FrameVmo>> {
        self.frame_vmo.clone()
    }

    pub(crate) fn packet_frames(&self) -> usize {
        self.packet_frames
    }

    fn frames_per_second(&self) -> u32 {
        *self.supported_formats.frame_rates.as_ref().unwrap().get(0).unwrap()
    }

    /// Delay that is reported to the audio subsystem.
    /// Includes the buffered packets if this is an output, and the current external delay.
    fn current_delay(&self) -> zx::MonotonicDuration {
        let packet_delay_nanos = if self.is_output {
            (i64::try_from(self.packet_frames).unwrap() * 1_000_000_000)
                / self.frames_per_second() as i64
        } else {
            0
        };
        zx::MonotonicDuration::from_nanos(packet_delay_nanos) + self.external_delay
    }

    async fn process_requests(mut self) -> Result<()> {
        loop {
            select! {
                stream_config_request = self.stream_config_stream.next() => {
                    match stream_config_request {
                        Some(Ok(r)) => {
                            if let Err(e) = self.handle_stream_request(r) {
                                warn!(e:?; "stream config request")
                            }
                        },
                        Some(Err(e)) => {
                            warn!(e:?; "stream config error, stopping");
                            return Err(e.into());
                        },
                        None => {
                            warn!("stream config disconnected, stopping");
                            return Ok(());
                        },
                    }
                }
                ring_buffer_request = self.ring_buffer_stream.next() => {
                    match ring_buffer_request {
                        Some(Ok(r)) => {
                            if let Err(e) = self.handle_ring_buffer_request(r) {
                                warn!(e:?; "ring buffer request")
                            }
                        },
                        Some(Err(e)) => {
                            warn!(e:?; "ring buffer error, dropping stream");
                            let _ = MaybeStream::take(&mut self.ring_buffer_stream);
                        },
                        None => {
                            warn!("ring buffer finished, dropping");
                            let _ = MaybeStream::take(&mut self.ring_buffer_stream);
                        },
                    }
                }
            }
        }
    }

    fn handle_stream_request(
        &mut self,
        request: StreamConfigRequest,
    ) -> std::result::Result<(), anyhow::Error> {
        match request {
            StreamConfigRequest::GetHealthState { responder } => {
                responder.send(&HealthState::default())?;
            }
            StreamConfigRequest::SignalProcessingConnect { protocol, control_handle: _ } => {
                let _ = protocol.close_with_epitaph(zx::Status::NOT_SUPPORTED);
            }
            StreamConfigRequest::GetProperties { responder } => {
                #[rustfmt::skip]
                let prop = StreamProperties {
                    unique_id:                Some(self.unique_id),
                    is_input:                 Some(!self.is_output),
                    can_mute:                 Some(false),
                    can_agc:                  Some(false),
                    min_gain_db:              Some(0f32),
                    max_gain_db:              Some(0f32),
                    gain_step_db:             Some(0f32),
                    plug_detect_capabilities: Some(PlugDetectCapabilities::Hardwired),
                    clock_domain:             Some(self.clock_domain),
                    manufacturer:             Some(self.manufacturer.to_string()),
                    product:                  Some(self.product.to_string()),
                    ..Default::default()
                };
                responder.send(&prop)?;
            }
            StreamConfigRequest::GetSupportedFormats { responder } => {
                let pcm_formats = self.supported_formats.clone();
                let formats_vector = &[SupportedFormats {
                    pcm_supported_formats: Some(pcm_formats),
                    ..Default::default()
                }];
                responder.send(formats_vector)?;
            }
            StreamConfigRequest::CreateRingBuffer { format, ring_buffer, control_handle: _ } => {
                let pcm = format.pcm_format.ok_or_else(|| format_err!("No pcm_format included"))?;
                // If the ring buffer was previously active, we must shut it down.
                let _ = self.frame_vmo.lock().stop();
                drop(MaybeStream::take(&mut self.ring_buffer_stream));
                let current = (pcm.frame_rate, pcm.into(), pcm.number_of_channels.into());
                self.inspect.record_current_format(&current);
                if let Err(e) = self.frame_vmo.lock().set_format(current.0, current.1, current.2) {
                    info!("Error creating ring buffer: {e:?}");
                    let _ = ring_buffer.close_with_epitaph(zx::Status::INVALID_ARGS);
                    return Ok(());
                }
                self.ring_buffer_stream.set(ring_buffer.into_stream());
                self.delay_info_replied = false;
            }
            StreamConfigRequest::WatchGainState { responder } => {
                if self.gain_state_replied {
                    // We will never change gain state.
                    responder.drop_without_shutdown();
                    return Ok(());
                }
                let gain_state = GainState {
                    muted: Some(false),
                    agc_enabled: Some(false),
                    gain_db: Some(0.0f32),
                    ..Default::default()
                };
                responder.send(&gain_state)?;
                self.gain_state_replied = true
            }
            StreamConfigRequest::WatchPlugState { responder } => {
                if self.plug_state_replied {
                    // We will never change plug state.
                    responder.drop_without_shutdown();
                    return Ok(());
                }
                let time = fasync::MonotonicInstant::now();
                let plug_state = PlugState {
                    plugged: Some(true),
                    plug_state_time: Some(time.into_nanos() as i64),
                    ..Default::default()
                };
                responder.send(&plug_state)?;
                self.plug_state_replied = true;
            }
            StreamConfigRequest::SetGain { target_state, control_handle: _ } => {
                if let Some(true) = target_state.muted {
                    warn!("Mute is not supported");
                }
                if let Some(true) = target_state.agc_enabled {
                    warn!("AGC is not supported");
                }
                if let Some(gain) = target_state.gain_db {
                    if gain != 0.0 {
                        warn!("Non-zero gain setting not supported");
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_ring_buffer_request(
        &mut self,
        request: RingBufferRequest,
    ) -> std::result::Result<(), anyhow::Error> {
        match request {
            RingBufferRequest::GetProperties { responder } => {
                let prop = RingBufferProperties {
                    needs_cache_flush_or_invalidate: Some(false),
                    // TODO(https://fxbug.dev/42074396): Make driver_transfer_bytes (output) more accurate.
                    driver_transfer_bytes: Some((self.packet_frames * self.frame_bytes) as u32),
                    ..Default::default()
                };
                responder.send(&prop)?;
            }
            RingBufferRequest::GetVmo {
                min_frames,
                clock_recovery_notifications_per_ring,
                responder,
            } => {
                // Require a minimum amount of frames for three packets.
                let min_frames_from_duration = 3 * self.packet_frames as u32;
                let ring_buffer_frames =
                    (min_frames + self.packet_frames as u32).max(min_frames_from_duration);
                self.inspect.record_vmo_status("gotten");
                match self
                    .frame_vmo
                    .lock()
                    .get_vmo(ring_buffer_frames as usize, clock_recovery_notifications_per_ring)
                {
                    Err(e) => {
                        warn!(e:?; "Error on vmo set format");
                        responder.send(Err(GetVmoError::InternalError))?;
                    }
                    Ok(vmo_handle) => {
                        responder.send(Ok((ring_buffer_frames, vmo_handle)))?;
                    }
                }
            }
            RingBufferRequest::Start { responder } => {
                let time = fasync::MonotonicInstant::now();
                self.inspect.record_vmo_status(&format!("started @ {time:?}"));
                match self.frame_vmo.lock().start(time.into()) {
                    Ok(()) => responder.send(time.into_nanos() as i64)?,
                    Err(e) => {
                        warn!(e:?; "Error on frame vmo start");
                        responder.control_handle().shutdown_with_epitaph(zx::Status::BAD_STATE);
                    }
                }
            }
            RingBufferRequest::Stop { responder } => match self.frame_vmo.lock().stop() {
                Ok(stopped) => {
                    if !stopped {
                        info!("Stopping an unstarted ring buffer");
                    }
                    self.inspect.record_vmo_status(&format!(
                        "stopped @ {:?}",
                        fasync::MonotonicInstant::now()
                    ));
                    responder.send()?;
                }
                Err(e) => {
                    warn!(e:?; "Error on frame vmo stop");
                    responder.control_handle().shutdown_with_epitaph(zx::Status::BAD_STATE);
                }
            },
            RingBufferRequest::WatchClockRecoveryPositionInfo { responder } => {
                self.frame_vmo.lock().set_position_responder(responder);
            }
            RingBufferRequest::SetActiveChannels { active_channels_bitmask, responder } => {
                match self.frame_vmo.lock().set_active_channels(active_channels_bitmask) {
                    Ok(time) => responder.send(Ok(time.into_nanos()))?,
                    Err(e) => responder.send(Err(Into::<zx::Status>::into(e).into_raw()))?,
                }
            }
            RingBufferRequest::WatchDelayInfo { responder } => {
                if self.delay_info_replied {
                    // We will never change delay state.
                    // TODO(https://fxbug.dev/42128949): Reply again when the external_delay changes from
                    // outside instead of just on startup.
                    responder.drop_without_shutdown();
                    return Ok(());
                }
                // internal_delay is at least our packet duration (we buffer at least that much)
                // plus whatever delay has been communicated from the client.
                let delay_info = DelayInfo {
                    internal_delay: Some(self.current_delay().into_nanos()),
                    ..Default::default()
                };
                responder.send(&delay_info)?;
                self.delay_info_replied = true;
            }
            RingBufferRequest::_UnknownMethod { .. } => (),
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use fidl_fuchsia_media::{AudioChannelId, AudioPcmMode, PcmFormat};

    use async_utils::PollExt;
    use fixture::fixture;
    use futures::future;
    use futures::task::Poll;

    const TEST_UNIQUE_ID: &[u8; 16] = &[5; 16];
    const TEST_CLOCK_DOMAIN: u32 = 0x00010203;

    pub(crate) fn with_audio_frame_stream<F>(_name: &str, test: F)
    where
        F: FnOnce(fasync::TestExecutor, StreamConfigProxy, AudioFrameStream) -> (),
    {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let format = PcmFormat {
            pcm_mode: AudioPcmMode::Linear,
            bits_per_sample: 16,
            frames_per_second: 44100,
            channel_map: vec![AudioChannelId::Lf, AudioChannelId::Rf],
        };
        let (client, frame_stream) = SoftStreamConfig::create_output(
            TEST_UNIQUE_ID,
            "Google",
            "UnitTest",
            TEST_CLOCK_DOMAIN,
            format,
            zx::MonotonicDuration::from_millis(100),
            zx::MonotonicDuration::from_millis(50),
        )
        .expect("should always build");
        test(exec, client.into_proxy(), frame_stream)
    }

    #[fuchsia::test]
    fn test_frames_from_duration() {
        const FPS: usize = 48000;
        // At 48kHz, each frame is 20833 and 1/3 nanoseconds. We add one nanosecond
        // because frames need to be completely within the duration.
        const ONE_FRAME_NANOS: i64 = 20833 + 1;
        const THREE_FRAME_NANOS: i64 = 20833 * 3 + 1;

        assert_eq!(0, frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(0)));

        assert_eq!(
            0,
            frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS - 1))
        );
        assert_eq!(
            1,
            frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(ONE_FRAME_NANOS))
        );

        // Three frames is an exact number of nanoseconds, we should be able to get an exact number
        // of frames from the duration.
        assert_eq!(
            2,
            frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS - 1))
        );
        assert_eq!(
            3,
            frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS))
        );
        assert_eq!(
            3,
            frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(THREE_FRAME_NANOS + 1))
        );

        assert_eq!(FPS, frames_from_duration(FPS, zx::MonotonicDuration::from_seconds(1)));
        assert_eq!(72000, frames_from_duration(FPS, zx::MonotonicDuration::from_millis(1500)));

        assert_eq!(10660, frames_from_duration(FPS, zx::MonotonicDuration::from_nanos(222084000)));
    }

    #[fuchsia::test]
    fn soft_stream_config_audio_should_end_when_stream_dropped() {
        let format = PcmFormat {
            pcm_mode: AudioPcmMode::Linear,
            bits_per_sample: 16,
            frames_per_second: 48000,
            channel_map: vec![AudioChannelId::Lf, AudioChannelId::Rf],
        };

        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (client, frame_stream) = SoftStreamConfig::build(
            TEST_UNIQUE_ID,
            &"Google".to_string(),
            &"UnitTest".to_string(),
            TEST_CLOCK_DOMAIN,
            true,
            format,
            zx::MonotonicDuration::from_millis(100),
            zx::MonotonicDuration::from_millis(50),
        )
        .expect("should always build");

        drop(frame_stream);

        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future::pending::<()>()));

        // The audio client should be dropped (normally this causes audio to remove the device)
        assert_eq!(Err(zx::Status::PEER_CLOSED), client.channel().write(&[0], &mut Vec::new()));
    }

    // Returns the number of frames that were ready in the stream, draining the stream.
    fn frames_ready(exec: &mut fasync::TestExecutor, frame_stream: &mut AudioFrameStream) -> usize {
        let mut frames = 0;
        while exec.run_until_stalled(&mut frame_stream.next()).is_ready() {
            frames += 1;
        }
        frames
    }

    #[fixture(with_audio_frame_stream)]
    #[fuchsia::test]
    fn send_positions(
        mut exec: fasync::TestExecutor,
        stream_config: StreamConfigProxy,
        mut frame_stream: AudioFrameStream,
    ) {
        // Poll the frame stream, which should start the processing of proxy requests.
        assert_eq!(0, frames_ready(&mut exec, &mut frame_stream));
        let _stream_config_properties = exec.run_until_stalled(&mut stream_config.get_properties());
        let _formats = exec.run_until_stalled(&mut stream_config.get_supported_formats());
        let (ring_buffer, server) = fidl::endpoints::create_proxy::<RingBufferMarker>();

        #[rustfmt::skip]
        let format = Format {
            pcm_format: Some(fidl_fuchsia_hardware_audio::PcmFormat {
                number_of_channels:      2u8,
                sample_format:           SampleFormat::PcmSigned,
                bytes_per_sample:        2u8,
                valid_bits_per_sample:   16u8,
                frame_rate:              44100,
            }),
            ..Default::default()
        };

        let result = stream_config.create_ring_buffer(&format, server);
        assert!(result.is_ok());

        let _ring_buffer_properties = exec.run_until_stalled(&mut ring_buffer.get_properties());

        let some_active_channels_mask = 0xc3u64;
        let result =
            exec.run_until_stalled(&mut ring_buffer.set_active_channels(some_active_channels_mask));
        assert!(result.is_ready());
        let _ = match result {
            Poll::Ready(Ok(Err(e))) => assert_eq!(e, zx::Status::INVALID_ARGS.into_raw()),
            x => panic!("Expected error reply to set_active_channels, got {:?}", x),
        };

        let clock_recovery_notifications_per_ring = 10u32;
        let _ = exec.run_until_stalled(
            &mut ring_buffer.get_vmo(88200, clock_recovery_notifications_per_ring),
        ); // 2 seconds.

        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(42));
        let _ = exec.wake_expired_timers();
        let start_time = exec.run_until_stalled(&mut ring_buffer.start());
        if let Poll::Ready(s) = start_time {
            assert_eq!(s.expect("start time error"), 42);
        } else {
            panic!("start error");
        }

        // Watch number 1.
        let mut position_info = ring_buffer.watch_clock_recovery_position_info();
        let result = exec.run_until_stalled(&mut position_info);
        assert!(!result.is_ready());

        // Now advance in between notifications, with a 2 seconds total in the ring buffer
        // and 10 notifications per ring we can get watch notifications every 200 msecs.
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
            201,
        )));
        let _ = exec.wake_expired_timers();
        // Each frame is 100ms, there should be two of them ready now.
        assert_eq!(2, frames_ready(&mut exec, &mut frame_stream));
        let result = exec.run_until_stalled(&mut position_info);
        assert!(result.is_ready());

        // Watch number 2.
        let mut position_info = ring_buffer.watch_clock_recovery_position_info();
        let result = exec.run_until_stalled(&mut position_info);
        assert!(!result.is_ready());
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
            201,
        )));
        let _ = exec.wake_expired_timers();
        assert_eq!(2, frames_ready(&mut exec, &mut frame_stream));
        let result = exec.run_until_stalled(&mut position_info);
        assert!(result.is_ready());

        // Watch number 3.
        let mut position_info = ring_buffer.watch_clock_recovery_position_info();
        let result = exec.run_until_stalled(&mut position_info);
        assert!(!result.is_ready());
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
            201,
        )));
        let _ = exec.wake_expired_timers();
        assert_eq!(2, frames_ready(&mut exec, &mut frame_stream));
        let result = exec.run_until_stalled(&mut position_info);
        assert!(result.is_ready());

        let result = exec.run_until_stalled(&mut ring_buffer.stop());
        assert!(result.is_ready());
    }

    #[fixture(with_audio_frame_stream)]
    #[fuchsia::test]
    fn watch_delay_info(
        mut exec: fasync::TestExecutor,
        stream_config: StreamConfigProxy,
        mut frame_stream: AudioFrameStream,
    ) {
        let mut frame_fut = frame_stream.next();
        // Poll the frame stream, which should start the processing of proxy requests.
        exec.run_until_stalled(&mut frame_fut).expect_pending("no frames at the start");
        let _stream_config_properties = exec.run_until_stalled(&mut stream_config.get_properties());
        let _formats = exec.run_until_stalled(&mut stream_config.get_supported_formats());
        let (ring_buffer, server) = fidl::endpoints::create_proxy::<RingBufferMarker>();

        #[rustfmt::skip]
        let format = Format {
            pcm_format: Some(fidl_fuchsia_hardware_audio::PcmFormat {
                number_of_channels:      2u8,
                sample_format:           SampleFormat::PcmSigned,
                bytes_per_sample:        2u8,
                valid_bits_per_sample:   16u8,
                frame_rate:              44100,
            }),
            ..Default::default()
        };

        let result = stream_config.create_ring_buffer(&format, server);
        assert!(result.is_ok());

        let result = exec.run_until_stalled(&mut ring_buffer.watch_delay_info());

        // Should account for the external_delay here.
        match result {
            Poll::Ready(Ok(DelayInfo { internal_delay: Some(x), .. })) => {
                assert_eq!(zx::MonotonicDuration::from_millis(150).into_nanos(), x)
            }
            other => panic!("Expected the correct delay info, got {other:?}"),
        }
    }
}
