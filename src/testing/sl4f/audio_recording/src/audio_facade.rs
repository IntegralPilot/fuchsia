// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow, format_err};
use async_utils::async_once::Once;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_media::*;
use fidl_fuchsia_test_audio::QueuedCaptureResult;
use fuchsia_async::{self as fasync, TimeoutExt};
use futures::StreamExt;
use futures::channel::mpsc;
use futures::lock::Mutex;
use log::{error, info, trace};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::input_worker::InputWorker;
use crate::output_worker::{OutputWorker, PreCaptureArguments, PreCaptureCallback};

// Fixed configuration for our virtual output device.
const OUTPUT_SAMPLE_FORMAT: AudioSampleFormat = AudioSampleFormat::Signed16;
const OUTPUT_CHANNELS: u8 = 2;
const OUTPUT_FRAMES_PER_SECOND: u32 = 48000;

// Fixed configuration for our virtual input device.
const INPUT_SAMPLE_FORMAT: AudioSampleFormat = AudioSampleFormat::Signed16;
const INPUT_CHANNELS: u8 = 2;
const INPUT_FRAMES_PER_SECOND: u32 = 16000;

const ASF_RANGE_FLAG_FPS_CONTINUOUS: u16 = 1 << 0;

// If this changes, so too must the astro audio_core_config.
const AUDIO_OUTPUT_ID: [u8; 16] = [0x01; 16];
const AUDIO_INPUT_ID: [u8; 16] = [0x02; 16];

#[derive(Debug)]
struct VirtualOutput {
    sample_format: AudioSampleFormat,
    channels: u8,
    frames_per_second: u32,

    worker_task: Option<fasync::Task<()>>,
    worker: Arc<Mutex<OutputWorker>>,
}

impl VirtualOutput {
    pub fn new(
        sample_format: AudioSampleFormat,
        channels: u8,
        frames_per_second: u32,
    ) -> VirtualOutput {
        VirtualOutput {
            sample_format,
            channels,
            frames_per_second,

            worker_task: None,
            worker: Arc::new(Mutex::new(OutputWorker::default())),
        }
    }

    pub async fn start_output(
        &mut self,
        vad_control: &fidl_fuchsia_virtualaudio::ControlProxy,
    ) -> Result<(), Error> {
        // set buffer size to be at least 1s.
        let frames_1ms = self.frames_per_second / 1000;
        let frames_low = 1000 * frames_1ms;
        let frames_high = 2000 * frames_1ms;
        let frames_modulo = 1 * frames_1ms;

        let mut config = vad_control
            .get_default_configuration(
                fidl_fuchsia_virtualaudio::DeviceType::StreamConfig,
                &fidl_fuchsia_virtualaudio::Direction {
                    is_input: Some(false),
                    ..Default::default()
                },
            )
            .await?
            .map_err(|status| anyhow!("GetDefaultConfiguration returned error {:?}", status))?;
        config.unique_id = Some(AUDIO_OUTPUT_ID);

        match config
            .device_specific
            .as_mut()
            .ok_or_else(|| format_err!("device_specific not in config"))?
        {
            fidl_fuchsia_virtualaudio::DeviceSpecific::StreamConfig(stream_config) => {
                let ring_buffer = stream_config
                    .ring_buffer
                    .as_mut()
                    .ok_or_else(|| format_err!("ring buffer not in StreamConfig config"))?;
                ring_buffer.supported_formats =
                    Some(vec![fidl_fuchsia_virtualaudio::FormatRange {
                        sample_format_flags: crate::util::get_zircon_sample_format(
                            self.sample_format,
                        ),
                        min_frame_rate: self.frames_per_second,
                        max_frame_rate: self.frames_per_second,
                        min_channels: self.channels,
                        max_channels: self.channels,
                        rate_family_flags: ASF_RANGE_FLAG_FPS_CONTINUOUS,
                    }]);
                ring_buffer.ring_buffer_constraints =
                    Some(fidl_fuchsia_virtualaudio::RingBufferConstraints {
                        min_frames: frames_low,
                        max_frames: frames_high,
                        modulo_frames: frames_modulo,
                    });
            }
            fidl_fuchsia_virtualaudio::DeviceSpecificUnknown!() => {
                return Err(format_err!("device_specific in config is not StreamConfig"));
            }
        }

        // Create the output.
        info!("Installing output device");
        let (va_output_client, va_output_server) =
            create_endpoints::<fidl_fuchsia_virtualaudio::DeviceMarker>();
        vad_control
            .add_device(&config, va_output_server)
            .await?
            .map_err(|status| anyhow!("AddDevice returned error {:?}", status))?;

        let worker = self.worker.clone();
        self.worker_task = Some(fasync::Task::spawn(async move {
            if let Err(e) = OutputWorker::run(worker, va_output_client.into_proxy()).await {
                error!("OutputWorker failed: {:?}", e);
            }
        }));
        Ok(())
    }

