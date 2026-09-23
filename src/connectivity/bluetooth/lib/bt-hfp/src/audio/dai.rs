// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_hardware_audio::{self as audio, DaiFormat, PcmFormat};
use fidl_fuchsia_media as media;
use fuchsia_audio_dai::{self as dai, DaiAudioDevice, DigitalAudioInterface};
use fuchsia_bluetooth::types::{PeerId, peer_audio_stream_id};
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::stream::BoxStream;
use log::{info, warn};
use media::AudioDeviceEnumeratorProxy;

use crate::audio::{HF_INPUT_UUID, HF_OUTPUT_UUID};
use crate::codec_id::CodecId;
use crate::sco;

use super::{Control, ControlEvent, Error};

/// Control that connects the Audio start / stop to a Digital Audio Interface
/// device, which is added to an AudioCore configured Fuchsia Audio system.  The
/// DAI driver provides the ring buffer and is connected to the controller,
/// providing audio without encoding in-band.  This Control cannot start audio
/// without a call in progress.
pub struct DaiControl {
    output: DaiAudioDevice,
    input: DaiAudioDevice,
    audio_core: media::AudioDeviceEnumeratorProxy,
    active_connection: Option<sco::Connection>,
    sender: Mutex<futures::channel::mpsc::Sender<ControlEvent>>,
}

impl DaiControl {
    pub async fn discover(proxy: AudioDeviceEnumeratorProxy) -> Result<Self, Error> {
        let devices = dai::find_devices().await.or(Err(Error::DiscoveryFailed))?;
        Self::setup(devices, proxy).await
    }

    // Public for tests within the crate
    pub(crate) async fn setup(
        devices: Vec<DigitalAudioInterface>,
        proxy: media::AudioDeviceEnumeratorProxy,
    ) -> Result<Self, Error> {
        let mut input_dai = None;
        let mut output_dai = None;

        for mut device in devices {
            if device.connect().is_err() {
                continue;
            }
            let props = match device.properties().await {
                Err(e) => {
                    warn!("Couldn't find properties for device: {:?}", e);
                    continue;
                }
                Ok(props) => props,
            };
            let dai_input = props.is_input.ok_or(Error::DiscoveryFailed)?;
            if input_dai.is_none() && dai_input {
                let dai_device = DaiAudioDevice::build(device).await.map_err(|e| {
                    warn!("Couldn't build a DAI input audio device: {e:?}");
                    Error::DiscoveryFailed
                })?;
                input_dai = Some(dai_device);
            } else if output_dai.is_none() && !dai_input {
                let dai_device = DaiAudioDevice::build(device).await.map_err(|e| {
                    warn!("Couldn't build a DAI output audio device: {e:?}");
                    Error::DiscoveryFailed
                })?;
                output_dai = Some(dai_device);
            }

            if input_dai.is_some() && output_dai.is_some() {
                return Ok(Self::build(proxy, input_dai.unwrap(), output_dai.unwrap()));
            }
        }

        info!("Couldn't find the correct combination of DAI devices");
        Err(Error::DiscoveryFailed)
    }

    pub fn build(
        audio_core: media::AudioDeviceEnumeratorProxy,
        input: DaiAudioDevice,
        output: DaiAudioDevice,
    ) -> Self {
        // Sender will be overwritten when the stream is taken.
        let (sender, _) = futures::channel::mpsc::channel(0);
        Self { input, output, audio_core, active_connection: None, sender: Mutex::new(sender) }
    }

    fn start_device(
        &mut self,
        peer_id: &PeerId,
        input: bool,
        pcm_format: PcmFormat,
    ) -> Result<(), anyhow::Error> {
        let (uuid, dev) = if input {
            (HF_INPUT_UUID, &mut self.input)
        } else {
            (HF_OUTPUT_UUID, &mut self.output)
        };
        // TODO(b/358666067): make dai-device choose this format
        // Use the first supported format for now.
        let missing_err = format_err!("Couldn't find DAI frame format");
        let Some(supported) = dev.dai_formats().first() else {
            return Err(missing_err);
        };
        let Some(frame_format) = supported.frame_formats.first().cloned() else {
            return Err(missing_err);
        };
        let dai_format = DaiFormat {
            number_of_channels: 1,
            channels_to_use_bitmask: 0x1,
            sample_format: audio::DaiSampleFormat::PcmSigned,
            frame_format,
            frame_rate: pcm_format.frame_rate,
            bits_per_slot: 16,
            bits_per_sample: 16,
        };
        let dev_id = peer_audio_stream_id(*peer_id, uuid);
        dev.config(dai_format, pcm_format)?;
        dev.start(self.audio_core.clone(), "HFP Audio", dev_id, "Fuchsia", "Sapphire HFP Headset")
    }
}

