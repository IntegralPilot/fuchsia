// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_bap::types::BroadcastId;
use bt_common::core::ltv::LtValue;
use bt_common::core::{AddressType, AdvertisingSetId, PeriodicAdvertisingInterval};
use bt_common::generic_audio::metadata_ltv::*;
use bt_common::packet_encoding::{Decodable, Encodable, Error as PacketError};
use bt_common::{decodable_enum, Uuid};
use std::str::FromStr;

pub const ADDRESS_BYTE_SIZE: usize = 6;
const NUM_SUBGROUPS_BYTE_SIZE: usize = 1;
const PA_SYNC_BYTE_SIZE: usize = 1;
const SOURCE_ID_BYTE_SIZE: usize = 1;

/// 16-bit UUID values for the Broadcast Audio Scan Service and its
/// characteristics.
pub const BROADCAST_AUDIO_SCAN_SERVICE_UUID: Uuid = Uuid::from_u16(0x184F);
pub const BROADCAST_AUDIO_SCAN_CONTROL_POINT_UUID: Uuid = Uuid::from_u16(0x2BC7);
pub const BROADCAST_RECEIVE_STATE_UUID: Uuid = Uuid::from_u16(0x2BC8);

pub type SourceId = u8;

/// Index into the vector of BIG subgroups. Valid value range is [0 to len of
/// BIG vector).
pub type SubgroupIndex = u8;

/// BIS index value of a particular BIS. Valid value range is [1 to len of BIS]
pub type BisIndex = u8;

decodable_enum! {
    pub enum ControlPointOpcode<u8, bt_common::packet_encoding::Error, OutOfRange> {
        RemoteScanStopped = 0x00,
        RemoteScanStarted = 0x01,
        AddSource = 0x02,
        ModifySource = 0x03,
        SetBroadcastCode = 0x04,
        RemoveSource = 0x05,
    }
}

/// Broadcast Audio Scan Control Point characteristic opcode as defined in
/// Broadcast Audio Scan Service spec v1.0 Section 3.1.
impl ControlPointOpcode {
    const BYTE_SIZE: usize = 1;
}

/// Trait for objects that represent a Broadcast Audio Scan Control Point
/// characteristic. When written by a client, the Broadcast Audio Scan Control
/// Point characteristic is defined as an 8-bit enumerated value, known as the
/// opcode, followed by zero or more parameter octets. The opcode represents the
/// operation that would be performed in the Broadcast Audio Scan Service
/// server. See BASS spec v1.0 Section 3.1 for details.
pub trait ControlPointOperation: Encodable<Error = PacketError> {
    // Returns the expected opcode for this operation.
    fn opcode() -> ControlPointOpcode;

    // Given the raw encoded value of the opcode, verifies it and returns the
    // equivalent ControlPointOpcode object.
    fn check_opcode(raw_value: u8) -> Result<ControlPointOpcode, PacketError> {
        let expected = Self::opcode();
        let got = ControlPointOpcode::try_from(raw_value)?;
        if got != expected {
            return Err(PacketError::InvalidParameter(format!(
                "got opcode {got:?}, expected {expected:?}"
            )));
        }
        Ok(got)
    }
}

/// See BASS spec v1.0 Section 3.1.1.2 for details.
#[derive(Debug, PartialEq)]
pub struct RemoteScanStoppedOperation;

impl ControlPointOperation for RemoteScanStoppedOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::RemoteScanStopped
    }
}

impl Decodable for RemoteScanStoppedOperation {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        const BYTE_SIZE: usize = ControlPointOpcode::BYTE_SIZE;
        if buf.len() < BYTE_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        (Self::check_opcode(buf[0]).map(|_| RemoteScanStoppedOperation), BYTE_SIZE)
    }
}

impl Encodable for RemoteScanStoppedOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }
        buf[0] = Self::opcode() as u8;
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        ControlPointOpcode::BYTE_SIZE
    }
}

/// See BASS spec v1.0 Section 3.1.1.3 for details.
#[derive(Debug, PartialEq)]
pub struct RemoteScanStartedOperation;

impl ControlPointOperation for RemoteScanStartedOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::RemoteScanStarted
    }
}

impl Decodable for RemoteScanStartedOperation {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        const BYTE_SIZE: usize = ControlPointOpcode::BYTE_SIZE;
        if buf.len() < BYTE_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        (Self::check_opcode(buf[0]).map(|_| RemoteScanStartedOperation), BYTE_SIZE)
    }
}

impl Encodable for RemoteScanStartedOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }
        buf[0] = Self::opcode() as u8;
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        ControlPointOpcode::BYTE_SIZE
    }
}

/// See BASS spec v1.0 Section 3.1.1.4 for details.
#[derive(Debug, PartialEq)]
pub struct AddSourceOperation {
    pub(crate) advertiser_address_type: AddressType,
    // Address in little endian.
    pub(crate) advertiser_address: [u8; ADDRESS_BYTE_SIZE],
    pub(crate) advertising_sid: AdvertisingSetId,
    pub(crate) broadcast_id: BroadcastId,
    pub(crate) pa_sync: PaSync,
    pub(crate) pa_interval: PeriodicAdvertisingInterval,
    pub(crate) subgroups: Vec<BigSubgroup>,
}

impl AddSourceOperation {
    const MIN_PACKET_SIZE: usize = ControlPointOpcode::BYTE_SIZE
        + AddressType::BYTE_SIZE
        + ADDRESS_BYTE_SIZE
        + AdvertisingSetId::BYTE_SIZE
        + BroadcastId::BYTE_SIZE
        + PA_SYNC_BYTE_SIZE
        + PeriodicAdvertisingInterval::BYTE_SIZE
        + NUM_SUBGROUPS_BYTE_SIZE;

