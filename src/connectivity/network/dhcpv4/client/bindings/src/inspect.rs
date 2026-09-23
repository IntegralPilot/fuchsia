// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;

use derivative::Derivative;
use dhcp_client_core::client::DebugLogPrefix;
use diagnostics_traits::Inspector;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use net_types::SpecifiedAddr;

pub(crate) struct Inspect {
    inner: Mutex<InspectInner>,
}

impl Inspect {
    pub(crate) fn new() -> Self {
        Self { inner: Mutex::new(InspectInner::new()) }
    }

    pub(crate) fn record(&self, inspector: &mut impl Inspector) {
        let inner = self.inner.lock();
        inner.record(inspector);
    }

    pub(crate) fn update(
        &self,
        state: StateInspect,
        lease: LeaseChangeInspect,
        debug_log_prefix: DebugLogPrefix,
    ) {
        let mut inner = self.inner.lock();
        inner.update_state(state);
        inner.update_lease(lease, debug_log_prefix);
    }
}

struct InspectInner {
    current_state: StateInspect,
    current_lease: Option<LeaseInspect>,
    state_history: LengthLimitedVecDeque<StateHistoryInspect>,
    lease_history: LengthLimitedVecDeque<LeaseInspect>,
}

const MAX_HISTORY_LENGTH: usize = 10;

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
struct LengthLimitedVecDeque<T> {
    inner: VecDeque<T>,
}

impl<T> LengthLimitedVecDeque<T> {
    fn push(&mut self, value: T) {
        if self.inner.len() >= MAX_HISTORY_LENGTH {
            let _: Option<T> = self.inner.pop_front();
        }
        self.inner.push_back(value)
    }
}

impl InspectInner {
    fn new() -> Self {
        Self {
            current_state: StateInspect::new(),
            current_lease: None,
            state_history: Default::default(),
            lease_history: Default::default(),
        }
    }

    fn record(&self, inspector: &mut impl Inspector) {
        let Self { current_state, current_lease, state_history, lease_history } = self;
        inspector.record_child("CurrentState", |inspector| {
            current_state.record(inspector);
        });
        if let Some(current_lease) = current_lease {
            inspector.record_child("CurrentLease", |inspector| {
                current_lease.record(inspector);
            });
        }
        inspector.record_child("StateHistory", |inspector| {
            for entry in state_history.inner.iter() {
                inspector.record_unnamed_child(|inspector| {
                    entry.record(inspector);
                });
            }
        });
        inspector.record_child("LeaseHistory", |inspector| {
            for entry in lease_history.inner.iter() {
                inspector.record_unnamed_child(|inspector| {
                    entry.record(inspector);
                });
            }
        });
    }

    fn update_state(&mut self, state: StateInspect) {
        let old_state_inspect = std::mem::replace(&mut self.current_state, state);
        self.state_history.push(old_state_inspect.into());
    }

    fn update_lease(&mut self, lease: LeaseChangeInspect, debug_log_prefix: DebugLogPrefix) {
        let Self { current_lease, lease_history, .. } = self;
        match lease {
            LeaseChangeInspect::NoChange => (),
            LeaseChangeInspect::LeaseDropped => {
                let Some(current_lease) = current_lease.take() else {
                    log::error!(
                        "{debug_log_prefix} recording lease drop in \
                        inspect history with no current lease"
                    );
                    return;
                };
                lease_history.push(current_lease);
            }
            LeaseChangeInspect::LeaseAdded { start_time, prefix_len, properties } => {
                if let Some(prev_lease) = current_lease.take() {
                    lease_history.push(prev_lease);
                }
                *current_lease =
                    Some(LeaseInspect { start_time, renewed_time: None, prefix_len, properties });
            }
            LeaseChangeInspect::LeaseRenewed { renewed_time, properties } => {
                let Some(prev_lease) = current_lease.as_mut() else {
                    log::error!(
                        "{debug_log_prefix} recording lease renewal in \
                        inspect history with no current lease"
                    );
                    return;
                };
                let start_time = prev_lease.start_time;
                let prefix_len = prev_lease.prefix_len;
                *prev_lease = LeaseInspect {
                    start_time,
                    renewed_time: Some(renewed_time),
                    prefix_len,
                    // Take the properties from the renewed version in case they
                    // changed with the renewal.
                    properties,
                };
            }
        }
    }
}

