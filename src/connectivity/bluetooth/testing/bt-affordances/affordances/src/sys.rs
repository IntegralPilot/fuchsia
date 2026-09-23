// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::proxies::Proxies;
use anyhow::anyhow;
use fidl_fuchsia_bluetooth::PeerId;
use fidl_fuchsia_bluetooth_sys::{AccessSetConnectionPolicyRequest, HostInfo, Peer};
use fuchsia_async::{TimeoutExt, Timer};
use fuchsia_sync::Mutex;
use futures::StreamExt;
use std::sync::Arc;

pub(crate) fn update_peer_cache(
    peer_cache: Arc<Mutex<Vec<Peer>>>,
    updated: Vec<Peer>,
    removed: Vec<PeerId>,
) {
    let mut peer_cache = peer_cache.lock();
    if !removed.is_empty() {
        peer_cache.retain(|peer| !removed.contains(&peer.id.unwrap()));
    }
    for updated_peer in updated {
        peer_cache.retain(|peer| peer.id.unwrap() != updated_peer.id.unwrap());
        peer_cache.push(updated_peer);
    }
}

pub(crate) async fn refresh_peer_cache(
    proxies: &mut Proxies,
    timeout: std::time::Duration,
    peer_cache: Arc<Mutex<Vec<Peer>>>,
) -> Result<(), fidl::Error> {
    match proxies.peer_watcher_stream.next().on_timeout(timeout, || None).await {
        Some(Ok((updated, removed))) => {
            update_peer_cache(peer_cache, updated, removed);
            Ok(())
        }
        Some(Err(err)) => Err(err),
        None => Ok(()),
    }
}

// TODO(https://fxbug.dev/450986278): Migrate to fasync::MonotonicInstant.
pub(crate) async fn get_peer(
    proxies: &mut Proxies,
    address: [u8; 6],
    mut timeout: std::time::Duration,
    peer_cache: Arc<Mutex<Vec<Peer>>>,
) -> Result<Option<Peer>, anyhow::Error> {
    let addr_matches =
        |peer: &Peer| peer.address.as_ref().map_or(false, |addr| addr.bytes == address);
    if let Some(peer) = peer_cache.lock().iter().find(|peer: &&Peer| addr_matches(peer)) {
        return Ok(Some(peer.clone()));
    }

    let (_token, discovery_session_server) = fidl::endpoints::create_proxy();
    let second = std::time::Duration::from_secs(1);
    if timeout >= second {
        timeout -= second;
        if let Err(err) = proxies.access_proxy.start_discovery(discovery_session_server).await? {
            return Err(anyhow!("fuchsia.bluetooth.sys.Access/StartDiscovery error: {err:?}"));
        }
        // Allow discovery session to activate.
        Timer::new(std::time::Duration::from_secs(5)).await;
    }

    refresh_peer_cache(proxies, timeout, peer_cache.clone()).await?;
    if let Some(peer) = peer_cache.lock().iter().find(|peer: &&Peer| addr_matches(peer)) {
        return Ok(Some(peer.clone()));
    }
    Ok(None)
}

pub(crate) async fn refresh_host_cache<'a>(
    proxies: &mut Proxies,
    host_cache: &'a mut Vec<HostInfo>,
) -> Result<(), anyhow::Error> {
    if let Some(host_watcher_result) = proxies
        .host_watcher_stream
        .next()
        .on_timeout(std::time::Duration::from_millis(100), || None)
        .await
    {
        let Ok(new_host_list) = host_watcher_result else {
            return Err(anyhow!(
                "fuchsia.bluetooth.sys.HostWatcher error: {}",
                host_watcher_result.unwrap_err()
            ));
        };
        *host_cache = new_host_list
    }
    Ok(())
}

pub(crate) async fn set_discovery(
    proxies: &mut Proxies,
    discovery: bool,
) -> Result<(), anyhow::Error> {
    if !discovery {
        if proxies.discovery_session.lock().take().is_none() {
            eprintln!("Asked to revoke nonexistent discovery session.");
        }
        return Ok(());
    }

    if proxies.discovery_session.lock().is_some() {
        return Ok(());
    }

    let (token, discovery_session_server) = fidl::endpoints::create_proxy();
    if let Err(err) = proxies.access_proxy.start_discovery(discovery_session_server).await? {
        return Err(anyhow!("fuchsia.bluetooth.sys.Access/StartDiscovery error: {err:?}"));
    }

    {
        let mut session = proxies.discovery_session.lock();
        if session.is_none() {
            *session = Some(token);
        }
    }

    // Allow discovery session to activate.
    Timer::new(std::time::Duration::from_secs(1)).await;
    Ok(())
}

pub(crate) async fn set_discoverability(
    proxies: &mut Proxies,
    discoverable: bool,
) -> Result<(), anyhow::Error> {
    if !discoverable {
        if proxies.discoverability_session.lock().take().is_none() {
            eprintln!("Asked to revoke nonexistent discoverability session.");
        }
        return Ok(());
    }

    if proxies.discoverability_session.lock().is_some() {
        return Ok(());
    }
    let (token, discoverability_session_server) = fidl::endpoints::create_proxy();
    if let Err(err) = proxies.access_proxy.make_discoverable(discoverability_session_server).await?
    {
        return Err(anyhow!("fuchsia.bluetooth.sys.Access/MakeDiscoverable error: {err:?}"));
    }

    {
        let mut session = proxies.discoverability_session.lock();
        if session.is_none() {
            *session = Some(token);
        }
    }

    Ok(())
}

pub(crate) async fn set_connectability(
    proxies: &Proxies,
    connectable: bool,
) -> Result<(), anyhow::Error> {
    {
        let mut suppress_connections_session = proxies.suppress_connections_session.lock();
        if connectable {
            if suppress_connections_session.take().is_none() {
                eprintln!("Device is already connectable.");
            }
            return Ok(());
        }
        if suppress_connections_session.is_some() {
            return Ok(());
        }
    }
    let (token, suppress_connections_server) = fidl::endpoints::create_proxy();
    if let Err(err) = proxies
        .access_proxy
        .set_connection_policy(AccessSetConnectionPolicyRequest {
            suppress_bredr_connections: Some(suppress_connections_server),
            ..Default::default()
        })
        .await?
    {
        return Err(anyhow!("fuchsia.bluetooth.sys.Access/SetConnectionPolicy error: {err:?}"));
    }
    *proxies.suppress_connections_session.lock() = Some(token);
    Ok(())
}