    pub fn new(
        address_type: AddressType,
        advertiser_address: [u8; ADDRESS_BYTE_SIZE],
        advertising_sid: AdvertisingSetId,
        broadcast_id: BroadcastId,
        pa_sync: PaSync,
        pa_interval: PeriodicAdvertisingInterval,
        subgroups: Vec<BigSubgroup>,
    ) -> Self {
        AddSourceOperation {
            advertiser_address_type: address_type,
            advertiser_address,
            advertising_sid,
            broadcast_id: broadcast_id,
            pa_sync,
            pa_interval,
            subgroups,
        }
    }
}

impl ControlPointOperation for AddSourceOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::AddSource
    }
}

impl Decodable for AddSourceOperation {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let decode_fn = || {
            let _ = Self::check_opcode(buf[0])?;
            let advertiser_address_type = AddressType::try_from(buf[1])?;
            let mut advertiser_address = [0; ADDRESS_BYTE_SIZE];
            advertiser_address.clone_from_slice(&buf[2..8]);
            let advertising_sid = AdvertisingSetId::try_from(buf[8])?;
            let broadcast_id = BroadcastId::decode(&buf[9..12]).0?;
            let pa_sync = PaSync::try_from(buf[12])?;
            let pa_interval =
                PeriodicAdvertisingInterval(u16::from_le_bytes(buf[13..15].try_into().unwrap()));
            let num_subgroups = buf[15] as usize;
            let mut subgroups = Vec::new();

            let mut idx: usize = 16;
            for _i in 0..num_subgroups {
                if buf.len() <= idx {
                    return Err(PacketError::UnexpectedDataLength);
                }
                let (decoded, consumed) = BigSubgroup::decode(&buf[idx..]);
                subgroups.push(decoded?);
                idx += consumed;
            }
            Ok((
                Self {
                    advertiser_address_type,
                    advertiser_address,
                    advertising_sid,
                    broadcast_id,
                    pa_sync,
                    pa_interval,
                    subgroups,
                },
                idx,
            ))
        };

        match decode_fn() {
            Ok((result, consumed)) => (Ok(result), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for AddSourceOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = Self::opcode() as u8;
        buf[1] = self.advertiser_address_type as u8;
        buf[2..8].copy_from_slice(&self.advertiser_address);
        buf[8] = self.advertising_sid.value();
        self.broadcast_id.encode(&mut buf[9..12])?;
        buf[12] = u8::from(self.pa_sync);
        buf[13..15].copy_from_slice(&self.pa_interval.0.to_le_bytes());
        buf[15] = self
            .subgroups
            .len()
            .try_into()
            .map_err(|_| PacketError::InvalidParameter("Num_Subgroups".to_string()))?;
        let mut idx = 16;
        for s in &self.subgroups {
            s.encode(&mut buf[idx..])?;
            idx += s.encoded_len();
        }
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::MIN_PACKET_SIZE + self.subgroups.iter().fold(0, |acc, g| acc + g.encoded_len())
    }
}

/// See Broadcast Audio Scan Service spec v1.0 Section 3.1.1.5 for details.
#[derive(Debug, PartialEq)]
pub struct ModifySourceOperation {
    source_id: SourceId,
    pa_sync: PaSync,
    pa_interval: PeriodicAdvertisingInterval,
    subgroups: Vec<BigSubgroup>,
}

impl ModifySourceOperation {
    const MIN_PACKET_SIZE: usize = ControlPointOpcode::BYTE_SIZE
        + SOURCE_ID_BYTE_SIZE
        + PA_SYNC_BYTE_SIZE
        + PeriodicAdvertisingInterval::BYTE_SIZE
        + NUM_SUBGROUPS_BYTE_SIZE;

    pub fn new(
        source_id: SourceId,
        pa_sync: PaSync,
        pa_interval: PeriodicAdvertisingInterval,
        subgroups: Vec<BigSubgroup>,
    ) -> Self {
        ModifySourceOperation { source_id, pa_sync, pa_interval, subgroups }
    }
}

impl ControlPointOperation for ModifySourceOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::ModifySource
    }
}

impl Decodable for ModifySourceOperation {
    type Error = PacketError;

    // Min size includes Source_ID, PA_Sync, PA_Interval, and Num_Subgroups params.
    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        let decode_fn = || {
            let _ = Self::check_opcode(buf[0])?;
            let source_id = buf[1];
            let pa_sync = PaSync::try_from(buf[2])?;
            let pa_interval =
                PeriodicAdvertisingInterval(u16::from_le_bytes(buf[3..5].try_into().unwrap()));
            let num_subgroups = buf[5] as usize;
            let mut subgroups = Vec::new();

            let mut idx = 6;
            for _i in 0..num_subgroups {
                if buf.len() < idx + BigSubgroup::MIN_PACKET_SIZE {
                    return Err(PacketError::UnexpectedDataLength);
                }
                let decoded = BigSubgroup::decode(&buf[idx..]);
                subgroups.push(decoded.0?);
                idx += decoded.1;
            }
            Ok((Self { source_id, pa_sync, pa_interval, subgroups }, idx))
        };

        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for ModifySourceOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = Self::opcode() as u8;
        buf[1] = self.source_id;
        buf[2] = u8::from(self.pa_sync);
        buf[3..5].copy_from_slice(&self.pa_interval.0.to_le_bytes());
        buf[5] = self
            .subgroups
            .len()
            .try_into()
            .map_err(|_| PacketError::InvalidParameter("Num_Subgroups".to_string()))?;
        let mut idx = 6;
        for s in &self.subgroups {
            s.encode(&mut buf[idx..])?;
            idx += s.encoded_len();
        }
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::MIN_PACKET_SIZE + self.subgroups.iter().fold(0, |acc, g| acc + g.encoded_len())
    }
}

