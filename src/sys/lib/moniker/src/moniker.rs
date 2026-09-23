// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::child_name::{BorrowedChildName, ChildName};
use crate::error::MonikerError;
use cm_rust::{FidlIntoNative, NativeIntoFidl};
use core::cmp::{self, Ordering, PartialEq};
use flyweights::FlyStr;
use std::fmt;
use std::hash::Hash;
use std::iter::{IntoIterator, Iterator};

/// [Moniker] describes the identity of a component instance in terms of its path relative to the
/// root of the component instance tree.
///
/// Display notation: ".", "name1", "name1/name2", ...
#[derive(Eq, PartialEq, Clone, Hash)]
pub struct Moniker {
    rep: FlyStr,
}

impl Moniker {
    pub fn new(path: &[ChildName]) -> Self {
        if path.is_empty() {
            Self::root()
        } else {
            Self { rep: path.iter().map(|s| s.as_ref()).collect::<Box<[&str]>>().join("/").into() }
        }
    }

    fn new_unchecked<S: AsRef<str> + ?Sized>(rep: &S) -> Self {
        Self { rep: rep.as_ref().into() }
    }

    pub fn new_from_borrowed(path: &[&BorrowedChildName]) -> Self {
        if path.is_empty() {
            Self::root()
        } else {
            Self {
                rep: path.iter().map(|s| (*s).as_ref()).collect::<Box<[&str]>>().join("/").into(),
            }
        }
    }

    pub fn path(&self) -> Box<[&BorrowedChildName]> {
        if self.is_root() {
            Box::new([])
        } else {
            self.rep.split('/').map(|s| BorrowedChildName::new_unchecked(s)).collect()
        }
    }

    pub fn parse<T: AsRef<str>>(path: &[T]) -> Result<Self, MonikerError> {
        if path.is_empty() {
            return Ok(Self::root());
        }
        let path = path
            .iter()
            .map(|n| {
                let _ = BorrowedChildName::parse(n.as_ref())?;
                Ok(n.as_ref())
            })
            .collect::<Result<Box<[&str]>, MonikerError>>()?;
        Ok(Self::new_unchecked(&path.join("/")))
    }

    pub fn parse_str(input: &str) -> Result<Self, MonikerError> {
        if input.is_empty() {
            return Err(MonikerError::invalid_moniker(input));
        }
        if input == "/" || input == "." || input == "./" {
            return Ok(Self::root());
        }

        // Optionally strip a prefix of "/" or "./".
        let stripped = match input.strip_prefix("/") {
            Some(s) => s,
            None => match input.strip_prefix("./") {
                Some(s) => s,
                None => input,
            },
        };
        stripped
            .split('/')
            .into_iter()
            .map(|s| {
                let _ = BorrowedChildName::parse(s)?;
                Ok::<(), MonikerError>(())
            })
            .collect::<Result<(), _>>()?;
        Ok(Self::new_unchecked(stripped))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.rep
    }

    /// Concatenates other onto the end of this moniker.
    pub fn concat(&self, other: &Moniker) -> Self {
        let rep = if self.is_root() {
            other.rep.clone()
        } else if !other.is_root() {
            format!("{}/{}", self.rep, other.rep).into()
        } else {
            self.rep.clone()
        };
        Self::new_unchecked(&rep)
    }

    /// Indicates whether this moniker is prefixed by prefix.
    pub fn has_prefix(&self, prefix: &Moniker) -> bool {
        if prefix.is_root() {
            return true;
        } else if self.path().len() < prefix.path().len() {
            return false;
        }

        let my_segments =
            self.rep.split('/').map(|s| BorrowedChildName::new_unchecked(s)).collect::<Box<_>>();
        let prefix_segments =
            prefix.rep.split('/').map(|s| BorrowedChildName::new_unchecked(s)).collect::<Box<_>>();
        my_segments[..prefix_segments.len()] == *prefix_segments
    }

    pub fn root() -> Self {
        Self { rep: ".".into() }
    }

    /// Returns the last child of this moniker if this is not the root moniker.
    pub fn leaf(&self) -> Option<&BorrowedChildName> {
        if self.is_root() {
            None
        } else {
            let back = match self.rep.rfind('/') {
                Some(i) => &self.rep[i + 1..],
                None => &self.rep,
            };
            Some(BorrowedChildName::new_unchecked(back))
        }
    }

    pub fn is_root(&self) -> bool {
        self.rep == "."
    }

