// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::att::AttributeHandle;
use crate::att::attribute::Attribute;
use crate::att::bearer::{
    AttReceiver, BearerRecvError, BearerSendError, BearerTx, DEFAULT_STARTING_MTU,
    MAX_ATTRIBUTE_SIZE, MAX_SUPPORTED_MTU,
};
use crate::att::database::Database;
use crate::att::l2cap::{L2CapChannelRx, L2CapChannelTx};
use crate::att::pdu::{
    ATT_ERROR_RSP_SIZE, ATT_EXCHANGE_MTU_REQ_SIZE, ATT_EXCHANGE_MTU_RSP_SIZE,
    ATT_EXECUTE_WRITE_REQ_SIZE, ATT_EXECUTE_WRITE_RSP_SIZE, ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE,
    ATT_FIND_INFORMATION_REQ_SIZE, ATT_FIND_INFORMATION_RSP_HEADER_SIZE, ATT_HANDLE_VALUE_CFM_SIZE,
    ATT_HANDLE_VALUE_IND_HEADER_SIZE, ATT_HANDLE_VALUE_NTF_HEADER_SIZE,
    ATT_HANDLES_INFORMATION_SIZE, ATT_HEADER_SIZE, ATT_INFORMATION_DATA_16_SIZE,
    ATT_INFORMATION_DATA_128_SIZE, ATT_PREPARE_WRITE_HEADER_SIZE, ATT_READ_BLOB_REQ_SIZE,
    ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE, ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE,
    ATT_READ_BY_TYPE_REQ_HEADER_SIZE, ATT_READ_REQ_SIZE, ATT_WRITE_CMD_HEADER_SIZE,
    ATT_WRITE_REQ_HEADER_SIZE, ATT_WRITE_RSP_SIZE, ErrorCode, ExecuteWriteFlags, Opcode, Packet,
    UuidFormat, uuid_to_format,
};
use crate::att::router::BearerRxHandle;
use core::cmp::{max, min};
use core::convert::Infallible;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, size_of};
use core::ptr::NonNull;
use sapphire_collections::storage::StorageFamily;
use sapphire_collections::vec::Vec;
use sapphire_common::{PeerId, Uuid};
use sapphire_emboss::att::{
    AttErrorRspWriter, AttExchangeMtuReq, AttExchangeMtuRspWriter, AttExecuteWriteReq,
    AttFindByTypeValueReqHeader, AttFindInformationReq, AttFindInformationRspHeaderWriter,
    AttHandleValueIndHeaderWriter, AttHandleValueNtfHeaderWriter, AttHandlesInformationWriter,
    AttHeader, AttHeaderWriter, AttInformationData16Writer, AttInformationData128Writer,
    AttPrepareWriteHeader, AttReadBlobReq, AttReadByGroupTypeReqHeader,
    AttReadByGroupTypeRspEntryHeaderWriter, AttReadByTypeReqHeader, AttReadReq, AttWriteCmd,
};
use sapphire_emboss::{CheckComplete, CheckOk, InfallibleRead};
use sapphire_sync::mutex::raw::{RawMutex, SingleThreadMutex};
use thiserror::Error;
use zerocopy::{IntoBytes, TryFromBytes};

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerError {
    #[error("Underlying logical link was closed")]
    LinkClosed,
}

/// Error type for server-initiated event transactions.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerEventError {
    #[error("Underlying logical link was closed")]
    LinkClosed,
    #[error("Received invalid confirmation from peer")]
    InvalidConfirmation,
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionError {
    #[error(transparent)]
    ServerError(#[from] ServerError),
    #[error("Unexpected PDU opcode: {received_opcode:?}")]
    UnexpectedPdu { received_opcode: Opcode },
    #[error("Invalid PDU structure for request: {request_opcode:?}")]
    InvalidPdu { request_opcode: Opcode },
    #[error(
        "Error response {error_code:?} for request {request_opcode:?} on handle {attribute_handle:#06X}"
    )]
    ErrorResponse { request_opcode: Opcode, attribute_handle: u16, error_code: ErrorCode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareError {
    QueueFull,
    PayloadTooLarge,
}

/// A parsed item produced by `PrepareQueueDrain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedWrite<'a> {
    pub attribute_handle: u16,
    pub value_offset: u16,
    pub part_attribute_value: &'a [u8],
}

/// A queue for buffering ATT Prepare Write request payloads.
///
/// Under the hood, this stores queued requests contiguously inside a single `Vec`
/// using length-prefix serialization. This is generic over a `StorageFamily` to
/// allow using either stack-allocated or heap-allocated memory backings.
pub struct PrepareQueue<S: StorageFamily> {
    buffer: Vec<u8, S>,
}

impl<S: StorageFamily> Default for PrepareQueue<S>
where
    Vec<u8, S>: Default,
{
    fn default() -> Self {
        Self { buffer: Default::default() }
    }
}

impl<S: StorageFamily> PrepareQueue<S> {
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Creates a new `PrepareQueue` using the provided storage allocator.
    pub fn new_in(alloc: S::Storage<u8>) -> Self {
        Self { buffer: Vec::new_in(alloc) }
    }

    /// Attempts to serialize and push a Prepare Write request.
    pub fn try_push(
        &mut self,
        attribute_handle: u16,
        value_offset: u16,
        part_attribute_value: &[u8],
    ) -> Result<(), PrepareError> {
        let total_payload_len = size_of::<u16>() + size_of::<u16>() + part_attribute_value.len();
        let initial_len = self.buffer.len();

        let len = u16::try_from(total_payload_len).map_err(|_| PrepareError::PayloadTooLarge)?;
        if self.buffer.try_extend(&len.to_ne_bytes()).is_err()
            || self.buffer.try_extend(&attribute_handle.to_le_bytes()).is_err()
            || self.buffer.try_extend(&value_offset.to_le_bytes()).is_err()
            || self.buffer.try_extend(part_attribute_value).is_err()
        {
            self.buffer.truncate(initial_len);
            return Err(PrepareError::QueueFull);
        }
        Ok(())
    }

    /// Clears all queued requests.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Returns true if the prepare queue is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Returns a draining iterator over the queued write requests.
    ///
    /// The queue is automatically cleared when the iterator is dropped.
    pub fn drain<'a>(&'a mut self) -> PrepareQueueDrain<'a, S> {
        let queue = NonNull::from(&mut *self);
        let slice = &self.buffer[..];
        PrepareQueueDrain { slice, cursor: 0, queue, _marker: PhantomData }
    }
}

/// A draining iterator over the queued write requests in `PrepareQueue`.
pub struct PrepareQueueDrain<'a, S: StorageFamily> {
    slice: &'a [u8],
    cursor: usize,
    queue: NonNull<PrepareQueue<S>>,
    // Conceptually borrows the `PrepareQueue` mutably for `'a` to ensure the borrow checker
    // locks the queue exclusively and the drop checker prevents drop-before-use bugs.
    _marker: PhantomData<&'a mut PrepareQueue<S>>,
}

impl<'a, S: StorageFamily> Iterator for PrepareQueueDrain<'a, S> {
    type Item = PreparedWrite<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let buffer_len = self.slice.len();
        if self.cursor == buffer_len {
            return None;
        }
        if self.cursor > buffer_len {
            panic!("PrepareQueue cursor {} exceeded buffer length {}", self.cursor, buffer_len);
        }

        const LENGTH_PREFIX_SIZE: usize = size_of::<u16>();
        let start = self.cursor;
        if start + LENGTH_PREFIX_SIZE > buffer_len {
            panic!("PrepareQueue truncated length prefix at offset {}", start);
        }
        let len_bytes = &self.slice[start..start + LENGTH_PREFIX_SIZE];
        let len = u16::from_ne_bytes([len_bytes[0], len_bytes[1]]) as usize;

        let req_start = start + LENGTH_PREFIX_SIZE;
        let req_end = req_start + len;
        if req_end > buffer_len {
            panic!("PrepareQueue truncated request payload at offset {}", req_start);
        }

        self.cursor = req_end;
        let raw_req = &self.slice[req_start..req_end];
        let (handle_bytes, rest) =
            raw_req.split_first_chunk::<2>().expect("PrepareQueue element shorter than handle");
        let (offset_bytes, part_attribute_value) =
            rest.split_first_chunk::<2>().expect("PrepareQueue element shorter than offset");
        let attribute_handle = u16::from_le_bytes(*handle_bytes);
        let value_offset = u16::from_le_bytes(*offset_bytes);
        Some(PreparedWrite { attribute_handle, value_offset, part_attribute_value })
    }
}

impl<'a, S: StorageFamily> Drop for PrepareQueueDrain<'a, S> {
    fn drop(&mut self) {
        // SAFETY: The lifetime 'a of the mutable borrow of the queue ensures
        // that the raw pointer is valid and exclusive.
        unsafe {
            self.queue.as_mut().clear();
        }
    }
}

/// The ATT Server protocol wrapper.
pub struct Server<Tx, R, DB, S>
where
    S: StorageFamily,
{
    peer_id: PeerId,
    bearer_tx: BearerTx<Tx>,
    bearer_rx: R,
    server_rx_mtu: u16,
    database: DB,
    prepare_storage: PrepareQueue<S>,
}

