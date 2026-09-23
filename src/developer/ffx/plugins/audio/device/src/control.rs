// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_audio_device as fadevice;
use fdomain_fuchsia_hardware_audio as fhaudio;
use ffx_command_error::{FfxContext, Result, bug, user_error};
use fuchsia_audio_fdomain::dai::DaiFormat;
use zx_status::Status;

#[async_trait]
pub trait DeviceControl {
    async fn set_dai_format(
        &self,
        dai_format: DaiFormat,
        element_id: Option<fadevice::ElementId>,
    ) -> Result<()>;

    async fn start(&self) -> Result<zx_types::zx_time_t>;

    async fn stop(&self) -> Result<zx_types::zx_time_t>;

    async fn reset(&self) -> Result<()>;
}

pub struct HardwareCodec(pub fhaudio::CodecProxy);

#[async_trait]
impl DeviceControl for HardwareCodec {
    async fn set_dai_format(
        &self,
        dai_format: DaiFormat,
        _element_id: Option<fadevice::ElementId>,
    ) -> Result<()> {
        let _ = self
            .0
            .set_dai_format(&dai_format.into())
            .await
            .bug_context("Failed to call SetDaiFormat")?
            .map_err(Status::err_from_raw)
            .user_message("failed to set DAI format")?;
        Ok(())
    }

    async fn start(&self) -> Result<zx_types::zx_time_t> {
        let start_time = self.0.start().await.bug_context("Failed to call Start")?;
        Ok(start_time)
    }

    async fn stop(&self) -> Result<zx_types::zx_time_t> {
        let stop_time = self.0.stop().await.bug_context("Failed to call Stop")?;
        Ok(stop_time)
    }

    async fn reset(&self) -> Result<()> {
        let _ = self.0.reset().await.bug_context("Failed to call Reset")?;
        Ok(())
    }
}

pub struct HardwareComposite(pub fhaudio::CompositeProxy);

#[async_trait]
impl DeviceControl for HardwareComposite {
    async fn set_dai_format(
        &self,
        dai_format: DaiFormat,
        element_id: Option<fadevice::ElementId>,
    ) -> Result<()> {
        let element_id = element_id
            .ok_or_else(|| user_error!("element ID is required for Composite devices"))?;
        self.0
            .set_dai_format(element_id, &dai_format.into())
            .await
            .bug_context("Failed to call SetDaiFormat")?
            .map_err(|err| user_error!("failed to set DAI format: {:?}", err))?;
        Ok(())
    }

    async fn start(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("start is not supported for Composite devices"))
    }

    async fn stop(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("stop is not supported for Composite devices"))
    }

    async fn reset(&self) -> Result<()> {
        let _ = self
            .0
            .reset()
            .await
            .bug_context("Failed to call Reset")?
            .map_err(|err| user_error!("Failed to reset: {:?}", err))?;
        Ok(())
    }
}

pub struct HardwareDai(#[allow(unused)] pub fhaudio::DaiProxy);

#[async_trait]
impl DeviceControl for HardwareDai {
    async fn set_dai_format(
        &self,
        _dai_format: DaiFormat,
        _element_id: Option<fadevice::ElementId>,
    ) -> Result<()> {
        Err(user_error!("set dai-format is not supported for DAI devices"))
    }

    async fn start(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("start is not supported for DAI devices"))
    }

    async fn stop(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("stop is not supported for DAI devices"))
    }

    async fn reset(&self) -> Result<()> {
        let _ = self.0.reset().await.bug_context("Failed to call Reset")?;
        Ok(())
    }
}

pub struct HardwareStreamConfig(#[allow(unused)] pub fhaudio::StreamConfigProxy);

#[async_trait]
impl DeviceControl for HardwareStreamConfig {
    async fn set_dai_format(
        &self,
        _dai_format: DaiFormat,
        _element_id: Option<fadevice::ElementId>,
    ) -> Result<()> {
        Err(user_error!("set dai-format is not supported for StreamConfig devices"))
    }

    async fn start(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("start is not supported for StreamConfig devices"))
    }

    async fn stop(&self) -> Result<zx_types::zx_time_t> {
        Err(user_error!("stop is not supported for StreamConfig devices"))
    }

    async fn reset(&self) -> Result<()> {
        Err(user_error!("reset is not supported for StreamConfig devices"))
    }
}

