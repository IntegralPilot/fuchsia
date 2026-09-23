// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_media as media;
use fuchsia_async as fasync;
use fuchsia_audio_codec::{StreamProcessor, StreamProcessorOutputStream};
use fuchsia_audio_device::stream_config::SoftStreamConfig;
use fuchsia_audio_device::{AudioFrameSink, AudioFrameStream, AudioStreamItem};
use fuchsia_bluetooth::types::{PeerId, peer_audio_stream_id};
use fuchsia_sync::Mutex;
use futures::stream::BoxStream;
use futures::task::Context;
use futures::{AsyncWriteExt, FutureExt, StreamExt};
use log::{error, info, warn};
use media::AudioDeviceEnumeratorProxy;
use std::pin::pin;

use crate::audio::{Control, ControlEvent, Error, HF_INPUT_UUID, HF_OUTPUT_UUID};
use crate::codec_id::CodecId;
use crate::sco;

/// Audio Control for inband audio, i.e. encoding and decoding audio before sending them
/// to the controller via HCI (in contrast to offloaded audio).
pub struct InbandControl {
    audio_core: media::AudioDeviceEnumeratorProxy,
    session_task: Option<(PeerId, fasync::Task<()>)>,
    event_sender: Mutex<futures::channel::mpsc::Sender<ControlEvent>>,
    stream: Mutex<Option<futures::channel::mpsc::Receiver<ControlEvent>>>,
}

// Setup for a running AudioSession.
// AudioSesison::run() consumes the session and should handle the data path in both directions:
//   - SCO -> decoder -> audio_core input (audio_frame_sink)
//   - audio_core output -> encoder -> SCO
struct AudioSession {
    audio_frame_sink: AudioFrameSink,
    audio_frame_stream: AudioFrameStream,
    sco: sco::Connection,
    codec: CodecId,
    decoder: StreamProcessor,
    encoder: StreamProcessor,
    event_sender: futures::channel::mpsc::Sender<ControlEvent>,
}

impl AudioSession {
    fn setup(
        connection: sco::Connection,
        codec: CodecId,
        audio_frame_sink: AudioFrameSink,
        audio_frame_stream: AudioFrameStream,
        event_sender: futures::channel::mpsc::Sender<ControlEvent>,
    ) -> Result<Self, Error> {
        if !codec.is_supported() {
            return Err(Error::UnsupportedParameters {
                source: format_err!("unsupported codec {codec}"),
            });
        }
        let decoder = StreamProcessor::create_decoder(codec.mime_type()?, Some(codec.oob_bytes()))
            .map_err(|e| Error::audio_core(format_err!("creating decoder: {e:?}")))?;
        let encoder = StreamProcessor::create_encoder(codec.try_into()?, codec.try_into()?)
            .map_err(|e| Error::audio_core(format_err!("creating encoder: {e:?}")))?;
        Ok(Self {
            sco: connection,
            decoder,
            encoder,
            audio_frame_sink,
            audio_frame_stream,
            codec,
            event_sender,
        })
    }

