// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use argh::{ArgsInfo, FromArgValue, FromArgs};
use ffx_core::ffx_command;
use regex::Regex;
use schemars::JsonSchema;
use serde::Serialize;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "forward",
    description = "Forward connections between host and target device.",
    example = "forward '8080=>8081' # forward TCP host port 8080 to target port 8081\n\
              forward 'tcp:[::1]:8080<=80' # forward TCP target port 80 to host port 8080 over IPv6"
)]
pub struct ForwardCommand {
    /// omits all interactive terminal output
    #[argh(switch, short = 'q')]
    pub quiet: bool,
    /// UI refresh interval in ms
    #[argh(option, short = 'i', from_str_fn(interval_ms), default = "Duration::from_millis(1000)")]
    pub ui_interval: Duration,
    /// exit upon failure to communicate with the target
    ///
    /// If specified, errors cause forward to exit. Otherwise, the default
    /// behavior is to keep attempting target connection.
    #[argh(switch)]
    pub once: bool,
    /// forwarding rules
    ///
    /// Forwarding rules are given as a list of forwarding specifications that
    /// take the form: [host fwd spec][<=|=>][target fwd spec].
    ///
    /// The arrows inform the directionality: => forwards host to target, <=
    /// forwards target to host (i.e. reverse forwarding).
    ///
    /// A forwarding spec takes the form [proto:][address:]port.
    ///
    /// If proto is omitted, tcp is used.
    ///
    /// If address is omitted the protocol equivalent for localhost/loopback is
    /// used. The IPv4 localhost address is used for omitted IP-based addresses.
    /// Note that IPv6 addresses may have square brackets in the address
    /// position, but they're not required.
    ///
    /// port may only be zero on the listener side (host for forward, target
    /// for reverse).
    ///
    /// Supported protocols:
    /// - tcp. Address is an IPv4 or IPv6 address.
    #[argh(positional)]
    pub spec: Vec<ForwardSpec>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, PartialEq)]
pub struct ForwardSpec {
    pub host: ProtoSpec,
    pub target: ProtoSpec,
    pub direction: Direction,
}

impl FromArgValue for ForwardSpec {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        let (host, direction, target) = value
            .split_once("=>")
            .map(|(h, t)| (h, Direction::HostToTarget, t))
            .or_else(|| value.split_once("<=").map(|(h, t)| (h, Direction::TargetToHost, t)))
            .ok_or_else(|| format!("can't determine direction for: '{value}'"))?;
        Ok(Self {
            host: ProtoSpec::from_arg_value(host, direction == Direction::HostToTarget)?,
            target: ProtoSpec::from_arg_value(target, direction == Direction::TargetToHost)?,
            direction,
        })
    }
}

impl std::fmt::Display for ForwardSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { host, direction, target } = self;
        write!(f, "{}{}{}", host, direction, target)
    }
}

#[derive(Debug, Clone, JsonSchema, Serialize, PartialEq)]
pub enum ProtoSpec {
    Tcp(SocketAddr),
}

impl ProtoSpec {
    fn from_arg_value(value: &str, listener: bool) -> Result<Self, String> {
        if value.is_empty() {
            return Err("empty protocol spec".to_string());
        }
        let re = Regex::new(r"^(?:([a-z]+):)?(?:\[?(.+?)\]?:)?([0-9]+)$").unwrap();
        let captures =
            re.captures(value).ok_or_else(|| format!("invalid protocol spec: '{value}'"))?;
        let proto = captures.get(1).map(|m| m.as_str()).unwrap_or("tcp");
        let address = captures.get(2).map(|m| m.as_str());
        let port = captures
            .get(3)
            .map(|m| m.as_str().parse::<u16>())
            .ok_or("missing port")?
            .map_err(|_| format!("invalid port value '{value}'"))?;
        if port == 0 && !listener {
            return Err("port may only be zero on the listener side".to_string());
        }

        match proto {
            "tcp" => Self::parse_tcp(address, port),
            v => Err(format!("unrecognized protocol '{v}'")),
        }
    }