/// See Broadcast Audio Scan Service spec v1.0 Section 3.1.1.6 for details.
#[derive(Debug, PartialEq)]
pub struct SetBroadcastCodeOperation {
    source_id: SourceId,
    broadcast_code: [u8; 16],
}

impl SetBroadcastCodeOperation {
    const BROADCAST_CODE_LEN: usize = 16;
    const PACKET_SIZE: usize =
        ControlPointOpcode::BYTE_SIZE + SOURCE_ID_BYTE_SIZE + Self::BROADCAST_CODE_LEN;

    pub fn new(source_id: SourceId, broadcast_code: [u8; 16]) -> Self {
        SetBroadcastCodeOperation { source_id, broadcast_code }
    }
}

impl ControlPointOperation for SetBroadcastCodeOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::SetBroadcastCode
    }
}

impl Decodable for SetBroadcastCodeOperation {
    type Error = PacketError;

    // Min size includes Source_ID, PA_Sync, PA_Interval, and Num_Subgroups params.
    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        let decode_fn = || {
            let _ = Self::check_opcode(buf[0])?;
            let source_id = buf[1];
            let mut broadcast_code = [0; Self::BROADCAST_CODE_LEN];
            broadcast_code.copy_from_slice(&buf[2..2 + Self::BROADCAST_CODE_LEN]);
            Ok((Self { source_id, broadcast_code }, Self::PACKET_SIZE))
        };

        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for SetBroadcastCodeOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = Self::opcode() as u8;
        buf[1] = self.source_id;
        buf[2..2 + Self::BROADCAST_CODE_LEN].copy_from_slice(&self.broadcast_code);
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::PACKET_SIZE
    }
}

/// See Broadcast Audio Scan Service spec v1.0 Section 3.1.1.7 for details.
#[derive(Debug, PartialEq)]
pub struct RemoveSourceOperation(SourceId);

impl RemoveSourceOperation {
    const PACKET_SIZE: usize = ControlPointOpcode::BYTE_SIZE + SOURCE_ID_BYTE_SIZE;

    pub fn new(source_id: SourceId) -> Self {
        RemoveSourceOperation(source_id)
    }
}

impl ControlPointOperation for RemoveSourceOperation {
    fn opcode() -> ControlPointOpcode {
        ControlPointOpcode::RemoveSource
    }
}

impl Decodable for RemoveSourceOperation {
    type Error = PacketError;

    // Min size includes Source_ID, PA_Sync, PA_Interval, and Num_Subgroups params.
    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        let decode_fn = || {
            let _ = Self::check_opcode(buf[0])?;
            let source_id = buf[1];
            Ok((RemoveSourceOperation(source_id), Self::PACKET_SIZE))
        };
        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for RemoveSourceOperation {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = Self::opcode() as u8;
        buf[1] = self.0;
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::PACKET_SIZE
    }
}

decodable_enum! {
    pub enum PaSync<u8, bt_common::packet_encoding::Error, OutOfRange> {
        DoNotSync = 0x00,
        SyncPastAvailable = 0x01,
        SyncPastUnavailable = 0x02,
    }
}

impl FromStr for PaSync {
    type Err = PacketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PaSyncOff" => Ok(PaSync::DoNotSync),
            "PaSyncPast" => Ok(PaSync::SyncPastAvailable),
            "PaSyncNoPast" => Ok(PaSync::SyncPastUnavailable),
            _ => Err(PacketError::InvalidParameter(format!("invalid pa_sync: {s}"))),
        }
    }
}

/// 4-octet bitfield. Bit 0-30 = BIS_index[1-31]
/// 0x00000000: 0b0 = Do not synchronize to BIS_index[x]
/// 0xxxxxxxxx: 0b1 = Synchronize to BIS_index[x]
/// 0xFFFFFFFF: means No preference if used in BroadcastAudioScanControlPoint,
///             Failed to sync if used in ReceiveState.
#[derive(Clone, Debug, PartialEq)]
pub struct BisSync(u32);

impl BisSync {
    const BYTE_SIZE: usize = 4;
    const NO_PREFERENCE: u32 = 0xFFFFFFFF;

    /// Creates a new BisSync that doens't synchronzie to any BISes.
    pub fn no_sync() -> BisSync {
        BisSync(0)
    }

    /// Updates the specified BIS index to be synchronized.
    /// Doesn't touch the synchronize value of other BIS indices.
    ///
    /// # Arguments
    ///
    /// * `bis_index` - BIS index as defined in the spec. Range should be [1,
    ///   31]. The specified BIS index will be set to synchronized (0b1).
    pub fn synchronize_to_index(&mut self, bis_index: BisIndex) -> Result<(), PacketError> {
        if bis_index < 1 || bis_index > 31 {
            return Err(PacketError::OutOfRange);
        }
        let bit_mask = 0b1 << (bis_index - 1);

        if self.0 == Self::NO_PREFERENCE {
            // No preference should be re-set to 0 so that all subsequent bit_mask
            // operations correctly set the bits for the specified `bis_index`.
            // See BASS v1.0.1 Section 3.1.1.4 Table 3.5.
            self.0 = 0;
        }
        self.0 |= bit_mask;
        Ok(())
    }

