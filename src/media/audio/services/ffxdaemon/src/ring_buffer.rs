// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use fuchsia_audio::{Format, VmoBuffer};
use {
    fidl_fuchsia_audio as faudio, fidl_fuchsia_audio_device as fadevice,
    fidl_fuchsia_hardware_audio as fhaudio,
};

// TODO(b/317991807) Remove #[async_trait] when supported by compiler.
#[async_trait]
pub trait RingBuffer {
    /// Starts the ring buffer.
    ///
    /// Returns the CLOCK_MONOTONIC time at which it started.
    async fn start(&self) -> Result<zx::MonotonicInstant, anyhow::Error>;

    /// Stops the ring buffer.
    async fn stop(&self) -> Result<(), anyhow::Error>;

    /// Returns a references to the [VmoBuffer] that contains this ring buffer's data.
    fn vmo_buffer(&self) -> &VmoBuffer;

    /// Returns the number of bytes allocated to the producer.
    fn producer_bytes(&self) -> u64;

    /// Returns the number of bytes allocated to the consumer.
    fn consumer_bytes(&self) -> u64;

    /// Changes which channels are active.
    async fn set_active_channels(
        &self,
        active_channels_bitmask: u64,
    ) -> Result<zx::MonotonicInstant, anyhow::Error>;
}

/// A [RingBuffer] backed by the `fuchsia.hardware.audio.RingBuffer` protocol.
pub struct HardwareRingBuffer {
    proxy: fhaudio::RingBufferProxy,
    vmo_buffer: VmoBuffer,
    driver_transfer_bytes: u64,
}

impl HardwareRingBuffer {
    pub async fn new(
        proxy: fhaudio::RingBufferProxy,
        format: Format,
    ) -> Result<Self, anyhow::Error> {
        // Request at least 100ms worth of frames.
        let min_frames = format.frames_per_second / 10;

        let (num_frames, vmo) = proxy
            .get_vmo(min_frames, 0 /* ring buffer notifications unused */)
            .await
            .context("Failed to call GetVmo")?
            .map_err(|e| anyhow!("Couldn't receive vmo from ring buffer: {:?}", e))?;

        let vmo_buffer = VmoBuffer::new(vmo, num_frames as u64, format)?;

        let driver_transfer_bytes = proxy
            .get_properties()
            .await?
            .driver_transfer_bytes
            .ok_or_else(|| anyhow::anyhow!("driver transfer bytes unavailable"))?
            as u64;

        log::debug!("driver transfer bytes: {} num_frames: {}", driver_transfer_bytes, num_frames);

        Ok(Self { proxy, vmo_buffer, driver_transfer_bytes })
    }
}

#[async_trait]
impl RingBuffer for HardwareRingBuffer {
    async fn start(&self) -> Result<zx::MonotonicInstant, anyhow::Error> {
        let start_time = self.proxy.start().await?;
        Ok(zx::MonotonicInstant::from_nanos(start_time))
    }

    async fn stop(&self) -> Result<(), anyhow::Error> {
        self.proxy.stop().await?;
        Ok(())
    }

    fn vmo_buffer(&self) -> &VmoBuffer {
        &self.vmo_buffer
    }

    fn producer_bytes(&self) -> u64 {
        self.driver_transfer_bytes
    }

    fn consumer_bytes(&self) -> u64 {
        self.driver_transfer_bytes
    }

    async fn set_active_channels(
        &self,
        active_channels_bitmask: u64,
    ) -> Result<zx::MonotonicInstant, anyhow::Error> {
        let set_time = self
            .proxy
            .set_active_channels(active_channels_bitmask)
            .await
            .context("failed to call SetActiveChannels")?
            .map_err(|err| anyhow!("failed to set active channels: {:?}", err))?;
        Ok(zx::MonotonicInstant::from_nanos(set_time))
    }
}

/// A [RingBuffer] backed by the `fuchsia.audio.device.RingBuffer` protocol.
pub struct AudioDeviceRingBuffer {
    proxy: fadevice::RingBufferProxy,
    vmo_buffer: VmoBuffer,
    producer_bytes: u64,
    consumer_bytes: u64,
}

impl AudioDeviceRingBuffer {
    pub async fn new(
        proxy: fadevice::RingBufferProxy,
        ring_buffer: faudio::RingBuffer,
        _properties: fadevice::RingBufferProperties,
        format: Format,
    ) -> Result<Self, anyhow::Error> {
        let producer_bytes =
            ring_buffer.producer_bytes.ok_or_else(|| anyhow!("missing 'producer_bytes'"))?;
        let consumer_bytes =
            ring_buffer.consumer_bytes.ok_or_else(|| anyhow!("missing 'consumer_bytes'"))?;
        let buffer = ring_buffer.buffer.ok_or_else(|| anyhow!("missing 'buffer'"))?;

        let rb_format: Format = ring_buffer
            .format
            .ok_or_else(|| anyhow!("missing 'format'"))?
            .try_into()
            .map_err(|err| anyhow!("invalid ring buffer format: {}", err))?;
        if rb_format != format {
            bail!("requested format {} does not match ring buffer format {}", format, rb_format);
        }

        let num_frames = buffer.size / (format.bytes_per_frame() as u64);
        let vmo_buffer = VmoBuffer::new(buffer.vmo, num_frames, format)?;

        Ok(Self { proxy, vmo_buffer, producer_bytes, consumer_bytes })
    }
}

#[async_trait]
impl RingBuffer for AudioDeviceRingBuffer {
    async fn start(&self) -> Result<zx::MonotonicInstant, anyhow::Error> {
        let response = self
            .proxy
            .start(&Default::default())
            .await
            .context("failed to call Start")?
            .map_err(|err| anyhow!("failed to start ring buffer: {:?}", err))?;
        let start_time = response.start_time.ok_or_else(|| anyhow!("missing 'start_time'"))?;
        Ok(zx::MonotonicInstant::from_nanos(start_time))
    }

    async fn stop(&self) -> Result<(), anyhow::Error> {
        let _ = self
            .proxy
            .stop(&Default::default())
            .await
            .context("failed to call Stop")?
            .map_err(|err| anyhow!("failed to stop ring buffer: {:?}", err))?;
        Ok(())
    }

    fn vmo_buffer(&self) -> &VmoBuffer {
        &self.vmo_buffer
    }

    fn producer_bytes(&self) -> u64 {
        self.producer_bytes
    }

    fn consumer_bytes(&self) -> u64 {
        self.consumer_bytes
    }

    async fn set_active_channels(
        &self,
        active_channels_bitmask: u64,
    ) -> Result<zx::MonotonicInstant, anyhow::Error> {
        let response = self
            .proxy
            .set_active_channels(&fadevice::RingBufferSetActiveChannelsRequest {
                channel_bitmask: Some(active_channels_bitmask),
                ..Default::default()
            })
            .await
            .context("failed to call SetActiveChannels")?
            .map_err(|err| anyhow!("failed to set active channels: {:?}", err))?;
        let set_time = response.set_time.unwrap_or(0);
        Ok(zx::MonotonicInstant::from_nanos(set_time))
    }
}
