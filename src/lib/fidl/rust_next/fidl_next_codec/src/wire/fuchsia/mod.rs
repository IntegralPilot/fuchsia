// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia-specific FIDL wire type definitions and implementations.

mod handle;
mod handle_types;
mod object_type;
mod rights;
mod status;

pub use self::handle::*;
pub use self::handle_types::*;
pub use self::object_type::*;
pub use self::rights::*;
pub use self::status::*;
