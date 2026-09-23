// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A variant that fills the device, inducing disk-full errors and tries to avoid the page-cache.
pub mod aggressive;
/// A variant that uses modest storage and tries to exercise all the basic IOP variations.
pub mod gentle;
