// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;
use std::mem::transmute;

/// DnsUpstream callback from platform to OT
pub trait DnsUpstream {
    /// Update DNS upstream query done
    fn plat_dns_upstream_query_done(
        &self,
        query_context: &PlatDnsUpstreamQuery,
        response: ot::Box<Message<'_>>,
    );
}

impl<T: DnsUpstream + ot::Boxable> DnsUpstream for ot::Box<T> {
    fn plat_dns_upstream_query_done(
        &self,
        query_context: &PlatDnsUpstreamQuery,
        response: ot::Box<Message<'_>>,
    ) {
        self.as_ref().plat_dns_upstream_query_done(query_context, response)
    }
}

// SAFETY: this call will be called only once for each &mut PlatDnsUpstreamQuery passed to lowpan,
//         and the reference will be removed right after the call. It is ok to transmute it to
//         mutable reference.
#[allow(clippy::useless_transmute)]
unsafe fn dns_upstream_query_context_get_mut(
    original: &PlatDnsUpstreamQuery,
) -> *mut otPlatDnsUpstreamQuery {
    unsafe { transmute(original) }
}

impl DnsUpstream for Instance {
    fn plat_dns_upstream_query_done(
        &self,
        query_context: &PlatDnsUpstreamQuery,
        response: ot::Box<Message<'_>>,
    ) {
        unsafe {
            otPlatDnsUpstreamQueryDone(
                self.as_ot_ptr(),
                dns_upstream_query_context_get_mut(query_context),
                response.take_ot_ptr(),
            );
        }
    }
}
