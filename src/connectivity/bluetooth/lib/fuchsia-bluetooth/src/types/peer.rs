// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth::{Appearance, DeviceClass};
use fidl_fuchsia_bluetooth_sys as fsys;
#[cfg(target_os = "fuchsia")]
use fuchsia_inspect::Node;
#[cfg(target_os = "fuchsia")]
use fuchsia_inspect_contrib::log::WriteInspect;
use std::fmt;

use crate::assigned_numbers::find_service_uuid;
use crate::error::Error;
#[cfg(target_os = "fuchsia")]
use crate::inspect::*;
use crate::types::{Address, PeerId, Uuid};

#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    /// Uniquely identifies this peer on the current system.
    pub id: PeerId,

    /// Bluetooth device address that identifies this peer.
    /// Avoid using this and prefer to use name and/or appearance instead.
    ///
    /// NOTE: Clients should prefer to use the `id` field to keep track of
    /// peers instead of their address.
    pub address: Address,

    /// The Bluetooth technologies that are supported by this peer.
    pub technology: fsys::TechnologyType,

    /// Whether or not a BR/EDR and/or LE connection exists to this peer.
    pub connected: bool,

    /// Whether or not this peer is bonded.
    pub bonded: bool,

    /// The name of the peer, if known.
    pub name: Option<String>,

    /// The LE appearance property. Present if this peer supports LE and the
    /// appearance information was obtained over advertising and/or GATT.
    pub appearance: Option<Appearance>,

    /// The class of device for this device, if known.
    pub device_class: Option<DeviceClass>,

    /// The most recently obtained advertising signal strength for this peer. Present if known.
    pub rssi: Option<i8>,

    /// The most recently obtained transmission power for this peer. Present if known.
    pub tx_power: Option<i8>,

    /// The list of peer service UUIDs known to be available on the LE transport.
    pub le_services: Vec<Uuid>,

    /// The cached list of service UUIDs previously discovered on the BR/EDR transport.
    pub bredr_services: Vec<Uuid>,
}

#[cfg(target_os = "fuchsia")]
impl Peer {
    /// Records the immutable properties of this peer to the Node given.
    fn record_inspect(&self, node: &Node) {
        node.record_string("peer_id", &self.id.to_string());
        node.record_string("technology", &self.technology.debug());
        node.record_string("appearance", &self.appearance.debug());
        node.record_string("address", &self.address.to_string());
        if let Some(rssi) = self.rssi {
            node.record_int("rssi", rssi as i64);
        }
        if let Some(tx_power) = self.tx_power {
            node.record_int("tx_power", tx_power as i64);
        }
        node.record_uint("connected", self.connected.to_property());
        node.record_uint("bonded", self.bonded.to_property());
        if !self.le_services.is_empty() {
            node.record_string("le_services", &self.le_services.to_property());
        }
        if !self.bredr_services.is_empty() {
            node.record_string("bredr_services", &self.bredr_services.to_property());
        }
        if let Some(name) = &self.name {
            node.record_string("name", name);
        }
    }
}

/// Generate a unique ID to use with audio_core, given the `peer_id` and whether it
/// will be an input device. Current format is:
/// [
///   0x42, 0x54, - Prefix reserved for Bluetooth Audio devices
///   0xUU, 0xID, - UUID for the service being provided locally on this device:
///      - 0x11, 0x1E Handsfree (for input devices)
///      - 0x11, 0x1F Handsfree Audio Gateway (for output devices)
///      - 0x11, 0x0A A2DP AudioSource
///      - 0x11, 0x0B A2DP AudioSink (unused for now)
///   0x00, 0x00, 0x00, 0x00 - Reserved for Future Use
///   (PeerId in big endian, 8 bytes)
/// ]
///
/// Panics if the uuid provided is not a 16-bit Bluetooth Service UUID.
pub fn peer_audio_stream_id(peer_id: PeerId, uuid: Uuid) -> [u8; 16] {
    let mut unique_id = [0; 16];
    unique_id[0] = 0x42;
    unique_id[1] = 0x54;
    let short: u16 = uuid.try_into().expect("UUID should be 16-bit");
    unique_id[2..4].copy_from_slice(&(short.to_be_bytes()));
    unique_id[8..].copy_from_slice(&(peer_id.0.to_be_bytes()));
    unique_id
}

