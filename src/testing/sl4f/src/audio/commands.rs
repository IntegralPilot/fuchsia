// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::audio::types::AudioMethod;
use crate::server::Facade;
use anyhow::{Context, Error};
use async_trait::async_trait;
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use fidl_fuchsia_media::{AudioRenderUsage2, AudioSampleFormat, AudioStreamType};
use fidl_fuchsia_media_sounds::{PlayerMarker, PlayerProxy};
use fidl_fuchsia_test_audio::{CaptureMarker, CaptureProxy, InjectionMarker, InjectionProxy};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::lock::Mutex;
use futures::{AsyncReadExt, AsyncWriteExt};
use log::{error, info};
use serde_json::{Value, to_value};

#[async_trait(?Send)]
impl Facade for AudioFacade {
    async fn handle_request(&self, method: String, args: Value) -> Result<Value, Error> {
        match method.parse()? {
            AudioMethod::PutInputAudio => self.put_input_audio(args).await,
            AudioMethod::StartInputInjection => self.start_input_injection(args).await,
            AudioMethod::StopInputInjection => self.stop_input_injection().await,
            AudioMethod::StartOutputSave => self.start_output_save().await,
            AudioMethod::StopOutputSave => self.stop_output_save().await,
            AudioMethod::GetOutputAudio => self.get_output_audio().await,
            AudioMethod::PlaySound => self.play_sine_wave().await,
        }
    }
}

#[derive(Debug)]
pub struct AudioFacade {
    injection_proxy: InjectionProxy,
    capture_proxy: CaptureProxy,
    player_proxy: PlayerProxy,
    sound_buffer_id: Mutex<u32>,
}

impl AudioFacade {
    pub fn new() -> Result<AudioFacade, Error> {
        info!("Launching audio_recording component");
        let injection_proxy = connect_to_protocol::<InjectionMarker>()?;
        let recording_proxy = connect_to_protocol::<CaptureMarker>()?;
        let player_proxy = connect_to_protocol::<PlayerMarker>()?;
        let sound_buffer_id = Mutex::new(0u32);

        Ok(AudioFacade {
            injection_proxy,
            capture_proxy: recording_proxy,
            player_proxy,
            sound_buffer_id,
        })
    }

    pub async fn put_input_audio(&self, args: Value) -> Result<Value, Error> {
        let data = args.get("data").ok_or_else(|| format_err!("PutInputAudio failed, no data"))?;
        let data =
            data.as_str().ok_or_else(|| format_err!("PutInputAudio failed, data not string"))?;

        let wave_data_vec = BASE64_STANDARD.decode(data)?;

        let sample_index =
            args["index"].as_u64().ok_or_else(|| format_err!("index not a number"))?;
        let sample_index = sample_index.try_into()?;

        let _ = self
            .injection_proxy
            .clear_input_audio(sample_index)
            .await
            .context("Error calling clear_input_audio")?;

        let (tx, rx) = zx::Socket::create_stream();
        tx.half_close().expect("prevent writes on other side");
        self.injection_proxy.write_input_audio(sample_index, rx)?;

        let mut tx = fasync::Socket::from_socket(tx);
        if let Err(e) = tx.write_all(&wave_data_vec).await {
            error!("Failed to write audio data to socket: {:?}", e);
            return Err(e.into());
        }
        std::mem::drop(tx); // close socket to prevent hang

        let byte_count = self
            .injection_proxy
            .get_input_audio_size(sample_index)
            .await
            .context("Error calling get_input_audio_size")?
            .map_err(|e| anyhow!("get_input_audio_size failed: {:?}", e))?;
        if byte_count != wave_data_vec.len() as u64 {
            bail!("Expected to write {} bytes, found {} bytes", wave_data_vec.len(), byte_count);
        }

        Ok(to_value(byte_count)?)
    }

    pub async fn start_input_injection(&self, args: Value) -> Result<Value, Error> {
        let sample_index =
            args["index"].as_u64().ok_or_else(|| format_err!("index not a number"))?;
        let sample_index = sample_index.try_into()?;
        let status = self
            .injection_proxy
            .start_input_injection(sample_index)
            .await
            .context("Error calling start_input_injection")?;
        match status {
            Ok(_) => return Ok(to_value(true)?),
            Err(_) => return Ok(to_value(false)?),
        }
    }

