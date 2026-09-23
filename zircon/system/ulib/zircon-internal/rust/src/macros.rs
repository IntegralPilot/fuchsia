// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const KB: usize = 1024;
pub const MB: usize = 1024 * KB;
pub const GB: usize = 1024 * MB;

// Expands to a string literial containing the filename and line number of the
// point at which it is evaluated.  E.g. "/somedir/somefile.cc:123".
//
// TODO(maniscalco): Consider stripping the path off the filename component
// (think basename).
#[macro_export]
macro_rules! source_tag {
    () => {
        concat!(file!(), ":", line!())
    };
}
