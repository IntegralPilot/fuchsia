// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The flexibility of a method.
#[derive(Clone, Copy, Debug)]
pub enum Flexibility {
    /// The method is strict.
    Strict,
    /// The method is flexible.
    Flexible,
}
