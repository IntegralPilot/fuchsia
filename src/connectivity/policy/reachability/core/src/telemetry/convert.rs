// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use network_policy_metrics_registry as metrics;

use crate::{IpVersions, State};

pub(crate) fn convert_yes_no_dim(
    b: bool,
) -> metrics::ReachabilityGlobalSnapshotDurationMetricDimensionInternetAvailable {
    if b {
        metrics::ReachabilityGlobalSnapshotDurationMetricDimensionInternetAvailable::Yes
    } else {
        metrics::ReachabilityGlobalSnapshotDurationMetricDimensionInternetAvailable::No
    }
}

pub(crate) fn convert_route_config(
    global_system_state: &IpVersions<Option<State>>,
) -> Option<metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig> {
    match (global_system_state.ipv4.is_some(), global_system_state.ipv6.is_some()) {
        (true, true) => {
            Some(metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig::Ipv4Ipv6)
        }
        (true, false) => {
            Some(metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig::Ipv4Only)
        }
        (false, true) => {
            Some(metrics::ReachabilityGlobalSnapshotDurationMetricDimensionRouteConfig::Ipv6Only)
        }
        (false, false) => None,
    }
}

pub(crate) fn convert_default_route(
    has_ipv4: bool,
    has_ipv6: bool,
) -> Option<metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute> {
    match (has_ipv4, has_ipv6) {
        (true, true) => Some(
            metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv4Ipv6,
        ),
        (true, false) => Some(
            metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv4Only,
        ),
        (false, true) => Some(
            metrics::ReachabilityGlobalDefaultRouteDurationMetricDimensionDefaultRoute::Ipv6Only,
        ),
        (false, false) => None,
    }
}
