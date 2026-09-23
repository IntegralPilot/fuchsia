// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zx` by zither, a Fuchsia platform tool.

use zerocopy::{Immutable, IntoBytes, TryFromBytes};

pub type Status = i32;

pub type Time = i64;

pub type InstantMono = i64;

pub type InstantBoot = i64;

pub type Ticks = i64;

pub type InstantMonoTicks = i64;

pub type InstantBootTicks = i64;

pub type Duration = i64;

pub type DurationMono = i64;

pub type DurationBoot = i64;

pub type Koid = u64;

pub type Off = u64;

pub type Signals = u32;

pub const CHANNEL_MAX_MSG_BYTES: u64 = 65536;

pub const CHANNEL_MAX_MSG_HANDLES: u64 = 64;

pub const IOB_MAX_REGIONS: u64 = 64;

pub const MAX_NAME_LEN: u64 = 32;

pub const MAX_CPUS: u64 = 512;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum ObjType {
    None = 0,
    Process = 1,
    Thread = 2,
    Vmo = 3,
    Channel = 4,
    Event = 5,
    Port = 6,
    Interrupt = 9,
    PciDevice = 11,
    Log = 12,
    Socket = 14,
    Resource = 15,
    Eventpair = 16,
    Job = 17,
    Vmar = 18,
    Fifo = 19,
    Guest = 20,
    Vcpu = 21,
    Timer = 22,
    Iommu = 23,
    Bti = 24,
    Profile = 25,
    Pmt = 26,
    SuspendToken = 27,
    Pager = 28,
    Exception = 29,
    Clock = 30,
    Stream = 31,
    Msi = 32,
    Iob = 33,
    Counter = 34,
}

impl ObjType {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::None),

            1 => Some(Self::Process),

            2 => Some(Self::Thread),

            3 => Some(Self::Vmo),

            4 => Some(Self::Channel),

            5 => Some(Self::Event),

            6 => Some(Self::Port),

            9 => Some(Self::Interrupt),

            11 => Some(Self::PciDevice),

            12 => Some(Self::Log),

            14 => Some(Self::Socket),

            15 => Some(Self::Resource),

            16 => Some(Self::Eventpair),

            17 => Some(Self::Job),

            18 => Some(Self::Vmar),

            19 => Some(Self::Fifo),

            20 => Some(Self::Guest),

            21 => Some(Self::Vcpu),

            22 => Some(Self::Timer),

            23 => Some(Self::Iommu),

            24 => Some(Self::Bti),

            25 => Some(Self::Profile),

            26 => Some(Self::Pmt),

            27 => Some(Self::SuspendToken),

            28 => Some(Self::Pager),

            29 => Some(Self::Exception),

            30 => Some(Self::Clock),

            31 => Some(Self::Stream),

            32 => Some(Self::Msi),

            33 => Some(Self::Iob),

            34 => Some(Self::Counter),

            _ => None,
        }
    }
}

impl From<ObjType> for u32 {
    fn from(val: ObjType) -> Self {
        val as Self
    }
}

pub type Handle = u32;
