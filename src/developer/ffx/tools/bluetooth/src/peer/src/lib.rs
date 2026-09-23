// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::ffx_bluetooth_peer_args::{LeSecurityLevel, PeerCommand, PeerSubCommand, Transport};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_bluetooth::PeerId as FidlPeerId;
use fdomain_fuchsia_bluetooth_affordances::PeerControllerProxy;
use fdomain_fuchsia_bluetooth_sys::{
    AccessProxy, BondableMode, InputCapability, OutputCapability, PairingDelegateMarker,
    PairingOptions, PairingProxy, PairingSecurityLevel,
};
use ffx_bluetooth_common::{PeerIdOrAddr, handle_pairing_delegate_requests};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fuchsia_bluetooth::types::{Address, Peer, PeerId};
use prettytable::{Row, Table, format, row};
use std::cmp::Ordering;
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct PeerTool {
    #[command]
    cmd: PeerCommand,
    #[with(toolbox())]
    peer_controller: PeerControllerProxy,
    #[with(toolbox())]
    pairing_proxy: PairingProxy,
    #[with(toolbox())]
    access_proxy: AccessProxy,
}

fho::embedded_plugin!(PeerTool);
#[async_trait(?Send)]
impl FfxMain for PeerTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> Result<()> {
        let peers: Vec<Peer> = self.get_peers().await?;
        match self.cmd.subcommand.clone() {
            // ffx bluetooth peer list
            PeerSubCommand::List(ref mut cmd) => {
                writer.line(get_peer_list(
                    &peers,
                    cmd.filter.get_or_insert_with(|| "".to_string()),
                    cmd.details,
                ))?;
            }
            // ffx bluetooth peer show
            PeerSubCommand::Show(ref cmd) => {
                if let Some(peer_id) = to_identifier(&peers, &cmd.id_or_addr) {
                    writer.line(get_peer(&peers, &peer_id).unwrap())?;
                } else {
                    writer.line("No known peer")?;
                }
            }
            // ffx bluetooth peer connect
            PeerSubCommand::Connect(ref cmd) => {
                let Some(peer_id) = to_identifier(&peers, &cmd.id_or_addr) else {
                    return Err(fho::Error::User(anyhow::anyhow!(
                        "Unable to connect: Unknown peer {}",
                        cmd.id_or_addr
                    )));
                };

                if cmd.with_pairing {
                    writer.line("Allowing pairing")?;
                    futures::try_join!(
                        self.allow_pairing(peer_id, &mut writer),
                        self.connect_peer(peer_id)
                    )?;
                } else {
                    self.connect_peer(peer_id).await?;
                }
                writer.line(format!("Successfully sent connection request to peer {peer_id}"))?;
            }
            // ffx bluetooth peer disconnect
            PeerSubCommand::Disconnect(ref cmd) => {
                let Some(peer_id) = to_identifier(&peers, &cmd.id_or_addr) else {
                    return Err(fho::Error::User(anyhow::anyhow!(
                        "Unable to disconnect: Unknown peer {}",
                        cmd.id_or_addr
                    )));
                };

                self.disconnect_peer(peer_id).await?;
                writer.line(format!("Successfully disconnected from peer {peer_id}"))?;
            }
            // ffx bluetooth peer forget
            PeerSubCommand::Forget(ref cmd) => {
                let Some(peer_id) = to_identifier(&peers, &cmd.id_or_addr) else {
                    return Err(fho::Error::User(anyhow::anyhow!(
                        "Unable to forget: Unknown peer {}",
                        cmd.id_or_addr
                    )));
                };

                self.forget_peer(peer_id).await?;
                writer.line(format!("Successfully forgot peer {peer_id}"))?;
            }
            // ffx bluetooth peer pair
            PeerSubCommand::Pair(ref cmd) => {
                let Some(peer_id) = to_identifier(&peers, &cmd.id_or_addr) else {
                    return Err(fho::Error::User(anyhow::anyhow!(
                        "Unable to forget: Unknown address {}",
                        cmd.id_or_addr
                    )));
                };

                // Check for invalid args
                if cmd.transport == Transport::Classic {
                    match (cmd.le_security_level.is_some(), cmd.non_bondable) {
                        (true, true) => {
                            return Err(fho::Error::User(anyhow::anyhow!(
                                "Unable to pair: Both --le-security-level and --non-bondable are \
not supported with the 'classic' transport"
                            )));
                        }
                        (true, false) => {
                            return Err(fho::Error::User(anyhow::anyhow!(
                                "Unable to pair: The --le-security-level option is not supported \
with the 'classic' transport"
                            )));
                        }
                        (false, true) => {
                            return Err(fho::Error::User(anyhow::anyhow!(
                                "Unable to pair: The --non-bondable option is not supported with \
the 'classic' transport"
                            )));
                        }
                        _ => {}
                    }
                }

                let le_security_level =
                    match cmd.le_security_level.as_ref().unwrap_or(&LeSecurityLevel::Authenticated)
                    {
                        LeSecurityLevel::Encrypted => PairingSecurityLevel::Encrypted,
                        LeSecurityLevel::Authenticated => PairingSecurityLevel::Authenticated,
                    };
                let bondable_mode = match cmd.non_bondable {
                    false => BondableMode::Bondable,
                    true => BondableMode::NonBondable,
                };
                let transport = cmd.transport.clone().into();
                let options = PairingOptions {
                    le_security_level: Some(le_security_level),
                    bondable_mode: Some(bondable_mode),
                    transport: Some(transport),
                    ..Default::default()
                };

                self.pair(peer_id, options).await?;
                writer.line(format!("Successfully paired with peer {peer_id}"))?;
            }
        }
        Ok(())
    }
}

