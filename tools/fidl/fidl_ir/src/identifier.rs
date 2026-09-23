// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::borrow::Borrow;
use core::ops::Deref;

use serde::Deserialize;

/// A FIDL identifier.
#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub struct Identifier {
    string: String,
}

impl Deref for Identifier {
    type Target = Ident;

    fn deref(&self) -> &Self::Target {
        Ident::from_str(&self.string)
    }
}

impl Borrow<Ident> for Identifier {
    fn borrow(&self) -> &Ident {
        Ident::from_str(&self.string)
    }
}

/// A borrowed FIDL identifier.
#[derive(Debug)]
#[repr(transparent)]
pub struct Ident {
    str: str,
}

impl Ident {
    /// Returns a new `Ident` from the given string.
    pub fn from_str(name: &str) -> &Self {
        // SAFETY: `Ident` is a transparent wrapper around `str`, so a `&str` has the same layout as
        // `&Ident`.
        unsafe { &*(name as *const str as *const Self) }
    }

    /// Returns the underlying, non-canonicalized string.
    pub fn non_canonical(&self) -> &str {
        &self.str
    }
}

/// A compound FIDL identifier.
#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct CompoundIdentifier {
    string: String,
}

impl Deref for CompoundIdentifier {
    type Target = CompoundIdent;

    fn deref(&self) -> &CompoundIdent {
        CompoundIdent::from_str(&self.string)
    }
}

impl Borrow<CompoundIdent> for CompoundIdentifier {
    fn borrow(&self) -> &CompoundIdent {
        CompoundIdent::from_str(&self.string)
    }
}

/// A borrowed compound FIDL identifier.
#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CompoundIdent {
    str: str,
}

impl CompoundIdent {
    /// Returns a `CompoundIdent` wrapping the given `str`.
    pub fn from_str(s: &str) -> &Self {
        // SAFETY: `CompoundIdent` is a transparent wrapper around `str`, so a `&str` has the same
        // layout as `&CompoundIdent`.
        unsafe { &*(s as *const str as *const Self) }
    }

    /// Splits this identifier into a library name and decl name.
    pub fn split(&self) -> (&str, &Ident) {
        let (library, type_name) = self.str.split_once('/').unwrap();
        (library, Ident::from_str(type_name))
    }

    /// Returns the library of the identifier.
    pub fn library(&self) -> &str {
        self.split().0
    }

    /// Get the name excluding the library and member name.
    pub fn decl_name(&self) -> &Ident {
        self.split().1
    }
}

impl ToOwned for CompoundIdent {
    type Owned = CompoundIdentifier;

    fn to_owned(&self) -> Self::Owned {
        CompoundIdentifier { string: self.str.to_owned() }
    }
}

/// A compound FIDL identifier which may additionally reference a member.
#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct CompoundIdentifierOrMember {
    string: String,
}

impl CompoundIdentifierOrMember {
    /// Splits this identifier into a library name, decl name, and member name (if any).
    pub fn split(&self) -> (&CompoundIdent, Option<&Ident>) {
        let slash_pos = self.string.find('/').unwrap();
        if let Some(dot_pos) = self.string.rfind('.') {
            if dot_pos > slash_pos {
                return (
                    CompoundIdent::from_str(&self.string[..dot_pos]),
                    Some(Ident::from_str(&self.string[dot_pos + 1..])),
                );
            }
        }
        (CompoundIdent::from_str(&self.string), None)
    }
}
