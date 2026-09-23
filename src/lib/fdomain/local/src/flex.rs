// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "fdomain")]
pub use fdomain_local::*;

#[cfg(not(feature = "fdomain"))]
pub fn local_client_empty() -> fidl::endpoints::ZirconClient {
    fidl::endpoints::ZirconClient
}