impl PeerTool {
    async fn get_peers(&self) -> Result<Vec<Peer>> {
        let response = self
            .peer_controller
            .get_known_peers()
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.affordances.PeerController error: {err:?}"
                ))
            })?;

        let peers = response.peers.unwrap_or_default();

        Ok(peers
            .into_iter()
            .map(|peer| Peer::try_from(peer).expect("Failed to convert between Peer types"))
            .collect())
    }

    async fn connect_peer(&self, id: PeerId) -> Result<()> {
        let fidl_peer_id: FidlPeerId = id.into();
        Ok(self
            .access_proxy
            .connect(&fidl_peer_id)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.sys.Access/Connect error: {err:?}"
                ))
            })?)
    }

    async fn allow_pairing(&self, peer_id: PeerId, writer: &mut SimpleWriter) -> Result<()> {
        let (pairing_delegate_client, delegate_stream) =
            self.pairing_proxy.domain().create_request_stream::<PairingDelegateMarker>();

        if let Err(err) = self.pairing_proxy.set_pairing_delegate(
            InputCapability::None,
            OutputCapability::None,
            pairing_delegate_client,
        ) {
            return Err(fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")));
        }
        handle_pairing_delegate_requests(delegate_stream, Some(peer_id), writer).await?;
        Ok(())
    }

    async fn disconnect_peer(&self, id: PeerId) -> Result<()> {
        let fidl_peer_id: FidlPeerId = id.into();
        Ok(self
            .access_proxy
            .disconnect(&fidl_peer_id)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.sys.Access/Disconnect error: {err:?}"
                ))
            })?)
    }

    async fn forget_peer(&self, id: PeerId) -> Result<()> {
        let fidl_peer_id: FidlPeerId = id.into();
        Ok(self
            .access_proxy
            .forget(&fidl_peer_id)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.sys.Access/Forget error: {err:?}"
                ))
            })?)
    }

    async fn pair(&self, id: PeerId, options: PairingOptions) -> Result<()> {
        let fidl_peer_id: FidlPeerId = id.into();
        Ok(self
            .access_proxy
            .pair(&fidl_peer_id, &options)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.sys.Access/Pair error: {err:?}"
                ))
            })?)
    }
}

fn get_peer_list(peers: &Vec<Peer>, filter: &String, full_details: bool) -> String {
    if peers.is_empty() {
        return String::from("No known peers");
    }
    let mut matched_peers: Vec<&Peer> = peers.iter().filter(|p| match_peer(filter, p)).collect();
    matched_peers.sort_by(|a, b| cmp_peers(&*a, &*b));
    let match_msg = format!("Showing {}/{} peers\n", matched_peers.len(), peers.len());
    if matched_peers.is_empty() {
        return match_msg;
    }

    if full_details {
        return String::from_iter(
            std::iter::once(match_msg).chain(matched_peers.iter().map(|p| p.to_string())),
        );
    }

    // Create table of results
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_BORDER);
    let _ = table.set_titles(row![
        "PeerId",
        "Address",
        "Technology",
        "Name",
        "Appearance",
        "Connected",
        "Bonded",
    ]);
    for val in matched_peers.into_iter() {
        let _ = table.add_row(peer_to_table_row(&val));
    }
    [match_msg, format!("{}", table)].join("\n")
}