    /// Creates a BisSync value with the specified BIS indices set to be
    /// synchronized.
    ///
    /// # Arguments
    ///
    /// * `bis_indices` - A vector of BIS indices to synchronize to.
    pub fn sync(bis_indices: Vec<BisIndex>) -> Result<Self, PacketError> {
        let mut new_sync = Self::no_sync();
        for bis_index in bis_indices {
            new_sync.synchronize_to_index(bis_index)?;
        }
        Ok(new_sync)
    }
}

impl Default for BisSync {
    fn default() -> Self {
        Self(Self::NO_PREFERENCE)
    }
}

impl From<BisSync> for u32 {
    fn from(bis_sync: BisSync) -> u32 {
        bis_sync.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigSubgroup {
    pub(crate) bis_sync: BisSync,
    pub(crate) metadata: Vec<Metadata>,
}

impl BigSubgroup {
    const METADATA_LENGTH_BYTE_SIZE: usize = 1;
    const MIN_PACKET_SIZE: usize = BisSync::BYTE_SIZE + Self::METADATA_LENGTH_BYTE_SIZE;

    pub fn new(bis_sync: Option<BisSync>) -> Self {
        Self { bis_sync: bis_sync.unwrap_or_default(), metadata: vec![] }
    }

    pub fn with_metadata(mut self, metadata: Vec<Metadata>) -> Self {
        self.metadata = metadata;
        self
    }
}

impl Decodable for BigSubgroup {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < BigSubgroup::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        let decode_fn = || {
            let bis_sync = u32::from_le_bytes(buf[0..4].try_into().unwrap());
            let metadata_len = buf[4] as usize;

            let mut start_idx = 5;
            if buf.len() < start_idx + metadata_len {
                return Err(PacketError::UnexpectedDataLength);
            }

            let (results_metadata, consumed_len) =
                Metadata::decode_all(&buf[start_idx..start_idx + metadata_len]);
            start_idx += consumed_len;
            if start_idx != 5 + metadata_len {
                return Err(PacketError::UnexpectedDataLength);
            }
            // Ignore any undecodable metadata types
            let metadata = results_metadata.into_iter().filter_map(Result::ok).collect();
            Ok((BigSubgroup { bis_sync: BisSync(bis_sync), metadata }, start_idx))
        };
        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for BigSubgroup {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0..4].copy_from_slice(&u32::from(self.bis_sync.clone()).to_le_bytes());
        let metadata_len = self
            .metadata
            .iter()
            .fold(0, |acc, m| acc + m.encoded_len())
            .try_into()
            .map_err(|_| PacketError::InvalidParameter("Metadata".to_string()))?;
        buf[4] = metadata_len;
        let mut next_idx = 5;
        for m in &self.metadata {
            m.encode(&mut buf[next_idx..])
                .map_err(|e| PacketError::InvalidParameter(format!("{e}")))?;
            next_idx += m.encoded_len();
        }
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::MIN_PACKET_SIZE + self.metadata.iter().map(Encodable::encoded_len).sum::<usize>()
    }
}

/// Broadcast Receive State characteristic as defined in
/// Broadcast Audio Scan Service spec v1.0 Section 3.2.
/// The Broadcast Receive State characteristic is used by the server to expose
/// information about a Broadcast Source. If the server has not written a
/// Source_ID value to the Broadcast Receive State characteristic, the Broadcast
/// Recieve State characteristic value shall be empty.
#[derive(Clone, Debug, PartialEq)]
pub enum BroadcastReceiveState {
    Empty,
    NonEmpty(ReceiveState),
}

impl BroadcastReceiveState {
    pub fn is_empty(&self) -> bool {
        *self == BroadcastReceiveState::Empty
    }

    pub fn broadcast_id(&self) -> Option<BroadcastId> {
        match self {
            BroadcastReceiveState::Empty => None,
            BroadcastReceiveState::NonEmpty(state) => Some(state.broadcast_id),
        }
    }

    pub fn has_same_broadcast_id(&self, other: &BroadcastReceiveState) -> bool {
        match self {
            BroadcastReceiveState::Empty => false,
            BroadcastReceiveState::NonEmpty(this) => match other {
                BroadcastReceiveState::Empty => false,
                BroadcastReceiveState::NonEmpty(that) => this.broadcast_id == that.broadcast_id,
            },
        }
    }
}

impl Decodable for BroadcastReceiveState {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() == 0 {
            return (Ok(Self::Empty), 0);
        }
        match ReceiveState::decode(&buf[..]) {
            (Ok(state), consumed) => (Ok(Self::NonEmpty(state)), consumed),
            (Err(e), consumed) => (Err(e), consumed),
        }
    }
}