/// Given the unique ID used with the audio_core, attempts to extract
/// the Bluetooth PeerId and the UUID for the service being provided to
/// the peer from the local device:
/// * `audio_id` - See [peer_audio_stream_id] for expected format
///
/// Returns an error if the device is not a valid Bluetooth audio device.
pub fn audio_stream_id_to_peer(audio_id: &[u8; 16]) -> Result<(PeerId, Uuid), Error> {
    if audio_id[0] != 0x42 || audio_id[1] != 0x54 {
        return Err(Error::other("Not a Bluetooth audio device"));
    }
    let uuid = Uuid::new16(u16::from_be_bytes(audio_id[2..4].try_into().unwrap()));
    let peer_id = PeerId(u64::from_be_bytes(audio_id[8..16].try_into().unwrap()));
    Ok((peer_id, uuid))
}

#[cfg(target_os = "fuchsia")]
impl ImmutableDataInspect<Peer> for ImmutableDataInspectManager {
    fn new(data: &Peer, manager: Node) -> ImmutableDataInspectManager {
        data.record_inspect(&manager);
        Self { _manager: manager }
    }
}

#[cfg(target_os = "fuchsia")]
impl IsInspectable for Peer {
    type I = ImmutableDataInspectManager;
}

#[cfg(target_os = "fuchsia")]
impl WriteInspect for Peer {
    fn write_inspect<'a>(&self, writer: &Node, key: impl Into<std::borrow::Cow<'a, str>>) {
        writer.record_child(key, |node| {
            self.record_inspect(node);
        });
    }
}

impl fmt::Display for Peer {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(fmt, "Peer:")?;
        writeln!(fmt, "\tId:\t\t{}", self.id)?;
        writeln!(fmt, "\tAddress:\t{}", self.address)?;
        writeln!(fmt, "\tTechnology:\t{:?}", self.technology)?;
        if let Some(name) = &self.name {
            writeln!(fmt, "\tName:\t\t{}", name)?;
        }
        if let Some(appearance) = &self.appearance {
            writeln!(fmt, "\tAppearance:\t{:?}", appearance)?;
        }
        if let Some(rssi) = &self.rssi {
            writeln!(fmt, "\tRSSI:\t\t{}", rssi)?;
        }
        if let Some(tx_power) = &self.tx_power {
            writeln!(fmt, "\tTX Power:\t{}", tx_power)?;
        }
        writeln!(fmt, "\tConnected:\t{}", self.connected)?;
        writeln!(fmt, "\tBonded:\t\t{}", self.bonded)?;
        writeln!(fmt, "\tLE Services:\t{:?}", names_from_services(&self.le_services))?;
        writeln!(fmt, "\tBR/EDR Serv.:\t{:?}", names_from_services(&self.bredr_services))?;
        Ok(())
    }
}

fn names_from_services(services: &Vec<Uuid>) -> Vec<String> {
    services
        .iter()
        .map(|uuid| {
            let uuid = uuid.to_string();
            find_service_uuid(&uuid).map(|an| an.name.into()).unwrap_or(uuid)
        })
        .collect()
}

impl TryFrom<fsys::Peer> for Peer {
    type Error = Error;
    fn try_from(src: fsys::Peer) -> Result<Peer, Self::Error> {
        Ok(Peer {
            id: src.id.ok_or_else(|| Error::missing("sys.Peer.id"))?.into(),
            address: src.address.ok_or_else(|| Error::missing("sys.Peer.address"))?.into(),
            technology: src.technology.ok_or_else(|| Error::missing("sys.Peer.technology"))?,
            connected: src.connected.unwrap_or_default(),
            bonded: src.bonded.unwrap_or_default(),
            name: src.name.clone(),
            appearance: src.appearance,
            device_class: src.device_class,
            rssi: src.rssi,
            tx_power: src.tx_power,
            le_services: src.le_services.unwrap_or_default().iter().map(Uuid::from).collect(),
            bredr_services: src.bredr_services.unwrap_or_default().iter().map(Uuid::from).collect(),
        })
    }
}