pub(crate) struct StateInspect {
    pub(crate) state: dhcp_client_core::client::State<fasync::MonotonicInstant>,
    pub(crate) time: fasync::MonotonicInstant,
}

impl StateInspect {
    fn new() -> Self {
        Self {
            state: dhcp_client_core::client::State::default(),
            time: fasync::MonotonicInstant::now(),
        }
    }

    fn record(&self, inspector: &mut impl Inspector) {
        let StateInspect { state, time } = self;
        inspector.record_inspectable_value("State", state);
        inspector.record_instant(diagnostics_traits::instant_property_name!("Entered"), time);
    }
}

struct StateHistoryInspect {
    time: fasync::MonotonicInstant,
    state_name: &'static str,
}

impl StateHistoryInspect {
    fn record(&self, inspector: &mut impl Inspector) {
        let Self { state_name: state, time } = self;
        inspector.record_str("State", state);
        inspector.record_instant(diagnostics_traits::instant_property_name!("Entered"), time);
    }
}

impl From<StateInspect> for StateHistoryInspect {
    fn from(state_inspect: StateInspect) -> Self {
        let StateInspect { state, time } = state_inspect;
        let state_name = state.state_name();
        Self { time, state_name }
    }
}

pub(crate) enum LeaseChangeInspect {
    NoChange,
    LeaseDropped,
    LeaseAdded {
        start_time: fasync::MonotonicInstant,
        /// `prefix_len` is only recorded when a lease is initially acquired
        /// because we do not currently support updating the prefix length
        /// attached to an address that we have installed.
        prefix_len: u8,
        properties: LeaseInspectProperties,
    },
    LeaseRenewed {
        renewed_time: fasync::MonotonicInstant,
        properties: LeaseInspectProperties,
    },
}

#[derive(Clone)]
pub(crate) struct LeaseInspectProperties {
    pub(crate) ip_address: SpecifiedAddr<net_types::ip::Ipv4Addr>,
    pub(crate) lease_length: fasync::MonotonicDuration,
    pub(crate) dns_servers: Option<Vec<net_types::ip::Ipv4Addr>>,
    pub(crate) routers: Option<Vec<net_types::ip::Ipv4Addr>>,
}

#[derive(Clone)]
pub(crate) struct LeaseInspect {
    pub(crate) start_time: fasync::MonotonicInstant,
    pub(crate) renewed_time: Option<fasync::MonotonicInstant>,
    pub(crate) prefix_len: u8,
    properties: LeaseInspectProperties,
}

impl LeaseInspect {
    fn record(&self, inspector: &mut impl Inspector) {
        let Self {
            start_time,
            renewed_time,
            prefix_len,
            properties: LeaseInspectProperties { ip_address, lease_length, dns_servers, routers },
        } = self;
        inspector.record_ip_addr("IpAddress", **ip_address);
        inspector.record_instant(diagnostics_traits::instant_property_name!("Start"), start_time);
        match renewed_time {
            Some(renewed_time) => {
                inspector.record_instant(
                    diagnostics_traits::instant_property_name!("Renewed"),
                    renewed_time,
                );
            }
            None => {
                inspector.record_str(
                    diagnostics_traits::instant_property_name!("Renewed").into(),
                    "None",
                );
            }
        }
        inspector.record_int("LeaseLengthSecs", lease_length.into_seconds());
        inspector.record_uint("PrefixLen", *prefix_len);

        inspector.record_child("DnsServers", |inspector| {
            for (i, server) in dns_servers.iter().flatten().enumerate() {
                inspector.record_display(&format!("{i}"), server);
            }
        });

        inspector.record_child("Routers", |inspector| {
            for (i, router) in routers.iter().flatten().enumerate() {
                inspector.record_display(&format!("{i}"), router);
            }
        });
    }
}