    /// Creates a new moniker with the last child removed. Returns `None` if this is the root
    /// moniker.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            None
        } else {
            match self.rep.rfind('/') {
                Some(i) => Some(Self::new_unchecked(&self.rep[0..i])),
                None => Some(Self::root()),
            }
        }
    }

    /// Creates a new Moniker with `child` added to the end of this moniker.
    pub fn child(&self, child: ChildName) -> Self {
        if self.is_root() {
            Self::new_unchecked(&child)
        } else {
            Self::new_unchecked(&format!("{self}/{child}"))
        }
    }

    /// Splits off the last child of this moniker returning the parent as a moniker and the child.
    /// Returns `None` if this is the root moniker.
    pub fn split_leaf(&self) -> Option<(Self, &BorrowedChildName)> {
        if self.is_root() {
            None
        } else {
            let (rest, back) = match self.rep.rfind('/') {
                Some(i) => {
                    let path = Self::new_unchecked(&self.rep[0..i]);
                    let back = BorrowedChildName::new_unchecked(&self.rep[i + 1..]);
                    (path, back)
                }
                None => (Self::root(), BorrowedChildName::new_unchecked(&self.rep)),
            };
            Some((rest, back))
        }
    }

    /// Strips the moniker parts in prefix from the beginning of this moniker.
    pub fn strip_prefix(&self, prefix: &Moniker) -> Result<Self, MonikerError> {
        if !self.has_prefix(prefix) {
            return Err(MonikerError::MonikerDoesNotHavePrefix {
                moniker: self.to_string(),
                prefix: prefix.to_string(),
            });
        }

        if prefix.is_root() {
            Ok(self.clone())
        } else if self == prefix {
            Ok(Self::root())
        } else {
            assert!(!self.is_root(), "strip_prefix: caught by has_prefix above");
            Ok(Self::new_unchecked(&self.rep[prefix.rep.len() + 1..]))
        }
    }
}

impl FidlIntoNative<Moniker> for String {
    fn fidl_into_native(self) -> Moniker {
        // This is used in capability_source::CapabilitySource, and the FIDL version of
        // this should only be generated in-process from already valid monikers.
        self.parse().unwrap()
    }
}

impl NativeIntoFidl<String> for Moniker {
    fn native_into_fidl(self) -> String {
        self.to_string()
    }
}

impl Default for Moniker {
    fn default() -> Self {
        Self::root()
    }
}

impl TryFrom<&[&str]> for Moniker {
    type Error = MonikerError;

    fn try_from(rep: &[&str]) -> Result<Self, MonikerError> {
        Self::parse(rep)
    }
}

impl<const N: usize> TryFrom<[&str; N]> for Moniker {
    type Error = MonikerError;

    fn try_from(rep: [&str; N]) -> Result<Self, MonikerError> {
        Self::parse(&rep)
    }
}

impl TryFrom<&str> for Moniker {
    type Error = MonikerError;

    fn try_from(input: &str) -> Result<Self, MonikerError> {
        Self::parse_str(input)
    }
}

impl std::str::FromStr for Moniker {
    type Err = MonikerError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_str(s)
    }
}

impl cmp::Ord for Moniker {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        let self_path = self.path();
        let other_path = other.path();
        let min_size = cmp::min(self_path.len(), other_path.len());
        for i in 0..min_size {
            if self_path[i] < other_path[i] {
                return cmp::Ordering::Less;
            } else if self_path[i] > other_path[i] {
                return cmp::Ordering::Greater;
            }
        }
        if self_path.len() > other_path.len() {
            return cmp::Ordering::Greater;
        } else if self_path.len() < other_path.len() {
            return cmp::Ordering::Less;
        }

        cmp::Ordering::Equal
    }
}

impl PartialOrd for Moniker {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Moniker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rep)
    }
}

impl fmt::Debug for Moniker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl AsRef<str> for Moniker {
    fn as_ref(&self) -> &str {
        &self.rep
    }
}

impl<'a> IntoIterator for &'a Moniker {
    type Item = &'a str;
    type IntoIter = std::str::Split<'a, char>;

