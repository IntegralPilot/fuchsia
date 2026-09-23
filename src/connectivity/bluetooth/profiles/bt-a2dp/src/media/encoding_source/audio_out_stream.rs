// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use bt_a2dp::media_task::MediaTaskError;
use fidl_fuchsia_media::{AudioDeviceEnumeratorMarker, PcmFormat};
use fuchsia_audio_device::stream_config::SoftStreamConfig;
use fuchsia_bluetooth::types::{PeerId, peer_audio_stream_id};
use fuchsia_inspect::Node;
use fuchsia_inspect_derive::Inspect;
use futures::FutureExt;
use futures::future::BoxFuture;
use zx;

use super::{AudioSourceStream, AudioSourceStreamBuilder};

pub struct AudioOutStream {}

const LOCAL_MONOTONIC_CLOCK_DOMAIN: u32 = 0;

impl AudioSourceStream for fuchsia_audio_device::AudioFrameStream {
    fn watch_active(&self) -> BoxFuture<'static, bool> {
        self.watch_active_channels().boxed()
    }
}

impl AudioSourceStreamBuilder for AudioOutStream {
    fn build(
        &self,
        peer_id: &PeerId,
        pcm_format: PcmFormat,
        external_delay: std::time::Duration,
        inspect_parent: &mut Node,
    ) -> Result<Box<dyn AudioSourceStream>, MediaTaskError> {
        let mut stream = AudioOutStream::new(peer_id, pcm_format, external_delay.into())?;
        let _ = stream.iattach(inspect_parent, "audio_source");
        Ok(Box::new(stream))
    }
}

impl AudioOutStream {
    pub fn new(
        peer_id: &PeerId,
        pcm_format: PcmFormat,
        external_delay: zx::MonotonicDuration,
    ) -> Result<fuchsia_audio_device::AudioFrameStream, MediaTaskError> {
        let id = peer_audio_stream_id(*peer_id, crate::media::AUDIO_SOURCE_UUID);
        let (client, frame_stream) = SoftStreamConfig::create_output(
            &id,
            "Google",
            "Bluetooth A2DP",
            LOCAL_MONOTONIC_CLOCK_DOMAIN,
            pcm_format,
            zx::Duration::from_millis(10),
            external_delay,
        )
        .map_err(|e| MediaTaskError::Other(e.to_string()))?;

        let svc = fuchsia_component::client::connect_to_protocol::<AudioDeviceEnumeratorMarker>()
            .context("Failed to connect to AudioDeviceEnumerator")?;
        svc.add_device_by_channel("Bluetooth A2DP", false, client)?;

        Ok(frame_stream)
    }
}