/// Get the string representation of a peer
fn get_peer(peers: &Vec<Peer>, peer_id: &PeerId) -> Option<String> {
    peers.iter().find(|peer| peer.id.eq(peer_id)).map(|peer| peer.to_string())
}

fn match_peer<'a>(pattern: &'a str, peer: &Peer) -> bool {
    let pattern_upper = &pattern.to_uppercase();
    peer.id.to_string().to_uppercase().contains(pattern_upper)
        || peer.address.to_string().to_uppercase().contains(pattern_upper)
        || peer.name.as_ref().is_some_and(|p| p.contains(pattern))
}

/// Order connected peers as greater than unconnected peers and bonded peers greater than unbonded
/// peers.
fn cmp_peers(a: &Peer, b: &Peer) -> Ordering {
    (a.connected, a.bonded).cmp(&(b.connected, b.bonded))
}

/// Returns basic peer information formatted as a prettytable Row
fn peer_to_table_row(peer: &Peer) -> Row {
    let addr_hex = peer.address.as_hex_string();
    let addr_short = match peer.address {
        Address::Public(_) => format!("public {addr_hex}"),
        Address::Random(_) => format!("random {addr_hex}"),
    };
    row![
        peer.id.to_string(),
        addr_short,
        format! {"{:?}", peer.technology},
        peer.name.as_ref().map_or_else(|| "".to_string(), |x| format!("{:?}", x)),
        peer.appearance.as_ref().map_or_else(|| "".to_string(), |x| format!("{:?}", x)),
        peer.connected.to_string(),
        peer.bonded.to_string(),
    ]
}

// Find the identifier for a `Peer` based on a `key` that is either an identifier or an address.
// Returns `None` if the given address does not belong to a known peer.
fn to_identifier(peers: &Vec<Peer>, key: &PeerIdOrAddr) -> Option<PeerId> {
    match key {
        PeerIdOrAddr::PeerId(id) => Some(*id),
        PeerIdOrAddr::BdAddr(addr) => {
            peers.iter().find(|peer| peer.address.as_hex_string() == addr.0).map(|peer| peer.id)
        }
    }
}

