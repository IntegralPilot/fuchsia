// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::codec::CodecInterface;
use crate::config::{Config, Device};
use crate::configurator::Configurator;
use crate::dai::DaiInterface;
use crate::indexes::StreamConfigIndex;
use crate::signal::SignalInterface;
use anyhow::{Context, Error, anyhow};
use async_trait::async_trait;
use async_utils::hanging_get::client::HangingGetStream;
use fidl::prelude::*;
use fidl_fuchsia_hardware_audio::*;
use fidl_fuchsia_hardware_audio_signalprocessing::*;
use fidl_fuchsia_media::AudioDeviceEnumeratorMarker;
use fuchsia_async as fasync;
use futures::lock::Mutex;
use futures::{StreamExt, select};
use std::collections::HashMap;
use std::sync::Arc;

pub struct CodecState {
    /// Codec manufacturer name.
    manufacturer: String,

    /// Codec product name.
    product: String,

    /// Formats supported by the Codec driver.
    supported_formats: Vec<DaiSupportedFormats>,

    /// Codec interface in use.
    proxy: CodecProxy,

    /// DAI Channel, for instance TDM slot.
    dai_channel: u8,

    /// Watch plug detect task.
    _plug_detect_task: fasync::Task<()>,
}

pub struct DaiState {
    /// DAI manufacturer name.
    manufacturer: String,

    /// DAI product name.
    product: String,

    /// Formats supported by the DAI driver.
    supported_formats: Vec<DaiSupportedFormats>,

    /// DAI interface in use.
    interface: crate::dai::DaiInterface,
}

pub struct StreamConfigInner {
    /// Have we replied to the very first gain state watch.
    gain_state_first_watch_replied: bool,

    /// Hardwired and hence no plug detect support.
    hardwired: bool,

    /// Current plug state.
    plugged: bool,

    /// Last plugged time.
    plugged_time: i64,

    /// The plug state shas been updated since the last watch reply.
    plug_state_updated: bool,

    /// Responder for plug detects hanging get asynchronous replies.
    plug_state_responder: Option<StreamConfigWatchPlugStateResponder>,

    /// Needs a codec to work.
    codec_needed: bool,

    /// Codec drivers states.
    codec_states: Vec<CodecState>,

    /// DAI driver state.
    dai_state: Option<DaiState>,

    /// DAI format used.
    dai_format: Option<DaiFormat>,

    /// Ring buffer format used.
    ring_buffer_format: Option<Format>,

    /// Ring buffer server end.
    ring_buffer: Option<fidl::endpoints::ServerEnd<RingBufferMarker>>,
}

impl Default for StreamConfigInner {
    fn default() -> StreamConfigInner {
        StreamConfigInner {
            gain_state_first_watch_replied: false,
            hardwired: true,
            // Must reply to the first Watch request, if there is no plug state update before the
            // first Watch, reply with plugged at time 0.
            plugged: true,
            plugged_time: 0,
            // Mark plug state as updated to always reply to first watch.
            // If plugged and plugged_time are not updated reply with default values.
            plug_state_updated: true,
            plug_state_responder: None,
            codec_needed: false,
            codec_states: vec![],
            dai_state: None,
            dai_format: None,
            ring_buffer_format: None,
            ring_buffer: None,
        }
    }
}
pub struct StreamConfig {
    /// Handle to remove the audio device processing future when this is dropped.
    control_handle: StreamConfigControlHandle,

    /// StreamConfig properties.
    properties: StreamProperties,

    /// Stream to handle StreamConfig API protocol.
    stream: StreamConfigRequestStream,

    /// Client end.
    client: Option<fidl::endpoints::ClientEnd<StreamConfigMarker>>,

    /// Inner state.
    inner: Arc<Mutex<StreamConfigInner>>,
}

impl Drop for StreamConfig {
    fn drop(&mut self) {
        self.control_handle.shutdown();
    }
}

impl StreamConfig {
    async fn process_stream_requests(mut self) {
        loop {
            select! {
                stream_config_request = self.stream.next() => {
                    match stream_config_request {
                        Some(Ok(request)) => {
                            if let Err(e) = self.handle_stream_request(request).await {
                                log::warn!("stream config request error: {:?}", e)
                            }
                        },
                        Some(Err(e)) => {
                            log::warn!("stream config error: {:?}, stopping", e);
                        },
                        None => {
                            log::warn!("no stream config error");
                        },
                    }
                }
                complete => break,
            }
        }
    }

