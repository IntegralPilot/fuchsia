// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args;
use anyhow::{Context, Error};
use fidl_fuchsia_update_channel as fupdate_channel;
use fidl_fuchsia_update_channelcontrol::{ChannelControlMarker, ChannelControlProxy};
use fuchsia_component::client::connect_to_protocol;

pub async fn handle_channel_cmd(cmd: args::channel::Command) -> Result<(), Error> {
    let channel_provider = connect_to_protocol::<fupdate_channel::ProviderMarker>()
        .context("Failed to connect to channel provider service")?;
    let channel_control = connect_to_protocol::<ChannelControlMarker>()
        .context("Failed to connect to channel control service")?;
    handle_channel_cmd_impl(cmd, &channel_provider, &channel_control).await
}

async fn handle_channel_cmd_impl(
    cmd: args::channel::Command,
    channel_provider: &fupdate_channel::ProviderProxy,
    channel_control: &ChannelControlProxy,
) -> Result<(), Error> {
    match cmd {
        args::channel::Command::Get(args::channel::Get {}) => {
            let channel = channel_provider.get_current().await?;
            println!("current channel: {channel}");
        }
        args::channel::Command::Target(_) => {
            let channel = channel_control.get_target().await?;
            println!("target channel: {channel}");
        }
        args::channel::Command::Set(args::channel::Set { channel }) => {
            channel_control.set_target(&channel).await?;
        }
        args::channel::Command::List(_) => {
            let channels = channel_control.get_target_list().await?;
            if channels.is_empty() {
                println!("known channels list is empty.");
            } else {
                println!("known channels:");
                for channel in channels {
                    println!("{channel}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_update_channelcontrol::ChannelControlRequest;
    use futures::prelude::*;

    async fn perform_channel_test<PV, CV>(
        argument: args::channel::Command,
        provider_verifier: PV,
        control_verifier: CV,
    ) where
        PV: Fn(Option<fupdate_channel::ProviderRequest>),
        CV: Fn(Option<ChannelControlRequest>),
    {
        let (provider_proxy, mut provider_stream) =
            create_proxy_and_stream::<fupdate_channel::ProviderMarker>();
        let (control_proxy, mut control_stream) = create_proxy_and_stream::<ChannelControlMarker>();
        let fut = async move {
            assert_matches!(
                handle_channel_cmd_impl(argument, &provider_proxy, &control_proxy).await,
                Ok(())
            );
        };
        let provider_stream_fut =
            async move { provider_verifier(provider_stream.try_next().await.unwrap()) };
        let control_stream_fut =
            async move { control_verifier(control_stream.try_next().await.unwrap()) };
        let ((), (), ()) = futures::join!(fut, provider_stream_fut, control_stream_fut);
    }

    #[fuchsia::test]
    async fn test_channel_get() {
        perform_channel_test(
            args::channel::Command::Get(args::channel::Get {}),
            |cmd| match cmd.unwrap() {
                fupdate_channel::ProviderRequest::GetCurrent { responder } => {
                    responder.send("channel").unwrap();
                }
            },
            |cmd| assert_matches!(cmd, None),
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_target() {
        perform_channel_test(
            args::channel::Command::Target(args::channel::Target {}),
            |cmd| assert_matches!(cmd, None),
            |cmd| match cmd.unwrap() {
                ChannelControlRequest::GetTarget { responder } => {
                    responder.send("target-channel").unwrap();
                }
                request => panic!("Unexpected request: {request:?}"),
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_set() {
        perform_channel_test(
            args::channel::Command::Set(args::channel::Set { channel: "new-channel".to_string() }),
            |cmd| assert_matches!(cmd, None),
            |cmd| match cmd.unwrap() {
                ChannelControlRequest::SetTarget { channel, responder } => {
                    assert_eq!(channel, "new-channel");
                    responder.send().unwrap();
                }
                request => panic!("Unexpected request: {request:?}"),
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_channel_list() {
        perform_channel_test(
            args::channel::Command::List(args::channel::List {}),
            |cmd| assert_matches!(cmd, None),
            |cmd| match cmd.unwrap() {
                ChannelControlRequest::GetTargetList { responder } => {
                    responder
                        .send(&["some-channel".to_owned(), "other-channel".to_owned()])
                        .unwrap();
                }
                request => panic!("Unexpected request: {request:?}"),
            },
        )
        .await;
    }
}
