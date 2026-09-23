// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Iterator type for multicast listeners.
#[allow(missing_debug_implementations)]
pub struct MulticastListenerIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_listener_iter: otBackboneRouterMulticastListenerIterator,
}

impl<T: ?Sized + BackboneRouter> Iterator for MulticastListenerIterator<'_, T> {
    type Item = BackboneRouterMulticastListenerInfo;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.multicast_listener_get_next(&mut self.ot_listener_iter)
    }
}

/// Methods from the [OpenThread "Backbone Router" Module][1].
/// Currently only multicast routing related methods are added.
///
/// [1]: https://openthread.io/reference/group/api-backbone-router
pub trait BackboneRouter {
    /// Functional equilvanet of
    /// [`otsys::otBackboneRouterSetEnabled`](crate::otsys::otBackboneRouterSetEnabled).
    fn set_backbone_router_enabled(&self, enable: bool);

    /// Functional equivalent of
    /// [`otsys::otBackboneRouterMulticastListenerGetNext`](crate::otsys::otBackboneRouterMulticastListenerGetNext).
    // TODO: Determine if the underlying implementation of
    //       this method has undefined behavior when network data
    //       is being mutated while iterating. If it is undefined,
    //       we may need to make it unsafe and provide a safe method
    //       that collects the results.
    fn multicast_listener_get_next(
        &self,
        listener_iter: &mut otBackboneRouterMulticastListenerIterator,
    ) -> Option<BackboneRouterMulticastListenerInfo>;

    /// Functional equivalent of
    /// [`otsys::otBackboneRouterSetMulticastListenerCallback`](crate::otsys::otBackboneRouterSetMulticastListenerCallback).
    fn set_multicast_listener_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a;

    /// Returns an iterator for iterating over multicast listeners.
    fn iter_multicast_listeners(&self) -> MulticastListenerIterator<'_, Self> {
        MulticastListenerIterator {
            ot_instance: self,
            ot_listener_iter: OT_BACKBONE_ROUTER_MULTICAST_LISTENER_ITERATOR_INIT
                .try_into()
                .unwrap(),
        }
    }
}

impl<T: BackboneRouter + Boxable> BackboneRouter for ot::Box<T> {
    fn set_backbone_router_enabled(&self, enable: bool) {
        self.as_ref().set_backbone_router_enabled(enable)
    }

    fn multicast_listener_get_next(
        &self,
        listener_iter: &mut otBackboneRouterMulticastListenerIterator,
    ) -> Option<BackboneRouterMulticastListenerInfo> {
        self.as_ref().multicast_listener_get_next(listener_iter)
    }

    fn set_multicast_listener_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a,
    {
        self.as_ref().set_multicast_listener_callback(f)
    }
}

impl BackboneRouter for Instance {
    fn set_backbone_router_enabled(&self, enable: bool) {
        unsafe { otBackboneRouterSetEnabled(self.as_ot_ptr(), enable) }
    }

    fn multicast_listener_get_next(
        &self,
        listener_iter: &mut otBackboneRouterMulticastListenerIterator,
    ) -> Option<BackboneRouterMulticastListenerInfo> {
        unsafe {
            let mut ret = BackboneRouterMulticastListenerInfo::default();
            match Error::from(otBackboneRouterMulticastListenerGetNext(
                self.as_ot_ptr(),
                listener_iter as *mut otBackboneRouterMulticastListenerIterator,
                ret.as_ot_mut_ptr(),
            )) {
                Error::NotFound => None,
                Error::None => Some(ret),
                err => {
                    // If something's wrong in OpenThread when iterate multicast listeners,
                    // do not crash `lowpan-ot-driver`.
                    warn!(
                        "Unexpected error from otBackboneRouterMulticastListenerIterator: {err:?}"
                    );
                    None
                }
            }
        }
    }

    fn set_multicast_listener_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a,
    {
        unsafe extern "C" fn _ot_backbone_router_multicast_listener_callback<
            'a,
            F: FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a,
        >(
            context: *mut ::std::os::raw::c_void,
            event: otBackboneRouterMulticastListenerEvent,
            address: *const otIp6Address,
        ) {
            trace!("_ot_backbone_router_multicast_listener_callback");

            // Convert the `*otIp6Address` into an `&ot::Ip6AddressInfo`.
            let address = unsafe { Ip6Address::ref_from_ot_ptr(address).unwrap() };

            // Convert `otBackboneRouterMulticastListenerCallback` to
            // `BackboneRouterMulticastListenerEvent`
            let event = BackboneRouterMulticastListenerEvent::from(event);

            // Reconstitute a reference to our closure.
            let sender = unsafe { &mut *(context as *mut F) };

            sender(event, address)
        }

        let (fn_ptr, fn_box, cb): (_, _, otBackboneRouterMulticastListenerCallback) =
            if let Some(f) = f {
                let mut x = Box::new(f);

                (
                    x.as_mut() as *mut F as *mut ::std::os::raw::c_void,
                    Some(
                        x as Box<dyn FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a>,
                    ),
                    Some(_ot_backbone_router_multicast_listener_callback::<F>),
                )
            } else {
                (std::ptr::null_mut() as *mut ::std::os::raw::c_void, None, None)
            };

        unsafe {
            otBackboneRouterSetMulticastListenerCallback(self.as_ot_ptr(), cb, fn_ptr);

            // Make sure our object eventually gets cleaned up.
            // Here we must also transmute our closure to have a 'static lifetime.
            // We need to do this because the borrow checker cannot infer the
            // proper lifetime for the singleton instance backing, but
            // this is guaranteed by the API.
            self.borrow_backing().multicast_listener_callback.set(std::mem::transmute::<
                Option<Box<dyn FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'a>>,
                Option<Box<dyn FnMut(BackboneRouterMulticastListenerEvent, &Ip6Address) + 'static>>,
            >(fn_box));
        }
    }
}
