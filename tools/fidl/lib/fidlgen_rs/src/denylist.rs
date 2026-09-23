// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::any::Any;
use core::fmt;

use fidl_ir::{Attributes, CompoundIdent, Library, Protocol};

use crate::{Decl, LibraryExt as _};

pub enum Denylist {
    Allowed,
    CfgDriver,
    Denied,
}

impl Denylist {
    fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::Allowed, x) | (x, Self::Allowed) => x,
            (Self::CfgDriver, Self::CfgDriver) => Self::CfgDriver,
            _ => Self::Denied,
        }
    }

    pub fn for_ident(library: &Library, ident: &CompoundIdent, bindings: &[&str]) -> Self {
        let Some(decl) = library.get_local_decl(ident) else {
            return Self::Allowed;
        };

        let mut result = Self::for_decl(decl, bindings);

        if let Some(naming_context) = decl.naming_context() {
            let mut path = format!("{}/", library.name);

            for (name, next) in naming_context.iter().zip(naming_context.iter().skip(1)) {
                path = format!("{path}{name}");
                let comp_id = CompoundIdent::from_str(&path);
                if let Some(parent_decl) = library.get_local_decl(comp_id) {
                    if let Some(protocol) = (parent_decl as &dyn Any).downcast_ref::<Protocol>()
                        && let Some(method) =
                            protocol.methods.iter().find(|m| m.name.non_canonical() == next)
                    {
                        result = result.max(Self::for_attributes(&method.attributes, bindings));
                    }

                    result = result.max(Self::for_decl(parent_decl, bindings));
                }
            }
        }

        result
    }

    fn for_decl(decl: &dyn Decl, bindings: &[&str]) -> Self {
        let mut result = Self::Allowed;

        result = result.max(Self::for_attributes(decl.attributes(), bindings));

        if let Some(protocol) = (decl as &dyn Any).downcast_ref::<Protocol>() {
            // The denylist for a protocol is implicitly affected by its transport
            let transport_denylist = match protocol.transport() {
                Some("Syscall") => Denylist::Denied,
                Some("Driver") => Denylist::CfgDriver,
                _ => Denylist::Allowed,
            };
            result = result.max(transport_denylist);
        }

        result
    }

    fn for_attributes(attributes: &Attributes, bindings: &[&str]) -> Self {
        if let Some(denylist) = attributes.get_value("bindings_denylist") {
            for denied in denylist.split(", ") {
                if bindings.contains(&denied) {
                    return Self::Denied;
                }
            }
        }

        Self::Allowed
    }
}

impl fmt::Display for Denylist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => Ok(()),
            Self::CfgDriver => write!(f, "#[cfg(feature = \"driver\")]"),
            Self::Denied => panic!("attempted to emit a bindings restriction of 'never'"),
        }
    }
}
