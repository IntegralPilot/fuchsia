// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::{AudioProxy, AudioSettings2};
use ffx_setui_audio_args::Audio;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct AudioTool {
    #[command]
    cmd: Audio,
    #[with(moniker("/core/setui_service"))]
    audio_proxy: AudioProxy,
}

fho::embedded_plugin!(AudioTool);

#[async_trait(?Send)]
impl FfxMain for AudioTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.audio_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn run_command<W: std::io::Write>(
    audio_proxy: AudioProxy,
    audio: Audio,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result("Audio", command(audio_proxy, audio).await, writer).await
}

async fn command(proxy: AudioProxy, audio: Audio) -> WatchOrSetResult {
    let settings = AudioSettings2::try_from(audio).expect("Input arguments have errors");

    if settings == AudioSettings2::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch2())))
    } else {
        Ok(Either::Set(if let Err(err) = proxy.set2(&settings).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set Audio to {:?}", audio)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_media::AudioRenderUsage2;
    use fdomain_fuchsia_settings::AudioRequest;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_run_command() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AudioRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            AudioRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
            AudioRequest::Set2 { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            AudioRequest::Watch2 { .. } => {
                panic!("Unexpected call to watch2");
            }
            AudioRequest::_UnknownMethod { .. } => {
                panic!("Unexpected call to unknown method");
            }
        });

        let audio = Audio {
            stream: Some(AudioRenderUsage2::Background),
            source: Some(fdomain_fuchsia_settings::AudioStreamSettingSource::User),
            level: Some(0.5),
            volume_muted: Some(false),
        };
        let response = run_command(proxy, audio, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        Audio {
            stream: Some(AudioRenderUsage2::Media),
            source: Some(fdomain_fuchsia_settings::AudioStreamSettingSource::User),
            level: Some(0.6),
            volume_muted: Some(false),
        };
        "Test audio set() output with non-empty input."
    )]
    #[test_case(
        Audio {
            stream: Some(AudioRenderUsage2::Background),
            source: Some(fdomain_fuchsia_settings::AudioStreamSettingSource::User),
            level: Some(0.1),
            volume_muted: Some(false),
        };
        "Test audio set() output with a different non-empty input."
    )]
    #[fuchsia::test]
    async fn validate_audio_set_output(expected_audio: Audio) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AudioRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            AudioRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
            AudioRequest::Set2 { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            AudioRequest::Watch2 { .. } => {
                panic!("Unexpected call to watch2");
            }
            AudioRequest::_UnknownMethod { .. } => {
                panic!("Unexpected call to unknown method");
            }
        });

        let output = utils::assert_set!(command(proxy, expected_audio));
        assert_eq!(output, format!("Successfully set Audio to {:?}", expected_audio));
        Ok(())
    }

    #[test_case(
        Audio {
            stream: None,
            source: None,
            level: None,
            volume_muted: None,
        };
        "Test audio watch() output with empty input."
    )]
    #[test_case(
        Audio {
            stream: Some(AudioRenderUsage2::Background),
            source: Some(fdomain_fuchsia_settings::AudioStreamSettingSource::User),
            level: Some(0.1),
            volume_muted: Some(false),
        };
        "Test audio watch() output with non-empty input."
    )]
    #[fuchsia::test]
    async fn validate_audio_watch_output(expected_audio: Audio) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AudioRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            AudioRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
            AudioRequest::Set2 { .. } => {
                panic!("Unexpected call to set2");
            }
            AudioRequest::Watch2 { responder } => {
                let _ = responder.send(
                    &AudioSettings2::try_from(expected_audio).expect("Invalid input arguments"),
                );
            }
            AudioRequest::_UnknownMethod { .. } => {
                panic!("Unexpected call to unknown method");
            }
        });

        let output = utils::assert_watch!(command(
            proxy,
            Audio { stream: None, source: None, level: None, volume_muted: None }
        ));
        assert_eq!(
            output,
            format!(
                "{:#?}",
                AudioSettings2::try_from(expected_audio).expect("Invalid input arguments")
            )
        );
        Ok(())
    }
}
