// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
pub use crate::fidl_pipe::{FidlPipe, create_overnet_socket};
use crate::info::{self, TargetInfo};
pub use crate::resolve::{
    DefaultTargetResolver, Resolution, TargetResolver, get_discovery_stream,
    maybe_locally_resolve_target_spec, resolve_target_address,
};
use crate::{KnockCriticalError, KnockError, KnockNonCriticalError, TargetInfoQuery};

use anyhow::Result;
use ffx_config::EnvironmentContext;
use fuchsia_async::TimeoutExt;
use futures::StreamExt;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const DEFAULT_SSH_TIMEOUT_MS: u64 = 10000;
async fn try_get_target_info(
    spec: TargetInfoQuery,
    context: &EnvironmentContext,
) -> Result<
    (info::RemoteControlState, Option<String>, Option<String>, Option<u64>, Option<String>),
    KnockError,
> {
    // We want to make sure to provide an up-to-date list, so don't rely on the cache
    let resolution = resolve_target_address(&spec, false, context)
        .await
        .map_err(|e| KnockError::Critical(KnockCriticalError::TargetError(format!("{:?}", e))))?;
    let (rcs_state, pc, bc, bi, sn) = match resolution.identify(context).await {
        Ok(id_result) => (
            info::RemoteControlState::Up,
            id_result.product_config,
            id_result.board_config,
            id_result.boot_id,
            id_result.serial_number,
        ),
        _ => (info::RemoteControlState::Down, None, None, None, None),
    };
    Ok((rcs_state, pc, bc, bi, sn))
}

async fn get_target_info(
    context: &EnvironmentContext,
    addrs: &[addr::TargetAddr],
) -> Result<(info::RemoteControlState, Option<String>, Option<String>, Option<u64>, Option<String>)>
{
    let ssh_timeout: u64 =
        context.get("target.host_pipe_ssh_timeout").unwrap_or(DEFAULT_SSH_TIMEOUT_MS);
    let ssh_timeout = Duration::from_millis(ssh_timeout);
    for addr in addrs {
        let query = TargetInfoQuery::from(*addr);
        log::debug!("Trying to make a connection to query {query:?}");
        match try_get_target_info(query, context)
            .on_timeout(ssh_timeout, || {
                Err(KnockError::NonCritical(KnockNonCriticalError::Timeout {
                    detail: "knock_rcs() timed out".to_string(),
                }))
            })
            .await
        {
            Ok(res) => {
                return Ok(res);
            }
            Err(KnockError::NonCritical(e)) => {
                log::debug!("Could not connect to {addr:?}: {e:?}");
                continue;
            }
            e => {
                log::debug!("Got error {e:?} when trying to connect to {addr:?}");
                return Ok((info::RemoteControlState::Unknown, None, None, None, None));
            }
        }
    }
    Ok((info::RemoteControlState::Down, None, None, None, None))
}

// Convert the handle to a TargetInfo, filling in the information from the target if we are
// asked to make a connection to RCS.
async fn handle_to_info(
    context: &EnvironmentContext,
    handle: discovery::TargetHandle,
    connect_to_target: bool,
    query: TargetInfoQuery,
) -> Result<TargetInfo> {
    let (rcs_state, product_config, board_config, boot_id, serial_number) =
        if let discovery::TargetState::Product { ref addrs, .. } = handle.state {
            // A let-chain would be cleaner, but they are only available in Rust 2024
            if connect_to_target {
                get_target_info(context, addrs).await?
            } else {
                (info::RemoteControlState::Unknown, None, None, None, None)
            }
        } else {
            (info::RemoteControlState::Unknown, None, None, None, None)
        };
    let info: TargetInfo = handle.into();
    let is_default = Some(info.match_query(&query));
    Ok(TargetInfo {
        rcs_state,
        board_config,
        product_config,
        boot_id,
        is_default,
        serial_number: serial_number.or_else(|| info.serial_number.clone()),
        ..info
    })
}

async fn handles_to_infos(
    stream: impl futures::Stream<Item = discovery::TargetHandle>,
    ctx: &EnvironmentContext,
    connect: bool,
) -> Result<Vec<TargetInfo>> {
    let default = TargetInfoQuery::try_from(crate::get_target_specifier(ctx)?)?;

    let infos: Vec<Result<TargetInfo>> = stream
        .map(|t| handle_to_info(ctx, t, connect, default.clone()))
        .buffered(16)
        .collect()
        .await;
    let targets = infos.into_iter().collect::<Result<Vec<_>>>()?;
    let targets = merge_target_addrs(targets);
    Ok(targets)
}

