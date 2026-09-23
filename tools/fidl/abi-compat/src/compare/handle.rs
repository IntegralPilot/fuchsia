// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use std::fmt::Display;

#[allow(unused)]
#[derive(PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub enum HandleType {
    // None = 0,
    Process = 1,
    Thread = 2,
    VMO = 3,
    Channel = 4,
    Event = 5,
    Port = 6,
    Interrupt = 9,
    PCIDevice = 11,
    Debuglog = 12,
    Socket = 14,
    Resource = 15,
    Eventpair = 16,
    Job = 17,
    VMAR = 18,
    FIFO = 19,
    Guest = 20,
    VCPU = 21,
    Timer = 22,
    IOMMU = 23,
    BTI = 24,
    Profile = 25,
    PMT = 26,
    SuspendToken = 27,
    Pager = 28,
    Exception = 29,
    Clock = 30,
    Stream = 31,
    MSI = 32,
    Counter = 34,
}

impl Display for HandleType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandleType::Process => write!(f, "zx.Handle:PROCESS"),
            HandleType::Thread => write!(f, "zx.Handle:THREAD"),
            HandleType::VMO => write!(f, "zx.Handle:VMO"),
            HandleType::Channel => write!(f, "zx.Handle:CHANNEL"),
            HandleType::Event => write!(f, "zx.Handle:EVENT"),
            HandleType::Port => write!(f, "zx.Handle:PORT"),
            HandleType::Interrupt => write!(f, "zx.Handle:INTERRUPT"),
            HandleType::PCIDevice => write!(f, "zx.Handle:PCIDEVICE"),
            HandleType::Debuglog => write!(f, "zx.Handle:DEBUGLOG"),
            HandleType::Socket => write!(f, "zx.Handle:SOCKET"),
            HandleType::Resource => write!(f, "zx.Handle:RESOURCE"),
            HandleType::Eventpair => write!(f, "zx.Handle:EVENTPAIR"),
            HandleType::Job => write!(f, "zx.Handle:JOB"),
            HandleType::VMAR => write!(f, "zx.Handle:VMAR"),
            HandleType::FIFO => write!(f, "zx.Handle:FIFO"),
            HandleType::Guest => write!(f, "zx.Handle:GUEST"),
            HandleType::VCPU => write!(f, "zx.Handle:VCPU"),
            HandleType::Timer => write!(f, "zx.Handle:TIMER"),
            HandleType::IOMMU => write!(f, "zx.Handle:IOMMU"),
            HandleType::BTI => write!(f, "zx.Handle:BTI"),
            HandleType::Profile => write!(f, "zx.Handle:PROFILE"),
            HandleType::PMT => write!(f, "zx.Handle:PMT"),
            HandleType::SuspendToken => write!(f, "zx.Handle:SUSPEND_TOKEN"),
            HandleType::Pager => write!(f, "zx.Handle:PAGER"),
            HandleType::Exception => write!(f, "zx.Handle:EXCEPTION"),
            HandleType::Clock => write!(f, "zx.Handle:CLOCK"),
            HandleType::Stream => write!(f, "zx.Handle:STREAM"),
            HandleType::MSI => write!(f, "zx.Handle:MSI"),
            HandleType::Counter => write!(f, "zx.Handle:COUNTER"),
        }
    }
}

bitflags! {
    #[derive(PartialEq, PartialOrd, Debug, Clone, Copy)]
    pub struct HandleRights : u32 {
        /// No rights.
        const NONE = 0;
        /// Duplicate right.
        const DUPLICATE = 1 << 0;
        /// Transfer right.
        const TRANSFER = 1 << 1;
        /// Read right.
        const READ = 1 << 2;
        /// Write right.
        const WRITE = 1 << 3;
        /// Execute right.
        const EXECUTE = 1 << 4;
        /// Map right.
        const MAP = 1 << 5;
        /// Get Property right.
        const GET_PROPERTY = 1 << 6;
        /// Set Property right.
        const SET_PROPERTY = 1 << 7;
        /// Enumerate right.
        const ENUMERATE = 1 << 8;
        /// Destroy right.
        const DESTROY = 1 << 9;
        /// Set Policy right.
        const SET_POLICY = 1 << 10;
        /// Get Policy right.
        const GET_POLICY = 1 << 11;
        /// Signal right.
        const SIGNAL = 1 << 12;
        /// Signal Peer right.
        const SIGNAL_PEER = 1 << 13;
        /// Wait right.
        const WAIT = 1 << 14;
        /// Inspect right.
        const INSPECT = 1 << 15;
        /// Manage Job right.
        const MANAGE_JOB = 1 << 16;
        /// Manage Process right.
        const MANAGE_PROCESS = 1 << 17;
        /// Manage Thread right.
        const MANAGE_THREAD = 1 << 18;
        /// Apply Profile right.
        const APPLY_PROFILE = 1 << 19;
        /// Manage Socket right.
        const MANAGE_SOCKET = 1 << 20;

        /// Same rights.
        const SAME_RIGHTS = 1 << 31;
    }
}
impl Default for HandleRights {
    fn default() -> Self {
        // In FIDL if you don't specify rights you get SAME_RIGHTS.
        Self::SAME_RIGHTS
    }
}
