// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// for now, we allow this for this module because we can't apply it
// specifically to the type `ChangedFlags`, due to a bug in `bitflags!`.
#![allow(missing_docs)]

use crate::ot::DnsTxtEntryIterator;
use crate::prelude_internal::*;

/// Represents the SRP server state.
///
/// Functional equivalent of [`otsys::otSrpServerState`](crate::otsys::otSrpServerState).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, num_derive::FromPrimitive)]
#[allow(missing_docs)]
pub enum SrpServerState {
    /// Functional equivalent of
    /// [`otsys::otSrpServerState_OT_SRP_SERVER_STATE_DISABLED`](crate::otsys::otSrpServerState_OT_SRP_SERVER_STATE_DISABLED).
    Disabled = OT_SRP_SERVER_STATE_DISABLED as isize,

    /// Functional equivalent of
    /// [`otsys::otSrpServerState_OT_SRP_SERVER_STATE_RUNNING`](crate::otsys::otSrpServerState_OT_SRP_SERVER_STATE_RUNNING).
    Running = OT_SRP_SERVER_STATE_RUNNING as isize,

    /// Functional equivalent of
    /// [`otsys::otSrpServerState_OT_SRP_SERVER_STATE_STOPPED`](crate::otsys::otSrpServerState_OT_SRP_SERVER_STATE_STOPPED).
    Stopped = OT_SRP_SERVER_STATE_STOPPED as isize,
}

/// Represents the SRP server address mode.
///
/// Functional equivalent of
/// [`otsys::otSrpServerAddressMode`](crate::otsys::otSrpServerAddressMode).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, num_derive::FromPrimitive)]
#[allow(missing_docs)]
pub enum SrpServerAddressMode {
    /// Functional equivalent of
    /// [`otsys::otSrpServerAddressMode_OT_SRP_SERVER_ADDRESS_MODE_UNICAST`](crate::otsys::otSrpServerAddressMode_OT_SRP_SERVER_ADDRESS_MODE_UNICAST).
    Unicast = OT_SRP_SERVER_ADDRESS_MODE_UNICAST as isize,

    /// Functional equivalent of
    /// [`otsys::otSrpServerAddressMode_OT_SRP_SERVER_ADDRESS_MODE_ANYCAST`](crate::otsys::otSrpServerAddressMode_OT_SRP_SERVER_ADDRESS_MODE_ANYCAST).
    Anycast = OT_SRP_SERVER_ADDRESS_MODE_ANYCAST as isize,
}

/// Iterates over the available SRP server hosts. See [`SrpServer::srp_server_get_hosts`].
pub struct SrpServerHostIterator<'a, T: SrpServer> {
    prev: Option<&'a SrpServerHost>,
    ot_instance: &'a T,
}

// This cannot be easily derived because T doesn't implement `Clone`,
// so we must implement it manually.
impl<T: SrpServer> Clone for SrpServerHostIterator<'_, T> {
    fn clone(&self) -> Self {
        SrpServerHostIterator { prev: self.prev, ot_instance: self.ot_instance }
    }
}

impl<T: SrpServer> std::fmt::Debug for SrpServerHostIterator<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for item in self.clone() {
            item.fmt(f)?;
            write!(f, ",")?;
        }
        write!(f, "]")
    }
}

impl<'a, T: SrpServer> Iterator for SrpServerHostIterator<'a, T> {
    type Item = &'a SrpServerHost;

    fn next(&mut self) -> Option<Self::Item> {
        self.prev = self.ot_instance.srp_server_next_host(self.prev);
        self.prev
    }
}

/// Iterates over all the available SRP services.
#[derive(Clone)]
pub struct SrpServerServiceIterator<'a> {
    prev: Option<&'a SrpServerService>,
    host: &'a SrpServerHost,
}

impl std::fmt::Debug for SrpServerServiceIterator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for item in self.clone() {
            item.fmt(f)?;
            write!(f, ",")?;
        }
        write!(f, "]")
    }
}

