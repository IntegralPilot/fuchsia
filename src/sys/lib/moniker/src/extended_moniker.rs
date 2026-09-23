// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::MonikerError;
use crate::moniker::Moniker;
use cm_rust::{FidlIntoNative, NativeIntoFidl};
use core::cmp::Ord;
use std::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// One of:
/// - A moniker
/// - A marker representing component manager's realm
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Eq, Ord, PartialOrd, PartialEq, Debug, Clone, Hash)]
pub enum ExtendedMoniker {
    ComponentInstance(Moniker),
    ComponentManager,
}

/// The string representation of ExtendedMoniker::ComponentManager
pub const EXTENDED_MONIKER_COMPONENT_MANAGER_STR: &'static str = "<component_manager>";

impl ExtendedMoniker {
    pub fn unwrap_instance_moniker_or<E: std::error::Error>(
        &self,
        error: E,
    ) -> Result<&Moniker, E> {
        match self {
            Self::ComponentManager => Err(error),
            Self::ComponentInstance(moniker) => Ok(moniker),
        }
    }

    pub fn has_prefix(&self, other: &Self) -> bool {
        match (self, other) {
            (_, Self::ComponentManager) => true,
            (Self::ComponentManager, Self::ComponentInstance(_)) => false,
            (Self::ComponentInstance(a), Self::ComponentInstance(b)) => a.has_prefix(b),
        }
    }

    pub fn parse_str(rep: &str) -> Result<Self, MonikerError> {
        if rep == EXTENDED_MONIKER_COMPONENT_MANAGER_STR {
            Ok(ExtendedMoniker::ComponentManager)
        } else {
            Ok(ExtendedMoniker::ComponentInstance(Moniker::parse_str(rep)?))
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::ComponentInstance(m) => m.as_str(),
            Self::ComponentManager => EXTENDED_MONIKER_COMPONENT_MANAGER_STR,
        }
    }
}

impl TryFrom<&str> for ExtendedMoniker {
    type Error = MonikerError;

    fn try_from(input: &str) -> Result<Self, MonikerError> {
        Self::parse_str(input)
    }
}

impl std::str::FromStr for ExtendedMoniker {
    type Err = MonikerError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_str(s)
    }
}

impl fmt::Display for ExtendedMoniker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentInstance(m) => {
                write!(f, "{}", m)?;
            }
            Self::ComponentManager => {
                write!(f, "{}", EXTENDED_MONIKER_COMPONENT_MANAGER_STR)?;
            }
        }
        Ok(())
    }
}

impl AsRef<str> for ExtendedMoniker {
    fn as_ref(&self) -> &str {
        match self {
            Self::ComponentInstance(m) => m.as_ref(),
            Self::ComponentManager => EXTENDED_MONIKER_COMPONENT_MANAGER_STR,
        }
    }
}

impl From<Moniker> for ExtendedMoniker {
    fn from(m: Moniker) -> Self {
        Self::ComponentInstance(m)
    }
}

impl FidlIntoNative<ExtendedMoniker> for String {
    fn fidl_into_native(self) -> ExtendedMoniker {
        self.parse().unwrap()
    }
}

impl NativeIntoFidl<String> for ExtendedMoniker {
    fn native_into_fidl(self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(
            ExtendedMoniker::parse_str(EXTENDED_MONIKER_COMPONENT_MANAGER_STR).unwrap(),
            ExtendedMoniker::ComponentManager
        );
        assert_eq!(
            ExtendedMoniker::parse_str("/foo/bar").unwrap(),
            ExtendedMoniker::ComponentInstance(Moniker::parse_str("/foo/bar").unwrap())
        );
        assert!(ExtendedMoniker::parse_str("").is_err(), "cannot be empty");
    }

    #[test]
    fn has_prefix() {
        let cm = ExtendedMoniker::ComponentManager;
        let root = ExtendedMoniker::ComponentInstance(Moniker::root());
        let a = ExtendedMoniker::ComponentInstance(Moniker::parse_str("a").unwrap());
        let ab = ExtendedMoniker::ComponentInstance(Moniker::parse_str("a/b").unwrap());

        assert!(cm.has_prefix(&cm));
        assert!(!cm.has_prefix(&root));
        assert!(!cm.has_prefix(&a));
        assert!(!cm.has_prefix(&ab));

        assert!(root.has_prefix(&cm));
        assert!(root.has_prefix(&root));
        assert!(!root.has_prefix(&a));
        assert!(!root.has_prefix(&ab));

        assert!(a.has_prefix(&cm));
        assert!(a.has_prefix(&root));
        assert!(a.has_prefix(&a));
        assert!(!a.has_prefix(&ab));

        assert!(ab.has_prefix(&cm));
        assert!(ab.has_prefix(&root));
        assert!(ab.has_prefix(&a));
        assert!(ab.has_prefix(&ab));
    }

    #[test]
    fn to_string_functions() {
        let cm_moniker =
            ExtendedMoniker::parse_str(EXTENDED_MONIKER_COMPONENT_MANAGER_STR).unwrap();
        let foobar_moniker = ExtendedMoniker::parse_str("foo/bar").unwrap();
        let empty_moniker = ExtendedMoniker::parse_str(".").unwrap();

        assert_eq!(format!("{}", cm_moniker), EXTENDED_MONIKER_COMPONENT_MANAGER_STR.to_string());
        assert_eq!(cm_moniker.to_string(), EXTENDED_MONIKER_COMPONENT_MANAGER_STR.to_string());
        assert_eq!(format!("{}", foobar_moniker), "foo/bar".to_string());
        assert_eq!(foobar_moniker.to_string(), "foo/bar".to_string());
        assert_eq!(format!("{}", empty_moniker), ".".to_string());
        assert_eq!(empty_moniker.to_string(), ".".to_string());
    }
}
