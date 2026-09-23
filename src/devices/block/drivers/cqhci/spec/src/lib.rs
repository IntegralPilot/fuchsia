// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitfield::bitfield;
use bitflags::bitflags;
use std::num::NonZeroU16;

/// The CQHCI spec requires 512 byte blocks (JESD84-B51A, 6.6.39.1)
pub const MMC_BLOCK_SIZE: u64 = 512;

// EXT_CSD fields (JESD84-B51A, 7.4)

pub const EXT_CSD_SIZE: usize = 512;

pub const EXT_CSD_BARRIER_EN: usize = 31;
pub const EXT_CSD_BARRIER_ENABLED: u8 = 1;

pub const EXT_CSD_FLUSH_CACHE: usize = 32;
pub const EXT_CSD_FLUSH_CACHE_FLUSH: u8 = 0x1;
pub const EXT_CSD_FLUSH_CACHE_BARRIER: u8 = 0x2;

pub const EXT_CSD_CACHE_CTRL: usize = 33;
pub const EXT_CSD_CACHE_EN_MASK: u8 = 1;

pub const EXT_CSD_PARTITION_CONFIG: usize = 179;
pub const EXT_CSD_PARTITION_ACCESS_MASK: u8 = 0xf8;

pub const EXT_CSD_PARTITON_SWITCH_TIME: usize = 199;

pub const EXT_CSD_SEC_FEATURE_SUPPORT: usize = 231;
pub const EXT_CSD_SEC_FEATURE_SUPPORT_SEC_GB_CL_EN: u8 = 1 << 4;

pub const EXT_CSD_CACHE_FLUSH_POLICY: usize = 240;
pub const EXT_CSD_CACHE_FLUSH_POLICY_FIFO: u8 = 1;

pub const EXT_CSD_GENERIC_CMD6_TIME: usize = 248;

pub const EXT_CSD_BARRIER_SUPPORT: usize = 486;
pub const EXT_CSD_BARRIER_SUPPORT_MASK: u8 = 0x1;

#[derive(Clone, Copy, Debug, PartialEq, enumn::N)]
#[repr(u8)]
/// Command codes for MMC (JESD84-B51A, 6.10.4).
///
/// Only a limited subset which are useful for the CQHCI driver are included.
pub enum MmcCommand {
    Switch = 6,
    SendStatus = 13,
    EraseGroupStart = 35,
    EraseGroupEnd = 36,
    Erase = 38,
    QueuedTaskParams = 44,
    QueuedTaskAddress = 45,
    CommandQueueTaskManagement = 48,
}

/// The argument for a CMD38 when requesting a TRIM.  The contents of a trimmed block will be either
/// all 1s or all 0s, depending on the device (see JESD84-B51A, 6.6.10).
pub const MMC_ERASE_TRIM_ARG: u32 = 0x00000001;

/// The argument for a CMD38 when requesting a DISCARD.  The contents of a discarded block will be
/// indeterminate and the block will be asynchronously trimmed at a later time (see JESD84-B51A,
/// 6.6.12).
pub const MMC_ERASE_DISCARD_ARG: u32 = 0x00000003;

impl MmcCommand {
    fn response_type(&self) -> DcmdResponseType {
        match self {
            Self::Switch => DcmdResponseType::R1B,
            Self::SendStatus => DcmdResponseType::R1,
            Self::EraseGroupStart => DcmdResponseType::R1,
            Self::EraseGroupEnd => DcmdResponseType::R1,
            Self::Erase => DcmdResponseType::R1B,
            Self::QueuedTaskParams => DcmdResponseType::R1,
            Self::QueuedTaskAddress => DcmdResponseType::R1,
            Self::CommandQueueTaskManagement => DcmdResponseType::R1B,
        }
    }
}

impl TryFrom<u8> for MmcCommand {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        Self::n(value).ok_or(())
    }
}

