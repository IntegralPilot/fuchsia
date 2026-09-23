// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::net::IpAddr;

use itertools::Itertools;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket};
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use std::{io, str};

use anyhow::{Context, Result, bail, format_err};
use mdns::protocol as dns;
use netext::{IsLocalAddr, McastInterface, get_mcast_interfaces};
use packet::{InnerPacketBuilder, ParseBuffer};
use socket2::{Domain, Protocol, Socket, Type};

const FUCHSIA_DOMAIN: &str = "_fuchsia._udp.local";
const MDNS_MCAST_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x00fb);
const MDNS_PORT: u16 = 5353;
const MDNS_TIMEOUT: Duration = Duration::from_secs(10);

static MDNS_QUERY: LazyLock<&'static [u8]> = LazyLock::new(|| construct_query_buf(FUCHSIA_DOMAIN));

/// Find Fuchsia devices.
pub(crate) trait Finder {
    /// Find a Fuchsia device, preferring `device_name` if specified.
    fn find_device(&self, device_name: Option<String>) -> Result<Answer>;
}

/// Answer from a Finder.
pub(crate) struct Answer {
    /// Name of the Fuchsia device.
    pub name: String,
    /// IP address of the Fuchsia device.
    pub ip: IpAddr,
    /// Port of the Fuchsia device for SSH.
    pub ssh_port: Option<u16>,
}

pub(crate) struct FfxDevice {
    pub ffx_binary: PathBuf,
}
pub(crate) struct MulticastDns {}

impl Finder for FfxDevice {
    /// Queries FFX for a registered device
    fn find_device(&self, device_name: Option<String>) -> Result<Answer> {
        let program = self
            .ffx_binary
            .to_str()
            .ok_or_else(|| format_err!("FFX binary path is not valid UTF-8"))?;
        let mut args: Vec<&str> = vec!["--machine", "json"];
        if let Some(ref device) = device_name {
            args.push("-t");
            args.push(device);
        }
        args.push("target");
        args.push("show");

        println!("Querying FFX for device parameters: {} {}", program, args.iter().format(" "));

        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("Failed to execute process: {program}"))?;
        if output.status.success() {
            let output_str =
                String::from_utf8(output.stdout).context("Failed to parse FFX stdout as UTF-8")?;
            let output_json: serde_json::Value =
                serde_json::from_str(&output_str).context("Failed to parse FFX output as JSON")?;
            let target = output_json
                .get("target")
                .and_then(|t| t.as_object())
                .context("Missing 'target' object in FFX target show output")?;
            let name = target
                .get("name")
                .and_then(|n| n.as_str())
                .context("Missing 'name' in FFX target show output")?;
            let ssh_address = target
                .get("ssh_address")
                .and_then(|a| a.as_object())
                .with_context(|| {
                    format!(
                        "Target '{name}' has no direct SSH address in FFX target show (e.g. connected via USB)"
                    )
                })?;
            let host = ssh_address
                .get("host")
                .and_then(|h| h.as_str())
                .context("Missing 'ssh_address.host' in FFX target show output")?;
            let port = ssh_address
                .get("port")
                .and_then(|p| p.as_u64())
                .context("Missing 'ssh_address.port' in FFX target show output")?;
            let ip = host
                .replace("[", "") // FFX returns IPv6 addresses wrapped in brackets, which doesn't work with `.parse()`
                .replace("]", "")
                .parse()
                .with_context(|| format!("Attempting to parse string into IP address: {}", host))?;

            let answer = Answer { name: name.to_string(), ip, ssh_port: Some(port as u16) };
            println!("Device {} at {}:{:?}", answer.name, answer.ip, port);
            Ok(answer)
        } else {
            return Err(format_err!(
                "FFX exited with status {}: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
}

impl Finder for MulticastDns {
    /// Find a Fuchsia device using mDNS. If `device_name` is not specified, the
    /// first device will be used.
    fn find_device(&self, device_name: Option<String>) -> Result<Answer> {
        let interfaces =
            get_mcast_interfaces().context("Failed to list multicast-enabled interfaces")?;
        let interface_names =
            interfaces.iter().map(|i| i.name.clone()).collect::<Vec<String>>().join(", ");
        if let Some(ref d) = device_name {
            println!("Performing mDNS discovery for {d} on interfaces: {interface_names}");
        } else {
            println!("Performing mDNS discovery on interfaces: {interface_names}");
        }

        let socket = create_socket(interfaces.iter()).context("Failed to create mDNS socket")?;

        // TODO(http://b/264936590): Remove the race condition where the Fuchsia
        // device can send its answer before this socket starts listening. Add an
        // async runtime and concurrently listen for answers while sending queries.
        send_queries(&socket, interfaces.iter()).context("Failed to send mDNS queries")?;
        let answer = listen_for_answers(socket, device_name)?;

        println!("Device {} found at {}", answer.name, answer.ip,);
        Ok(Answer { name: answer.name, ip: answer.ip, ssh_port: None })
    }
}

fn construct_query_buf(service: &str) -> &'static [u8] {
    let question = dns::QuestionBuilder::new(
        dns::DomainBuilder::from_str(service).unwrap(),
        dns::Type::Ptr,
        dns::Class::In,
        true,
    );

    let mut message = dns::MessageBuilder::new(0, true);
    message.add_question(question);

    let mut buf = vec![0; message.bytes_len()];
    message.serialize(buf.as_mut_slice());
    Box::leak(buf.into_boxed_slice())
}

/// Create a socket for both sending and listening on all multicast-capable
/// interfaces.
fn create_socket<'a>(interfaces: impl Iterator<Item = &'a McastInterface>) -> Result<Socket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    let read_timeout = Duration::from_millis(100);
    socket
        .set_read_timeout(Some(read_timeout))
        .with_context(|| format!("Failed to set SO_RCVTIMEO to {}ms", read_timeout.as_millis()))?;
    socket.set_only_v6(true).context("Failed to set IPV6_V6ONLY")?;
    socket.set_reuse_address(true).context("Failed to set SO_REUSEADDR")?;
    socket.set_reuse_port(true).context("Failed to set SO_REUSEPORT")?;