pub struct Registry(pub fadevice::ControlProxy);

#[async_trait]
impl DeviceControl for Registry {
    async fn set_dai_format(
        &self,
        dai_format: DaiFormat,
        element_id: Option<fadevice::ElementId>,
    ) -> Result<()> {
        let _ = self
            .0
            .set_dai_format(&fadevice::ControlSetDaiFormatRequest {
                element_id,
                dai_format: Some(dai_format.into()),
                ..Default::default()
            })
            .await
            .bug_context("Failed to call SetDaiFormat")?
            .map_err(|err| user_error!("Failed to set DAI format: {:?}", err))?;
        Ok(())
    }

    async fn start(&self) -> Result<zx_types::zx_time_t> {
        let response = self
            .0
            .codec_start()
            .await
            .bug_context("Failed to call CodecStart")?
            .map_err(|err| user_error!("Failed to start: {:?}", err))?;
        let start_time = response
            .start_time
            .ok_or_else(|| bug!("CodecStart response is missing 'start_time'"))?;
        Ok(start_time)
    }

    async fn stop(&self) -> Result<zx_types::zx_time_t> {
        let response = self
            .0
            .codec_stop()
            .await
            .bug_context("Failed to call CodecStop")?
            .map_err(|err| user_error!("Failed to stop: {:?}", err))?;
        let stop_time =
            response.stop_time.ok_or_else(|| bug!("CodecStop response is missing 'stop_time'"))?;
        Ok(stop_time)
    }

