// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Packet capture management library.

use std::io::Write as _;
use std::path::PathBuf;
use std::str::FromStr as _;

use futures::StreamExt as _;
use futures::stream::FuturesOrdered;

use flex_client::ProxyHasDomain;
use flex_fuchsia_ebpf as febpf;
use flex_fuchsia_io as fio;
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_debug as fnet_debug;
use flex_fuchsia_net_interfaces as fnet_interfaces;
use zx_status;

#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as fnet_interfaces_ext;

use anyhow::Context as _;
use writer::ToolIO as _;

use crate::CaptureDeps;
use crate::opts::capture::{StartRollingCommand, StopRollingCommand};

/// The default name used for packet captures if none is specified.
pub const DEFAULT_CAPTURE_NAME: &str = "capture";
const DEFAULT_OUTPUT_SUBDIR: &str = "pcap";

/// Specifier for matching an interface by name, ID, or IP.
#[derive(Debug, PartialEq, Eq)]
enum CaptureInterfaceSpec {
    /// Match by interface ID.
    Id(std::num::NonZeroU64),
    /// Match by interface name.
    Name(String),
    /// Match by IP address.
    Ip(std::net::IpAddr),
}

impl CaptureInterfaceSpec {
    /// Parses an interface specifier string (e.g. "name:lo", "id:1", "ip:127.0.0.1").
    fn parse(s: &str) -> Result<Self, anyhow::Error> {
        if let Some(id_str) = s.strip_prefix("id:") {
            let id = id_str
                .parse::<std::num::NonZeroU64>()
                .context("failed to parse interface ID as non-zero integer")?;
            return Ok(Self::Id(id));
        }
        if let Some(name_str) = s.strip_prefix("name:") {
            return Ok(Self::Name(name_str.to_string()));
        }
        if let Some(ip_str) = s.strip_prefix("ip:") {
            let ip = std::net::IpAddr::from_str(ip_str).context("failed to parse IP address")?;
            return Ok(Self::Ip(ip));
        }
        anyhow::bail!(
            "Invalid interface format. Interface must start with 'id:', 'name:', or 'ip:' prefix \
                (e.g. 'name:lo')"
        )
    }
}

