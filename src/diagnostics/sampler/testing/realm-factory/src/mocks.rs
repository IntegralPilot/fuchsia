// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_id_index::{Index, IndexEntry, InstanceId};
use fidl_fuchsia_hardware_power_statecontrol as reboot;
use fidl_fuchsia_mockrebootcontroller as controller;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{SinkExt, StreamExt, TryStreamExt};
use moniker::Moniker;
use std::str::FromStr;
use std::sync::Arc;

/// Test data for moniker <-> ID file.
/// This will be sent to Sampler as though it were coming from Product Assembly.
pub(crate) fn fake_index() -> Index {
    let mut index = Index::default();
    let id: InstanceId =
        InstanceId::from_str("1111222233334444111111111111111111111111111111111111111111111111")
            .unwrap();
    index
        .insert(IndexEntry {
            moniker: Moniker::try_from("integer_42").unwrap(),
            instance_id: id,
            ignore_duplicate_id: false,
        })
        .unwrap();
    let id: InstanceId =
        InstanceId::from_str("2222222233334444111111111111111111111111111111111111111111112222")
            .unwrap();
    index
        .insert(IndexEntry {
            moniker: Moniker::try_from("not_listed_1").unwrap(),
            instance_id: id,
            ignore_duplicate_id: false,
        })
        .unwrap();
    index
}

pub fn serve_shutdown_server(
    mut stream: reboot::ShutdownWatcherRegisterRequestStream,
    mut proxy_sender: mpsc::Sender<reboot::ShutdownWatcherProxy>,
) {
    fasync::Task::spawn(async move {
        while let Some(req) = stream.try_next().await.unwrap() {
            match req {
                reboot::ShutdownWatcherRegisterRequest::RegisterWatcher { watcher, responder } => {
                    proxy_sender.send(watcher.into_proxy()).await.unwrap();
                    responder.send().unwrap();
                }
                _ => {
                    unimplemented!();
                }
            }
        }
    })
    .detach();
}

pub fn serve_reboot_controller(
    mut stream: controller::MockRebootControllerRequestStream,
    proxy_receiver: Arc<Mutex<mpsc::Receiver<reboot::ShutdownWatcherProxy>>>,
) {
    fasync::Task::spawn(async move {
        while let Some(req) = stream.try_next().await.unwrap() {
            let proxy = proxy_receiver.lock().await.next().await.unwrap();
            match req {
                controller::MockRebootControllerRequest::TriggerReboot { responder } => match proxy
                    .on_shutdown(&reboot::ShutdownOptions {
                        action: Some(reboot::ShutdownAction::Reboot),
                        reasons: Some(vec![reboot::ShutdownReason::UserRequest]),
                        __source_breaking: fidl::marker::SourceBreaking,
                    })
                    .await
                {
                    Err(_) => {
                        responder.send(Err(controller::RebootError::ClientError)).unwrap();
                    }
                    Ok(()) => {
                        responder.send(Ok(())).unwrap();
                    }
                },
                controller::MockRebootControllerRequest::CrashRebootChannel { responder } => {
                    drop(proxy);
                    responder.send(Ok(())).unwrap();
                }
            }
        }
    })
    .detach();
}