    async fn handle_stream_request(
        &mut self,
        request: StreamConfigRequest,
    ) -> std::result::Result<(), anyhow::Error> {
        match request {
            StreamConfigRequest::GetHealthState { responder } => {
                log::trace!("StreamConfig get health state");
                responder.send(&HealthState::default())?;
            }

            StreamConfigRequest::SignalProcessingConnect { protocol: _, control_handle } => {
                log::trace!("StreamConfig signal processing connect");
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
            }

            StreamConfigRequest::GetProperties { responder } => {
                log::trace!("StreamConfig properties: {:?}", self.properties);
                responder.send(&self.properties)?;
            }

            StreamConfigRequest::GetSupportedFormats { responder } => {
                let inner = self.inner.lock().await;
                let formats = match inner
                    .dai_state
                    .as_ref()
                    .ok_or_else(|| anyhow!("DAI state must be present before Stream Config"))?
                    .interface
                    .get_ring_buffer_formats()
                    .await
                {
                    Err(e) => {
                        log::warn!("Couldn't get DAI ring buffer formats for device: {:?}", e);
                        responder.send(&[])?;
                        return Ok(());
                    }
                    Ok(formats) => formats.map_err(|e| anyhow!(e.to_string()))?,
                };
                log::trace!("StreamConfig ring buffer formats: {:?}", formats);
                responder.send(&formats)?;
            }

            StreamConfigRequest::CreateRingBuffer { format, ring_buffer, control_handle: _ } => {
                let mut inner = self.inner.lock().await;
                log::trace!(
                    "StreamConfig create ring buffer DAI format: {:?}  ring buffer format: {:?}",
                    inner.dai_format,
                    format
                );
                inner.ring_buffer_format = Some(format.clone());
                inner.ring_buffer = Some(ring_buffer);
                StreamConfig::try_to_create_ring_buffer(&mut inner).await?;
            }

            StreamConfigRequest::WatchGainState { responder } => {
                log::trace!("StreamConfig watch gain state");
                let mut state = self.inner.lock().await;
                if state.gain_state_first_watch_replied == true {
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
                state.gain_state_first_watch_replied = true;
                responder.send(&gain_state)?
            }

            StreamConfigRequest::WatchPlugState { responder } => {
                let mut state = self.inner.lock().await;

                // plug_state_time can't be any later than "right now"
                let plug_time =
                    std::cmp::min(state.plugged_time, zx::MonotonicInstant::get().into_nanos());
                let plug_state = fidl_fuchsia_hardware_audio::PlugState {
                    plugged: Some(state.plugged),
                    plug_state_time: Some(plug_time),
                    ..Default::default()
                };
                log::trace!("StreamConfig watch plug state: {:?}", plug_state.plugged);
                if state.plug_state_updated {
                    state.plug_state_updated = false;
                    responder.send(&plug_state)?;
                    return Ok(());
                } else if state.plug_state_responder.is_none() {
                    state.plug_state_responder = Some(responder);
                    return Ok(());
                } else {
                    log::warn!("Client watched plug state when another hanging get was pending");
                    // We drop responder which causes a shutdown (no call to drop_without_shutdown).
                }
            }

            StreamConfigRequest::SetGain { target_state: _, control_handle: _ } => {
                // We ignore this API since we report no gain change support.
                log::trace!("Set gain state");
            }
        }
        Ok(())
    }

    // Not an error if we can't create the ring buffer because not all preconditions are met.
    async fn try_to_create_ring_buffer(
        stream_config_state: &mut StreamConfigInner,
    ) -> Result<(), Error> {
        // Create a ring buffer from the DAI if the ring buffer was already requested and we have
        // DAI state, codec state, a common DAI format and a ring buffer format.
        if let (Some(ring_buffer), Some(dai_state), Some(dai_format), Some(ring_buffer_format)) = (
            stream_config_state.ring_buffer.take(),
            stream_config_state.dai_state.as_ref(),
            stream_config_state.dai_format,
            stream_config_state.ring_buffer_format.as_ref(),
        ) {
            log::info!("DAI {:?} {:?} setup", dai_state.manufacturer, dai_state.product);
            let _ = dai_state
                .interface
                .create_ring_buffer(dai_format, ring_buffer_format.clone(), ring_buffer)
                .await?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct DefaultConfiguratorInner {
    /// Indexes to the available StreamConfigs not found yet.
    stream_config_available_indexes: HashMap<Device, Vec<StreamConfigIndex>>,

    /// States for each StreamConfigs.
    stream_config_states: HashMap<StreamConfigIndex, Arc<Mutex<StreamConfigInner>>>,
}

/// This configurator uses the first element of the DAI formats reported by the codec and
/// configures one channel to be used per codec.
pub struct DefaultConfigurator {
    /// Inner state.
    inner: Arc<Mutex<DefaultConfiguratorInner>>,

    /// Stream Configs to serve.
    stream_configs: Vec<StreamConfig>,
}

impl DefaultConfigurator {
    pub fn common_dai_format(
        formats1: &Vec<DaiSupportedFormats>,
        formats2: &Vec<DaiSupportedFormats>,
    ) -> Option<DaiFormat> {
        // TODO(https://fxbug.dev/42177443): Add heuristics and configurability for the decision of what DAI formats
        // to use when there is more than one match here..
        for i in formats1 {
            for j in formats2 {
                // All these values must be set before we say we found the common dai_format.
                let mut dai_format = DaiFormat {
                    number_of_channels: 0,
                    channels_to_use_bitmask: 0,
                    sample_format: DaiSampleFormat::PcmSigned,
                    frame_format: DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                    frame_rate: 0,
                    bits_per_sample: 0,
                    bits_per_slot: 0,
                };
                let mut found = false;
                for k in &i.number_of_channels {
                    if j.number_of_channels.contains(k) {
                        dai_format.number_of_channels = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                // Default to use all channels.
                dai_format.channels_to_use_bitmask = (1 << dai_format.number_of_channels) - 1;
                found = false;
                for k in &i.sample_formats {
                    if j.sample_formats.contains(k) {
                        dai_format.sample_format = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                found = false;
                for k in &i.frame_formats {
                    if j.frame_formats.contains(k) {
                        dai_format.frame_format = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                found = false;
                for k in &i.frame_rates {
                    if j.frame_rates.contains(k) {
                        dai_format.frame_rate = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                found = false;
                for k in &i.bits_per_sample {
                    if j.bits_per_sample.contains(k) {
                        dai_format.bits_per_sample = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                found = false;
                for k in &i.bits_per_slot {
                    if j.bits_per_slot.contains(k) {
                        dai_format.bits_per_slot = *k;
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
                log::trace!("Found common DAI format {:?}", dai_format);
                return Some(dai_format);
            }
        }
        None
    }
    async fn set_dai_format(
        proxy: &CodecProxy,
        mut codec_dai_format: DaiFormat,
        dai_channel: u8,
        manufacturer: &str,
        product: &str,
    ) -> Result<(), Error> {
        // Enable only the DAI channel as channel to use in the codec.
        // TODO(https://fxbug.dev/42177443): Add flexibility instead of only allowing one DAI channel per codec.
        let bitmask: u64 = 1 << (dai_channel as u32 % codec_dai_format.number_of_channels);
        codec_dai_format.channels_to_use_bitmask = bitmask;
        log::debug!(
            "Setting Codec {:?} {:?} to DAI format {:?}",
            manufacturer,
            product,
            codec_dai_format
        );
        if let Err(e) = proxy.set_dai_format(&codec_dai_format).await {
            return Err(anyhow!("Error when setting the DAI format: {:?}", e));
        }
        Ok(())
    }

    async fn watch_plug_detect(
        proxy: CodecProxy,
        stream_config_state: Arc<Mutex<StreamConfigInner>>,
    ) {
        let mut stream =
            HangingGetStream::new(proxy.clone(), CodecProxyInterface::watch_plug_state);
        loop {
            let plug_detect = stream.next().await;
            let mut stream_config_state = stream_config_state.lock().await;
            let plug_detect = match plug_detect {
                Some(v) => v,
                None => {
                    log::warn!("Watch stream got no plug state");
                    break;
                }
            };
            match plug_detect {
                Ok(v) => {
                    stream_config_state.plugged = match v.plugged {
                        Some(v) => v,
                        None => {
                            log::warn!("Plug state from codec with no plugged field");
                            break;
                        }
                    };
                    stream_config_state.plugged_time = if stream_config_state.hardwired {
                        0
                    } else {
                        match v.plug_state_time {
                            Some(v) => v,
                            None => {
                                log::warn!("Plug state from codec with no plug_state_time field");
                                break;
                            }
                        }
                    };
                }
                Err(e) => {
                    log::warn!("Error getting plug state from codec: {:?}", e);
                    break;
                }
            }
            if let Some(responder) = stream_config_state.plug_state_responder.take() {
                let plug_state = fidl_fuchsia_hardware_audio::PlugState {
                    plugged: Some(stream_config_state.plugged),
                    plug_state_time: Some(stream_config_state.plugged_time),
                    ..Default::default()
                };
                match responder.send(&plug_state) {
                    Ok(()) => continue,
                    Err(e) => {
                        log::warn!("Could not respond to plug state: {:?}", e);
                        continue;
                    }
                };
            } else {
                stream_config_state.plug_state_updated = true;
            }
        }
        log::warn!("Exiting watch plug detect");
    }
}

#[async_trait]
impl Configurator for DefaultConfigurator {
    fn new(config: Config) -> Result<Self, Error> {
        // Create a StreamConfigInner state for each index found in the configuration.
        let mut states = HashMap::new();
        for (device, indexes) in &config.stream_config_indexes {
            for index in indexes {
                if !states.contains_key(index) {
                    states
                        .insert(index.clone(), Arc::new(Mutex::new(StreamConfigInner::default())));
                }
                // If there is a device that is not hardwired then make the stream not hardwired.
                if !device.hardwired {
                    if let Some(state) = states.get_mut(&index) {
                        state.try_lock().expect("Must exist").hardwired = false;
                    }
                }
                // If there is at least one codec, we mark the StreamConfig as codec_needed.
                if device.is_codec {
                    if let Some(state) = states.get_mut(&index) {
                        state.try_lock().expect("Must exist").codec_needed = true;
                    }
                }
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(DefaultConfiguratorInner {
                stream_config_states: states,
                stream_config_available_indexes: config.stream_config_indexes,
            })),
            stream_configs: vec![],
        })
    }

    async fn process_new_codec(&mut self, mut interface: CodecInterface) -> Result<(), Error> {
        let _ = interface.connect().context("Couldn't connect to codec")?;

        // Get codec properties.
        let properties = interface.get_properties().await?;

        // Reset the codec.
        let _ = interface.reset().await?;

        // Set the default gain state for the codec.
        let mut signal = SignalInterface::new();
        let proxy = interface.get_proxy()?;
        let _ = signal.connect_codec(proxy)?;
        match signal.get_elements().await {
            Ok(elements) => {
                for e in elements {
                    if let Some(id) = e.id {
                        if let Some(element_type) = e.type_ {
                            if element_type == ElementType::Gain {
                                let state = SettableElementState {
                                    type_specific: Some(SettableTypeSpecificElementState::Gain(
                                        GainElementState {
                                            gain: Some(0.0f32),
                                            ..Default::default()
                                        },
                                    )),
                                    ..Default::default()
                                };
                                signal.set_element_state(id, state).await?;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                // We allow to continue if the Signal Processing API is not supported.
                log::debug!("Couldn't get elements from signal processing: {:?}", e)
            }
        }

        let plug_detect_capabilities = properties
            .plug_detect_capabilities
            .ok_or_else(|| anyhow!("Codec with no plug detect capabilities"))?;

        let inner = self.inner.clone();
        let mut inner = inner.lock().await;

        // Get the codec formats.
        let codec_formats =
            interface.get_dai_formats().await?.map_err(|e| anyhow!(e.to_string()))?;
        if codec_formats.len() == 0
            || codec_formats[0].number_of_channels.len() == 0
            || codec_formats[0].sample_formats.len() == 0
            || codec_formats[0].frame_formats.len() == 0
            || codec_formats[0].frame_rates.len() == 0
            || codec_formats[0].bits_per_sample.len() == 0
            || codec_formats[0].bits_per_slot.len() == 0
        {
            return Err(anyhow!("Codec with bad format reported"));
        }
        log::debug!("Codec properties {:?} formats {:?}", properties, codec_formats);
        // Use an empty string if no manufacturer reported.
        let mut manufacturer = "".to_string();
        if let Some(value) = properties.manufacturer {
            manufacturer = value;
        }
        // Use an empty string if no product reported.
        let mut product = "".to_string();
        if let Some(value) = properties.product {
            product = value;
        }
        let mut is_input = false;
        if let Some(value) = properties.is_input {
            is_input = value;
        }

        // Update the stream config state for this codec.
        let device = Device {
            manufacturer: manufacturer,
            product: product,
            hardwired: plug_detect_capabilities
                != fidl_fuchsia_hardware_audio::PlugDetectCapabilities::CanAsyncNotify,
            is_codec: true,
            is_input: is_input,
        };
        let stream_config_indexes = inner
            .stream_config_available_indexes
            .get(&device)
            .ok_or_else(|| anyhow!("Codec ({:?}) not in config", device))?
            .clone();
        let stream_config_index = stream_config_indexes
            .last()
            .ok_or_else(|| anyhow!("Codec index ({:?}) not in config", device))?;

        // Use DAI channel matching the StreamConfigIndex used.
        // TODO(https://fxbug.dev/42177443): Add configurability for the DAI channel in addition to this heuristic.
        let dai_channel = (stream_config_indexes.len() - 1) as u8;

        let stream_config_state = inner
            .stream_config_states
            .get_mut(&stream_config_index)
            .ok_or_else(|| anyhow!("Codec state ({:?}) not found", device))?
            .clone();
        let mut stream_config_state2 = stream_config_state.lock().await;

        // Use codec's hardwired state for the StreamConfig.
        stream_config_state2.hardwired = device.hardwired;

        // If the DAI already got its formats, lets check we have a match.
        if let Some(dai_state) = &stream_config_state2.dai_state {
            if let Some(common_format) =
                DefaultConfigurator::common_dai_format(&codec_formats, &dai_state.supported_formats)
            {
                stream_config_state2.dai_format = Some(common_format.clone());
                DefaultConfigurator::set_dai_format(
                    interface.get_proxy()?,
                    common_format,
                    dai_channel,
                    &device.manufacturer,
                    &device.product,
                )
                .await?;

                StreamConfig::try_to_create_ring_buffer(&mut stream_config_state2).await?;
            } else {
                log::warn!(
                    "Codec ({:?} {:?}) formats ({:?}) not found in DAI ({:?} {:?}) formats ({:?})",
                    device.manufacturer,
                    device.product,
                    codec_formats,
                    dai_state.manufacturer,
                    dai_state.product,
                    dai_state.supported_formats
                );
            }
        } else {
            log::debug!("When codec was found, there was no format reported by the DAI yet");
        }

        let proxy = interface.get_proxy()?.clone();
        let stream_config_state_clone = stream_config_state.clone();
        // TODO(https://fxbug.dev/42177443): Improve handing of misbehaving codecs for instance dropping the
        // corresponding StreamConfigs.
        let task = fasync::Task::spawn(async move {
            DefaultConfigurator::watch_plug_detect(proxy, stream_config_state_clone).await;
        });

        let proxy = interface.get_proxy()?.clone();
        let codec_state = CodecState {
            manufacturer: device.manufacturer.clone(),
            product: device.product.clone(),
            supported_formats: codec_formats.clone(),
            proxy: proxy,
            dai_channel: dai_channel,
            _plug_detect_task: task,
        };
        stream_config_state2.codec_states.push(codec_state);

        // All good we can now remove the device from the available indexes to match.
        _ = inner
            .stream_config_available_indexes
            .get_mut(&device)
            .ok_or_else(|| anyhow!("Codec ({:?}) not in config", device))?
            .pop()
            .ok_or_else(|| anyhow!("Codec index ({:?}) not in config", device))?;

        Ok(())
    }

    async fn process_new_dai(&mut self, mut interface: DaiInterface) -> Result<(), Error> {
        let _ = interface.connect().context("Couldn't connect to DAI")?;
        let (client, request_stream) =
            fidl::endpoints::create_request_stream::<StreamConfigMarker>();
        let dai_properties = interface.get_properties().await?;
        let mut inner = self.inner.lock().await;
        // Use an empty string if no manufacturer reported.
        let mut manufacturer = "".to_string();
        if let Some(value) = dai_properties.manufacturer {
            manufacturer = value;
        }
        // Use an empty string if no product reported.
        let mut product = "".to_string();
        if let Some(value) = dai_properties.product_name {
            product = value;
        }
        let mut is_input = false;
        if let Some(value) = dai_properties.is_input {
            is_input = value;
        }
        let device = Device {
            manufacturer: manufacturer,
            product: product,
            hardwired: true, // Set all DAIs to hardwired.
            is_codec: false,
            is_input: is_input,
        };
        let indexes = inner
            .stream_config_available_indexes
            .get(&device)
            .ok_or_else(|| anyhow!("DAI ({:?}) not in config", device))?
            .clone();
        let index =
            indexes.last().ok_or_else(|| anyhow!("DAI index ({:?}) not in config", device))?;
        let stream_config_state = inner
            .stream_config_states
            .get_mut(&index)
            .ok_or_else(|| anyhow!("DAI state ({:?}) not found", device))?
            .clone();
        let mut stream_config_state2 = stream_config_state.lock().await;

        let _ = interface.reset().await?;
        let dai_formats = interface.get_dai_formats().await?.map_err(|e| anyhow!(e.to_string()))?;
        if dai_formats.len() == 0
            || dai_formats[0].number_of_channels.len() == 0
            || dai_formats[0].sample_formats.len() == 0
            || dai_formats[0].frame_formats.len() == 0
            || dai_formats[0].frame_rates.len() == 0
            || dai_formats[0].bits_per_sample.len() == 0
            || dai_formats[0].bits_per_slot.len() == 0
        {
            return Err(anyhow!("DAI with bad format reported"));
        }
        log::debug!("DAI {:?} {:?} formats {:?}", device.manufacturer, device.product, dai_formats);

        let dai_state = DaiState {
            manufacturer: device.manufacturer.clone(),
            product: device.product.clone(),
            supported_formats: dai_formats.clone(),
            interface: interface,
        };
        stream_config_state2.dai_state = Some(dai_state);

        // If a codec is not needed lets go ahead and create the ring buffer
        if !stream_config_state2.codec_needed {
            // Pick the first format.
            let dai_format = DaiFormat {
                number_of_channels: dai_formats[0].number_of_channels[0],
                channels_to_use_bitmask: 0,
                sample_format: dai_formats[0].sample_formats[0],
                frame_format: dai_formats[0].frame_formats[0],
                frame_rate: dai_formats[0].frame_rates[0],
                bits_per_sample: dai_formats[0].bits_per_sample[0],
                bits_per_slot: dai_formats[0].bits_per_slot[0],
            };
            stream_config_state2.dai_format = Some(dai_format);
            StreamConfig::try_to_create_ring_buffer(&mut stream_config_state2).await?;
        } else {
            // If the codecs already got state, lets check if we have a formats match.
            // We only request one ring buffer for all codecs in a stream config, but we
            // do configure all codecs via set_dai_format.
            for i in 0..stream_config_state2.codec_states.len() {
                let codec_state = &mut stream_config_state2.codec_states[i];
                if let Some(common_format) = DefaultConfigurator::common_dai_format(
                    &dai_formats,
                    &codec_state.supported_formats,
                ) {
                    DefaultConfigurator::set_dai_format(
                        &codec_state.proxy,
                        common_format.clone(),
                        codec_state.dai_channel,
                        &codec_state.manufacturer,
                        &codec_state.product,
                    )
                    .await?;

                    stream_config_state2.dai_format = Some(common_format.clone());

                    // Only one of these calls will actually create a ring buffer for a
                    // given Stream Config.
                    StreamConfig::try_to_create_ring_buffer(&mut stream_config_state2).await?;
                } else {
                    log::warn!(
                        "DAI ({:?} {:?}) formats ({:?}) not found in\
                             Codec ({:?} {:?}) formats ({:?})",
                        device.manufacturer,
                        device.product,
                        dai_formats,
                        codec_state.manufacturer,
                        codec_state.product,
                        codec_state.supported_formats
                    );
                }
            }
        }

        let configurator_product = "Driver Configurator for ".to_string() + &device.product;
        let stream_properties = StreamProperties {
            unique_id: Some(index.id),
            is_input: Some(
                dai_properties.is_input.ok_or_else(|| anyhow!("No is_input in DAI properties"))?,
            ),
            can_mute: Some(false),
            can_agc: Some(false),
            min_gain_db: Some(0f32),
            max_gain_db: Some(0f32),
            gain_step_db: Some(0f32),
            plug_detect_capabilities: Some(if stream_config_state2.hardwired {
                fidl_fuchsia_hardware_audio::PlugDetectCapabilities::Hardwired
            } else {
                fidl_fuchsia_hardware_audio::PlugDetectCapabilities::CanAsyncNotify
            }),
            clock_domain: Some(fidl_fuchsia_hardware_audio::CLOCK_DOMAIN_MONOTONIC),
            manufacturer: Some("Google".to_string()),
            product: Some(configurator_product),
            ..Default::default()
        };
        let control_handle = request_stream.control_handle().clone();
        let stream_config = StreamConfig {
            control_handle: control_handle,
            properties: stream_properties,
            stream: request_stream,
            inner: stream_config_state.clone(),
            client: Some(client),
        };
        self.stream_configs.push(stream_config);

        // All good we can now remove the device from the available indexes to match.
        _ = inner
            .stream_config_available_indexes
            .get_mut(&device)
            .ok_or_else(|| anyhow!("DAI ({:?}) not in config", device))?
            .pop()
            .ok_or_else(|| anyhow!("DAI index ({:?}) not in config", device))?;

        Ok(())
    }

    fn serve_interface(&mut self) -> Result<Vec<fasync::Task<()>>, Error> {
        let mut tasks = Vec::new();
        while let Some(mut stream_config) = self.stream_configs.pop() {
            let client = stream_config.client.take().ok_or_else(|| anyhow!("Must have client"))?;
            let svc =
                fuchsia_component::client::connect_to_protocol::<AudioDeviceEnumeratorMarker>()
                    .context("Failed to connect to AudioDeviceEnumerator")?;
            svc.add_device_by_channel(
                &stream_config
                    .properties
                    .product
                    .as_ref()
                    .ok_or_else(|| anyhow!("Must have product"))?,
                stream_config.properties.is_input.ok_or_else(|| anyhow!("Must have is_input"))?,
                client,
            )?;
            tasks.push(fasync::Task::spawn(stream_config.process_stream_requests()));
        }
        if tasks.len() == 0 {
            return Err(anyhow!("No Stream Configs to serve"));
        }
        return Ok(tasks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{find_codecs, find_dais};
    use crate::indexes::{
        STREAM_CONFIG_INDEX_HEADSET_OUT, STREAM_CONFIG_INDEX_MICS, STREAM_CONFIG_INDEX_SPEAKERS,
    };
    use crate::testing::tests::get_dev_proxy;
    use anyhow::Result;
    use assert_matches::assert_matches;
    use fixture::fixture;
    use futures::future;
    use futures::task::Poll;

    // Integration tests using //src/media/audio/drivers/testing/realm devices.

    #[fuchsia::test]
    #[ignore]
    async fn test_default_configurator_devices_found() -> Result<()> {
        let (_realm_instance, codec_proxy) = get_dev_proxy("class/codec").await?;
        let (_realm_instance, dai_proxy) = get_dev_proxy("class/dai").await?;
        let mut config = Config::new()?;
        // Codec 1.
        config.load_device(
            Device {
                manufacturer: "456".to_string(),
                product: "789".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );
        // Codec 2.
        config.load_device(
            Device {
                manufacturer: "def".to_string(),
                product: "ghi".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );
        // DAI.
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "test".to_string(),
                hardwired: true,
                is_codec: false,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );

        let configurator = Arc::new(Mutex::new(DefaultConfigurator::new(config)?));
        if let Err(e) = find_codecs(&codec_proxy, 2, configurator.clone()).await {
            // One of the test drivers reports bad formats.
            assert_eq!(e.to_string(), "Codec processing error: Codec with bad format reported");
        }
        assert_matches!(find_dais(&dai_proxy, 1, configurator).await, Ok(()));

        Ok(())
    }

    #[fuchsia::test]
    #[ignore]
    async fn test_default_configurator_devices_not_found() -> Result<()> {
        let (_realm_instance, codec_proxy) = get_dev_proxy("class/codec").await?;
        let (_realm_instance, dai_proxy) = get_dev_proxy("class/dai").await?;
        let mut config = Config::new()?;
        config.load_device(
            Device {
                manufacturer: "error".to_string(),
                product: "e".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: true,
            },
            STREAM_CONFIG_INDEX_MICS,
        );
        config.load_device(
            Device {
                manufacturer: "error".to_string(),
                product: "e".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_HEADSET_OUT,
        );
        config.load_device(
            Device {
                manufacturer: "error".to_string(),
                product: "e".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );
        let configurator = Arc::new(Mutex::new(DefaultConfigurator::new(config)?));
        if let Err(e) = find_codecs(&codec_proxy, 2, configurator.clone()).await {
            // Driver discovery order is not deterministic, hence multiple errors may be returned
            // by find_codecs depending which codec is found first.
            // One of the test drivers reports bad formats, if found first its bad format error is
            // returned.
            // Another test driver reports good formats but it won't be found in the devices loaded
            // above.
            assert!(
                (&e.to_string()
                    == "Codec processing error: Codec (Device { manufacturer: \"456\", product: \"789\", \
                 is_codec: true, hardwired: true, is_input: false }) not in config")
                    || (&e.to_string() == "Codec processing error: Codec with bad format reported")
            );
        }
        if let Err(e) = find_dais(&dai_proxy, 1, configurator).await {
            // One of the test drivers reports bad formats.
            assert_eq!(
                e.to_string(),
                "DAI processing error: DAI (Device { manufacturer: \"test\", product: \"test\", is\
                 _codec: false, hardwired: true, is_input: false }) not in config"
            );
        }
        Ok(())
    }

    #[fuchsia::test]
    #[ignore]
    async fn test_default_configurator_create_ring_buffer() -> Result<()> {
        let (_realm_instance, dai_proxy) = get_dev_proxy("class/dai").await?;
        let mut config = Config::new()?;
        // One DAI only, no codecs needed.
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "test".to_string(),
                is_codec: false,
                hardwired: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );

        let configurator = Arc::new(Mutex::new(DefaultConfigurator::new(config)?));
        assert_matches!(find_dais(&dai_proxy, 1, configurator.clone()).await, Ok(()));

        let configurator = configurator.clone();
        let configurator = configurator.lock();
        let stream_configs = &mut configurator.await.stream_configs;
        let mut stream_config = stream_configs.pop().expect("Must have a Stream Config");
        let pcm_format = PcmFormat {
            number_of_channels: 2,
            sample_format: SampleFormat::PcmSigned,
            frame_rate: 48_000,
            valid_bits_per_sample: 16,
            bytes_per_sample: 2,
        };
        let ring_buffer_format = Format { pcm_format: Some(pcm_format), ..Default::default() };
        let (_client, server) = fidl::endpoints::create_endpoints::<RingBufferMarker>();
        let proxy = stream_config.client.take().expect("Must have a client").into_proxy();
        let _task = fasync::Task::spawn(stream_config.process_stream_requests());
        proxy.create_ring_buffer(&ring_buffer_format, server)?;

        // To make sure we really complete running create ring buffer we call a 2-way method.
        let _props = proxy.get_properties().await?;

        Ok(())
    }

    #[fuchsia::test]
    #[ignore]
    async fn test_default_configurator_create_ring_buffer_with_codec() -> Result<()> {
        let (_realm_instance, dai_proxy) = get_dev_proxy("class/dai").await?;
        let (_realm_instance, codec_proxy) = get_dev_proxy("class/codec").await?;
        let mut config = Config::new()?;
        // Codec.
        config.load_device(
            Device {
                manufacturer: "456".to_string(),
                product: "789".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );
        // DAI.
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "test".to_string(),
                hardwired: true,
                is_codec: false,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );

        let configurator = Arc::new(Mutex::new(DefaultConfigurator::new(config)?));
        if let Err(e) = find_codecs(&codec_proxy, 2, configurator.clone()).await {
            // One of the test drivers reports bad formats.
            assert_eq!(e.to_string(), "Codec processing error: Codec with bad format reported");
        }
        assert_matches!(find_dais(&dai_proxy, 1, configurator.clone()).await, Ok(()));

        let configurator = configurator.clone();
        let configurator = configurator.lock();
        let stream_configs = &mut configurator.await.stream_configs;
        let mut stream_config = stream_configs.pop().expect("Must have a Stream Config");
        let pcm_format = PcmFormat {
            number_of_channels: 2,
            sample_format: SampleFormat::PcmSigned,
            frame_rate: 48_000,
            valid_bits_per_sample: 16,
            bytes_per_sample: 2,
        };
        let ring_buffer_format = Format { pcm_format: Some(pcm_format), ..Default::default() };
        let (_client, server) = fidl::endpoints::create_endpoints::<RingBufferMarker>();
        let proxy = stream_config.client.take().expect("Must have a client").into_proxy();
        let _task = fasync::Task::spawn(stream_config.process_stream_requests());
        proxy.create_ring_buffer(&ring_buffer_format, server)?;

        // To make sure we really complete running create ring buffer we call a 2-way method.
        let _props = proxy.get_properties().await?;

        Ok(())
    }

    // Unit tests.

    #[fuchsia::test]
    fn test_common_dai_format() -> Result<()> {
        // Empty vectors return none.
        let mut all_formats_left: Vec<DaiSupportedFormats> = vec![];
        let mut all_formats_right: Vec<DaiSupportedFormats> = vec![];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_none());

        // One format in both side returns the format.
        let formats_left = DaiSupportedFormats {
            number_of_channels: vec![2],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S)],
            frame_rates: vec![48000],
            bits_per_sample: vec![24],
            bits_per_slot: vec![32],
        };
        all_formats_left.push(formats_left.clone());
        all_formats_right.push(formats_left.clone());
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_some());
        assert!(common.unwrap().number_of_channels == formats_left.number_of_channels[0]);
        assert!(common.unwrap().sample_format == formats_left.sample_formats[0]);
        assert!(common.unwrap().frame_format == formats_left.frame_formats[0]);
        assert!(common.unwrap().frame_rate == formats_left.frame_rates[0]);
        assert!(common.unwrap().bits_per_sample == formats_left.bits_per_sample[0]);
        assert!(common.unwrap().bits_per_slot == formats_left.bits_per_slot[0]);

        // A format non-matching returns none.
        let formats_right = DaiSupportedFormats {
            number_of_channels: vec![8],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S)],
            frame_rates: vec![48000],
            bits_per_sample: vec![24],
            bits_per_slot: vec![32],
        };
        all_formats_left = vec![formats_left.clone()];
        all_formats_right = vec![formats_right];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_none());

        // Matching format found. Left only advertises one format.
        let formats_right = DaiSupportedFormats {
            number_of_channels: vec![2, 4, 6, 8],
            sample_formats: vec![DaiSampleFormat::PcmUnsigned, DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoLeft),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoRight),
            ],
            frame_rates: vec![16000, 24000, 48000, 96000],
            bits_per_sample: vec![16, 24, 32],
            bits_per_slot: vec![16, 32],
        };
        all_formats_left = vec![formats_left.clone()];
        all_formats_right = vec![formats_right];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_some());
        assert!(common.unwrap().number_of_channels == formats_left.number_of_channels[0]);
        assert!(common.unwrap().sample_format == formats_left.sample_formats[0]);
        assert!(common.unwrap().frame_format == formats_left.frame_formats[0]);
        assert!(common.unwrap().frame_rate == formats_left.frame_rates[0]);
        assert!(common.unwrap().bits_per_sample == formats_left.bits_per_sample[0]);
        assert!(common.unwrap().bits_per_slot == formats_left.bits_per_slot[0]);

        // Matching format found. Right only advertises one format.
        let formats_right = formats_left;
        let formats_left = DaiSupportedFormats {
            number_of_channels: vec![2, 4, 6, 8],
            sample_formats: vec![DaiSampleFormat::PcmUnsigned, DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoLeft),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoRight),
            ],
            frame_rates: vec![16000, 24000, 48000, 96000],
            bits_per_sample: vec![16, 24, 32],
            bits_per_slot: vec![16, 32],
        };
        all_formats_left = vec![formats_left];
        all_formats_right = vec![formats_right.clone()];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_some());
        assert!(common.unwrap().number_of_channels == formats_right.number_of_channels[0]);
        assert!(common.unwrap().sample_format == formats_right.sample_formats[0]);
        assert!(common.unwrap().frame_format == formats_right.frame_formats[0]);
        assert!(common.unwrap().frame_rate == formats_right.frame_rates[0]);
        assert!(common.unwrap().bits_per_sample == formats_right.bits_per_sample[0]);
        assert!(common.unwrap().bits_per_slot == formats_right.bits_per_slot[0]);

        // Matching format found. Both sides have multiple formats.
        let formats_right = DaiSupportedFormats {
            number_of_channels: vec![2, 4, 8],
            sample_formats: vec![DaiSampleFormat::PcmUnsigned, DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoRight),
            ],
            frame_rates: vec![16000, 24000, 48000],
            bits_per_sample: vec![16, 24, 32],
            bits_per_slot: vec![16, 32],
        };
        let formats_left = DaiSupportedFormats {
            number_of_channels: vec![1, 2, 3],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoLeft),
            ],
            frame_rates: vec![48000, 96000],
            bits_per_sample: vec![20, 24],
            bits_per_slot: vec![32],
        };
        all_formats_left = vec![formats_left];
        all_formats_right = vec![formats_right];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_some());
        assert!(common.unwrap().number_of_channels == 2);
        assert!(common.unwrap().sample_format == DaiSampleFormat::PcmSigned);
        assert!(
            common.unwrap().frame_format
                == DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S)
        );
        assert!(common.unwrap().frame_rate == 48000);
        assert!(common.unwrap().bits_per_sample == 24);
        assert!(common.unwrap().bits_per_slot == 32);

        // Matching format found.
        // Both sides have vectors with multiple entries and each entry has multiple values formats.
        let formats_right1 = DaiSupportedFormats {
            number_of_channels: vec![8],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S)],
            frame_rates: vec![48000],
            bits_per_sample: vec![32],
            bits_per_slot: vec![16, 32],
        };
        let formats_right2 = DaiSupportedFormats {
            number_of_channels: vec![2, 4, 8],
            sample_formats: vec![DaiSampleFormat::PcmUnsigned, DaiSampleFormat::PcmUnsigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoRight),
            ],
            frame_rates: vec![16000, 24000, 48000],
            bits_per_sample: vec![16, 24, 32],
            bits_per_slot: vec![16, 32],
        };
        let formats_left1 = DaiSupportedFormats {
            number_of_channels: vec![1],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoLeft),
            ],
            frame_rates: vec![48000, 96000],
            bits_per_sample: vec![20, 24],
            bits_per_slot: vec![32],
        };
        let formats_left2 = DaiSupportedFormats {
            number_of_channels: vec![1, 2, 3],
            sample_formats: vec![DaiSampleFormat::PcmSigned],
            frame_formats: vec![
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
                DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoLeft),
            ],
            frame_rates: vec![48000],
            bits_per_sample: vec![16, 20, 24],
            bits_per_slot: vec![16, 32],
        };
        let formats_left3 = DaiSupportedFormats {
            number_of_channels: vec![1, 2, 3],
            sample_formats: vec![DaiSampleFormat::PcmUnsigned],
            frame_formats: vec![DaiFrameFormat::FrameFormatStandard(
                DaiFrameFormatStandard::StereoRight,
            )],
            frame_rates: vec![48000],
            bits_per_sample: vec![16],
            bits_per_slot: vec![16],
        };
        all_formats_left = vec![formats_left1, formats_left2, formats_left3];
        all_formats_right = vec![formats_right1, formats_right2];
        let common = DefaultConfigurator::common_dai_format(&all_formats_left, &all_formats_right);
        assert!(common.is_some());
        assert!(common.unwrap().number_of_channels == 2);
        assert!(common.unwrap().sample_format == DaiSampleFormat::PcmUnsigned);
        assert!(
            common.unwrap().frame_format
                == DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::StereoRight)
        );
        assert!(common.unwrap().frame_rate == 48000);
        assert!(common.unwrap().bits_per_sample == 16);
        assert!(common.unwrap().bits_per_slot == 16);

        Ok(())
    }

    fn with_stream_config_stream<F>(_name: &str, test: F)
    where
        F: FnOnce(fasync::TestExecutor, StreamConfig) -> (),
    {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let (client, request_stream) =
            fidl::endpoints::create_request_stream::<StreamConfigMarker>();
        let control_handle = request_stream.control_handle().clone();
        let mut stream_config_state: StreamConfigInner = Default::default();
        stream_config_state.dai_state = None;
        let dai_format = DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 3,
            sample_format: DaiSampleFormat::PcmSigned,
            frame_format: DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
            frame_rate: 48_000,
            bits_per_sample: 24,
            bits_per_slot: 32,
        };
        stream_config_state.dai_format = Some(dai_format);
        let stream_properties = StreamProperties {
            unique_id: Some(STREAM_CONFIG_INDEX_MICS.id),
            is_input: Some(false),
            can_mute: Some(false),
            can_agc: Some(false),
            min_gain_db: Some(0f32),
            max_gain_db: Some(0f32),
            gain_step_db: Some(0f32),
            plug_detect_capabilities: Some(
                fidl_fuchsia_hardware_audio::PlugDetectCapabilities::Hardwired,
            ),
            clock_domain: Some(fidl_fuchsia_hardware_audio::CLOCK_DOMAIN_MONOTONIC),
            manufacturer: Some("Test Manufacturer".to_string()),
            product: Some("Test Product".to_string()),
            ..Default::default()
        };
        let stream_config = StreamConfig {
            control_handle: control_handle,
            properties: stream_properties,
            stream: request_stream,
            client: Some(client),
            inner: Arc::new(Mutex::new(stream_config_state)),
        };
        test(exec, stream_config)
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_drop(mut exec: fasync::TestExecutor, mut stream_config: StreamConfig) {
        let client = stream_config.client.take().expect("Must have client");
        drop(stream_config);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future::pending::<()>()));
        // The audio client should be dropped.
        assert_eq!(Err(zx::Status::PEER_CLOSED), client.channel().write(&[0], &mut Vec::new()));
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_get_properties(
        mut exec: fasync::TestExecutor,
        mut stream_config: StreamConfig,
    ) {
        let client = stream_config.client.take().expect("Must have client");
        let proxy = client.into_proxy();
        let _task = fasync::Task::spawn(stream_config.process_stream_requests());
        let properties = match exec.run_until_stalled(&mut proxy.get_properties()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from get_properties, got: {:?}", x),
        };
        assert_eq!(properties.unique_id, Some(STREAM_CONFIG_INDEX_MICS.id));
        assert_eq!(properties.is_input, Some(false));
        assert_eq!(properties.can_mute, Some(false));
        assert_eq!(properties.can_agc, Some(false));
        assert_eq!(properties.min_gain_db, Some(0f32));
        assert_eq!(properties.max_gain_db, Some(0f32));
        assert_eq!(properties.gain_step_db, Some(0f32));
        assert_eq!(
            properties.plug_detect_capabilities,
            Some(fidl_fuchsia_hardware_audio::PlugDetectCapabilities::Hardwired)
        );
        assert_eq!(properties.clock_domain, Some(0u32));
        assert_eq!(properties.manufacturer, Some("Test Manufacturer".to_string()));
        assert_eq!(properties.product, Some("Test Product".to_string()));
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_watch_gain(
        mut exec: fasync::TestExecutor,
        mut stream_config: StreamConfig,
    ) {
        let client = stream_config.client.take().expect("Must have client");
        let proxy = client.into_proxy();
        let _task = fasync::Task::spawn(stream_config.process_stream_requests());
        let gain_state = match exec.run_until_stalled(&mut proxy.watch_gain_state()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from watch gain state, got: {:?}", x),
        };
        assert_eq!(gain_state.muted, Some(false));
        assert_eq!(gain_state.agc_enabled, Some(false));
        assert_eq!(gain_state.gain_db, Some(0.0f32));
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_watch_plug(
        mut exec: fasync::TestExecutor,
        mut stream_config: StreamConfig,
    ) {
        let client = stream_config.client.take().expect("Must have client");
        let proxy = client.into_proxy();
        let _task = fasync::Task::spawn(stream_config.process_stream_requests());
        let plug_state = match exec.run_until_stalled(&mut proxy.watch_plug_state()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from watch plug state, got: {:?}", x),
        };
        assert_eq!(plug_state.plugged, Some(true));
        assert_eq!(plug_state.plug_state_time, Some(0i64));
    }

    const TEST_CODEC_PLUGGED: bool = false;
    const TEST_CODEC_PLUG_STATE_TIME: i64 = 123i64;
    const TEST_CODEC_GAIN_PROCESSING_ELEMENT_ID: u64 = 1u64;
    const TEST_CODEC_GAIN: f32 = 0.0f32;

    pub struct TestCodec {
        /// Stream to handle Codec API protocol.
        codec_stream: CodecRequestStream,

        /// Stream to handle Signal Processing API protocol.
        signal_stream: Option<SignalProcessingRequestStream>,

        /// Last gain set.
        gain: Option<f32>,

        /// Is input.
        is_input: bool,
    }

    impl TestCodec {
        async fn process_codec_and_signal_requests(mut self) {
            loop {
                select! {
                    request = self.codec_stream.next() => {
                        match request {
                            Some(Ok(request)) => {
                                if let Err(e) = self.handle_codec_request(request, true).await {
                                    log::warn!("codec request error: {:?}", e)
                                }
                            },
                            Some(Err(e)) => {
                                log::warn!("codec error: {:?}, stopping", e);
                            },
                            None => {
                                log::warn!("no codec error");
                            },
                        }
                    },
                    request =
                        self.signal_stream.as_mut().expect("Must have signal stream").next() => {
                        match request {
                            Some(Ok(request)) => {
                                if let Err(e) = self.handle_signal_request(request).await {
                                    log::warn!("signal processing request error: {:?}", e)
                                }
                            },
                            Some(Err(e)) => {
                                log::warn!("signal processing error: {:?}, stopping", e);
                            },
                            None => {
                                log::warn!("no signal processing error");
                            },
                        }
                    },
                    complete => break,
                }
            }
        }

        async fn handle_codec_request(
            &mut self,
            request: CodecRequest,
            with_signal: bool,
        ) -> std::result::Result<(), anyhow::Error> {
            match request {
                CodecRequest::Reset { responder } => {
                    responder.send()?;
                }
                CodecRequest::GetProperties { responder } => {
                    let info = CodecProperties {
                        unique_id: Some([0; fidl_fuchsia_hardware_audio::UNIQUE_ID_SIZE as usize]),
                        manufacturer: Some("test".to_string()),
                        product: Some("testy".to_string()),
                        is_input: Some(self.is_input),
                        plug_detect_capabilities: Some(
                            fidl_fuchsia_hardware_audio::PlugDetectCapabilities::Hardwired,
                        ),
                        ..Default::default()
                    };
                    responder.send(&info)?;
                }
                CodecRequest::Stop { responder: _ } => {}
                CodecRequest::Start { responder: _ } => {}
                CodecRequest::GetDaiFormats { responder } => {
                    let formats = &[DaiSupportedFormats {
                        number_of_channels: vec![2],
                        sample_formats: vec![DaiSampleFormat::PcmSigned],
                        frame_formats: vec![DaiFrameFormat::FrameFormatStandard(
                            DaiFrameFormatStandard::I2S,
                        )],
                        frame_rates: vec![48000],
                        bits_per_sample: vec![24],
                        bits_per_slot: vec![32],
                    }];
                    responder.send(Ok(formats))?;
                }
                CodecRequest::SetDaiFormat { responder: _, format: _ } => {}
                CodecRequest::WatchPlugState { responder } => {
                    responder.send(&fidl_fuchsia_hardware_audio::PlugState {
                        plugged: Some(TEST_CODEC_PLUGGED),
                        plug_state_time: Some(TEST_CODEC_PLUG_STATE_TIME),
                        ..Default::default()
                    })?;
                }
                CodecRequest::SignalProcessingConnect { protocol, control_handle } => {
                    if with_signal {
                        self.signal_stream = Some(protocol.into_stream());
                    } else {
                        control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                    }
                }

                r => panic!("{:?} Not covered by test", r),
            }
            Ok(())
        }

        async fn handle_signal_request(
            &mut self,
            request: SignalProcessingRequest,
        ) -> std::result::Result<(), anyhow::Error> {
            match request {
                SignalProcessingRequest::GetElements { responder } => {
                    let pe = Element {
                        id: Some(TEST_CODEC_GAIN_PROCESSING_ELEMENT_ID),
                        type_: Some(ElementType::Gain),
                        type_specific: Some(TypeSpecificElement::Gain(Gain {
                            type_: Some(
                                fidl_fuchsia_hardware_audio_signalprocessing::GainType::Decibels,
                            ),
                            min_gain: Some(0.0f32),
                            max_gain: Some(0.0f32),
                            min_gain_step: Some(0.0f32),
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    responder.send(Ok(&[pe]))?;
                }
                SignalProcessingRequest::SetElementState {
                    processing_element_id,
                    state,
                    responder,
                } => {
                    if processing_element_id == TEST_CODEC_GAIN_PROCESSING_ELEMENT_ID {
                        match state.type_specific {
                            Some(type_specific) => match type_specific {
                                SettableTypeSpecificElementState::Gain(gain) => {
                                    self.gain = gain.gain;
                                    return Ok(responder.send(Ok(()))?);
                                }
                                _ => panic!("Must be of type gain"),
                            },
                            _ => panic!("Must have gain"),
                        }
                    }
                    panic!("Not covered by test");
                }
                SignalProcessingRequest::WatchElementState { processing_element_id, responder } => {
                    if processing_element_id == TEST_CODEC_GAIN_PROCESSING_ELEMENT_ID {
                        let state = ElementState {
                            type_specific: Some(TypeSpecificElementState::Gain(GainElementState {
                                gain: Some(TEST_CODEC_GAIN),
                                ..Default::default()
                            })),
                            ..Default::default()
                        };
                        return Ok(responder.send(&state)?);
                    }
                    panic!("Not covered by test");
                }
                r => panic!("{:?} Not covered by test", r),
            }
            Ok(())
        }
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_watch_plug_with_codec(
        mut exec: fasync::TestExecutor,
        mut stream_config: StreamConfig,
    ) {
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let (_signal_client, signal_stream) =
            fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
        let codec = TestCodec {
            codec_stream: codec_stream,
            signal_stream: Some(signal_stream),
            gain: None,
            is_input: false,
        };
        let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
        let stream_config_inner = stream_config.inner.clone();
        let _codec_plug_detect_task = fasync::Task::spawn(async move {
            DefaultConfigurator::watch_plug_detect(codec_client.into_proxy(), stream_config_inner)
                .await;
        });

        {
            let stream_config_inner = stream_config.inner.clone();
            let mut inner = match exec.run_until_stalled(&mut stream_config_inner.lock()) {
                Poll::Ready(v) => v,
                Poll::Pending => panic!("Expected Ready Ok from stream config inne, got Pending"),
            };
            inner.hardwired = false;
        }

        let stream_config_client = stream_config.client.take().expect("Must have client");
        let proxy = stream_config_client.into_proxy();
        let _stream_config_task = fasync::Task::spawn(stream_config.process_stream_requests());

        // First get plugged with time 0 since there is no plug state before this first watch.
        let plug_state = match exec.run_until_stalled(&mut proxy.watch_plug_state()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from watch plug state, got: {:?}", x),
        };
        assert_eq!(plug_state.plugged, Some(true));
        assert_eq!(plug_state.plug_state_time, Some(0i64));

        // Then get unplugged from the TestCodec.
        let plug_state = match exec.run_until_stalled(&mut proxy.watch_plug_state()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from watch plug state, got: {:?}", x),
        };
        assert_eq!(plug_state.plugged, Some(TEST_CODEC_PLUGGED));
        assert_eq!(plug_state.plug_state_time, Some(TEST_CODEC_PLUG_STATE_TIME));
    }

    #[fuchsia::test]
    async fn test_default_configurator_find_codecs_check_dai_channel() -> Result<()> {
        let mut config = Config::new()?;
        let number_of_codecs = 5;
        for _ in 0..number_of_codecs {
            config.load_device(
                Device {
                    manufacturer: "test".to_string(),
                    product: "testy".to_string(),
                    hardwired: true,
                    is_codec: true,
                    is_input: false,
                },
                STREAM_CONFIG_INDEX_SPEAKERS,
            );
        }
        let mut configurator = DefaultConfigurator::new(config)?;
        for _ in 0..number_of_codecs {
            let (codec_client, codec_stream) =
                fidl::endpoints::create_request_stream::<CodecMarker>();
            let (_signal_client, signal_stream) =
                fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
            let codec = TestCodec {
                codec_stream: codec_stream,
                signal_stream: Some(signal_stream),
                gain: None,
                is_input: false,
            };
            let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
            let codec_proxy = codec_client.into_proxy();
            let codec_interface = CodecInterface::new_with_proxy(codec_proxy);
            configurator.process_new_codec(codec_interface).await?;
        }
        let inner = configurator.inner.lock().await;
        let codec_state =
            inner.stream_config_states.get(&STREAM_CONFIG_INDEX_SPEAKERS).unwrap().lock().await;

        assert_eq!(codec_state.codec_states[0].dai_channel, 4);
        assert_eq!(codec_state.codec_states[1].dai_channel, 3);
        assert_eq!(codec_state.codec_states[2].dai_channel, 2);
        assert_eq!(codec_state.codec_states[3].dai_channel, 1);
        assert_eq!(codec_state.codec_states[4].dai_channel, 0);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_default_configurator_find_codecs_check_input_output_codecs() -> Result<()> {
        let mut config = Config::new()?;

        // 1 output device.
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "testy".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );

        // 2 input devices.
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "testy".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: true,
            },
            STREAM_CONFIG_INDEX_MICS,
        );
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "testy".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: true,
            },
            STREAM_CONFIG_INDEX_MICS,
        );

        let mut configurator = DefaultConfigurator::new(config)?;

        // Add output device.
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let (_signal_client, signal_stream) =
            fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
        let codec = TestCodec {
            codec_stream: codec_stream,
            signal_stream: Some(signal_stream),
            gain: None,
            is_input: false, // Make the device to be of type output.
        };
        let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
        let codec_proxy = codec_client.into_proxy();
        let codec_interface = CodecInterface::new_with_proxy(codec_proxy);
        configurator.process_new_codec(codec_interface).await?;

        // Add input device 1.
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let (_signal_client, signal_stream) =
            fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
        let codec = TestCodec {
            codec_stream: codec_stream,
            signal_stream: Some(signal_stream),
            gain: None,
            is_input: true, // Make the device to be of type input.
        };
        let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
        let codec_proxy = codec_client.into_proxy();
        let codec_interface = CodecInterface::new_with_proxy(codec_proxy);
        configurator.process_new_codec(codec_interface).await?;

        // Add input device 2.
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let (_signal_client, signal_stream) =
            fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
        let codec = TestCodec {
            codec_stream: codec_stream,
            signal_stream: Some(signal_stream),
            gain: None,
            is_input: true, // Make the device to be of type input.
        };
        let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
        let codec_proxy = codec_client.into_proxy();
        let codec_interface = CodecInterface::new_with_proxy(codec_proxy);
        configurator.process_new_codec(codec_interface).await?;

        // Check states
        let inner = configurator.inner.lock().await;
        let codec_speakers_state =
            inner.stream_config_states.get(&STREAM_CONFIG_INDEX_SPEAKERS).unwrap().lock().await;
        assert_eq!(codec_speakers_state.codec_states[0].dai_channel, 0);
        let codec_mics_state =
            inner.stream_config_states.get(&STREAM_CONFIG_INDEX_MICS).unwrap().lock().await;
        assert_eq!(codec_mics_state.codec_states[0].dai_channel, 1);
        assert_eq!(codec_mics_state.codec_states[1].dai_channel, 0);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_default_configurator_find_codec_with_signal_processing() -> Result<()> {
        let mut config = Config::new()?;
        config.load_device(
            Device {
                manufacturer: "test".to_string(),
                product: "testy".to_string(),
                hardwired: true,
                is_codec: true,
                is_input: false,
            },
            STREAM_CONFIG_INDEX_SPEAKERS,
        );
        let mut configurator = DefaultConfigurator::new(config)?;
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let (_signal_client, signal_stream) =
            fidl::endpoints::create_request_stream::<SignalProcessingMarker>();
        let codec = TestCodec {
            codec_stream: codec_stream,
            signal_stream: Some(signal_stream),
            gain: None,
            is_input: false,
        };
        let _codec_task = fasync::Task::spawn(codec.process_codec_and_signal_requests());
        let codec_proxy = codec_client.into_proxy();
        let codec_interface = CodecInterface::new_with_proxy(codec_proxy);
        let proxy = codec_interface.get_proxy()?.clone();
        configurator.process_new_codec(codec_interface).await?;

        let mut signal = SignalInterface::new();
        let _ = signal.connect_codec(&proxy)?;
        let elements = signal.get_elements().await?;
        for e in elements {
            if let Some(id) = e.id {
                if let Some(element_type) = e.type_ {
                    if element_type == ElementType::Gain {
                        let state = signal.watch_element_state(id).await?;
                        match state.type_specific {
                            Some(type_specific) => match type_specific {
                                TypeSpecificElementState::Gain(gain) => {
                                    assert_eq!(gain.gain, Some(TEST_CODEC_GAIN));
                                    return Ok(());
                                }
                                _ => panic!("Must be of type gain"),
                            },
                            _ => panic!("Must have gain"),
                        }
                    }
                }
            }
        }
        panic!("Must have found gain");
    }

    pub struct TestCodecBad {
        /// Stream to handle Codec API protocol.
        codec_stream: CodecRequestStream,
    }

    impl TestCodecBad {
        async fn process_codec_requests(mut self) {
            loop {
                select! {
                    request = self.codec_stream.next() => {
                        match request {
                            Some(Ok(request)) => {
                                if let Err(e) = self.handle_codec_request(request).await {
                                    log::warn!("codec request error: {:?}", e)
                                }
                            },
                            Some(Err(e)) => {
                                log::warn!("codec error: {:?}, stopping", e);
                            },
                            None => {
                                log::warn!("no codec error");
                            },
                        }
                    }
                    complete => break,
                }
            }
        }

        async fn handle_codec_request(
            &mut self,
            request: CodecRequest,
        ) -> std::result::Result<(), anyhow::Error> {
            match request {
                CodecRequest::WatchPlugState { responder } => {
                    responder.send(&fidl_fuchsia_hardware_audio::PlugState {
                        // A plug state with missing plugged field is bad.
                        plug_state_time: Some(TEST_CODEC_PLUG_STATE_TIME),
                        ..Default::default()
                    })?;
                }
                r => panic!("{:?} Not covered by test", r),
            }
            Ok(())
        }
    }

    #[fixture(with_stream_config_stream)]
    #[fuchsia::test]
    fn test_stream_config_watch_plug_with_bad_codec(
        mut exec: fasync::TestExecutor,
        mut stream_config: StreamConfig,
    ) {
        let (codec_client, codec_stream) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let codec = TestCodecBad { codec_stream: codec_stream };
        let _codec_task = fasync::Task::spawn(codec.process_codec_requests());
        let stream_config_inner = stream_config.inner.clone();
        let _codec_plug_detect_task = fasync::Task::spawn(async move {
            DefaultConfigurator::watch_plug_detect(codec_client.into_proxy(), stream_config_inner)
                .await;
        });

        {
            let stream_config_inner = stream_config.inner.clone();
            let mut inner = match exec.run_until_stalled(&mut stream_config_inner.lock()) {
                Poll::Ready(v) => v,
                Poll::Pending => panic!("Expected Ready Ok from stream config inne, got Pending"),
            };
            inner.hardwired = false;
        }

        let stream_config_client = stream_config.client.take().expect("Must have client");
        let proxy = stream_config_client.into_proxy();
        let _stream_config_task = fasync::Task::spawn(stream_config.process_stream_requests());

        let plug_state = match exec.run_until_stalled(&mut proxy.watch_plug_state()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected Ready Ok from watch plug state, got: {:?}", x),
        };
        assert_eq!(plug_state.plugged, Some(true));
        assert_eq!(plug_state.plug_state_time, Some(0i64));

        // Then never respond because our test codec is bad.
        match exec.run_until_stalled(&mut proxy.watch_plug_state()) {
            Poll::Pending => {}
            x => panic!("Expected Pending from watch plug state, got: {:?}", x),
        }
    }
}
