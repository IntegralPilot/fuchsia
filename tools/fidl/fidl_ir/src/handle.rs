// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HandleRights(u32);

bitflags! {
    impl HandleRights: u32 {
        const DUPLICATE = 1 << 0;
        const TRANSFER = 1 << 1;
        const READ = 1 << 2;
        const WRITE = 1 << 3;
        const EXECUTE = 1 << 4;
        const MAP = 1 << 5;
        const GET_PROPERTY = 1 << 6;
        const SET_PROPERTY = 1 << 7;
        const ENUMERATE = 1 << 8;
        const DESTROY = 1 << 9;
        const SET_POLICY = 1 << 10;
        const GET_POLICY = 1 << 11;
        const SIGNAL = 1 << 12;
        const SIGNAL_PEER = 1 << 13;
        const WAIT = 1 << 14;
        const INSPECT = 1 << 15;
        const MANAGE_JOB = 1 << 16;
        const MANAGE_PROCESS = 1 << 17;
        const MANAGE_THREAD = 1 << 18;
        const APPLY_PROFILE = 1 << 19;

        const SAME_RIGHTS = 1 << 31;

        const BASIC_RIGHTS = {
            Self::TRANSFER.bits()
            | Self::DUPLICATE.bits()
            | Self::WAIT.bits()
            | Self::INSPECT.bits()
        };
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandleSubtype {
    #[serde(rename = "handle")]
    None = 0,
    Process = 1,
    Thread = 2,
    Vmo = 3,
    Channel = 4,
    Event = 5,
    Port = 6,
    Interrupt = 9,
    PciDevice = 11,
    DebugLog = 12,
    Socket = 14,
    Resource = 15,
    EventPair = 16,
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
