// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use blocking::Unblock;
use fdomain_client::Socket;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_audio_controller as fac;
use futures::future::Either;
use futures::{AsyncReadExt, AsyncWrite, FutureExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::Arc;

#[derive(Debug, Serialize, PartialEq, Deserialize)]
pub struct PlayResult {
    pub bytes_processed: Option<u64>,
}

#[derive(Debug, Serialize, PartialEq, Deserialize)]
pub struct RecordResult {
    pub bytes_processed: Option<u64>,
    pub packets_processed: Option<u64>,
    pub late_wakeups: Option<u64>,
}

pub async fn play(
    request: fac::PlayerPlayRequest,
    controller: fac::PlayerProxy,
    mut play_local: Socket, // Send data from ffx to target.
    input_reader: Box<dyn std::io::Read + std::marker::Send + 'static>,
    // Input generalized to stdin or test buffer. Forward to socket.
) -> Result<PlayResult, Error>
where
{
    let (play_result, _stdin_reader_result) = futures::future::try_join(
        controller.play(request).map_err(|e| anyhow!("future try join failed with error {e}")),
        async move {
            let stdin_res = futures::io::copy(Unblock::new(input_reader), &mut play_local).await;

            // Close ffx end of socket so that daemon end reads EOF and stops waiting for data.
            drop(play_local);
            stdin_res.map_err(|e| anyhow!("Error stdin: {}", e))
        },
    )
    .await?;
    play_result
        .map(|result| PlayResult { bytes_processed: result.bytes_processed })
        .map_err(|e| anyhow!("Play failed with error {:?}", e))
}

pub async fn record<W>(
    controller: fac::RecorderProxy,
    request: fac::RecorderRecordRequest,
    mut record_local: Socket,
    mut output_writer: W, // Output generalized to stdout or a test buffer. Forward data
    // from daemon to this writer.
    keypress_handler: impl futures::Future<Output = Result<(), std::io::Error>>,
) -> Result<RecordResult, Error>
where
    W: AsyncWrite + std::marker::Unpin,
{
    let response_fut =
        { controller.record(request).map_err(|e| anyhow!("Controller record failure: {:?}", e)) };

    let wav_fut = {
        futures::io::copy(&mut record_local, &mut output_writer)
            .map_err(|e| anyhow!("wav output failure: {:?}", e))
    };

    let (record_result, _keypress_result, _socket_result) = futures::future::try_join3(
        response_fut,
        keypress_handler.map_err(|e| anyhow!("Failed to wait for keypress: {e}")),
        wav_fut,
    )
    .await?;

    record_result
        .map(|result| RecordResult {
            bytes_processed: result.bytes_processed,
            packets_processed: result.packets_processed,
            late_wakeups: result.late_wakeups,
        })
        .map_err(|e| anyhow!("Record request failed with error {:?}", e))
}

pub fn get_stdin_waiter() -> futures::future::BoxFuture<'static, Result<(), std::io::Error>> {
    blocking::unblock(move || {
        let _ = std::io::stdin().lock().read_line(&mut String::new());
        Ok(())
    })
    .boxed()
}

fn maybe_to_str(maybe_bytes: Option<u64>) -> String {
    maybe_bytes.map_or_else(|| "Unavailable".to_owned(), |s| ToString::to_string(&s))
}

pub fn format_record_result(result: Result<RecordResult, Error>) -> String {
    match result {
        Ok(result) => {
            format!(
                "Successfully recorded {} bytes of audio. \nPackets processed: {} \nLate wakeups: {}",
                maybe_to_str(result.bytes_processed),
                maybe_to_str(result.packets_processed),
                maybe_to_str(result.late_wakeups)
            )
        }
        Err(e) => {
            format!("Record failed with error {:?}", e)
        }
    }
}

pub async fn cancel_on_keypress(
    proxy: fac::RecordCancelerProxy,
    input_waiter: impl futures::Future<Output = Result<(), std::io::Error>>
    + futures::future::FusedFuture,
) -> Result<(), std::io::Error> {
    let closed_fut = async {
        let _ = proxy.on_closed().await;
    }
    .fuse();

    futures::pin_mut!(closed_fut, input_waiter);

    if let Either::Right(_) = futures::future::select(closed_fut, input_waiter).await {
        match proxy.cancel().await {
            Err(e) => return Err(std::io::Error::other(format!("FIDL error {e}"))),
            Ok(Err(e)) => return Err(std::io::Error::other(format!("Canceler error {e}"))),
            Ok(_res) => return Ok(()),
        };
    };
    Ok(())
}

pub mod tests {
    use super::*;
    use fuchsia_audio_fdomain::stop_listener;
    use timeout::timeout;