/// Starts a rolling packet capture on the target.
pub async fn start_rolling<C>(connector: &C, cmd: StartRollingCommand) -> Result<(), anyhow::Error>
where
    C: crate::NetCliDepsConnector,
{
    let StartRollingCommand { name, interface, pcap_filter, snap_len, capture_size } = cmd;
    let name = name.unwrap_or_else(|| DEFAULT_CAPTURE_NAME.to_string());

    let spec = CaptureInterfaceSpec::parse(&interface).map_err(|e| {
        crate::user_facing_error(format!("Failed to parse interface specifier: {e:?}"))
    })?;

    let get_interfaces = async {
        let state_proxy = crate::connect_with_context::<fnet_interfaces::StateMarker, _>(connector)
            .await
            .context("Failed to connect to interfaces state")?;
        let stream =
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::AllInterest>(
                &state_proxy,
                fnet_interfaces_ext::WatchOptions {
                    included_addresses: fnet_interfaces_ext::IncludedAddresses::All,
                    ..Default::default()
                },
            )
            .context("Failed to get watcher stream")?;
        fnet_interfaces_ext::existing(
            stream,
            std::collections::HashMap::<
                std::num::NonZeroU64,
                fnet_interfaces_ext::PropertiesAndState<(), fnet_interfaces_ext::AllInterest>,
            >::new(),
        )
        .await
        .context("Failed to list interfaces")
    };
    let interface_id = match spec {
        CaptureInterfaceSpec::Id(id) => id.get(),
        CaptureInterfaceSpec::Name(name) => {
            let id = get_interfaces
                .await?
                .values()
                .find_map(|p| (&p.properties.name == &name).then_some(p.properties.id))
                .ok_or_else(|| {
                    crate::user_facing_error(format!("No interface found with name '{name}'"))
                })?;
            id.get()
        }
        CaptureInterfaceSpec::Ip(ip) => {
            let id = get_interfaces
                .await?
                .values()
                .find_map(|p| {
                    let has_ip = p.properties.addresses.iter().any(|a| match a.addr.addr {
                        fnet::IpAddress::Ipv4(v4) => std::net::IpAddr::V4(v4.addr.into()) == ip,
                        fnet::IpAddress::Ipv6(v6) => std::net::IpAddr::V6(v6.addr.into()) == ip,
                    });
                    has_ip.then_some(p.properties.id)
                })
                .ok_or_else(|| {
                    crate::user_facing_error(format!(
                        "No interface found with IP address '{interface}'"
                    ))
                })?;
            id.get()
        }
    };

    // TODO(https://fxbug.dev/535265151): Support capturing on all interfaces
    // via "any".
    // TODO(https://fxbug.dev/535265209): Support capturing on multiple interfaces.
    let interfaces = fnet_debug::InterfaceSpecifier::InterfaceIds(vec![interface_id]);

    let bpf_program = pcap_filter
        .map(|filter_str| {
            let fidl_fuchsia_ebpf::VerifiedProgram {
                code,
                struct_access_instructions,
                maps,
                __source_breaking,
            } = pcap::compile::compile_filter(&filter_str).map_err(|e| {
                crate::user_facing_error(format!(
                    "Failed to compile pcap filter '{filter_str}': {e:?}"
                ))
            })?;
            assert!(maps.unwrap().is_empty());
            assert!(struct_access_instructions.unwrap().is_empty());
            Ok::<_, anyhow::Error>(febpf::VerifiedProgram {
                code,
                struct_access_instructions: Some(Vec::new()),
                maps: Some(Vec::new()),
                ..Default::default()
            })
        })
        .transpose()?;

    let common_params = fnet_debug::CommonPacketCaptureParams {
        interfaces: Some(interfaces),
        bpf_program,
        snap_len,
        ..Default::default()
    };

    let provider =
        crate::connect_with_context::<fnet_debug::PacketCaptureProviderMarker, _>(connector)
            .await
            .context("Failed to connect to packet capture provider")?;
    let rolling_params =
        fnet_debug::RollingPacketCaptureParams { capture_size, ..Default::default() };
    let rolling_client = provider
        .start_rolling(common_params, &rolling_params)
        .await
        .context("FIDL error calling StartRolling")?
        .map_err(|e| crate::user_facing_error(format!("StartRolling error: {e:?}")))?
        .into_proxy();

    rolling_client
        .detach(&name)
        .await
        .context("FIDL error calling Detach")?
        .map_err(|e| crate::user_facing_error(format!("Detach failed with error: {e:?}")))?;

    Ok(())
}