impl From<MmcCommand> for u8 {
    fn from(value: MmcCommand) -> Self {
        value as u8
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    /// Response for CMD13 SEND_STATUS (JESD84-B51A, 6.10.4)
    pub struct MmcSendStatusResponse: u32 {
        const STATUS_ERR = 1 << 19;
        const CURRENT_STATE_STDBY = 0x3 << 9;
        const CURRENT_STATE_TRAN = 0x4 << 9;
        const CURRENT_STATE_DATA = 0x5 << 9;
        const CURRENT_STATE_RECV = 0x6 << 9;
        const CURRENT_STATE_SLP = 0xa << 9;
        const READY_FOR_DATA = 1 << 8;
        const SWITCH_ERR = 1 << 7;
        const EXCEPTION_EVENT = 1 << 6;
        const APP_CMD = 1 << 5;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Direction of data transfer.
pub enum Direction {
    Read,
    Write,
}

impl Direction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

// All task descriptors have a constant act
const TASK_DESCRIPTOR_ACT: u8 = 0b101;

bitfield! {
    #[derive(
        Clone, Copy, Eq, PartialEq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
    )]
    /// A 128-bit task descriptor in the CQHCI Task Descriptor List (JESD84-B51A, B.2.1)
    pub struct CommandQueueTaskDescriptor(u128);
    impl Debug;
    pub bool, valid, set_valid: 0;
    pub bool, end, set_end: 1;
    pub bool, int, set_int: 2;
    pub u8, act, set_act: 5, 3;
    pub bool, forced_programming, set_forced_programming: 6;
    pub u8, context_id, set_context_id: 10, 7;
    pub bool, tag_request, set_tag_request: 11;
    pub bool, data_direction, set_data_direction: 12;
    pub bool, priority, set_priority: 13;
    pub bool, qbr, set_qbr: 14;
    pub bool, reliable_write, set_reliable_write: 15;
    pub u16, block_count, set_block_count: 31, 16;
    pub u32, block_offset, set_block_offset: 63, 32;
    pub u32, dun, set_dun: 95, 64;
    pub u8, cci, set_cci: 103, 96;
    pub bool, ce, set_ce: 111;
    // 112..=127 reserved
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CryptoParams {
    pub slot: u8,
    pub dun: u32,
}

impl CommandQueueTaskDescriptor {
    fn new(
        direction: Direction,
        block_offset: u32,
        block_count: NonZeroU16,
        queue_barrier: bool,
        crypto_params: Option<CryptoParams>,
    ) -> Self {
        let mut this = Self(0);
        this.set_valid(true);
        this.set_end(true);
        this.set_int(true);
        this.set_act(TASK_DESCRIPTOR_ACT);
        this.set_data_direction(direction == Direction::Read);
        this.set_block_count(block_count.get());
        this.set_block_offset(block_offset as u32);
        this.set_qbr(queue_barrier);
        if let Some(CryptoParams { slot, dun }) = crypto_params {
            this.set_ce(true);
            this.set_cci(slot);
            this.set_dun(dun);
        }
        this
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DcmdResponseType {
    /// No response is expected for the command
    NoResponse = 0b00,
    /// Normal response expected
    R1 = 0b10,
    /// Like R1, but with an optional busy signal transmitted on the DATA line.
    R1B = 0b11,
}

impl From<DcmdResponseType> for u8 {
    fn from(value: DcmdResponseType) -> Self {
        value as u8
    }
}

bitfield! {
    #[derive(
        Clone, Copy, Eq, PartialEq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
    )]
    /// A Direct Command task descriptor in the CQHCI Task Descriptor List (JESD84-B51A, B.2.3)
    pub struct CommandQueueDirectCmdTaskDescriptor(u128);
    impl Debug;
    pub bool, valid, set_valid: 0;
    pub bool, end, set_end: 1;
    pub bool, int, set_int: 2;
    pub u8, act, set_act: 5, 3;
    pub bool, qbr, set_qbr: 14;
    pub u8, from try_into MmcCommand, cmd_index, set_cmd_index: 21, 16;
    pub bool, cmd_timing, set_cmd_timing: 22;
    pub u8, from into DcmdResponseType, _, set_response_type: 24, 23;
    pub u32, cmd_arg, set_cmd_arg: 63, 32;
}

impl CommandQueueDirectCmdTaskDescriptor {
    fn new(command: MmcCommand, command_arg: u32) -> Self {
        let mut this = Self(0);
        this.set_valid(true);
        this.set_end(true);
        this.set_act(TASK_DESCRIPTOR_ACT);
        this.set_qbr(true);
        this.set_int(true);
        this.set_cmd_index(command);
        let response_type = command.response_type();
        this.set_response_type(response_type);
        // Whether the command may be sent to device during data activity or busy time.
        // From the spec: "NOTE Shall be set to 0 if response type is b11 (R1b)"
        this.set_cmd_timing(response_type != DcmdResponseType::R1B);
        this.set_cmd_arg(command_arg);
        this
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
/// A wrapper around the transfer length field in CQHCI transfer descriptors.
/// The raw value of 0 is interpreted as 64KiB, which is hidden behind this type for clarity
/// (JESD84-B51A, B.3.2).
pub struct TransferBytes(u16);

impl TransferBytes {
    /// The maximum number of bytes which can be referenced by a single transfer descriptor.
    pub const MAX_BYTES: usize = u16::MAX as usize + 1;

    /// The maximum number of blocks which can be referenced by a single transfer descriptor.
    pub const MAX_BLOCKS: u64 = Self::MAX_BYTES as u64 / MMC_BLOCK_SIZE;

    pub const MAX: Self = Self(0);
}

impl From<TransferBytes> for u32 {
    fn from(length: TransferBytes) -> u32 {
        if length == TransferBytes::MAX { TransferBytes::MAX_BYTES as u32 } else { length.0 as u32 }
    }
}

impl TryFrom<usize> for TransferBytes {
    type Error = usize;

    fn try_from(size: usize) -> Result<Self, Self::Error> {
        if size == 0 {
            Err(size)
        } else if size < Self::MAX_BYTES {
            debug_assert!(size <= u16::MAX as usize);
            Ok(Self(size as u16))
        } else if size == Self::MAX_BYTES {
            Ok(Self::MAX)
        } else {
            Err(size)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, enumn::N)]
#[repr(u8)]
pub enum TransferAct {
    /// The transfer descriptor points to a data region to read/write to.
    Tran = 0b100,
    /// The transfer descriptor points to a list of transfer descriptors.
    Link = 0b110,
}

impl TryFrom<u8> for TransferAct {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        Self::n(value).ok_or(())
    }
}

impl From<TransferAct> for u8 {
    fn from(value: TransferAct) -> Self {
        value as u8
    }
}

bitfield! {
    #[derive(
        Clone, Copy, Eq, PartialEq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
    )]
    /// A transfer descriptor in the CQHCI Task Descriptor List (JESD84-B51A, B.2.2).
    pub struct CommandQueueTransferDescriptor(u128);
    impl Debug;
    pub bool, valid, set_valid: 0;
    pub bool, end, set_end: 1;
    pub bool, int, set_int: 2;
    pub u8, from try_into TransferAct, act, set_act: 5, 3;
    // 6..=15 reserved
    pub u16, length, set_length: 31, 16;
    pub u64, address, set_address: 95, 32;
    // 96..=127 reserved
}

impl CommandQueueTransferDescriptor {
    /// Creates a new [`CommandQueueTransferDescriptor`] pointing to a data buffer.
    pub fn transfer(address: u64, length: TransferBytes, end: bool) -> Self {
        let mut this = Self(0);
        this.set_valid(true);
        this.set_end(end);
        this.set_int(false);
        this.set_act(TransferAct::Tran);
        this.set_length(length.0);
        this.set_address(address);
        this
    }

    /// Creates a new [`CommandQueueTransferDescriptor`] pointing to a list of transfer descriptors.
    pub fn link(address: u64) -> Self {
        let mut this = Self(0);
        this.set_valid(true);
        this.set_end(false);
        this.set_int(false);
        this.set_act(TransferAct::Link);
        this.set_address(address);
        this
    }
}

#[repr(C)]
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
)]
/// An entry in the CQHCI Task Descriptor List (JESD84-B51A, B.2).
///
/// Note that this assumes 16-byte descriptors.
pub struct CommandQueueTDLEntry {
    pub task: CommandQueueTaskDescriptor,
    pub transfer: CommandQueueTransferDescriptor,
}