    // Header for an infinite-length audio stream: 16 kHz, 3-channel, 16-bit signed.
    // This slice contains a complete WAVE_FORMAT_EXTENSIBLE file header
    // (everything except the audio data itself), which includes the 'fmt ' chunk
    // and the first two fields of the 'data' chunk. After this, the next bytes
    // in this stream would be the audio itself (in 6-byte frames).
    pub const WAV_HEADER_EXT: &'static [u8; 83] = b"\x52\x49\x46\x46\xff\xff\xff\xff
    \x57\x41\x56\x45\x66\x6d\x74\x20\x28\x00\x00\x00\xfe\xff\x03\x00\x80\x3e\x00\x00
    \x00\x77\x01\x00\x06\x00\x10\x00\x16\x00\x10\x00\x07\x00\x00\x00\x01\x00\x00\x00
    \x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71\x64\x61\x74\x61\xff\xff\xff\xff";

    // Complete WAV file (140 bytes in all) for a 48 kHz, 1-channel, unsigned 8-bit
    // audio stream. The file is the old-style PCMWAVEFORMAT and contains 96 frames
    // of a sinusoid of approx. 439 Hz.
    pub const SINE_WAV: &'static [u8; 140] = b"\
    \x52\x49\x46\x46\x84\x00\x00\x00\x57\x41\x56\x45\x66\x6d\x74\x20\
    \x10\x00\x00\x00\x01\x00\x01\x00\x80\xbb\x00\x00\x80\xbb\x00\x00\
    \x01\x00\x08\x00\x64\x61\x74\x61\x60\x00\x00\x00\x80\x87\x8f\x96\
    \x9d\xa4\xab\xb2\xb9\xbf\xc6\xcc\xd2\xd7\xdc\xe1\xe6\xea\xee\xf2\
    \xf5\xf8\xfa\xfc\xfe\xff\xff\xff\xff\xff\xfe\xfd\xfb\xf9\xf7\xf4\
    \xf0\xec\xe8\xe4\xdf\xda\xd5\xcf\xc9\xc3\xbc\xb6\xaf\xa8\xa1\x9a\
    \x93\x8b\x84\x7d\x75\x6e\x67\x60\x58\x52\x4b\x44\x3e\x38\x32\x2c\
    \x26\x21\x1d\x18\x14\x10\x0d\x0a\x07\x05\x03\x02\x01\x00\x00\x00\
    \x01\x02\x04\x06\x08\x0b\x0e\x11\x15\x1a\x1e\x23";

    pub fn fake_audio_player(client: Arc<fdomain_client::Client>) -> fac::PlayerProxy {
        let callback = |req| match req {
            fac::PlayerRequest::Play { payload, responder } => {
                let data_socket =
                    payload.wav_source.ok_or_else(|| anyhow!("Socket argument missing.")).unwrap();

                let mut socket = data_socket;
                let mut wav_file = vec![0u8; 11];
                fuchsia_async::Task::local(async move {
                    let _ = socket.read_exact(&mut wav_file).await.unwrap();
                    // Pass back a fake value for total bytes processed.
                    // ffx tests are focused on the interaction between ffx and the user,
                    // so controller specific functionality can be covered by tests targeting
                    // it specifically.
                    let response =
                        fac::PlayerPlayResponse { bytes_processed: Some(1), ..Default::default() };

                    responder.send(Ok(response)).unwrap();
                })
                .detach();
            }
            _ => unimplemented!(),
        };

        target_holders::fake_proxy(client, callback)
    }

    pub fn fake_audio_recorder(client: Arc<fdomain_client::Client>) -> fac::RecorderProxy {
        let callback = |req| match req {
            fac::RecorderRequest::Record { payload, responder } => {
                let wav_socket =
                    payload.wav_data.ok_or_else(|| anyhow!("Socket argument missing.")).unwrap();

                fuchsia_async::Task::local(async move {
                    wav_socket.fdomain_write_all(SINE_WAV).await.unwrap();
                })
                .detach();

                // Let either the duration run out and close the cancel channel or
                // respond to cancel FIDL request.
                fuchsia_async::Task::local(async move {
                    if let Some(cancel_server) = payload.canceler {
                        if let Some(duration_nanos) = payload.duration {
                            // Duration provided, after duration passes we need to close the
                            // server channel and sockets by dropping them.
                            let _ = timeout(
                                std::time::Duration::from_nanos(duration_nanos.try_into().unwrap()),
                                async {},
                            )
                            .await;
                        } else {
                            // No duration provided, should receive a cancel() FIDL message.
                            let stop_signal = std::sync::atomic::AtomicBool::new(false);
                            let _ = stop_listener(cancel_server, &stop_signal).await;
                            assert_eq!(stop_signal.load(std::sync::atomic::Ordering::SeqCst), true);
                        }
                    }
                    let _ = responder.send(Ok(fac::RecorderRecordResponse {
                        // Calculating bytes & packets processed can be tested in controller tests.
                        // ffx tests are for I/O and command line validation.
                        bytes_processed: Some(123),
                        packets_processed: Some(123),
                        late_wakeups: None,
                        ..Default::default()
                    }));
                })
                .detach();
            }
            _ => unimplemented!(),
        };

        target_holders::fake_proxy(client, callback)
    }
}
