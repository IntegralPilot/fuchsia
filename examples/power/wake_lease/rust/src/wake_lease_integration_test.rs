// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::prelude::*;
use stream::StreamExt;
use wake_lease::WakeLease;

struct SuspendBlocker {
    before_suspend_sender: mpsc::UnboundedSender<()>,
}

impl SuspendBlocker {
    async fn run(&self, stream: fsystem::SuspendBlockerRequestStream) -> Result<()> {
        let before_suspend_sender = self.before_suspend_sender.clone();
        stream
            .map(|request| request.context("failed request"))
            .try_for_each(|request| async {
                match request {
                    fsystem::SuspendBlockerRequest::AfterResume { responder } => {
                        responder.send().context("send failed")
                    }
                    fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                        assert!(before_suspend_sender.unbounded_send(()).is_ok());
                        responder.send().context("send failed")
                    }
                    _ => unreachable!(),
                }
            })
            .await
    }
}

#[fuchsia::test]
async fn wake_lease_blocks_system_suspend_until_release() -> Result<()> {
    let topology = connect_to_protocol::<fbroker::TopologyMarker>()?;
    let sag = connect_to_protocol::<fsystem::ActivityGovernorMarker>()?;
    let boot_control = connect_to_protocol::<fsystem::BootControlMarker>()?;

    // Fetch the dependency token for ApplicationActivity.
    let power_elements = sag.get_power_elements().await?;
    let activity_token =
        power_elements.application_activity.unwrap().assertive_dependency_token.unwrap();

    // Take an assertive lease on ApplicationActivity to indicate boot completion.
    // System Activity Governor waits for this signal before handling suspend or resume.
    let (lease_token, activity_lease) = fbroker::LeaseToken::create();
    topology
        .lease(fbroker::LeaseSchema {
            lease_token: Some(lease_token),
            lease_name: Some("boot-complete-lease".into()),
            dependencies: Some(vec![fbroker::LeaseDependency {
                requires_token: Some(activity_token),
                requires_level: Some(fsystem::ApplicationActivityLevel::Active.into_primitive()),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .await?
        .map_err(|e| anyhow::anyhow!("Lease error: {e:?}"))?;
    let _ = boot_control.set_boot_complete().await?;

    // Create and take a wake lease, ensuring the system doesn't suspend.
    let wake_lease = WakeLease::acquire(&sag, "test-wake-lease".to_string()).await?;

    // Register a suspend blocker on System Activity Governor to check for suspend callbacks.
    let (client, stream) = create_request_stream::<fsystem::SuspendBlockerMarker>();
    let (before_suspend_sender, mut before_suspend_receiver) = mpsc::unbounded();
    fasync::Task::local(async move {
        let suspend_blocker = SuspendBlocker { before_suspend_sender };
        suspend_blocker.run(stream).await.expect("SuspendBlocker server completion");
        unreachable!(); // Suspend blocker should run for the entire test.
    })
    .detach();

    // The RegisterSuspendBlocker call returns another wake lease. Functionally, we could replace
    // the `wake_lease` from above with the one that's obtained here, but we want this example to
    // clearly demonstrate that the token returned by AcquireWakeLease will block suspension.
    {
        let _registration_lease = sag
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(client),
                name: Some("test_suspend_blocker".into()),
                ..Default::default()
            })
            .await?
            .expect("error registering suspend blocker");
    }

    assert!(before_suspend_receiver.try_next().is_err()); // OnSuspend not called yet.

    // Closing the ApplicationActivity lease shouldn't cause the system to suspend as long as
    // the wake lease is active.
    drop(activity_lease);
    assert!(before_suspend_receiver.try_next().is_err()); // OnSuspend not called yet.

    // Release the wake lease and observe a suspend callback within a timeout.
    drop(wake_lease);
    before_suspend_receiver.next().await; // OnSuspend called.

    Ok(())
}
