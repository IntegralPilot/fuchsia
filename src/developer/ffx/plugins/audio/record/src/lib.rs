// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use blocking::Unblock;

use fdomain_client::Socket;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_audio_controller as fac;
use fdomain_fuchsia_media as fmedia;
use ffx_audio_common;
use ffx_audio_record_args::{AudioCaptureUsageExtended, RecordCommand};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fho::{FfxMain, FfxTool};
use futures::{AsyncWrite, FutureExt};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct RecordTool {
    #[command]
    cmd: RecordCommand,

    #[with(moniker("/core/audio_ffx_daemon"))]
    controller: fac::RecorderProxy,
}

fho::embedded_plugin!(RecordTool);
#[async_trait(?Send)]
impl FfxMain for RecordTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let capturer_usage = match self.cmd.usage {
            AudioCaptureUsageExtended::Background(usage)
            | AudioCaptureUsageExtended::Foreground(usage)
            | AudioCaptureUsageExtended::Communication(usage)
            | AudioCaptureUsageExtended::SystemAgent(usage) => Some(usage),
            _ => None,
        };

        let (location, gain_settings) = match self.cmd.usage {
            AudioCaptureUsageExtended::Loopback => {
                (fac::RecordSource::Loopback(fac::Loopback {}), None)
            }
            AudioCaptureUsageExtended::Ultrasound => (
                fac::RecordSource::Capturer(fac::CapturerConfig::UltrasoundCapturer(
                    fac::UltrasoundCapturer {},
                )),
                None,
            ),
            _ => (
                fac::RecordSource::Capturer(fac::CapturerConfig::StandardCapturer(
                    fac::StandardCapturerConfig { usage: capturer_usage, ..Default::default() },
                )),
                Some(fac::GainSettings {
                    mute: Some(self.cmd.mute),
                    gain: Some(self.cmd.gain),
                    ..Default::default()
                }),
            ),
        };

        let (record_remote, record_local) = self.controller.domain().create_datagram_socket();
        let (cancel_proxy, cancel_server) =
            self.controller.domain().create_proxy::<fac::RecordCancelerMarker>();

        let request = fac::RecorderRecordRequest {
            source: Some(location),
            stream_type: Some(fmedia::AudioStreamType::from(self.cmd.format)),
            duration: self.cmd.duration.map(|duration| duration.as_nanos() as i64),
            canceler: Some(cancel_server),
            gain_settings,
            buffer_size: self.cmd.buffer_size,
            wav_data: Some(record_remote),
            ..Default::default()
        };

        let mut stdout = Unblock::new(std::io::stdout());
        let keypress_waiter = ffx_audio_common::cancel_on_keypress(
            cancel_proxy,
            ffx_audio_common::get_stdin_waiter().fuse(),
        );

        record_impl(self.controller, request, keypress_waiter, record_local, &mut stdout, writer)
            .await?;
        Ok(())
    }
}

async fn record_impl<W>(
    controller: fac::RecorderProxy,
    request: fac::RecorderRecordRequest,
    keypress_waiter: impl futures::Future<Output = Result<(), std::io::Error>>,
    record_local: Socket,
    mut wav_writer: W, // Output generalized to stdout or a test buffer. Forward data
    // from daemon to this writer.
    mut output_result_writer: SimpleWriter,
) -> Result<()>
where
    W: AsyncWrite + std::marker::Unpin,
{
    let result = ffx_audio_common::record(
        controller,
        request,
        record_local,
        &mut wav_writer,
        keypress_waiter,
    )
    .await;

    let message = ffx_audio_common::format_record_result(result);
    writeln!(output_result_writer.stderr(), "{}", message)
        .map_err(|e| anyhow!("Writing result failed with error {e}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_audio_common::tests::SINE_WAV;
    use ffx_writer::{TestBuffer, TestBuffers};

    #[fuchsia::test]
    pub async fn test_record_no_cancel() -> Result<(), fho::Error> {
        // Test without sending a cancel message. Still set up the canceling proxy and server,
        // but never send the message from proxy to daemon to cancel. Test daemon should
        // exit after duration (real daemon exits after sending all duration amount of packets).
        let client = fdomain_local::local_client_empty();
        let controller = ffx_audio_common::tests::fake_audio_recorder(client.clone());
        let test_buffers = TestBuffers::default();
        let result_writer: SimpleWriter = SimpleWriter::new_test(&test_buffers);

        let (cancel_proxy, cancel_server) = client.create_proxy::<fac::RecordCancelerMarker>();

        let test_stdout = TestBuffer::default();

        let (record_remote, record_local) = client.create_datagram_socket();
        let request = fac::RecorderRecordRequest {
            source: None,
            stream_type: None,
            duration: Some(500),
            canceler: Some(cancel_server),
            gain_settings: None,
            buffer_size: None,
            wav_data: Some(record_remote),
            ..Default::default()
        };

        // Pass a future that will never complete as an input waiter.
        let keypress_waiter =
            ffx_audio_common::cancel_on_keypress(cancel_proxy, futures::future::pending().fuse());

        let _res = record_impl(
            controller,
            request,
            keypress_waiter,
            record_local,
            test_stdout.clone(),
            result_writer,
        )
        .await?;

        let expected_result_output = format!(
            "Successfully recorded 123 bytes of audio. \nPackets processed: 123 \nLate wakeups: Unavailable\n"
        );
        let stderr = test_buffers.into_stderr_str();
        assert_eq!(stderr, expected_result_output);

        let stdout = test_stdout.into_inner();
        let expected_wav_output = Vec::from(SINE_WAV);
        assert_eq!(stdout, expected_wav_output);
        Ok(())
    }

    #[fuchsia::test]
    pub async fn test_record_immediate_cancel() -> Result<(), fho::Error> {
        let client = fdomain_local::local_client_empty();
        let controller = ffx_audio_common::tests::fake_audio_recorder(client.clone());
        let test_buffers = TestBuffers::default();
        let result_writer: SimpleWriter = SimpleWriter::new_test(&test_buffers);

        let (cancel_proxy, cancel_server) = client.create_proxy::<fac::RecordCancelerMarker>();

        let test_stdout = TestBuffer::default();

        let (record_remote, record_local) = client.create_datagram_socket();
        let request = fac::RecorderRecordRequest {
            source: None,
            stream_type: None,
            duration: None,
            canceler: Some(cancel_server),
            gain_settings: None,
            buffer_size: None,
            wav_data: Some(record_remote),
            ..Default::default()
        };

        // Test canceler signaling. Not concerned with how much data gets back through socket.
        // Test failing is never finishing execution before timeout.
        let keypress_waiter =
            ffx_audio_common::cancel_on_keypress(cancel_proxy, futures::future::ready(Ok(())));

        let _res = record_impl(
            controller,
            request,
            keypress_waiter,
            record_local,
            test_stdout.clone(),
            result_writer,
        )
        .await?;

        Ok(())
    }
}