impl<'a> Iterator for SrpServerServiceIterator<'a> {
    type Item = &'a SrpServerService;

    fn next(&mut self) -> Option<Self::Item> {
        self.prev = self.host.next_service(self.prev);
        self.prev
    }
}

/// This opaque type (only used by reference) represents a SRP host.
///
/// Functional equivalent of [`otsys::otSrpServerHost`](crate::otsys::otSrpServerHost).
#[repr(transparent)]
pub struct SrpServerHost(otSrpServerHost);
impl_ot_castable!(opaque SrpServerHost, otSrpServerHost);

impl std::fmt::Debug for SrpServerHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("otSrpServerHost")
            .field("full_name", &self.full_name_cstr())
            .field("addresses", &self.addresses())
            .field("is_deleted", &self.is_deleted())
            .field("services", &self.services())
            .finish()
    }
}

impl SrpServerHost {
    /// Functional equivalent of
    /// [`otsys::otSrpServerHostGetAddresses`](crate::otsys::otSrpServerHostGetAddresses).
    pub fn addresses(&self) -> &[Ip6Address] {
        let mut addresses_len = 0u8;
        unsafe {
            let addresses_ptr =
                otSrpServerHostGetAddresses(self.as_ot_ptr(), &mut addresses_len as *mut u8);

            std::slice::from_raw_parts(addresses_ptr as *const Ip6Address, addresses_len as usize)
        }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHostMatchesFullName`](crate::otsys::otSrpServerHostMatchesFullName).
    pub fn matches_full_name_cstr(&self, full_name: &CStr) -> bool {
        unsafe { otSrpServerHostMatchesFullName(self.as_ot_ptr(), full_name.as_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHostGetFullName`](crate::otsys::otSrpServerHostGetFullName).
    pub fn full_name_cstr(&self) -> &CStr {
        unsafe { CStr::from_ptr(otSrpServerHostGetFullName(self.as_ot_ptr())) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHostIsDeleted`](crate::otsys::otSrpServerHostIsDeleted).
    pub fn is_deleted(&self) -> bool {
        unsafe { otSrpServerHostIsDeleted(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHostGetLeaseInfo`](crate::otsys::otSrpServerHostGetLeaseInfo).
    pub fn get_lease_info(&self, lease_info: &mut SrpServerLeaseInfo) {
        unsafe { otSrpServerHostGetLeaseInfo(self.as_ot_ptr(), lease_info.as_ot_mut_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHostGetNextService`](crate::otsys::otSrpServerHostGetNextService).
    pub fn next_service<'a>(
        &'a self,
        prev: Option<&'a SrpServerService>,
    ) -> Option<&'a SrpServerService> {
        let prev = prev.map(|x| x.as_ot_ptr()).unwrap_or(null());
        unsafe {
            SrpServerService::ref_from_ot_ptr(otSrpServerHostGetNextService(self.as_ot_ptr(), prev))
        }
    }

    /// Returns an iterator over all of the services for this host.
    pub fn services(&self) -> SrpServerServiceIterator<'_> {
        SrpServerServiceIterator { prev: None, host: self }
    }
}

/// Iterates over all the available SRP services subtypes.
#[derive(Clone)]
pub struct SrpServerServiceSubtypeIterator<'a> {
    service: &'a SrpServerService,
    next_i: u16,
}

impl std::fmt::Debug for SrpServerServiceSubtypeIterator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for item in self.clone() {
            item.fmt(f)?;
            write!(f, ",")?;
        }
        write!(f, "]")
    }
}

impl<'a> Iterator for SrpServerServiceSubtypeIterator<'a> {
    type Item = &'a CStr;

    fn next(&mut self) -> Option<Self::Item> {
        let ret = self.service.subtype_service_name_at(self.next_i);
        if ret.is_some() {
            self.next_i += 1;
        }
        ret
    }
}

/// This opaque type (only used by reference) represents a SRP service.
///
/// Functional equivalent of [`otsys::otSrpServerService`](crate::otsys::otSrpServerService).
#[repr(transparent)]
pub struct SrpServerService(otSrpServerService);
impl_ot_castable!(opaque SrpServerService, otSrpServerService);

impl std::fmt::Debug for SrpServerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_deleted() {
            f.debug_struct("otSrpServerService")
                .field("service_name", &self.service_name_cstr())
                .field("instance_name", &self.instance_name_cstr())
                .field("is_deleted", &self.is_deleted())
                .finish()
        } else {
            f.debug_struct("otSrpServerService")
                .field("service_name", &self.service_name_cstr())
                .field("instance_name", &self.instance_name_cstr())
                .field("is_deleted", &self.is_deleted())
                .field("txt_data", &ascii_dump(self.txt_data()))
                .field("txt_entries", &self.txt_entries().collect::<Vec<_>>())
                .field("port", &self.port())
                .field("priority", &self.priority())
                .field("weight", &self.weight())
                .field("subtypes", &self.subtypes())
                .field("ttl", &self.ttl())
                .finish()
        }
    }
}

impl SrpServerService {
    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetPort`](crate::otsys::otSrpServerServiceGetPort).
    pub fn port(&self) -> u16 {
        unsafe { otSrpServerServiceGetPort(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetPriority`](crate::otsys::otSrpServerServiceGetPriority).
    pub fn priority(&self) -> u16 {
        unsafe { otSrpServerServiceGetPriority(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceIsDeleted`](crate::otsys::otSrpServerServiceIsDeleted).
    pub fn is_deleted(&self) -> bool {
        unsafe { otSrpServerServiceIsDeleted(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetLeaseInfo`](crate::otsys::otSrpServerServiceGetLeaseInfo).
    pub fn get_lease_info(&self, lease_info: &mut SrpServerLeaseInfo) {
        unsafe { otSrpServerServiceGetLeaseInfo(self.as_ot_ptr(), lease_info.as_ot_mut_ptr()) }
    }

    /// Returns an iterator over all of the subtypes.
    pub fn subtypes(&self) -> SrpServerServiceSubtypeIterator<'_> {
        SrpServerServiceSubtypeIterator { service: self, next_i: 0 }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetNumberOfSubTypes`](crate::otsys::otSrpServerServiceGetNumberOfSubTypes).
    pub fn number_of_subtypes(&self) -> u16 {
        unsafe { otSrpServerServiceGetNumberOfSubTypes(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetSubTypeServiceNameAt`](crate::otsys::otSrpServerServiceGetSubTypeServiceNameAt).
    pub fn subtype_service_name_at(&self, i: u16) -> Option<&CStr> {
        unsafe {
            let ptr = otSrpServerServiceGetSubTypeServiceNameAt(self.as_ot_ptr(), i);
            if ptr.is_null() { None } else { Some(CStr::from_ptr(ptr)) }
        }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetTxtData`](crate::otsys::otSrpServerServiceGetTxtData).
    pub fn txt_data(&self) -> &[u8] {
        let mut txt_data_len = 0u16;
        unsafe {
            let txt_data_ptr =
                otSrpServerServiceGetTxtData(self.as_ot_ptr(), &mut txt_data_len as *mut u16);

            std::slice::from_raw_parts(txt_data_ptr, txt_data_len as usize)
        }
    }

    /// Returns iterator over all the DNS TXT entries.
    pub fn txt_entries(&self) -> DnsTxtEntryIterator<'_> {
        DnsTxtEntryIterator::try_new(self.txt_data()).unwrap()
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetWeight`](crate::otsys::otSrpServerServiceGetWeight).
    pub fn weight(&self) -> u16 {
        unsafe { otSrpServerServiceGetWeight(self.as_ot_ptr()) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetServiceName`](crate::otsys::otSrpServerServiceGetServiceName).
    pub fn service_name_cstr(&self) -> &CStr {
        unsafe { CStr::from_ptr(otSrpServerServiceGetServiceName(self.as_ot_ptr())) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetInstanceName`](crate::otsys::otSrpServerServiceGetInstanceName).
    pub fn instance_name_cstr(&self) -> &CStr {
        unsafe { CStr::from_ptr(otSrpServerServiceGetInstanceName(self.as_ot_ptr())) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetInstanceLabel`](crate::otsys::otSrpServerServiceGetInstanceLabel).
    pub fn instance_label_cstr(&self) -> &CStr {
        unsafe { CStr::from_ptr(otSrpServerServiceGetInstanceLabel(self.as_ot_ptr())) }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerServiceGetTtl`](crate::otsys::otSrpServerServiceGetTtl).
    pub fn ttl(&self) -> zx::MonotonicDuration {
        unsafe {
            zx::MonotonicDuration::from_seconds(otSrpServerServiceGetTtl(self.as_ot_ptr()).into())
        }
    }
}

/// The ID of a SRP service update transaction on the SRP Server.
///
/// Functional equivalent of
/// [`otsys::otSrpServerServiceUpdateId`](crate::otsys::otSrpServerServiceUpdateId).
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct SrpServerServiceUpdateId(otSrpServerServiceUpdateId);

impl SrpServerServiceUpdateId {
    fn new(x: otSrpServerServiceUpdateId) -> Self {
        Self(x)
    }

    fn take(self) -> otSrpServerServiceUpdateId {
        self.0
    }
}

/// Server Methods from the [OpenThread SRP Module][1].
///
/// [1]: https://openthread.io/reference/group/api-srp
pub trait SrpServer {
    /// Functional equivalent of
    /// [`otsys::otSrpServerGetAddressMode`](crate::otsys::otSrpServerGetAddressMode).
    fn srp_server_get_address_mode(&self) -> SrpServerAddressMode;

    /// Functional equivalent of
    /// [`otsys::otSrpServerGetState`](crate::otsys::otSrpServerGetState).
    fn srp_server_get_state(&self) -> SrpServerState;

    /// Functional equivalent of
    /// [`otsys::otSrpServerGetPort`](crate::otsys::otSrpServerGetPort).
    fn srp_server_get_port(&self) -> u16;

    /// Functional equivalent of
    /// [`otsys::otSrpServerSetEnabled`](crate::otsys::otSrpServerSetEnabled).
    fn srp_server_set_enabled(&self, enabled: bool);

    /// Functional equivalent of
    /// [`otsys::otSrpServerSetEnabled`](crate::otsys::otSrpServerSetAutoEnableMode).
    fn srp_server_set_auto_enable_mode(&self, enabled: bool);

    /// Returns true if the SRP server is enabled.
    fn srp_server_is_enabled(&self) -> bool;

    /// Returns true if SRP server auto-enable mode is enabled.
    fn srp_server_is_auto_enable_mode(&self) -> bool;

    /// Returns true if the SRP server is running, false if it is stopped or disabled.
    fn srp_server_is_running(&self) -> bool;

    /// Functional equivalent of
    /// [`otsys::otSrpServerSetDomain`](crate::otsys::otSrpServerSetDomain).
    fn srp_server_set_domain(&self, domain: &CStr) -> Result;

    /// Functional equivalent of
    /// [`otsys::otSrpServerGetDomain`](crate::otsys::otSrpServerGetDomain).
    fn srp_server_get_domain(&self) -> &CStr;

    /// Functional equivalent of
    /// [`otsys::otSrpServerGetResponseCounters`](crate::otsys::otSrpServerGetResponseCounters).
    fn srp_server_get_response_counters(&self) -> &SrpServerResponseCounters;

    /// Functional equivalent of
    /// [`otsys::otSrpServerGetNextHost`](crate::otsys::otSrpServerGetNextHost).
    fn srp_server_next_host<'a>(
        &'a self,
        prev: Option<&'a SrpServerHost>,
    ) -> Option<&'a SrpServerHost>;

    /// Returns an iterator over the SRP hosts.
    fn srp_server_hosts(&self) -> SrpServerHostIterator<'_, Self>
    where
        Self: Sized,
    {
        SrpServerHostIterator { prev: None, ot_instance: self }
    }

    /// Functional equivalent of
    /// [`otsys::otSrpServerHandleServiceUpdateResult`](crate::otsys::otSrpServerHandleServiceUpdateResult).
    fn srp_server_handle_service_update_result(&self, id: SrpServerServiceUpdateId, result: Result);

    /// Functional equivalent of
    /// [`otsys::otSrpServerSetServiceUpdateHandler`](crate::otsys::otSrpServerSetServiceUpdateHandler).
    fn srp_server_set_service_update_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(&'a ot::Instance, SrpServerServiceUpdateId, &'a SrpServerHost, u32) + 'a;

    /// Functional equivalent of
    /// [`otsys::otSrpServerParseSubTypeServiceName`](crate::otsys::otSrpServerParseSubTypeServiceName).
    fn parse_label_from_subtype_service_name(
        &self,
        subtype_service_name: &CStr,
    ) -> Result<CString, Error>;
}

impl<T: SrpServer + Boxable> SrpServer for ot::Box<T> {
    fn srp_server_get_address_mode(&self) -> SrpServerAddressMode {
        self.as_ref().srp_server_get_address_mode()
    }

    fn srp_server_get_state(&self) -> SrpServerState {
        self.as_ref().srp_server_get_state()
    }

    fn srp_server_get_port(&self) -> u16 {
        self.as_ref().srp_server_get_port()
    }

    fn srp_server_set_auto_enable_mode(&self, enabled: bool) {
        self.as_ref().srp_server_set_auto_enable_mode(enabled)
    }

    fn srp_server_set_enabled(&self, enabled: bool) {
        self.as_ref().srp_server_set_enabled(enabled)
    }

    fn srp_server_is_enabled(&self) -> bool {
        self.as_ref().srp_server_is_enabled()
    }

    fn srp_server_is_auto_enable_mode(&self) -> bool {
        self.as_ref().srp_server_is_auto_enable_mode()
    }

    fn srp_server_is_running(&self) -> bool {
        self.as_ref().srp_server_is_running()
    }

    fn srp_server_set_domain(&self, domain: &CStr) -> Result {
        self.as_ref().srp_server_set_domain(domain)
    }

    fn srp_server_get_domain(&self) -> &CStr {
        self.as_ref().srp_server_get_domain()
    }

    fn srp_server_get_response_counters(&self) -> &SrpServerResponseCounters {
        self.as_ref().srp_server_get_response_counters()
    }

    fn srp_server_next_host<'a>(
        &'a self,
        prev: Option<&'a SrpServerHost>,
    ) -> Option<&'a SrpServerHost> {
        self.as_ref().srp_server_next_host(prev)
    }

    fn srp_server_handle_service_update_result(
        &self,
        id: SrpServerServiceUpdateId,
        result: Result,
    ) {
        self.as_ref().srp_server_handle_service_update_result(id, result)
    }

    fn srp_server_set_service_update_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(&'a ot::Instance, SrpServerServiceUpdateId, &'a SrpServerHost, u32) + 'a,
    {
        self.as_ref().srp_server_set_service_update_fn(f)
    }

    fn parse_label_from_subtype_service_name(
        &self,
        subtype_service_name: &CStr,
    ) -> Result<CString, Error> {
        self.as_ref().parse_label_from_subtype_service_name(subtype_service_name)
    }
}

impl SrpServer for Instance {
    fn srp_server_get_address_mode(&self) -> SrpServerAddressMode {
        unsafe {
            SrpServerAddressMode::from_u32(otSrpServerGetAddressMode(self.as_ot_ptr())).unwrap()
        }
    }

    fn srp_server_get_state(&self) -> SrpServerState {
        unsafe { SrpServerState::from_u32(otSrpServerGetState(self.as_ot_ptr())).unwrap() }
    }

    fn srp_server_get_port(&self) -> u16 {
        unsafe { otSrpServerGetPort(self.as_ot_ptr()) }
    }

    fn srp_server_set_auto_enable_mode(&self, enabled: bool) {
        unsafe { otSrpServerSetAutoEnableMode(self.as_ot_ptr(), enabled) }
    }

    fn srp_server_set_enabled(&self, enabled: bool) {
        unsafe { otSrpServerSetEnabled(self.as_ot_ptr(), enabled) }
    }

    fn srp_server_is_enabled(&self) -> bool {
        #[allow(non_upper_case_globals)]
        match unsafe { otSrpServerGetState(self.as_ot_ptr()) } {
            OT_SRP_SERVER_STATE_DISABLED => false,
            OT_SRP_SERVER_STATE_RUNNING => true,
            OT_SRP_SERVER_STATE_STOPPED => true,
            _ => panic!("Unexpected value from otSrpServerGetState"),
        }
    }

    fn srp_server_is_auto_enable_mode(&self) -> bool {
        unsafe { otSrpServerIsAutoEnableMode(self.as_ot_ptr()) }
    }

    fn srp_server_is_running(&self) -> bool {
        #[allow(non_upper_case_globals)]
        match unsafe { otSrpServerGetState(self.as_ot_ptr()) } {
            OT_SRP_SERVER_STATE_DISABLED => false,
            OT_SRP_SERVER_STATE_RUNNING => true,
            OT_SRP_SERVER_STATE_STOPPED => false,
            _ => panic!("Unexpected value from otSrpServerGetState"),
        }
    }

    fn srp_server_set_domain(&self, domain: &CStr) -> Result {
        Error::from(unsafe { otSrpServerSetDomain(self.as_ot_ptr(), domain.as_ptr()) }).into()
    }

    fn srp_server_get_domain(&self) -> &CStr {
        unsafe { CStr::from_ptr(otSrpServerGetDomain(self.as_ot_ptr())) }
    }

    fn srp_server_get_response_counters(&self) -> &SrpServerResponseCounters {
        unsafe {
            SrpServerResponseCounters::ref_from_ot_ptr(otSrpServerGetResponseCounters(
                self.as_ot_ptr(),
            ))
            .unwrap()
        }
    }

    fn srp_server_next_host<'a>(
        &'a self,
        prev: Option<&'a SrpServerHost>,
    ) -> Option<&'a SrpServerHost> {
        let prev = prev.map(|x| x.as_ot_ptr()).unwrap_or(null());
        unsafe { SrpServerHost::ref_from_ot_ptr(otSrpServerGetNextHost(self.as_ot_ptr(), prev)) }
    }

    fn srp_server_handle_service_update_result(
        &self,
        id: SrpServerServiceUpdateId,
        result: Result,
    ) {
        unsafe {
            otSrpServerHandleServiceUpdateResult(
                self.as_ot_ptr(),
                id.take(),
                Error::from(result).into(),
            )
        }
    }

    fn srp_server_set_service_update_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(&'a ot::Instance, SrpServerServiceUpdateId, &'a SrpServerHost, u32) + 'a,
    {
        unsafe extern "C" fn _ot_srp_server_service_update_handler<'a, F>(
            id: otSrpServerServiceUpdateId,
            host: *const otSrpServerHost,
            timeout: u32,
            context: *mut ::std::os::raw::c_void,
        ) where
            F: FnMut(SrpServerServiceUpdateId, &'a SrpServerHost, u32) + 'a,
        {
            // Reconstitute a reference to our closure.
            let sender = unsafe { &mut *(context as *mut F) };

            sender(
                SrpServerServiceUpdateId::new(id),
                unsafe { SrpServerHost::ref_from_ot_ptr(host) }.unwrap(),
                timeout,
            )
        }

        // This helper func is just to allow us to get the type of the
        // wrapper closure (`f_wrapped`) so we can pass that type
        // as a type argument to `_ot_srp_server_service_update_handler`.
        fn get_service_update_handler<'a, X>(_: &X) -> otSrpServerServiceUpdateHandler
        where
            X: FnMut(SrpServerServiceUpdateId, &'a SrpServerHost, u32) + 'a,
        {
            Some(_ot_srp_server_service_update_handler::<X>)
        }

        let (fn_ptr, fn_box, cb): (_, _, otSrpServerServiceUpdateHandler) = if let Some(mut f) = f {
            // Grab a pointer to our ot::Instance for use below in the wrapper closure.
            let ot_instance_ptr = self.as_ot_ptr();

            // Wrap around `f` with a closure that fills in the `ot_instance` field, which we
            // won't have access to inside of `_ot_srp_server_service_update_handler::<_>`.
            let f_wrapped =
                move |id: SrpServerServiceUpdateId, host: &'a SrpServerHost, timeout: u32| {
                    // SAFETY: This ot::Instance will be passed to the original closure as
                    //         an argument. We know that it is valid because:
                    //         1. It is being called by the instance, so it must still be around.
                    //         2. By design there are no mutable references to the `ot::Instance`
                    //            in existence.
                    let ot_instance =
                        unsafe { ot::Instance::ref_from_ot_ptr(ot_instance_ptr) }.unwrap();
                    f(ot_instance, id, host, timeout)
                };

            // Since we don't have a way to directly refer to the type of the closure `f_wrapped`,
            // we need to use a helper function that can pass the type as a generic argument to
            // `_ot_srp_server_service_update_handler::<X>` as `X`.
            let service_update_handler = get_service_update_handler(&f_wrapped);

            let mut x = Box::new(f_wrapped);

            (
                x.as_mut() as *mut _ as *mut ::std::os::raw::c_void,
                Some(
                    x as Box<
                        dyn FnMut(ot::SrpServerServiceUpdateId, &'a ot::SrpServerHost, u32) + 'a,
                    >,
                ),
                service_update_handler,
            )
        } else {
            (std::ptr::null_mut() as *mut ::std::os::raw::c_void, None, None)
        };

        unsafe {
            otSrpServerSetServiceUpdateHandler(self.as_ot_ptr(), cb, fn_ptr);

            // Make sure our object eventually gets cleaned up.
            // Here we must also transmute our closure to have a 'static lifetime.
            // We need to do this because the borrow checker cannot infer the
            // proper lifetime for the singleton instance backing, but
            // this is guaranteed by the API.
            self.borrow_backing().srp_server_service_update_fn.set(std::mem::transmute::<
                Option<Box<dyn FnMut(SrpServerServiceUpdateId, &'a ot::SrpServerHost, u32) + 'a>>,
                Option<Box<dyn FnMut(SrpServerServiceUpdateId, &ot::SrpServerHost, u32) + 'static>>,
            >(fn_box));
        }
    }

    fn parse_label_from_subtype_service_name(
        &self,
        subtype_service_name: &CStr,
    ) -> Result<CString, Error> {
        let mut bytes = [0 as c_char; 256];

        // SAFETY: We are passing in valid pointers with a length one less than the array size.
        Error::from(unsafe {
            otSrpServerParseSubTypeServiceName(
                subtype_service_name.as_ptr(),
                (&mut bytes) as *mut c_char,
                255,
            )
        })
        .into_result()?;

        // SAFETY: `bytes` is guaranteed to be zero terminated because of the size of the array.
        Ok(unsafe { CStr::from_ptr(&bytes as *const c_char) }.to_owned())
    }
}