    async fn reset(&self) -> Result<()> {
        let _ = self
            .0
            .reset()
            .await
            .bug_context("Failed to call Reset")?
            .map_err(|err| user_error!("Failed to reset: {:?}", err))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_test_util::fdomain::spawn_stream_handler;
    use fuchsia_audio_fdomain::dai::{DaiFrameFormat, DaiFrameFormatStandard, DaiSampleFormat};
    use fuchsia_audio_fdomain::format::{BITS_16, BITS_32, SampleSize};
    use std::sync::{Arc, Mutex};

    const TEST_CODEC_START_TIME: zx_types::zx_time_t = 123;
    const TEST_CODEC_STOP_TIME: zx_types::zx_time_t = 456;

    const TEST_DAI_FORMAT: DaiFormat = DaiFormat {
        number_of_channels: 2,
        channels_to_use_bitmask: 0x3,
        sample_format: DaiSampleFormat::PcmSigned,
        frame_format: DaiFrameFormat::Standard(DaiFrameFormatStandard::I2S),
        frame_rate: 48000,
        sample_size: SampleSize::from_partial_bits(BITS_16, BITS_32).unwrap(),
    };

    const TEST_ELEMENT_ID: fadevice::ElementId = 1;

    fn serve_hw_codec(client: &Arc<fdomain_client::Client>) -> fhaudio::CodecProxy {
        spawn_stream_handler(client, move |request| async move {
            match request {
                fhaudio::CodecRequest::SetDaiFormat { format: _, responder } => {
                    responder
                        .send(Ok(&fhaudio::CodecFormatInfo {
                            external_delay: None,
                            turn_on_delay: None,
                            turn_off_delay: None,
                            ..Default::default()
                        }))
                        .unwrap();
                }
                fhaudio::CodecRequest::Start { responder } => {
                    responder.send(TEST_CODEC_START_TIME).unwrap();
                }
                fhaudio::CodecRequest::Stop { responder } => {
                    responder.send(TEST_CODEC_STOP_TIME).unwrap();
                }
                fhaudio::CodecRequest::Reset { responder } => {
                    responder.send().unwrap();
                }
                _ => unimplemented!(),
            }
        })
    }

    fn serve_hw_composite(client: &Arc<fdomain_client::Client>) -> fhaudio::CompositeProxy {
        spawn_stream_handler(client, move |request| async move {
            match request {
                fhaudio::CompositeRequest::SetDaiFormat {
                    processing_element_id: _,
                    format: _,
                    responder,
                } => {
                    responder.send(Ok(())).unwrap();
                }
                fhaudio::CompositeRequest::Reset { responder } => {
                    responder.send(Ok(())).unwrap();
                }
                _ => unimplemented!(),
            }
        })
    }

    fn serve_hw_dai(client: &Arc<fdomain_client::Client>) -> fhaudio::DaiProxy {
        spawn_stream_handler(client, move |request| async move {
            match request {
                fhaudio::DaiRequest::Reset { responder } => {
                    responder.send().unwrap();
                }
                _ => unimplemented!(),
            }
        })
    }

    fn serve_hw_streamconfig(client: &Arc<fdomain_client::Client>) -> fhaudio::StreamConfigProxy {
        spawn_stream_handler(client, move |request| async move {
            match request {
                _ => unimplemented!(),
            }
        })
    }

    #[derive(Default)]
    struct FakeRegistryControlState {
        is_dai_format_set: bool,
        is_codec_started: bool,
    }

    fn serve_registry_control(client: &Arc<fdomain_client::Client>) -> fadevice::ControlProxy {
        let state = Arc::new(Mutex::new(FakeRegistryControlState::default()));
        spawn_stream_handler(client, move |request| {
            let state = state.clone();
            async move {
                match request {
                    fadevice::ControlRequest::SetDaiFormat { payload, responder } => {
                        assert!(payload.dai_format.is_some());
                        assert!(payload.element_id.is_some());

                        state.lock().unwrap().is_dai_format_set = true;

                        responder
                            .send(Ok(&fadevice::ControlSetDaiFormatResponse {
                                state: Some(fhaudio::CodecFormatInfo {
                                    external_delay: None,
                                    turn_on_delay: None,
                                    turn_off_delay: None,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }))
                            .unwrap();
                    }
                    fadevice::ControlRequest::CodecStart { responder } => {
                        let mut state = state.lock().unwrap();

                        if state.is_codec_started {
                            responder
                                .send(Err(fadevice::ControlCodecStartError::AlreadyStarted))
                                .unwrap();
                            return;
                        }

                        if !state.is_dai_format_set {
                            responder
                                .send(Err(fadevice::ControlCodecStartError::DaiFormatNotSet))
                                .unwrap();
                            return;
                        }

                        state.is_codec_started = true;
                        responder
                            .send(Ok(&fadevice::ControlCodecStartResponse {
                                start_time: Some(TEST_CODEC_START_TIME),
                                ..Default::default()
                            }))
                            .unwrap();
                    }
                    fadevice::ControlRequest::CodecStop { responder } => {
                        let mut state = state.lock().unwrap();

                        if !state.is_codec_started {
                            responder
                                .send(Err(fadevice::ControlCodecStopError::AlreadyStopped))
                                .unwrap();
                            return;
                        }

                        if !state.is_dai_format_set {
                            responder
                                .send(Err(fadevice::ControlCodecStopError::DaiFormatNotSet))
                                .unwrap();
                            return;
                        }

                        state.is_codec_started = false;
                        responder
                            .send(Ok(&fadevice::ControlCodecStopResponse {
                                stop_time: Some(TEST_CODEC_STOP_TIME),
                                ..Default::default()
                            }))
                            .unwrap();
                    }
                    fadevice::ControlRequest::Reset { responder } => {
                        *state.lock().unwrap() = FakeRegistryControlState::default();
                        responder.send(Ok(&fadevice::ControlResetResponse::default())).unwrap();
                    }
                    _ => unimplemented!(),
                }
            }
        })
    }

    #[fuchsia::test]
    async fn test_set_dai_format() {
        let client = fdomain_local::local_client_empty();
        let codec: Box<dyn DeviceControl> = Box::new(HardwareCodec(serve_hw_codec(&client)));
        assert!(codec.set_dai_format(TEST_DAI_FORMAT, None).await.is_ok());

        let composite: Box<dyn DeviceControl> =
            Box::new(HardwareComposite(serve_hw_composite(&client)));
        assert!(composite.set_dai_format(TEST_DAI_FORMAT, Some(TEST_ELEMENT_ID)).await.is_ok());

        let dai: Box<dyn DeviceControl> = Box::new(HardwareDai(serve_hw_dai(&client)));
        assert!(dai.set_dai_format(TEST_DAI_FORMAT, None).await.is_err());

        let streamconfig: Box<dyn DeviceControl> =
            Box::new(HardwareStreamConfig(serve_hw_streamconfig(&client)));
        assert!(streamconfig.set_dai_format(TEST_DAI_FORMAT, None).await.is_err());

        let registry: Box<dyn DeviceControl> = Box::new(Registry(serve_registry_control(&client)));
        assert!(registry.set_dai_format(TEST_DAI_FORMAT, Some(TEST_ELEMENT_ID)).await.is_ok());
    }

    #[fuchsia::test]
    async fn test_start() {
        let client = fdomain_local::local_client_empty();
        let codec: Box<dyn DeviceControl> = Box::new(HardwareCodec(serve_hw_codec(&client)));
        assert_matches!(codec.start().await, Ok(TEST_CODEC_START_TIME));

        let composite: Box<dyn DeviceControl> =
            Box::new(HardwareComposite(serve_hw_composite(&client)));
        assert!(composite.start().await.is_err());

        let dai: Box<dyn DeviceControl> = Box::new(HardwareDai(serve_hw_dai(&client)));
        assert!(dai.start().await.is_err());

        let streamconfig: Box<dyn DeviceControl> =
            Box::new(HardwareStreamConfig(serve_hw_streamconfig(&client)));
        assert!(streamconfig.start().await.is_err());

        let registry: Box<dyn DeviceControl> = Box::new(Registry(serve_registry_control(&client)));
        // DAI format must be set before codec is started.
        assert!(registry.set_dai_format(TEST_DAI_FORMAT, Some(TEST_ELEMENT_ID)).await.is_ok());
        assert_matches!(registry.start().await, Ok(TEST_CODEC_START_TIME));
    }

    #[fuchsia::test]
    async fn test_stop() {
        let client = fdomain_local::local_client_empty();
        let codec: Box<dyn DeviceControl> = Box::new(HardwareCodec(serve_hw_codec(&client)));
        assert_matches!(codec.stop().await, Ok(TEST_CODEC_STOP_TIME));

        let composite: Box<dyn DeviceControl> =
            Box::new(HardwareComposite(serve_hw_composite(&client)));
        assert!(composite.stop().await.is_err());

        let dai: Box<dyn DeviceControl> = Box::new(HardwareDai(serve_hw_dai(&client)));
        assert!(dai.stop().await.is_err());

        let streamconfig: Box<dyn DeviceControl> =
            Box::new(HardwareStreamConfig(serve_hw_streamconfig(&client)));
        assert!(streamconfig.stop().await.is_err());

        let registry: Box<dyn DeviceControl> = Box::new(Registry(serve_registry_control(&client)));
        // DAI format must be set before codec is started.
        assert!(registry.set_dai_format(TEST_DAI_FORMAT, Some(TEST_ELEMENT_ID)).await.is_ok());
        // Codec must be started before it is stopped.
        assert_matches!(registry.start().await, Ok(TEST_CODEC_START_TIME));
        assert_matches!(registry.stop().await, Ok(TEST_CODEC_STOP_TIME));
    }

    #[fuchsia::test]
    async fn test_reset() {
        let client = fdomain_local::local_client_empty();
        let codec: Box<dyn DeviceControl> = Box::new(HardwareCodec(serve_hw_codec(&client)));
        assert!(codec.reset().await.is_ok());

        let composite: Box<dyn DeviceControl> =
            Box::new(HardwareComposite(serve_hw_composite(&client)));
        assert!(composite.reset().await.is_ok());

        let dai: Box<dyn DeviceControl> = Box::new(HardwareDai(serve_hw_dai(&client)));
        assert!(dai.reset().await.is_ok());

        let streamconfig: Box<dyn DeviceControl> =
            Box::new(HardwareStreamConfig(serve_hw_streamconfig(&client)));
        assert!(streamconfig.reset().await.is_err());

        let registry: Box<dyn DeviceControl> = Box::new(Registry(serve_registry_control(&client)));
        assert!(registry.reset().await.is_ok());
    }
}
