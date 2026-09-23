// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use num_derive::FromPrimitive;

/// This structure represents a Thread network info in the history tracker report.
///
/// Functional equivalent of [`otsys::otHistoryTrackerNetworkInfo`](crate::otsys::otHistoryTrackerNetworkInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct HistoryTrackerNetworkInfo(pub otHistoryTrackerNetworkInfo);

impl_ot_castable!(HistoryTrackerNetworkInfo, otHistoryTrackerNetworkInfo);

impl HistoryTrackerNetworkInfo {
    /// Returns the device Role.
    pub fn role(&self) -> DeviceRole {
        DeviceRole::from(self.0.mRole)
    }

    /// Returns the device's MLE link mode.
    pub fn mode(&self) -> LinkModeConfig {
        LinkModeConfig::from(self.0.mMode)
    }

    /// Returns the device's RLOC16.
    pub fn rloc16(&self) -> u16 {
        self.0.mRloc16
    }

    /// Returns the Thread network partition ID.
    pub fn partition_id(&self) -> u32 {
        self.0.mPartitionId
    }
}

/// Defines the events in a neighbor info (i.e. whether neighbor is added, removed, or changed).
///
/// Functional equivalent of [`otsys::otHistoryTrackerNeighborEvent`](crate::otsys::otHistoryTrackerNeighborEvent).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, FromPrimitive)]
#[allow(missing_docs)]
pub enum HistoryTrackerNeighborEvent {
    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_ADDED`](crate::otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_ADDED).
    Added = OT_HISTORY_TRACKER_NEIGHBOR_EVENT_ADDED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_REMOVED`](crate::otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_REMOVED).
    Removed = OT_HISTORY_TRACKER_NEIGHBOR_EVENT_REMOVED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_CHANGED`](crate::otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_CHANGED).
    Changed = OT_HISTORY_TRACKER_NEIGHBOR_EVENT_CHANGED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_RESTORING`](crate::otsys::OT_HISTORY_TRACKER_NEIGHBOR_EVENT_RESTORING).
    Restoring = OT_HISTORY_TRACKER_NEIGHBOR_EVENT_RESTORING as isize,
}

impl From<otHistoryTrackerNeighborEvent> for HistoryTrackerNeighborEvent {
    fn from(x: otHistoryTrackerNeighborEvent) -> Self {
        use num::FromPrimitive;
        Self::from_u32(x)
            .unwrap_or_else(|| panic!("Unknown otHistoryTrackerNeighborEvent value: {x}"))
    }
}

/// This structure represents a Thread neighbor info in the history tracker report.
///
/// Functional equivalent of [`otsys::otHistoryTrackerNeighborInfo`](crate::otsys::otHistoryTrackerNeighborInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct HistoryTrackerNeighborInfo(pub otHistoryTrackerNeighborInfo);

impl_ot_castable!(HistoryTrackerNeighborInfo, otHistoryTrackerNeighborInfo);

impl HistoryTrackerNeighborInfo {
    /// Returns Neighbor's Extended Address.
    pub fn ext_address(&self) -> ExtAddress {
        ExtAddress(self.0.mExtAddress)
    }

    /// Returns the Neighbor's RLOC16.
    pub fn rloc16(&self) -> u16 {
        self.0.mRloc16
    }

    /// Returns the Average RSSI of rx frames from neighbor at the time of recording entry.
    pub fn avg_rssi(&self) -> i8 {
        self.0.mAverageRssi
    }

    /// Returns the neighbor event (`OT_HISTORY_TRACKER_NEIGHBOR_EVENT_*` enumeration).
    pub fn event(&self) -> HistoryTrackerNeighborEvent {
        HistoryTrackerNeighborEvent::from(self.0.mEvent() as u32)
    }

    /// Returns true if the neighbor has its receiver on when not transmitting.
    pub fn rx_on_while_idle(&self) -> bool {
        self.0.mRxOnWhenIdle()
    }

    /// Returns true if the neighbor is a Full Thread Device.
    pub fn full_thread_device(&self) -> bool {
        self.0.mFullThreadDevice()
    }

    /// Returns true if the neighbor requires the full Network Data.
    pub fn full_network_data(&self) -> bool {
        self.0.mFullNetworkData()
    }

    /// Returns true if the neighbor is a child, otherwise, is a router.
    pub fn is_child(&self) -> bool {
        self.0.mIsChild()
    }
}

/// Defines the events in a router info (i.e. whether router is added, removed, or changed).
///
/// Functional equivalent of [`otsys::otHistoryTrackerRouterEvent`](crate::otsys::otHistoryTrackerRouterEvent).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, FromPrimitive)]
#[allow(missing_docs)]
pub enum HistoryTrackerRouterEvent {
    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_ADDED`](crate::otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_ADDED).
    Added = OT_HISTORY_TRACKER_ROUTER_EVENT_ADDED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_REMOVED`](crate::otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_REMOVED).
    Removed = OT_HISTORY_TRACKER_ROUTER_EVENT_REMOVED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_NEXT_HOP_CHANGED`](crate::otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_NEXT_HOP_CHANGED).
    NextHopChanged = OT_HISTORY_TRACKER_ROUTER_EVENT_NEXT_HOP_CHANGED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_COST_CHANGED`](crate::otsys::OT_HISTORY_TRACKER_ROUTER_EVENT_COST_CHANGED).
    PathCostChanged = OT_HISTORY_TRACKER_ROUTER_EVENT_COST_CHANGED as isize,
}