impl Control for DaiControl {
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        _codec: CodecId,
    ) -> Result<(), Error> {
        if self.active_connection.is_some() {
            // Can only handle one connection at once
            return Err(Error::AlreadyStarted);
        }
        // I/O bandwidth is matched to frame rate but includes both source and sink, so the
        // audio frame rate is half that.
        let frame_rate = match connection.params.io_bandwidth {
            32000 => 16000,
            16000 => 8000,
            _ => {
                return Err(Error::UnsupportedParameters {
                    source: format_err!("Unsupported frame_rate"),
                });
            }
        };
        let pcm_format = PcmFormat {
            number_of_channels: 1,
            sample_format: audio::SampleFormat::PcmSigned,
            bytes_per_sample: 2,
            valid_bits_per_sample: 16,
            frame_rate,
        };
        self.start_device(&id, true, pcm_format.clone()).map_err(Error::audio_core)?;
        if let Err(e) = self.start_device(&id, false, pcm_format) {
            // Stop the input device, so we have only two states: started and not started.
            self.input.stop();
            return Err(Error::audio_core(e));
        }
        self.active_connection = Some(connection);
        let _ = self.sender.get_mut().try_send(ControlEvent::Started { id });
        Ok(())
    }

    fn stop(&mut self, id: PeerId) -> Result<(), Error> {
        if !self.active_connection.as_ref().is_some_and(|c| c.peer_id == id) {
            return Err(Error::NotStarted);
        };
        self.active_connection = None;
        self.output.stop();
        self.input.stop();
        let _ = self.sender.get_mut().try_send(ControlEvent::Stopped { id, error: None });
        Ok(())
    }

    fn connect(&mut self, _id: PeerId, _supported_codecs: &[CodecId]) {
        // nothing to do here
    }

    fn disconnect(&mut self, id: PeerId) {
        let _ = self.stop(id);
    }

    fn take_events(&self) -> BoxStream<'static, ControlEvent> {
        let (sender, receiver) = futures::channel::mpsc::channel(1);
        let mut lock = self.sender.lock();
        *lock = sender;
        receiver.boxed()
    }

    fn failed_request(&self, _request: ControlEvent, _error: Error) {
        // We do not send requests, so do nothing here.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl::endpoints::Proxy;
    use fuchsia_async as fasync;
    use futures::SinkExt;
    use futures::channel::mpsc;

    use crate::sco::test_utils::connection_for_codec;

    #[fuchsia::test]
    async fn fails_if_all_devices_not_found() {
        let (proxy, _requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();

        let setup_result = DaiControl::setup(vec![], proxy.clone()).await;
        assert!(matches!(setup_result, Err(Error::DiscoveryFailed)));

        let (input, _handle) = dai::test::test_digital_audio_interface(true);
        let setup_result = DaiControl::setup(vec![input], proxy.clone()).await;
        assert!(matches!(setup_result, Err(Error::DiscoveryFailed)));

        let (output, _handle) = dai::test::test_digital_audio_interface(false);
        let setup_result = DaiControl::setup(vec![output], proxy.clone()).await;
        assert!(matches!(setup_result, Err(Error::DiscoveryFailed)));

        let (input, _handle) = dai::test::test_digital_audio_interface(true);
        let (output, _handle) = dai::test::test_digital_audio_interface(false);
        let _audio = DaiControl::setup(vec![input, output], proxy.clone()).await.unwrap();
    }

    const TEST_PEER: PeerId = PeerId(0);
    const OTHER_PEER: PeerId = PeerId(1);

    #[fuchsia::test]
    async fn starts_dai() {
        let (proxy, audio_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();

        let (send, mut new_client_recv) = mpsc::channel(1);
        let _audio_req_task = fasync::Task::spawn(handle_audio_requests(audio_requests, send));

        let (input, _input_handle) = dai::test::test_digital_audio_interface(true);
        let (output, _output_handle) = dai::test::test_digital_audio_interface(false);
        let mut audio = DaiControl::setup(vec![input, output], proxy.clone()).await.unwrap();
        let mut events = audio.take_events();

        let (connection, _stream) = connection_for_codec(TEST_PEER, CodecId::CVSD, false);
        let result = audio.start(TEST_PEER, connection, CodecId::CVSD);
        result.expect("audio should start okay");

        // Expect new audio devices that are output and input.
        let audio_client_one = new_client_recv.next().await.expect("new audio device");
        let audio_client_two = new_client_recv.next().await.expect("new audio device");
        assert!(audio_client_one.is_input != audio_client_two.is_input, "input and output");

        // A started event should have been delivered to indicate it's started.
        match events.next().await {
            Some(ControlEvent::Started { id }) => assert_eq!(TEST_PEER, id),
            x => panic!("Expected a Started event and got {x:?}"),
        };

        let (connection, mut stream2) = connection_for_codec(TEST_PEER, CodecId::CVSD, false);
        let result = audio.start(TEST_PEER, connection, CodecId::CVSD);
        let _ = result.expect_err("Starting an already started source is an error");
        // Should have dropped the connection since it's already started.
        let request = stream2.next().await;
        assert!(request.is_none());

        // Can't start a connection for another peer either, DaiControl only supports one
        // active peer at a time.
        let (connection, mut stream2) = connection_for_codec(OTHER_PEER, CodecId::CVSD, false);
        let result = audio.start(OTHER_PEER, connection, CodecId::CVSD);
        let _ = result.expect_err("Starting an already started source is an error");
        // Should have dropped the connection since it's already started.
        let request = stream2.next().await;
        assert!(request.is_none());
    }

    #[fuchsia::test]
    async fn stop_dai() {
        let (proxy, audio_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();

        let (send, mut new_client_recv) = mpsc::channel(1);
        let _audio_req_task = fasync::Task::spawn(handle_audio_requests(audio_requests, send));

        let (input, input_handle) = dai::test::test_digital_audio_interface(true);
        let (output, output_handle) = dai::test::test_digital_audio_interface(false);
        let mut audio = DaiControl::setup(vec![input, output], proxy.clone()).await.unwrap();
        let mut events = audio.take_events();

        let _ = audio.stop(OTHER_PEER).expect_err("stopping without starting is an error");

        let (connection, _stream) = connection_for_codec(TEST_PEER, CodecId::CVSD, false);
        let result = audio.start(TEST_PEER, connection, CodecId::CVSD);
        result.expect("audio should start okay");

        // Expect a new audio devices that we can start.
        let audio_client_one = new_client_recv.next().await.expect("new audio device");
        let rb_one = audio_client_one.start_up().await.expect("should be able to start");
        let _start_time = rb_one.start().await.expect("DAI ringbuffer should start okay");
        let audio_client_two = new_client_recv.next().await.expect("new audio device");
        let rb_two = audio_client_two.start_up().await.expect("should be able to start");
        let _start_time = rb_two.start().await.expect("DAI ringbuffer should start okay");

        assert!(output_handle.is_started(), "Output DAI should be started");
        assert!(input_handle.is_started(), "Input DAI should be started");

        // A started event should have been delivered to indicate it's started.
        match events.next().await {
            Some(ControlEvent::Started { id }) => assert_eq!(TEST_PEER, id),
            x => panic!("Expected a Started event and got {x:?}"),
        };

        // Stopping a peer that's not started is an error.
        let _ =
            audio.stop(OTHER_PEER).expect_err("shouldn't be able to stop a peer we didn't start");

        // Stopping should close the audio client.
        audio.stop(TEST_PEER).expect("audio should stop okay");
        let _audio_closed = audio_client_one.client.on_closed().await;
        let _audio_closed = audio_client_two.client.on_closed().await;

        // and send a stopped for successful stops.
        match events.next().await {
            Some(ControlEvent::Stopped { id, error: None }) => {
                assert_eq!(TEST_PEER, id);
            }
            x => panic!("Expected a non-error Stopped event and got {x:?}"),
        };

        let _ = audio.stop(TEST_PEER).expect_err("audio should not be able to stop after stopping");
    }

    struct TestAudioClient {
        _name: String,
        is_input: bool,
        client: audio::StreamConfigProxy,
    }

    impl TestAudioClient {
        async fn start_up(&self) -> Result<audio::RingBufferProxy, fidl::Error> {
            let _prop = self.client.get_properties().await?;
            let formats = self.client.get_supported_formats().await?;
            // Pick the first one, why not.
            let pcm_formats = formats.first().unwrap().pcm_supported_formats.as_ref().unwrap();
            let pcm_format = Some(PcmFormat {
                number_of_channels: pcm_formats.channel_sets.as_ref().unwrap()[0]
                    .attributes
                    .as_ref()
                    .unwrap()
                    .len() as u8,
                sample_format: pcm_formats.sample_formats.as_ref().unwrap()[0],
                bytes_per_sample: pcm_formats.bytes_per_sample.as_ref().unwrap()[0],
                valid_bits_per_sample: pcm_formats.valid_bits_per_sample.as_ref().unwrap()[0],
                frame_rate: pcm_formats.frame_rates.as_ref().unwrap()[0],
            });
            let (proxy, server_end) = fidl::endpoints::create_proxy();
            self.client.create_ring_buffer(
                &audio::Format { pcm_format, ..Default::default() },
                server_end,
            )?;
            Ok(proxy)
        }
    }

    async fn handle_audio_requests(
        mut requests: media::AudioDeviceEnumeratorRequestStream,
        mut stream_proxies: mpsc::Sender<TestAudioClient>,
    ) {
        while let Some(req) = requests.next().await {
            match req.expect("AudioDeviceEnumerator stream error: {:?}") {
                media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    device_name,
                    is_input,
                    channel,
                    ..
                } => {
                    let dev = TestAudioClient {
                        _name: device_name.to_owned(),
                        is_input,
                        client: channel.into_proxy(),
                    };
                    if let Err(e) = stream_proxies.feed(dev).await {
                        panic!("Couldn't send new device: {:?}", e);
                    }
                }
                x => unimplemented!("Got unimplemented AudioDeviceEnumerator: {:?}", x),
            }
        }
    }
}
