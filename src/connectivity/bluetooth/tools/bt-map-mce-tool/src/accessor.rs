// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl::endpoints::{create_endpoints, ServerEnd};
use fidl_fuchsia_bluetooth::PeerId;
use fidl_fuchsia_bluetooth_map::{
    AccessorProxy, AccessorSetNotificationRegistrationRequest, NotificationRegistrationMarker,
    NotificationRegistrationRequest,
};
use fuchsia_async::Task;
use fuchsia_sync::Mutex;
use futures::stream::StreamExt;
use log::{info, warn};
use std::sync::Arc;

use crate::commands::Cmd;

#[derive(Clone)]
pub(crate) struct AccessorClient {
    peer_id: PeerId,
    proxy: AccessorProxy,
    notification_task: Arc<Mutex<Option<Task<()>>>>,
}

impl AccessorClient {
    pub fn new(peer_id: PeerId, proxy: AccessorProxy) -> Self {
        Self { peer_id, proxy, notification_task: Arc::new(Mutex::new(None)) }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }
}

pub async fn list_all_mas_instances(client: AccessorClient) -> Result<(), Error> {
    let res = client.proxy.list_all_mas_instances().await;
    match res {
        Ok(Ok(instances)) => {
            info!("Peer ({:?}) has {:} MAS instances", client.peer_id, instances.len());
            for r in instances {
                info!("\t{r:?}");
            }
        }
        Ok(Err(e)) => warn!(e:?; "Command failed"),
        Err(e) => return Err(format_err!("{e:?}")),
    }
    Ok(())
}

// Takes the NotificationRegistration FIDL server end and prints incoming notifications.
async fn print_notifications(relayer_server: ServerEnd<NotificationRegistrationMarker>) {
    let mut notification_stream = relayer_server.into_stream().fuse();
    while let Some(res) = notification_stream.next().await {
        if let Err(e) = res {
            warn!(e:?; "Error with FIDL stream");
            break;
        }
        match res.unwrap() {
            NotificationRegistrationRequest::NewEventReport { payload, responder } => {
                info!("NOTIFICATION: {payload:?}");
                let _ = responder.send();
            }
            other => {
                info!("Unknown request: {:?}", other);
            }
        }
    }
}

pub async fn register_for_notifications<'a>(
    args: &'a [&'a str],
    accessor_client: AccessorClient,
) -> Result<(), Error> {
    if args.len() > 1 {
        info!("usage: {}", Cmd::TurnOnNotifications.cmd_help());
        return Ok(());
    }
    if accessor_client.notification_task.lock().is_some() {
        info!("No/op. There is a pre-existing notification registration. To reset, run `{}` and try again", Cmd::TurnOffNotifications);
        return Ok(());
    }

    let mut instance_ids = vec![];
    if args.len() == 1 {
        // TODO(https://fxbug.dev/365179989): support multiple repo UIDS.
        instance_ids = vec![args[0].to_string().parse::<u8>()?];
    }
    let (relayer_client, relayer_server) = create_endpoints::<NotificationRegistrationMarker>();
    match accessor_client
        .proxy
        .set_notification_registration(AccessorSetNotificationRegistrationRequest {
            mas_instance_ids: Some(instance_ids),
            server: Some(relayer_client),
            ..Default::default()
        })
        .await
    {
        Ok(Ok(())) => {
            info!("Successfully registered for notifications");
        }
        Ok(Err(e)) => {
            warn!(e:?; "Command failed");
            return Ok(());
        }
        Err(e) => return Err(format_err!("{e:?}")),
    };

    let mut lock = accessor_client.notification_task.lock();
    let _ = lock.replace(fuchsia_async::Task::local(print_notifications(relayer_server)));
    info!("Incoming notifications will be printed to console. To stop receiving notifications, use `{}`", Cmd::TurnOffNotifications);
    Ok(())
}

pub async fn unregister_notifications<'a>(accessor_client: AccessorClient) -> Result<(), Error> {
    let mut lock = accessor_client.notification_task.lock();
    match lock.take() {
        Some(previous_task) => {
            let _ = previous_task.abort();
            info!("Will no longer receive notifications from peer...",);
        }
        None => info!("No/op since there is no active notification registration",),
    };
    info!("To turn on notifications, use `{}`", Cmd::TurnOnNotifications);
    Ok(())
}
