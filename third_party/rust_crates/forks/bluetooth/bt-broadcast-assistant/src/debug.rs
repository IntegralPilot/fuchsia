// Copyright 2024 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_bap::types::BroadcastId;
use bt_bass::client::error::Error as BassClientError;
use bt_bass::client::event::Event as BassEvent;
use bt_bass::types::{BisSync, PaSync, SubgroupIndex};

#[cfg(any(test, feature = "debug"))]
use bt_common::core::ltv::LtValue;
#[cfg(any(test, feature = "debug"))]
use bt_common::core::AddressType;
use bt_common::core::AdvertisingSetId;
use bt_common::debug_command::CommandRunner;
use bt_common::debug_command::CommandSet;
use bt_common::gen_commandset;
#[cfg(any(test, feature = "debug"))]
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_common::PeerId;
use bt_gatt::pii::GetPeerAddr;
use std::collections::{HashMap, HashSet};

use futures::stream::FusedStream;
use futures::Future;
use futures::Stream;
use num::Num;
use parking_lot::Mutex;
use std::num::ParseIntError;
use std::sync::Arc;

use crate::assistant::event::*;
use crate::assistant::peer::Peer;
use crate::assistant::Error;
use crate::*;

gen_commandset! {
    AssistantCmd {
        Info = ("info", [], [], "Print information from broadcast assistant"),
        Connect = ("connect", [], ["peer_id"], "Attempt connection to scan delegator"),
        Disconnect = ("disconnect", [], [], "Disconnect from connected scan delegator"),
        SendBroadcastCode = ("set-broadcast-code", [], ["broadcast_id", "broadcast_code"], "Attempt to send decryption key for a particular broadcast source to the scan delegator"),
        AddBroadcastSource = ("add-broadcast-source", [], ["source_peer_id", "advertising_sid", "PaSyncOff|PaSyncPast|PaSyncNoPast", "[bis_sync]"], "Attempt to add a particular broadcast source to the scan delegator"),
        UpdatePaSync = ("update-pa-sync", [], ["broadcast_id", "PaSyncOff|PaSyncPast|PaSyncNoPast", "[bis_sync]"], "Attempt to update the scan delegator's desired pa sync to a particular broadcast source"),
        RemoveBroadcastSource = ("remove-broadcast-source", [], ["broadcast_id"], "Attempt to remove a particular broadcast source to the scan delegator"),
        RemoteScanStarted = ("inform-scan-started", [], [], "Inform the scan delegator that we have started scanning on behalf of it"),
        RemoteScanStopped = ("inform-scan-stopped", [], [], "Inform the scan delegator that we have stopped scanning on behalf of it"),
        // TODO(http://b/433285146): Once PA scanning is implemented, remove bottom 3 commands.
        ForceDiscoverBroadcastSource = ("force-discover-broadcast-source", [], ["source_peer_id", "address", "Public|Random", "advertising_sid"], "Force the broadcast assistant to become aware of the provided broadcast source"),
        ForceDiscoverSourceMetadata = ("force-discover-source-metadata", [], ["source_peer_id", "advertising_sid", "metadata_big1", "[metadata_big_n]..."], "Force the broadcast assistant to become aware of the provided metadata, each BIG's metadata is comma separated"),
        ForceDiscoverEmptySourceMetadata = ("force-discover-empty-source-metadata", [], ["source_peer_id", "advertising_sid", "num_big"], "Force the broadcast assistant to become aware of the provided empty metadata, as many as # BIGs specified"),
    }
}

pub struct AssistantDebug<T: bt_gatt::GattTypes, R: GetPeerAddr> {
    assistant: BroadcastAssistant<T>,
    connected_peer: Mutex<Option<Arc<Peer<T>>>>,
    peer_addr_getter: R,
}