    fn parse_tcp(addr: Option<&str>, port: u16) -> Result<Self, String> {
        let ip_addr = addr
            .map(|a| IpAddr::from_arg_value(a).map_err(|e| format!("{e}: '{a}'")))
            .transpose()?
            .unwrap_or_else(|| Ipv4Addr::LOCALHOST.into());
        Ok(Self::Tcp(SocketAddr::new(ip_addr, port)))
    }
}

impl std::fmt::Display for ProtoSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(addr) => {
                write!(f, "tcp:{}", addr)
            }
        }
    }
}

#[derive(Debug, Copy, Clone, JsonSchema, Serialize, PartialEq)]
pub enum Direction {
    HostToTarget,
    TargetToHost,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::HostToTarget => &"=>",
            Self::TargetToHost => &"<=",
        };
        write!(f, "{}", message)
    }
}

fn interval_ms(v: &str) -> Result<Duration, String> {
    v.parse::<u64>()
        .map_err(|e| e.to_string())
        .and_then(|v| if v > 0 { Ok(v) } else { Err("must be positive".to_string()) })
        .map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    use net_declare::std_socket_addr;
    use test_case::test_case;

    impl ForwardSpec {
        fn h2t_tcp(host: SocketAddr, target: SocketAddr) -> Self {
            Self {
                host: ProtoSpec::Tcp(host),
                target: ProtoSpec::Tcp(target),
                direction: Direction::HostToTarget,
            }
        }

        fn t2h_tcp(host: SocketAddr, target: SocketAddr) -> Self {
            Self {
                host: ProtoSpec::Tcp(host),
                target: ProtoSpec::Tcp(target),
                direction: Direction::TargetToHost,
            }
        }
    }

    #[test_case("8080=>8081" => Ok(
        ForwardSpec::h2t_tcp(
            std_socket_addr!("127.0.0.1:8080"),
            std_socket_addr!("127.0.0.1:8081")
        )
    ); "basic host to target")]
    #[test_case("tcp:127.0.0.1:13<=10.0.0.1:14" => Ok(
        ForwardSpec::t2h_tcp(
            std_socket_addr!("127.0.0.1:13"),
            std_socket_addr!("10.0.0.1:14")
        )
    ); "host to target explicit addr")]
    #[test_case("[fe80::1]:10=>200::1:20" => Ok(
        ForwardSpec::h2t_tcp(
            std_socket_addr!("[fe80::1]:10"),
            std_socket_addr!("[200::1]:20")
        )
    ); "explicit addr v6")]
    #[test_case("tcp:10=>tcp:20" => Ok(
        ForwardSpec::h2t_tcp(
            std_socket_addr!("127.0.0.1:10"),
            std_socket_addr!("127.0.0.1:20")
        )
    ); "omit tcp addr")]
    #[test_case("10=20" => Err(
        "can't determine direction for: '10=20'".to_string()
    ); "missing direction")]
    #[test_case("10=>20=>30" => Err(
        "invalid protocol spec: '20=>30'".to_string()
    ); "double direction")]
    #[test_case("10=>" => Err(
        "empty protocol spec".to_string()
    ); "empty spec")]
    #[test_case("10=>-1" => Err(
        "invalid protocol spec: '-1'".to_string()
    ); "bad port")]
    #[test_case("10=>65536" => Err(
        "invalid port value '65536'".to_string()
    ); "port too large")]
    #[test_case("10=>0" => Err(
        "port may only be zero on the listener side".to_string()
    ); "no zero for target")]
    #[test_case("0<=10" => Err(
        "port may only be zero on the listener side".to_string()
    ); "no zero for host")]
    #[test_case("0=>10" => Ok(
        ForwardSpec::h2t_tcp(
            std_socket_addr!("127.0.0.1:0"),
            std_socket_addr!("127.0.0.1:10")
        )
    ); "ok zero port host")]
    #[test_case("10<=0" => Ok(
        ForwardSpec::t2h_tcp(
            std_socket_addr!("127.0.0.1:10"),
            std_socket_addr!("127.0.0.1:0")
        )
    ); "ok zero port target")]
    #[test_case("xxx:10=>10" => Err(
        "unrecognized protocol 'xxx'".to_string()
    ); "unknown protocol")]
    fn parse_forward_spec(str: &str) -> Result<ForwardSpec, String> {
        ForwardSpec::from_arg_value(str)
    }
}