impl CommandQueueTDLEntry {
    pub fn task(&self) -> &CommandQueueTaskDescriptor {
        &self.task
    }

    /// Creates a new [`CommandQueueTDLEntry`] which points to a single memory region at
    /// `phys_address`.
    ///
    /// The caller must ensure that `block_count` does not exceed the maximum transfer size of
    /// [`TransferBytes::MAX_BLOCKS`], otherwise an error is returned.
    pub fn single_buffer(
        direction: Direction,
        block_offset: u32,
        block_count: NonZeroU16,
        phys_address: u64,
        queue_barrier: bool,
        crypto_params: Option<CryptoParams>,
    ) -> Result<Self, ()> {
        let length = TransferBytes::try_from(block_count.get() as usize * MMC_BLOCK_SIZE as usize)
            .map_err(|_| ())?;
        Ok(Self {
            task: CommandQueueTaskDescriptor::new(
                direction,
                block_offset,
                block_count,
                queue_barrier,
                crypto_params,
            ),
            transfer: CommandQueueTransferDescriptor::transfer(phys_address, length, true),
        })
    }

    /// Creates a new [`CommandQueueTDLEntry`] which points to a list of
    /// [`CommandQueueTransferDescriptor`]s at `descriptors_phys_address`.  The caller must ensure
    /// that one or more descriptors, ending with one that has END set, is initialized at this
    /// address before submitting the task.
    pub fn scatter_gather_buffers(
        direction: Direction,
        block_offset: u32,
        block_count: NonZeroU16,
        descriptors_phys_address: u64,
        queue_barrier: bool,
        crypto_params: Option<CryptoParams>,
    ) -> Self {
        debug_assert!(
            descriptors_phys_address
                .is_multiple_of(std::mem::align_of::<CommandQueueTransferDescriptor>() as u64)
        );
        Self {
            task: CommandQueueTaskDescriptor::new(
                direction,
                block_offset,
                block_count,
                queue_barrier,
                crypto_params,
            ),
            transfer: CommandQueueTransferDescriptor::link(descriptors_phys_address),
        }
    }
}