    pub async fn get_saved_output(&self) -> Result<Vec<u8>, Error> {
        let mut worker = self.worker.lock().await;
        let len = worker.extracted_data_len()?;

        // Room for header created by write_header.
        let mut output = Vec::with_capacity(len + 44);
        self.write_header(len.try_into()?, &mut output)?;
        output.extend(worker.take_extracted_data()?);
        Ok(output)
    }

    fn write_header(&self, len: u32, dest: &mut impl Write) -> Result<(), Error> {
        let bytes_per_sample = crate::util::get_sample_size(
            crate::util::get_zircon_sample_format(self.sample_format),
        )?;

        // 8 Bytes
        dest.write_all("RIFF".as_bytes())?;
        dest.write_all(&u32::to_le_bytes(len + 8 + 28 + 8))?;

        // 28 bytes
        dest.write_all("WAVE".as_bytes())?; // wave_four_cc uint32
        dest.write_all("fmt ".as_bytes())?; // fmt_four_cc uint32
        dest.write_all(&u32::to_le_bytes(16))?; // fmt_chunk_len
        dest.write_all(&u16::to_le_bytes(1))?; // format
        dest.write_all(&u16::to_le_bytes(self.channels.into()))?;
        dest.write_all(&u32::to_le_bytes(self.frames_per_second))?;
        let channels: u32 = self.channels.into();
        dest.write_all(&u32::to_le_bytes(bytes_per_sample * channels * self.frames_per_second))?; // avg_byte_rate
        dest.write_all(&u16::to_le_bytes((bytes_per_sample * channels).try_into()?))?;
        dest.write_all(&u16::to_le_bytes((bytes_per_sample * 8).try_into()?))?;

        // 8 bytes
        dest.write_all("data".as_bytes())?;
        dest.write_all(&u32::to_le_bytes(len))?;

        Ok(())
    }
}

#[derive(Debug)]
struct VirtualInput {
    injection_data: Mutex<HashMap<usize, Vec<u8>>>,

    sample_format: AudioSampleFormat,
    channels: u8,
    frames_per_second: u32,

    worker_task: Option<fasync::Task<()>>,
    worker: Arc<Mutex<InputWorker>>,
}

impl VirtualInput {
    fn new(sample_format: AudioSampleFormat, channels: u8, frames_per_second: u32) -> Self {
        VirtualInput {
            injection_data: Mutex::new(HashMap::new()),
            sample_format,
            channels,
            frames_per_second,
            worker: Arc::new(Mutex::new(InputWorker::default())),
            worker_task: None,
        }
    }

