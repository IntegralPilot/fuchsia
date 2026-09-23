// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Methods from the "Multi Radio Link " group [1]
///
/// [1] https://openthread.io/reference/group/api-multi-radio
pub trait MultiRadioLink {
    /// Gets the multi radio link information associated with a neighbor with a given Extended Address.
    fn multi_radio_get_neighbor_info(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<MultiRadioNeighborInfo>;
}

impl<T: MultiRadioLink + ot::Boxable> MultiRadioLink for ot::Box<T> {
    fn multi_radio_get_neighbor_info(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<MultiRadioNeighborInfo> {
        self.as_ref().multi_radio_get_neighbor_info(ext_addr)
    }
}

impl MultiRadioLink for Instance {
    fn multi_radio_get_neighbor_info(
        &self,
        ext_addr: &ExtAddress,
    ) -> Result<MultiRadioNeighborInfo> {
        let mut info = MultiRadioNeighborInfo::default();
        Error::from(unsafe {
            otMultiRadioGetNeighborInfo(
                self.as_ot_ptr(),
                ext_addr.as_ot_ptr(),
                info.as_ot_mut_ptr(),
            )
        })
        .into_result()?;
        Ok(info)
    }
}