    for interface in interfaces {
        // Listen on all multicast-enabled interfaces
        match interface.id() {
            Ok(id) => match socket.join_multicast_v6(&MDNS_MCAST_V6, id) {
                Ok(()) => {}
                Err(e) => eprintln!("Failed to join mDNS multicast group on interface {id}: {e}"),
            },
            Err(e) => eprintln!("Failed to listen on interface {}: {}", interface.name, e),
        }
    }

    socket
        .bind(&SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0).into())
        .with_context(|| format!("Failed to bind to unspecified IPv6"))?;

    Ok(socket)
}

fn send_queries<'a>(
    socket: &Socket,
    interfaces: impl Iterator<Item = &'a McastInterface>,
) -> Result<()> {
    let to_addr = SocketAddrV6::new(MDNS_MCAST_V6, MDNS_PORT, 0, 0).into();

    for interface in interfaces {
        let id = interface
            .id()
            .with_context(|| format!("Failed to get interface ID for {}", interface.name))?;
        socket
            .set_multicast_if_v6(id)
            .with_context(|| format!("Failed to set multicast interface for {}", interface.name))?;
        for addr in &interface.addrs {
            if let SocketAddr::V6(addr_v6) = addr {
                if !addr.ip().is_local_addr() || addr.ip().is_loopback() {
                    continue;
                }
                if let Err(e) = socket.send_to(&MDNS_QUERY, &to_addr) {
                    eprintln!(
                        "Failed to send mDNS query out {} via {}: {e}",
                        interface.name,
                        addr_v6.ip()
                    );
                    continue;
                }
            }
        }
    }
    Ok(())
}

struct MdnsAnswer {
    name: String,
    ip: IpAddr,
}

fn listen_for_answers(socket: Socket, device_name: Option<String>) -> Result<MdnsAnswer> {
    let s: UdpSocket = socket.into();
    let mut buf = [0; 1500];

    let end = Instant::now() + MDNS_TIMEOUT;
    while Instant::now() < end {
        match s.recv_from(&mut buf) {
            Ok((packet_bytes, src_sock_addr)) => {
                if !src_sock_addr.ip().is_local_addr() {
                    continue;
                }

                let mut packet_buf = &mut buf[..packet_bytes];
                match packet_buf.parse::<dns::Message<_>>() {
                    Ok(message) => {
                        if !message.answers.iter().any(|a| a.domain == FUCHSIA_DOMAIN) {
                            continue;
                        }
                        for answer in message.additional {
                            if let Some(std::net::IpAddr::V6(addr)) = answer.rdata.ip_addr() {
                                if let SocketAddr::V6(src_v6) = src_sock_addr {
                                    let name = answer
                                        .domain
                                        .to_string()
                                        .trim_end_matches(".local")
                                        .to_string();
                                    let scope_id = scope_id_to_name_checked(src_v6.scope_id())?;

                                    if let Some(ref device) = device_name {
                                        if &name != device {
                                            println!(
                                                "Found irrelevant device {name} at {addr}%{scope_id}"
                                            );
                                            continue;
                                        }
                                    }

                                    return Ok(MdnsAnswer {
                                        name,
                                        ip: IpAddr::V6(addr, Some(scope_id)),
                                    });
                                }
                            }
                        }
                    }
                    Err(err) => eprintln!("Failed to parse mDNS packet: {err:?}"),
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(err.into()),
        }
    }

    bail!("device {device_name:?} not found")
}

fn scope_id_to_name_checked(scope_id: u32) -> Result<String> {
    let mut buf = vec![0; libc::IF_NAMESIZE];
    let res = unsafe { libc::if_indextoname(scope_id, buf.as_mut_ptr() as *mut libc::c_char) };
    if res.is_null() {
        bail!("{scope_id} is not a valid network interface ID")
    } else {
        Ok(String::from_utf8_lossy(&buf.split(|&c| c == 0u8).next().unwrap_or(&[0u8])).to_string())
    }
}