/// Tracks all state local to the command line tool.
#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_bluetooth as fbt;
    use fdomain_fuchsia_bluetooth_sys as fsys;
    use ffx_bluetooth_common::{BdAddr, PeerIdOrAddr};
    use fuchsia_bluetooth::types::Address;
    use regex::Regex;

    fn named_peer(id: PeerId, address: Address, name: Option<String>) -> Peer {
        Peer {
            id,
            address,
            technology: fsys::TechnologyType::LowEnergy,
            connected: false,
            bonded: false,
            name,
            appearance: Some(fbt::Appearance::Phone),
            device_class: None,
            rssi: None,
            tx_power: None,
            le_services: vec![],
            bredr_services: vec![],
        }
    }

    fn custom_peer(
        id: PeerId,
        address: Address,
        connected: bool,
        bonded: bool,
        rssi: Option<i8>,
    ) -> Peer {
        Peer {
            id,
            address,
            technology: fsys::TechnologyType::LowEnergy,
            connected,
            bonded,
            name: None,
            appearance: Some(fbt::Appearance::Phone),
            device_class: None,
            rssi,
            tx_power: None,
            le_services: vec![],
            bredr_services: vec![],
        }
    }

    #[fuchsia::test]
    fn test_match_peer() {
        let nameless_peer =
            named_peer(PeerId(0xabcd), Address::Public([0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]), None);
        let named_peer = named_peer(
            PeerId(0xbeef),
            Address::Public([0x11, 0x00, 0x55, 0x7E, 0xDE, 0xAD]),
            Some("Sapphire".to_string()),
        );

        assert!(match_peer("23", &nameless_peer));
        assert!(!match_peer("23", &named_peer));

        assert!(match_peer("cd", &nameless_peer));
        assert!(match_peer("bee", &named_peer));
        assert!(match_peer("BEE", &named_peer));

        assert!(!match_peer("Sapphire", &nameless_peer));
        assert!(match_peer("Sapphire", &named_peer));

        assert!(match_peer("", &nameless_peer));
        assert!(match_peer("", &named_peer));

        assert!(match_peer("DE", &named_peer));
        assert!(match_peer("de", &named_peer));
    }

    #[test]
    fn test_get_peer_list_full_details() {
        let peers = vec![
            named_peer(PeerId(0xabcd), Address::Public([0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]), None),
            named_peer(
                PeerId(0xbeef),
                Address::Public([0x11, 0x00, 0x55, 0x7E, 0xDE, 0xAD]),
                Some("Sapphire".to_string()),
            ),
        ];

        let get_peer_list =
            |filter: &str| -> String { get_peer_list(&peers, &filter.to_string(), true) };

        // Fields for detailed view of peers
        let fields = Regex::new(r"Id(?s).*Address(?s).*Technology(?s).*Name(?s).*Appearance(?s).*Connected(?s).*Bonded(?s).*LE Services(?s).*BR/EDR Serv\.").unwrap();

        // Empty arguments matches everything
        assert!(fields.is_match(&get_peer_list("")));
        assert!(get_peer_list("").contains("2/2 peers"));
        assert!(get_peer_list("").contains("01:23:45"));
        assert!(get_peer_list("").contains("AD:DE:7E"));

        // No matches prints nothing.
        assert!(!fields.is_match(&get_peer_list("nomatch")));
        assert!(get_peer_list("nomatch").contains("0/2 peers"));
        assert!(!get_peer_list("nomatch").contains("01:23:45"));
        assert!(!get_peer_list("nomatch").contains("AD:DE:7E"));

        // We can match either one
        assert!(get_peer_list("01:23").contains("1/2 peers"));
        assert!(get_peer_list("01:23").contains("01:23:45"));
        assert!(get_peer_list("abcd").contains("1/2 peers"));
        assert!(get_peer_list("beef").contains("AD:DE:7E"));
    }

    #[test]
    fn test_get_peer_list_less_details() {
        let peers = vec![
            named_peer(PeerId(0xabcd), Address::Public([0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]), None),
            named_peer(
                PeerId(0xbeef),
                Address::Public([0x11, 0x00, 0x55, 0x7E, 0xDE, 0xAD]),
                Some("Sapphire".to_string()),
            ),
        ];

        let get_peer_list =
            |filter: &str| -> String { get_peer_list(&peers, &filter.to_string(), false) };

        // Fields for table view of peers
        let fields = Regex::new(r"PeerId[ \t]*\|[ \t]*Address[ \t]*\|[ \t]*Technology[ \t]*\|[ \t]*Name[ \t]*\|[ \t]*Appearance[ \t]*\|[ \t]*Connected[ \t]*\|[ \t]*Bonded").unwrap();

        // Empty arguments matches everything
        assert!(fields.is_match(&get_peer_list("")));
        assert!(get_peer_list("").contains("2/2 peers"));
        assert!(get_peer_list("").contains("01:23:45"));
        assert!(get_peer_list("").contains("AD:DE:7E"));

        // No matches prints nothing.
        assert!(!fields.is_match(&get_peer_list("nomatch")));
        assert!(get_peer_list("nomatch").contains("0/2 peers"));
        assert!(!get_peer_list("nomatch").contains("01:23:45"));
        assert!(!get_peer_list("nomatch").contains("AD:DE:7E"));

        // We can match either one
        assert!(get_peer_list("01:23").contains("1/2 peers"));
        assert!(get_peer_list("01:23").contains("01:23:45"));
        assert!(get_peer_list("abcd").contains("1/2 peers"));
        assert!(get_peer_list("beef").contains("AD:DE:7E"));
    }

    #[test]
    fn cmp_peers_correctly_orders_peers() {
        // Sorts connected correctly
        let peer_a =
            custom_peer(PeerId(0xbeef), Address::Public([1, 0, 0, 0, 0, 0]), false, false, None);
        let peer_b =
            custom_peer(PeerId(0xbaaf), Address::Public([2, 0, 0, 0, 0, 0]), true, false, None);
        assert_eq!(cmp_peers(&peer_a, &peer_b), Ordering::Less);

        // Sorts bonded correctly
        let peer_a =
            custom_peer(PeerId(0xbeef), Address::Public([1, 0, 0, 0, 0, 0]), false, false, None);
        let peer_b =
            custom_peer(PeerId(0xbaaf), Address::Public([2, 0, 0, 0, 0, 0]), false, true, None);
        assert_eq!(cmp_peers(&peer_a, &peer_b), Ordering::Less);
    }

    #[test]
    fn test_get_peer() {
        let mut peers = vec![
            named_peer(
                PeerId(0xabcd),
                Address::Public([0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]),
                Some("Sapphire".to_string()),
            ),
            named_peer(PeerId(0xbeef), Address::Public([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), None),
        ];

        // Valid ID
        assert_eq!(get_peer(&peers, &PeerId(0xabcd)), Some(peers[0].to_string()));
        assert_eq!(get_peer(&peers, &PeerId(0xbeef)), Some(peers[1].to_string()));

        // Invalid ID
        assert_eq!(get_peer(&peers, &PeerId(0x1234)), None);

        // Empty peer cache
        peers.clear();
        assert_eq!(get_peer(&peers, &PeerId(0xabcd)), None);
        assert_eq!(get_peer(&peers, &PeerId(0xbeef)), None);
    }

    #[test]
    fn test_to_identifier() {
        let mut peers = vec![
            named_peer(
                PeerId(0xabcd),
                Address::Public([0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]),
                Some("Sapphire".to_string()),
            ),
            named_peer(PeerId(0xbeef), Address::Public([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), None),
        ];

        // Valid ID Input
        assert_eq!(
            to_identifier(&peers, &PeerIdOrAddr::PeerId(PeerId(0xabcd))),
            Some(PeerId(0xabcd))
        );

        // Valid Address Input
        let bd_addr = PeerIdOrAddr::BdAddr(BdAddr(
            Address::Public([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]).as_hex_string(),
        ));
        assert_eq!(to_identifier(&peers, &bd_addr), Some(PeerId(0xbeef)));

        // Invalid Address Input
        let invalid_address = PeerIdOrAddr::BdAddr(BdAddr("00:00:00:00:00:00".to_string()));
        assert_eq!(to_identifier(&peers, &invalid_address), None);

        // Invalid Address Format
        let invalid_format = PeerIdOrAddr::BdAddr(BdAddr("invalid-format".to_string()));
        assert_eq!(to_identifier(&peers, &invalid_format), None);

        // Empty State
        peers.clear();
        assert_eq!(to_identifier(&peers, &bd_addr), None);
    }

    #[test]
    fn test_parse_pairing_security_level() {
        let cases = vec![
            ("Enc", Ok(LeSecurityLevel::Encrypted)),
            ("encrypted", Ok(LeSecurityLevel::Encrypted)),
            ("AUTH", Ok(LeSecurityLevel::Authenticated)),
            ("authenticated", Ok(LeSecurityLevel::Authenticated)),
            ("TEST", Err("security level should be 'encrypted' or 'authenticated'")),
        ];
        for (input_str, expected) in cases {
            assert_eq!(input_str.parse::<LeSecurityLevel>(), expected);
        }
    }

    #[test]
    fn test_parse_pairing_transport() {
        let cases = vec![
            ("LE", Ok(Transport::LowEnergy)),
            ("low-energy", Ok(Transport::LowEnergy)),
            ("c", Ok(Transport::Classic)),
            ("Classic", Ok(Transport::Classic)),
            ("bredr", Ok(Transport::Classic)),
            ("dm", Ok(Transport::DualMode)),
            ("dual_mode", Ok(Transport::DualMode)),
            ("both", Ok(Transport::DualMode)),
            ("dual", Ok(Transport::DualMode)),
            ("TEST", Err("transport should be 'lowenergy', 'classic', or 'dualmode'")),
        ];
        for (input_str, expected) in cases {
            assert_eq!(input_str.parse::<Transport>(), expected);
        }
    }
}
