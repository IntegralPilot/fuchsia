// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod child_name;
mod error;
mod extended_moniker;
mod moniker;
#[cfg(feature = "serde")]
mod serde_ext;

pub use self::child_name::{BorrowedChildName, ChildName};
pub use self::error::MonikerError;
pub use self::extended_moniker::{ExtendedMoniker, EXTENDED_MONIKER_COMPONENT_MANAGER_STR};
pub use self::moniker::Moniker;
