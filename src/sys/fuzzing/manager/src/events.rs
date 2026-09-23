// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Context as _, Result};
use fidl_fuchsia_test_manager as test_manager;
use futures::channel::{mpsc, oneshot};
use futures::{pin_mut, select, FutureExt, SinkExt};

pub type LaunchResult = std::result::Result<(), test_manager::LaunchError>;

pub async fn handle_suite_events(
    suite_proxy: test_manager::SuiteControllerProxy,
    mut artifact_sender: mpsc::UnboundedSender<test_manager::Artifact>,
    start_sender: oneshot::Sender<LaunchResult>,
    kill_receiver: oneshot::Receiver<()>,
) -> Result<()> {
    // Wrap |start_sender| in an option to enforce using it at most once.
    let mut start_sender = Some(start_sender);

    let kill_fut = kill_receiver.fuse();
    pin_mut!(kill_fut);

    loop {
        let events_fut = suite_proxy.watch_events().fuse();
        pin_mut!(events_fut);
        let events = select! {
            result = kill_fut => {
                if result.is_ok() {
                    suite_proxy.kill().context("fuchsia.test.manager.SuiteController/Kill")?;
                }
                break;
            }
            result = events_fut => match result.context("fuchsia.test.manager.SuiteController/WatchEvents")? {
                Ok(events) => events,
                Err(e) => {
                    if let Some(start_sender) = start_sender.take() {
                        start_sender
                            .send(Err(e))
                            .map_err(|_| anyhow!("failed to send launch error"))?;
                    }
                    break;
                }
            }
        };

        if events.is_empty() {
            break;
        }
        for event in events {
            match event.details {
                Some(test_manager::EventDetails::SuiteStarted(_)) => {
                    if let Some(start_sender) = start_sender.take() {
                        start_sender
                            .send(Ok(()))
                            .map_err(|_| anyhow!("failed to send launch result"))?;
                    }
                }
                Some(test_manager::EventDetails::SuiteArtifactGenerated(details)) => {
                    artifact_sender
                        .send(details.artifact.expect("suite artifact details have artifact"))
                        .await
                        .context("failed to send suite artifact")?;
                }
                Some(test_manager::EventDetails::TestCaseArtifactGenerated(details)) => {
                    artifact_sender
                        .send(details.artifact.expect("test case artifact details have artifact"))
                        .await
                        .context("failed to send case artifact")?;
                }
                _ => {}
            };
        }
    }
    Ok(())
}
