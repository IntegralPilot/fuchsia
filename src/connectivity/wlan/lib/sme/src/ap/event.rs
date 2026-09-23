// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ieee80211::MacAddr;
use wlan_common::timer::TimeoutDuration;

// https://fxbug.dev/42131271 exposed the issue that longer timeout is needed for starting AP while the
// client iface is scanning, this is not a magic number, but a number we chose after
// discussion.
pub const START_TIMEOUT_SECONDS: i64 = 10;
pub const STOP_TIMEOUT_SECONDS: i64 = 10;

#[derive(Debug, Clone)]
pub enum Event {
    Sme { event: SmeEvent },
    Client { addr: MacAddr, event: ClientEvent },
}

impl TimeoutDuration for Event {
    fn timeout_duration(&self) -> zx::MonotonicDuration {
        match self {
            Event::Sme { event } => event.timeout_duration(),
            Event::Client { event, .. } => event.timeout_duration(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SmeEvent {
    StartTimeout,
    StopTimeout,
}

impl SmeEvent {
    pub fn timeout_duration(&self) -> zx::MonotonicDuration {
        match self {
            SmeEvent::StartTimeout => zx::MonotonicDuration::from_seconds(START_TIMEOUT_SECONDS),
            SmeEvent::StopTimeout => zx::MonotonicDuration::from_seconds(STOP_TIMEOUT_SECONDS),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RsnaTimeout {
    Request,
    Negotiation,
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    AssociationTimeout,
    RsnaTimeout(RsnaTimeout),
}

impl ClientEvent {
    pub fn timeout_duration(&self) -> zx::MonotonicDuration {
        match self {
            // We only use schedule_at, so we ignore these timeout durations here.
            // TODO(tonyy): Switch everything to use schedule_at, maybe?
            ClientEvent::AssociationTimeout => zx::MonotonicDuration::from_seconds(0),
            ClientEvent::RsnaTimeout { .. } => zx::MonotonicDuration::from_seconds(0),
        }
    }
}