impl<Tx, R, DB, S> Server<Tx, R, DB, S>
where
    Tx: L2CapChannelTx,
    R: AttReceiver,
    DB: Database,
    S: StorageFamily,
{
    /// Creates a new ATT Server instance.
    pub fn new(
        peer_id: PeerId,
        bearer_tx: BearerTx<Tx>,
        bearer_rx: R,
        server_rx_mtu: u16,
        database: DB,
        prepare_storage: PrepareQueue<S>,
    ) -> Self {
        assert!(
            usize::from(server_rx_mtu) <= MAX_SUPPORTED_MTU,
            "server_rx_mtu ({}) exceeds MAX_SUPPORTED_MTU ({})",
            server_rx_mtu,
            MAX_SUPPORTED_MTU
        );
        Self { peer_id, bearer_tx, bearer_rx, server_rx_mtu, database, prepare_storage }
    }

    /// Runs the server receive loop, processing inbound requests sequentially
    /// until the underlying channel is closed or an error occurs.
    pub async fn run(&mut self) -> Result<Infallible, ServerError> {
        loop {
            self.handle_request().await?;
        }
    }

    /// Processes a single inbound request packet.
    pub async fn handle_request(&mut self) -> Result<(), ServerError> {
        // TODO(https://fxbug.dev/530178099): Reconsider stack allocation here, as it bloats the generated Future's
        // size. Consider storing a reusable buffer in Server or heap-allocating to match MTU.
        let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
        let rx_packet = match self.bearer_rx.next_packet(&mut rx_buf).await {
            Ok(pkt) => pkt,
            // Channel disconnected. Terminate server.
            Err(BearerRecvError::LinkClosed) => return Err(ServerError::LinkClosed),

            // If the Attribute Opcode cannot be determined because the request was too short or
            // exceeded MAX_SUPPORTED_MTU, the server responds with an Error Response (Invalid PDU)
            // setting the Opcode In Error to 0x00.
            //
            // see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.1.1)
            Err(BearerRecvError::HeaderTooShort) => {
                self.send_error_response(0u8, None, ErrorCode::INVALID_PDU).await?;
                return Ok(());
            }
            Err(BearerRecvError::BufferTooSmall) => {
                panic!(
                    "Programming error: provided buffer size is smaller than the negotiated MTU."
                );
            }

            // Packet size exceeds MTU. According to BT Spec, the server shall return an
            // Error Response with the error code set to Invalid PDU (0x04).
            //
            // see (Vol 3, Part F, 3.4.1.1)
            Err(BearerRecvError::PacketTooLarge { opcode }) => {
                self.send_error_response(opcode, None, ErrorCode::INVALID_PDU).await?;
                return Ok(());
            }

            // Unknown or unsupported opcode. According to BT Spec, the server shall respond
            // with Request Not Supported (0x06).
            //
            // see (Vol 3, Part F, 3.4.1.1)
            Err(BearerRecvError::InvalidOpcode(raw_opcode)) => {
                self.send_error_response(raw_opcode, None, ErrorCode::REQUEST_NOT_SUPPORTED)
                    .await?;
                return Ok(());
            }
        };

        let header = AttHeader::new(rx_packet.as_bytes());
        let opcode = header
            .attribute_opcode()
            .try_read()
            .expect("packet opcode already validated by bearer");
        let transaction_result = match opcode {
            Opcode::ATT_EXCHANGE_MTU_REQ => self.handle_exchange_mtu(rx_packet.as_bytes()).await,
            Opcode::ATT_FIND_INFORMATION_REQ => {
                self.handle_find_information(rx_packet.as_bytes()).await
            }
            Opcode::ATT_FIND_BY_TYPE_VALUE_REQ => {
                self.handle_find_by_type_value(rx_packet.as_bytes()).await
            }
            Opcode::ATT_READ_REQ => self.handle_read(rx_packet.as_bytes()).await,
            Opcode::ATT_READ_BLOB_REQ => self.handle_read_blob(rx_packet.as_bytes()).await,
            Opcode::ATT_READ_BY_TYPE_REQ => self.handle_read_by_type(rx_packet.as_bytes()).await,
            Opcode::ATT_READ_BY_GROUP_TYPE_REQ => {
                self.handle_read_by_group_type(rx_packet.as_bytes()).await
            }
            Opcode::ATT_WRITE_REQ => self.handle_write_req(rx_packet.as_bytes()).await,
            Opcode::ATT_WRITE_CMD => self.handle_write_cmd(rx_packet.as_bytes()).await,
            Opcode::ATT_PREPARE_WRITE_REQ => self.handle_prepare_write(rx_packet.as_bytes()).await,
            Opcode::ATT_EXECUTE_WRITE_REQ => self.handle_execute_write(rx_packet.as_bytes()).await,
            other => Err(TransactionError::UnexpectedPdu { received_opcode: other }),
        };

        match transaction_result {
            Ok(()) => Ok(()),
            Err(TransactionError::ServerError(e)) => Err(e),
            Err(TransactionError::UnexpectedPdu { received_opcode }) => {
                self.send_error_response(received_opcode, None, ErrorCode::REQUEST_NOT_SUPPORTED)
                    .await
            }
            Err(TransactionError::InvalidPdu { request_opcode }) => {
                self.send_error_response(request_opcode, None, ErrorCode::INVALID_PDU).await
            }
            Err(TransactionError::ErrorResponse {
                request_opcode,
                attribute_handle,
                error_code,
            }) => {
                self.send_error_response(
                    request_opcode,
                    AttributeHandle::new(attribute_handle),
                    error_code,
                )
                .await
            }
        }
    }

    /// Handles an incoming Exchange MTU Request and responds with an Exchange MTU Response.
    /// Negotiates the ATT_MTU for the connection.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.2.1) and (Vol 3, Part G, Section 5.2.1)
    async fn handle_exchange_mtu(&mut self, packet_bytes: &[u8]) -> Result<(), TransactionError> {
        let req = AttExchangeMtuReq::new(packet_bytes).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_EXCHANGE_MTU_REQ }
        })?;
        let client_mtu = req.client_rx_mtu().read();

        let negotiated_mtu = max(DEFAULT_STARTING_MTU, min(client_mtu, self.server_rx_mtu));

        // Update MTU for both active bearer halves
        self.bearer_tx.set_mtu(negotiated_mtu);
        self.bearer_rx.set_mtu(negotiated_mtu);

        // Respond with ExchangeMtuRsp containing our supported rx MTU
        let mut buf = [0u8; ATT_EXCHANGE_MTU_RSP_SIZE];
        let _ = AttExchangeMtuRspWriter::new(&mut buf[..])
            .check_complete()
            .expect("statically sized Exchange MTU response buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_EXCHANGE_MTU_RSP)
            .write_server_rx_mtu(self.server_rx_mtu);

        let tx_packet = Packet::try_ref_from_bytes(&buf[..]).expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Handles an incoming Find Information Request, querying the database and sending a
    /// Find Information Response (or Error Response) back to the client.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.3).
    async fn handle_find_information(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() != ATT_FIND_INFORMATION_REQ_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_FIND_INFORMATION_REQ,
            });
        }
        let req = AttFindInformationReq::new(payload).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_FIND_INFORMATION_REQ }
        })?;
        let start = req.starting_handle().read();
        let end = req.ending_handle().read();

        if start > end {
            return Err(TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_FIND_INFORMATION_REQ,
                attribute_handle: start,
                error_code: ErrorCode::INVALID_HANDLE,
            });
        }

        let start_handle = to_handle(start, Opcode::ATT_FIND_INFORMATION_REQ)?;
        let end_handle = to_handle(end, Opcode::ATT_FIND_INFORMATION_REQ)?;
        let mut attributes = self.database.query_range(start_handle, end_handle).peekable();
        let format = match attributes.peek() {
            Some((_, attr)) => uuid_to_format(attr.uuid()),
            None => {
                return Err(TransactionError::ErrorResponse {
                    request_opcode: Opcode::ATT_FIND_INFORMATION_REQ,
                    attribute_handle: start,
                    error_code: ErrorCode::ATTRIBUTE_NOT_FOUND,
                });
            }
        };

        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        assert!(
            tx_buf.len() >= usize::from(self.mtu()),
            "Programming error: transmission buffer size is smaller than the negotiated MTU."
        );

        let _ = AttFindInformationRspHeaderWriter::new(
            &mut tx_buf[..ATT_FIND_INFORMATION_RSP_HEADER_SIZE],
        )
        .check_complete()
        .expect("statically sized Find Information header buffer must be complete")
        .write_attribute_opcode(Opcode::ATT_FIND_INFORMATION_RSP)
        .write_format(format);

        let limit = self.effective_mtu();
        let mut offset = ATT_FIND_INFORMATION_RSP_HEADER_SIZE;

        for (handle, attr) in
            attributes.take_while(|(_, attr)| uuid_to_format(attr.uuid()) == format)
        {
            match format {
                UuidFormat::BIT16 => {
                    if offset + ATT_INFORMATION_DATA_16_SIZE > limit {
                        break;
                    }
                    let uuid16 = u16::try_from(*attr.uuid()).expect("valid 16-bit uuid");
                    let _ = AttInformationData16Writer::new(
                        &mut tx_buf[offset..offset + ATT_INFORMATION_DATA_16_SIZE],
                    )
                    .check_complete()
                    .expect(
                        "statically sized Find Information 16-bit entry buffer must be complete",
                    )
                    .write_attribute_handle(handle.value())
                    .write_uuid(uuid16);
                    offset += ATT_INFORMATION_DATA_16_SIZE;
                }
                UuidFormat::BIT128 => {
                    if offset + ATT_INFORMATION_DATA_128_SIZE > limit {
                        break;
                    }
                    let _ = AttInformationData128Writer::new(
                        &mut tx_buf[offset..offset + ATT_INFORMATION_DATA_128_SIZE],
                    )
                    .check_complete()
                    .expect(
                        "statically sized Find Information 128-bit entry buffer must be complete",
                    )
                    .write_attribute_handle(handle.value());
                    tx_buf[offset + 2..offset + 18].copy_from_slice(attr.uuid().as_bytes());
                    offset += ATT_INFORMATION_DATA_128_SIZE;
                }
            }
        }

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..offset]).expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Handles an incoming Find By Type Value Request.
    ///
    /// Queries the database for attributes matching the requested range, type, and value,
    /// and responds with their handle ranges. If no matches are found, returns `AttributeNotFound`.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.3.3).
    async fn handle_find_by_type_value(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_FIND_BY_TYPE_VALUE_REQ,
            });
        }
        let header = AttFindByTypeValueReqHeader::new(payload).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_FIND_BY_TYPE_VALUE_REQ }
        })?;
        let start = header.starting_handle().read();
        let end = header.ending_handle().read();
        let attr_type = header.attribute_type().read();
        let requested_value = &payload[ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE..];

        if start > end {
            return Err(TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_FIND_BY_TYPE_VALUE_REQ,
                attribute_handle: start,
                error_code: ErrorCode::INVALID_HANDLE,
            });
        }

        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        assert!(
            tx_buf.len() >= usize::from(self.mtu()),
            "Programming error: transmission buffer size is smaller than the negotiated MTU."
        );

        tx_buf[0] = Opcode::ATT_FIND_BY_TYPE_VALUE_RSP as u8;
        let limit = self.effective_mtu();
        let mut offset = ATT_HEADER_SIZE;

        let start_handle = to_handle(start, Opcode::ATT_FIND_BY_TYPE_VALUE_REQ)?;
        let end_handle = to_handle(end, Opcode::ATT_FIND_BY_TYPE_VALUE_REQ)?;
        let attributes = self.database.query_range(start_handle, end_handle).filter(|(_, attr)| {
            <[u8; 2]>::try_from(*attr.uuid())
                .is_ok_and(|bytes16| u16::from_le_bytes(bytes16) == attr_type)
        });
        for (handle, attr) in attributes {
            // TODO(https://fxbug.dev/527551044): Implement zero-copy value matching in Attribute trait to avoid stack copying in ATT Server
            // Should be done when production GATT Database is implemented (which will implement the Attribute trait)
            let mut read_buf = [0u8; MAX_ATTRIBUTE_SIZE];
            if let Ok(read_len) = attr.read_chunk(self.peer_id, 0, &mut read_buf).await {
                if read_len == requested_value.len() && &read_buf[..read_len] == requested_value {
                    let group_end = attr.group_end_handle().unwrap_or_else(|| handle.value());
                    if offset + ATT_HANDLES_INFORMATION_SIZE > limit {
                        break;
                    }
                    let _ = AttHandlesInformationWriter::new(
                        &mut tx_buf[offset..offset + ATT_HANDLES_INFORMATION_SIZE],
                    )
                    .check_complete()
                    .expect("statically sized Find By Type Value entry buffer must be complete")
                    .write_attribute_handle(handle.value())
                    .write_group_end_handle(group_end);
                    offset += ATT_HANDLES_INFORMATION_SIZE;
                }
            }
        }

        if offset == ATT_HEADER_SIZE {
            return Err(TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_FIND_BY_TYPE_VALUE_REQ,
                attribute_handle: start,
                error_code: ErrorCode::ATTRIBUTE_NOT_FOUND,
            });
        }

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..offset]).expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Handles an incoming Read Request and responds with a Read Response containing the value.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.1 & 3.4.4.2)
    async fn handle_read(&mut self, data: &[u8]) -> Result<(), TransactionError> {
        if data.len() != ATT_READ_REQ_SIZE {
            return Err(TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_REQ });
        }
        // Parse the incoming Read Request.
        let req = AttReadReq::new(data)
            .check_ok()
            .map_err(|_| TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_REQ })?;
        let handle_val = req.attribute_handle().read();

        // Read the attribute value from the database, capped to the maximum possible response size.
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttHeaderWriter::new(&mut tx_buf[..ATT_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Read Response header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_READ_RSP);
        let response_capacity = self.effective_mtu() - ATT_HEADER_SIZE;
        let read_len = self
            .read_attribute_at(
                handle_val,
                0,
                Opcode::ATT_READ_REQ,
                &mut tx_buf[ATT_HEADER_SIZE..ATT_HEADER_SIZE + response_capacity],
            )
            .await?;

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..ATT_HEADER_SIZE + read_len])
            .expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Handles an incoming Read Blob Request and responds with a Read Blob Response.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.4.3 & 3.4.4.4)
    async fn handle_read_blob(&mut self, data: &[u8]) -> Result<(), TransactionError> {
        if data.len() != ATT_READ_BLOB_REQ_SIZE {
            return Err(TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_BLOB_REQ });
        }
        // Parse the incoming Read Blob Request.
        let req = AttReadBlobReq::new(data).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_BLOB_REQ }
        })?;
        let handle_val = req.attribute_handle().read();
        let offset = req.value_offset().read();

        // Read the attribute chunk starting from the requested offset, capped to the response size.
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttHeaderWriter::new(&mut tx_buf[..ATT_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Read Blob Response header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_READ_BLOB_RSP);
        let response_capacity = self.effective_mtu() - ATT_HEADER_SIZE;
        let read_len = self
            .read_attribute_at(
                handle_val,
                offset,
                Opcode::ATT_READ_BLOB_REQ,
                &mut tx_buf[ATT_HEADER_SIZE..ATT_HEADER_SIZE + response_capacity],
            )
            .await?;

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..ATT_HEADER_SIZE + read_len])
            .expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Shares range query execution logic for Read By Type and Read By Group Type requests.
    ///
    /// Senders format entry headers into a buffer using `write_entry_header` (returning Err on
    /// failure), which determines the first entry's header size
    /// before the output packet is assembled.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Sections 3.4.4.7 to 3.4.4.10).
    async fn handle_read_by_type_generic(
        &mut self,
        start_handle_val: u16,
        end_handle_val: u16,
        type_bytes: &[u8],
        req_opcode: Opcode,
        rsp_opcode: Opcode,
        mut write_entry_header: impl FnMut(
            &mut [u8],
            AttributeHandle,
            &DB::Attr,
        ) -> Result<usize, ErrorCode>,
    ) -> Result<(), TransactionError> {
        let start_handle = to_handle(start_handle_val, req_opcode)?;
        let end_handle = to_handle(end_handle_val, req_opcode)?;

        if start_handle > end_handle {
            return Err(TransactionError::ErrorResponse {
                request_opcode: req_opcode,
                attribute_handle: start_handle_val,
                error_code: ErrorCode::INVALID_HANDLE,
            });
        }

        let uuid = Uuid::try_from(type_bytes)
            .map_err(|_| TransactionError::InvalidPdu { request_opcode: req_opcode })?;

        const MAX_ENTRY_HEADER_SIZE: usize = ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE;

        // Query range for matching attributes.
        let mut attributes = self
            .database
            .query_range(start_handle, end_handle)
            .filter(|(_, attr)| attr.uuid() == &uuid)
            .peekable();

        if attributes.peek().is_none() {
            return Err(TransactionError::ErrorResponse {
                request_opcode: req_opcode,
                attribute_handle: start_handle_val,
                error_code: ErrorCode::ATTRIBUTE_NOT_FOUND,
            });
        }

        // We must read the first attribute to establish the element length
        // before serializing entries, so that the header length is populated correctly.
        let (first_handle, first_attr) =
            attributes.next().expect("peek verified at least one attribute present");

        // Pre-validate grouping type constraints on the first matched attribute.
        let mut first_entry_header_buf = [0u8; MAX_ENTRY_HEADER_SIZE];
        let first_entry_header_len =
            match write_entry_header(&mut first_entry_header_buf, first_handle, first_attr) {
                Ok(len) => len,
                Err(error_code) => {
                    return Err(TransactionError::ErrorResponse {
                        request_opcode: req_opcode,
                        attribute_handle: start_handle_val,
                        error_code,
                    });
                }
            };

        // The Length field is 1 byte, so the maximum size of an entry is u8::MAX (255).
        let limit = self.effective_mtu();

        // Since each entry must contain the entry header,
        // the maximum value length is u8::MAX - entry header size.
        let max_value_len = usize::from(u8::MAX) - first_entry_header_len;
        let min_payload_size = ATT_HEADER_SIZE + size_of::<u8>() + first_entry_header_len;

        // If the attribute value is longer than the remaining MTU space
        // or the max possible entry size, only the first chunk is read in this response
        //
        // (see Bluetooth Core Spec v6.0, Vol 3, Part F, Section 3.4.4.8 for Read By Type,
        // and Section 3.4.4.10 for Read By Group Type).
        //
        // Note: since the minimum ATT MTU is 23, `limit` is guaranteed to be larger than
        // `min_payload_size` (at most 6), so `first_read_limit` is always > 0.
        let first_read_limit = min(limit.saturating_sub(min_payload_size), max_value_len);
        debug_assert_ne!(first_read_limit, 0);

        let mut first_val_buf = [0u8; MAX_ATTRIBUTE_SIZE];
        let first_read_len = first_attr
            .read_chunk(self.peer_id, 0, &mut first_val_buf[..first_read_limit])
            .await
            .map_err(|error_code| TransactionError::ErrorResponse {
                request_opcode: req_opcode,
                attribute_handle: first_handle.value(),
                error_code,
            })?;

        let entry_size = first_entry_header_len + first_read_len;
        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttHeaderWriter::new(&mut tx_buf[..ATT_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Read By Type Response header buffer must be complete")
            .write_attribute_opcode(rsp_opcode);
        tx_buf[ATT_HEADER_SIZE] =
            u8::try_from(entry_size).expect("Range response payload length fits in u8");

        let mut offset = ATT_HEADER_SIZE + 1;
        tx_buf[offset..offset + first_entry_header_len]
            .copy_from_slice(&first_entry_header_buf[..first_entry_header_len]);
        offset += first_entry_header_len;
        tx_buf[offset..offset + first_read_len].copy_from_slice(&first_val_buf[..first_read_len]);
        offset += first_read_len;

        // Pack matching attributes into response.
        for (handle, attr) in attributes {
            let mut entry_header_buf = [0u8; MAX_ENTRY_HEADER_SIZE];
            let entry_header_len = match write_entry_header(&mut entry_header_buf, handle, attr) {
                Ok(len) => len,
                Err(_) => break,
            };

            if offset + entry_header_len + first_read_len > limit {
                break;
            }

            let mut val_buf = [0u8; MAX_ATTRIBUTE_SIZE];
            let Ok(read_len) =
                attr.read_chunk(self.peer_id, 0, &mut val_buf[..first_read_len + 1]).await
            else {
                break;
            };
            if read_len != first_read_len {
                break;
            }
            tx_buf[offset..offset + entry_header_len]
                .copy_from_slice(&entry_header_buf[..entry_header_len]);
            offset += entry_header_len;
            tx_buf[offset..offset + read_len].copy_from_slice(&val_buf[..read_len]);
            offset += read_len;
        }

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..offset]).expect("valid packet");
        self.send_packet(tx_packet).await?;
        Ok(())
    }

    async fn handle_read_by_type(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_READ_BY_TYPE_REQ_HEADER_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_READ_BY_TYPE_REQ,
            });
        }
        let req = AttReadByTypeReqHeader::new(payload).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_BY_TYPE_REQ }
        })?;
        let start_handle_val = req.starting_handle().read();
        let end_handle_val = req.ending_handle().read();
        let type_bytes = &payload[ATT_READ_BY_TYPE_REQ_HEADER_SIZE..];

        self.handle_read_by_type_generic(
            start_handle_val,
            end_handle_val,
            type_bytes,
            Opcode::ATT_READ_BY_TYPE_REQ,
            Opcode::ATT_READ_BY_TYPE_RSP,
            |buf, handle, _attr| {
                let handle_size = size_of::<AttributeHandle>();
                buf[..handle_size].copy_from_slice(&handle.value().to_le_bytes());
                Ok(handle_size)
            },
        )
        .await
    }

    /// Handles a Read By Group Type Request, querying the database for grouped attributes
    /// matching the given UUID group type and handle range, and returning a packed list
    /// of handles, end group handles, and values.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Sections 3.4.4.9 & 3.4.4.10).
    async fn handle_read_by_group_type(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_READ_BY_GROUP_TYPE_REQ,
            });
        }
        let req = AttReadByGroupTypeReqHeader::new(payload).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_READ_BY_GROUP_TYPE_REQ }
        })?;
        let start_handle_val = req.starting_handle().read();
        let end_handle_val = req.ending_handle().read();
        let type_bytes = &payload[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE..];

        self.handle_read_by_type_generic(
            start_handle_val,
            end_handle_val,
            type_bytes,
            Opcode::ATT_READ_BY_GROUP_TYPE_REQ,
            Opcode::ATT_READ_BY_GROUP_TYPE_RSP,
            |buf, handle, attr| {
                let end_group_handle = match attr.group_end_handle() {
                    Some(end_group) => end_group,
                    None => return Err(ErrorCode::UNSUPPORTED_GROUP_TYPE),
                };
                let _ = AttReadByGroupTypeRspEntryHeaderWriter::new(
                    &mut buf[..ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE],
                )
                .check_complete()
                .expect("statically sized Read By Group Type entry header buffer must be complete")
                .write_attribute_handle(handle.value())
                .write_end_group_handle(end_group_handle);
                Ok(ATT_READ_BY_GROUP_TYPE_RSP_ENTRY_HEADER_SIZE)
            },
        )
        .await
    }

    /// Handles a Write Request, invoking a database write operation and returning an empty Write Response.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Sections 3.4.5.1 & 3.4.5.2).
    async fn handle_write_req(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_WRITE_REQ_HEADER_SIZE {
            return Err(TransactionError::InvalidPdu { request_opcode: Opcode::ATT_WRITE_REQ });
        }
        let req = AttWriteCmd::new(&payload[..ATT_WRITE_REQ_HEADER_SIZE])
            .check_ok()
            .map_err(|_| TransactionError::InvalidPdu { request_opcode: Opcode::ATT_WRITE_REQ })?;
        let handle_val = req.attribute_handle().read();
        let handle = to_handle(handle_val, Opcode::ATT_WRITE_REQ)?;
        let value = &payload[ATT_WRITE_REQ_HEADER_SIZE..];

        let attr = self.database.find_attribute(handle).ok_or_else(|| {
            TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_WRITE_REQ,
                attribute_handle: handle_val,
                error_code: ErrorCode::INVALID_HANDLE,
            }
        })?;

        attr.write_chunk(self.peer_id, 0, value).await.map_err(|error_code| {
            TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_WRITE_REQ,
                attribute_handle: handle_val,
                error_code,
            }
        })?;

        let mut rsp_buf = [0u8; ATT_WRITE_RSP_SIZE];
        let _ = AttHeaderWriter::new(&mut rsp_buf[..])
            .check_complete()
            .expect("statically sized Write Response header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_WRITE_RSP);
        let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).expect("valid packet");
        self.send_packet(tx_packet).await?;

        Ok(())
    }

    /// Handles a Write Command, executing database mutation but ignoring any errors/responses.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.5.3).
    async fn handle_write_cmd(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_WRITE_CMD_HEADER_SIZE {
            return Ok(());
        }
        let req = match AttWriteCmd::new(&payload[..ATT_WRITE_CMD_HEADER_SIZE]).check_ok() {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let handle_val = req.attribute_handle().read();
        let handle = match to_handle(handle_val, Opcode::ATT_WRITE_CMD) {
            Ok(h) => h,
            Err(_) => return Ok(()),
        };

        let attr = match self.database.find_attribute(handle) {
            Some(a) => a,
            None => return Ok(()),
        };

        // Silently execute write and ignore any error codes
        let _ = attr.write_chunk(self.peer_id, 0, &payload[ATT_WRITE_CMD_HEADER_SIZE..]).await;
        Ok(())
    }

    /// Handles a Prepare Write Request.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.6.1).
    async fn handle_prepare_write(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() < ATT_PREPARE_WRITE_HEADER_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
            });
        }
        let req = AttPrepareWriteHeader::new(&payload[..ATT_PREPARE_WRITE_HEADER_SIZE])
            .check_ok()
            .map_err(|_| TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
            })?;
        let handle_val = req.attribute_handle().read();
        let handle = to_handle(handle_val, Opcode::ATT_PREPARE_WRITE_REQ)?;
        let offset = req.value_offset().read();
        let part_attribute_value = &payload[ATT_PREPARE_WRITE_HEADER_SIZE..];

        let attr = self.database.find_attribute(handle).ok_or_else(|| {
            TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
                attribute_handle: handle_val,
                error_code: ErrorCode::INVALID_HANDLE,
            }
        })?;

        // Validate offset. If offset > attribute value length, return InvalidOffset.
        attr.read_chunk(self.peer_id, offset, &mut []).await.map_err(|error_code| {
            TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
                attribute_handle: handle_val,
                error_code,
            }
        })?;

        // Enforce maximum BT Spec attribute value length (512 bytes)
        if usize::from(offset) + part_attribute_value.len() > MAX_ATTRIBUTE_SIZE {
            return Err(TransactionError::ErrorResponse {
                request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
                attribute_handle: handle_val,
                error_code: ErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH,
            });
        }

        self.prepare_storage.try_push(handle_val, offset, part_attribute_value).map_err(
            |error| match error {
                PrepareError::QueueFull => TransactionError::ErrorResponse {
                    request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
                    attribute_handle: handle_val,
                    error_code: ErrorCode::PREPARE_QUEUE_FULL,
                },
                PrepareError::PayloadTooLarge => TransactionError::ErrorResponse {
                    request_opcode: Opcode::ATT_PREPARE_WRITE_REQ,
                    attribute_handle: handle_val,
                    error_code: ErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH,
                },
            },
        )?;

        let mut rsp_buf = [0u8; MAX_SUPPORTED_MTU];
        rsp_buf[..payload.len()].copy_from_slice(payload);
        let _ = AttHeaderWriter::new(&mut rsp_buf[..ATT_HEADER_SIZE])
            .check_complete()
            .expect("statically sized Prepare Write Response header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_PREPARE_WRITE_RSP);
        let tx_packet =
            Packet::try_ref_from_bytes(&rsp_buf[..payload.len()]).expect("valid packet");
        self.send_packet(tx_packet).await?;
        Ok(())
    }

    /// Handles an Execute Write Request.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.6.3).
    async fn handle_execute_write(&mut self, payload: &[u8]) -> Result<(), TransactionError> {
        if payload.len() != ATT_EXECUTE_WRITE_REQ_SIZE {
            return Err(TransactionError::InvalidPdu {
                request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ,
            });
        }
        let req = AttExecuteWriteReq::new(payload).check_ok().map_err(|_| {
            TransactionError::InvalidPdu { request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ }
        })?;
        let flags = match req.flags().read() {
            Ok(f) => f,
            Err(_) => {
                return Err(TransactionError::InvalidPdu {
                    request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ,
                });
            }
        };

        match flags {
            ExecuteWriteFlags::CANCEL => {
                self.prepare_storage.clear();
            }
            ExecuteWriteFlags::WRITE => {
                // If any queued write fails, the server aborts the transaction, discards all
                // remaining prepared writes, and returns the error.
                //
                // see Bluetooth Core Spec Vol 3, Part F, Section 3.4.6.3.
                for req in self.prepare_storage.drain() {
                    let handle_val = req.attribute_handle;
                    let handle = match AttributeHandle::try_from(handle_val) {
                        Ok(h) => h,
                        Err(_) => {
                            // Note: Returning early drops the draining iterator, triggering
                            // RAII cancellation and clearing of all remaining queued writes.
                            return Err(TransactionError::ErrorResponse {
                                request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ,
                                attribute_handle: handle_val,
                                error_code: ErrorCode::INVALID_HANDLE,
                            });
                        }
                    };
                    let attr = match self.database.find_attribute(handle) {
                        Some(a) => a,
                        None => {
                            return Err(TransactionError::ErrorResponse {
                                request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ,
                                attribute_handle: handle_val,
                                error_code: ErrorCode::INVALID_HANDLE,
                            });
                        }
                    };
                    let offset = req.value_offset;
                    if let Err(error_code) =
                        attr.write_chunk(self.peer_id, offset, req.part_attribute_value).await
                    {
                        return Err(TransactionError::ErrorResponse {
                            request_opcode: Opcode::ATT_EXECUTE_WRITE_REQ,
                            attribute_handle: handle_val,
                            error_code,
                        });
                    }
                }
            }
        }

        let mut rsp_buf = [0u8; ATT_EXECUTE_WRITE_RSP_SIZE];
        let _ = AttHeaderWriter::new(&mut rsp_buf[..])
            .check_complete()
            .expect("statically sized Execute Write Response header buffer must be complete")
            .write_attribute_opcode(Opcode::ATT_EXECUTE_WRITE_RSP);
        let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).expect("valid packet");
        self.send_packet(tx_packet).await?;
        Ok(())
    }

    /// Helper to read an attribute value at an offset and cap it to a maximum buffer size.
    async fn read_attribute_at(
        &self,
        handle_val: u16,
        offset: u16,
        request_opcode: Opcode,
        buf: &mut [u8],
    ) -> Result<usize, TransactionError> {
        let handle = to_handle(handle_val, request_opcode)?;
        let attr = self.database.find_attribute(handle).ok_or_else(|| {
            TransactionError::ErrorResponse {
                request_opcode,
                attribute_handle: handle_val,
                error_code: ErrorCode::INVALID_HANDLE,
            }
        })?;
        attr.read_chunk(self.peer_id, offset, buf).await.map_err(|error_code| {
            TransactionError::ErrorResponse {
                request_opcode,
                attribute_handle: handle_val,
                error_code,
            }
        })
    }

    async fn send_packet(&mut self, packet: &Packet) -> Result<(), ServerError> {
        match self.bearer_tx.send(packet).await {
            Ok(()) => Ok(()),
            // Channel disconnected. Terminate server.
            Err(BearerSendError::LinkClosed) => Err(ServerError::LinkClosed),
            // Outgoing packet size exceeds MTU. This is a local logic error.
            Err(BearerSendError::PacketTooLarge) => {
                panic!("Programming error: outgoing packet size exceeds the negotiated MTU.");
            }
        }
    }

    /// Formats and transmits an ATT Error Response PDU.
    ///
    /// Accepts the `request_opcode` as an `impl Into<u8>` (which fits both the typed `Opcode`
    /// enum and raw `u8` invalid opcodes) to satisfy the Bluetooth Specification requirements
    /// for error reporting on unknown/invalid opcodes.
    async fn send_error_response(
        &mut self,
        request_opcode: impl Into<u8>,
        attribute_handle: Option<AttributeHandle>,
        error_code: ErrorCode,
    ) -> Result<(), ServerError> {
        let handle_raw = attribute_handle.map(|h| h.value()).unwrap_or(0);
        let mut buf = [0u8; ATT_ERROR_RSP_SIZE];
        let _ = AttErrorRspWriter::new(&mut buf[..])
            .check_complete()
            .expect("valid buffer")
            .write_attribute_opcode(Opcode::ATT_ERROR_RSP)
            .write_request_opcode_in_error_uint(request_opcode.into())
            .write_attribute_handle(handle_raw)
            .write_error_code(error_code);
        let tx_packet = Packet::try_ref_from_bytes(&buf[..]).expect("valid packet");
        self.send_packet(tx_packet).await
    }

    /// Obtains a concurrent ServerNotifier handle sharing the underlying BearerTx.
    pub fn notifier(&self) -> ServerNotifier<Tx> {
        ServerNotifier::new(self.bearer_tx.clone())
    }

    /// Obtains a concurrent ServerIndicator handle sharing the underlying BearerTx and confirmation receiver.
    pub fn indicator<'a, RxPhys: L2CapChannelRx, Mtx: RawMutex>(
        &self,
        cfm_rx_handle: BearerRxHandle<'a, RxPhys, Mtx>,
    ) -> ServerIndicator<'a, Tx, RxPhys, Mtx> {
        ServerIndicator::new(self.bearer_tx.clone(), cfm_rx_handle)
    }

    pub fn mtu(&self) -> u16 {
        self.bearer_tx.mtu()
    }

    fn effective_mtu(&self) -> usize {
        usize::try_from(self.mtu()).unwrap_or(usize::MAX)
    }
}