impl Encodable for BroadcastReceiveState {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        match self {
            Self::Empty => Ok(()),
            Self::NonEmpty(state) => state.encode(&mut buf[..]),
        }
    }

    fn encoded_len(&self) -> core::primitive::usize {
        match self {
            Self::Empty => 0,
            Self::NonEmpty(state) => state.encoded_len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceiveState {
    pub(crate) source_id: SourceId,
    pub(crate) source_address_type: AddressType,
    // Address in little endian.
    pub(crate) source_address: [u8; ADDRESS_BYTE_SIZE],
    pub(crate) source_adv_sid: AdvertisingSetId,
    pub(crate) broadcast_id: BroadcastId,
    pub(crate) pa_sync_state: PaSyncState,
    // Represents BIG_Encryption param with optional Bad_Code param.
    pub(crate) big_encryption: EncryptionStatus,
    pub(crate) subgroups: Vec<BigSubgroup>,
}

impl ReceiveState {
    const MIN_PACKET_SIZE: usize = SOURCE_ID_BYTE_SIZE
        + AddressType::BYTE_SIZE
        + ADDRESS_BYTE_SIZE
        + AdvertisingSetId::BYTE_SIZE
        + BroadcastId::BYTE_SIZE
        + PA_SYNC_BYTE_SIZE
        + EncryptionStatus::MIN_PACKET_SIZE
        + NUM_SUBGROUPS_BYTE_SIZE;

    #[cfg(any(test, feature = "test-utils"))]
    pub fn new(
        source_id: u8,
        source_address_type: AddressType,
        source_address: [u8; ADDRESS_BYTE_SIZE],
        source_adv_sid: AdvertisingSetId,
        broadcast_id: BroadcastId,
        pa_sync_state: PaSyncState,
        big_encryption: EncryptionStatus,
        subgroups: Vec<BigSubgroup>,
    ) -> ReceiveState {
        Self {
            source_id,
            source_address_type,
            source_address,
            source_adv_sid,
            broadcast_id,
            pa_sync_state,
            big_encryption,
            subgroups,
        }
    }

    pub fn pa_sync_state(&self) -> PaSyncState {
        self.pa_sync_state
    }

    pub fn big_encryption(&self) -> EncryptionStatus {
        self.big_encryption
    }

    pub fn broadcast_id(&self) -> BroadcastId {
        self.broadcast_id
    }

    pub fn source_id(&self) -> SourceId {
        self.source_id
    }
}

impl Decodable for ReceiveState {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let decode_fn = || {
            let source_id = buf[0];
            let source_address_type = AddressType::try_from(buf[1])?;
            let mut source_address = [0; ADDRESS_BYTE_SIZE];
            source_address.clone_from_slice(&buf[2..8]);
            let source_adv_sid = AdvertisingSetId::try_from(buf[8])?;
            let broadcast_id = BroadcastId::decode(&buf[9..12]).0?;
            let pa_sync_state = PaSyncState::try_from(buf[12])?;

            let big_encryption;
            let mut idx = 13;
            match EncryptionStatus::decode(&buf[13..]) {
                (Ok(encryption), consumed) => {
                    big_encryption = encryption;
                    idx += consumed;
                }
                (Err(e), _) => {
                    return Err(e);
                }
            }
            if buf.len() <= idx {
                return Err(PacketError::UnexpectedDataLength);
            }
            let num_subgroups = buf[idx] as usize;
            let mut subgroups = Vec::new();
            idx += 1;
            for _i in 0..num_subgroups {
                if buf.len() <= idx {
                    return Err(PacketError::UnexpectedDataLength);
                }
                let (subgroup, consumed) = BigSubgroup::decode(&buf[idx..]);
                subgroups.push(subgroup?);
                idx += consumed;
            }
            Ok((
                ReceiveState {
                    source_id,
                    source_address_type,
                    source_address,
                    source_adv_sid,
                    broadcast_id,
                    pa_sync_state,
                    big_encryption,
                    subgroups,
                },
                idx,
            ))
        };
        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

impl Encodable for ReceiveState {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = self.source_id;
        buf[1] = self.source_address_type as u8;
        buf[2..8].copy_from_slice(&self.source_address);
        buf[8] = self.source_adv_sid.value();
        self.broadcast_id.encode(&mut buf[9..12])?;
        buf[12] = u8::from(self.pa_sync_state);
        let mut idx = 13 + self.big_encryption.encoded_len();
        self.big_encryption.encode(&mut buf[13..idx])?;
        buf[idx] = self
            .subgroups
            .len()
            .try_into()
            .map_err(|_| PacketError::InvalidParameter("Metadata".to_string()))?;
        idx += 1;
        for s in &self.subgroups {
            s.encode(&mut buf[idx..])?;
            idx += s.encoded_len();
        }
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        // Length including Source_ID, Source_Address_Type, Source_Address,
        // Source_Adv_SID, Broadcast_ID, PA_Sync_State, BIG_Encryption, Bad_Code,
        // Num_Subgroups and subgroup-related params.
        SOURCE_ID_BYTE_SIZE
            + AddressType::BYTE_SIZE
            + self.source_address.len()
            + AdvertisingSetId::BYTE_SIZE
            + self.broadcast_id.encoded_len()
            + PA_SYNC_BYTE_SIZE
            + self.big_encryption.encoded_len()
            + NUM_SUBGROUPS_BYTE_SIZE
            + self.subgroups.iter().map(Encodable::encoded_len).sum::<usize>()
    }
}

decodable_enum! {
    pub enum PaSyncState<u8, bt_common::packet_encoding::Error, OutOfRange> {
        NotSynced = 0x00,
        SyncInfoRequest = 0x01,
        Synced = 0x02,
        FailedToSync = 0x03,
        NoPast = 0x04,
    }
}

/// Represents BIG_Encryption and Bad_Code params from BASS spec v.1.0 Table
/// 3.9.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EncryptionStatus {
    NotEncrypted,
    BroadcastCodeRequired,
    Decrypting,
    BadCode([u8; 16]),
}

impl EncryptionStatus {
    // Should at least include the BIG_Encryption enum value which is 1 byte long.
    const MIN_PACKET_SIZE: usize = 1;

    // Returns the u8 value that represents the status of encryption
    // as described for BIG_Encryption parameter.
    pub const fn raw_value(self) -> u8 {
        match self {
            EncryptionStatus::NotEncrypted => 0x00,
            EncryptionStatus::BroadcastCodeRequired => 0x01,
            EncryptionStatus::Decrypting => 0x02,
            EncryptionStatus::BadCode(_) => 0x03,
        }
    }
}

impl Decodable for EncryptionStatus {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < 1 {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }
        match buf[0] {
            0x00 => (Ok(Self::NotEncrypted), 1),
            0x01 => (Ok(Self::BroadcastCodeRequired), 1),
            0x02 => (Ok(Self::Decrypting), 1),
            0x03 => {
                if buf.len() < 17 {
                    return (Err(PacketError::UnexpectedDataLength), buf.len());
                }
                (Ok(Self::BadCode(buf[1..17].try_into().unwrap())), 17)
            }
            _ => (Err(PacketError::OutOfRange), buf.len()),
        }
    }
}