#[repr(C)]
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
)]
/// A DCMD entry in the CQHCI Task Descriptor List (JESD84-B51A, B.2.2).
///
/// Note that this assumes 16-byte descriptors.
///
/// Should only be written into the DCMD slot in the TDL; regular transfers must be of type
/// [`CommandQueueTDLEntry`].
pub struct CommandQueueTDLDirectCmdEntry {
    pub task: CommandQueueDirectCmdTaskDescriptor,
    _transfer: u128,
}

impl CommandQueueTDLDirectCmdEntry {
    pub fn new(command: MmcCommand, command_arg: u32) -> Self {
        Self { task: CommandQueueDirectCmdTaskDescriptor::new(command, command_arg), _transfer: 0 }
    }
}

pub const CQHCI_TASK_DESCRIPTOR_LIST_NUM_SLOTS: usize = 32;
pub const CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT: u8 = 31;
pub const CQHCI_TASK_DESCRIPTOR_LIST_SIZE: usize =
    CQHCI_TASK_DESCRIPTOR_LIST_NUM_SLOTS * size_of::<CommandQueueTDLEntry>();

// CQHCI registers (JESD84-B51A, B.3.1)

pub const CQHCI_CQ_VER_OFFSET: usize = 0x0;
pub const CQHCI_CQ_CAP_OFFSET: usize = 0x4;
pub const CQHCI_CQ_CFG_OFFSET: usize = 0x8;
pub const CQHCI_CQ_CTL_OFFSET: usize = 0xC;
pub const CQHCI_CQ_IS_OFFSET: usize = 0x10;
pub const CQHCI_CQ_ISTE_OFFSET: usize = 0x14;
pub const CQHCI_CQ_ISGE_OFFSET: usize = 0x18;
pub const CQHCI_CQ_IC_OFFSET: usize = 0x1c;
pub const CQHCI_CQ_TDLBA_OFFSET: usize = 0x20;
pub const CQHCI_CQ_TDLBAU_OFFSET: usize = 0x24;
pub const CQHCI_CQ_TDBR_OFFSET: usize = 0x28;
pub const CQHCI_CQ_TCN_OFFSET: usize = 0x2C;
pub const CQHCI_CQ_DQS_OFFSET: usize = 0x30;
pub const CQHCI_CQ_DPT_OFFSET: usize = 0x34;
pub const CQHCI_CQ_TDPE_OFFSET: usize = 0x3C;
pub const CQHCI_CQ_SSC1_OFFSET: usize = 0x40;
pub const CQHCI_CQ_SSC2_OFFSET: usize = 0x44;
pub const CQHCI_CQ_CRDCT_OFFSET: usize = 0x48;
pub const CQHCI_CQ_RMEM_OFFSET: usize = 0x50;
pub const CQHCI_CQ_TERRI_OFFSET: usize = 0x54;
pub const CQHCI_CQ_CRI_OFFSET: usize = 0x58;
pub const CQHCI_CQ_CRA_OFFSET: usize = 0x5C;
pub const CQHCI_CQ_HCCAP_OFFSET: usize = 0x60;
pub const CQHCI_CQ_HCCFG_OFFSET: usize = 0x64;
// The following registers are valid iff CS is set in CQHCI_CQ_CAP_OFFSET.
pub const CQHCI_CQ_CRYPTO_NQP_OFFSET: usize = 0x70;
pub const CQHCI_CQ_CRYPTO_NQDUN_OFFSET: usize = 0x74;
pub const CQHCI_CQ_CRYPTO_NQIS_OFFSET: usize = 0x78;
pub const CQHCI_CQ_CRYPTO_NQIE_OFFSET: usize = 0x7C;
pub const CQHCI_CQ_CRYPTO_CAP_OFFSET: usize = 0x100;

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqCapsRegister(u32);
    impl Debug;
    pub u16, timer_clock_freq, set_timer_clock_freq: 9, 0;
    pub u8, timer_clock_freq_multiplier, set_timer_clock_freq_multiplier: 15, 12;
    pub bool, crypto_support, set_crypto_support: 28;
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CqhciCqCfgRegister: u32 {
        const DCMD_ENABLE = 1 << 12;
        const TASK_DESC_128 = 1 << 8;  // If 0, 64-bit
        const CRYPTO_ENABLE = 1 << 1;
        const CQE_ENABLE = 1;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct CqhciCqCtlRegister: u32 {
        const HALT = 1;
        const HALT_ON_EMPTY_QUEUE = 1 << 1;
        const CLEAR_ALL_TASKS = 1 << 8;
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqSendStatusConfiguration1Register(u32);
    impl Debug;
    pub u16, ssc_idle_timer, set_ssc_idle_timer: 15, 0;
    pub u8, ssc_block_counter, set_ssc_block_counter: 19, 16;
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqSendStatusConfiguration2Register(u32);
    impl Debug;
    pub u16, rca, set_rca: 15, 0;
}

impl CqhciCqSendStatusConfiguration2Register {
    pub fn from_rca(rca: u16) -> Self {
        let mut this = Self(0);
        this.set_rca(rca);
        this
    }

    pub fn raw(&self) -> u32 {
        self.0
    }
}

// TODO(https://fxbug.dev/42176727): Add crypto errors.
bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqInterruptStatusRegister(u32);
    impl Debug;
    pub bool, halt_complete, set_halt_complete: 0;
    pub bool, task_complete, set_task_complete: 1;
    pub bool, response_error_detected, set_response_error_detected: 2;
    pub bool, task_cleared, set_task_cleared: 3;
    pub bool, general_crypto_error, set_general_crypto_error: 4;
    pub bool, invalid_crypto_config_error, set_invalid_crypto_config_error: 5;
    pub bool, device_exception_event, set_device_exception_event: 6;
    pub bool, host_controller_fatal_error, _: 7;
}