/// A concurrent, statically dispatched server notification handle driving unsolicited Server Notifications
/// side-by-side with an actively running Server instance.
#[derive(Clone, Debug)]
pub struct ServerNotifier<Tx> {
    bearer_tx: BearerTx<Tx>,
}

impl<Tx> ServerNotifier<Tx>
where
    Tx: L2CapChannelTx,
{
    pub fn new(bearer_tx: BearerTx<Tx>) -> Self {
        Self { bearer_tx }
    }

    fn effective_mtu(&self) -> usize {
        usize::from(self.bearer_tx.mtu())
    }

    /// Sends a Handle Value Notification asynchronously down the shared BearerTx link.
    ///
    /// Panics if the notification payload size exceeds the currently
    /// negotiated ATT MTU boundary.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.7.1).
    pub async fn notify(&mut self, handle: u16, value: &[u8]) -> Result<(), ServerEventError> {
        let total_size = ATT_HANDLE_VALUE_NTF_HEADER_SIZE + value.len();

        assert!(
            total_size <= self.effective_mtu(),
            "Programming error: Handle Value Notification payload size ({}) exceeds effective MTU ({}).",
            total_size,
            self.effective_mtu()
        );

        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttHandleValueNtfHeaderWriter::new(&mut tx_buf[..ATT_HANDLE_VALUE_NTF_HEADER_SIZE])
            .check_complete()
            .expect("valid buffer")
            .write_attribute_opcode(Opcode::ATT_HANDLE_VALUE_NTF)
            .write_attribute_handle(handle);
        tx_buf[ATT_HANDLE_VALUE_NTF_HEADER_SIZE..total_size].copy_from_slice(value);

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..total_size]).expect("valid packet");
        match self.bearer_tx.send(tx_packet).await {
            Ok(()) => Ok(()),
            Err(BearerSendError::LinkClosed) => Err(ServerEventError::LinkClosed),
            Err(BearerSendError::PacketTooLarge) => {
                panic!("Programming error: outgoing packet size exceeds the negotiated MTU.");
            }
        }
    }
}

