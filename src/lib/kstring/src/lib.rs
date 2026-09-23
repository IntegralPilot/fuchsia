// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

pub mod interned_category;
pub mod interned_string;

#[doc(hidden)]
pub use kstring_macro::import_category;
#[doc(hidden)]
pub use kstring_macro::import_string;
#[doc(hidden)]
pub use kstring_macro::interned_category_export_name;
#[doc(hidden)]
pub use kstring_macro::interned_string_export_name;
