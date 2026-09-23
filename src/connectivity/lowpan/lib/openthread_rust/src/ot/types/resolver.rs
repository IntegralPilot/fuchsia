// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents the DNS Upstream Query context.
///
/// Functional equivalent of [`otsys::otPlatDnsUpstreamQuery`](crate::otsys::otPlatDnsUpstreamQuery).
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct PlatDnsUpstreamQuery(otPlatDnsUpstreamQuery);

impl_ot_castable!(opaque PlatDnsUpstreamQuery, otPlatDnsUpstreamQuery);
