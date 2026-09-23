// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_test_audio::{
    AudioTestError, CaptureRequest, CaptureRequestStream, INJECTED_AUDIO_MAXIMUM_FILE_SIZE,
    InjectionRequest, InjectionRequestStream, WaitForQuietResult,
};
use fuchsia_async as fasync;
use futures::lock::Mutex;
use futures::stream::TryStreamExt;
use futures::{AsyncReadExt, AsyncWriteExt};
use log::{debug, error, warn};
use std::sync::Arc;

use crate::audio_facade::AudioFacade;

const DEFAULT_QUIET_WAIT_TIME_MS: u32 = 3000;
const DEFAULT_MAXIMUM_TIME_TO_WAIT_FOR_QUIET_MS: u32 = 30000;
const DEFAULT_MAXIMUM_TIME_TO_WAIT_FOR_SOUND_MS: u32 = 30000;
const DEFAULT_MAXIMUM_CAPTURE_DURATION_MS: u32 = 30000;

pub async fn handle_injection_request(
    facade: Arc<AudioFacade>,
    stream: InjectionRequestStream,
) -> Result<(), fidl::Error> {
    stream
        .try_for_each(move |request| {
            let facade_clone = facade.clone();
            Box::pin(async move {
                match request {
                    InjectionRequest::ClearInputAudio { index, responder } => {
                        debug!("ClearInputAudio({})", index);
                        facade_clone.clear_input_audio(index.try_into().unwrap()).await;
                        responder.send(Ok(())).context("error sending response").unwrap();
                    }
                    InjectionRequest::StartInputInjection { index, responder } => {
                        debug!("StartInputInjection({})", index);
                        let result = match facade_clone
                            .start_input_injection(index.try_into().unwrap())
                            .await
                        {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!("StartInputInjection failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    InjectionRequest::StopInputInjection { responder } => {
                        debug!("StopInputInjection");
                        let result = match facade_clone.stop_input_injection().await {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!("StopInputInjection failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    InjectionRequest::WriteInputAudio { index, audio_writer, ..} => {
                        debug!("WriteInputAudio({})", index);
                        let mut audio_stream = fasync::Socket::from_socket(audio_writer);
                        let mut read_bytes = 0;
                        let mut audio_data = Vec::new();
                        let mut local_buf = [0u8; 1024 * 16];

                        while let Ok(val) = audio_stream.read(&mut local_buf).await {
                            debug!("Got {} bytes from audio stream", val);
                            if val == 0 {
                                break;
                            }
                            read_bytes += val;
                            if read_bytes > INJECTED_AUDIO_MAXIMUM_FILE_SIZE as usize {
                                error!("Attempted to write too many bytes to audio injection. Limit is {INJECTED_AUDIO_MAXIMUM_FILE_SIZE} but already tried to write {read_bytes}");
                            }
                            audio_data.extend_from_slice(&local_buf[..val]);
                        }

                        debug!("Done reading audio stream");

                        facade_clone.put_input_audio( index.try_into().unwrap(), audio_data)
                        .await
                        .context("put_input_audio errored")
                        .unwrap();
                    }
                    InjectionRequest::GetInputAudioSize { index, responder } => {
                        debug!("GetInputAudioSize({})", index);
                        let result = match facade_clone
                            .get_input_audio_size(index.try_into().unwrap()).await {
                                Ok(size) => Ok(size.try_into().unwrap()),
                                Err(e) => {
                                    error!("GetInputAudioSize failed: {:?}", e);
                                    Err(AudioTestError::Fail)
                                }
                            };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    InjectionRequest::WaitUntilInputIsDone { responder } => {
                        debug!("WaitUntilInputIsDone...");
                        let response = match facade_clone.wait_until_input_playing_is_finished().await {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!("WaitUntilInputIsDone failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(response).context("error sending response").unwrap();
                    }
                    InjectionRequest::_UnknownMethod { ordinal, .. } => {
                        error!("Unknown method received, ordinal {ordinal}");
                    }
                }
                Ok(())
            })
        })
        .await
}

pub async fn handle_capture_request(
    facade: Arc<AudioFacade>,
    stream: CaptureRequestStream,
) -> Result<(), fidl::Error> {
    let current_trigger_capture_fut: Arc<
        Mutex<
            Option<
                fasync::Task<Result<fidl_fuchsia_test_audio::QueuedCaptureResult, AudioTestError>>,
            >,
        >,
    > = Arc::new(Mutex::new(None));

    stream
        .try_for_each(move |request| {
            let facade_clone = facade.clone();
            let fut_clone = current_trigger_capture_fut.clone();
            Box::pin(async move {
                match request {
                    CaptureRequest::GetOutputAudio { responder } => {
                        let result = match facade_clone
                            .get_output_audio_vec()
                            .await
                            .context("get_output_audio errored")
                        {
                            Ok(data) => data,
                            Err(e) => {
                                error!("GetOutputAudio failed: {:?}", e);
                                responder
                                    .send(Err(AudioTestError::Fail))
                                    .context("error sending response")
                                    .unwrap();
                                return Ok(());
                            }
                        };
                        let (sender, receiver) = zx::Socket::create_stream();
                        sender.half_close().expect("prevent writes to receiver");
                        fasync::Task::spawn(async move {
                            debug!("Starting output audio stream");
                            let mut sender = fasync::Socket::from_socket(sender);
                            if let Err(e) = sender.write_all(result.as_slice()).await {
                                warn!("Failed to write audio output stream: {e:?}");
                            } else {
                                debug!(
                                    "Finished output audio stream, wrote {} bytes",
                                    result.len()
                                );
                            }
                        })
                        .detach();
                        responder.send(Ok(receiver)).context("error sending response").unwrap();
                    }
                    CaptureRequest::StartOutputCapture { responder } => {
                        debug!("StartOutputSave");
                        let result = match facade_clone.start_output_capture().await {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!("StartOutputSave failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    CaptureRequest::StopOutputCapture { responder } => {
                        debug!("StopOutputSave");
                        let result = match facade_clone.stop_output_capture().await {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                error!("StopOutputSave failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    CaptureRequest::WaitForQuiet { payload, responder } => {
                        debug!("WaitForQuiet");
                        let result = match facade_clone
                            .wait_for_quiet_period(
                                std::time::Duration::from_millis(
                                    payload
                                        .requested_quiet_period_ms
                                        .unwrap_or(DEFAULT_QUIET_WAIT_TIME_MS)
                                        as u64,
                                ),
                                std::time::Duration::from_millis(
                                    payload
                                        .maximum_wait_time_ms
                                        .unwrap_or(DEFAULT_MAXIMUM_TIME_TO_WAIT_FOR_QUIET_MS)
                                        as u64,
                                ),
                            )
                            .await
                        {
                            Ok(true) => Ok(WaitForQuietResult::Success),
                            Ok(false) => Ok(WaitForQuietResult::QuietPeriodNotObserved),
                            Err(e) => {
                                error!("WaitForQuiet failed: {:?}", e);
                                Err(AudioTestError::Fail)
                            }
                        };
                        responder.send(result).context("error sending response").unwrap();
                    }
                    CaptureRequest::QueueTriggeredCapture { payload, responder } => {
                        debug!("QueueTriggeredCapture");
                        let mut current_fut = fut_clone.lock().await;
                        if let Some(v) = current_fut.take() {
                            error!("Queuing another capture while one is queued. Canceling original one.");
                            v.abort().await;
                        }
                        *current_fut = Some(
                            fasync::Task::spawn(async move {

                        facade_clone
                            .trigger_capture_on_sound(
                                std::time::Duration::from_millis(
                                    payload
                                        .maximum_time_to_wait_for_sound_ms
                                        .unwrap_or(DEFAULT_MAXIMUM_TIME_TO_WAIT_FOR_SOUND_MS)
                                        as u64,
                                ),
                                std::time::Duration::from_millis(
                                    payload
                                        .maximum_capture_duration_ms
                                        .unwrap_or(DEFAULT_MAXIMUM_CAPTURE_DURATION_MS)
                                        as u64,
                                ),
                                payload
                                    .optional_quiet_before_stopping_ms
                                    .map(|v| std::time::Duration::from_millis(v as u64)),
                            )
                            .await
                            .map_err(|e| {
                                error!("QueueAndWaitForCapture failed: {:?}", e);
                                AudioTestError::Fail
                            })
                            })
                        );
                        responder.send(Ok(())).context("error sending response").unwrap();
                    }
                    CaptureRequest::WaitForTriggeredCapture { responder, .. } => {
                        debug!("WaitForTriggeredCapture");
                        let current_fut = fut_clone.lock().await.take();
                        if current_fut.is_none() {
                            error!("No pending triggered capture to wait for.");
                            responder.send(Err(AudioTestError::Fail)).context("error sending response").unwrap();
                            return Ok(());
                        }

                        responder.send(current_fut.unwrap().await).context("error sending response").unwrap();

                    },
                    CaptureRequest::_UnknownMethod { ordinal, .. } => {
                        error!("Unknown method received, ordinal {ordinal}");
                    }
                }
                Ok(())
            })
        })
        .await
}