impl From<otHistoryTrackerRouterEvent> for HistoryTrackerRouterEvent {
    fn from(x: otHistoryTrackerRouterEvent) -> Self {
        use num::FromPrimitive;
        Self::from_u32(x)
            .unwrap_or_else(|| panic!("Unknown otHistoryTrackerRouterEvent value: {x}"))
    }
}

/// This structure represents a router table entry event in the history tracker report.
///
/// Functional equivalent of [`otsys::otHistoryTrackerRouterInfo`](crate::otsys::otHistoryTrackerRouterInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct HistoryTrackerRouterInfo(pub otHistoryTrackerRouterInfo);

impl_ot_castable!(HistoryTrackerRouterInfo, otHistoryTrackerRouterInfo);

impl HistoryTrackerRouterInfo {
    /// Returns a router table entry event (`OT_HISTORY_TRACKER_ROUTER_EVENT_*` enumeration).
    pub fn event(&self) -> HistoryTrackerRouterEvent {
        HistoryTrackerRouterEvent::from(self.0.mEvent() as u32)
    }

    /// Returns the Rotuer ID.
    pub fn router_id(&self) -> u8 {
        self.0.mRouterId()
    }

    /// Returns the next hop Router ID.
    pub fn next_hop(&self) -> u8 {
        self.0.mNextHop
    }

    /// Returns the old path cost (`OT_HISTORY_TRACKER_INFINITE_PATH_COST` if infinite or unknown).
    pub fn old_path_cost(&self) -> u8 {
        self.0.mOldPathCost()
    }

    /// Returns the new path cost (`OT_HISTORY_TRACKER_INFINITE_PATH_COST` if infinite or unknown).
    pub fn path_cost(&self) -> u8 {
        self.0.mPathCost()
    }
}

/// Defines the events for a Network Data entry (i.e., whether an entry is added or removed).
///
/// Functional equivalent of [`otsys::otHistoryTrackerNetDataEvent`](crate::otsys::otHistoryTrackerNetDataEvent).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, FromPrimitive)]
#[allow(missing_docs)]
pub enum HistoryTrackerNetDataEvent {
    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NET_DATA_ENTRY_ADDED`](crate::otsys::OT_HISTORY_TRACKER_NET_DATA_ENTRY_ADDED).
    Added = OT_HISTORY_TRACKER_ROUTER_EVENT_ADDED as isize,

    /// Functional equivalent of [`otsys::OT_HISTORY_TRACKER_NET_DATA_ENTRY_REMOVED`](crate::otsys::OT_HISTORY_TRACKER_NET_DATA_ENTRY_REMOVED).
    Removed = OT_HISTORY_TRACKER_NET_DATA_ENTRY_REMOVED as isize,
}

impl From<otHistoryTrackerNetDataEvent> for HistoryTrackerNetDataEvent {
    fn from(x: otHistoryTrackerNetDataEvent) -> Self {
        use num::FromPrimitive;
        Self::from_u32(x)
            .unwrap_or_else(|| panic!("Unknown otHistoryTrackerNetDataEvent value: {x}"))
    }
}

/// This structure represents a NetData on-mesh prefix info in the history tracker report.
///
/// Functional equivalent of [`otsys::otHistoryTrackerOnMeshPrefixInfo`](crate::otsys::otHistoryTrackerOnMeshPrefixInfo).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct HistoryTrackerOnMeshPrefixInfo(pub otHistoryTrackerOnMeshPrefixInfo);

impl_ot_castable!(HistoryTrackerOnMeshPrefixInfo, otHistoryTrackerOnMeshPrefixInfo);

impl HistoryTrackerOnMeshPrefixInfo {
    /// Returns the on-mesh prefix entry.
    pub fn prefix(&self) -> BorderRouterConfig {
        BorderRouterConfig(self.0.mPrefix)
    }

    /// Returns the NetData on-mesh prefix event (added/removed).
    pub fn event(&self) -> HistoryTrackerNetDataEvent {
        HistoryTrackerNetDataEvent::from(self.0.mEvent)
    }
}

impl std::fmt::Debug for HistoryTrackerOnMeshPrefixInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("HistoryTrackerOnMeshPrefixInfo");
        ds.field("on-mesh prefix", &self.prefix());
        ds.field("event", &self.event());
        ds.finish()
    }
}

/// This structure represents a NetData external route info in the history tracker report.
///
/// Functional equivalent of [`otsys::otHistoryTrackerExternalRouteInfo`](crate::otsys::otHistoryTrackerExternalRouteInfo).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct HistoryTrackerExternalRouteInfo(pub otHistoryTrackerExternalRouteInfo);

impl_ot_castable!(HistoryTrackerExternalRouteInfo, otHistoryTrackerExternalRouteInfo);

impl HistoryTrackerExternalRouteInfo {
    /// Returns the external route entry.
    pub fn route(&self) -> ExternalRouteConfig {
        ExternalRouteConfig(self.0.mRoute)
    }

    /// Returns the NetData external route event (added/removed).
    pub fn event(&self) -> HistoryTrackerNetDataEvent {
        HistoryTrackerNetDataEvent::from(self.0.mEvent)
    }
}

impl std::fmt::Debug for HistoryTrackerExternalRouteInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("HistoryTrackerExternalRouteInfo");
        ds.field("external route", &self.route());
        ds.field("event", &self.event());
        ds.finish()
    }
}