impl Encodable for EncryptionStatus {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        buf[0] = self.raw_value();
        match self {
            EncryptionStatus::BadCode(code) => buf[1..17].copy_from_slice(code),
            _ => {}
        }
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        match self {
            // For Bad_Code value, we also have to encrypt the incorrect
            // 16-octet Broadcast_Code. See BASS spec Table 3.9.
            EncryptionStatus::BadCode(_) => 1 + 16,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_common::generic_audio::ContextType;

    #[test]
    fn encryption_status_enum() {
        let not_encrypted = EncryptionStatus::NotEncrypted;
        let encrypted = EncryptionStatus::BroadcastCodeRequired;
        let decrypting = EncryptionStatus::Decrypting;
        let bad_code = EncryptionStatus::BadCode([
            0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03,
            0x02, 0x01,
        ]);

        assert_eq!(0x00, not_encrypted.raw_value());
        assert_eq!(0x01, encrypted.raw_value());
        assert_eq!(0x02, decrypting.raw_value());
        assert_eq!(0x03, bad_code.raw_value());
    }

    #[test]
    fn encryption_status() {
        // Encoding not encrypted status.
        let not_encrypted = EncryptionStatus::NotEncrypted;
        assert_eq!(not_encrypted.encoded_len(), 1);
        let mut buf = vec![0; not_encrypted.encoded_len()];
        let _ = not_encrypted.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x00];
        assert_eq!(buf, bytes);

        // Decoding not encrypted.
        let (decoded, len) = EncryptionStatus::decode(&bytes);
        assert_eq!(decoded, Ok(not_encrypted));
        assert_eq!(len, 1);

        // Encoding bad code status with code.
        let bad_code = EncryptionStatus::BadCode([
            0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03,
            0x02, 0x01,
        ]);
        assert_eq!(bad_code.encoded_len(), 17);
        let mut buf = vec![0; bad_code.encoded_len()];
        let _ = bad_code.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![
            0x03, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04,
            0x03, 0x02, 0x01,
        ];
        assert_eq!(buf, bytes);

        // Decoding bad code statsu with code.
        let (decoded, len) = EncryptionStatus::decode(&bytes);
        assert_eq!(decoded, Ok(bad_code));
        assert_eq!(len, 17);
    }

    #[test]
    fn invalid_encryption_status() {
        // Cannot encode into empty buffer.
        let not_encrypted = EncryptionStatus::NotEncrypted;
        let mut buf = vec![];
        let _ = not_encrypted.encode(&mut buf[..]).expect_err("should fail");

        // Not enough buffer space for encoding.
        let bad_code = EncryptionStatus::BadCode([
            0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03,
            0x02, 0x01,
        ]);
        let mut buf = vec![0; 1];
        let _ = bad_code.encode(&mut buf[..]).expect_err("should fail");

        // Cannot decode empty buffer.
        let buf = vec![];
        let _ = EncryptionStatus::decode(&buf).0.expect_err("should fail");

        // Bad code status with no code.
        let buf = vec![0x03];
        let _ = EncryptionStatus::decode(&buf).0.expect_err("should fail");
    }

    #[test]
    fn bis_sync_sync() {
        let bis_sync = BisSync::sync(vec![1, 6, 31]).expect("should succeed");
        assert_eq!(u32::from(bis_sync), 0x40000021);

        let bis_sync_empty = BisSync::sync(vec![]).expect("should succeed");
        assert_eq!(u32::from(bis_sync_empty), 0);
    }

    #[test]
    fn invalid_bis_sync() {
        BisSync::sync(vec![0]).expect_err("should fail");
        BisSync::sync(vec![32]).expect_err("should fail");
    }

    #[test]
    fn synchronize_to_index() {
        let mut bis_sync = BisSync::no_sync();
        assert_eq!(u32::from(bis_sync.clone()), 0);

        bis_sync.synchronize_to_index(1).expect("should succeed");
        assert_eq!(u32::from(bis_sync.clone()), 0x1);

        bis_sync.synchronize_to_index(31).expect("should succeed");
        assert_eq!(u32::from(bis_sync.clone()), 0x40000001);

        bis_sync.synchronize_to_index(0).expect_err("should fail");
        bis_sync.synchronize_to_index(32).expect_err("should fail");
    }

