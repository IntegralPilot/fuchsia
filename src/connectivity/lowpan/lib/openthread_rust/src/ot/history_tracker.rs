// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Represents an iterator to iterate the net info history.
#[allow(missing_debug_implementations)]
pub struct HistoryTrackerNetInfoIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otHistoryTrackerIterator,
}

impl<T: ?Sized + HistoryTracker> Iterator for HistoryTrackerNetInfoIterator<'_, T> {
    type Item = (HistoryTrackerNetworkInfo, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let mut entry_age = 0;
        self.ot_instance
            .iter_next_net_info_history(&mut self.ot_iter, &mut entry_age)
            .map(|info| (info, entry_age))
    }
}

/// Represents an iterator to iterate the neighbor history.
#[allow(missing_debug_implementations)]
pub struct HistoryTrackerNeighborIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otHistoryTrackerIterator,
}

impl<T: ?Sized + HistoryTracker> Iterator for HistoryTrackerNeighborIterator<'_, T> {
    type Item = (HistoryTrackerNeighborInfo, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let mut entry_age = 0;
        self.ot_instance
            .iter_next_neighbor_history(&mut self.ot_iter, &mut entry_age)
            .map(|info| (info, entry_age))
    }
}

/// Represents an iterator to iterate the router info history.
#[allow(missing_debug_implementations)]
pub struct HistoryTrackerRouterIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otHistoryTrackerIterator,
}

impl<T: ?Sized + HistoryTracker> Iterator for HistoryTrackerRouterIterator<'_, T> {
    type Item = (HistoryTrackerRouterInfo, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let mut entry_age = 0;
        self.ot_instance
            .iter_next_router_history(&mut self.ot_iter, &mut entry_age)
            .map(|info| (info, entry_age))
    }
}

/// Represents an iterator to iterate the NetData on-mesh prefix info history.
#[allow(missing_debug_implementations)]
pub struct HistoryTrackerOnMeshPrefixIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otHistoryTrackerIterator,
}

impl<T: ?Sized + HistoryTracker> Iterator for HistoryTrackerOnMeshPrefixIterator<'_, T> {
    type Item = (HistoryTrackerOnMeshPrefixInfo, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let mut entry_age = 0;
        self.ot_instance
            .iter_next_on_mesh_prefix_history(&mut self.ot_iter, &mut entry_age)
            .map(|info| (info, entry_age))
    }
}

/// Represents an iterator to iterate the NetData external route info history.
#[allow(missing_debug_implementations)]
pub struct HistoryTrackerExternalRouteIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otHistoryTrackerIterator,
}

impl<T: ?Sized + HistoryTracker> Iterator for HistoryTrackerExternalRouteIterator<'_, T> {
    type Item = (HistoryTrackerExternalRouteInfo, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let mut entry_age = 0;
        self.ot_instance
            .iter_next_external_route_history(&mut self.ot_iter, &mut entry_age)
            .map(|info| (info, entry_age))
    }
}

/// Methods from the [OpenThread "history-tracker" Module][1].
///
/// [1]: https://openthread.io/reference/group/api-history-tracker
pub trait HistoryTracker {
    /// Functional equivalent of [`otsys::otHistoryTrackerInitIterator`]
    /// (crate::otsys::otHistoryTrackerInitIterator).
    fn history_tracker_init_iterator(&self, iter: &mut otHistoryTrackerIterator);

