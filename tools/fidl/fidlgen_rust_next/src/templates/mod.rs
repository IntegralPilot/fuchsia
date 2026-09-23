// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Templates generate a lot of code which have tendencies to trip lints.
// TODO(501102481): Remove the unreachable code and the `allow` after rolling to the new toolchain
#![expect(clippy::diverging_sub_expression, dead_code, unreachable_code)]
#![allow(unfulfilled_lint_expectations)]

mod alias;
mod bits;
mod compound_identifier;
mod r#const;
mod constant;
mod context;
mod doc_string;
mod r#enum;
mod library;
mod natural_type;
mod prim;
mod protocol;
mod service;
mod r#struct;
mod table;
mod r#union;
mod validation;
mod wire_type;

pub use self::alias::*;
pub use self::bits::*;
pub use self::compound_identifier::*;
pub use self::r#const::*;
pub use self::constant::*;
pub use self::context::*;
pub use self::doc_string::*;
pub use self::r#enum::*;
pub use self::library::*;
pub use self::natural_type::*;
pub use self::prim::*;
pub use self::protocol::*;
pub use self::service::*;
pub use self::r#struct::*;
pub use self::table::*;
pub use self::r#union::*;
pub use self::validation::*;
pub use self::wire_type::*;
