// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// The maximum length of Thread network data, in bytes.
pub const MAX_NET_DATA_LEN: usize = 255;

/// Iterator type for on-mesh prefixes in network data.
#[allow(missing_debug_implementations)]
pub struct OnMeshPrefixIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otNetworkDataIterator,
}

impl<T: ?Sized + NetData> Iterator for OnMeshPrefixIterator<'_, T> {
    type Item = BorderRouterConfig;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.iter_next_on_mesh_prefix(&mut self.ot_iter)
    }
}

/// Iterator type for external routes in network data.
#[allow(missing_debug_implementations)]
pub struct ExternalRouteIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otNetworkDataIterator,
}

impl<T: ?Sized + NetData> Iterator for ExternalRouteIterator<'_, T> {
    type Item = ExternalRouteConfig;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.iter_next_external_route(&mut self.ot_iter)
    }
}

/// Iterator type for services in network data.
#[allow(missing_debug_implementations)]
pub struct ServiceIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otNetworkDataIterator,
}

impl<T: ?Sized + NetData> Iterator for ServiceIterator<'_, T> {
    type Item = ServiceConfig;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.iter_next_service(&mut self.ot_iter)
    }
}

/// Iterator type for lowpan context info in network data.
#[allow(missing_debug_implementations)]
pub struct LowpanContextInfoIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otNetworkDataIterator,
}

impl<T: ?Sized + NetData> Iterator for LowpanContextInfoIterator<'_, T> {
    type Item = LowpanContextInfo;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.iter_next_lowpan_context_info(&mut self.ot_iter)
    }
}

/// Methods from the [OpenThread "NetData" Module][1].
///
/// [1]: https://openthread.io/reference/group/api-thread-general
pub trait NetData {
    /// Functional equivalent of [`otsys::otNetDataGet`](crate::otsys::otNetDataGet).
    fn net_data_get<'a>(&self, stable: bool, data: &'a mut [u8]) -> Result<&'a [u8]>;

    /// Same as [`net_data_get`], but returns the net data as a vector.
    fn net_data_as_vec(&self, stable: bool) -> Result<Vec<u8>> {
        let mut ret = vec![0; MAX_NET_DATA_LEN];

        let len = self.net_data_get(stable, ret.as_mut_slice())?.len();

        ret.truncate(len);

        Ok(ret)
    }

    /// Functional equivalent of [`otsys::otNetDataGetVersion`](crate::otsys::otNetDataGetVersion).
    fn net_data_get_version(&self) -> u8;

    /// Functional equivalent of
    /// [`otsys::otNetDataGetStableVersion`](crate::otsys::otNetDataGetStableVersion).
    fn net_data_get_stable_version(&self) -> u8;

    /// Functional equivalent of
    /// [`otsys::otNetDataGetCommissioningDataset`](crate::otsys::otNetDataGetCommissioningDataset).
    fn net_data_get_commissioning_dataset(&self, dataset: &mut CommissioningDataset);

    /// Functional equivalent of [`otsys::otNetDataGetNextOnMeshPrefix`](crate::otsys::otNetDataGetNextOnMeshPrefix).
    fn iter_next_on_mesh_prefix(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<BorderRouterConfig>;

    /// Functional equivalent of [`otsys::otNetDataGetNextRoute`](crate::otsys::otNetDataGetNextRoute).
    fn iter_next_external_route(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<ExternalRouteConfig>;

    /// Functional equivalent of [`otsys::otNetDataGetNextService`](crate::otsys::otNetDataGetNextService).
    fn iter_next_service(&self, ot_iter: &mut otNetworkDataIterator) -> Option<ServiceConfig>;

    /// Functional equivalent of [`otsys::otNetDataGetNextLowpanContextInfo`](crate::otsys::otNetDataGetNextLowpanContextInfo).
    fn iter_next_lowpan_context_info(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<LowpanContextInfo>;

    /// Returns an iterator for iterating over on-mesh prefixes.
    fn iter_on_mesh_prefixes(&self) -> OnMeshPrefixIterator<'_, Self> {
        OnMeshPrefixIterator { ot_instance: self, ot_iter: OT_NETWORK_DATA_ITERATOR_INIT }
    }

    /// Returns an iterator for iterating over external routes.
    fn iter_external_routes(&self) -> ExternalRouteIterator<'_, Self> {
        ExternalRouteIterator { ot_instance: self, ot_iter: OT_NETWORK_DATA_ITERATOR_INIT }
    }

    /// Returns an iterator for iterating over services.
    fn iter_services(&self) -> ServiceIterator<'_, Self> {
        ServiceIterator { ot_instance: self, ot_iter: OT_NETWORK_DATA_ITERATOR_INIT }
    }

    /// Returns an iterator for iterating over lowpan context info.
    fn iter_lowpan_contexts_info(&self) -> LowpanContextInfoIterator<'_, Self> {
        LowpanContextInfoIterator { ot_instance: self, ot_iter: OT_NETWORK_DATA_ITERATOR_INIT }
    }
}

