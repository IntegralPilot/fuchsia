// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides trait and functions to log mapping from IDs of an enum to its variants
//! or mapped variant values.
//!
//! ## Example
//!
//! ```rust
//! use diagnostics_assertions::assert_data_tree;
//! use fuchsia_inspect::Inspector;
//! use strum_macros::{Display, EnumIter};
//!
//! #[derive(Display, EnumIter)]
//! enum Enum {
//!     A,
//!     B,
//! }
//!
//! impl IdEnum for Enum {
//!     type Id = u8;
//!     fn to_id(&self) -> Self::Id {
//!         match self {
//!             Self::A => 0,
//!             Self::B => 1,
//!         }
//!     }
//! }
//!
//! let inspector = Inspector::default();
//! inspect_record_id_enum::<Enum>(inspector.root(), "enum");
//! assert_data_tree!(inspector, root: {
//!     "enum": {
//!         "0": "A",
//!         "1": "B",
//!     }
//! });
//!
//! inspect_record_id_enum_mapped::<Enum, _>(inspector.root(), "highlight", |variant| {
//!     match variant {
//!         Enum::A => None,
//!         Enum::B => Some(2u8),
//!     }
//! });
//! assert_data_tree!(inspector, root: contains {
//!     "highlight": {
//!         "1": 2u64,
//!     }
//! });
//! ```

use fuchsia_inspect::Node as InspectNode;
use std::fmt::Display;
use strum;

use crate::log::WriteInspect;

pub trait IdEnum {
    type Id: Into<u64>;
    fn to_id(&self) -> Self::Id;
}

/// Record the mapping of `IdEnum`'s ids to variants in Inspect.
pub fn inspect_record_id_enum<R>(node: &InspectNode, name: &str)
where
    R: IdEnum + strum::IntoEnumIterator + Display,
{
    let enum_node = node.create_child(name);
    for variant in R::iter() {
        enum_node.record_string(variant.to_id().into().to_string(), format!("{variant}"));
    }
    node.record(enum_node);
}

/// Record the mapping of `IdEnum`'s ids to mapped variant values.
/// Only variants with mapped `Some` values are recorded.
pub fn inspect_record_id_enum_mapped<R, V>(
    node: &InspectNode,
    name: &str,
    map_fn: impl Fn(&R) -> Option<V>,
) where
    R: IdEnum + strum::IntoEnumIterator,
    V: WriteInspect,
{
    let enum_node = node.create_child(name);
    for variant in R::iter() {
        let value = map_fn(&variant);
        if let Some(value) = value {
            let key = variant.to_id().into().to_string();
            value.write_inspect(&enum_node, key);
        }
    }
    node.record(enum_node);
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;
    use strum_macros::{Display, EnumIter};

    #[derive(Display, EnumIter)]
    enum Enum {
        A,
        B,
    }

    impl IdEnum for Enum {
        type Id = u8;
        fn to_id(&self) -> Self::Id {
            match self {
                Self::A => 0,
                Self::B => 1,
            }
        }
    }

    #[fuchsia::test]
    async fn test_inspect_record_id_enum() {
        let inspector = Inspector::default();
        inspect_record_id_enum::<Enum>(inspector.root(), "enum");
        assert_data_tree!(inspector, root: {
            "enum": {
                "0": "A",
                "1": "B",
            }
        });
    }

    #[fuchsia::test]
    async fn test_inspect_record_id_enum_mapped() {
        let inspector = Inspector::default();
        inspect_record_id_enum_mapped::<Enum, _>(inspector.root(), "highlight", |variant| {
            match variant {
                Enum::A => None,
                Enum::B => Some(2u8),
            }
        });
        assert_data_tree!(inspector, root: {
            "highlight": {
                "1": 2u64,
            },
        });
    }
}
