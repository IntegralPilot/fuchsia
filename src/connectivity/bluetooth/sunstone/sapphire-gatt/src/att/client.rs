// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::att::AttributeHandle;
use crate::att::bearer::{
    AttReceiver, BearerRecvError, BearerSendError, BearerTx, DEFAULT_STARTING_MTU,
    MAX_SUPPORTED_MTU,
};
use crate::att::l2cap::{L2CapChannelRx, L2CapChannelTx};
use crate::att::pdu::{
    ATT_EXCHANGE_MTU_REQ_SIZE, ATT_EXECUTE_WRITE_REQ_SIZE, ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE,
    ATT_FIND_INFORMATION_REQ_SIZE, ATT_FIND_INFORMATION_RSP_HEADER_SIZE, ATT_HANDLE_VALUE_CFM_SIZE,
    ATT_HANDLES_INFORMATION_SIZE, ATT_HEADER_SIZE, ATT_PREPARE_WRITE_HEADER_SIZE,
    ATT_READ_BLOB_REQ_SIZE, ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE,
    ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE, ATT_READ_BY_TYPE_REQ_HEADER_SIZE,
    ATT_READ_REQ_SIZE, ATT_WRITE_CMD_HEADER_SIZE, ATT_WRITE_REQ_HEADER_SIZE, ErrorCode,
    ExecuteWriteFlags, Opcode, Packet, PduList, UuidFormat,
};
use crate::att::router::{BearerRouter, BearerRxHandle, RouteFilter};
use sapphire_emboss::att::{
    AttErrorRsp, AttExchangeMtuReqWriter, AttExchangeMtuRsp, AttExecuteWriteReqWriter,
    AttFindByTypeValueReqHeaderWriter, AttFindInformationReqWriter, AttFindInformationRspHeader,
    AttHeader, AttHeaderWriter, AttPrepareWriteHeaderWriter, AttReadBlobReqWriter,
    AttReadByGroupTypeReqHeaderWriter, AttReadByTypeReqHeaderWriter, AttReadReqWriter,
    AttWriteCmdWriter,
};
use sapphire_emboss::{CheckComplete, CheckOk, InfallibleRead};

use core::cmp::{max, min};
use core::mem::{MaybeUninit, size_of};
use sapphire_common::Uuid;
use sapphire_sync::mutex::raw::RawMutex;
use thiserror::Error;
use zerocopy::byteorder::little_endian::U16;
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes};

pub use crate::att::pdu::{DiscoveredInformation, ReadByGroupTypeResults, ReadByTypeResults};

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientError {
    #[error("Underlying logical link was closed")]
    LinkClosed,
    #[error("Unexpected response opcode: {0:?}")]
    UnexpectedOpcode(Opcode),
    #[error("Error response from server: {0:?}")]
    ErrorResponse(ErrorCode),
    #[error("Invalid incoming data from server")]
    InvalidIncomingData,
}

/// ATT Client protocol wrapper.
pub struct Client<Tx, R> {
    bearer_tx: BearerTx<Tx>,
    bearer_rx: R,
    preferred_mtu: u16,
}

