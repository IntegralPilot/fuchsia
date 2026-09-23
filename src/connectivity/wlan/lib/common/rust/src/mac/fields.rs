// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;
use wlan_bitfield::bitfield;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// IEEE Std 802.11-2016, 9.2.4.1.3
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct FrameType(pub u8);

impl FrameType {
    pub const MGMT: Self = Self(0);
    pub const CTRL: Self = Self(1);
    pub const DATA: Self = Self(2);
    pub const EXT: Self = Self(3);

    pub fn is_supported(&self) -> bool {
        let FrameType(inner) = *self;
        let FrameType(max) = FrameType::EXT;
        !(inner > max)
    }
}

// IEEE Std 802.11-2016, 9.2.4.1.3
#[bitfield(
    0 cf_ack,
    1 cf_poll,
    2 null,
    3 qos,
    4..=7 _ // subtype is a 4-bit number
)]
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct DataSubtype(pub u8);

// IEEE Std 802.11-2016, 9.2.4.1.3
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct MgmtSubtype(u8);

impl MgmtSubtype {
    pub const ASSOC_REQ: Self = Self(0b0000);
    pub const ASSOC_RESP: Self = Self(0b0001);
    pub const REASSOC_REQ: Self = Self(0b0010);
    pub const REASSOC_RESP: Self = Self(0b0011);
    pub const PROBE_REQ: Self = Self(0b0100);
    pub const PROBE_RESP: Self = Self(0b0101);
    pub const TIMING_AD: Self = Self(0b0110);
    // 0111 reserved
    pub const BEACON: Self = Self(0b1000);
    pub const ATIM: Self = Self(0b1001);
    pub const DISASSOC: Self = Self(0b1010);
    pub const AUTH: Self = Self(0b1011);
    pub const DEAUTH: Self = Self(0b1100);
    pub const ACTION: Self = Self(0b1101);
    pub const ACTION_NO_ACK: Self = Self(0b1110);
    // 1111 reserved

    pub fn is_supported(&self) -> bool {
        let MgmtSubtype(inner) = *self;
        let MgmtSubtype(max) = MgmtSubtype::ACTION_NO_ACK;
        !(inner == 0b0111 || inner == 0b1111 || inner > max)
    }
}

// IEEE Std 802.11-2016, 9.2.4.1.3
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct CtrlSubtype(u8);

impl CtrlSubtype {
    // 0000 - 0011 reserved
    pub const BEAM_FORMING: Self = Self(0b0100);
    pub const VHT_NDP_ANNOUNCE: Self = Self(0b0101);
    pub const CTRL_EXT: Self = Self(0b0110);
    pub const CTRL_WRAP: Self = Self(0b0111);
    pub const BLOCK_ACK: Self = Self(0b1000);
    pub const BLOCK_ACK_REQ: Self = Self(0b1001);
    pub const PS_POLL: Self = Self(0b1010);
    pub const RTS: Self = Self(0b1011);
    pub const CTS: Self = Self(0b1100);
    pub const ACK: Self = Self(0b1101);
    pub const CF_END: Self = Self(0b1110);
    pub const CF_END_ACK: Self = Self(0b1111);
}

/// The power management state of a station.
///
/// Represents the possible power states from IEEE-802.11-2016, 11.2.7.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PowerState(bool);

impl PowerState {
    /// The awake power management state. When in this state, stations are expected to be reliable
    /// and handle transmitted frames.
    pub const AWAKE: Self = Self(false);
    /// The doze power management state. When in this state, stations may be less reliable and APs
    /// should typically buffer frames.
    pub const DOZE: Self = Self(true);
}

// IEEE Std 802.11-2016, 9.2.4.1.1
#[bitfield(
    0..=1   protocol_version,
    2..=3   frame_type as FrameType(u8),
    4..=7   union {
                frame_subtype,
                mgmt_subtype as MgmtSubtype(u8),
                data_subtype as DataSubtype(u8),
                ctrl_subtype as CtrlSubtype(u8),
            },
    8       to_ds,
    9       from_ds,
    10      more_fragments,
    11      retry,
    12      power_mgmt as PowerState(bool),
    13      more_data,
    14      protected,
    15      htc_order
)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, PartialEq, Eq, Clone, Copy)]
#[repr(C)]
pub struct FrameControl(pub u16);

impl FrameControl {
    pub fn is_mgmt(&self) -> bool {
        self.frame_type() == FrameType::MGMT
    }
    pub fn is_ctrl(&self) -> bool {
        self.frame_type() == FrameType::CTRL
    }
    pub fn is_data(&self) -> bool {
        self.frame_type() == FrameType::DATA
    }
}

// IEEE Std 802.11-2016, 9.2.4.4
#[bitfield(
    0..=3   frag_num,
    4..=15  seq_num,
)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, PartialEq, Eq, Clone, Copy)]
#[repr(C)]
pub struct SequenceControl(pub u16);

// IEEE Std 802.11-2016, 9.2.4.6
#[bitfield(
    0       vht,
    1..=29  middle, // see 9.2.4.6.2 for HT and 9.2.4.6.3 for VHT
    30      ac_constraint,
    31      rdg_more_ppdu,
)]
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Copy, Clone, PartialEq, Eq)]
pub struct HtControl(pub u32);

#[derive(PartialEq, Eq)]
pub struct Presence<T: ?Sized>(bool, PhantomData<T>);

impl<T: ?Sized> Presence<T> {
    pub fn from_bool(present: bool) -> Self {
        Self(present, PhantomData)
    }
}

pub trait OptionalField {
    const PRESENT: Presence<Self> = Presence::<Self>(true, PhantomData);
    const ABSENT: Presence<Self> = Presence::<Self>(false, PhantomData);
}
impl<T: ?Sized> OptionalField for T {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WlanGi(pub u8);

// Guart intervals
impl WlanGi {
    pub const G_800NS: Self = Self(0x1); // all 802.11 phy
    pub const G_400NS: Self = Self(0x2); // 802.11n/ac
    pub const G_200NS: Self = Self(0x4); // 802.11n/ac
    pub const G_3200NS: Self = Self(0x10); // 802.11ax
    pub const G_1600NS: Self = Self(0x20); // 802.11ax
}

impl std::ops::BitAnd for WlanGi {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for WlanGi {
    fn bitand_assign(&mut self, rhs: Self) {
        *self = Self(self.0 & rhs.0)
    }
}

impl std::ops::BitOr for WlanGi {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for WlanGi {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = Self(self.0 | rhs.0)
    }
}

impl std::ops::BitXor for WlanGi {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self {
        Self(self.0 ^ rhs.0)
    }
}

impl std::ops::BitXorAssign for WlanGi {
    fn bitxor_assign(&mut self, rhs: Self) {
        *self = Self(self.0 ^ rhs.0)
    }
}
