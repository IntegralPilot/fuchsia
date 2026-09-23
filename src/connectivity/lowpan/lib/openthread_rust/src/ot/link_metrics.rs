// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Methods from the "Link Metrics " group [1]
///
/// [1] https://openthread.io/reference/group/api-link-metrics
pub trait LinkMetrics {
    /// Get Link Metrics Manager enabled/disabled state.
    fn link_metrics_manager_is_enabled(&self) -> bool;

    /// Enable or disable Link Metrics Manager.
    fn link_metrics_manager_set_enabled(&self, enabled: bool);

    /// Get Link Metrics data of a neighbor by its extended address.
    fn link_metrics_manager_get_metrics_value_by_ext_addr(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<LinkMetricsValues>;
}

impl<T: LinkMetrics + ot::Boxable> LinkMetrics for ot::Box<T> {
    fn link_metrics_manager_is_enabled(&self) -> bool {
        self.as_ref().link_metrics_manager_is_enabled()
    }

    fn link_metrics_manager_set_enabled(&self, enabled: bool) {
        self.as_ref().link_metrics_manager_set_enabled(enabled)
    }

    fn link_metrics_manager_get_metrics_value_by_ext_addr(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<LinkMetricsValues> {
        self.as_ref().link_metrics_manager_get_metrics_value_by_ext_addr(ext_addr)
    }
}

impl LinkMetrics for Instance {
    fn link_metrics_manager_is_enabled(&self) -> bool {
        unsafe { otLinkMetricsManagerIsEnabled(self.as_ot_ptr()) }
    }

    fn link_metrics_manager_set_enabled(&self, enabled: bool) {
        unsafe { otLinkMetricsManagerSetEnabled(self.as_ot_ptr(), enabled) }
    }

    fn link_metrics_manager_get_metrics_value_by_ext_addr(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<LinkMetricsValues> {
        let mut values = LinkMetricsValues::default();
        Error::from(unsafe {
            otLinkMetricsManagerGetMetricsValueByExtAddr(
                self.as_ot_ptr(),
                ext_addr.as_ot_ptr(),
                values.as_ot_mut_ptr(),
            )
        })
        .into_result()?;
        Ok(values)
    }
}