impl<T: bt_gatt::GattTypes + 'static, R: GetPeerAddr> AssistantDebug<T, R> {
    pub fn new(central: T::Central, peer_addr_getter: R) -> Self
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        Self {
            assistant: BroadcastAssistant::<T>::new(central),
            connected_peer: Mutex::new(None),
            peer_addr_getter,
        }
    }

    pub fn start(&mut self) -> Result<EventStream<T>, Error> {
        let event_stream = self.assistant.start()?;
        Ok(event_stream)
    }

    pub fn look_for_scan_delegators(&mut self) -> Result<T::ScanResultStream, Error> {
        self.assistant.scan_for_scan_delegators()
    }

    pub fn take_connected_peer_event_stream(
        &mut self,
    ) -> Result<impl Stream<Item = Result<BassEvent, BassClientError>> + FusedStream, Error> {
        let mut lock = self.connected_peer.lock();
        let Some(peer_arc) = lock.as_mut() else {
            return Err(Error::Generic(format!("not connected to any scan delegator peer")));
        };
        let Some(peer) = Arc::get_mut(peer_arc) else {
            return Err(Error::Generic(format!(
                "cannot get mutable peer reference, it is shared elsewhere"
            )));
        };
        peer.take_event_stream().map_err(|e| Error::Generic(format!("{e:?}")))
    }

    async fn with_peer<F, Fut>(&self, f: F)
    where
        F: FnOnce(Arc<Peer<T>>) -> Fut,
        Fut: Future<Output = Result<(), crate::assistant::peer::Error>>,
    {
        let Some(peer) = self.connected_peer.lock().clone() else {
            eprintln!("not connected to a scan delegator");
            return;
        };
        if let Err(e) = f(peer).await {
            eprintln!("failed to perform operation: {e:?}");
        }
    }
}

/// Attempt to parse a string into an integer.  If the string begins with 0x,
/// treat the rest of the string as a hex value, otherwise treat it as decimal.
pub(crate) fn parse_int<N>(input: &str) -> Result<N, ParseIntError>
where
    N: Num<FromStrRadixErr = ParseIntError>,
{
    if input.starts_with("0x") {
        N::from_str_radix(&input[2..], 16)
    } else {
        N::from_str_radix(input, 10)
    }
}

pub fn parse_peer_id(input: &str) -> Result<PeerId, String> {
    let raw_id = match parse_int(input) {
        Err(_) => return Err(format!("falied to parse int from {input}")),
        Ok(i) => i,
    };

    Ok(PeerId(raw_id))
}

#[cfg(any(test, feature = "debug"))]
/// Returns the bd address in little endian ordering.
pub fn parse_bd_addr(input: &str) -> Result<[u8; 6], String> {
    let mut tokens: Vec<u8> =
        input.split(':').map(|t| u8::from_str_radix(t, 16)).filter_map(Result::ok).collect();
    if tokens.len() != 6 {
        return Err(format!("failed to parse bd address from {input}"));
    }
    tokens.reverse();
    tokens.try_into().map_err(|e| format!("{e:?}"))
}

fn parse_broadcast_id(input: &str) -> Result<BroadcastId, String> {
    let raw_id: u32 = match parse_int(input) {
        Err(_) => return Err(format!("falied to parse int from {input}")),
        Ok(i) => i,
    };
    raw_id.try_into().map_err(|e| format!("{e:?}"))
}

fn parse_advertising_sid(input: &str) -> Result<AdvertisingSetId, String> {
    let raw_sid: u8 = parse_int(input).map_err(|_| format!("failed to parse int from {input}"))?;
    AdvertisingSetId::try_from(raw_sid).map_err(|e| format!("{e:?}"))
}

fn parse_bis_sync(input: &str) -> HashMap<SubgroupIndex, BisSync> {
    let mut map = HashMap::new();
    for t in input.split(',') {
        let parts: Vec<_> = t.split('-').collect();
        if parts.len() != 2 {
            eprintln!(
                "invalid big-bis sync info {t}. should be in <Ith_BIG>-<BIS_INDEX> format, will be ignored"
            );
            continue;
        }
        let Ok(ith_big) = parse_int(parts[0]) else {
            eprintln!("Failed to parse big index from '{}', ignoring.", parts[0]);
            continue;
        };
        match parse_int::<u8>(parts[1]) {
            Ok(bis_index) => {
                let entry = map.entry(ith_big).or_insert(BisSync::no_sync());
                if let Err(e) = entry.synchronize_to_index(bis_index) {
                    eprintln!("Failed to set sync to BIS index: {e:?}");
                }
            }
            Err(_) if parts[1] == "OFF" => {
                map.insert(ith_big, BisSync::no_sync());
            }
            Err(e) => {
                eprintln!("{e:?} - BIS index should be a number from 1-31, ignoring {}", parts[1]);
            }
        }
    }
    map
}