    fn into_iter(self) -> Self::IntoIter {
        self.rep.split('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_types::BorrowedName;

    #[test]
    fn monikers() {
        let root = Moniker::root();
        assert_eq!(true, root.is_root());
        assert_eq!(".", format!("{}", root));
        assert_eq!(root, Moniker::new(&[]));
        assert_eq!(root, Moniker::try_from([]).unwrap());

        let m = Moniker::new(&[
            ChildName::try_new("a", None).unwrap(),
            ChildName::try_new("b", Some("coll")).unwrap(),
        ]);
        assert_eq!(false, m.is_root());
        assert_eq!("a/coll:b", format!("{}", m));
        assert_eq!(m, Moniker::try_from(["a", "coll:b"]).unwrap());
        assert_eq!(
            m.leaf().map(|m| m.collection()).flatten(),
            Some(BorrowedName::new("coll").unwrap())
        );
        assert_eq!(m.leaf().map(|m| m.name().as_str()), Some("b"));
        assert_eq!(m.leaf(), Some(BorrowedChildName::parse("coll:b").unwrap()));
    }

    #[test]
    fn moniker_parent() {
        let root = Moniker::root();
        assert_eq!(true, root.is_root());
        assert_eq!(None, root.parent());

        let m = Moniker::new(&[
            ChildName::try_new("a", None).unwrap(),
            ChildName::try_new("b", None).unwrap(),
        ]);
        assert_eq!("a/b", format!("{}", m));
        assert_eq!("a", format!("{}", m.parent().unwrap()));
        assert_eq!(".", format!("{}", m.parent().unwrap().parent().unwrap()));
        assert_eq!(None, m.parent().unwrap().parent().unwrap().parent());
        assert_eq!(m.leaf(), Some(BorrowedChildName::parse("b").unwrap()));
    }

    #[test]
    fn moniker_concat() {
        let scope_root: Moniker = ["a:test1", "b:test2"].try_into().unwrap();

        let relative: Moniker = ["c:test3", "d:test4"].try_into().unwrap();
        let descendant = scope_root.concat(&relative);
        assert_eq!("a:test1/b:test2/c:test3/d:test4", format!("{}", descendant));

        let relative: Moniker = [].try_into().unwrap();
        let descendant = scope_root.concat(&relative);
        assert_eq!("a:test1/b:test2", format!("{}", descendant));
    }

    #[test]
    fn moniker_parse_str() {
        assert_eq!(Moniker::try_from("/foo").unwrap(), Moniker::try_from(["foo"]).unwrap());
        assert_eq!(Moniker::try_from("./foo").unwrap(), Moniker::try_from(["foo"]).unwrap());
        assert_eq!(Moniker::try_from("foo").unwrap(), Moniker::try_from(["foo"]).unwrap());
        assert_eq!(Moniker::try_from("/").unwrap(), Moniker::try_from([]).unwrap());
        assert_eq!(Moniker::try_from("./").unwrap(), Moniker::try_from([]).unwrap());

        assert!(Moniker::try_from("//foo").is_err());
        assert!(Moniker::try_from(".//foo").is_err());
        assert!(Moniker::try_from("/./foo").is_err());
        assert!(Moniker::try_from("../foo").is_err());
        assert!(Moniker::try_from(".foo").is_err());
    }

    #[test]
    fn moniker_has_prefix() {
        assert!(Moniker::parse_str("a").unwrap().has_prefix(&Moniker::parse_str("a").unwrap()));
        assert!(Moniker::parse_str("a/b").unwrap().has_prefix(&Moniker::parse_str("a").unwrap()));
        assert!(
            Moniker::parse_str("a/b:test").unwrap().has_prefix(&Moniker::parse_str("a").unwrap())
        );
        assert!(
            Moniker::parse_str("a/b/c/d")
                .unwrap()
                .has_prefix(&Moniker::parse_str("a/b/c").unwrap())
        );
        assert!(
            !Moniker::parse_str("a/b").unwrap().has_prefix(&Moniker::parse_str("a/b/c").unwrap())
        );
        assert!(
            !Moniker::parse_str("a/c").unwrap().has_prefix(&Moniker::parse_str("a/b/c").unwrap())
        );
        assert!(!Moniker::root().has_prefix(&Moniker::parse_str("a").unwrap()));
        assert!(
            !Moniker::parse_str("a/b:test")
                .unwrap()
                .has_prefix(&Moniker::parse_str("a/b").unwrap())
        );
    }

    #[test]
    fn moniker_child() {
        assert_eq!(
            Moniker::root().child(ChildName::try_from("a").unwrap()),
            Moniker::parse_str("a").unwrap()
        );
        assert_eq!(
            Moniker::parse_str("a").unwrap().child(ChildName::try_from("b").unwrap()),
            Moniker::parse_str("a/b").unwrap()
        );
        assert_eq!(
            Moniker::parse_str("a:test").unwrap().child(ChildName::try_from("b").unwrap()),
            Moniker::parse_str("a:test/b").unwrap()
        );
        assert_eq!(
            Moniker::parse_str("a").unwrap().child(ChildName::try_from("b:test").unwrap()),
            Moniker::parse_str("a/b:test").unwrap()
        );
    }

    #[test]
    fn moniker_split_leaf() {
        assert_eq!(Moniker::root().split_leaf(), None);
        assert_eq!(
            Moniker::parse_str("a/b:test").unwrap().split_leaf(),
            Some((Moniker::parse_str("a").unwrap(), BorrowedChildName::parse("b:test").unwrap()))
        );
    }

    #[test]
    fn moniker_strip_prefix() {
        assert_eq!(
            Moniker::parse_str("a").unwrap().strip_prefix(&Moniker::parse_str("a").unwrap()),
            Ok(Moniker::root())
        );
        assert_eq!(
            Moniker::parse_str("a/b").unwrap().strip_prefix(&Moniker::parse_str("a").unwrap()),
            Ok(Moniker::parse_str("b").unwrap())
        );
        assert!(
            Moniker::parse_str("a/b")
                .unwrap()
                .strip_prefix(&Moniker::parse_str("b").unwrap())
                .is_err()
        );
        assert!(Moniker::root().strip_prefix(&Moniker::parse_str("b").unwrap()).is_err());
    }
}