impl From<&Peer> for fsys::Peer {
    fn from(src: &Peer) -> fsys::Peer {
        fsys::Peer {
            id: Some(src.id.into()),
            address: Some(src.address.into()),
            technology: Some(src.technology),
            connected: Some(src.connected),
            bonded: Some(src.bonded),
            name: src.name.clone(),
            appearance: src.appearance,
            device_class: src.device_class,
            rssi: src.rssi,
            tx_power: src.tx_power,
            services: None,
            le_services: Some(src.le_services.iter().map(|uuid| uuid.into()).collect()),
            bredr_services: Some(src.bredr_services.iter().map(|uuid| uuid.into()).collect()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl_fuchsia_bluetooth as fbt;
    use proptest::collection::vec;
    use proptest::option;
    use proptest::prelude::*;

    #[test]
    fn try_from_sys_id_not_present() {
        let peer = fsys::Peer::default();
        let peer = Peer::try_from(peer);
        assert!(peer.is_err());
    }

    #[test]
    fn try_from_sys_address_not_present() {
        let peer = fsys::Peer { id: Some(fbt::PeerId { value: 1 }), ..Default::default() };
        let peer = Peer::try_from(peer);
        assert!(peer.is_err());
    }

    #[test]
    fn try_from_sys_technology_not_present() {
        let peer = fsys::Peer {
            id: Some(fbt::PeerId { value: 1 }),
            address: Some(fbt::Address {
                type_: fbt::AddressType::Public,
                bytes: [1, 2, 3, 4, 5, 6],
            }),
            ..Default::default()
        };
        let peer = Peer::try_from(peer);
        assert!(peer.is_err());
    }

    fn any_address() -> impl Strategy<Value = Address> {
        any::<[u8; 6]>().prop_map(Address::Public)
    }

    fn any_technology() -> impl Strategy<Value = fsys::TechnologyType> {
        prop_oneof![
            Just(fsys::TechnologyType::LowEnergy),
            Just(fsys::TechnologyType::Classic),
            Just(fsys::TechnologyType::DualMode),
        ]
    }

    fn any_appearance() -> impl Strategy<Value = Appearance> {
        prop_oneof![Just(Appearance::Unknown), Just(Appearance::Computer),]
    }

    fn any_device_class() -> impl Strategy<Value = DeviceClass> {
        any::<u32>().prop_map(|value| DeviceClass { value })
    }

    fn any_uuids() -> impl Strategy<Value = Vec<Uuid>> {
        vec(any::<[u8; 16]>().prop_map(Uuid::from_bytes), 0..3)
    }

    fn any_peer() -> impl Strategy<Value = Peer> {
        (
            // Trait `Strategy` is only implemented for tuples of up to size 10.
            (
                any::<u64>(), // id
                any_address(),
                any_technology(),
                any::<bool>(),                       // connected
                any::<bool>(),                       // bonded
                option::of("[a-zA-Z][a-zA-Z0-9_]*"), // name
                option::of(any_appearance()),
                option::of(any_device_class()),
                option::of(any::<i8>()), // rssi
                option::of(any::<i8>()), // tx power
            ),
            any_uuids(), // le_services
            any_uuids(), // bredr_services
        )
            .prop_map(
                |(
                    (
                        id,
                        address,
                        technology,
                        connected,
                        bonded,
                        name,
                        appearance,
                        device_class,
                        rssi,
                        tx_power,
                    ),
                    le_services,
                    bredr_services,
                )| {
                    Peer {
                        id: PeerId(id),
                        address,
                        technology,
                        connected,
                        bonded,
                        name,
                        appearance,
                        device_class,
                        rssi,
                        tx_power,
                        le_services,
                        bredr_services,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn peer_sys_roundtrip(peer in any_peer()) {
            use std::convert::TryInto;

            let sys = fsys::Peer::from(&peer);
            assert_eq!(peer, sys.try_into().expect("valid Peer"));
        }
    }

    proptest! {
        #[test]
        fn peer_audio_stream_id_generation(id1 in prop::num::u64::ANY, id2 in prop::num::u64::ANY, uuid1 in prop::num::u16::ANY, uuid2 in prop::num::u16::ANY) {
            let peer1 = PeerId(id1);
            let peer2 = PeerId(id2);
            let service1: Uuid = Uuid::new16(uuid1);
            let service2: Uuid = Uuid::new16(uuid2);

            if id1 == id2 {
                assert_eq!(peer_audio_stream_id(peer1, service1.clone()), peer_audio_stream_id(peer2, service1.clone()));
            } else {
                assert_ne!(peer_audio_stream_id(peer1, service1.clone()), peer_audio_stream_id(peer2, service1.clone()));
            }

            if service1 == service2 {
                assert_eq!(peer_audio_stream_id(peer1, service1), peer_audio_stream_id(peer1, service2));
            } else {
                assert_ne!(peer_audio_stream_id(peer1, service1), peer_audio_stream_id(peer1, service2));
            }
        }
    }

    #[test]
    fn peer_from_audio_stream_id() {
        use fidl_fuchsia_bluetooth_bredr::ServiceClassProfileIdentifier;

        let a2dp_device = [
            0x42, 0x54, 0x11, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08,
        ];
        let (peer_id, uuid) = audio_stream_id_to_peer(&a2dp_device).expect("should be converted");
        assert_eq!(peer_id.0, 0x0102030405060708);
        assert_eq!(uuid, ServiceClassProfileIdentifier::AudioSource.into());

        let non_bluetooth_device = [0x0A; 16];
        let _ = audio_stream_id_to_peer(&non_bluetooth_device)
            .expect_err("should have failed to convert");
    }
}
