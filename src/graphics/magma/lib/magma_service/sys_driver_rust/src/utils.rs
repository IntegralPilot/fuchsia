// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

macro_rules! debug_assert_ok {
    ($result:expr) => {
        debug_assert!($result.is_ok(), "{:#?}", $result);
    };
}
pub(crate) use debug_assert_ok;