    pub async fn stop_input_injection(&self) -> Result<Value, Error> {
        let status = self
            .injection_proxy
            .stop_input_injection()
            .await
            .context("Error calling stop_input_injection")?;
        match status {
            Ok(_) => return Ok(to_value(true)?),
            Err(_) => return Ok(to_value(false)?),
        }
    }

    pub async fn start_output_save(&self) -> Result<Value, Error> {
        let status = self
            .capture_proxy
            .start_output_capture()
            .await
            .context("Error calling start_output_save")?;
        match status {
            Ok(_) => return Ok(to_value(true)?),
            Err(_) => return Ok(to_value(false)?),
        }
    }

    pub async fn stop_output_save(&self) -> Result<Value, Error> {
        let status = self
            .capture_proxy
            .stop_output_capture()
            .await
            .context("Error calling stop_output_save")?;
        match status {
            Ok(_) => return Ok(to_value(true)?),
            Err(_) => return Ok(to_value(false)?),
        }
    }

    pub async fn get_output_audio(&self) -> Result<Value, Error> {
        let mut rx_socket = fasync::Socket::from_socket(
            match self
                .capture_proxy
                .get_output_audio()
                .await
                .context("Error calling get_output_audio")?
            {
                Ok(socket) => socket,
                Err(e) => {
                    bail!("Failure: {:?}", e);
                }
            },
        );

        let mut buffer = Vec::new();
        rx_socket.read_to_end(&mut buffer).await?;
        Ok(to_value(BASE64_STANDARD.encode(&buffer))?)
    }

    // This will play a 1-second 399-Hz sine wave at volume 0.1, to the default audio
    // output device, using the standard `PlaySound2` client playback API.
    pub async fn play_sine_wave(&self) -> Result<Value, Error> {
        let mut id = self.sound_buffer_id.lock().await;
        *(id) += 1;
        const FREQUENCY: f32 = 399.0;
        const VOLUME: f32 = 0.1;
        const DURATION: std::time::Duration = std::time::Duration::from_secs(1);
        const FRAMES_PER_SECOND: u32 = 44100;

        let (buffer, stream_type) =
            self.sound_in_buffer(FREQUENCY, VOLUME, FRAMES_PER_SECOND, DURATION)?;

        match self.player_proxy.add_sound_buffer(*id, buffer, &stream_type) {
            Ok(()) => (),
            Err(e) => return Err(format_err!("Cannot add sound to buffer: {}", e)),
        };
        self.player_proxy
            .play_sound2(*id, AudioRenderUsage2::Media)
            .await?
            .map_err(|err| format_err!("PlaySound2 failed: {:?}", err))?;
        Ok(to_value(true)?)
    }

    fn sound_in_buffer(
        &self,
        frequency: f32,
        volume: f32,
        frames_per_second: u32,
        duration: std::time::Duration,
    ) -> Result<(fidl_fuchsia_mem::Buffer, AudioStreamType), Error> {
        let frame_count = (frames_per_second as f32 * duration.as_secs_f32()) as usize;

        let amplitude = volume * (i16::MAX as f32);
        let frames_per_period = (frames_per_second as f32) / (frequency as f32);
        let mut samples = std::vec::Vec::with_capacity(frame_count);
        for i in 0..frame_count {
            let sample_f = f32::sin((i as f32) / frames_per_period * 2.0 * std::f32::consts::PI);
            samples.push((sample_f * amplitude) as i16);
        }

        // This is safe since `bytes` will cover the same memory range as `samples`.
        let bytes =
            unsafe { std::slice::from_raw_parts(samples.as_ptr() as *const _, samples.len() * 2) };
        let vmo = zx::Vmo::create((frame_count * 2) as u64).context("Creating VMO")?;
        vmo.write(&bytes, 0).context("Writing to VMO")?;

        Ok((
            fidl_fuchsia_mem::Buffer { vmo: vmo, size: (frame_count * 2) as u64 },
            AudioStreamType {
                sample_format: AudioSampleFormat::Signed16,
                channels: 1,
                frames_per_second: frames_per_second,
            },
        ))
    }
}