// Merge targets that have the same serial number or boot_id.
//
// We prefer merging by boot_id first, as it is more reliable (it only ever
// matches the same device boot instance, whereas serial numbers can be duplicate
// in misconfigured environments). If a target has a boot_id, we combine its
// addresses with any other targets sharing the same boot_id.
//
// If a target has no boot_id but has a serial number, we fall back to merging
// by serial number.
fn merge_target_addrs(targets: Vec<TargetInfo>) -> Vec<TargetInfo> {
    let mut boot_merged: HashMap<u64, TargetInfo> = HashMap::with_capacity(targets.len());
    let mut serial_merged: HashMap<String, TargetInfo> = HashMap::with_capacity(targets.len());
    let mut unmerged = HashSet::with_capacity(targets.len());

    for mut t in targets {
        t.addresses.sort();
        t.addresses.dedup();

        if let Some(boot_id) = t.boot_id {
            match boot_merged.entry(boot_id) {
                Entry::Occupied(mut e) => merge_infos(e.get_mut(), t),
                Entry::Vacant(e) => {
                    e.insert(t);
                }
            }
        } else if let Some(serial) = t.serial_number.as_ref().filter(|s| !s.is_empty()) {
            match serial_merged.entry(serial.clone()) {
                Entry::Occupied(mut e) => merge_infos(e.get_mut(), t),
                Entry::Vacant(e) => {
                    e.insert(t);
                }
            }
        } else {
            unmerged.insert(t);
        }
    }

    let mut result = Vec::with_capacity(boot_merged.len() + serial_merged.len() + unmerged.len());
    result.extend(boot_merged.into_values());

    for t in serial_merged.into_values() {
        if let Some(serial) = &t.serial_number {
            let mut matches = result
                .iter_mut()
                .filter(|x| x.serial_number.as_ref() == Some(serial))
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                merge_infos(matches[0], t);
                continue;
            } else if matches.len() > 1 {
                let boot_ids = matches.iter().map(|x| x.boot_id).collect::<Vec<_>>();
                log::warn!(
                    "Multiple targets discovered with the same serial number {} but different boot IDs ({:?}). Ambiguous serial target will not be merged.",
                    serial,
                    boot_ids
                );
            }
        }
        result.push(t);
    }

    result.extend(unmerged);
    result
}

fn merge_infos(a: &mut TargetInfo, b: TargetInfo) {
    a.addresses.extend(b.addresses);
    a.addresses.sort();
    a.addresses.dedup();

    a.serial_number = a.serial_number.take().or(b.serial_number);
    a.boot_id = a.boot_id.take().or(b.boot_id);
    a.product_config = a.product_config.take().or(b.product_config);
    a.board_config = a.board_config.take().or(b.board_config);

    if a.rcs_state == info::RemoteControlState::Unknown
        || a.rcs_state == info::RemoteControlState::Down
    {
        a.rcs_state = b.rcs_state;
    }
    if a.target_state == info::TargetState::Unknown {
        a.target_state = b.target_state;
    }

    a.is_default = match (a.is_default, b.is_default) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        (None, None) => None,
    };
}