/// Reconnects, stops, and downloads a rolling packet capture from the target.
pub async fn stop_rolling<C, D>(
    connector: &C,
    deps: &D,
    cmd: StopRollingCommand,
) -> Result<Option<std::path::PathBuf>, anyhow::Error>
where
    C: crate::NetCliDepsConnector,
    D: CaptureDeps,
{
    let StopRollingCommand { name, output, skip_download } = cmd;

    if skip_download && output.is_some() {
        return Err(crate::user_facing_error("Cannot specify both --skip-download and --output"));
    }

    let name = name.unwrap_or_else(|| DEFAULT_CAPTURE_NAME.to_string());

    let provider =
        crate::connect_with_context::<fnet_debug::PacketCaptureProviderMarker, _>(connector)
            .await
            .context("Failed to connect to packet capture provider")?;
    let rolling_client = provider
        .reconnect_rolling(&name)
        .await
        .context("FIDL error calling ReconnectRolling")?
        .map_err(|e| {
            crate::user_facing_error(format!("ReconnectRolling to name '{name}' failed: {e:?}"))
        })?
        .into_proxy();

    if skip_download {
        rolling_client.discard().await.context("Failed to discard rolling pcap")?;
        return Ok(None);
    }

    let output_path = match output {
        Some(path) => PathBuf::from(path),
        None => {
            let dir = std::env::temp_dir().join(DEFAULT_OUTPUT_SUBDIR);
            dir.join(format!("{name}.pcapng"))
        }
    };

    let download_res = async {
        let client = rolling_client.domain();
        let (file_proxy, file_server) = client.create_proxy::<fio::FileMarker>();
        rolling_client.stop_and_download(file_server).context("Failed to call StopAndDownload")?;

        let mut file = deps.create_output_writer(&output_path).map_err(|e| {
            crate::user_facing_error(format!(
                "Failed to create output file {}: {e:?}",
                output_path.display()
            ))
        })?;

        let (_mutable_attributes, immutable_attributes) = file_proxy
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await
            .context("Failed get_attributes wire call")?
            .map_err(zx_status::Status::err_from_raw)
            .context("Failed to get attributes of file")?;
        let content_size = immutable_attributes
            .content_size
            .ok_or_else(|| anyhow::anyhow!("Failed to get content size of file"))?;

        let mut queue = FuturesOrdered::new();
        const CONCURRENT_READS: usize = 16;

        for _ in 0..CONCURRENT_READS {
            queue.push_back(file_proxy.read(fio::MAX_BUF));
        }

        let mut bytes_written = 0;
        loop {
            let data = queue
                .next()
                .await
                .expect("read queue should never exhaust")
                .context("FIDL error reading packet capture")?
                .map_err(zx_status::Status::err_from_raw)
                .context("Failed to read packet capture")?;
            if data.is_empty() {
                file.flush().context("Failed to flush packet capture to file")?;
                break;
            }
            file.write_all(&data).context("Failed to write packet capture to file")?;
            bytes_written += data.len();
            queue.push_back(file_proxy.read(fio::MAX_BUF));
        }

        if u64::try_from(bytes_written).expect("bytes written does not fit into u64")
            != content_size
        {
            return Err(anyhow::anyhow!(
                "Download mismatch: Expected {} bytes, but instead read {} bytes",
                content_size,
                bytes_written
            ));
        }
        Ok(())
    }
    .await;

    if let Err(e) = download_res {
        if let Err(e) = std::fs::remove_file(&output_path) {
            log::warn!("Failed to delete partial file {output_path:?}: {e:?}");
        }
        return Err(e);
    }

    rolling_client.discard().await.context("Failed to discard rolling pcap")?;

    Ok(Some(output_path))
}

pub async fn do_capture<C, D>(
    mut out: writer::JsonWriter<serde_json::Value>,
    crate::opts::capture::Capture { capture_cmd }: crate::opts::capture::Capture,
    connector: &C,
    deps: &D,
) -> Result<(), anyhow::Error>
where
    C: crate::NetCliDepsConnector,
    D: CaptureDeps,
{
    match capture_cmd {
        crate::opts::capture::CaptureEnum::StartRolling(cmd) => start_rolling(connector, cmd).await,
        crate::opts::capture::CaptureEnum::StopRolling(cmd) => {
            let path = stop_rolling(connector, deps, cmd).await?;
            if let Some(path) = path {
                if out.is_machine() {
                    out.machine(&serde_json::json!({ "path": path.display().to_string() }))?;
                } else {
                    out.line(format!("{}", path.display()))?;
                }
            } else {
                if out.is_machine() {
                    out.machine(&serde_json::json!({ "path": null }))?;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_parse_interface_spec() {
        assert_eq!(
            CaptureInterfaceSpec::parse("id:12").unwrap(),
            CaptureInterfaceSpec::Id(std::num::NonZeroU64::new(12).unwrap())
        );
        assert_matches!(CaptureInterfaceSpec::parse("id:0"), Err(_));
        assert_matches!(CaptureInterfaceSpec::parse("id:abc"), Err(_));

        assert_eq!(
            CaptureInterfaceSpec::parse("name:loopback").unwrap(),
            CaptureInterfaceSpec::Name("loopback".to_string())
        );

        assert_eq!(
            CaptureInterfaceSpec::parse("ip:127.0.0.1").unwrap(),
            CaptureInterfaceSpec::Ip(net_declare::std_ip!("127.0.0.1"))
        );
        assert_eq!(
            CaptureInterfaceSpec::parse("ip:fe80::1").unwrap(),
            CaptureInterfaceSpec::Ip(net_declare::std_ip!("fe80::1"))
        );

        assert_matches!(CaptureInterfaceSpec::parse("ip:invalid_ip"), Err(_));

        assert_matches!(CaptureInterfaceSpec::parse("loopback"), Err(_));
    }
}
