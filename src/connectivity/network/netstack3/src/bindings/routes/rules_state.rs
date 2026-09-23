// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bindings::routes::watcher::{
    FidlWatcherEvent, Update, UpdateDispatcher, WatcherEvent, WatcherInterest,
};
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext::rules::FidlRuleIpExt;
use fidl_fuchsia_net_routes_ext::{self as fnet_routes_ext, SliceResponder as _};
use fnet_routes_ext::rules::{InstalledRule, RuleEvent};
use net_types::ip::Ip;

#[derive(Debug)]
pub(crate) struct RuleInterest;

impl<I: Ip> WatcherInterest<RuleEvent<I>> for RuleInterest {
    fn has_interest_in(&self, _event: &RuleEvent<I>) -> bool {
        true
    }
}

pub(crate) type RuleUpdateDispatcher<I> = UpdateDispatcher<RuleEvent<I>, RuleInterest>;

impl<I: Ip> WatcherEvent for RuleEvent<I> {
    type Resource = InstalledRule<I>;

    const MAX_EVENTS: usize = fnet_routes::MAX_EVENTS as usize;

    const IDLE: Self = Self::Idle;

    fn existing(installed: Self::Resource) -> Self {
        Self::Existing(installed)
    }
}

impl<I: Ip> From<Update<RuleEvent<I>>> for RuleEvent<I> {
    fn from(update: Update<RuleEvent<I>>) -> Self {
        match update {
            Update::Added(added) => Self::Added(added),
            Update::Removed(removed) => Self::Removed(removed),
        }
    }
}

impl<I: FidlRuleIpExt> FidlWatcherEvent for RuleEvent<I> {
    type WatcherMarker = I::RuleWatcherMarker;

    fn respond_to_watch_request(
        req: fidl::endpoints::Request<Self::WatcherMarker>,
        events: Vec<Self>,
    ) -> Result<(), fidl::Error> {
        match I::into_rule_watcher_request(req) {
            fidl_fuchsia_net_routes_ext::rules::RuleWatcherRequest::Watch { responder } => {
                let events = events.into_iter().map(From::from).collect::<Vec<_>>();
                responder.send(&events[..])
            }
        }
    }
}