pub async fn list_targets(
    ctx: &EnvironmentContext,
    query: TargetInfoQuery,
    include_usb: bool,
    include_mdns: bool,
    connect: bool,
) -> std::result::Result<Vec<TargetInfo>, crate::FfxTargetCrateError> {
    // When explicitly listing all targets, we don't want to use the
    // cache, for a couple reasons:
    // * explicitly listing the targets probably warrants accurate results
    // * if we get back a stale target, we don't want to waste time trying
    //   to connect to RCS
    let stream = get_discovery_stream(query, include_usb, include_mdns, ctx)?;
    Ok(handles_to_infos(stream, ctx, connect).await?)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::info::{RemoteControlState, TargetState};
    use addr::TargetAddr;
    use std::collections::HashSet;
    use std::net::TcpListener;

    #[fuchsia::test]
    async fn test_serial_addresses() {
        // USB targets should have an empty list of addresses
        let env = ffx_config::test_init().unwrap();
        let handle = discovery::TargetHandle {
            node_name: Some("nodename".to_string()),
            state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                serial_number: "12345678".to_string(),
                connection_state: discovery::FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let stream = futures::stream::once(async { handle });
        let targets = handles_to_infos(stream, &env.context, true).await;
        let targets = targets.unwrap();
        assert!(targets[0].addresses.is_empty());
    }

    #[fuchsia::test]
    async fn test_handle_to_info_address_sorting() {
        let env = ffx_config::test_init().unwrap();
        let non_link_local_addr: addr::TargetAddr = "[2001:db8::1]:0".parse().unwrap();
        let link_local_addr: addr::TargetAddr = "[fe80::1]:0".parse().unwrap();
        let handle = discovery::TargetHandle {
            node_name: Some("test-node".to_string()),
            state: discovery::TargetState::Product {
                addrs: vec![non_link_local_addr.clone(), link_local_addr.clone()],
                serial: None,
            },
            manual: false,
        };
        let info =
            handle_to_info(&env.context, handle, false, TargetInfoQuery::First).await.unwrap();
        let addrs = info.addresses;
        assert_eq!(addrs.len(), 2);
        let addrs: Vec<addr::TargetAddr> = addrs.into_iter().map(|a| a.into()).collect();
        // The link-local address should come first.
        assert_eq!(addrs[0], link_local_addr);
        assert_eq!(addrs[1], non_link_local_addr);
    }

    fn make_target_info(addr: TargetAddr, boot_id: Option<u64>) -> TargetInfo {
        TargetInfo {
            nodename: Some("t".to_string()),
            addresses: vec![addr],
            rcs_state: RemoteControlState::Up,
            target_state: TargetState::Product,
            product_config: Some("product".to_string()),
            board_config: Some("board".to_string()),
            serial_number: None,
            is_manual: false,
            boot_id,
            is_default: None,
        }
    }

    #[fuchsia::test]
    fn test_merge_target_ip_addrs() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, Some(999));
        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let t2 = make_target_info(addr2, Some(999));
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 1);
        let merged = vec![addr1, addr2];
        let target0 = targets[0].clone();
        assert_eq!(
            HashSet::<TargetAddr>::from_iter(target0.addresses.into_iter()),
            HashSet::from_iter(merged.into_iter())
        );
    }

    #[fuchsia::test]
    fn test_merge_target_duplicate_addrs() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, Some(999));
        let t2 = make_target_info(addr1, Some(999));
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 1);
        let target0 = targets[0].clone();
        assert_eq!(target0.addresses.len(), 1);
        assert_eq!(target0.addresses[0], addr1);
    }

    #[fuchsia::test]
    fn test_merge_target_non_ip_addrs() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, Some(999));
        let addr2: addr::TargetAddr = addr::TargetAddr::VSockCtx(123);
        let t2 = make_target_info(addr2, Some(999));
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 1);
        let merged = vec![addr1, addr2];
        let target0 = targets[0].clone();
        assert_eq!(
            HashSet::<TargetAddr>::from_iter(target0.addresses.into_iter()),
            HashSet::from_iter(merged.into_iter())
        );
    }

    #[fuchsia::test]
    fn test_merge_target_distinct_bootids() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, Some(888));
        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let t2 = make_target_info(addr2, Some(999));
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 2);
    }

    #[fuchsia::test]
    fn test_merge_target_no_bootids() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, None);
        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let t2 = make_target_info(addr2, None);
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 2);
    }

    #[fuchsia::test]
    fn test_merge_target_one_bootid() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let t1 = make_target_info(addr1, Some(999));
        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let t2 = make_target_info(addr2, None);
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 2);
    }

    #[fuchsia::test]
    fn test_merge_target_duplicate_targets_no_bootid() {
        let addr1: addr::TargetAddr = "127.0.0.1:1".parse().unwrap();
        let t1 = make_target_info(addr1, None);
        let t2 = make_target_info(addr1, None);
        let t3 = make_target_info(addr1, None);
        let targets = merge_target_addrs(vec![t1, t2, t3]);
        assert_eq!(targets.len(), 1);
    }

    #[fuchsia::test]
    fn test_merge_target_by_serial() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let mut t1 = make_target_info(addr1, Some(999));
        t1.serial_number = Some("serial-123".to_string());

        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let mut t2 = make_target_info(addr2, None);
        t2.serial_number = Some("serial-123".to_string());

        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets.len(), 1);

        let target0 = &targets[0];
        assert_eq!(target0.serial_number.as_deref(), Some("serial-123"));
        assert_eq!(target0.boot_id, Some(999));
        assert_eq!(target0.addresses.len(), 2);
    }

    #[fuchsia::test]
    fn test_merge_target_is_default() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let mut t1 = make_target_info(addr1, Some(999));
        t1.serial_number = Some("serial-123".to_string());
        t1.is_default = Some(true);

        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let mut t2 = make_target_info(addr2, None);
        t2.serial_number = Some("serial-123".to_string());
        t2.is_default = None;

        let targets = merge_target_addrs(vec![t1.clone(), t2.clone()]);
        assert_eq!(targets[0].is_default, Some(true));

        t1.is_default = None;
        t2.is_default = Some(true);
        let targets = merge_target_addrs(vec![t1.clone(), t2.clone()]);
        assert_eq!(targets[0].is_default, Some(true));

        t1.is_default = Some(false);
        t2.is_default = None;
        let targets = merge_target_addrs(vec![t1.clone(), t2.clone()]);
        assert_eq!(targets[0].is_default, Some(false));

        t1.is_default = None;
        t2.is_default = None;
        let targets = merge_target_addrs(vec![t1, t2]);
        assert_eq!(targets[0].is_default, None);
    }

    #[fuchsia::test]
    fn test_merge_target_multiple_boots_same_serial() {
        let addr1: addr::TargetAddr = "[fe80::1]:1".parse().unwrap();
        let mut t1 = make_target_info(addr1, Some(999));
        t1.serial_number = Some("serial-123".to_string());
        t1.nodename = Some("node-1".to_string());

        let addr2: addr::TargetAddr = "[fe80::1]:2".parse().unwrap();
        let mut t2 = make_target_info(addr2, Some(888));
        t2.serial_number = Some("serial-123".to_string());
        t2.nodename = Some("node-2".to_string());

        let addr3: addr::TargetAddr = "[fe80::1]:3".parse().unwrap();
        let mut t3 = make_target_info(addr3, None);
        t3.serial_number = Some("serial-123".to_string());
        t3.nodename = Some("node-3".to_string());

        let targets = merge_target_addrs(vec![t1, t2, t3]);
        assert_eq!(targets.len(), 3);

        let mut addr_lens: Vec<usize> = targets.iter().map(|x| x.addresses.len()).collect();
        addr_lens.sort();
        assert_eq!(addr_lens, vec![1, 1, 1]);
    }

    #[fuchsia::test]
    async fn test_handle_to_info_is_default() {
        let env = ffx_config::test_init().unwrap();
        let matching_nodename = "matching-node".to_string();
        let non_matching_nodename = "non-matching-node".to_string();
        let query = TargetInfoQuery::try_from(matching_nodename.clone()).unwrap();

        // Test with a matching target
        let matching_handle = discovery::TargetHandle {
            node_name: Some(matching_nodename.clone()),
            state: discovery::TargetState::Product { addrs: vec![], serial: None },
            manual: false,
        };
        let info =
            handle_to_info(&env.context, matching_handle, false, query.clone()).await.unwrap();
        assert_eq!(info.is_default, Some(true));

        // Test with a non-matching target
        let non_matching_handle = discovery::TargetHandle {
            node_name: Some(non_matching_nodename.clone()),
            state: discovery::TargetState::Product { addrs: vec![], serial: None },
            manual: false,
        };
        let info =
            handle_to_info(&env.context, non_matching_handle, false, query.clone()).await.unwrap();
        assert_eq!(info.is_default, Some(false));
    }

    #[fuchsia::test]
    async fn test_handle_to_info_serial_number() {
        let env = ffx_config::test_init().unwrap();
        let handle = discovery::TargetHandle {
            node_name: Some("test-node".to_string()),
            state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                serial_number: "fastboot_serial".to_string(),
                connection_state: discovery::FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let query = TargetInfoQuery::First;
        let info = handle_to_info(&env.context, handle, false, query).await.unwrap();
        assert_eq!(info.serial_number, Some("fastboot_serial".to_string()));
    }

    #[fuchsia::test]
    async fn test_handles_to_infos_concurrency() {
        let env = ffx_config::test_init().unwrap();
        let mut config = ffx_config::Config::from_env(&env.load()).unwrap();
        config
            .set(
                "target.host_pipe_ssh_timeout",
                ffx_config::ConfigLevel::User,
                serde_json::json!(50),
            )
            .unwrap();

        // Bind a local listener so the connection is accepted and hangs briefly (no Network Unreachable)
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let handle1 = discovery::TargetHandle {
            node_name: Some("target1-hang".to_string()),
            state: discovery::TargetState::Product {
                addrs: vec![addr::TargetAddr::Net(addr)],
                serial: None,
            },
            manual: false,
        };

        let handle2 = discovery::TargetHandle {
            node_name: Some("target2-fast".to_string()),
            state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                serial_number: "2222".to_string(),
                connection_state: discovery::FastbootConnectionState::Usb,
            }),
            manual: false,
        };

        let stream_items = vec![handle1, handle2];
        let stream = futures::stream::iter(stream_items.into_iter());

        let targets = handles_to_infos(stream, &env.context, true).await.unwrap();

        assert_eq!(targets.len(), 2, "Expected 2 targets");
    }
}