impl<Tx, R> Client<Tx, R>
where
    Tx: L2CapChannelTx,
    R: AttReceiver,
{
    /// Creates a new ATT client instance.
    pub fn new(bearer_tx: BearerTx<Tx>, bearer_rx: R, preferred_mtu: u16) -> Self {
        Self { bearer_tx, bearer_rx, preferred_mtu }
    }

    /// Helper to perform a sequential ATT request-response transaction.
    ///
    /// This method:
    /// 1. Transmits `req_packet` over the ATT bearer.
    /// 2. Awaits the incoming response packet from the peer into `rx_buf`.
    /// 3. Validates the response:
    ///    - If it matches `expected_rsp_opcode`, it is returned as `Ok`.
    ///    - If it is an `ErrorRsp` and corresponds to the transmitted request opcode,
    ///      the specific `ErrorCode` is parsed and returned as `ClientError::ErrorResponse`.
    ///    - If the response opcode is unrecognized or unexpected, returns `ClientError::UnexpectedOpcode`.
    ///    - If the incoming packet or header is malformed, returns `ClientError::InvalidIncomingData`.
    async fn transaction<'a>(
        &mut self,
        req_opcode: Opcode,
        req_packet: &Packet,
        rx_buf: &'a mut [MaybeUninit<u8>],
        expected_rsp_opcode: Opcode,
    ) -> Result<&'a Packet, ClientError> {
        let negotiated_mtu = usize::from(self.bearer_tx.mtu());
        assert!(
            rx_buf.len() >= negotiated_mtu,
            "Programming error: rx_buf ({} bytes) is smaller than negotiated MTU ({} bytes)",
            rx_buf.len(),
            negotiated_mtu
        );

        self.send_packet(req_packet).await?;

        let rx_packet = self.bearer_rx.next_packet(rx_buf).await.map_err(|e| match e {
            BearerRecvError::LinkClosed => ClientError::LinkClosed,
            BearerRecvError::BufferTooSmall => {
                panic!("Programming error: rx_buf is smaller than negotiated MTU")
            }
            BearerRecvError::HeaderTooShort => ClientError::InvalidIncomingData,
            BearerRecvError::PacketTooLarge { .. } => ClientError::InvalidIncomingData,
            BearerRecvError::InvalidOpcode(_) => ClientError::InvalidIncomingData,
        })?;

        let header = AttHeader::new(rx_packet.as_bytes());
        match header.attribute_opcode().try_read() {
            Ok(opcode) if opcode == expected_rsp_opcode => Ok(rx_packet),
            Ok(Opcode::ATT_ERROR_RSP) => {
                let err = AttErrorRsp::new(rx_packet.as_bytes())
                    .check_ok()
                    .map_err(|_| ClientError::InvalidIncomingData)?;
                let err_req_op = err.request_opcode_in_error_uint().read();
                let err_code = match err.error_code().read() {
                    Ok(c) => c,
                    Err(_) => return Err(ClientError::InvalidIncomingData),
                };
                if err_req_op == u8::from(req_opcode) {
                    Err(ClientError::ErrorResponse(err_code))
                } else {
                    Err(ClientError::UnexpectedOpcode(Opcode::ATT_ERROR_RSP))
                }
            }
            Ok(other) => Err(ClientError::UnexpectedOpcode(other)),
            Err(_) => Err(ClientError::InvalidIncomingData),
        }
    }

    /// Helper to send a single packet. Panics if the packet is too large for the negotiated MTU.
    async fn send_packet(&mut self, packet: &Packet) -> Result<(), ClientError> {
        self.bearer_tx.send(packet).await.map_err(|e| match e {
            BearerSendError::LinkClosed => ClientError::LinkClosed,
            BearerSendError::PacketTooLarge => {
                panic!("Programming error: outgoing packet size exceeds the negotiated MTU.");
            }
        })
    }

    /// Performs the Exchange MTU handshake procedure sequentially.
    ///
    /// Updates the negotiated MTU on the underlying bearer.
    ///
    /// see (Vol 3, Part G, Section 5.2.1) and (Vol 3, Part F, Section 3.4.2)
    pub async fn exchange_mtu(&mut self) -> Result<(), ClientError> {
        let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
        let _ = AttExchangeMtuReqWriter::new(&mut req_buf[..])
            .check_complete()
            .expect("statically sized Exchange MTU request buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_EXCHANGE_MTU_REQ)
            .write_client_rx_mtu(self.preferred_mtu);
        let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).expect("valid packet");
        let mut rx_buf = [MaybeUninit::uninit(); DEFAULT_STARTING_MTU as usize];

        match self
            .transaction(
                Opcode::ATT_EXCHANGE_MTU_REQ,
                tx_packet,
                &mut rx_buf,
                Opcode::ATT_EXCHANGE_MTU_RSP,
            )
            .await
        {
            Ok(rx_packet) => {
                let rsp = AttExchangeMtuRsp::new(rx_packet.as_bytes())
                    .check_ok()
                    .map_err(|_| ClientError::InvalidIncomingData)?;
                let server_mtu = rsp.server_rx_mtu().read();
                let negotiated_mtu = max(DEFAULT_STARTING_MTU, min(self.preferred_mtu, server_mtu));

                self.bearer_tx.set_mtu(negotiated_mtu);
                self.bearer_rx.set_mtu(negotiated_mtu);
                Ok(())
            }
            Err(ClientError::ErrorResponse(ErrorCode::REQUEST_NOT_SUPPORTED)) => {
                // Safely recover by locking in the default fallback MTU
                self.bearer_tx.set_mtu(DEFAULT_STARTING_MTU);
                self.bearer_rx.set_mtu(DEFAULT_STARTING_MTU);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Performs the ATT Find Information procedure to discover attribute handles
    /// and their associated UUIDs within a given handle range.
    ///
    /// The returned zero-copy data slice borrow-maps directly over the provided `rx_buf`.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.3).
    pub async fn find_information<'a>(
        &mut self,
        starting_handle: AttributeHandle,
        ending_handle: AttributeHandle,
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<DiscoveredInformation<'a>, ClientError> {
        let mut tx_buf = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
        let _ = AttFindInformationReqWriter::new(&mut tx_buf[..])
            .check_complete()
            .expect("valid buffer size")
            .write_attribute_opcode(Opcode::ATT_FIND_INFORMATION_REQ)
            .write_starting_handle(starting_handle.value())
            .write_ending_handle(ending_handle.value());
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..]).expect("valid packet");

        let rx_packet = self
            .transaction(
                Opcode::ATT_FIND_INFORMATION_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_FIND_INFORMATION_RSP,
            )
            .await?;

        let header_view = AttFindInformationRspHeader::new(rx_packet.as_bytes())
            .check_ok()
            .map_err(|_| ClientError::InvalidIncomingData)?;
        let format = match header_view.format().read() {
            Ok(f) => f,
            Err(_) => return Err(ClientError::InvalidIncomingData),
        };
        let data_list = &rx_packet.data[1..];

        match format {
            UuidFormat::BIT16 => {
                let list = PduList::new(data_list).ok_or(ClientError::InvalidIncomingData)?;
                Ok(DiscoveredInformation::Uuid16(list))
            }
            UuidFormat::BIT128 => {
                let list = PduList::new(data_list).ok_or(ClientError::InvalidIncomingData)?;
                Ok(DiscoveredInformation::Uuid128(list))
            }
        }
    }

    /// Initiates a Find By Type Value procedure to obtain the handle range (start and group
    /// end handle) of attributes with a specific 16-bit UUID type and value. Commonly used
    /// to discover the range of a specific service type (e.g. Heart Rate service).
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.3.3).
    pub async fn find_by_type_value<'a>(
        &mut self,
        starting_handle: AttributeHandle,
        ending_handle: AttributeHandle,
        attribute_type: u16, // 16-bit UUID only
        attribute_value: &[u8],
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<PduList<'a, ATT_HANDLES_INFORMATION_SIZE>, ClientError> {
        let total_size = ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE + attribute_value.len();
        assert!(
            total_size <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttFindByTypeValueReqHeaderWriter::new(
            &mut tx_buf[..ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE],
        )
        .check_complete()
        .expect("valid buffer size")
        .write_attribute_opcode(Opcode::ATT_FIND_BY_TYPE_VALUE_REQ)
        .write_starting_handle(starting_handle.value())
        .write_ending_handle(ending_handle.value())
        .write_attribute_type(attribute_type);
        tx_buf[ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE..total_size].copy_from_slice(attribute_value);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..total_size]).expect("valid packet");

        let rx_packet = self
            .transaction(
                Opcode::ATT_FIND_BY_TYPE_VALUE_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_FIND_BY_TYPE_VALUE_RSP,
            )
            .await?;

        PduList::new(&rx_packet.data).ok_or(ClientError::InvalidIncomingData)
    }

    /// Sends a Read Request and awaits a Read Response.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.1 & 3.4.4.2)
    pub async fn read<'a>(
        &mut self,
        handle: AttributeHandle,
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a [u8], ClientError> {
        let mut buf = [0u8; ATT_READ_REQ_SIZE];
        let _ = AttReadReqWriter::new(&mut buf[..])
            .check_complete()
            .expect("statically sized Read request buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_READ_REQ)
            .write_attribute_handle(handle.value());
        let tx_packet = Packet::try_ref_from_bytes(&buf[..]).expect("valid packet");

        // Perform the transaction and await the matching Read Response.
        let rsp_packet =
            self.transaction(Opcode::ATT_READ_REQ, tx_packet, rx_buf, Opcode::ATT_READ_RSP).await?;

        // Return the variable-length attribute value.
        Ok(&rsp_packet.data)
    }

    /// Sends a Read Blob Request and awaits a Read Blob Response.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.3 & 3.4.4.4)
    pub async fn read_blob<'a>(
        &mut self,
        handle: AttributeHandle,
        offset: u16,
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a [u8], ClientError> {
        let mut buf = [0u8; ATT_READ_BLOB_REQ_SIZE];
        let _ = AttReadBlobReqWriter::new(&mut buf[..])
            .check_complete()
            .expect("statically sized Read Blob request buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_READ_BLOB_REQ)
            .write_attribute_handle(handle.value())
            .write_value_offset(offset);
        let tx_packet = Packet::try_ref_from_bytes(&buf[..]).expect("valid packet");

        // Perform the transaction and await the matching Read Blob Response.
        let rsp_packet = self
            .transaction(Opcode::ATT_READ_BLOB_REQ, tx_packet, rx_buf, Opcode::ATT_READ_BLOB_RSP)
            .await?;

        // Return the variable-length value chunk.
        Ok(&rsp_packet.data)
    }

    pub fn mtu(&self) -> u16 {
        self.bearer_tx.mtu()
    }

    fn effective_mtu(&self) -> usize {
        usize::try_from(self.mtu()).unwrap_or(usize::MAX)
    }

    /// Initiates a Read By Type procedure to obtain the values of attributes with a specific
    /// attribute type (UUID).
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.7 & 3.4.4.8).
    pub async fn read_by_type<'a>(
        &mut self,
        starting_handle: AttributeHandle,
        ending_handle: AttributeHandle,
        attribute_type: &Uuid,
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<ReadByTypeResults<'a>, ClientError> {
        let type_bytes = attribute_type.as_bytes();
        let total_size = ATT_READ_BY_TYPE_REQ_HEADER_SIZE + type_bytes.len();
        assert!(
            total_size <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttReadByTypeReqHeaderWriter::new(&mut tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Read By Type header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_READ_BY_TYPE_REQ)
            .write_starting_handle(starting_handle.value())
            .write_ending_handle(ending_handle.value());
        tx_buf[ATT_READ_BY_TYPE_REQ_HEADER_SIZE..total_size].copy_from_slice(type_bytes);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..total_size]).expect("valid packet");

        // Perform the transaction and await the response.
        let rx_packet = self
            .transaction(
                Opcode::ATT_READ_BY_TYPE_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_READ_BY_TYPE_RSP,
            )
            .await?;

        if rx_packet.data.is_empty() {
            return Err(ClientError::InvalidIncomingData);
        }
        let length = usize::from(rx_packet.data[0]);
        let data = &rx_packet.data[1..];
        ReadByTypeResults::new(length, data).ok_or(ClientError::InvalidIncomingData)
    }

    /// Initiates a Read By Group Type procedure to obtain the values of attributes with a specific
    /// attribute group type (UUID).
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.9 & 3.4.4.10).
    pub async fn read_by_group_type<'a>(
        &mut self,
        starting_handle: AttributeHandle,
        ending_handle: AttributeHandle,
        attribute_group_type: &Uuid,
        rx_buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<ReadByGroupTypeResults<'a>, ClientError> {
        let type_bytes = attribute_group_type.as_bytes();
        let total_size = ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + type_bytes.len();
        assert!(
            total_size <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttReadByGroupTypeReqHeaderWriter::new(
            &mut tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE],
        )
        .check_complete()
        .expect("statically sized Read By Group Type header buffer must be complete")
        .write_attribute_opcode(Opcode::ATT_READ_BY_GROUP_TYPE_REQ)
        .write_starting_handle(starting_handle.value())
        .write_ending_handle(ending_handle.value());
        tx_buf[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE..total_size].copy_from_slice(type_bytes);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..total_size]).expect("valid packet");

        // Perform the transaction and await the response.
        let rx_packet = self
            .transaction(
                Opcode::ATT_READ_BY_GROUP_TYPE_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_READ_BY_GROUP_TYPE_RSP,
            )
            .await?;

        if rx_packet.data.is_empty() {
            return Err(ClientError::InvalidIncomingData);
        }
        let length = usize::from(rx_packet.data[0]);
        let data = &rx_packet.data[1..];
        ReadByGroupTypeResults::new(length, data).ok_or(ClientError::InvalidIncomingData)
    }

    /// Initiates a Write Request procedure to write the value of an attribute.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.5.1 & 3.4.5.2).
    pub async fn write(
        &mut self,
        attribute_handle: AttributeHandle,
        attribute_value: &[u8],
        rx_buf: &mut [MaybeUninit<u8>],
    ) -> Result<(), ClientError> {
        let req_len = ATT_WRITE_REQ_HEADER_SIZE + attribute_value.len();
        assert!(
            req_len <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        assert!(
            req_len <= MAX_SUPPORTED_MTU,
            "Programming error: request packet size exceeds buffer."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttWriteCmdWriter::new(&mut tx_buf[..ATT_WRITE_REQ_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Write Request header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_WRITE_REQ)
            .write_attribute_handle(attribute_handle.value());
        tx_buf[ATT_WRITE_REQ_HEADER_SIZE..req_len].copy_from_slice(attribute_value);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..req_len]).expect("valid packet");

        let _rx_packet = self
            .transaction(Opcode::ATT_WRITE_REQ, tx_packet, rx_buf, Opcode::ATT_WRITE_RSP)
            .await?;
        Ok(())
    }

    /// Initiates a Write Command procedure to write the value of an attribute without response.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.5.3).
    pub async fn write_command(
        &mut self,
        attribute_handle: AttributeHandle,
        attribute_value: &[u8],
    ) -> Result<(), ClientError> {
        let req_len = ATT_WRITE_CMD_HEADER_SIZE + attribute_value.len();
        assert!(
            req_len <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        assert!(
            req_len <= MAX_SUPPORTED_MTU,
            "Programming error: request packet size exceeds buffer."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttWriteCmdWriter::new(&mut tx_buf[..ATT_WRITE_CMD_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Write Command header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_WRITE_CMD)
            .write_attribute_handle(attribute_handle.value());
        tx_buf[ATT_WRITE_CMD_HEADER_SIZE..req_len].copy_from_slice(attribute_value);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..req_len]).expect("valid packet");

        self.send_packet(tx_packet).await?;
        Ok(())
    }

    /// Initiates a Prepare Write procedure to write a part of an attribute value.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.6.1).
    pub async fn prepare_write(
        &mut self,
        attribute_handle: AttributeHandle,
        value_offset: u16,
        part_attribute_value: &[u8],
        rx_buf: &mut [MaybeUninit<u8>],
    ) -> Result<(), ClientError> {
        let req_len = ATT_PREPARE_WRITE_HEADER_SIZE + part_attribute_value.len();
        assert!(
            req_len <= self.effective_mtu(),
            "Programming error: request packet size exceeds negotiated MTU."
        );
        assert!(
            req_len <= MAX_SUPPORTED_MTU,
            "Programming error: request packet size exceeds buffer."
        );
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttPrepareWriteHeaderWriter::new(&mut tx_buf[..ATT_PREPARE_WRITE_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Prepare Write header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_PREPARE_WRITE_REQ)
            .write_attribute_handle(attribute_handle.value())
            .write_value_offset(value_offset);
        tx_buf[ATT_PREPARE_WRITE_HEADER_SIZE..req_len].copy_from_slice(part_attribute_value);
        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..req_len]).expect("valid packet");

        let rx_packet = self
            .transaction(
                Opcode::ATT_PREPARE_WRITE_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_PREPARE_WRITE_RSP,
            )
            .await?;

        if rx_packet.data != tx_packet.data {
            return Err(ClientError::InvalidIncomingData);
        }

        Ok(())
    }

    /// Initiates an Execute Write procedure to commit or cancel prepared writes.
    ///
    /// Note: Although the Execute Write Response has no payload, `rx_buf` must be
    /// provided to satisfy the transaction MTU safety guardrail and allow the caller
    /// to reuse their existing buffer without stack duplication.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.6.3).
    pub async fn execute_write(
        &mut self,
        flags: ExecuteWriteFlags,
        rx_buf: &mut [MaybeUninit<u8>],
    ) -> Result<(), ClientError> {
        let mut buf = [0u8; ATT_EXECUTE_WRITE_REQ_SIZE];
        let _ = AttExecuteWriteReqWriter::new(&mut buf[..])
            .check_complete()
            .expect("statically sized Execute Write request buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_EXECUTE_WRITE_REQ)
            .write_flags(flags);
        let tx_packet = Packet::try_ref_from_bytes(&buf[..]).expect("valid packet");

        let rx_packet = self
            .transaction(
                Opcode::ATT_EXECUTE_WRITE_REQ,
                tx_packet,
                rx_buf,
                Opcode::ATT_EXECUTE_WRITE_RSP,
            )
            .await?;

        if !rx_packet.data.is_empty() {
            return Err(ClientError::InvalidIncomingData);
        }

        Ok(())
    }

    /// Returns an asynchronous stream handle for consuming unsolicited server events
    /// pushed from the remote server endpoint.
    pub fn server_event_stream<'a, Rx: L2CapChannelRx, Mtx: RawMutex>(
        &self,
        rx_handle: BearerRxHandle<'a, Rx, Mtx>,
    ) -> ServerEventStream<Tx, BearerRxHandle<'a, Rx, Mtx>> {
        ServerEventStream { bearer_tx: self.bearer_tx.clone(), rx_handle }
    }
}