    pub async fn start_input(
        &mut self,
        vad_control: &fidl_fuchsia_virtualaudio::ControlProxy,
    ) -> Result<(), Error> {
        // set buffer size to be at least 1s.
        let frames_1ms = self.frames_per_second / 1000;
        let frames_low = 1000 * frames_1ms;
        let frames_high = 2000 * frames_1ms;
        let frames_modulo = 1 * frames_1ms;

        let mut config = vad_control
            .get_default_configuration(
                fidl_fuchsia_virtualaudio::DeviceType::StreamConfig,
                &fidl_fuchsia_virtualaudio::Direction {
                    is_input: Some(true),
                    ..Default::default()
                },
            )
            .await?
            .map_err(|status| anyhow!("GetDefaultConfiguration returned error {:?}", status))?;
        config.unique_id = Some(AUDIO_INPUT_ID);

        match config
            .device_specific
            .as_mut()
            .ok_or_else(|| format_err!("device_specific not in config"))?
        {
            fidl_fuchsia_virtualaudio::DeviceSpecific::StreamConfig(stream_config) => {
                let ring_buffer = stream_config
                    .ring_buffer
                    .as_mut()
                    .ok_or_else(|| format_err!("ring buffer not in StreamConfig config"))?;
                ring_buffer.supported_formats =
                    Some(vec![fidl_fuchsia_virtualaudio::FormatRange {
                        sample_format_flags: crate::util::get_zircon_sample_format(
                            self.sample_format,
                        ),
                        min_frame_rate: self.frames_per_second,
                        max_frame_rate: self.frames_per_second,
                        min_channels: self.channels,
                        max_channels: self.channels,
                        rate_family_flags: ASF_RANGE_FLAG_FPS_CONTINUOUS,
                    }]);
                ring_buffer.ring_buffer_constraints =
                    Some(fidl_fuchsia_virtualaudio::RingBufferConstraints {
                        min_frames: frames_low,
                        max_frames: frames_high,
                        modulo_frames: frames_modulo,
                    });
            }
            fidl_fuchsia_virtualaudio::DeviceSpecificUnknown!() => {
                return Err(format_err!("device_specific in config is not StreamConfig"));
            }
        }

        // Create the input.
        info!("Installing input device");
        let (va_input_client, va_input_server) =
            create_endpoints::<fidl_fuchsia_virtualaudio::DeviceMarker>();
        vad_control
            .add_device(&config, va_input_server)
            .await?
            .map_err(|status| anyhow!("AddDevice returned error {:?}", status))?;

        let worker = self.worker.clone();
        self.worker_task = Some(fasync::Task::spawn(async move {
            if let Err(e) = InputWorker::run(worker, va_input_client.into_proxy()).await {
                error!("InputWorker failed: {:?}", e);
            }
        }));

        Ok(())
    }

    pub async fn append_input_track(&self, index: usize, data: Vec<u8>) {
        self.injection_data.lock().await.entry(index).or_default().extend(data);
    }

    pub async fn clear_input_track(&self, index: usize) -> bool {
        self.injection_data.lock().await.remove(&index).is_some()
    }

    pub async fn get_track_size_in_bytes(&self, index: usize) -> Result<usize, Error> {
        let data = self
            .injection_data
            .lock()
            .await
            .get(&index)
            .map(|v| v.len())
            .ok_or_else(|| format_err!("No data found at index {index}"))?;

        Ok(data)
    }

    pub async fn play(&self, index: usize) -> Result<(), Error> {
        let data = self
            .injection_data
            .lock()
            .await
            .get(&index)
            .map(|v| v.clone())
            .ok_or_else(|| format_err!("No data found in index {index}"))?;

        let mut worker = self.worker.lock().await;
        worker.clear_injection_data();
        worker.append_injection_data(data)?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), Error> {
        let mut worker = self.worker.lock().await;
        worker.clear_injection_data();
        Ok(())
    }
}

/// Perform Audio operations.
///
/// Note this object is shared among all threads created by server.
#[derive(Debug)]
pub struct AudioFacade {
    vad_control: fidl_fuchsia_virtualaudio::ControlProxy,

    // Lazily initialize output and input devices.
    // TODO(https://fxbug.dev/422488990): Fix isolation issues that prevent us from eagerly creating virtual audio devices.
    audio_output: Once<VirtualOutput>,
    audio_input: Once<VirtualInput>,
}

