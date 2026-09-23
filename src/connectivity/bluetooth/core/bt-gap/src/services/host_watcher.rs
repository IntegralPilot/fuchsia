// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use async_helpers::hanging_get::asynchronous as hanging_get;
use fidl_fuchsia_bluetooth_sys::{self as sys, HostWatcherRequest, HostWatcherRequestStream};
use fuchsia_bluetooth::types::host_info::HostInfo;

use futures::StreamExt;
use log::warn;

use crate::host_dispatcher::*;

pub async fn run(hd: HostDispatcher, mut stream: HostWatcherRequestStream) -> Result<(), Error> {
    let mut watch_hosts_subscriber = hd.watch_hosts().await;
    while let Some(event) = stream.next().await {
        handler(hd.clone(), &mut watch_hosts_subscriber, event?).await?;
    }
    Ok(())
}

async fn handler(
    hd: HostDispatcher,
    watch_hosts_subscriber: &mut hanging_get::Subscriber<sys::HostWatcherWatchResponder>,
    request: HostWatcherRequest,
) -> Result<(), Error> {
    match request {
        HostWatcherRequest::Watch { responder } => {
            watch_hosts_subscriber.register(responder).await.map_err(|e| {
                // If we cannot register the observation, we return an error from the handler
                // function. This terminates the stream and will drop the channel, as we are unable
                // to fulfill our contract for Watch(). The client can attempt to reconnect and
                // if successful will receive a fresh session with initial state of the world
                format_err!("Failed to watch hosts: {:?}", e)
            })
        }
        HostWatcherRequest::SetActive { id, responder } => {
            let result =
                hd.set_active_host(id.into()).map_err(|_| zx::Status::NOT_FOUND.into_raw());
            responder.send(result).map_err(Error::from)
        }
    }
}

// Written as a free function in order to match the signature of the HangingGet
pub fn observe_hosts(new_hosts: &Vec<HostInfo>, responder: sys::HostWatcherWatchResponder) -> bool {
    let hosts: Vec<_> = new_hosts.iter().map(|host| sys::HostInfo::from(host)).collect();
    if let Err(err) = responder.send(&hosts) {
        warn!("Unable to respond to host_watcher watch hanging get: {:?}", err);
    }
    true
}