/// An individual unsolicited server event pushed from the remote server endpoint.
#[derive(TryFromBytes, KnownLayout, Immutable, IntoBytes, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ServerEvent {
    attribute_handle: U16,
    value: [u8],
}

impl ServerEvent {
    /// Returns the handle of the attribute whose value was updated.
    pub fn handle(&self) -> AttributeHandle {
        AttributeHandle::new(self.attribute_handle.get()).expect("valid non-zero handle")
    }

    /// Returns the updated attribute value bytes.
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

/// An asynchronous stream consumption API for consuming unsolicited server events
/// pushed from the remote server endpoint.
#[derive(Debug)]
pub struct ServerEventStream<Tx, R> {
    bearer_tx: BearerTx<Tx>,
    rx_handle: R,
}

impl<'a, Tx: L2CapChannelTx, Rx: L2CapChannelRx, Mtx: RawMutex>
    ServerEventStream<Tx, BearerRxHandle<'a, Rx, Mtx>>
{
    pub fn new(router: &'a BearerRouter<Rx, Mtx>, bearer_tx: BearerTx<Tx>) -> Option<Self> {
        let rx_handle = router.route_to(RouteFilter::ServerEvents)?;
        Some(Self { bearer_tx, rx_handle })
    }
}

impl<Tx: L2CapChannelTx, R: AttReceiver> ServerEventStream<Tx, R> {
    /// Awaits the next server event frame from the network.
    /// Acknowledgment PDUs are automatically transmitted down the underlying bearer
    /// when required by the ATT protocol before returning.
    pub async fn next<'a>(
        &mut self,
        buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a ServerEvent, ClientError> {
        let packet = self.rx_handle.next_packet(buf).await.map_err(|e| match e {
            BearerRecvError::LinkClosed => ClientError::LinkClosed,
            BearerRecvError::BufferTooSmall => {
                panic!(
                    "Programming error: provided buffer size is smaller than the negotiated MTU."
                );
            }
            BearerRecvError::InvalidOpcode(_) => {
                panic!("Programming error: rx_handle returned an invalid opcode.");
            }
            BearerRecvError::HeaderTooShort | BearerRecvError::PacketTooLarge { .. } => {
                ClientError::InvalidIncomingData
            }
        })?;

        let event = ServerEvent::try_ref_from_bytes(&packet.data)
            .map_err(|_| ClientError::InvalidIncomingData)?;

        let header = AttHeader::new(packet.as_bytes());
        match header.attribute_opcode().try_read() {
            Ok(Opcode::ATT_HANDLE_VALUE_NTF) => Ok(event),
            Ok(Opcode::ATT_HANDLE_VALUE_IND) => {
                let mut cfm_buf = [0u8; ATT_HANDLE_VALUE_CFM_SIZE];
                let _ = AttHeaderWriter::new(&mut cfm_buf)
                    .check_complete()
                    .expect("valid buffer size")
                    .write_attribute_opcode(Opcode::ATT_HANDLE_VALUE_CFM);
                let tx_packet = Packet::try_ref_from_bytes(&cfm_buf).expect("valid packet");
                match self.bearer_tx.send(tx_packet).await {
                    Ok(()) => {}
                    Err(BearerSendError::LinkClosed) => return Err(ClientError::LinkClosed),
                    Err(BearerSendError::PacketTooLarge) => {
                        unreachable!("HandleValueCfm (1 byte) never exceeds minimum ATT MTU")
                    }
                }
                Ok(event)
            }
            _ => Err(ClientError::InvalidIncomingData),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::att::bearer::BearerRx;
    use crate::att::l2cap::mock::setup_mock_channel;
    use crate::att::pdu::{
        ATT_ERROR_RSP_SIZE, ATT_EXCHANGE_MTU_RSP_SIZE, ATT_EXECUTE_WRITE_RSP_SIZE,
        ATT_HANDLE_VALUE_IND_HEADER_SIZE,
    };
    use sapphire_async::executor::BoundedExecutor;
    use sapphire_async::testing::TestExecutor;
    use sapphire_emboss::CheckComplete;
    use sapphire_emboss::att::{
        AttErrorRspWriter, AttExchangeMtuReq, AttExchangeMtuRspWriter, AttExecuteWriteReq,
        AttFindByTypeValueReqHeader, AttFindInformationReq, AttFindInformationRspHeaderWriter,
        AttHandleValueIndHeaderWriter, AttHandlesInformation, AttHandlesInformationWriter,
        AttInformationData16, AttInformationData16Writer, AttPrepareWriteHeader, AttReadBlobReq,
        AttReadByGroupTypeReqHeader, AttReadByGroupTypeRspEntryHeaderWriter,
        AttReadByTypeReqHeader, AttReadReq, AttWriteCmd,
    };

    const CLIENT_PREFERRED_MTU: u16 = 512;
    const SERVER_MTU: u16 = 256;

    fn h(val: u16) -> AttributeHandle {
        AttributeHandle::try_from(val).unwrap()
    }

    fn make_error_rsp(
        opcode: Opcode,
        handle: u16,
        error_code: ErrorCode,
    ) -> [u8; ATT_ERROR_RSP_SIZE] {
        let mut buf = [0u8; ATT_ERROR_RSP_SIZE];
        let _ = AttErrorRspWriter::new(&mut buf[..])
            .check_complete()
            .unwrap()
            .write_attribute_opcode(Opcode::ATT_ERROR_RSP)
            .write_request_opcode_in_error_uint(u8::from(opcode))
            .write_attribute_handle(handle)
            .write_error_code(error_code);
        buf
    }

    #[test]
    fn test_client_exchange_mtu_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Spawn mock server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let req = AttExchangeMtuReq::new(packet.as_bytes());
                assert_eq!(
                    req.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_REQ
                );
                assert_eq!(req.client_rx_mtu().try_read().unwrap(), CLIENT_PREFERRED_MTU);

                // Reply with ExchangeMtuRsp containing 256-byte MTU
                let mut rsp_buf = [0u8; ATT_EXCHANGE_MTU_RSP_SIZE];
                let _ = AttExchangeMtuRspWriter::new(&mut rsp_buf[..])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_EXCHANGE_MTU_RSP)
                    .write_server_rx_mtu(SERVER_MTU);

                let mut server_tx_bearer = BearerTx::new(test_tx);
                let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            // Spawn client driver task
            let client_handle = executor.spawn(async move {
                client.exchange_mtu().await.expect("handshake completes");
                assert_eq!(client.mtu(), SERVER_MTU);
            });

            executor.run_until_stalled();

            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_exchange_mtu_unsupported_fallback() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Mock server responds with ErrorRsp for ExchangeMtuReq with error RequestNotSupported
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_EXCHANGE_MTU_REQ.into());

