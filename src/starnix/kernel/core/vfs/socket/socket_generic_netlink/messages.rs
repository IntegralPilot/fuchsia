// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netlink_packet_core::{NetlinkDeserializable, NetlinkHeader, NetlinkSerializable};
use netlink_packet_generic::GenlMessage;
use netlink_packet_generic::constants::GENL_ID_CTRL;
use netlink_packet_generic::ctrl::GenlCtrl;
use netlink_packet_generic::message::EmptyDeserializeOptions;
use netlink_packet_utils::DecodeError;

#[derive(Clone, Debug)]
pub enum GenericMessage {
    /// The default supported Netlink Control protocol.
    Ctrl(GenlMessage<GenlCtrl>),
    /// Any other Netlink protocol. Netlink family servers should perform their own
    /// parsing on the given payload, likely also using GenlMessage/GenlFamily.
    Other { family: u16, payload: Vec<u8> },
}

impl NetlinkDeserializable for GenericMessage {
    type DeserializeOptions = EmptyDeserializeOptions;
    type Error = DecodeError;

    fn deserialize(
        header: &NetlinkHeader,
        payload: &[u8],
        options: EmptyDeserializeOptions,
    ) -> Result<Self, Self::Error> {
        match header.message_type {
            GENL_ID_CTRL => GenlMessage::<GenlCtrl>::deserialize(header, payload, options)
                .map(GenericMessage::Ctrl),
            family => Ok(GenericMessage::Other { family, payload: payload.to_vec() }),
        }
    }
}

impl NetlinkSerializable for GenericMessage {
    fn message_type(&self) -> u16 {
        match self {
            GenericMessage::Ctrl(c) => NetlinkSerializable::message_type(c),
            GenericMessage::Other { family, .. } => *family,
        }
    }
    fn buffer_len(&self) -> usize {
        match self {
            GenericMessage::Ctrl(c) => NetlinkSerializable::buffer_len(c),
            GenericMessage::Other { payload, .. } => payload.len(),
        }
    }
    fn serialize(&self, buffer: &mut [u8]) {
        match self {
            GenericMessage::Ctrl(c) => NetlinkSerializable::serialize(c, buffer),
            GenericMessage::Other { payload, .. } => buffer.copy_from_slice(&payload[..]),
        }
    }
}