impl<T: NetData + Boxable> NetData for ot::Box<T> {
    fn net_data_get<'a>(&self, stable: bool, data: &'a mut [u8]) -> Result<&'a [u8]> {
        self.as_ref().net_data_get(stable, data)
    }

    fn net_data_get_version(&self) -> u8 {
        self.as_ref().net_data_get_version()
    }

    fn net_data_get_stable_version(&self) -> u8 {
        self.as_ref().net_data_get_version()
    }

    fn net_data_get_commissioning_dataset(&self, dataset: &mut CommissioningDataset) {
        self.as_ref().net_data_get_commissioning_dataset(dataset);
    }

    fn iter_next_on_mesh_prefix(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<BorderRouterConfig> {
        self.as_ref().iter_next_on_mesh_prefix(ot_iter)
    }

    fn iter_next_external_route(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<ExternalRouteConfig> {
        self.as_ref().iter_next_external_route(ot_iter)
    }

    fn iter_next_service(&self, ot_iter: &mut otNetworkDataIterator) -> Option<ServiceConfig> {
        self.as_ref().iter_next_service(ot_iter)
    }

    fn iter_next_lowpan_context_info(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<LowpanContextInfo> {
        self.as_ref().iter_next_lowpan_context_info(ot_iter)
    }
}

impl NetData for Instance {
    fn net_data_get<'a>(&self, stable: bool, data: &'a mut [u8]) -> Result<&'a [u8]> {
        let mut len: u8 = data.len().min(MAX_NET_DATA_LEN).try_into().unwrap();

        Error::from(unsafe {
            otNetDataGet(self.as_ot_ptr(), stable, data.as_mut_ptr(), (&mut len) as *mut u8)
        })
        .into_result()?;

        Ok(&data[..(len as usize)])
    }

    fn net_data_get_version(&self) -> u8 {
        unsafe { otNetDataGetVersion(self.as_ot_ptr()) }
    }

    fn net_data_get_stable_version(&self) -> u8 {
        unsafe { otNetDataGetStableVersion(self.as_ot_ptr()) }
    }

    fn net_data_get_commissioning_dataset(&self, dataset: &mut CommissioningDataset) {
        unsafe { otNetDataGetCommissioningDataset(self.as_ot_ptr(), dataset.as_ot_mut_ptr()) }
    }

    fn iter_next_on_mesh_prefix(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<BorderRouterConfig> {
        unsafe {
            let mut ret = BorderRouterConfig::default();
            match Error::from(otNetDataGetNextOnMeshPrefix(
                self.as_ot_ptr(),
                ot_iter as *mut otNetworkDataIterator,
                ret.as_ot_mut_ptr(),
            )) {
                Error::NotFound => None,
                Error::None => Some(ret),
                err => panic!("Unexpected error from otNetDataGetNextOnMeshPrefix: {err:?}"),
            }
        }
    }

    fn iter_next_external_route(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<ExternalRouteConfig> {
        unsafe {
            let mut ret = ExternalRouteConfig::default();
            match Error::from(otNetDataGetNextRoute(
                self.as_ot_ptr(),
                ot_iter as *mut otNetworkDataIterator,
                ret.as_ot_mut_ptr(),
            )) {
                Error::NotFound => None,
                Error::None => Some(ret),
                err => panic!("Unexpected error from otNetDataGetNextRoute: {err:?}"),
            }
        }
    }

    fn iter_next_service(&self, ot_iter: &mut otNetworkDataIterator) -> Option<ServiceConfig> {
        unsafe {
            let mut ret = ServiceConfig::default();
            match Error::from(otNetDataGetNextService(
                self.as_ot_ptr(),
                ot_iter as *mut otNetworkDataIterator,
                ret.as_ot_mut_ptr(),
            )) {
                Error::NotFound => None,
                Error::None => Some(ret),
                err => panic!("Unexpected error from otNetDataGetNextService: {err:?}"),
            }
        }
    }

    fn iter_next_lowpan_context_info(
        &self,
        ot_iter: &mut otNetworkDataIterator,
    ) -> Option<LowpanContextInfo> {
        unsafe {
            let mut ret = LowpanContextInfo::default();
            match Error::from(otNetDataGetNextLowpanContextInfo(
                self.as_ot_ptr(),
                ot_iter as *mut otNetworkDataIterator,
                ret.as_ot_mut_ptr(),
            )) {
                Error::NotFound => None,
                Error::None => Some(ret),
                err => panic!("Unexpected error from otNetDataGetNextLowpanContextInfo: {err:?}"),
            }
        }
    }
}
