// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::clock::create_reference_clock;
use crate::error::ControllerError;
use crate::wav_socket::WavSocket;
use anyhow::{Context, Error, anyhow};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_audio_controller as fac;
use fidl_fuchsia_media as fmedia;
use fidl_fuchsia_media_audio as fmedia_audio;
use fidl_fuchsia_ultrasound as fultrasound;
use fuchsia_async as fasync;
use fuchsia_audio::Format;
use fuchsia_component::client::connect_to_protocol;
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use std::cmp::min;

/// Default number of packets to send to the `AudioRenderer`.
const DEFAULT_PACKET_COUNT: u32 = 4;

pub struct Renderer {
    proxy: fmedia::AudioRendererProxy,
    format: Format,
    packet_count: u32,
}

impl Renderer {
    pub async fn new(
        config: fac::RendererConfig,
        format: Format,
        gain_settings: Option<fac::GainSettings>,
    ) -> Result<Self, Error> {
        let (proxy, renderer_server_end) = create_proxy::<fmedia::AudioRendererMarker>();

        let packet_count = match config {
            fac::RendererConfig::StandardRenderer(ref cfg) => cfg.packet_count.clone(),
            fac::RendererConfig::UltrasoundRenderer(ref cfg) => cfg.packet_count.clone(),
            _ => return Err(anyhow!("Unexpected RendererConfig")),
        }
        .unwrap_or(DEFAULT_PACKET_COUNT);

        match config {
            fac::RendererConfig::StandardRenderer(renderer_config) => {
                let audio_proxy = connect_to_protocol::<fmedia::AudioMarker>()
                    .context("Failed to connect to fuchsia.media.Audio")?;
                audio_proxy.create_audio_renderer(renderer_server_end)?;

                if let Some(clock_type) = renderer_config.clock {
                    let reference_clock = create_reference_clock(clock_type)?;
                    proxy.set_reference_clock(reference_clock)?;
                }

                renderer_config.usage.and_then(|usage| proxy.set_usage2(usage).ok());

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
            }
            fac::RendererConfig::UltrasoundRenderer(_) => {
                let ultrasound_factory = connect_to_protocol::<fultrasound::FactoryMarker>()
                    .context("Failed to connect to fuchsia.ultrasound.Factory")?;

                let (_reference_clock, got_stream_type) =
                    ultrasound_factory.create_renderer(renderer_server_end).await?;

                let got_format = Format::from(got_stream_type);
                if format != got_format {
                    return Err(anyhow!(
                        "Requested format {} for ultrasound renderer does not match available format: {}",
                        format,
                        got_format
                    ));
                }
            }
            _ => return Err(anyhow!("Unexpected RendererConfig")),
        }

        Ok(Self { proxy, format, packet_count })
    }

    pub async fn play(
        &self,
        socket: WavSocket,
    ) -> Result<fac::PlayerPlayResponse, ControllerError> {
        let vmo_size_bytes =
            self.format.frames_per_second as usize * self.format.bytes_per_frame() as usize;
        let vmo = zx::Vmo::create(vmo_size_bytes as u64)?;

        let bytes_per_packet = min(vmo_size_bytes / self.packet_count as usize, 32000 as usize);

        self.proxy.add_payload_buffer(0, vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?)?;

        self.proxy.enable_min_lead_time_events(true)?;

        // Wait for AudioRenderer to initialize (lead_time > 0)
        let mut stream = self.proxy.take_event_stream();
        while let Some(event) = stream.try_next().await? {
            match event {
                fmedia::AudioRendererEvent::OnMinLeadTimeChanged { min_lead_time_nsec } => {
                    if min_lead_time_nsec > 0 {
                        break;
                    }
                }
                fmedia::AudioRendererEvent::_UnknownEvent { .. } => {
                    todo!("AudioRenderer UnknownEvent")
                }
            }
        }

        let offsets: Vec<usize> =
            (0..self.packet_count).map(|packet| packet as usize * bytes_per_packet).collect();

        let futs = offsets.iter().map(|offset| async {
            self.send_next_packet(
                offset.to_owned() as u64,
                fasync::Socket::from_socket(
                    socket.0.as_ref().duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                ),
                vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                bytes_per_packet,
                1,
            )
            .await
        });

        futures::future::try_join_all(futs).await?;

        // TODO(b/300279107): Calculate total bytes sent to an AudioRenderer.
        Ok(fac::PlayerPlayResponse { bytes_processed: None, ..Default::default() })
    }

    fn send_next_packet<'b>(
        &'b self,
        payload_offset: u64,
        socket: fasync::Socket,
        vmo: zx::Vmo,
        bytes_per_packet: usize,
        iteration: u32,
    ) -> BoxFuture<'b, Result<(), Error>> {
        async move {
            let mut socket = WavSocket(socket);
            let mut buf = vec![0u8; bytes_per_packet];
            let total_bytes_read = socket.read_until_full(&mut buf).await? as usize;

            if total_bytes_read == 0 {
                return Ok(());
            }
            vmo.write(&buf[..total_bytes_read], payload_offset)?;

            let packet_fut = self.proxy.send_packet(&fmedia::StreamPacket {
                pts: fmedia::NO_TIMESTAMP,
                payload_buffer_id: 0,
                payload_offset,
                payload_size: total_bytes_read as u64,
                flags: 0,
                buffer_config: 0,
                stream_segment_id: 0,
            });

            if payload_offset == 0 && iteration == 1 {
                self.proxy.play(fmedia::NO_TIMESTAMP, fmedia::NO_TIMESTAMP).await?;
            }

            packet_fut.await?;

            if total_bytes_read == bytes_per_packet {
                self.send_next_packet(
                    payload_offset,
                    socket.0,
                    vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                    bytes_per_packet,
                    iteration + 1,
                )
                .await
            } else {
                Ok(())
            }
        }
        .boxed()
    }
}