    #[test]
    fn synchronize_to_index_from_default() {
        let mut bis_sync_default = BisSync::default();
        assert_eq!(u32::from(bis_sync_default.clone()), 0xFFFFFFFF);

        // An initial synchronization of no preference should correctly be set with the
        // requested index.
        bis_sync_default.synchronize_to_index(1).expect("should succeed");
        assert_eq!(u32::from(bis_sync_default.clone()), 0x1);

        // Additional index should be accumulated correctly.
        bis_sync_default.synchronize_to_index(6).expect("should succeed");
        assert_eq!(u32::from(bis_sync_default), 0x21);
    }

    #[test]
    fn pa_sync_from_str() {
        let sync = PaSync::from_str("PaSyncOff").expect("should succeed");
        assert_eq!(sync, PaSync::DoNotSync);
        let sync = PaSync::from_str("PaSyncPast").expect("should succeed");
        assert_eq!(sync, PaSync::SyncPastAvailable);
        let sync = PaSync::from_str("PaSyncNoPast").expect("should succeed");
        assert_eq!(sync, PaSync::SyncPastUnavailable);
        PaSync::from_str("invalid").expect_err("should fail");
    }

    #[test]
    fn remote_scan_stopped() {
        // Encoding remote scan stopped.
        let stopped = RemoteScanStoppedOperation;
        assert_eq!(stopped.encoded_len(), 1);
        let mut buf = vec![0u8; stopped.encoded_len()];
        stopped.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![0x00];
        assert_eq!(buf, bytes);

        // Decoding remote scan stopped.
        let (decoded, len) = RemoteScanStoppedOperation::decode(&bytes);
        assert_eq!(decoded, Ok(stopped));
        assert_eq!(len, 1);
        assert_eq!(
            RemoteScanStoppedOperation::decode(&[]).0,
            Err(PacketError::UnexpectedDataLength)
        );
    }

    #[test]
    fn remote_scan_started() {
        // Encoding remote scan started.
        let started = RemoteScanStartedOperation;
        assert_eq!(started.encoded_len(), 1);
        let mut buf = vec![0u8; started.encoded_len()];
        started.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![0x01];
        assert_eq!(buf, vec![0x01]);

        // Decoding remote scan started.
        let (decoded, len) = RemoteScanStartedOperation::decode(&bytes);
        assert_eq!(decoded, Ok(started));
        assert_eq!(len, 1);
    }

