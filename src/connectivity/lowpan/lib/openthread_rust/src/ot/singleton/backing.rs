// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use std::cell::{Cell, RefCell};
use std::task::Waker;

/// Backing struct for OpenThread instances. This type contains information
/// related to the rust API implementation.
#[allow(clippy::type_complexity)]
pub(crate) struct InstanceBacking {
    pub waker: Cell<Waker>,
    pub platform: RefCell<std::boxed::Box<dyn Platform>>,
    pub state_change_fn: Cell<Option<std::boxed::Box<dyn FnMut(ot::ChangedFlags)>>>,
    pub cli_output_fn: Cell<Option<std::boxed::Box<dyn FnMut(&CStr)>>>,
    pub ip6_receive_fn: Cell<Option<std::boxed::Box<dyn FnMut(OtMessageBox<'_>)>>>,
    pub ip6_address_fn: Cell<Option<std::boxed::Box<dyn FnMut(Ip6AddressInfo<'_>, bool)>>>,
    pub multicast_listener_callback:
        Cell<Option<std::boxed::Box<dyn FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address)>>>,
    pub active_scan_fn: Cell<Option<std::boxed::Box<dyn FnMut(Option<&ActiveScanResult>)>>>,
    pub energy_scan_fn: Cell<Option<std::boxed::Box<dyn FnMut(Option<&EnergyScanResult>)>>>,
    pub joiner_fn: Cell<Option<std::boxed::Box<dyn FnOnce(Result)>>>,
    pub srp_server_service_update_fn: Cell<
        Option<std::boxed::Box<dyn FnMut(ot::SrpServerServiceUpdateId, &ot::SrpServerHost, u32)>>,
    >,
    pub dnssd_query_sub_unsub_fn: Cell<Option<std::boxed::Box<dyn FnMut(bool, &CStr)>>>,
    pub nat64_receive_fn: Cell<Option<std::boxed::Box<dyn FnMut(OtMessageBox<'_>)>>>,
    pub dhcp6pd_state_change_callback_fn:
        Cell<Option<std::boxed::Box<dyn FnMut(BorderRoutingDhcp6PdState)>>>,
    pub ephemeral_key_callback: Cell<Option<std::boxed::Box<dyn FnMut()>>>,
    pub meshcop_service_changed_callback: Cell<Option<std::boxed::Box<dyn FnMut()>>>,
}

impl InstanceBacking {
    pub(crate) fn new<T: Platform + 'static>(platform: T) -> Self {
        Self {
            waker: Cell::new(std::task::Waker::noop().clone()),
            platform: RefCell::new(Box::new(platform) as std::boxed::Box<dyn Platform>),
            cli_output_fn: Cell::new(None),
            ip6_receive_fn: Cell::new(None),
            ip6_address_fn: Cell::new(None),
            state_change_fn: Cell::new(None),
            active_scan_fn: Cell::new(None),
            energy_scan_fn: Cell::new(None),
            joiner_fn: Cell::new(None),
            srp_server_service_update_fn: Cell::new(None),
            dnssd_query_sub_unsub_fn: Cell::new(None),
            multicast_listener_callback: Cell::new(None),
            nat64_receive_fn: Cell::new(None),
            dhcp6pd_state_change_callback_fn: Cell::new(None),
            ephemeral_key_callback: Cell::new(None),
            meshcop_service_changed_callback: Cell::new(None),
        }
    }
}

impl InstanceBacking {
    unsafe fn glob() -> &'static mut Option<InstanceBacking> {
        static mut SINGLETON_BACKING: Option<InstanceBacking> = None;
        // TODO(b/319328255) -- Fix usage so lint no longer applies
        #[allow(static_mut_refs)]
        unsafe {
            &mut SINGLETON_BACKING
        }
    }

    /// Returns a reference to the global singleton `InstanceBacking`.
    ///
    /// If you are tempted to use this method, consider using `Instance::borrow_backing()`
    /// instead.
    ///
    /// ## Safety ##
    ///
    /// The behavior of this method is undefined when called at the same
    /// time that `InstanceBacking::set_singleton()`/`InstanceBacking::drop_singleton()`
    /// is being called from a different thread.
    ///
    /// In order to keep things safe and straightforward, this method should
    /// only be called by the implementation of `Instance::borrow_backing()`.
    pub(crate) unsafe fn as_ref() -> &'static InstanceBacking {
        unsafe { Self::glob() }.as_ref().expect("otInstance is uninitialized")
    }

    /// Initializes the singleton instance backing with the given value.
    /// Will panic if this is called twice without calling
    /// `InstanceBacking::finalize` in between.
    ///
    /// ## Safety ##
    ///
    /// The behavior of this method is undefined when called at the same
    /// time that `InstanceBacking::as_ref()`/`InstanceBacking::drop_singleton()`
    /// is being called from a different thread.
    ///
    /// In order to keep things safe and straightforward, this method should
    /// only be called by the implementation of `Instance::new()`.
    pub(crate) unsafe fn set_singleton(backing: InstanceBacking) {
        trace!("Setting Singleton InstanceBacking");
        assert!(
            unsafe { Self::glob() }.replace(backing).is_none(),
            "Tried to make two OpenThread instances"
        );
    }

    /// Finalizes and drops the singleton instance backing. After this
    /// method is called, [`InstacnceBacking::set`] may be called again.
    ///
    /// ## Safety ##
    ///
    /// The behavior of this method is undefined when called at the same
    /// time that `InstanceBacking::set_singleton()`/`InstanceBacking::as_ref()`
    /// is being called from a different thread.
    ///
    /// In order to keep things safe and straightforward, this method should
    /// only be called by the implementation of `Instance::finalize()`.
    pub(crate) unsafe fn drop_singleton() {
        trace!("Dropping Singleton InstanceBacking");
        assert!(
            unsafe { Self::glob() }.take().is_some(),
            "Tried to drop singleton that was never allocated"
        );
    }
}
