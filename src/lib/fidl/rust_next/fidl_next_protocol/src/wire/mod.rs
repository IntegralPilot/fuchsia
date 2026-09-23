// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL protocol wire type definitions and implementations.

use fidl_next_codec::wire::*;

mod empty;
mod epitaph;
mod flexible;
mod message_header;
mod strict;

pub use self::empty::*;
pub use self::epitaph::*;
pub use self::flexible::*;
pub use self::message_header::*;
pub use self::strict::*;
