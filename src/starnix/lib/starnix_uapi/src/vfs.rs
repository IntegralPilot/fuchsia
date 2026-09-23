// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use linux_uapi as uapi;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const EPOLLWAKEUP: u32 = 1 << 29;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct FdEvents: u32 {
        const POLLIN = uapi::POLLIN;
        const POLLPRI = uapi::POLLPRI;
        const POLLOUT = uapi::POLLOUT;
        const POLLERR = uapi::POLLERR;
        const POLLHUP = uapi::POLLHUP;
        const POLLNVAL = uapi::POLLNVAL;
        const POLLRDNORM = uapi::POLLRDNORM;
        const POLLRDBAND = uapi::POLLRDBAND;
        const POLLWRNORM = uapi::POLLWRNORM;
        const POLLWRBAND = uapi::POLLWRBAND;
        const POLLMSG = uapi::POLLMSG;
        const POLLREMOVE = uapi::POLLREMOVE;
        const POLLRDHUP = uapi::POLLRDHUP;
        const EPOLLET = EPOLLET;
        const EPOLLONESHOT = EPOLLONESHOT;
        const EPOLLWAKEUP = EPOLLWAKEUP;
    }
}

impl FdEvents {
    /// Build events from the given value, truncating any bits that do not correspond to an event.
    pub fn from_u64(value: u64) -> Self {
        Self::from_bits_truncate((value & (u32::MAX as u64)) as u32)
    }

    /// This function adds `POLLRDNORM` and `POLLWRNORM` to the FdEvents return
    /// from the FileOps because these FdEvents are equivalent to `POLLIN` and
    /// `POLLOUT`, respectively, in the Linux UAPI. For example, polling with
    /// events = POLLIN | POLLRDNORM will always return the same value for both
    /// bits in revents.
    ///
    /// See https://man7.org/linux/man-pages/man2/poll.2.html
    pub fn add_equivalent_fd_events(self) -> Self {
        let mut events = self;
        if events.contains(FdEvents::POLLIN) {
            events |= FdEvents::POLLRDNORM;
        }
        if events.contains(FdEvents::POLLOUT) {
            events |= FdEvents::POLLWRNORM;
        }
        events
    }
}

#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
pub struct EpollEvent(uapi::epoll_event);

impl EpollEvent {
    pub fn new(events: FdEvents, data: u64) -> Self {
        Self(uapi::epoll_event { events: events.bits(), data, ..Default::default() })
    }

    pub fn events(&self) -> FdEvents {
        FdEvents::from_bits_retain(self.0.events)
    }

    pub fn ignore(&mut self, events_to_ignore: FdEvents) {
        let mut previous_events = self.events();
        previous_events.remove(events_to_ignore);
        self.0.events = previous_events.bits();
    }

    pub fn data(&self) -> u64 {
        self.0.data
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ResolveFlags: u32 {
        const NO_XDEV = uapi::RESOLVE_NO_XDEV;
        const NO_MAGICLINKS = uapi::RESOLVE_NO_MAGICLINKS;
        const NO_SYMLINKS = uapi::RESOLVE_NO_SYMLINKS;
        const BENEATH = uapi::RESOLVE_BENEATH;
        const IN_ROOT = uapi::RESOLVE_IN_ROOT;
        const CACHED = uapi::RESOLVE_CACHED;
    }
}