/// A handle for driving Handle Value Indications side-by-side with an actively running Server instance.
pub struct ServerIndicator<'a, Tx, Rx, Mtx = SingleThreadMutex> {
    bearer_tx: BearerTx<Tx>,
    cfm_rx_handle: BearerRxHandle<'a, Rx, Mtx>,
}

impl<'a, Tx, Rx, Mtx> ServerIndicator<'a, Tx, Rx, Mtx>
where
    Tx: L2CapChannelTx,
    Rx: L2CapChannelRx,
    Mtx: RawMutex,
{
    pub fn new(bearer_tx: BearerTx<Tx>, cfm_rx_handle: BearerRxHandle<'a, Rx, Mtx>) -> Self {
        Self { bearer_tx, cfm_rx_handle }
    }

    fn effective_mtu(&self) -> usize {
        usize::from(self.bearer_tx.mtu())
    }

    /// Transmits a Handle Value Indication PDU and awaits the matching Handle Value Confirmation from the peer.
    ///
    /// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.4.7.2 and 3.4.7.3).
    pub async fn indicate(&mut self, handle: u16, value: &[u8]) -> Result<(), ServerEventError> {
        let total_size = ATT_HANDLE_VALUE_IND_HEADER_SIZE + value.len();

        assert!(
            total_size <= self.effective_mtu(),
            "Programming error: Handle Value Indication payload size ({}) exceeds effective MTU ({}).",
            total_size,
            self.effective_mtu()
        );

        let mut tx_buf = [0u8; MAX_SUPPORTED_MTU];
        let _ = AttHandleValueIndHeaderWriter::new(&mut tx_buf[..ATT_HANDLE_VALUE_IND_HEADER_SIZE])
            .check_complete()
            .expect("valid buffer")
            .write_attribute_opcode(Opcode::ATT_HANDLE_VALUE_IND)
            .write_attribute_handle(handle);
        tx_buf[ATT_HANDLE_VALUE_IND_HEADER_SIZE..total_size].copy_from_slice(value);

        let tx_packet = Packet::try_ref_from_bytes(&tx_buf[..total_size]).expect("valid packet");
        match self.bearer_tx.send(tx_packet).await {
            Ok(()) => {}
            Err(BearerSendError::LinkClosed) => return Err(ServerEventError::LinkClosed),
            Err(BearerSendError::PacketTooLarge) => {
                panic!("Programming error: outgoing packet size exceeds the negotiated MTU.");
            }
        }

        let mut cfm_buf = [MaybeUninit::uninit(); ATT_HANDLE_VALUE_CFM_SIZE];
        let cfm_packet = match self.cfm_rx_handle.next_packet(&mut cfm_buf).await {
            Ok(p) => p,
            Err(BearerRecvError::LinkClosed) => return Err(ServerEventError::LinkClosed),
            Err(_) => return Err(ServerEventError::InvalidConfirmation),
        };

        let header = AttHeader::new(cfm_packet.as_bytes());
        if header.attribute_opcode().try_read() != Ok(Opcode::ATT_HANDLE_VALUE_CFM) {
            return Err(ServerEventError::InvalidConfirmation);
        }

        Ok(())
    }
}

