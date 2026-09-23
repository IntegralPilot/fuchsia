// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_starnix_runner as fstarnix;
use fuchsia_async as fasync;
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams, RealmInstance};
use futures::task::Poll;
use std::pin::pin;
use zx::Peered;

const AWAKE_SIGNAL: zx::Signals = zx::Signals::USER_0;
const ASLEEP_SIGNAL: zx::Signals = zx::Signals::USER_1;

async fn build_realm() -> RealmInstance {
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url("#meta/realm.cm"))
            .await
            .expect("created");
    builder.build().await.unwrap()
}

#[fuchsia::test]
async fn test_register_wake_watcher() {
    let realm_instance = build_realm().await;
    let manager: fstarnix::ManagerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let job = fuchsia_runtime::job_default();
    let child_job = job.create_child_job().unwrap();

    let (wake_watcher, wake_watcher_remote) = zx::EventPair::create();

    manager
        .register_wake_watcher(fstarnix::ManagerRegisterWakeWatcherRequest {
            watcher: Some(wake_watcher_remote),
            ..Default::default()
        })
        .await
        .unwrap();

    // Get initial AWAKE SIGNAL.
    fasync::OnSignals::new(&wake_watcher, AWAKE_SIGNAL).await.expect("awake");

    let mut signal_fut = pin!(fasync::OnSignals::new(&wake_watcher, ASLEEP_SIGNAL));
    assert_eq!(Poll::Pending, futures::poll!(&mut signal_fut));

    let suspend_fut = manager.suspend_container(fstarnix::ManagerSuspendContainerRequest {
        container_job: Some(child_job.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
        wake_locks: None,
        ..Default::default()
    });

    signal_fut.await.expect("asleep future");

    fasync::OnSignals::new(&wake_watcher, AWAKE_SIGNAL).await.expect("final awake signal");
    let _ = suspend_fut.await;
}

#[fuchsia::test]
async fn test_wake_lock() {
    let realm_instance = build_realm().await;
    let manager: fstarnix::ManagerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let (wake_lock, wake_lock_remote) = zx::EventPair::create();
    let job = fuchsia_runtime::job_default();
    let child_job = job.create_child_job().unwrap();

    // If we signal on the wake lock then suspend will fail.
    wake_lock.signal_peer(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
    assert!(matches!(
        manager
            .suspend_container(fstarnix::ManagerSuspendContainerRequest {
                container_job: Some(child_job.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                wake_locks: Some(wake_lock_remote),
                ..Default::default()
            })
            .await
            .unwrap(),
        Err(fstarnix::SuspendError::WakeLocksExist)
    ));
}