/// Converts a passcode string into a 16-byte broadcast code.
/// The string is UTF-8 encoded and then padded with zeros on the right to a
/// total length of 16 bytes. This result is a little-endian byte array
/// equivalent to a 128-bit value.
fn passcode_to_broadcast_code(passcode: &str) -> Result<[u8; 16], String> {
    if passcode.is_empty() {
        return Err("invalid broadcast code: passcode cannot be empty".to_string());
    }
    let code = passcode.as_bytes();
    if code.len() > 16 {
        return Err(format!(
            "invalid broadcast code: '{}'. should be at max length 16, but was {}",
            passcode,
            code.len()
        ));
    }
    let mut broadcast_code = [0u8; 16];
    broadcast_code[..code.len()].copy_from_slice(code);
    Ok(broadcast_code)
}

impl<T: bt_gatt::GattTypes + 'static, R: GetPeerAddr> CommandRunner for AssistantDebug<T, R>
where
    <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
{
    type Set = AssistantCmd;

    fn run(
        &self,
        cmd: Self::Set,
        args: Vec<String>,
    ) -> impl futures::Future<Output = Result<(), impl std::error::Error>> {
        let help_subcommands: HashSet<&str> = HashSet::from(["help", "-h", "--help"]);
        async move {
            if args.len() >= 1 && help_subcommands.contains(args[0].as_str()) {
                eprintln!("usage: {}", cmd.help_simple());
                return Ok(());
            }
            match cmd {
                AssistantCmd::Info => {
                    let known = self.assistant.known_broadcast_sources();
                    println!("Known Broadcast Sources:");
                    for (id, s) in known {
                        println!("({id:?}): {s:?}");
                    }
                }
                AssistantCmd::Connect => {
                    if self.connected_peer.lock().is_some() {
                        eprintln!(
                            "peer already connected. Call `disconnect` first: {}",
                            AssistantCmd::Disconnect.help_simple()
                        );
                        return Ok(());
                    }
                    if args.len() != 1 {
                        eprintln!("usage: {}", AssistantCmd::Connect.help_simple());
                        return Ok(());
                    }

                    let Ok(peer_id) = parse_peer_id(&args[0]) else {
                        eprintln!("invalid peer id: {}", args[0]);
                        return Ok(());
                    };

                    let peer = self.assistant.connect_to_scan_delegator(peer_id).await;
                    match peer {
                        Ok(peer) => {
                            *self.connected_peer.lock() = Some(Arc::new(peer));
                        }
                        Err(e) => {
                            eprintln!("failed to connect to scan delegator: {e:?}");
                        }
                    };
                }
                AssistantCmd::Disconnect => {
                    if self.connected_peer.lock().take().is_none() {
                        eprintln!("not connected to a scan delegator");
                    }
                }
                AssistantCmd::SendBroadcastCode => {
                    if args.len() != 2 {
                        eprintln!("usage: {}", AssistantCmd::SendBroadcastCode.help_simple());
                        return Ok(());
                    }

                    let Ok(broadcast_id) = parse_broadcast_id(&args[0]) else {
                        eprintln!("invalid broadcast id: {}", args[0]);
                        return Ok(());
                    };

                    let broadcast_code = match passcode_to_broadcast_code(&args[1]) {
                        Ok(code) => code,
                        Err(e) => {
                            eprintln!("{e:?}");
                            return Ok(());
                        }
                    };

                    self.with_peer(|peer| async move {
                        peer.send_broadcast_code(broadcast_id, broadcast_code).await
                    })
                    .await;
                }
                AssistantCmd::AddBroadcastSource => {
                    if args.len() < 3 {
                        eprintln!("usage: {}", AssistantCmd::AddBroadcastSource.help_simple());
                        return Ok(());
                    }

                    let Ok(source_peer_id) = parse_peer_id(&args[0]) else {
                        eprintln!("invalid peer id: {}", args[0]);
                        return Ok(());
                    };

                    let Ok(advertising_sid) = parse_advertising_sid(&args[1]) else {
                        eprintln!("invalid advertising sid: {}", args[1]);
                        return Ok(());
                    };

                    let pa_sync: PaSync = match args[2].parse() {
                        Ok(sync) => sync,
                        Err(e) => {
                            eprintln!("invalid pa_sync: {e:?}");
                            return Ok(());
                        }
                    };

                    let bis_sync =
                        if args.len() == 4 { parse_bis_sync(&args[3]) } else { HashMap::new() };

                    self.with_peer(|peer| async move {
                        peer.add_broadcast_source(
                            source_peer_id,
                            advertising_sid,
                            &self.peer_addr_getter,
                            pa_sync,
                            bis_sync,
                        )
                        .await
                    })
                    .await;
                }
                AssistantCmd::UpdatePaSync => {
                    if args.len() < 2 {
                        eprintln!("usage: {}", AssistantCmd::UpdatePaSync.help_simple());
                        return Ok(());
                    }

                    let Ok(broadcast_id) = parse_broadcast_id(&args[0]) else {
                        eprintln!("invalid broadcast id: {}", args[0]);
                        return Ok(());
                    };

                    let pa_sync: PaSync = match args[1].parse() {
                        Ok(sync) => sync,
                        Err(e) => {
                            eprintln!("invalid pa_sync: {e:?}");
                            return Ok(());
                        }
                    };

                    let bis_sync =
                        if args.len() == 3 { parse_bis_sync(&args[2]) } else { HashMap::new() };

                    self.with_peer(|peer| async move {
                        peer.update_broadcast_source_sync(broadcast_id, pa_sync, bis_sync).await
                    })
                    .await;
                }
                AssistantCmd::RemoveBroadcastSource => {
                    if args.len() != 1 {
                        eprintln!("usage: {}", AssistantCmd::RemoveBroadcastSource.help_simple());
                        return Ok(());
                    }

                    let Ok(broadcast_id) = parse_broadcast_id(&args[0]) else {
                        eprintln!("invalid broadcast id: {}", args[0]);
                        return Ok(());
                    };

                    self.with_peer(|peer| async move {
                        peer.remove_broadcast_source(broadcast_id).await
                    })
                    .await;
                }
                AssistantCmd::RemoteScanStarted => {
                    self.with_peer(|peer: Arc<Peer<T>>| async move {
                        peer.inform_remote_scan_started().await
                    })
                    .await;
                }
                AssistantCmd::RemoteScanStopped => {
                    self.with_peer(|peer| async move { peer.inform_remote_scan_stopped().await })
                        .await;
                }
                #[cfg(feature = "debug")]
                AssistantCmd::ForceDiscoverBroadcastSource => {
                    if args.len() != 4 {
                        eprintln!(
                            "usage: {}",
                            AssistantCmd::ForceDiscoverBroadcastSource.help_simple()
                        );
                        return Ok(());
                    }

                    let Ok(source_peer_id) = parse_peer_id(&args[0]) else {
                        eprintln!("invalid peer id: {}", args[0]);
                        return Ok(());
                    };

                    let Ok(address) = parse_bd_addr(&args[1]) else {
                        eprintln!("invalid address: {}", args[1]);
                        return Ok(());
                    };

                    let address_type: AddressType = match args[2].parse() {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("invalid address type: {e:?}");
                            return Ok(());
                        }
                    };

                    let Ok(advertising_sid) = parse_advertising_sid(&args[3]) else {
                        eprintln!("invalid advertising sid: {}", args[3]);
                        return Ok(());
                    };

                    match self.assistant.force_discover_broadcast_source(
                        source_peer_id,
                        address,
                        address_type,
                        advertising_sid,
                    ) {
                        Ok(source) => {
                            println!("broadcast source after additional info: {source:?}")
                        }
                        Err(e) => {
                            eprintln!("failed to enter in broadcast source information: {e:?}")
                        }
                    }
                }
                #[cfg(feature = "debug")]
                AssistantCmd::ForceDiscoverSourceMetadata => {
                    if args.len() < 3 {
                        eprintln!(
                            "usage: {}",
                            AssistantCmd::ForceDiscoverSourceMetadata.help_simple()
                        );
                        return Ok(());
                    }

                    let Ok(source_peer_id) = parse_peer_id(&args[0]) else {
                        eprintln!("invalid peer id: {}", args[0]);
                        return Ok(());
                    };

                    let Ok(advertising_sid) = parse_advertising_sid(&args[1]) else {
                        eprintln!("invalid advertising sid: {}", args[1]);
                        return Ok(());
                    };

                    let mut all_big_metadata = Vec::new();
                    for i in 2..args.len() {
                        let raw_metadata: Vec<u8> = args[i]
                            .split(',')
                            .map(|t| parse_int(t))
                            .filter_map(Result::ok)
                            .collect();

                        if raw_metadata.len() > 0 {
                            let (decoded_metadata, consumed_len) =
                                Metadata::decode_all(raw_metadata.as_slice());
                            if consumed_len != raw_metadata.len() {
                                eprintln!("Metadata length is not valid");
                                return Ok(());
                            }
                            all_big_metadata.push(
                                decoded_metadata.into_iter().filter_map(Result::ok).collect(),
                            );
                        } else {
                            all_big_metadata.push(vec![]);
                        }
                    }

                    match self.assistant.force_discover_broadcast_source_metadata(
                        source_peer_id,
                        advertising_sid,
                        all_big_metadata,
                    ) {
                        Ok(source) => println!("broadcast source with metadata: {source:?}"),
                        Err(e) => eprintln!("failed to enter in broadcast source metadata: {e:?}"),
                    }
                }
                #[cfg(feature = "debug")]
                AssistantCmd::ForceDiscoverEmptySourceMetadata => {
                    if args.len() != 3 {
                        eprintln!(
                            "usage: {}",
                            AssistantCmd::ForceDiscoverEmptySourceMetadata.help_simple()
                        );
                        return Ok(());
                    }

                    let Ok(source_peer_id) = parse_peer_id(&args[0]) else {
                        eprintln!("invalid peer id: {}", args[0]);
                        return Ok(());
                    };

                    let Ok(advertising_sid) = parse_advertising_sid(&args[1]) else {
                        eprintln!("invalid advertising sid: {}", args[1]);
                        return Ok(());
                    };

                    let Ok(num_big) = parse_int::<usize>(&args[2]) else {
                        eprintln!("invalid # of bigs: {}", args[2]);
                        return Ok(());
                    };

                    let mut all_big_metadata = Vec::new();
                    for _i in 0..num_big {
                        all_big_metadata.push(vec![]);
                    }

                    match self.assistant.force_discover_broadcast_source_metadata(
                        source_peer_id,
                        advertising_sid,
                        all_big_metadata,
                    ) {
                        Ok(source) => println!("broadcast source with metadata: {source:?}"),
                        Err(e) => {
                            eprintln!("failed to enter in empty broadcast source metadata: {e:?}")
                        }
                    }
                }
                #[cfg(not(feature = "debug"))]
                c => eprintln!("unknown command: {c:?}"),
            }
            Ok::<(), Error>(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_peer_id() {
        // In hex string.
        assert_eq!(parse_peer_id("0x678abc").expect("should be ok"), PeerId(0x678abc));
        // Decimal equivalent.
        assert_eq!(parse_peer_id("6785724").expect("should be ok"), PeerId(0x678abc));

        // Invalid peer id.
        let _ = parse_peer_id("0123zzz").expect_err("should fail");
    }

    #[test]
    fn test_parse_bd_addr() {
        assert_eq!(
            parse_bd_addr("3c:80:f1:ed:32:2c").expect("should be ok"),
            [0x2c, 0x32, 0xed, 0xf1, 0x80, 0x3c]
        );
        // Address with 5 parts is invalid.
        let _ = parse_bd_addr("3c:80:f1:ed:32").expect_err("should fail");
        // Address with 6 parts but one of them empty is invalid.
        let _ = parse_bd_addr("3c:80:f1::32:2c").expect_err("should fail");
        let _ = parse_bd_addr(":80:f1:ed:32:2c").expect_err("should fail");
        let _ = parse_bd_addr("3c:80:f1:ed:32:").expect_err("should fail");
        // Address not delimited by : is invalid.
        let _ = parse_bd_addr("3c.80.f1.ed.32.2c").expect_err("should fail");
    }

    #[test]
    fn test_parse_broadcast_id() {
        assert_eq!(parse_broadcast_id("0xABCD").expect("should work"), 0xABCD.try_into().unwrap());
        assert_eq!(parse_broadcast_id("123456").expect("should work"), 123456.try_into().unwrap());

        // Invalid string cannot be parsed.
        let _ = parse_broadcast_id("0xABYZ").expect_err("should fail");

        // Broadcast ID is actually a 3 byte long number.
        let _ = parse_broadcast_id("16777216").expect_err("should fail");
    }

    #[test]
    fn test_parse_bis_sync() {
        // Basic case with multiple BIGs and BIS indices.
        let bis_sync = parse_bis_sync("0-1,0-2,1-1");
        assert_eq!(bis_sync.len(), 2);
        assert_eq!(bis_sync.get(&0), Some(&BisSync::sync(vec![1, 2]).unwrap()));
        assert_eq!(bis_sync.get(&1), Some(&BisSync::sync(vec![1]).unwrap()));

        // Case with "OFF" to disable sync for a BIG.
        let bis_sync = parse_bis_sync("0-1,1-OFF,0-2");
        assert_eq!(bis_sync.len(), 2);
        assert_eq!(bis_sync.get(&0), Some(&BisSync::sync(vec![1, 2]).unwrap()));
        assert_eq!(bis_sync.get(&1), Some(&BisSync::no_sync()));

        // Case where sync is set and then turned off for the same BIG.
        let bis_sync = parse_bis_sync("0-5,0-OFF");
        assert_eq!(bis_sync.len(), 1);
        assert_eq!(bis_sync.get(&0), Some(&BisSync::no_sync()));

        // Will ignore invalid values.
        let bis_sync = parse_bis_sync("0-1,0-2,1:1,1-1-1,");
        assert_eq!(bis_sync.len(), 1);
        assert_eq!(bis_sync.get(&0), Some(&BisSync::sync(vec![1, 2]).unwrap()));

        let bis_sync = parse_bis_sync("hellothisistoallynotvalid");
        assert_eq!(bis_sync.len(), 0);
    }

    #[test]
    fn test_passcode_to_broadcast_code() {
        // UTF-8 string that is less than 16 bytes.
        // Source of truth test case from Bluetooth Spec.
        let code = "Børne House";
        let expected = [
            0x42, 0xc3, 0xb8, 0x72, 0x6e, 0x65, 0x20, 0x48, 0x6f, 0x75, 0x73, 0x65, 0x00, 0x00,
            0x00, 0x00,
        ];
        let actual = passcode_to_broadcast_code(code).expect("should succeed");
        assert_eq!(actual, expected);
        assert_eq!(u128::from_le_bytes(actual), 0x00000000_6573756F_4820656E_72B8C342);

        // Valid ASCII passcode, exactly 16 bytes.
        let code = "1234567890123456";
        let expected = [
            0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34,
            0x35, 0x36,
        ];
        assert_eq!(passcode_to_broadcast_code(code).unwrap(), expected);

        // Invalid passcode, over 16 bytes.
        let code = "12345678901234567";
        assert!(passcode_to_broadcast_code(code).is_err());

        // Empty passcode should be an error.
        let code = "";
        assert!(passcode_to_broadcast_code(code).is_err());
    }
}
