// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The default tag used when an object participates in only one list.
///
/// Tags are used to disambiguate which node state should be used when an object
/// participates in multiple intrusive containers simultaneously. By providing a
/// default tag, we make the common case (participating in a single list) more
/// ergonomic by not requiring the user to specify a tag explicitly.
pub struct DefaultObjectTag;