    async fn encoder_to_sco(
        mut encoded_stream: StreamProcessorOutputStream,
        proxy: bredr::ScoConnectionProxy,
        codec: CodecId,
    ) -> Error {
        // Pre-allocate the packet vector and reuse to avoid allocating for every packet.
        let packet: Vec<u8> = vec![0; 60]; // SCO has 60 byte packets
        let mut request =
            bredr::ScoConnectionWriteRequest { data: Some(packet), ..Default::default() };

        const MSBC_ENCODED_LEN: usize = 57; // Length of a MSBC packet after encoding.
        if codec == CodecId::MSBC {
            let packet: &mut [u8] = request.data.as_mut().unwrap().as_mut_slice();
            packet[0] = 0x01; // H2 header has a constant part (0b1000_0000_0001_AABB) with AABB
            // cycling 0000, 0011, 1100, 1111
        }
        // The H2 Header marker cycle, with the constant part
        let mut h2_marker = [0x08u8, 0x38, 0xc8, 0xf8].iter().cycle();
        loop {
            match encoded_stream.next().await {
                Some(Ok(encoded)) => {
                    if codec == CodecId::MSBC {
                        if encoded.len() % MSBC_ENCODED_LEN != 0 {
                            warn!("Got {} bytes, uneven number of packets", encoded.len());
                        }
                        for sbc_packet in encoded.as_slice().chunks_exact(MSBC_ENCODED_LEN) {
                            let packet: &mut [u8] = request.data.as_mut().unwrap().as_mut_slice();
                            packet[1] = *h2_marker.next().unwrap();
                            packet[2..59].copy_from_slice(sbc_packet);
                            if let Err(e) = proxy.write(&request).await {
                                return e.into();
                            }
                        }
                    } else {
                        // CVSD has no padding or header. Encoder sends us multiples of 60 bytes as
                        // long as we provide a multiple of 7.5ms audio packets.
                        for cvsd_packet in encoded.as_slice().chunks_exact(60) {
                            let packet: &mut [u8] = request.data.as_mut().unwrap().as_mut_slice();
                            packet.copy_from_slice(cvsd_packet);
                            if let Err(e) = proxy.write(&request).await {
                                return e.into();
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    warn!("Error in encoding: {e:?}");
                    return Error::audio_core(format_err!("Couldn't read encoded: {e:?}"));
                }
                None => {
                    warn!("Error in encoding: Stream is ended!");
                    return Error::audio_core(format_err!("Encoder stream ended early"));
                }
            }
        }
    }

    async fn pcm_to_encoder(mut encoder: StreamProcessor, mut stream: AudioFrameStream) -> Error {
        loop {
            match stream.next().await {
                Some(Ok(AudioStreamItem::Data(pcm))) => {
                    if let Err(e) = encoder.write_all(pcm.as_slice()).await {
                        return Error::audio_core(format_err!("write to encoder: {e:?}"));
                    }
                    // Packets should be exactly the right size.
                    if let Err(e) = encoder.flush().await {
                        return Error::audio_core(format_err!("flush encoder: {e:?}"));
                    }
                }
                Some(Ok(AudioStreamItem::AudioDisabled)) => {
                    continue;
                }
                Some(Err(e)) => {
                    warn!("Audio output error: {e:?}");
                    return Error::audio_core(format_err!("output error: {e:?}"));
                }
                None => {
                    warn!("Ran out of audio input!");
                    return Error::audio_core(format_err!("Audio input end"));
                }
            }
        }
    }

    async fn decoder_to_pcm(
        mut decoded_stream: StreamProcessorOutputStream,
        mut sink: AudioFrameSink,
    ) -> Error {
        let mut decoded_packets = 0;
        loop {
            match decoded_stream.next().await {
                Some(Ok(decoded)) => {
                    decoded_packets += 1;
                    if decoded_packets % 500 == 0 {
                        info!(
                            "Got {} decoded bytes from decoder: {decoded_packets} packets",
                            decoded.len()
                        );
                    }
                    if let Err(e) = sink.write_all(decoded.as_slice()).await {
                        warn!("Error sending to sink: {e:?}");
                        return Error::audio_core(format_err!("send to sink: {e:?}"));
                    }
                }
                Some(Err(e)) => {
                    warn!("Error in decoding: {e:?}");
                    return Error::audio_core(format_err!("Couldn't read decoder: {e:?}"));
                }
                None => {
                    warn!("Error in decoding: Stream is ended!");
                    return Error::audio_core(format_err!("Decoder stream ended early"));
                }
            }
        }
    }

    async fn sco_to_decoder(
        proxy: bredr::ScoConnectionProxy,
        mut decoder: StreamProcessor,
        codec: CodecId,
    ) -> Error {
        loop {
            let data = match proxy.read().await {
                Ok(bredr::ScoConnectionReadResponse { data: Some(data), .. }) => data,
                Ok(_) => return Error::audio_core(format_err!("Invalid Read response")),
                Err(e) => return e.into(),
            };
            let packet = match codec {
                CodecId::CVSD => data.as_slice(),
                CodecId::MSBC => {
                    // H2 Header (two octets) is present on packets when WBS is used
                    let (_header, packet) = data.as_slice().split_at(2);
                    if packet[0] != 0xad {
                        info!(
                            "Packet didn't start with syncword: {:#02x} {}",
                            packet[0],
                            packet.len()
                        );
                    }
                    packet
                }
                _ => {
                    return Error::UnsupportedParameters {
                        source: format_err!("Unknown CodecId: {codec:?}"),
                    };
                }
            };
            if let Err(e) = decoder.write_all(packet).await {
                return Error::audio_core(format_err!("Failed to write to decoder: {e:?}"));
            }
            // TODO(https://fxbug.dev/42073275): buffer some packets before flushing instead of flushing on
            // every one.
            if let Err(e) = decoder.flush().await {
                return Error::audio_core(format_err!("Failed to flush decoder: {e:?}"));
            }
        }
    }

    async fn run(mut self) {
        let peer_id = self.sco.peer_id;
        let Ok(encoded_stream) = self.encoder.take_output_stream() else {
            error!("Couldn't take encoder output stream");
            return;
        };
        let sco_write =
            AudioSession::encoder_to_sco(encoded_stream, self.sco.proxy.clone(), self.codec);
        let sco_write = pin!(sco_write);
        let audio_to_encoder = AudioSession::pcm_to_encoder(self.encoder, self.audio_frame_stream);
        let audio_to_encoder = pin!(audio_to_encoder);

        let Ok(decoded_stream) = self.decoder.take_output_stream() else {
            error!("Couldn't take decoder output stream");
            return;
        };
        let decoder_to_sink =
            pin!(AudioSession::decoder_to_pcm(decoded_stream, self.audio_frame_sink));
        let sco_read =
            AudioSession::sco_to_decoder(self.sco.proxy.clone(), self.decoder, self.codec);
        let sco_read = pin!(sco_read);
        let e = futures::select! {
            e = audio_to_encoder.fuse() => { warn!(e:?; "PCM to encoder write"); e},
            e = sco_write.fuse() => { warn!(e:?; "Write encoded to SCO"); e},
            e = sco_read.fuse() => { warn!(e:?; "SCO read to decoder"); e},
            e = decoder_to_sink.fuse() => { warn!(e:?; "SCO decoder to PCM"); e},
        };
        let _ = self.event_sender.try_send(ControlEvent::Stopped { id: peer_id, error: Some(e) });
    }

    fn start(self) -> fasync::Task<()> {
        fasync::Task::spawn(self.run())
    }
}

impl InbandControl {
    pub fn create(proxy: AudioDeviceEnumeratorProxy) -> Result<Self, Error> {
        let (sender, receiver) = futures::channel::mpsc::channel(1);
        Ok(Self {
            audio_core: proxy,
            session_task: None,
            event_sender: Mutex::new(sender),
            stream: Mutex::new(Some(receiver)),
        })
    }

    fn running_id(&mut self) -> Option<PeerId> {
        self.session_task
            .as_mut()
            .and_then(|(running, task)| {
                let mut cx = Context::from_waker(&std::task::Waker::noop());
                // We are the only thing that polls this task, so we are ok to poll it and throw away a
                // wake.
                task.poll_unpin(&mut cx).is_pending().then_some(running)
            })
            .copied()
    }

    const LOCAL_MONOTONIC_CLOCK_DOMAIN: u32 = 0;

    // This is currently 2x an SCO frame which holds 7.5ms
    // This must be a multiple of 7.5ms for the CVSD encoder to not have any remainder bytes.
    const AUDIO_BUFFER_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(15);

    fn start_input(&mut self, peer_id: PeerId, codec_id: CodecId) -> Result<AudioFrameSink, Error> {
        let audio_dev_id = peer_audio_stream_id(peer_id, HF_INPUT_UUID);
        let (client, sink) = SoftStreamConfig::create_input(
            &audio_dev_id,
            "Fuchsia",
            super::DEVICE_NAME,
            Self::LOCAL_MONOTONIC_CLOCK_DOMAIN,
            codec_id.try_into()?,
            Self::AUDIO_BUFFER_DURATION,
        )
        .map_err(|e| Error::audio_core(format_err!("Couldn't create input: {e:?}")))?;

        self.audio_core.add_device_by_channel(super::DEVICE_NAME, true, client)?;
        Ok(sink)
    }

    fn start_output(
        &mut self,
        peer_id: PeerId,
        codec_id: CodecId,
    ) -> Result<AudioFrameStream, Error> {
        let audio_dev_id = peer_audio_stream_id(peer_id, HF_OUTPUT_UUID);
        let (client, stream) = SoftStreamConfig::create_output(
            &audio_dev_id,
            "Fuchsia",
            super::DEVICE_NAME,
            Self::LOCAL_MONOTONIC_CLOCK_DOMAIN,
            codec_id.try_into()?,
            Self::AUDIO_BUFFER_DURATION,
            zx::MonotonicDuration::from_millis(0),
        )
        .map_err(|e| Error::audio_core(format_err!("Couldn't create output: {e:?}")))?;
        self.audio_core.add_device_by_channel(super::DEVICE_NAME, false, client)?;
        Ok(stream)
    }
}

impl Control for InbandControl {
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        codec: CodecId,
    ) -> Result<(), Error> {
        if let Some(running) = self.running_id() {
            if running == id {
                return Err(Error::AlreadyStarted);
            }
            return Err(Error::UnsupportedParameters {
                source: format_err!("Only one peer can be started inband at once"),
            });
        }
        let frame_sink = self.start_input(id, codec)?;
        let frame_stream = self.start_output(id, codec)?;
        let session = AudioSession::setup(
            connection,
            codec,
            frame_sink,
            frame_stream,
            self.event_sender.lock().clone(),
        )?;
        self.session_task = Some((id, session.start()));
        Ok(())
    }

    fn stop(&mut self, id: PeerId) -> Result<(), Error> {
        if self.running_id() != Some(id) {
            return Err(Error::NotStarted);
        }
        self.session_task = None;
        let _ = self.event_sender.get_mut().try_send(ControlEvent::Stopped { id, error: None });
        Ok(())
    }

    fn connect(&mut self, _id: PeerId, _supported_codecs: &[CodecId]) {
        // Nothing to do here
    }

    fn disconnect(&mut self, id: PeerId) {
        let _ = self.stop(id);
    }

    fn take_events(&self) -> BoxStream<'static, ControlEvent> {
        self.stream.lock().take().unwrap().boxed()
    }

    fn failed_request(&self, _request: ControlEvent, _error: Error) {
        // We send no requests, so ignore this.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl_fuchsia_bluetooth_bredr::ScoConnectionRequestStream;

    use crate::sco::test_utils::connection_for_codec;

    /// A "Zero input response" SBC packet.  This is what SBC encodes to (with the MSBC settings)
    /// when passed a flat input at zero.  Each packet represents 7.5 milliseconds of audio.
    const ZERO_INPUT_SBC_PACKET: [u8; 60] = [
        0x80, 0x10, 0xad, 0x00, 0x00, 0xc5, 0x00, 0x00, 0x00, 0x00, 0x77, 0x6d, 0xb6, 0xdd, 0xdb,
        0x6d, 0xb7, 0x76, 0xdb, 0x6d, 0xdd, 0xb6, 0xdb, 0x77, 0x6d, 0xb6, 0xdd, 0xdb, 0x6d, 0xb7,
        0x76, 0xdb, 0x6d, 0xdd, 0xb6, 0xdb, 0x77, 0x6d, 0xb6, 0xdd, 0xdb, 0x6d, 0xb7, 0x76, 0xdb,
        0x6d, 0xdd, 0xb6, 0xdb, 0x77, 0x6d, 0xb6, 0xdd, 0xdb, 0x6d, 0xb7, 0x76, 0xdb, 0x6c, 0x00,
    ];

    /// A "zero input response" CVSD packet.
    const ZERO_INPUT_CVSD_PACKET: [u8; 60] = [0x55; 60];

    #[derive(PartialEq, Debug)]
    enum ProcessedRequest {
        ScoRead,
        ScoWrite(Vec<u8>),
    }

    // Processes one sco request.  Returns true if the stream was ended.
    async fn process_sco_request(
        sco_request_stream: &mut ScoConnectionRequestStream,
        read_data: Vec<u8>,
    ) -> Option<ProcessedRequest> {
        match sco_request_stream.next().await {
            Some(Ok(bredr::ScoConnectionRequest::Read { responder })) => {
                let response = bredr::ScoConnectionReadResponse {
                    status_flag: Some(bredr::RxPacketStatus::CorrectlyReceivedData),
                    data: Some(read_data),
                    ..Default::default()
                };
                responder.send(&response).expect("sends okay");
                Some(ProcessedRequest::ScoRead)
            }
            Some(Ok(bredr::ScoConnectionRequest::Write { payload, responder })) => {
                responder.send().expect("response to write");
                Some(ProcessedRequest::ScoWrite(payload.data.unwrap()))
            }
            None => None,
            x => panic!("Expected read or write requests, got {x:?}"),
        }
    }

    #[fuchsia::test]
    async fn reads_audio_from_connection() {
        let (proxy, _audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, mut sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);

        control.start(PeerId(1), connection, CodecId::MSBC).expect("should be able to start");

        let (connection2, _request_stream) = connection_for_codec(PeerId(1), CodecId::MSBC, true);
        let _ = control
            .start(PeerId(1), connection2, CodecId::MSBC)
            .expect_err("Starting twice shouldn't be allowed");

        // Test note: 10 packets is not enough to force a write to audio, which will stall this test if
        // it's not started.
        for _ in 1..10 {
            assert_eq!(
                Some(ProcessedRequest::ScoRead),
                process_sco_request(&mut sco_request_stream, ZERO_INPUT_SBC_PACKET.to_vec()).await
            );
        }

        control.stop(PeerId(1)).expect("should be able to stop");
        let _ = control.stop(PeerId(1)).expect_err("can't stop a stopped thing");

        // Should be able to drain the requests.
        let mut extra_requests = 0;
        while let Some(r) =
            process_sco_request(&mut sco_request_stream, ZERO_INPUT_SBC_PACKET.to_vec()).await
        {
            assert_eq!(ProcessedRequest::ScoRead, r);
            extra_requests += 1;
        }

        info!("Got {extra_requests} extra ScoConnectionProxy Requests after stop");
    }

    #[fuchsia::test]
    async fn audio_setup_error_bad_codec() {
        let (proxy, _) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, _sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);
        let res = control.start(PeerId(1), connection, 0xD0u8.into());
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn decode_sco_audio_path() {
        use fidl_fuchsia_hardware_audio as audio;
        let (proxy, mut audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, mut sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);

        control.start(PeerId(1), connection, CodecId::MSBC).expect("should be able to start");

        let audio_input_stream_config;
        let mut _audio_output_stream_config;
        loop {
            match audio_enumerator_requests.next().await {
                Some(Ok(media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    is_input,
                    channel,
                    ..
                })) => {
                    if is_input {
                        audio_input_stream_config = channel.into_proxy();
                        break;
                    } else {
                        _audio_output_stream_config = channel.into_proxy();
                    }
                }
                x => panic!("Expected audio device by channel, got {x:?}"),
            }
        }

        let (ring_buffer, server) = fidl::endpoints::create_proxy::<audio::RingBufferMarker>();
        audio_input_stream_config
            .create_ring_buffer(&CodecId::MSBC.try_into().unwrap(), server)
            .expect("create ring buffer");

        // We need to write to the stream at least once to start it up.
        assert_eq!(
            Some(ProcessedRequest::ScoRead),
            process_sco_request(&mut sco_request_stream, ZERO_INPUT_SBC_PACKET.to_vec()).await
        );

        let notifications_per_ring = 20;
        // Request a 1-second audio buffer. This is guaranteed to be greater than 1 second, since
        // the driver must reserve any space it needs ON TOP OF the client-requested 16000 bytes.
        let (frames, _vmo) = ring_buffer
            .get_vmo(16000, notifications_per_ring)
            .await
            .expect("fidl")
            .expect("response");

        // To be deterministic, we set the first notification before even starting the ring-buffer.
        let mut position_info = ring_buffer.watch_clock_recovery_position_info();
        let mut position_notifications = 0;

        let _ = ring_buffer.start().await;

        // For 100 MSBC Audio frames, we get 7.5 x 100 = 750 milliseconds, or 12000 frames.
        let frames_per_notification = frames / notifications_per_ring;
        // As noted above, `frames` > 16000, so `frames_per_notification` > 800. Assuming the ring-
        // buffer is < 17000 frames, `expected_notifications` will be 14.xx (as u32: 14), not 15.
        let expected_notifications = 12000 / frames_per_notification;

        // We might receive the first notification as early as ring-buffer position 0,
        // so we check for a notification before processing the first chunk of data.
        if position_info.poll_unpin(&mut Context::from_waker(&std::task::Waker::noop())).is_ready()
        {
            position_notifications += 1;
            position_info = ring_buffer.watch_clock_recovery_position_info();
        }
        for _ in 1..100 {
            assert_eq!(
                Some(ProcessedRequest::ScoRead),
                process_sco_request(&mut sco_request_stream, ZERO_INPUT_SBC_PACKET.to_vec()).await
            );
            // We are the only ones polling position_info, so we can ignore wakeups (noop waker).
            if position_info
                .poll_unpin(&mut Context::from_waker(&std::task::Waker::noop()))
                .is_ready()
            {
                position_notifications += 1;
                position_info = ring_buffer.watch_clock_recovery_position_info();
            }
        }

        // The audio driver protocol require notification VALUES [timestamp, position] to tightly
        // correlate. It is less concerned with notification ARRIVAL TIMES; these could occur up to
        // 1 notification's duration early. Thus, if we expect X notifications, then we allow X+1.
        assert!(position_notifications >= expected_notifications);
        assert!(position_notifications <= expected_notifications + 1);
    }

    #[fuchsia::test]
    async fn encode_sco_audio_path_msbc() {
        use fidl_fuchsia_hardware_audio as audio;
        let (proxy, mut audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, mut sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);

        control.start(PeerId(1), connection, CodecId::MSBC).expect("should be able to start");

        let audio_output_stream_config;
        let mut _audio_input_stream_config;
        loop {
            match audio_enumerator_requests.next().await {
                Some(Ok(media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    is_input,
                    channel,
                    ..
                })) => {
                    if !is_input {
                        audio_output_stream_config = channel.into_proxy();
                        break;
                    } else {
                        _audio_input_stream_config = channel.into_proxy();
                    }
                }
                x => panic!("Expected audio device by channel, got {x:?}"),
            }
        }

        let (ring_buffer, server) = fidl::endpoints::create_proxy::<audio::RingBufferMarker>();
        audio_output_stream_config
            .create_ring_buffer(&CodecId::MSBC.try_into().unwrap(), server)
            .unwrap();

        // Note: we don't need to read from the stream to start it, it gets polled automatically by
        // the read task.

        let notifications_per_ring = 20;
        // Request at least 1 second of audio buffer.
        let (_frames, _vmo) = ring_buffer
            .get_vmo(16000, notifications_per_ring)
            .await
            .expect("fidl")
            .expect("response");

        let _ = ring_buffer.start().await;

        // Expect 100 MSBC Audio frames, which should take ~ 750 milliseconds.
        let next_header = &mut [0x01, 0x08];
        for _sco_frame in 1..100 {
            'sco: loop {
                match process_sco_request(&mut sco_request_stream, ZERO_INPUT_SBC_PACKET.to_vec())
                    .await
                {
                    Some(ProcessedRequest::ScoRead) => continue 'sco,
                    Some(ProcessedRequest::ScoWrite(data)) => {
                        assert_eq!(60, data.len());
                        // Skip the H2 header which changes for every packet.
                        assert_eq!(&ZERO_INPUT_SBC_PACKET[2..], &data[2..]);
                        assert_eq!(next_header, &data[0..2]);
                        // Prep for the next heade
                        match next_header[1] {
                            0x08 => next_header[1] = 0x38,
                            0x38 => next_header[1] = 0xc8,
                            0xc8 => next_header[1] = 0xf8,
                            0xf8 => next_header[1] = 0x08,
                            _ => unreachable!(),
                        };
                        break 'sco;
                    }
                    x => panic!("Expected read or write but got {x:?}"),
                };
            }
        }
    }

    #[fuchsia::test]
    async fn encode_sco_audio_path_cvsd() {
        use fidl_fuchsia_hardware_audio as audio;
        let (proxy, mut audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, mut sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::CVSD, true);

        control.start(PeerId(1), connection, CodecId::CVSD).expect("should be able to start");

        let audio_output_stream_config;
        let mut _audio_input_stream_config;
        loop {
            match audio_enumerator_requests.next().await {
                Some(Ok(media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    is_input,
                    channel,
                    ..
                })) => {
                    if !is_input {
                        audio_output_stream_config = channel.into_proxy();
                        break;
                    } else {
                        _audio_input_stream_config = channel.into_proxy();
                    }
                }
                x => panic!("Expected audio device by channel, got {x:?}"),
            }
        }

        let (ring_buffer, server) = fidl::endpoints::create_proxy::<audio::RingBufferMarker>();
        audio_output_stream_config
            .create_ring_buffer(&CodecId::CVSD.try_into().unwrap(), server)
            .unwrap();

        // Note: we don't need to read from the stream to start it, it gets polled automatically by
        // the read task.

        let notifications_per_ring = 10;
        // Request at least 1 second of audio buffer.
        let (_frames, _vmo) = ring_buffer
            .get_vmo(64000, notifications_per_ring)
            .await
            .expect("fidl")
            .expect("response");

        let _ = ring_buffer.start().await;

        // Expect 100 CVSD Audio frames, which should take ~ 750 milliseconds.
        for _sco_frame in 1..100 {
            'sco: loop {
                match process_sco_request(&mut sco_request_stream, ZERO_INPUT_CVSD_PACKET.to_vec())
                    .await
                {
                    Some(ProcessedRequest::ScoRead) => continue 'sco,
                    Some(ProcessedRequest::ScoWrite(data)) => {
                        // Confirm the data is right
                        assert_eq!(60, data.len());
                        assert_eq!(&ZERO_INPUT_CVSD_PACKET, data.as_slice());
                        break 'sco;
                    }
                    x => panic!("Expected read or write but got {x:?}"),
                };
            }
        }
    }

    #[fuchsia::test]
    async fn read_from_audio_output() {
        use fidl_fuchsia_hardware_audio as audio;
        let (proxy, mut audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();

        let (connection, mut sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);

        control.start(PeerId(1), connection, CodecId::MSBC).expect("should be able to start");

        let audio_output_stream_config;
        let mut _audio_input_stream_config;
        loop {
            match audio_enumerator_requests.next().await {
                Some(Ok(media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    is_input,
                    channel,
                    ..
                })) => {
                    if !is_input {
                        audio_output_stream_config = channel.into_proxy();
                        break;
                    } else {
                        _audio_input_stream_config = channel.into_proxy();
                    }
                }
                x => panic!("Expected audio device by channel, got {x:?}"),
            }
        }

        let (ring_buffer, server) = fidl::endpoints::create_proxy::<audio::RingBufferMarker>();
        audio_output_stream_config
            .create_ring_buffer(&CodecId::MSBC.try_into().unwrap(), server)
            .expect("create ring buffer");

        let notifications_per_ring = 20;
        // Request at least 1 second of audio buffer.
        let (_frames, _vmo) = ring_buffer
            .get_vmo(16000, notifications_per_ring)
            .await
            .expect("fidl")
            .expect("response");

        let _ = ring_buffer.start().await;

        // We should be just reading from the audio output, track via position notifications.
        // 20 position notifications happen in one second.
        'position_notifications: for i in 1..20 {
            let mut position_info = ring_buffer.watch_clock_recovery_position_info();
            loop {
                let sco_activity = Box::pin(process_sco_request(
                    &mut sco_request_stream,
                    ZERO_INPUT_SBC_PACKET.to_vec(),
                ));
                use futures::future::Either;
                match futures::future::select(position_info, sco_activity).await {
                    Either::Left((result, _sco_fut)) => {
                        assert!(result.is_ok(), "Position Info failed at {i}");
                        continue 'position_notifications;
                    }
                    Either::Right((_sco_pkt, position_info_fut)) => {
                        position_info = position_info_fut;
                    }
                }
            }
        }
    }

    #[fuchsia::test]
    async fn audio_output_error_sends_to_events() {
        let (proxy, mut audio_enumerator_requests) =
            fidl::endpoints::create_proxy_and_stream::<media::AudioDeviceEnumeratorMarker>();
        let mut control = InbandControl::create(proxy).unwrap();
        let mut events = control.take_events();

        let (connection, _sco_request_stream) =
            connection_for_codec(PeerId(1), CodecId::MSBC, true);

        control.start(PeerId(1), connection, CodecId::MSBC).expect("should be able to start");

        let audio_output_stream_config;
        let mut _audio_input_stream_config;
        loop {
            match audio_enumerator_requests.next().await {
                Some(Ok(media::AudioDeviceEnumeratorRequest::AddDeviceByChannel {
                    is_input,
                    channel,
                    ..
                })) => {
                    if !is_input {
                        audio_output_stream_config = channel.into_proxy();
                        break;
                    } else {
                        _audio_input_stream_config = channel.into_proxy();
                    }
                }
                x => panic!("Expected audio device by channel, got {x:?}"),
            }
        }

        drop(audio_output_stream_config);

        // Events should produce an error because there was an issue with audio output.
        match events.next().await {
            Some(ControlEvent::Stopped { id, error: Some(_) }) => {
                assert_eq!(PeerId(1), id);
            }
            x => panic!("Expected the peer to have error stop, but got {x:?}"),
        };
    }
}
