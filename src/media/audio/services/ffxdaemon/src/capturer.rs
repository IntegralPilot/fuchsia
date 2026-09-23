// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::clock::create_reference_clock;
use crate::error::ControllerError;
use crate::wav_socket::WavSocket;
use anyhow::{Context, Error, anyhow};
use fidl::endpoints::{ServerEnd, create_proxy};
use fidl_fuchsia_audio_controller as fac;
use fidl_fuchsia_media as fmedia;
use fidl_fuchsia_media_audio as fmedia_audio;
use fidl_fuchsia_ultrasound as fultrasound;
use fuchsia_audio::{Format, stop_listener};
use fuchsia_component::client::connect_to_protocol;
use futures::{AsyncWriteExt, TryStreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Capturer {
    proxy: fmedia::AudioCapturerProxy,
    format: Format,
}

impl Capturer {
    pub async fn new(
        source: fac::RecordSource,
        format: Format,
        gain_settings: Option<fac::GainSettings>,
    ) -> Result<Self, Error> {
        let (proxy, capturer_server_end) = create_proxy::<fmedia::AudioCapturerMarker>();

        match source {
            fac::RecordSource::Capturer(capturer_type) => match capturer_type {
                fac::CapturerConfig::StandardCapturer(config) => {
                    let audio_proxy = connect_to_protocol::<fmedia::AudioMarker>()
                        .context("Failed to connect to fuchsia.media.Audio")?;
                    audio_proxy.create_audio_capturer(capturer_server_end, false)?;

                    // Check that connection to AudioCore is valid.
                    proxy.get_reference_clock().await.context("Failed to get reference clock")?;

                    proxy.set_pcm_stream_type(&fmedia::AudioStreamType::from(format))?;

                    if let Some(gain_settings) = gain_settings {
                        let (gain_control_proxy, gain_control_server_end) =
                            create_proxy::<fmedia_audio::GainControlMarker>();

                        proxy.bind_gain_control(gain_control_server_end)?;

                        gain_settings
                            .gain
                            .and_then(|gain_db| gain_control_proxy.set_gain(gain_db).ok());
                        gain_settings.mute.and_then(|mute| gain_control_proxy.set_mute(mute).ok());
                    }

                    config.usage.and_then(|usage| proxy.set_usage2(usage).ok());

                    if let Some(clock_type) = config.clock {
                        let reference_clock = create_reference_clock(clock_type)?;
                        proxy.set_reference_clock(reference_clock)?;
                    }
                }
                fac::CapturerConfig::UltrasoundCapturer(_) => {
                    let ultrasound_factory = connect_to_protocol::<fultrasound::FactoryMarker>()
                        .context("Failed to connect to fuchsia.ultrasound.Factory")?;

                    let (_reference_clock, got_stream_type) =
                        ultrasound_factory.create_capturer(capturer_server_end).await?;

                    let got_format = Format::from(got_stream_type);
                    if format != got_format {
                        return Err(anyhow!(
                            "Requested format {} for ultrasound capturer does not match available format: {}",
                            format,
                            got_format
                        ));
                    }
                }
                _ => return Err(anyhow!("Unsupported capturer type.")),
            },
            fac::RecordSource::Loopback(..) => {
                let audio_proxy = connect_to_protocol::<fmedia::AudioMarker>()
                    .context("Failed to connect to fuchsia.media.Audio")?;
                audio_proxy.create_audio_capturer(capturer_server_end, true)?;
                proxy.set_pcm_stream_type(&fmedia::AudioStreamType::from(format))?;
            }
            _ => return Err(anyhow!("Unsupported RecordSource")),
        };

        Ok(Self { proxy, format })
    }

    pub async fn record(
        &mut self,
        mut socket: WavSocket,
        duration: Option<Duration>,
        cancel_server: Option<ServerEnd<fac::RecordCancelerMarker>>,
        buffer_size: Option<u64>,
    ) -> Result<fac::RecorderRecordResponse, ControllerError> {
        let stop_signal = AtomicBool::new(false);

        let packet_count = 4;
        let bytes_per_frame = self.format.bytes_per_frame() as u64;
        let buffer_size_bytes =
            buffer_size.unwrap_or(self.format.frames_per_second as u64 * bytes_per_frame);

        let bytes_per_packet = buffer_size_bytes / packet_count;

        let frames_per_packet = bytes_per_packet / bytes_per_frame;

        let packets_to_capture = duration.map(|duration| {
            (self.format.frames_in_duration(duration) as f64 * bytes_per_frame as f64
                / bytes_per_packet as f64)
                .ceil() as u64
        });
        let vmo = zx::Vmo::create(buffer_size_bytes)?;

        self.proxy.add_payload_buffer(0, vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?)?;
        self.proxy.start_async_capture(
            frames_per_packet
                .try_into()
                .map_err(|e| anyhow!("Frames per packet argument is too large: {}", e))?,
        )?;

        let mut stream = self.proxy.take_event_stream();
        let mut packets_so_far = 0;

        socket.write_header(duration, self.format).await?;
        let packet_fut = async {
            while let Some(event) = stream.try_next().await? {
                if stop_signal.load(Ordering::SeqCst) {
                    break;
                }
                match event {
                    fmedia::AudioCapturerEvent::OnPacketProduced { packet } => {
                        packets_so_far += 1;

                        let mut data = vec![0u8; packet.payload_size as usize];
                        let _audio_data = vmo
                            .read(&mut data[..], packet.payload_offset)
                            .map_err(|e| anyhow!("Failed to read vmo {e}"))?;

                        socket
                            .0
                            .write_all(&data)
                            .await
                            .map_err(|e| anyhow!("Error writing to socket: {e}"))?;

                        self.proxy
                            .release_packet(&packet)
                            .map_err(|e| anyhow!("Release packet error: {}", e))?;

                        if let Some(packets_to_capture) = packets_to_capture {
                            if packets_so_far == packets_to_capture {
                                break;
                            }
                        }
                    }
                    fmedia::AudioCapturerEvent::OnEndOfStream {} => break,
                    fmedia::AudioCapturerEvent::_UnknownEvent { .. } => {
                        todo!("AudioCapturer UnknownEvent")
                    }
                }
            }

            Ok(fac::RecorderRecordResponse {
                bytes_processed: Some(packets_so_far * bytes_per_packet),
                packets_processed: Some(packets_so_far),
                late_wakeups: None,
                ..Default::default()
            })
        };

        if let Some(cancel_server) = cancel_server {
            let (_cancel_res, packet_res) =
                futures::future::try_join(stop_listener(cancel_server, &stop_signal), packet_fut)
                    .await?;
            Ok(packet_res)
        } else {
            Ok(packet_fut.await?)
        }
    }
}