impl AudioFacade {
    /// Create a new facade.
    ///
    /// This will create and start both an input and output virtual audio device.
    pub async fn new() -> Result<AudioFacade, Error> {
        let path = format!("/dev/{}", fidl_fuchsia_virtualaudio::LEGACY_CONTROL_NODE_NAME);
        // Connect to the virtual audio control service.
        let (control_client, control_server) =
            create_endpoints::<fidl_fuchsia_virtualaudio::ControlMarker>();
        let () = fdio::service_connect(&path, control_server.into_channel())
            .with_context(|| format!("failed to connect to '{path}'"))?;
        let vad_control = control_client.into_proxy();

        vad_control.remove_all().await?;

        Ok(AudioFacade {
            vad_control: vad_control,
            audio_output: Once::new(),
            audio_input: Once::new(),
        })
    }

    async fn get_audio_input<'a>(&'a self) -> &'a VirtualInput {
        self.audio_input
            .get_or_init(async || {
                let mut input =
                    VirtualInput::new(INPUT_SAMPLE_FORMAT, INPUT_CHANNELS, INPUT_FRAMES_PER_SECOND);
                input.start_input(&self.vad_control).await.expect("Failed to start audio input");
                input
            })
            .await
    }

    async fn get_audio_output<'a>(&'a self) -> &'a VirtualOutput {
        self.audio_output
            .get_or_init(async || {
                let mut output = VirtualOutput::new(
                    OUTPUT_SAMPLE_FORMAT,
                    OUTPUT_CHANNELS,
                    OUTPUT_FRAMES_PER_SECOND,
                );
                output.start_output(&self.vad_control).await.expect("Failed to start audio output");
                output
            })
            .await
    }

    /// Shutdown all virtual audio clients on the system and exit.
    #[allow(unused)]
    pub async fn shutdown(self) -> Result<(), Error> {
        self.vad_control.remove_all().await?;
        Ok(())
    }

    /// Start output capturing. This will fail if output capturing is already started.
    pub async fn start_output_capture(&self) -> Result<(), Error> {
        self.get_audio_output().await.worker.lock().await.start_capturing()
    }

    /// Stop output capturing. This will fail if output capturing is not started.
    pub async fn stop_output_capture(&self) -> Result<(), Error> {
        self.get_audio_output().await.worker.lock().await.stop_capturing()
    }

    pub async fn wait_for_quiet_period(
        &self,
        quiet_period: Duration,
        timeout: Duration,
    ) -> Result<bool, Error> {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        let tx = std::sync::Mutex::new(tx);

        info!("Loading the callback");
        let callback = Arc::new(PreCaptureCallback(Box::new(move |args: PreCaptureArguments| {
            info!(
                "Got {} accumulated, and {} saw not quiet",
                args.accumulated_quiet_ms, args.has_sound_in_current_frames
            );
            if args.accumulated_quiet_ms >= quiet_period.as_millis() as u64 {
                tx.lock().unwrap().try_send(()).ok();
            }
            return false;
        })));

        self.get_audio_output()
            .await
            .worker
            .lock()
            .await
            .set_precapture_callback(Arc::downgrade(&callback))?;

        fasync::Task::spawn(async move { Ok(rx.next().await.is_some()) })
            .on_timeout(timeout, || Ok(false))
            .await
    }

    pub async fn trigger_capture_on_sound(
        &self,
        maximum_time_to_wait_for_sound: Duration,
        maximum_time_spent_recording: Duration,
        maybe_ending_quiet_duration: Option<Duration>,
    ) -> Result<QueuedCaptureResult, Error> {
        struct State {
            waiting_start_time: Instant,
            recording_start_time: Option<Instant>,
            done_callback: Option<mpsc::Sender<QueuedCaptureResult>>,
        }

        let (tx, mut rx) = mpsc::channel::<QueuedCaptureResult>(1);

        let state = std::sync::Mutex::new(State {
            waiting_start_time: Instant::now(),
            recording_start_time: None,
            done_callback: Some(tx),
        });

        let callback = Arc::new(PreCaptureCallback(Box::new(move |args: PreCaptureArguments| {
            let mut state = state.lock().unwrap();
            if state.done_callback.is_none() {
                return false;
            }

            if let Some(start_time) = &state.recording_start_time {
                // Currently capturing, determine if we should stop.
                if let Some(ending_duration) = &maybe_ending_quiet_duration {
                    if args.accumulated_quiet_ms > ending_duration.as_millis() as u64 {
                        // Stop if we accumulated enough quiet time.
                        state
                            .done_callback
                            .take()
                            .unwrap()
                            .try_send(QueuedCaptureResult::Captured)
                            .ok();
                        return false;
                    }
                }
                if Instant::now() - *start_time > maximum_time_spent_recording {
                    // Stop if we hit the capture time limit.
                    state
                        .done_callback
                        .take()
                        .unwrap()
                        .try_send(QueuedCaptureResult::CapturedToTimeLimit)
                        .ok();
                    return false;
                }
                // Keep capturing
                return true;
            } else if args.has_sound_in_current_frames {
                // Start capturing the sound we just recorded.
                state.recording_start_time = Some(Instant::now());
                return true;
            } else if Instant::now() - state.waiting_start_time > maximum_time_to_wait_for_sound {
                // We did not start capturing and exceeded our time limit. Return this failure.
                state
                    .done_callback
                    .take()
                    .unwrap()
                    .try_send(QueuedCaptureResult::FailedNoSoundTimeout)
                    .ok();
            }

            return false;
        })));

        self.get_audio_output()
            .await
            .worker
            .lock()
            .await
            .set_precapture_callback(Arc::downgrade(&callback))?;

        rx.next().await.ok_or_else(|| format_err!("BUG: Did not receive result of capture result"))
    }

    /// Get the size of the input audio at the given sample index.
    ///
    /// Returns an error if no audio is stored for the given track.
    pub async fn get_input_audio_size(&self, sample_index: usize) -> Result<usize, Error> {
        self.get_audio_input().await.get_track_size_in_bytes(sample_index).await
    }

    /// Get the captured audio output.
    ///
    /// Returns an error if no output audio is currently stored.
    pub async fn get_output_audio_vec(&self) -> Result<Vec<u8>, Error> {
        self.get_audio_output().await.get_saved_output().await
    }

    /// Wait until input injection is completed.
    pub async fn wait_until_input_playing_is_finished(&self) -> Result<(), Error> {
        let fut = self.get_audio_input().await.worker.lock().await.get_injection_end_future();

        trace!("Waiting for injection to finish...");
        fut.await;
        trace!("Injection future finished");
        Ok(())
    }

    /// Append audio to the given sample index.
    ///
    /// If no audio was previously stored at that index, an empty stream is initialized.
    ///
    /// The values stored in each sample index must be complete WAV files.
    pub async fn put_input_audio(
        &self,
        sample_index: usize,
        wave_data_vec: Vec<u8>,
    ) -> Result<i32, Error> {
        // TODO(perley): check wave format for correct bits per sample and float/int.
        let len = wave_data_vec.len();

        self.get_audio_input().await.append_input_track(sample_index, wave_data_vec).await;

        Ok(len.try_into()?)
    }

    /// Clear the audio stored at the given sample index.
    ///
    /// Returns true if a value was cleared, false if the index was already empty..
    pub async fn clear_input_audio(&self, sample_index: usize) -> bool {
        self.get_audio_input().await.clear_input_track(sample_index).await
    }

    /// Start injecting audio from the given sample index.
    ///
    /// Returns an error if no audio is stored at that index.
    pub async fn start_input_injection(&self, sample_index: usize) -> Result<bool, Error> {
        self.get_audio_input().await.play(sample_index).await?;
        Ok(true)
    }

    /// Stop injecting audio.
    ///
    /// This clears the contents of the input buffer, but it does not clear any sample index.
    pub async fn stop_input_injection(&self) -> Result<bool, Error> {
        self.get_audio_input().await.stop().await?;
        Ok(true)
    }
}
