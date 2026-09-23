// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Event handlers that filter their input events to more specific events.
//!
//! Filters are powered by the `AsEvent` trait, which extracts a reference to an event sub-type
//! from a super-type (typically a sum type like `enum`s). The handlers composed by filters are
//! only invoked when such a sub-type is received. Otherwise, filters return
//! [`Handled::Unmatched`].
//!
//! [`Handled::Unmatched`]: crate::event::Handled

use fidl_fuchsia_wlan_tap as fidl_tap;

use crate::event::{Handled, Handler, StartMacArgs};

// This event has no fields and is zero-size. This constant is used to return a static reference in
// the corresponding `AsEvent` implementation.
const START_MAC_ARGS: StartMacArgs = StartMacArgs;

pub trait AsEvent<E> {
    fn as_event(&self) -> Option<&E>;
}

impl<E> AsEvent<E> for E {
    fn as_event(&self) -> Option<&E> {
        Some(self)
    }
}

impl AsEvent<fidl_tap::JoinBssArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&fidl_tap::JoinBssArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::JoinBss { args } => Some(args),
            _ => None,
        }
    }
}

impl AsEvent<fidl_tap::StartScanArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&fidl_tap::StartScanArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::StartScan { args } => Some(args),
            _ => None,
        }
    }
}

impl AsEvent<fidl_tap::SetChannelArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&fidl_tap::SetChannelArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::SetChannel { args } => Some(args),
            _ => None,
        }
    }
}

impl AsEvent<fidl_tap::SetCountryArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&fidl_tap::SetCountryArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::SetCountry { args } => Some(args),
            _ => None,
        }
    }
}

impl AsEvent<StartMacArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&StartMacArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::WlanSoftmacStart { .. } => Some(&START_MAC_ARGS),
            _ => None,
        }
    }
}

impl AsEvent<fidl_tap::TxArgs> for fidl_tap::WlantapPhyEvent {
    fn as_event(&self) -> Option<&fidl_tap::TxArgs> {
        match self {
            fidl_tap::WlantapPhyEvent::Tx { args } => Some(args),
            _ => None,
        }
    }
}

/// Filters [`WlantapPhyEvent`]s to [`JoinBssArgs`].
///
/// The composed event handler must accept [`JoinBssArgs`] and is only invoked when such an event
/// is received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`JoinBssArgs`]: fidl_fuchsia_wlan_tap::JoinBssArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_join_bss<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, fidl_tap::JoinBssArgs>,
    E: AsEvent<fidl_tap::JoinBssArgs>,
{
    filter(handler)
}

/// Filters [`WlantapPhyEvent`]s to [`StartScanArgs`].
///
/// The composed event handler must accept [`StartScanArgs`] and is only invoked when such an event
/// is received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`StartScanArgs`]: fidl_fuchsia_wlan_tap::StartScanArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_scan<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, fidl_tap::StartScanArgs>,
    E: AsEvent<fidl_tap::StartScanArgs>,
{
    filter(handler)
}

/// Filters [`WlantapPhyEvent`]s to [`SetChannelArgs`].
///
/// The composed event handler must accept [`SetChannelArgs`] and is only invoked when such an
/// event is received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`SetChannelArgs`]: fidl_fuchsia_wlan_tap::SetChannelArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_set_channel<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, fidl_tap::SetChannelArgs>,
    E: AsEvent<fidl_tap::SetChannelArgs>,
{
    filter(handler)
}

/// Filters [`WlantapPhyEvent`]s to [`SetCountryArgs`].
///
/// The composed event handler must accept [`SetCountryArgs`] and is only invoked when such an
/// event is received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`SetCountryArgs`]: fidl_fuchsia_wlan_tap::SetCountryArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_set_country<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, fidl_tap::SetCountryArgs>,
    E: AsEvent<fidl_tap::SetCountryArgs>,
{
    filter(handler)
}

/// Filters [`WlantapPhyEvent`]s to [`StartMacArgs`].
///
/// The composed event handler must accept [`StartMacArgs`] and is only invoked when such an
/// event is received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`StartMacArgs`]: crate::event::StartMacArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_start_mac<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, StartMacArgs>,
    E: AsEvent<StartMacArgs>,
{
    filter(handler)
}

/// Filters [`WlantapPhyEvent`]s to [`TxArgs`].
///
/// The composed event handler must accept [`TxArgs`] and is only invoked when such an event is
/// received by the filter. Otherwise, the filter returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
/// [`TxArgs`]: fidl_fuchsia_wlan_tap::TxArgs
/// [`WlantapPhyEvent`]: fidl_fuchsia_wlan_tap::WlantapPhyEvent
pub fn on_transmit<H, S, E>(handler: H) -> impl Handler<S, E, Output = H::Output>
where
    H: Handler<S, fidl_tap::TxArgs>,
    E: AsEvent<fidl_tap::TxArgs>,
{
    filter(handler)
}

/// Filters `E1` events to more specific `E2` events.
///
/// It must be possible to extract a reference to an `E2` from an `E1` via `AsEvent` to use this
/// function. The composed event handler must accept `E2` events and is only invoked when the
/// filter receives an `E1` event from which an `E2` event can be retrieved. Otherwise, the filter
/// returns [`Handled::Unmatched`].
///
/// [`Handled::Unmatched`]: crate::event::Handled::Unmatched
fn filter<H, S, E1, E2>(mut handler: H) -> impl Handler<S, E1, Output = H::Output>
where
    H: Handler<S, E2>,
    E1: AsEvent<E2>,
{
    move |state: &mut S, event: &E1| match event.as_event() {
        Some(event) => handler.call(state, event),
        _ => Handled::Unmatched,
    }
}