/// Converts a raw 16-bit value into a valid `AttributeHandle`.
///
/// If `val` is invalid (i.e. `0x0000`), returns an ATT Error Response with
/// `ErrorCode::INVALID_HANDLE` for the given request `opcode`.
///
/// see Bluetooth Core Spec v6.0 (Vol 3, Part F, Section 3.2.2).
fn to_handle(val: u16, opcode: Opcode) -> Result<AttributeHandle, TransactionError> {
    AttributeHandle::try_from(val).map_err(|_| TransactionError::ErrorResponse {
        request_opcode: opcode,
        attribute_handle: val,
        error_code: ErrorCode::INVALID_HANDLE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::att::attribute::testing::MockAttribute;
    use crate::att::bearer::BearerRx;
    use crate::att::database::testing::MockDb;
    use crate::att::l2cap::mock::setup_mock_channel;
    use crate::att::pdu::ReadByGroupTypeResults;
    use crate::att::router::{BearerRouter, RouteFilter};
    use sapphire_async::executor::BoundedExecutor;
    use sapphire_async::testing::TestExecutor;
    use sapphire_collections::storage::ArrayStorage;
    use sapphire_emboss::att::{
        AttErrorRsp, AttExchangeMtuReqMut, AttExchangeMtuRsp, AttExecuteWriteReqMut,
        AttFindByTypeValueReqHeaderMut, AttFindInformationReqMut, AttFindInformationRspHeader,
        AttHandleValueIndHeader, AttHandleValueNtfHeader, AttHandlesInformation,
        AttInformationData16, AttPrepareWriteHeaderMut, AttReadBlobReqMut,
        AttReadByGroupTypeReqHeaderMut, AttReadByTypeReqHeaderMut, AttReadReqMut, AttWriteCmdMut,
    };

    fn h(val: u16) -> AttributeHandle {
        AttributeHandle::try_from(val).unwrap()
    }

    const CLIENT_PREFERRED_MTU: u16 = 512;
    const SERVER_MTU: u16 = 256;
    const TEST_RX_BUF_SIZE: usize = 64;
    const TEST_ARENA_SIZE: usize = 1024;

    fn new_server<Tx, R, DB>(
        peer_id: PeerId,
        bearer_tx: BearerTx<Tx>,
        bearer_rx: R,
        server_rx_mtu: u16,
        database: DB,
    ) -> Server<Tx, R, DB, ArrayStorage<TEST_ARENA_SIZE>>
    where
        Tx: L2CapChannelTx,
        R: AttReceiver,
        DB: Database,
    {
        Server::new(peer_id, bearer_tx, bearer_rx, server_rx_mtu, database, PrepareQueue::new())
    }

    #[test]
    fn test_server_handle_mtu_exchange_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            // Spawn client driver task
            let client_handle = executor.spawn(async move {
                // Send ExchangeMtuReq requesting 512-byte MTU
                let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
                let mut view = AttExchangeMtuReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXCHANGE_MTU_REQ).unwrap();
                view.client_rx_mtu().try_write(CLIENT_PREFERRED_MTU).unwrap();

                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Receive ExchangeMtuRsp from server
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttExchangeMtuRsp::new(packet.as_bytes());
                assert_eq!(
                    rsp.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_RSP
                );
                assert_eq!(rsp.server_rx_mtu().try_read().unwrap(), SERVER_MTU);
            });

            // Spawn server driver task
            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
                assert_eq!(server.mtu(), SERVER_MTU);
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handles_unsupported_request_and_continues() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Send ExchangeMtuRsp (0x03) as a request (valid opcode but unsupported request)
                let mut rsp_buf = [0u8; ATT_EXCHANGE_MTU_RSP_SIZE];
                let _ = AttExchangeMtuRspWriter::new(&mut rsp_buf[..])
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_EXCHANGE_MTU_RSP)
                    .write_server_rx_mtu(SERVER_MTU);
                let tx_packet = Packet::try_ref_from_bytes(&rsp_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ErrorRsp indicating RequestNotSupported (0x06) for ExchangeMtuRsp (0x03)
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_EXCHANGE_MTU_RSP)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::REQUEST_NOT_SUPPORTED);

                // 2. Server should still be running! Send valid ExchangeMtuReq (0x02)
                let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
                let mut view = AttExchangeMtuReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXCHANGE_MTU_REQ).unwrap();
                view.client_rx_mtu().try_write(CLIENT_PREFERRED_MTU).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ExchangeMtuRsp from server
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttExchangeMtuRsp::new(packet.as_bytes());
                assert_eq!(
                    rsp.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_RSP
                );
                assert_eq!(rsp.server_rx_mtu().try_read().unwrap(), SERVER_MTU);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handles_invalid_payload_and_continues() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Send ExchangeMtuReq but with truncated (empty) payload
                let tx_packet =
                    Packet::try_ref_from_bytes(&[Opcode::ATT_EXCHANGE_MTU_REQ as u8]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ErrorRsp indicating InvalidPdu (0x04) for ExchangeMtuReq (0x02)
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_EXCHANGE_MTU_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_PDU);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handles_large_packet_and_continues() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut sender = app_channel.sender;

                // 1. Bypass BearerTx and send a packet larger than MAX_SUPPORTED_MTU (519) directly over L2CAP
                let large_packet = [0u8; 600];
                sender.send(&large_packet).await.unwrap();

                // Expect ErrorRsp indicating InvalidPdu for opcode 0x00
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(packet.as_bytes()[1], 0x00);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_PDU);

                // 2. Server should log the error and stay alive. Send a valid ExchangeMtuReq.
                let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
                let mut view = AttExchangeMtuReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXCHANGE_MTU_REQ).unwrap();
                view.client_rx_mtu().try_write(CLIENT_PREFERRED_MTU).unwrap();
                sender.send(&req_buf[..]).await.unwrap();

                // Expect ExchangeMtuRsp
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttExchangeMtuRsp::new(packet.as_bytes());
                assert_eq!(
                    rsp.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_RSP
                );
                assert_eq!(rsp.server_rx_mtu().try_read().unwrap(), SERVER_MTU);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handles_invalid_opcode() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 32];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut sender = app_channel.sender;

                // Send a packet with raw invalid opcode 0x99
                sender.send(&[0x99, 0x01, 0x02]).await.unwrap();

                // Expect ErrorRsp indicating RequestNotSupported for request opcode 0x99
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(packet.as_bytes()[1], 0x99);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::REQUEST_NOT_SUPPORTED);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handles_exceeding_mtu_packet() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut sender = app_channel.sender;

                // Default MTU is 23. Send a 30-byte packet directly over L2CAP.
                let mut oversized = [0u8; 30];
                oversized[0] = Opcode::ATT_EXCHANGE_MTU_REQ.into();
                sender.send(&oversized).await.unwrap();

                // Expect ErrorRsp indicating InvalidPdu for request opcode 0x02
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_EXCHANGE_MTU_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_PDU);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_find_information_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"); // handle 1
            let custom_uuid =
                Uuid::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            let custom_attr = MockAttribute::new(custom_uuid, b"Custom"); // handle 2
            db.insert(h(1), name_attr);
            db.insert(h(2), custom_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Send FindInformationReq for 1..=2
                let mut req_buf = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                let mut view = AttFindInformationReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(2).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect FindInformationRsp with only handle 1 (since handle 2 has a different format)
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_INFORMATION_RSP.into());

                let header = AttFindInformationRspHeader::new(packet.as_bytes());
                assert_eq!(header.format().try_read().unwrap(), UuidFormat::BIT16);
                let entry = AttInformationData16::new(&packet.data[1..]);
                assert_eq!(entry.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(entry.uuid().try_read().unwrap(), 0x2A00);

                // 2. Send FindInformationReq for 1..=0xFFFF (querying past end of database)
                let mut req_buf2 = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                let mut view2 = AttFindInformationReqMut::new(&mut req_buf2[..]);
                view2.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                view2.starting_handle().try_write(1).unwrap();
                view2.ending_handle().try_write(0xFFFF).unwrap();
                let tx_packet2 = Packet::try_ref_from_bytes(&req_buf2[..]).unwrap();
                client_tx_bearer.send(tx_packet2).await.unwrap();

                // Expect the same FindInformationRsp containing only handle 1
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_INFORMATION_RSP.into());

                let header = AttFindInformationRspHeader::new(packet.as_bytes());
                assert_eq!(header.format().try_read().unwrap(), UuidFormat::BIT16);
                let entry = AttInformationData16::new(&packet.data[1..]);
                assert_eq!(entry.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(entry.uuid().try_read().unwrap(), 0x2A00);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_find_information_errors() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"); // handle 1
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Invalid Handle (start = 0)
                let mut req_buf = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                let mut view = AttFindInformationReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                view.starting_handle().try_write(0).unwrap();
                view.ending_handle().try_write(2).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_FIND_INFORMATION_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 0);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);

                // 2. Invalid Handle (start > end)
                let mut req_buf2 = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                let mut view2 = AttFindInformationReqMut::new(&mut req_buf2[..]);
                view2.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                view2.starting_handle().try_write(3).unwrap();
                view2.ending_handle().try_write(2).unwrap();
                let tx_packet2 = Packet::try_ref_from_bytes(&req_buf2[..]).unwrap();
                client_tx_bearer.send(tx_packet2).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_FIND_INFORMATION_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 3);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);

                // 3. Attribute Not Found (no attributes in 5..=10)
                let mut req_buf3 = [0u8; ATT_FIND_INFORMATION_REQ_SIZE];
                let mut view3 = AttFindInformationReqMut::new(&mut req_buf3[..]);
                view3.attribute_opcode().try_write(Opcode::ATT_FIND_INFORMATION_REQ).unwrap();
                view3.starting_handle().try_write(5).unwrap();
                view3.ending_handle().try_write(10).unwrap();
                let tx_packet3 = Packet::try_ref_from_bytes(&req_buf3[..]).unwrap();
                client_tx_bearer.send(tx_packet3).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_FIND_INFORMATION_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 5);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::ATTRIBUTE_NOT_FOUND);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();

            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_find_by_type_value_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            // Handle 1: Primary Service (0x2800) with value 0x180D (Heart Rate), ends at 5
            let svc_attr = MockAttribute::new_grouped(Uuid::from_u16(0x2800), &[0x0D, 0x18], 5);
            // Handle 6: Primary Service (0x2800) with value 0x180F (Battery Service), ends at 8
            let svc_attr2 = MockAttribute::new_grouped(Uuid::from_u16(0x2800), &[0x0F, 0x18], 8);
            db.insert(h(1), svc_attr);
            db.insert(h(6), svc_attr2);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send request for 0x2800 with value 0x180D
                const VAL: &[u8] = &[0x0D, 0x18];
                let mut req_buf = [0u8; ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE + VAL.len()];
                let mut view = AttFindByTypeValueReqHeaderMut::new(
                    &mut req_buf[..ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE],
                );
                view.attribute_opcode().try_write(Opcode::ATT_FIND_BY_TYPE_VALUE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                view.attribute_type().try_write(0x2800).unwrap();
                req_buf[ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect FindByTypeValueRsp with entry [1, 5]
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_FIND_BY_TYPE_VALUE_RSP.into());
                let entry = AttHandlesInformation::new(&packet.data[..]);
                assert_eq!(entry.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(entry.group_end_handle().try_read().unwrap(), 5);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_find_by_type_value_errors() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let svc_attr = MockAttribute::new_grouped(Uuid::from_u16(0x2800), &[0x0D, 0x18], 5);
            db.insert(h(1), svc_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Attribute Not Found (value mismatch 0x180F)
                const VAL: &[u8] = &[0x0F, 0x18];
                let mut req_buf = [0u8; ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE + VAL.len()];
                let mut view = AttFindByTypeValueReqHeaderMut::new(
                    &mut req_buf[..ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE],
                );
                view.attribute_opcode().try_write(Opcode::ATT_FIND_BY_TYPE_VALUE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                view.attribute_type().try_write(0x2800).unwrap();
                req_buf[ATT_FIND_BY_TYPE_VALUE_REQ_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_FIND_BY_TYPE_VALUE_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::ATTRIBUTE_NOT_FOUND);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send ReadReq for handle 1
                let mut req_buf = [0u8; ATT_READ_REQ_SIZE];
                let mut view = AttReadReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_REQ).unwrap();
                view.attribute_handle().try_write(1).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ReadRsp containing "Sunstone"
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_RSP.into());
                let expected: &[u8] = b"Sunstone";
                assert_eq!(packet.data, *expected);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_invalid_handle() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); TEST_RX_BUF_SIZE];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Read Request for handle 0 (invalid handle value)
                let mut req_buf = [0u8; ATT_READ_REQ_SIZE];
                let mut view = AttReadReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_REQ).unwrap();
                view.attribute_handle().try_write(0).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ErrorRsp indicating InvalidHandle for handle 0
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 0);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_attribute_not_found() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); TEST_RX_BUF_SIZE];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Read Request for handle 99 (non-existent handle)
                let mut req_buf = [0u8; ATT_READ_REQ_SIZE];
                let mut view = AttReadReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_REQ).unwrap();
                view.attribute_handle().try_write(99).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ErrorRsp indicating InvalidHandle for handle 99
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 99);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_truncated() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            // 30-byte long value
            let long_val = b"012345678901234567890123456789";
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            db.insert(h(1), name_attr);

            // Set server MTU to 23 bytes
            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                23,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send ReadReq for handle 1
                let mut req_buf = [0u8; ATT_READ_REQ_SIZE];
                let mut view = AttReadReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_REQ).unwrap();
                view.attribute_handle().try_write(1).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ReadRsp containing first 22 bytes of long_val (MTU - 1)
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_RSP.into());
                let expected: &[u8] = b"0123456789012345678901";
                assert_eq!(packet.data, *expected);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_blob_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send ReadBlobReq for handle 1, offset 3
                let mut req_buf = [0u8; ATT_READ_BLOB_REQ_SIZE];
                let mut view = AttReadBlobReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_BLOB_REQ).unwrap();
                view.attribute_handle().try_write(1).unwrap();
                view.value_offset().try_write(3).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ReadBlobRsp containing "stone"
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BLOB_RSP.into());
                let expected: &[u8] = b"stone";
                assert_eq!(packet.data, *expected);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_blob_invalid_offset() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone");
            db.insert(h(1), name_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send ReadBlobReq for handle 1, offset 10 (length of "Sunstone" is 8)
                let mut req_buf = [0u8; ATT_READ_BLOB_REQ_SIZE];
                let mut view = AttReadBlobReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_BLOB_REQ).unwrap();
                view.attribute_handle().try_write(1).unwrap();
                view.value_offset().try_write(10).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ErrorRsp indicating InvalidOffset
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_BLOB_REQ)
                );
                assert_eq!(err.attribute_handle().try_read().unwrap(), 1);
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_OFFSET);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_blob_truncated() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            // 30-byte long value
            let long_val = b"012345678901234567890123456789";
            let name_attr = MockAttribute::new(Uuid::from_u16(0x2A00), long_val);
            db.insert(h(1), name_attr);

            // Set server MTU to 23 bytes
            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                23,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Send ReadBlobReq for handle 1, offset 5
                let mut req_buf = [0u8; ATT_READ_BLOB_REQ_SIZE];
                let mut view = AttReadBlobReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_BLOB_REQ).unwrap();
                view.attribute_handle().try_write(1).unwrap();
                view.value_offset().try_write(5).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ReadBlobRsp containing first 22 bytes starting from offset 5
                // long_val[5..] is "5678901234567890123456789"
                // 22 bytes from offset 5: "5678901234567890123456"
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BLOB_RSP.into());
                let expected: &[u8] = b"5678901234567890123456";
                assert_eq!(packet.data, *expected);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_by_type_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            db.insert(h(2), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"));
            db.insert(h(4), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sapphire"));
            db.insert(h(6), MockAttribute::new(Uuid::from_u16(0x2A00), b"Gatt")); // different length!

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                let uuid = Uuid::from_u16(0x2A00);
                let mut tx_buf = [0u8; 64];
                let mut view =
                    AttReadByTypeReqHeaderMut::new(&mut tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_BY_TYPE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                tx_buf[ATT_READ_BY_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_TYPE_RSP.into());

                const VALUE_SIZE: usize = 8;
                const ENTRY_SIZE: u8 = (size_of::<AttributeHandle>() + VALUE_SIZE) as u8;

                assert_eq!(packet.data[0], ENTRY_SIZE);

                // Entry 1 (handle 2, "Sunstone")
                let h1_val = u16::from_le_bytes([packet.data[1], packet.data[2]]);
                assert_eq!(h1_val, 2);
                assert_eq!(&packet.data[3..11], b"Sunstone");

                // Entry 2 (handle 4, "Sapphire")
                let h2_val = u16::from_le_bytes([packet.data[11], packet.data[12]]);
                assert_eq!(h2_val, 4);
                assert_eq!(&packet.data[13..21], b"Sapphire");

                assert_eq!(packet.data.len(), 1 + ENTRY_SIZE as usize * 2);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_by_type_errors() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            db.insert(h(2), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Query for non-existent UUID 0x2A01
                let uuid = Uuid::from_u16(0x2A01);
                let mut tx_buf = [0u8; 64];
                let mut view =
                    AttReadByTypeReqHeaderMut::new(&mut tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_READ_BY_TYPE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                tx_buf[ATT_READ_BY_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_BY_TYPE_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::ATTRIBUTE_NOT_FOUND);

                // 2. Query with invalid range (start > end)
                let uuid = Uuid::from_u16(0x2A00);
                let mut tx_buf = [0u8; 64];
                let mut view2 =
                    AttReadByTypeReqHeaderMut::new(&mut tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE]);
                view2.attribute_opcode().try_write(Opcode::ATT_READ_BY_TYPE_REQ).unwrap();
                view2.starting_handle().try_write(10).unwrap();
                view2.ending_handle().try_write(5).unwrap();
                tx_buf[ATT_READ_BY_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet2 = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet2).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_BY_TYPE_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_by_group_type_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            db.insert(h(2), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"Service1", 5));
            db.insert(h(6), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"Service2", 10));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                const NEGOTIATED_MTU: u16 = 64;
                let mut rx_buf = [MaybeUninit::uninit(); NEGOTIATED_MTU as usize];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. MTU Exchange (negotiate NEGOTIATED_MTU bytes)
                let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
                let mut view = AttExchangeMtuReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXCHANGE_MTU_REQ).unwrap();
                view.client_rx_mtu().try_write(NEGOTIATED_MTU).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttExchangeMtuRsp::new(packet.as_bytes());
                assert_eq!(
                    rsp.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_RSP
                );
                client_rx_bearer.set_mtu(NEGOTIATED_MTU);
                client_tx_bearer.set_mtu(NEGOTIATED_MTU);

                // 2. Read By Group Type Request
                let uuid = Uuid::from_u16(0x2800);
                let mut tx_buf = [0u8; NEGOTIATED_MTU as usize];
                let mut view = AttReadByGroupTypeReqHeaderMut::new(
                    &mut tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE],
                );
                view.attribute_opcode().try_write(Opcode::ATT_READ_BY_GROUP_TYPE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                tx_buf[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_GROUP_TYPE_RSP.into());

                // Parse the response using the client results parser and verify the returned
                // attribute group data list entries.
                let length = usize::from(packet.data[0]);
                let results = ReadByGroupTypeResults::new(length, &packet.data[1..])
                    .expect("Server response should be a valid Read By Group Type response packet");
                let mut iter = results.iter();
                let (e1, val1) = iter.next().unwrap();
                assert_eq!(e1.attribute_handle().try_read().unwrap(), 2);
                assert_eq!(e1.end_group_handle().try_read().unwrap(), 5);
                assert_eq!(val1, b"Service1");
                let (e2, val2) = iter.next().unwrap();
                assert_eq!(e2.attribute_handle().try_read().unwrap(), 6);
                assert_eq!(e2.end_group_handle().try_read().unwrap(), 10);
                assert_eq!(val2, b"Service2");
                assert!(iter.next().is_none());
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_by_group_type_errors() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            // Match but non-grouping type! (returns None for group_end_handle)
            db.insert(h(2), MockAttribute::new(Uuid::from_u16(0x2A00), b"Sunstone"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); 64];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Query for grouping type 0x2A00 which contains a non-grouping attribute
                let uuid = Uuid::from_u16(0x2A00);
                let mut tx_buf = [0u8; 64];
                let mut view = AttReadByGroupTypeReqHeaderMut::new(
                    &mut tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE],
                );
                view.attribute_opcode().try_write(Opcode::ATT_READ_BY_GROUP_TYPE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                tx_buf[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let err = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    err.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_READ_BY_GROUP_TYPE_REQ)
                );
                assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::UNSUPPORTED_GROUP_TYPE);
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_read_by_group_type_mixed_lengths() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            // Attribute 1 has value of length 8
            db.insert(h(2), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"Service1", 5));
            // Attribute 2 has value of length 11 (different!)
            db.insert(h(6), MockAttribute::new_grouped(Uuid::from_u16(0x2800), b"ServiceLong", 10));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                const NEGOTIATED_MTU: u16 = 64;
                let mut rx_buf = [MaybeUninit::uninit(); NEGOTIATED_MTU as usize];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. MTU Exchange
                let mut req_buf = [0u8; ATT_EXCHANGE_MTU_REQ_SIZE];
                let mut view = AttExchangeMtuReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXCHANGE_MTU_REQ).unwrap();
                view.client_rx_mtu().try_write(NEGOTIATED_MTU).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttExchangeMtuRsp::new(packet.as_bytes());
                assert_eq!(
                    rsp.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_EXCHANGE_MTU_RSP
                );
                client_rx_bearer.set_mtu(NEGOTIATED_MTU);
                client_tx_bearer.set_mtu(NEGOTIATED_MTU);

                // 2. Read By Group Type Request
                let uuid = Uuid::from_u16(0x2800);
                let mut tx_buf = [0u8; NEGOTIATED_MTU as usize];
                let mut view = AttReadByGroupTypeReqHeaderMut::new(
                    &mut tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE],
                );
                view.attribute_opcode().try_write(Opcode::ATT_READ_BY_GROUP_TYPE_REQ).unwrap();
                view.starting_handle().try_write(1).unwrap();
                view.ending_handle().try_write(10).unwrap();
                tx_buf[ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE
                    ..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()]
                    .copy_from_slice(uuid.as_bytes());
                let tx_packet = Packet::try_ref_from_bytes(
                    &tx_buf[..ATT_READ_BY_GROUP_TYPE_REQ_HEADER_SIZE + uuid.as_bytes().len()],
                )
                .unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_READ_BY_GROUP_TYPE_RSP.into());

                // Verify the response PDU using the client results parser.
                let length = usize::from(packet.data[0]);
                let results = ReadByGroupTypeResults::new(length, &packet.data[1..])
                    .expect("Server response should be a valid Read By Group Type response packet");
                let mut iter = results.iter();

                let (e1, val1) = iter.next().unwrap();
                assert_eq!(e1.attribute_handle().try_read().unwrap(), 2);
                assert_eq!(e1.end_group_handle().try_read().unwrap(), 5);
                assert_eq!(val1, b"Service1");

                assert!(iter.next().is_none());
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_write_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue");
            db.insert(h(10), attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); TEST_RX_BUF_SIZE];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Write Request for handle 10, value "Sunstone"
                const VAL: &[u8] = b"Sunstone";
                let mut req_buf = [0u8; ATT_WRITE_REQ_HEADER_SIZE + VAL.len()];
                let mut view = AttWriteCmdMut::new(&mut req_buf[..ATT_WRITE_REQ_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_WRITE_REQ).unwrap();
                view.attribute_handle().try_write(10).unwrap();
                req_buf[ATT_WRITE_REQ_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                client_tx_bearer.send(tx_packet).await.unwrap();

                // Expect WriteRsp
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_WRITE_RSP.into());
                assert!(packet.data.is_empty());
            });

            let mut server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
                server
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());

            // Verify value was written in database attribute
            let server = server_handle.get().unwrap();
            let db_attr = server.database.find_attribute(h(10)).unwrap();
            let mut check_buf = [0u8; 32];
            let read_len = executor.block_on(async {
                db_attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut check_buf).await.unwrap()
            });
            assert_eq!(&check_buf[..read_len], b"Sunstone");
        });
    }

    #[test]
    fn test_server_handle_write_errors() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"Val");
            attr.set_write_error(ErrorCode::WRITE_NOT_PERMITTED);
            db.insert(h(10), attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); TEST_RX_BUF_SIZE];
                let mut client_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut client_tx_bearer = BearerTx::new(app_channel.sender);

                // Write Request for non-existent handle 99
                {
                    const VAL: &[u8] = b"Value";
                    let mut req_buf = [0u8; ATT_WRITE_REQ_HEADER_SIZE + VAL.len()];
                    let mut view = AttWriteCmdMut::new(&mut req_buf[..ATT_WRITE_REQ_HEADER_SIZE]);
                    view.attribute_opcode().try_write(Opcode::ATT_WRITE_REQ).unwrap();
                    view.attribute_handle().try_write(99).unwrap();
                    req_buf[ATT_WRITE_REQ_HEADER_SIZE..].copy_from_slice(VAL);
                    let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                    client_tx_bearer.send(tx_packet).await.unwrap();

                    let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                    let err = AttErrorRsp::new(packet.as_bytes());
                    assert_eq!(err.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                    assert_eq!(
                        err.request_opcode_in_error_uint().try_read().unwrap(),
                        u8::from(Opcode::ATT_WRITE_REQ)
                    );
                    assert_eq!(err.attribute_handle().try_read().unwrap(), 99);
                    assert_eq!(err.error_code().try_read().unwrap(), ErrorCode::INVALID_HANDLE);
                }
            });

            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_write_cmd_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, server_tx, server_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            // Client driver task
            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);

                // Send Write Command
                const VAL: &[u8] = b"SunstoneCmd";
                let mut req_buf = [0u8; ATT_WRITE_CMD_HEADER_SIZE + VAL.len()];
                let mut view = AttWriteCmdMut::new(&mut req_buf[..ATT_WRITE_CMD_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_WRITE_CMD).unwrap();
                view.attribute_handle().try_write(10).unwrap();
                req_buf[ATT_WRITE_CMD_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                drop(app_tx_bearer);
            });

            // Server task
            let mut server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
                server
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());

            // Verify value was written in database attribute
            let server = server_handle.get().unwrap();
            let db_attr = server.database.find_attribute(h(10)).unwrap();
            let mut check_buf = [0u8; 32];
            let read_len = executor.block_on(async {
                db_attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut check_buf).await.unwrap()
            });
            assert_eq!(&check_buf[..read_len], b"SunstoneCmd");
        });
    }

    #[test]
    fn test_server_handle_write_cmd_errors_ignored() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, server_tx, server_rx) = setup_mock_channel();

            let mut db = MockDb::new();
            let readonly_attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue");
            readonly_attr.set_write_error(ErrorCode::WRITE_NOT_PERMITTED);
            db.insert(h(10), readonly_attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(server_tx),
                BearerRx::new(server_rx),
                SERVER_MTU,
                db,
            );

            // Client driver task
            let client_handle = executor.spawn(async move {
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);

                // 1. Invalid handle (99)
                const VAL: &[u8] = b"Value";
                let mut req_buf = [0u8; ATT_WRITE_CMD_HEADER_SIZE + VAL.len()];
                let mut view = AttWriteCmdMut::new(&mut req_buf[..ATT_WRITE_CMD_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_WRITE_CMD).unwrap();
                view.attribute_handle().try_write(99).unwrap();
                req_buf[ATT_WRITE_CMD_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                // 2. Read-only handle (10)
                let mut req_buf2 = [0u8; ATT_WRITE_CMD_HEADER_SIZE + VAL.len()];
                let mut view2 = AttWriteCmdMut::new(&mut req_buf2[..ATT_WRITE_CMD_HEADER_SIZE]);
                view2.attribute_opcode().try_write(Opcode::ATT_WRITE_CMD).unwrap();
                view2.attribute_handle().try_write(10).unwrap();
                req_buf2[ATT_WRITE_CMD_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet2 = Packet::try_ref_from_bytes(&req_buf2[..]).unwrap();
                app_tx_bearer.send(tx_packet2).await.unwrap();

                // 3. Malformed payload
                let malformed_buf = [Opcode::ATT_WRITE_CMD as u8, 0u8];
                let tx_packet3 = Packet::try_ref_from_bytes(&malformed_buf).unwrap();
                app_tx_bearer.send(tx_packet3).await.unwrap();

                drop(app_tx_bearer);

                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let result = app_rx_bearer.next_packet(&mut rx_buf).await;
                assert_eq!(result.err(), Some(BearerRecvError::LinkClosed));
            });

            // Server task
            let server_handle = executor.spawn(async move {
                let res = server.run().await;
                assert_eq!(res, Err(ServerError::LinkClosed));
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_prepare_write_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                // Send PrepareWriteReq
                const VAL: &[u8] = b"Part1";
                let mut req_buf = [0u8; ATT_PREPARE_WRITE_HEADER_SIZE + VAL.len()];
                let mut view =
                    AttPrepareWriteHeaderMut::new(&mut req_buf[..ATT_PREPARE_WRITE_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_PREPARE_WRITE_REQ).unwrap();
                view.attribute_handle().try_write(10).unwrap();
                view.value_offset().try_write(0).unwrap();
                req_buf[ATT_PREPARE_WRITE_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                // Read response
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_PREPARE_WRITE_RSP.into());
                let rsp = AttPrepareWriteHeader::new(packet.as_bytes());
                assert_eq!(rsp.attribute_handle().try_read().unwrap(), 10);
                assert_eq!(rsp.value_offset().try_read().unwrap(), 0);
                assert_eq!(&packet.as_bytes()[ATT_PREPARE_WRITE_HEADER_SIZE..], b"Part1");
            });

            let server_handle = executor.spawn(async move {
                server.handle_request().await.unwrap();
                let mut drain = server.prepare_storage.drain();
                let req = drain.next().unwrap();
                assert_eq!(req.attribute_handle, 10);
                assert_eq!(req.value_offset, 0);
                assert_eq!(req.part_attribute_value, b"Part1");
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_prepare_write_invalid_offset() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue")); // Length 12

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                // Send PrepareWriteReq with invalid offset 13 (exceeds value length 12)
                const VAL: &[u8] = b"Part1";
                let mut req_buf = [0u8; ATT_PREPARE_WRITE_HEADER_SIZE + VAL.len()];
                let mut view =
                    AttPrepareWriteHeaderMut::new(&mut req_buf[..ATT_PREPARE_WRITE_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_PREPARE_WRITE_REQ).unwrap();
                view.attribute_handle().try_write(10).unwrap();
                view.value_offset().try_write(13).unwrap();
                req_buf[ATT_PREPARE_WRITE_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                // Read response (should be ErrorRsp with InvalidOffset)
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(rsp.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(
                    rsp.request_opcode_in_error_uint().try_read().unwrap(),
                    u8::from(Opcode::ATT_PREPARE_WRITE_REQ)
                );
                assert_eq!(rsp.attribute_handle().try_read().unwrap(), 10);
                assert_eq!(rsp.error_code().try_read().unwrap(), ErrorCode::INVALID_OFFSET);
            });

            let server_handle = executor.spawn(async move {
                let _ = server.handle_request().await;
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_prepare_write_queue_full() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            // Manually fill prepare_queue to max capacity
            while server.prepare_storage.try_push(10, 0, &[]).is_ok() {}

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                // Send another PrepareWriteReq
                const VAL: &[u8] = b"Overflow";
                let mut req_buf = [0u8; ATT_PREPARE_WRITE_HEADER_SIZE + VAL.len()];
                let mut view =
                    AttPrepareWriteHeaderMut::new(&mut req_buf[..ATT_PREPARE_WRITE_HEADER_SIZE]);
                view.attribute_opcode().try_write(Opcode::ATT_PREPARE_WRITE_REQ).unwrap();
                view.attribute_handle().try_write(10).unwrap();
                view.value_offset().try_write(0).unwrap();
                req_buf[ATT_PREPARE_WRITE_HEADER_SIZE..].copy_from_slice(VAL);
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                // Read response (should be ErrorRsp with PrepareQueueFull)
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(rsp.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(rsp.error_code().try_read().unwrap(), ErrorCode::PREPARE_QUEUE_FULL);
            });

            let server_handle = executor.spawn(async move {
                let _ = server.handle_request().await;
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_execute_write_commit_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            // Queue up two write chunks
            server.prepare_storage.try_push(10, 0, b"Hello").unwrap();
            server.prepare_storage.try_push(10, 5, b"World").unwrap();

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                // Send ExecuteWriteReq with WriteAll flag
                let mut req_buf = [0u8; ATT_EXECUTE_WRITE_REQ_SIZE];
                let mut view = AttExecuteWriteReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXECUTE_WRITE_REQ).unwrap();
                view.flags().try_write(ExecuteWriteFlags::WRITE).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                // Expect ExecuteWriteRsp
                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_EXECUTE_WRITE_RSP.into());
            });

            let server_handle = executor.spawn(async move {
                server.handle_request().await.unwrap();
                assert!(server.prepare_storage.is_empty());

                // Check updated db value
                let attr = server.database.find_attribute(h(10)).unwrap();
                let mut val_buf = [0u8; 32];
                let len = attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut val_buf).await.unwrap();
                assert_eq!(&val_buf[..len], b"HelloWorld");
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_execute_write_cancel_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            db.insert(h(10), MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue"));

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            // Queue up a write
            server.prepare_storage.try_push(10, 0, b"Hello").unwrap();

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                // Send ExecuteWriteReq with CancelAll flag
                let mut req_buf = [0u8; ATT_EXECUTE_WRITE_REQ_SIZE];
                let mut view = AttExecuteWriteReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXECUTE_WRITE_REQ).unwrap();
                view.flags().try_write(ExecuteWriteFlags::CANCEL).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_EXECUTE_WRITE_RSP.into());
            });

            let server_handle = executor.spawn(async move {
                server.handle_request().await.unwrap();
                assert!(server.prepare_storage.is_empty());

                // Db value should remain unmodified
                let attr = server.database.find_attribute(h(10)).unwrap();
                let mut val_buf = [0u8; 32];
                let len = attr.read_chunk(PeerId::new(1).unwrap(), 0, &mut val_buf).await.unwrap();
                assert_eq!(&val_buf[..len], b"InitialValue");
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_handle_execute_write_failure_discards_rest() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, test_tx, test_rx) = setup_mock_channel();
            let mut db = MockDb::new();
            let attr = MockAttribute::new(Uuid::from_u16(0x2A00), b"InitialValue");
            attr.set_write_error(ErrorCode::WRITE_NOT_PERMITTED);
            db.insert(h(10), attr);

            let mut server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(test_tx),
                BearerRx::new(test_rx),
                SERVER_MTU,
                db,
            );

            // Queue up a write
            server.prepare_storage.try_push(10, 0, b"Hello").unwrap();

            let client_handle = executor.spawn(async move {
                let mut app_tx_bearer = BearerTx::new(app_channel.sender);
                let mut app_rx_bearer = BearerRx::new(app_channel.receiver);

                let mut req_buf = [0u8; ATT_EXECUTE_WRITE_REQ_SIZE];
                let mut view = AttExecuteWriteReqMut::new(&mut req_buf[..]);
                view.attribute_opcode().try_write(Opcode::ATT_EXECUTE_WRITE_REQ).unwrap();
                view.flags().try_write(ExecuteWriteFlags::WRITE).unwrap();
                let tx_packet = Packet::try_ref_from_bytes(&req_buf[..]).unwrap();
                app_tx_bearer.send(tx_packet).await.unwrap();

                let mut rx_buf = [MaybeUninit::uninit(); 128];
                let packet = app_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                let rsp = AttErrorRsp::new(packet.as_bytes());
                assert_eq!(rsp.attribute_opcode().try_read().unwrap(), Opcode::ATT_ERROR_RSP);
                assert_eq!(rsp.error_code().try_read().unwrap(), ErrorCode::WRITE_NOT_PERMITTED);
                assert_eq!(rsp.attribute_handle().try_read().unwrap(), 10);
            });

            let server_handle = executor.spawn(async move {
                let _ = server.handle_request().await;
                // Verify that the queue has been discarded and cleared on failure
                assert!(server.prepare_storage.is_empty());
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }

    #[test]
    fn test_server_notify_success() {
        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let (app_channel, _test_tx, mut test_rx) = setup_mock_channel();
            use crate::att::l2cap::L2CapChannelRx;
            let server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(app_channel.sender),
                BearerRx::new(app_channel.receiver),
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
                let sdu = test_rx.recv(&mut rx_buf).await.unwrap();
                let packet = Packet::try_ref_from_bytes(sdu).unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_HANDLE_VALUE_NTF.into());
                let ntf = AttHandleValueNtfHeader::new(packet.as_bytes());
                assert_eq!(
                    ntf.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_HANDLE_VALUE_NTF
                );
                assert_eq!(ntf.attribute_handle().try_read().unwrap(), 0x1234);
                assert_eq!(
                    &packet.as_bytes()[ATT_HANDLE_VALUE_NTF_HEADER_SIZE..],
                    &[0xAA, 0xBB, 0xCC]
                );
            });

            executor.block_on(async {
                let mut notifier = server.notifier();
                notifier.notify(0x1234, &[0xAA, 0xBB, 0xCC]).await.unwrap();
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
        });
    }

    #[test]
    fn test_server_indicate_success() {
        let (app_channel, test_tx, test_rx) = setup_mock_channel();
        let router = BearerRouter::<_>::new(app_channel.receiver);
        let req_handle = router.route_to(RouteFilter::Requests).unwrap();
        let cfm_handle = router.route_to(RouteFilter::Confirmations).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |executor| {
            let mut client_tx_bearer = BearerTx::new(test_tx);
            let mut client_rx_bearer = BearerRx::new(test_rx);

            let server = new_server(
                PeerId::new(1).unwrap(),
                BearerTx::new(app_channel.sender),
                req_handle,
                SERVER_MTU,
                MockDb::new(),
            );

            let client_handle = executor.spawn(async move {
                let mut rx_buf = [MaybeUninit::uninit(); MAX_SUPPORTED_MTU];
                let packet = client_rx_bearer.next_packet(&mut rx_buf).await.unwrap();
                assert_eq!(packet.opcode, Opcode::ATT_HANDLE_VALUE_IND.into());
                let ind = AttHandleValueIndHeader::new(packet.as_bytes());
                assert_eq!(
                    ind.attribute_opcode().try_read().unwrap(),
                    Opcode::ATT_HANDLE_VALUE_IND
                );
                assert_eq!(ind.attribute_handle().try_read().unwrap(), 0x5678);
                assert_eq!(&packet.as_bytes()[ATT_HANDLE_VALUE_IND_HEADER_SIZE..], &[0x11, 0x22]);

                // Respond with HandleValueCfm
                let mut cfm_buf = [0u8; ATT_HANDLE_VALUE_CFM_SIZE];
                let _ = AttHeaderWriter::new(&mut cfm_buf)
                    .check_complete()
                    .unwrap()
                    .write_attribute_opcode(Opcode::ATT_HANDLE_VALUE_CFM);
                let tx_packet = Packet::try_ref_from_bytes(&cfm_buf).unwrap();
                let _ = client_tx_bearer.send(tx_packet).await;
            });

            let server_handle = executor.spawn(async move {
                let mut indicator = server.indicator(cfm_handle);
                indicator.indicate(0x5678, &[0x11, 0x22]).await.unwrap();
            });

            executor.run_until_stalled();
            assert!(client_handle.is_finished());
            assert!(server_handle.is_finished());
        });
    }
}