                let mut server_tx_bearer = BearerTx::new(test_tx);
                let err_buf = make_error_rsp(
                    Opcode::ATT_EXCHANGE_MTU_REQ,
                    0,
                    ErrorCode::REQUEST_NOT_SUPPORTED,
                );
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                // Client must fall back to default 23-byte MTU
                client.exchange_mtu().await.expect("handshake completes");
                assert_eq!(client.mtu(), DEFAULT_STARTING_MTU);
            });

            executor.run_until_stalled();

            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_exchange_mtu_hard_error() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Mock server responds with ErrorRsp indicating InsufficientAuthentication
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_EXCHANGE_MTU_REQ.into());

                let mut server_tx_bearer = BearerTx::new(test_tx);
                let err_buf = make_error_rsp(
                    Opcode::ATT_EXCHANGE_MTU_REQ,
                    0,
                    ErrorCode::INSUFFICIENT_AUTHENTICATION,
                );
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                // Client must abort and propagate ClientError::ErrorResponse
                let res = client.exchange_mtu().await;
                assert_eq!(
                    res,
                    Err(ClientError::ErrorResponse(ErrorCode::INSUFFICIENT_AUTHENTICATION))
                );
            });

            executor.run_until_stalled();

            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_find_information_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Spawn mock server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_INFORMATION_REQ.into());

                let req = AttFindInformationReq::new(packet.as_bytes());
                assert_eq!(req.starting_handle().try_read().unwrap(), 1);
                assert_eq!(req.ending_handle().try_read().unwrap(), 10);

                // Respond with FindInformationRsp (0x05)
                // format: 0x01 (16-bit)
                // entries:
                // Handle 1: UUID 0x2A00
                // Handle 2: UUID 0x2A24
                let mut tx_buf = [0u8; 64];
                let _ = AttFindInformationRspHeaderWriter::new(
                    &mut tx_buf[..ATT_FIND_INFORMATION_RSP_HEADER_SIZE],
                )
                .check_complete()
                .unwrap()
                .write_attribute_opcode(Opcode::ATT_FIND_INFORMATION_RSP)
                .write_format(UuidFormat::BIT16);
                let _ = AttInformationData16Writer::new(&mut tx_buf[2..6])
                    .check_complete()
                    .unwrap()
                    .write_attribute_handle(1)
                    .write_uuid(0x2A00);
                let _ = AttInformationData16Writer::new(&mut tx_buf[6..10])
                    .check_complete()
                    .unwrap()
                    .write_attribute_handle(2)
                    .write_uuid(0x2A24);

                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..10]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let info = client
                    .find_information(h(1), h(10), &mut rx_buf)
                    .await
                    .expect("find_information succeeds");

                match info {
                    DiscoveredInformation::Uuid16(entries) => {
                        assert_eq!(entries.len(), 2);
                        let e1 = AttInformationData16::new(entries.get(0).unwrap());
                        assert_eq!(e1.attribute_handle().try_read().unwrap(), 1);
                        assert_eq!(e1.uuid().try_read().unwrap(), 0x2A00);
                        let e2 = AttInformationData16::new(entries.get(1).unwrap());
                        assert_eq!(e2.attribute_handle().try_read().unwrap(), 2);
                        assert_eq!(e2.uuid().try_read().unwrap(), 0x2A24);
                    }
                    _ => panic!("Expected Uuid16 discovered info"),
                }
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_find_information_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Spawn mock server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_INFORMATION_REQ.into());

                // Respond with ErrorRsp (InvalidHandle)
                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf =
                    make_error_rsp(Opcode::ATT_FIND_INFORMATION_REQ, 10, ErrorCode::INVALID_HANDLE);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            // Client task
            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let res = client.find_information(h(10), h(20), &mut rx_buf).await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_find_by_type_value_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_BY_TYPE_VALUE_REQ.into());

                let req_header = AttFindByTypeValueReqHeader::new(packet.as_bytes());
                assert_eq!(req_header.starting_handle().try_read().unwrap(), 1);
                assert_eq!(req_header.ending_handle().try_read().unwrap(), 10);
                assert_eq!(req_header.attribute_type().try_read().unwrap(), 0x2800);
                assert_eq!(
                    &packet.as_bytes()[ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE..],
                    &[0x0D, 0x18][..]
                );

                let mut tx_buf = [0u8; 64];
                tx_buf[0] = Opcode::ATT_FIND_BY_TYPE_VALUE_RSP as u8;
                let _ = AttHandlesInformationWriter::new(&mut tx_buf[1..5])
                    .check_complete()
                    .unwrap()
                    .write_attribute_handle(1)
                    .write_group_end_handle(5);
                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..5]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let results = client
                    .find_by_type_value(h(1), h(10), 0x2800, &[0x0D, 0x18], &mut rx_buf)
                    .await
                    .expect("find_by_type_value succeeds");

                assert_eq!(results.len(), 1);
                let e = AttHandlesInformation::new(results.get(0).unwrap());
                assert_eq!(e.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(e.group_end_handle().try_read().unwrap(), 5);
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_find_by_type_value_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_BY_TYPE_VALUE_REQ.into());

                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf = make_error_rsp(
                    Opcode::ATT_FIND_BY_TYPE_VALUE_REQ,
                    1,
                    ErrorCode::ATTRIBUTE_NOT_FOUND,
                );
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let res = client
                    .find_by_type_value(h(1), h(10), 0x2800, &[0x0D, 0x18], &mut rx_buf)
                    .await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_REQ.into());

                let req = AttReadReq::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 1);

                let val = b"Sunstone";
                let mut tx_buf = [0u8; 64];
                let _ = AttHeaderWriter::new(&mut tx_buf[..1])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_READ_RSP);
                tx_buf[1..1 + val.len()].copy_from_slice(val);
                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..1 + val.len()]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read(h(1), &mut rx_buf).await.unwrap();
                let expected: &[u8] = b"Sunstone";
                assert_eq!(val, expected);
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_REQ.into());

                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf = make_error_rsp(Opcode::ATT_READ_REQ, 1, ErrorCode::INVALID_HANDLE);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let res = client.read(h(1), &mut rx_buf).await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::INVALID_HANDLE)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_blob_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BLOB_REQ.into());

                let req = AttReadBlobReq::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(req.value_offset().try_read().unwrap(), 2);

                let val = b"nstone"; // Part of "Sunstone" starting at offset 2
                let mut tx_buf = [0u8; 64];
                let _ = AttHeaderWriter::new(&mut tx_buf[..1])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_READ_BLOB_RSP);
                tx_buf[1..1 + val.len()].copy_from_slice(val);
                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..1 + val.len()]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let val = client.read_blob(h(1), 2, &mut rx_buf).await.unwrap();
                let expected: &[u8] = b"nstone";
                assert_eq!(val, expected);
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_blob_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BLOB_REQ.into());

                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf =
                    make_error_rsp(Opcode::ATT_READ_BLOB_REQ, 1, ErrorCode::INVALID_OFFSET);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let res = client.read_blob(h(1), 100, &mut rx_buf).await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::INVALID_OFFSET)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_by_type_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let uuid = Uuid::from_u16(0x2800); // Primary Service 16-bit UUID
            const VALUE_SIZE: usize = 8; // b"PrimaryS".len()
            const ENTRY_SIZE: u8 = (size_of::<AttributeHandle>() + VALUE_SIZE) as u8;

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_TYPE_REQ.into());

                let req = AttReadByTypeReqHeader::new(packet.as_bytes());
                assert_eq!(req.starting_handle().try_read().unwrap(), 1);
                assert_eq!(req.ending_handle().try_read().unwrap(), 10);
                assert_eq!(&packet.as_bytes()[ATT_READ_BY_TYPE_REQ_HEADER_SIZE..], uuid.as_bytes());

                let mut tx_buf = [0u8; 64];
                let _ = AttHeaderWriter::new(&mut tx_buf[..1])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_READ_BY_TYPE_RSP);
                tx_buf[1] = ENTRY_SIZE;
                // Push entry 1: handle = 2, value = b"PrimaryS"
                tx_buf[2..4].copy_from_slice(&2u16.to_le_bytes());
                tx_buf[4..12].copy_from_slice(b"PrimaryS");
                // Push entry 2: handle = 6, value = b"PrimaryS"
                tx_buf[12..14].copy_from_slice(&6u16.to_le_bytes());
                tx_buf[14..22].copy_from_slice(b"PrimaryS");

                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..22]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let results = client.read_by_type(h(1), h(10), &uuid, &mut rx_buf).await.unwrap();

                let mut count = 0;
                for (handle, value) in results.iter() {
                    if count == 0 {
                        assert_eq!(handle.value(), 2);
                        assert_eq!(value, b"PrimaryS");
                    } else if count == 1 {
                        assert_eq!(handle.value(), 6);
                        assert_eq!(value, b"PrimaryS");
                    }
                    count += 1;
                }
                assert_eq!(count, 2);
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_by_type_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_TYPE_REQ.into());

                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf =
                    make_error_rsp(Opcode::ATT_READ_BY_TYPE_REQ, 1, ErrorCode::ATTRIBUTE_NOT_FOUND);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let uuid = Uuid::from_u16(0x2800);
                let res = client.read_by_type(h(1), h(10), &uuid, &mut rx_buf).await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::ATTRIBUTE_NOT_FOUND)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_by_group_type_success() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let uuid = Uuid::from_u16(0x2800); // Primary Service 16-bit UUID
            const VALUE_SIZE: usize = 2; // service UUID (0x1800/0x1801 is 2 bytes)
            let group_header_size = ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE;
            let entry_size = (group_header_size + VALUE_SIZE) as u8;

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_GROUP_TYPE_REQ.into());

                let req = AttReadByGroupTypeReqHeader::new(packet.as_bytes());
                assert_eq!(req.starting_handle().try_read().unwrap(), 1);
                assert_eq!(req.ending_handle().try_read().unwrap(), 10);
                assert_eq!(
                    &packet.as_bytes()[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE..],
                    uuid.as_bytes()
                );

                let mut tx_buf = [0u8; 64];
                let _ = AttHeaderWriter::new(&mut tx_buf[..1])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_READ_BY_GROUP_TYPE_RSP);
                tx_buf[1] = entry_size;
                // Push entry 1: handle = 2, group end = 5, value = 0x1801 (\x01\x18)
                let _ = AttReadByGroupTypeRspEntryHeaderWriter::new(&mut tx_buf[2..6])
                    .check_complete()
                    .unwrap()
                    .write_attribute_handle(2)
                    .write_end_group_handle(5);
                tx_buf[6..8].copy_from_slice(b"\x01\x18");
                // Push entry 2: handle = 6, group end = 10, value = 0x1800 (\x00\x18)
                let _ = AttReadByGroupTypeRspEntryHeaderWriter::new(&mut tx_buf[8..12])
                    .check_complete()
                    .unwrap()
                    .write_attribute_handle(6)
                    .write_end_group_handle(10);
                tx_buf[12..14].copy_from_slice(b"\x00\x18");

                let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..14]).unwrap();
                let mut server_tx_bearer = BearerTx::new(server_tx);
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let results =
                    client.read_by_group_type(h(1), h(10), &uuid, &mut rx_buf).await.unwrap();

                let mut iter = results.iter();
                let (e1_hdr, e1_val) = iter.next().unwrap();
                assert_eq!(e1_hdr.attribute_handle().try_read().unwrap(), 2);
                assert_eq!(e1_hdr.end_group_handle().try_read().unwrap(), 5);
                assert_eq!(e1_val, b"\x01\x18");

                let (e2_hdr, e2_val) = iter.next().unwrap();
                assert_eq!(e2_hdr.attribute_handle().try_read().unwrap(), 6);
                assert_eq!(e2_hdr.end_group_handle().try_read().unwrap(), 10);
                assert_eq!(e2_val, b"\x00\x18");

                assert!(iter.next().is_none());
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_read_by_group_type_error() {
        let (app_channel, server_tx, server_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(server_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_GROUP_TYPE_REQ.into());

                // Respond with ErrorRsp (UnsupportedGroupType)
                let mut server_tx_bearer = BearerTx::new(server_tx);
                let err_buf = make_error_rsp(
                    Opcode::ATT_READ_BY_GROUP_TYPE_REQ,
                    1,
                    ErrorCode::UNSUPPORTED_GROUP_TYPE,
                );
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let uuid = Uuid::from_u16(0x2800);
                let res = client.read_by_group_type(h(1), h(10), &uuid, &mut rx_buf).await;
                assert_eq!(res, Err(ClientError::ErrorResponse(ErrorCode::UNSUPPORTED_GROUP_TYPE)));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_client_write_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let mut server_tx_bearer = BearerTx::new(test_tx);

                // 1. Await write request
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_WRITE_REQ.into());
                let req = AttWriteCmd::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 10);
                assert_eq!(&packet.as_bytes()[ATT_WRITE_REQ_HEADER_SIZE..], b"Sunstone");

                // 2. Respond with empty WriteRsp
                let mut rsp_buf = [0u8; 1];
                let _ = AttHeaderWriter::new(&mut rsp_buf[..])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_WRITE_RSP);
                let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            // Client driver task
            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                client.write(h(10), b"Sunstone", &mut rx_buf).await.unwrap();
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_write_error() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let mut server_tx_bearer = BearerTx::new(test_tx);

                // 1. Await write request
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_WRITE_REQ.into());
                let req = AttWriteCmd::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 10);

                // 2. Respond with ErrorRsp (WriteNotPermitted)
                let err_buf =
                    make_error_rsp(Opcode::ATT_WRITE_REQ, 10, ErrorCode::WRITE_NOT_PERMITTED);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();
            });

            // Client driver task
            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); CLIENT_PREFERRED_MTU as usize];
                let result = client.write(h(10), b"Sunstone", &mut rx_buf).await;
                assert_eq!(
                    result.err(),
                    Some(ClientError::ErrorResponse(ErrorCode::WRITE_NOT_PERMITTED))
                );
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_write_command_success() {
        let (app_channel, _test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Server driver task
            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_WRITE_CMD.into());
                let req = AttWriteCmd::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 12);
                assert_eq!(&packet.as_bytes()[ATT_WRITE_CMD_HEADER_SIZE..], b"SunstoneCmd");
            });

            // Client driver task
            let client_handle = executor.spawn(async move {
                client.write_command(h(12), b"SunstoneCmd").await.unwrap();
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_write_command_link_closed() {
        let (app_channel, _test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            // Drop test_rx to simulate link closure
            drop(test_rx);

            // Client driver task
            let client_handle = executor.spawn(async move {
                let result = client.write_command(h(12), b"SunstoneCmd").await;
                assert_eq!(result.err(), Some(ClientError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_prepare_write_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx = BearerRx::new(test_rx);
                let mut server_tx = BearerTx::new(test_tx);

                // Receive PrepareWriteReq
                let packet = server_rx.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_PREPARE_WRITE_REQ.into());
                let req = AttPrepareWriteHeader::new(packet.as_bytes());
                assert_eq!(req.attribute_handle().try_read().unwrap(), 10);
                assert_eq!(req.value_offset().try_read().unwrap(), 0);
                assert_eq!(&packet.as_bytes()[ATT_PREPARE_WRITE_HEADER_SIZE..], b"Part1");

                // Echo back PrepareWriteRsp
                let mut rsp_buf = [0u8; 128];
                rsp_buf[..packet.as_bytes().len()].copy_from_slice(packet.as_bytes());
                let _ = AttHeaderWriter::new(&mut rsp_buf[..ATT_HEADER_SIZE])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_PREPARE_WRITE_RSP);
                let tx_packet =
                    Packet::try_ref_from_bytes(&rsp_buf[..packet.as_bytes().len()]).unwrap();
                server_tx.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                client.prepare_write(h(10), 0, b"Part1", &mut rx_buf).await.unwrap();
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_prepare_write_error() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx = BearerRx::new(test_rx);
                let mut server_tx = BearerTx::new(test_tx);

                let _packet = server_rx.next_packet(&mut rx_buf).await.unwrap();

                // Respond with ErrorRsp (InvalidOffset)
                let err_buf =
                    make_error_rsp(Opcode::ATT_PREPARE_WRITE_REQ, 10, ErrorCode::INVALID_OFFSET);
                let tx_packet = Packet::try_ref_from_bytes(&err_buf[..]).unwrap();
                server_tx.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let result = client.prepare_write(h(10), 5, b"Part1", &mut rx_buf).await;
                assert_eq!(
                    result.err(),
                    Some(ClientError::ErrorResponse(ErrorCode::INVALID_OFFSET))
                );
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_prepare_write_invalid_echo() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx = BearerRx::new(test_rx);
                let mut server_tx = BearerTx::new(test_tx);

                let _packet = server_rx.next_packet(&mut rx_buf).await.unwrap();

                // Respond with mismatched echoed payload (mismatched offset 1 instead of 0)
                const PART_VAL: &[u8] = b"Part1";
                let mut rsp_buf = [0u8; ATT_PREPARE_WRITE_HEADER_SIZE + PART_VAL.len()];
                let _ =
                    AttPrepareWriteHeaderWriter::new(&mut rsp_buf[..ATT_PREPARE_WRITE_HEADER_SIZE])
                        .check_complete()
                        .unwrap()
                        .write_attribute_opcode(Opcode::ATT_PREPARE_WRITE_RSP)
                        .write_attribute_handle(10)
                        .write_value_offset(1);
                rsp_buf[ATT_PREPARE_WRITE_HEADER_SIZE..].copy_from_slice(PART_VAL);
                let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).unwrap();
                server_tx.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let result = client.prepare_write(h(10), 0, b"Part1", &mut rx_buf).await;
                assert_eq!(result.err(), Some(ClientError::InvalidIncomingData));
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_execute_write_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );

            let server_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let mut server_rx = BearerRx::new(test_rx);
                let mut server_tx = BearerTx::new(test_tx);

                let packet = server_rx.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_EXECUTE_WRITE_REQ.into());
                let req = AttExecuteWriteReq::new(packet.as_bytes());
                assert_eq!(req.flags().try_read().unwrap(), ExecuteWriteFlags::WRITE);

                let mut rsp_buf = [0u8; ATT_EXECUTE_WRITE_RSP_SIZE];
                let _ = AttHeaderWriter::new(&mut rsp_buf[..])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_EXECUTE_WRITE_RSP);
                let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).unwrap();
                server_tx.send(tx_packet).await.unwrap();
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                client.execute_write(ExecuteWriteFlags::WRITE, &mut rx_buf).await.unwrap();
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_execute_write_error() {
        let (app_channel, _test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client = Client::new(
                BearerTx::new(app_channel.sender),
                router.route_to(RouteFilter::Responses).unwrap(),
                CLIENT_PREFERRED_MTU,
            );
            drop(test_rx); // Simulates LinkClosed

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let result = client.execute_write(ExecuteWriteFlags::WRITE, &mut rx_buf).await;
                assert_eq!(result.err(), Some(ClientError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_client_indication_auto_confirm_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);

        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut event_stream =
                ServerEventStream::new(&router, BearerTx::new(app_channel.sender))
                    .expect("server event stream created");

            let server_handle = executor.spawn(async move {
                let mut server_tx_bearer = BearerTx::new(test_tx);
                let mut ind_buf = [0u8; ATT_HANDLE_VALUE_IND_HEADER_SIZE];
                let _ = AttHandleValueIndHeaderWriter::new(&mut ind_buf)
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_HANDLE_VALUE_IND)
                    .write_attribute_handle(0x1234);
                let tx_packet = Packet::try_ref_from_bytes(&ind_buf).unwrap();
                server_tx_bearer.send(tx_packet).await.unwrap();

                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut server_rx_bearer = BearerRx::new(test_rx);
                let packet = server_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_HANDLE_VALUE_CFM.into());
            });

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let event = event_stream.next(&mut rx_buf).await.unwrap();
                assert_eq!(event.handle().get(), 0x1234);
            });

            executor.run_until_stalled();
            assert!(server_handle.is_finished());
            assert!(client_handle.is_finished());
        });
    }
}