impl CqhciCqInterruptStatusRegister {
    pub fn is_error(&self) -> bool {
        self.response_error_detected()
            || self.general_crypto_error()
            || self.invalid_crypto_config_error()
            || self.device_exception_event()
            || self.host_controller_fatal_error()
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
    pub fn from_raw(value: u32) -> Self {
        Self(value)
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqInterruptStatusEnableRegister(u32);
    impl Debug;
    pub bool, halt_complete, set_halt_complete: 0;
    pub bool, task_complete, set_task_complete: 1;
    pub bool, response_error_detected, set_response_error_detected: 2;
    pub bool, task_cleared, set_task_cleared: 3;
    pub bool, general_crypto_error, set_general_crypto_error: 4;
    pub bool, invalid_crypto_config_error, set_invalid_crypto_config_error: 5;
    pub bool, device_exception_event, set_device_exception_event: 6;
    pub bool, host_controller_fatal_error, set_host_controller_fatal_error: 7;
}

impl CqhciCqInterruptStatusEnableRegister {
    pub fn disabled() -> Self {
        Self(0)
    }
    pub fn enabled() -> Self {
        Self(0xff)
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
    pub fn from_raw(value: u32) -> Self {
        Self(value)
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqInterruptSignalEnableRegister(u32);
    impl Debug;
    pub bool, halt_complete, set_halt_complete: 0;
    pub bool, task_complete, set_task_complete: 1;
    pub bool, response_error_detected, set_response_error_detected: 2;
    pub bool, task_cleared, set_task_cleared: 3;
    pub bool, general_crypto_error, set_general_crypto_error: 4;
    pub bool, invalid_crypto_config_error, set_invalid_crypto_config_error: 5;
    pub bool, device_exception_event, set_device_exception_event: 6;
    pub bool, host_controller_fatal_error, set_host_controller_fatal_error: 7;
}

impl CqhciCqInterruptSignalEnableRegister {
    pub fn disabled() -> Self {
        Self(0)
    }
    pub fn enabled() -> Self {
        Self(0xff)
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
    pub fn from_raw(value: u32) -> Self {
        Self(value)
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqInterruptCoalescingRegister(u32);
    impl Debug;
    pub u8, ic_timeout_value, set_ic_timeout_value: 6, 0;
    pub bool, ic_timeout_value_write_enable, set_ic_timeout_value_write_enable: 7;
    pub u8, ic_counter_threshold, set_ic_counter_threshold: 12, 8;
    pub bool, ic_counter_threshold_write_enable, set_ic_counter_threshold_write_enable: 15;
    pub bool, ic_counter_timer_reset, set_ic_counter_timer_reset: 16;
    pub bool, ic_status_bit, set_ic_status_bit: 20;
    pub bool, ic_enable, set_ic_enable: 31;
}

impl CqhciCqInterruptCoalescingRegister {
    pub fn disabled() -> Self {
        let mut this = Self(0);
        this.set_ic_enable(false);
        this
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct CqhciCqTaskErrorRegister(u32);
    impl Debug;
    pub u8, response_mode_error_command_index, set_response_mode_error_command_index: 5, 0;
    pub u8, response_mode_error_task_id, set_response_mode_error_task_id: 12, 8;
    pub bool, response_mode_error_fields_valid, set_response_mode_error_fields_valid: 15;
    pub u8, data_transfer_error_command_index, set_data_transfer_error_command_index: 21, 16;
    pub u8, data_transfer_error_task_id, set_data_transfer_error_task_id: 28, 24;
    pub bool, data_transfer_error_fields_valid, set_data_transfer_error_fields_valid: 31;
}

// SDHCI registers (SD Host Controller Simplified Specification V4.20)
// The CQHCI driver can interact with a limited subset of the SDHCI interface to control interrupts.

pub const SDHCI_IS_OFFSET: usize = 0x30;
pub const SDHCI_ISTE_OFFSET: usize = 0x34;
pub const SDHCI_ISGE_OFFSET: usize = 0x38;

bitfield! {
    #[derive(Clone, Copy)]
    pub struct SdhciInterruptStatusRegister(u32);
    impl Debug;
    pub bool, tuning_error, set_tuning_error: 26;
    pub bool, adma_error, set_adma_error: 25;
    pub bool, auto_cmd_error, set_auto_cmd_error: 24;
    pub bool, current_limit_error, set_current_limit_error: 23;
    pub bool, data_end_bit_error, set_data_end_bit_error: 22;
    pub bool, data_crc_error, set_data_crc_error: 21;
    pub bool, data_timeout_error, set_data_timeout_error: 20;
    pub bool, command_index_error, set_command_index_error: 19;
    pub bool, command_end_bit_error, set_command_end_bit_error: 18;
    pub bool, command_crc_error, set_command_crc_error: 17;
    pub bool, command_timeout_error, set_command_timeout_error: 16;
    pub bool, error, set_error: 15;
    /// NB: This is technically reserved in the SDHCI spec.  The CQHCI spec describes this bit.
    pub bool, cqhci_interrupt, set_cqhci_interrupt: 14;
    pub bool, card_interrupt, set_card_interrupt: 8;
    pub bool, buffer_read_ready, set_buffer_read_ready: 5;
    pub bool, buffer_write_ready, set_buffer_write_ready: 4;
    pub bool, transfer_complete, set_transfer_complete: 1;
    pub bool, command_complete, set_command_complete: 0;
}

impl SdhciInterruptStatusRegister {
    pub fn take_cqhci_interrupt(&mut self) -> Self {
        let mut cqhci = Self(0);
        if self.cqhci_interrupt() {
            cqhci.set_cqhci_interrupt(true);
            self.set_cqhci_interrupt(false);
        }
        cqhci
    }

    pub fn is_empty(&self) -> bool {
        self.raw() == 0
    }

    pub fn is_error(&self) -> bool {
        self.tuning_error()
            || self.adma_error()
            || self.auto_cmd_error()
            || self.current_limit_error()
            || self.data_end_bit_error()
            || self.data_crc_error()
            || self.data_timeout_error()
            || self.command_index_error()
            || self.command_end_bit_error()
            || self.command_crc_error()
            || self.command_timeout_error()
            || self.error()
    }
    pub fn from_raw(value: u32) -> Self {
        Self(value)
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct SdhciInterruptStatusEnableRegister(u32);
    impl Debug;
    pub bool, tuning_error, set_tuning_error: 26;
    pub bool, adma_error, set_adma_error: 25;
    pub bool, auto_cmd_error, set_auto_cmd_error: 24;
    pub bool, current_limit_error, set_current_limit_error: 23;
    pub bool, data_end_bit_error, set_data_end_bit_error: 22;
    pub bool, data_crc_error, set_data_crc_error: 21;
    pub bool, data_timeout_error, set_data_timeout_error: 20;
    pub bool, command_index_error, set_command_index_error: 19;
    pub bool, command_end_bit_error, set_command_end_bit_error: 18;
    pub bool, command_crc_error, set_command_crc_error: 17;
    pub bool, command_timeout_error, set_command_timeout_error: 16;
    pub bool, error, set_error: 15;
    pub bool, cqhci_interrupt, set_cqhci_interrupt: 14;
    pub bool, card_interrupt, set_card_interrupt: 8;
    pub bool, buffer_read_ready, set_buffer_read_ready: 5;
    pub bool, buffer_write_ready, set_buffer_write_ready: 4;
    pub bool, transfer_complete, set_transfer_complete: 1;
    pub bool, command_complete, set_command_complete: 0;
}

impl SdhciInterruptStatusEnableRegister {
    pub fn disabled() -> Self {
        Self(0)
    }
    /// Enables the default set of interrupts needed for CQHCI (errors and the cqhci interrupt).
    pub fn enabled() -> Self {
        let mut this = Self::disabled();
        this.enable_errors();
        this.set_cqhci_interrupt(true);
        this
    }
    /// Enables only error interrupts.
    pub fn errors_enabled() -> Self {
        let mut this = Self::disabled();
        this.enable_errors();
        this
    }
    pub fn enable_errors(&mut self) {
        self.set_tuning_error(true);
        self.set_adma_error(true);
        self.set_auto_cmd_error(true);
        self.set_current_limit_error(true);
        self.set_data_end_bit_error(true);
        self.set_data_crc_error(true);
        self.set_data_timeout_error(true);
        self.set_command_index_error(true);
        self.set_command_end_bit_error(true);
        self.set_command_crc_error(true);
        self.set_command_timeout_error(true);
        self.set_error(true)
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
}

bitfield! {
    #[derive(Clone, Copy)]
    pub struct SdhciInterruptSignalEnableRegister(u32);
    impl Debug;
    pub bool, tuning_error, set_tuning_error: 26;
    pub bool, adma_error, set_adma_error: 25;
    pub bool, auto_cmd_error, set_auto_cmd_error: 24;
    pub bool, current_limit_error, set_current_limit_error: 23;
    pub bool, data_end_bit_error, set_data_end_bit_error: 22;
    pub bool, data_crc_error, set_data_crc_error: 21;
    pub bool, data_timeout_error, set_data_timeout_error: 20;
    pub bool, command_index_error, set_command_index_error: 19;
    pub bool, command_end_bit_error, set_command_end_bit_error: 18;
    pub bool, command_crc_error, set_command_crc_error: 17;
    pub bool, command_timeout_error, set_command_timeout_error: 16;
    pub bool, error, set_error: 15;
    pub bool, cqhci_interrupt, set_cqhci_interrupt: 14;
    pub bool, card_interrupt, set_card_interrupt: 8;
    pub bool, buffer_read_ready, set_buffer_read_ready: 5;
    pub bool, buffer_write_ready, set_buffer_write_ready: 4;
    pub bool, transfer_complete, set_transfer_complete: 1;
    pub bool, command_complete, set_command_complete: 0;
}

impl SdhciInterruptSignalEnableRegister {
    pub fn disabled() -> Self {
        Self(0)
    }
    /// Enables the default set of interrupts needed for CQHCI (errors and the cqhci interrupt).
    pub fn enabled() -> Self {
        let mut this = Self::disabled();
        this.enable_errors();
        this.set_cqhci_interrupt(true);
        this
    }
    /// Enables only error interrupts.
    pub fn errors_enabled() -> Self {
        let mut this = Self::disabled();
        this.enable_errors();
        this
    }
    pub fn enable_errors(&mut self) {
        self.set_tuning_error(true);
        self.set_adma_error(true);
        self.set_auto_cmd_error(true);
        self.set_current_limit_error(true);
        self.set_data_end_bit_error(true);
        self.set_data_crc_error(true);
        self.set_data_timeout_error(true);
        self.set_command_index_error(true);
        self.set_command_end_bit_error(true);
        self.set_command_crc_error(true);
        self.set_command_timeout_error(true);
        self.set_error(true)
    }
    pub fn raw(&self) -> u32 {
        self.0
    }
}

/// A snapshot of all crypto registers, for debugging purposes.
pub struct CqhciCryptoRegisterSnapshot {
    pub crnqp: u32,
    pub crnqdun: u32,
    pub crnqis: u32,
    pub crnqie: u32,
    pub crcap: u32,
}

/// A snapshot of all registers, for debugging purposes.
pub struct CqhciRegisterSnapshot {
    pub ver: u32,
    pub caps: u32,
    pub cfg: u32,
    pub ctl: u32,
    pub is: u32,
    pub iste: u32,
    pub isge: u32,
    pub tdlba: u32,
    pub tdlbau: u32,
    pub dbr: u32,
    pub tcn: u32,
    pub dqs: u32,
    pub dpt: u32,
    pub tdpe: u32,
    pub ssc1: u32,
    pub ssc2: u32,
    pub rmem: u32,
    pub terri: u32,
    pub cri: u32,
    pub cra: u32,
    pub hccap: u32,
    pub hccfg: u32,
    pub crypto: Option<CqhciCryptoRegisterSnapshot>,
}

impl std::fmt::Debug for CqhciRegisterSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "CQHCI Registers: ")?;
        writeln!(
            f,
            "   ver {:08x}  caps {:08x}   cfg {:08x}   ctl {:08x}",
            self.ver, self.caps, self.cfg, self.ctl
        )?;
        writeln!(
            f,
            "    is {:08x}  iste {:08x}  isge {:08x} tdlba {:08x}",
            self.is, self.iste, self.isge, self.tdlba
        )?;
        writeln!(
            f,
            "tdlbau {:08x}   dbr {:08x}   tcn {:08x}   dqs {:08x}",
            self.tdlbau, self.dbr, self.tcn, self.dqs
        )?;
        writeln!(
            f,
            "   dpt {:08x}  tdpe {:08x}  ssc1 {:08x}  ssc2 {:08x}",
            self.dpt, self.tdpe, self.ssc1, self.ssc2
        )?;
        writeln!(
            f,
            "  rmem {:08x} terri {:08x}   cri {:08x}   cra {:08x}",
            self.rmem, self.terri, self.cri, self.cra
        )?;
        write!(f, " hccap {:08x} hccfg {:08x}", self.hccap, self.hccfg)?;
        if let Some(crypto) = &self.crypto {
            writeln!(f)?;
            writeln!(f, "CQHCI Crypto Registers: ")?;
            writeln!(
                f,
                " crnqp {:08x}crnqdun {:08x}crnqis {:08x}crnqie {:08x}",
                crypto.crnqp, crypto.crnqdun, crypto.crnqis, crypto.crnqie
            )?;
            write!(f, " crcap {:08x}", crypto.crcap)?;
        }
        Ok(())
    }
}