    /// Get the history tracker net info history iterator instance.
    fn history_tracker_net_info_history_get_iterator(
        &self,
    ) -> HistoryTrackerNetInfoIterator<'_, Self> {
        let mut ot_iter = otHistoryTrackerIterator::default();
        self.history_tracker_init_iterator(&mut ot_iter);
        HistoryTrackerNetInfoIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otHistoryTrackerIterateNetInfoHistory`]
    /// (crate::otsys::otHistoryTrackerIterateNetInfoHistory).
    fn iter_next_net_info_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNetworkInfo>;

    /// Get the history tracker neighbor info history iterator instance.
    fn history_tracker_neighbor_history_get_iterator(
        &self,
    ) -> HistoryTrackerNeighborIterator<'_, Self> {
        let mut ot_iter = otHistoryTrackerIterator::default();
        self.history_tracker_init_iterator(&mut ot_iter);
        HistoryTrackerNeighborIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otHistoryTrackerIterateNeighborHistory`]
    /// (crate::otsys::otHistoryTrackerIterateNeighborHistory).
    fn iter_next_neighbor_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNeighborInfo>;

    /// Get the history tracker router info history iterator instance.
    fn history_tracker_router_history_get_iterator(
        &self,
    ) -> HistoryTrackerRouterIterator<'_, Self> {
        let mut ot_iter = otHistoryTrackerIterator::default();
        self.history_tracker_init_iterator(&mut ot_iter);
        HistoryTrackerRouterIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otHistoryTrackerIterateRouterHistory`]
    /// (crate::otsys::otHistoryTrackerIterateRouterHistory).
    fn iter_next_router_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerRouterInfo>;

    /// Get the history tracker netdata on-mesh prefix info history iterator instance.
    fn history_tracker_on_mesh_prefix_history_get_iterator(
        &self,
    ) -> HistoryTrackerOnMeshPrefixIterator<'_, Self> {
        let mut ot_iter = otHistoryTrackerIterator::default();
        self.history_tracker_init_iterator(&mut ot_iter);
        HistoryTrackerOnMeshPrefixIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otHistoryTrackerIterateOnMeshPrefixHistory`]
    /// (crate::otsys::otHistoryTrackerIterateOnMeshPrefixHistory).
    fn iter_next_on_mesh_prefix_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerOnMeshPrefixInfo>;

    /// Get the history tracker netdata external route info history iterator instance.
    fn history_tracker_external_route_history_get_iterator(
        &self,
    ) -> HistoryTrackerExternalRouteIterator<'_, Self> {
        let mut ot_iter = otHistoryTrackerIterator::default();
        self.history_tracker_init_iterator(&mut ot_iter);
        HistoryTrackerExternalRouteIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otHistoryTrackerIterateExternalRouteHistory`]
    /// (crate::otsys::otHistoryTrackerIterateExternalRouteHistory).
    fn iter_next_external_route_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerExternalRouteInfo>;
}

impl<T: HistoryTracker + Boxable> HistoryTracker for ot::Box<T> {
    fn history_tracker_init_iterator(&self, iter: &mut otHistoryTrackerIterator) {
        self.as_ref().history_tracker_init_iterator(iter)
    }

    fn iter_next_net_info_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNetworkInfo> {
        self.as_ref().iter_next_net_info_history(ot_iter, entry_age)
    }

    fn iter_next_neighbor_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNeighborInfo> {
        self.as_ref().iter_next_neighbor_history(ot_iter, entry_age)
    }

    fn iter_next_router_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerRouterInfo> {
        self.as_ref().iter_next_router_history(ot_iter, entry_age)
    }

    fn iter_next_on_mesh_prefix_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerOnMeshPrefixInfo> {
        self.as_ref().iter_next_on_mesh_prefix_history(ot_iter, entry_age)
    }

    fn iter_next_external_route_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerExternalRouteInfo> {
        self.as_ref().iter_next_external_route_history(ot_iter, entry_age)
    }
}

impl HistoryTracker for Instance {
    fn history_tracker_init_iterator(&self, iter: &mut otHistoryTrackerIterator) {
        unsafe { otHistoryTrackerInitIterator(iter) }
    }

    fn iter_next_net_info_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNetworkInfo> {
        unsafe {
            let info_ptr = otHistoryTrackerIterateNetInfoHistory(
                self.as_ot_ptr(),
                ot_iter as *mut otHistoryTrackerIterator,
                entry_age as *mut u32,
            );

            info_ptr.as_ref().map(|raw| HistoryTrackerNetworkInfo(*raw))
        }
    }

    fn iter_next_neighbor_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerNeighborInfo> {
        unsafe {
            let info_ptr = otHistoryTrackerIterateNeighborHistory(
                self.as_ot_ptr(),
                ot_iter as *mut otHistoryTrackerIterator,
                entry_age as *mut u32,
            );

            info_ptr.as_ref().map(|raw| HistoryTrackerNeighborInfo(*raw))
        }
    }

    fn iter_next_router_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerRouterInfo> {
        unsafe {
            let info_ptr = otHistoryTrackerIterateRouterHistory(
                self.as_ot_ptr(),
                ot_iter as *mut otHistoryTrackerIterator,
                entry_age as *mut u32,
            );

            info_ptr.as_ref().map(|raw| HistoryTrackerRouterInfo(*raw))
        }
    }

    fn iter_next_on_mesh_prefix_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerOnMeshPrefixInfo> {
        unsafe {
            let info_ptr = otHistoryTrackerIterateOnMeshPrefixHistory(
                self.as_ot_ptr(),
                ot_iter as *mut otHistoryTrackerIterator,
                entry_age as *mut u32,
            );

            info_ptr.as_ref().map(|raw| HistoryTrackerOnMeshPrefixInfo(*raw))
        }
    }

    fn iter_next_external_route_history(
        &self,
        ot_iter: &mut otHistoryTrackerIterator,
        entry_age: &mut u32,
    ) -> Option<HistoryTrackerExternalRouteInfo> {
        unsafe {
            let info_ptr = otHistoryTrackerIterateExternalRouteHistory(
                self.as_ot_ptr(),
                ot_iter as *mut otHistoryTrackerIterator,
                entry_age as *mut u32,
            );

            info_ptr.as_ref().map(|raw| HistoryTrackerExternalRouteInfo(*raw))
        }
    }
}
