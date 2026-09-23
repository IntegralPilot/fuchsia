// SPDX-License-Identifier: MIT

use netlink_packet_core::{
    NetlinkDeserializable, NetlinkHeader, NetlinkPayload, NetlinkSerializable,
};
use netlink_packet_utils::DecodeError;
use netlink_packet_utils::traits::{Emitable, ParseableParametrized};

use crate::{SOCK_DESTROY, SOCK_DIAG_BY_FAMILY, SockDiagBuffer, inet, unix};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SockDiagRequest {
    InetRequest(inet::InetRequest),
    UnixRequest(unix::UnixRequest),
    InetSockDestroy(inet::InetRequest),
}

impl SockDiagRequest {
    pub fn message_type(&self) -> u16 {
        match self {
            SockDiagRequest::InetRequest(_) | SockDiagRequest::UnixRequest(_) => {
                SOCK_DIAG_BY_FAMILY
            }
            SockDiagRequest::InetSockDestroy(_) => SOCK_DESTROY,
        }
    }
}

impl Emitable for SockDiagRequest {
    fn buffer_len(&self) -> usize {
        match self {
            SockDiagRequest::InetRequest(msg) => msg.buffer_len(),
            SockDiagRequest::UnixRequest(msg) => msg.buffer_len(),
            SockDiagRequest::InetSockDestroy(msg) => msg.buffer_len(),
        }
    }

    fn emit(&self, buffer: &mut [u8]) {
        match self {
            SockDiagRequest::InetRequest(msg) => msg.emit(buffer),
            SockDiagRequest::UnixRequest(msg) => msg.emit(buffer),
            SockDiagRequest::InetSockDestroy(msg) => msg.emit(buffer),
        }
    }
}

impl NetlinkSerializable for SockDiagRequest {
    fn message_type(&self) -> u16 {
        self.message_type()
    }

    fn buffer_len(&self) -> usize {
        <Self as Emitable>::buffer_len(self)
    }

    fn serialize(&self, buffer: &mut [u8]) {
        self.emit(buffer)
    }
}

/// `SOCK_DIAG` message deserialization does not take any options.
#[derive(Default)]
pub struct EmptyDeserializeOptions;

impl NetlinkDeserializable for SockDiagRequest {
    type DeserializeOptions = EmptyDeserializeOptions;
    type Error = DecodeError;
    fn deserialize(
        header: &NetlinkHeader,
        payload: &[u8],
        _options: EmptyDeserializeOptions,
    ) -> Result<Self, Self::Error> {
        let buffer = SockDiagBuffer::new(&payload)?;
        SockDiagRequest::parse_with_param(&buffer, header.message_type)
    }
}

impl From<SockDiagRequest> for NetlinkPayload<SockDiagRequest> {
    fn from(message: SockDiagRequest) -> Self {
        NetlinkPayload::InnerMessage(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SockDiagResponse {
    InetResponse(Box<inet::InetResponse>),
    UnixResponse(Box<unix::UnixResponse>),
}

impl SockDiagResponse {
    pub fn message_type(&self) -> u16 {
        match self {
            SockDiagResponse::InetResponse(_) | SockDiagResponse::UnixResponse(_) => {
                SOCK_DIAG_BY_FAMILY
            }
        }
    }
}

impl Emitable for SockDiagResponse {
    fn buffer_len(&self) -> usize {
        match self {
            SockDiagResponse::InetResponse(msg) => msg.buffer_len(),
            SockDiagResponse::UnixResponse(msg) => msg.buffer_len(),
        }
    }

    fn emit(&self, buffer: &mut [u8]) {
        match self {
            SockDiagResponse::InetResponse(msg) => msg.emit(buffer),
            SockDiagResponse::UnixResponse(msg) => msg.emit(buffer),
        }
    }
}

impl NetlinkSerializable for SockDiagResponse {
    fn message_type(&self) -> u16 {
        self.message_type()
    }

    fn buffer_len(&self) -> usize {
        <Self as Emitable>::buffer_len(self)
    }

    fn serialize(&self, buffer: &mut [u8]) {
        self.emit(buffer)
    }
}

impl NetlinkDeserializable for SockDiagResponse {
    type DeserializeOptions = EmptyDeserializeOptions;
    type Error = DecodeError;
    fn deserialize(
        header: &NetlinkHeader,
        payload: &[u8],
        _options: EmptyDeserializeOptions,
    ) -> Result<Self, Self::Error> {
        let buffer = SockDiagBuffer::new(&payload)?;
        SockDiagResponse::parse_with_param(&buffer, header.message_type)
    }
}

impl From<SockDiagResponse> for NetlinkPayload<SockDiagResponse> {
    fn from(message: SockDiagResponse) -> Self {
        NetlinkPayload::InnerMessage(message)
    }
}