    #[test]
    fn add_source_without_subgroups() {
        // Encoding operation with no subgroups.
        let op = AddSourceOperation::new(
            AddressType::Public,
            [0x04, 0x10, 0x00, 0x00, 0x00, 0x00],
            AdvertisingSetId::try_from(1).unwrap(),
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::DoNotSync,
            PeriodicAdvertisingInterval::unknown(),
            vec![],
        );
        assert_eq!(op.encoded_len(), 16);
        let mut buf = vec![0u8; op.encoded_len()];
        op.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![
            0x02, 0x00, 0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x01, 0x11, 0x00, 0x00, 0x00, 0xFF,
            0xFF, 0x00,
        ];
        assert_eq!(buf, bytes);

        // Decoding operation with no subgroups.
        let (decoded, len) = AddSourceOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 16);
    }

    #[test]
    fn add_source_with_subgroups() {
        // Encoding operation with subgroups.
        let subgroups = vec![BigSubgroup::new(None).with_metadata(vec![
            Metadata::PreferredAudioContexts(vec![ContextType::Media, ContextType::Game]), // encoded_len = 4
            Metadata::ProgramInfo("test".to_string()), // encoded_len = 6
        ])];
        let op = AddSourceOperation::new(
            AddressType::Random,
            [0x04, 0x10, 0x00, 0x00, 0x00, 0x00],
            AdvertisingSetId::try_from(1).unwrap(),
            BroadcastId::try_from(0x11).unwrap(),
            PaSync::SyncPastAvailable,
            PeriodicAdvertisingInterval::unknown(),
            subgroups,
        );
        assert_eq!(op.encoded_len(), 31); // 16 for minimum params and params 15 for the subgroup.
        let mut buf = vec![0u8; op.encoded_len()];
        op.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![
            0x02, 0x01, 0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x01, 0x11, 0x00, 0x00, 0x01, 0xFF,
            0xFF, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x0A, // BIS_Sync, Metdata_Length
            0x03, 0x01, 0x0C, 0x00, // Preferred_Audio_Contexts metadata
            0x05, 0x03, 0x74, 0x65, 0x73, 0x074, // Program_Info metadata
        ];
        assert_eq!(buf, bytes);

        // Decoding operation with subgroups.
        let (decoded, len) = AddSourceOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 31);
    }

    #[test]
    fn invalid_advertising_sid_decoding() {
        // AddSourceOperation with invalid Advertising_SID (0x10 > 0x0F)
        let invalid_add_source_bytes = vec![
            0x02, 0x00, 0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x10, 0x11, 0x00, 0x00, 0x00, 0xFF,
            0xFF, 0x00,
        ];
        let (decoded, _) = AddSourceOperation::decode(&invalid_add_source_bytes);
        assert_eq!(decoded, Err(PacketError::OutOfRange));

        // ReceiveState with invalid Advertising_SID (0x10 > 0x0F)
        let invalid_receive_state_bytes = vec![
            0x01, 0x00, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x10, 0x03, 0x02, 0x01, 0x02, 0x00,
            0x00,
        ];
        let (decoded, _) = BroadcastReceiveState::decode(&invalid_receive_state_bytes);
        assert_eq!(decoded, Err(PacketError::OutOfRange));
    }

    #[test]
    fn modify_source_without_subgroups() {
        // Encoding operation with no subgroups.
        let op = ModifySourceOperation::new(
            0x0A,
            PaSync::SyncPastAvailable,
            PeriodicAdvertisingInterval(0x1004),
            vec![],
        );
        assert_eq!(op.encoded_len(), 6);
        let mut buf = vec![0u8; op.encoded_len()];
        op.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![0x03, 0x0A, 0x01, 0x04, 0x10, 0x00];
        assert_eq!(buf, bytes);

        // Decoding operation with no subgroups.
        let (decoded, len) = ModifySourceOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 6);
    }

    #[test]
    fn modify_source_with_subgroups() {
        // Encoding operation with subgroups.
        let subgroups = vec![
            BigSubgroup::new(None).with_metadata(vec![Metadata::ParentalRating(Rating::all_age())]), /* encoded_len = 8 */
            BigSubgroup::new(Some(BisSync(0x000000FE)))
                .with_metadata(vec![Metadata::BroadcastAudioImmediateRenderingFlag]), /* encoded_len = 7 */
        ];
        let op = ModifySourceOperation::new(
            0x0B,
            PaSync::DoNotSync,
            PeriodicAdvertisingInterval::unknown(),
            subgroups,
        );
        assert_eq!(op.encoded_len(), 21); // 6 for minimum params and params 15 for two subgroups.
        let mut buf = vec![0u8; op.encoded_len()];
        op.encode(&mut buf[..]).expect("shoud succeed");

        let bytes = vec![
            0x03, 0x0B, 0x00, 0xFF, 0xFF, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x03, 0x02, 0x06,
            0x01, // First subgroup.
            0xFE, 0x00, 0x00, 0x00, 0x02, 0x01, 0x09, // Second subgroup.
        ];
        assert_eq!(buf, bytes);

        // Decoding operation with subgroups.
        let (decoded, len) = ModifySourceOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 21);
    }

    #[test]
    fn set_broadcast_code() {
        // Encoding.
        let op = SetBroadcastCodeOperation::new(
            0x0A,
            [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14,
                0x15, 0x16,
            ],
        );
        assert_eq!(op.encoded_len(), 18);
        let mut buf = vec![0; op.encoded_len()];
        op.encode(&mut buf[..]).expect("should succeed");

        let bytes = vec![
            0x04, 0x0A, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x12,
            0x13, 0x14, 0x15, 0x16,
        ];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = SetBroadcastCodeOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 18);
    }

    #[test]
    fn remove_source() {
        // Encoding.
        let op = RemoveSourceOperation::new(0x0A);
        assert_eq!(op.encoded_len(), 2);
        let mut buf = vec![0; op.encoded_len()];
        op.encode(&mut buf[..]).expect("should succeed");

        let bytes = vec![0x05, 0x0A];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = RemoveSourceOperation::decode(&bytes);
        assert_eq!(decoded, Ok(op));
        assert_eq!(len, 2);
    }

    #[test]
    fn broadcast_receive_state_without_subgroups() {
        // Encoding.
        let state = BroadcastReceiveState::NonEmpty(ReceiveState {
            source_id: 0x01,
            source_address_type: AddressType::Public,
            source_address: [0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A],
            source_adv_sid: AdvertisingSetId::try_from(0x01).unwrap(),
            broadcast_id: BroadcastId::try_from(0x00010203).unwrap(),
            pa_sync_state: PaSyncState::Synced,
            big_encryption: EncryptionStatus::BadCode([
                0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x7, 0x06, 0x05, 0x04, 0x03,
                0x02, 0x01,
            ]),
            subgroups: vec![],
        });
        assert_eq!(state.encoded_len(), 31);
        let mut buf = vec![0; state.encoded_len()];
        state.encode(&mut buf[..]).expect("should succeed");

        let bytes = vec![
            0x01, 0x00, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x01, 0x03, 0x02, 0x01, 0x02, 0x03,
            0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x10, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03,
            0x02, 0x01, // Bad_Code with the code.
            0x00,
        ];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = BroadcastReceiveState::decode(&bytes);
        assert_eq!(decoded, Ok(state));
        assert_eq!(len, 31);
    }

    #[test]
    fn broadcast_receive_state_with_subgroups() {
        // Encoding
        let state = BroadcastReceiveState::NonEmpty(ReceiveState {
            source_id: 0x01,
            source_address_type: AddressType::Random,
            source_address: [0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A],
            source_adv_sid: AdvertisingSetId::try_from(0x01).unwrap(),
            broadcast_id: BroadcastId::try_from(0x00010203).unwrap(),
            pa_sync_state: PaSyncState::NotSynced,
            big_encryption: EncryptionStatus::NotEncrypted,
            subgroups: vec![BigSubgroup::new(None)
                .with_metadata(vec![Metadata::ParentalRating(Rating::AllAge)]) /* encoded_len = 8 */],
        });
        assert_eq!(state.encoded_len(), 23);
        let mut buf = vec![0; state.encoded_len()];
        state.encode(&mut buf[..]).expect("should succeed");

        let bytes = vec![
            0x01, 0x01, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x01, 0x03, 0x02, 0x01, 0x00, 0x00,
            0x01, // 1 Subgroup.
            0xFF, 0xFF, 0xFF, 0xFF, 0x03, 0x02, 0x06, 0x01, // Subgroup.
        ];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = BroadcastReceiveState::decode(&bytes);
        assert_eq!(decoded, Ok(state));
        assert_eq!(len, 23);
    }
}
