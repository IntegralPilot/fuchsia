// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::MonikerError;
use cm_types::{BorrowedLongName, BorrowedName, LongName, Name};
use flyweights::FlyStr;
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// A [ChildName] locally identifies a child component instance using the name assigned by
/// its parent and its collection (if present). It is the building block of [Moniker].
///
/// Display notation: "[collection:]name".
#[derive(PartialEq, Eq, Clone)]
pub struct ChildName {
    rep: FlyStr,
}

impl ChildName {
    pub fn new(name: LongName, collection: Option<Name>) -> Self {
        match collection {
            Some(collection) => Self { rep: format!("{collection}:{name}").into() },
            None => Self { rep: format!("{name}").into() },
        }
    }

    pub fn try_new<S>(name: S, collection: Option<S>) -> Result<Self, MonikerError>
    where
        S: AsRef<str> + Into<String>,
    {
        let name = name.as_ref();
        let rep = match collection {
            Some(collection) => Self { rep: format!("{}:{name}", collection.as_ref()).into() },
            None => Self { rep: format!("{name}").into() },
        };
        Self::parse(rep)
    }

    /// Parses a `ChildName` from a string.
    ///
    /// Input strings should be of the format `[collection:]name`, e.g. `foo` or `biz:foo`.
    pub fn parse<T: AsRef<str>>(rep: T) -> Result<Self, MonikerError> {
        validate_child_name(rep.as_ref())?;
        Ok(Self { rep: rep.as_ref().into() })
    }

    pub fn name(&self) -> &BorrowedLongName {
        match self.rep.find(':') {
            Some(i) => {
                BorrowedLongName::new(&self.rep[i + 1..]).expect("name guaranteed to be valid")
            }
            None => BorrowedLongName::new(&self.rep).expect("name guaranteed to be valid"),
        }
    }

    pub fn collection(&self) -> Option<&BorrowedName> {
        self.rep
            .find(':')
            .map(|i| BorrowedName::new(&self.rep[0..i]).expect("collection guaranteed to be valid"))
    }
}

impl TryFrom<&str> for ChildName {
    type Error = MonikerError;

    #[inline]
    fn try_from(rep: &str) -> Result<Self, Self::Error> {
        Self::parse(rep)
    }
}

impl FromStr for ChildName {
    type Err = MonikerError;

    #[inline]
    fn from_str(rep: &str) -> Result<Self, Self::Err> {
        Self::parse(rep)
    }
}

impl From<cm_rust::ChildRef> for ChildName {
    fn from(child_ref: cm_rust::ChildRef) -> Self {
        Self::new(child_ref.name, child_ref.collection)
    }
}

impl From<&BorrowedChildName> for ChildName {
    fn from(o: &BorrowedChildName) -> Self {
        Self { rep: o.rep.into() }
    }
}

impl From<ChildName> for cm_rust::ChildRef {
    fn from(child_name: ChildName) -> Self {
        Self { name: child_name.name().into(), collection: child_name.collection().map(Into::into) }
    }
}

impl AsRef<str> for ChildName {
    #[inline]
    fn as_ref(&self) -> &str {
        self.rep.as_str()
    }
}

impl AsRef<BorrowedChildName> for ChildName {
    #[inline]
    fn as_ref(&self) -> &BorrowedChildName {
        BorrowedChildName::new_unchecked(self)
    }
}

impl Borrow<BorrowedChildName> for ChildName {
    #[inline]
    fn borrow(&self) -> &BorrowedChildName {
        &BorrowedChildName::new_unchecked(self)
    }
}

impl Borrow<str> for ChildName {
    #[inline]
    fn borrow(&self) -> &str {
        &self.rep
    }
}

impl std::ops::Deref for ChildName {
    type Target = BorrowedChildName;

    #[inline]
    fn deref(&self) -> &BorrowedChildName {
        BorrowedChildName::new_unchecked(self.rep.as_str())
    }
}

impl PartialEq<&str> for ChildName {
    #[inline]
    fn eq(&self, o: &&str) -> bool {
        &*self.rep == *o
    }
}

impl PartialEq<String> for ChildName {
    #[inline]
    fn eq(&self, o: &String) -> bool {
        &*self.rep == *o
    }
}

impl PartialEq<BorrowedChildName> for ChildName {
    #[inline]
    fn eq(&self, o: &BorrowedChildName) -> bool {
        &self.rep == &o.rep
    }
}

impl Ord for ChildName {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (self.collection(), self.name()).cmp(&(other.collection(), other.name()))
    }
}

impl PartialOrd for ChildName {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for ChildName {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rep.as_str().hash(state)
    }
}

impl fmt::Display for ChildName {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rep)
    }
}

impl fmt::Debug for ChildName {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Like [`ChildName`], except it holds a string slice rather than an allocated string. For
/// example, the [`Moniker`] API uses this to return path segments without making an allocation.
#[derive(Eq, PartialEq)]
#[repr(transparent)]
pub struct BorrowedChildName {
    rep: str,
}

impl BorrowedChildName {
    /// Parses a `ChildName` from a string.
    ///
    /// Input strings should be of the format `[collection:]name`, e.g. `foo` or `biz:foo`.
    pub fn parse<S: AsRef<str> + ?Sized>(s: &S) -> Result<&Self, MonikerError> {
        validate_child_name(s.as_ref())?;
        Ok(Self::new_unchecked(s))
    }

    /// Private variant of [`BorrowedChildName::parse`] that does not perform correctness checks.
    /// For efficiency when the caller is sure `s` is a valid [`ChildName`].
    pub(crate) fn new_unchecked<S: AsRef<str> + ?Sized>(s: &S) -> &Self {
        unsafe { &*(s.as_ref() as *const str as *const Self) }
    }

    pub fn name(&self) -> &BorrowedLongName {
        match self.rep.find(':') {
            Some(i) => {
                BorrowedLongName::new(&self.rep[i + 1..]).expect("name guaranteed to be valid")
            }
            None => BorrowedLongName::new(&self.rep).expect("name guaranteed to be valid"),
        }
    }

    pub fn collection(&self) -> Option<&BorrowedName> {
        self.rep
            .find(':')
            .map(|i| BorrowedName::new(&self.rep[0..i]).expect("collection guaranteed to be valid"))
    }
}

impl From<&BorrowedChildName> for cm_rust::ChildRef {
    fn from(child_name: &BorrowedChildName) -> Self {
        Self { name: child_name.name().into(), collection: child_name.collection().map(Into::into) }
    }
}

impl AsRef<str> for BorrowedChildName {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.rep
    }
}

impl Borrow<str> for BorrowedChildName {
    #[inline]
    fn borrow(&self) -> &str {
        &self.rep
    }
}

impl Borrow<str> for &BorrowedChildName {
    #[inline]
    fn borrow(&self) -> &str {
        &self.rep
    }
}

impl PartialEq<&str> for BorrowedChildName {
    #[inline]
    fn eq(&self, o: &&str) -> bool {
        &self.rep == *o
    }
}

impl PartialEq<String> for BorrowedChildName {
    #[inline]
    fn eq(&self, o: &String) -> bool {
        &self.rep == &*o
    }
}

impl PartialEq<ChildName> for BorrowedChildName {
    #[inline]
    fn eq(&self, o: &ChildName) -> bool {
        self.rep == *o.rep
    }
}

impl Ord for BorrowedChildName {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (self.collection(), self.name()).cmp(&(other.collection(), other.name()))
    }
}

impl PartialOrd for BorrowedChildName {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for BorrowedChildName {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rep.hash(state)
    }
}

impl fmt::Display for BorrowedChildName {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.rep)
    }
}

impl fmt::Debug for BorrowedChildName {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

fn validate_child_name(name: &str) -> Result<(), MonikerError> {
    match name.find(':') {
        Some(i) => {
            let collection = &name[0..i];
            let name = &name[i + 1..];
            let _ = BorrowedName::new(collection)?;
            let _ = BorrowedLongName::new(name)?;
        }
        None => {
            let _ = BorrowedLongName::new(name)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_types::{MAX_LONG_NAME_LENGTH, MAX_NAME_LENGTH};
    use std::collections::HashSet;
    use std::iter::repeat;

    #[test]
    fn child_monikers() {
        let m = ChildName::try_new("test", None).unwrap();
        assert_eq!("test", m.name().as_str());
        assert_eq!(None, m.collection());
        assert_eq!("test", format!("{}", m));
        assert_eq!(m, ChildName::try_from("test").unwrap());

        let m = ChildName::try_new("test", Some("coll")).unwrap();
        assert_eq!("test", m.name().as_str());
        assert_eq!(Some(BorrowedName::new("coll").unwrap()), m.collection());
        assert_eq!("coll:test", format!("{}", m));
        assert_eq!(m, ChildName::parse("coll:test").unwrap());

        let max_coll_length_part = "f".repeat(MAX_NAME_LENGTH);
        let max_name_length_part = "f".repeat(MAX_LONG_NAME_LENGTH);
        let max_moniker_length = format!("{}:{}", max_coll_length_part, max_name_length_part);
        let m = ChildName::parse(max_moniker_length).expect("valid moniker");
        assert_eq!(&max_name_length_part, m.name().as_str());
        assert_eq!(Some(BorrowedName::new(&max_coll_length_part).unwrap()), m.collection());

        assert!(ChildName::parse("").is_err(), "cannot be empty");
        assert!(ChildName::parse(":").is_err(), "cannot be empty with colon");
        assert!(ChildName::parse("f:").is_err(), "second part cannot be empty with colon");
        assert!(ChildName::parse(":f").is_err(), "first part cannot be empty with colon");
        assert!(ChildName::parse("f:f:f").is_err(), "multiple colons not allowed");
        assert!(ChildName::parse("@").is_err(), "invalid character in name");
        assert!(ChildName::parse("@:f").is_err(), "invalid character in collection");
        assert!(ChildName::parse("f:@").is_err(), "invalid character in name with collection");
        assert!(
            ChildName::parse(&format!("f:{}", "x".repeat(MAX_LONG_NAME_LENGTH + 1))).is_err(),
            "name too long"
        );
        assert!(
            ChildName::parse(&format!("{}:x", "f".repeat(MAX_NAME_LENGTH + 1))).is_err(),
            "collection too long"
        );
    }

    #[test]
    fn child_moniker_compare() {
        let a = ChildName::try_new("a", None).unwrap();
        let aa = ChildName::try_new("a", Some("a")).unwrap();
        let ab = ChildName::try_new("a", Some("b")).unwrap();
        let ba = ChildName::try_new("b", Some("a")).unwrap();
        let bb = ChildName::try_new("b", Some("b")).unwrap();
        let aa_same = ChildName::try_new("a", Some("a")).unwrap();

        assert_eq!(Ordering::Less, a.cmp(&aa));
        assert_eq!(Ordering::Greater, aa.cmp(&a));
        assert_eq!(Ordering::Less, a.cmp(&ab));
        assert_eq!(Ordering::Greater, ab.cmp(&a));
        assert_eq!(Ordering::Less, a.cmp(&ba));
        assert_eq!(Ordering::Greater, ba.cmp(&a));
        assert_eq!(Ordering::Less, a.cmp(&bb));
        assert_eq!(Ordering::Greater, bb.cmp(&a));

        assert_eq!(Ordering::Less, aa.cmp(&ab));
        assert_eq!(Ordering::Greater, ab.cmp(&aa));
        assert_eq!(Ordering::Less, aa.cmp(&ba));
        assert_eq!(Ordering::Greater, ba.cmp(&aa));
        assert_eq!(Ordering::Less, aa.cmp(&bb));
        assert_eq!(Ordering::Greater, bb.cmp(&aa));
        assert_eq!(Ordering::Equal, aa.cmp(&aa_same));
        assert_eq!(Ordering::Equal, aa_same.cmp(&aa));

        assert_eq!(Ordering::Greater, ab.cmp(&ba));
        assert_eq!(Ordering::Less, ba.cmp(&ab));
        assert_eq!(Ordering::Less, ab.cmp(&bb));
        assert_eq!(Ordering::Greater, bb.cmp(&ab));

        assert_eq!(Ordering::Less, ba.cmp(&bb));
        assert_eq!(Ordering::Greater, bb.cmp(&ba));
    }

    #[test]
    fn hash() {
        {
            let n1 = ChildName::new("a".parse().unwrap(), None);
            let s_b = repeat("b").take(1024).collect::<String>();
            let n2 = ChildName::new(s_b.parse().unwrap(), None);
            let b1 = BorrowedChildName::parse("a").unwrap();
            let b2 = BorrowedChildName::parse(&s_b).unwrap();

            let mut set = HashSet::new();
            set.insert(n1.clone());
            assert!(set.contains(&n1));
            assert!(set.contains(b1));
            assert!(!set.contains(&n2));
            assert!(!set.contains(b2));
            set.insert(n2.clone());
            assert!(set.contains(&n1));
            assert!(set.contains(b1));
            assert!(set.contains(&n2));
            assert!(set.contains(b2));
        }
        {
            let n1 = ChildName::new("a".parse().unwrap(), Some("c".parse().unwrap()));
            let s_b = repeat("b").take(1024).collect::<String>();
            let s_c = repeat("c").take(255).collect::<String>();
            let n2 = ChildName::new(s_b.parse().unwrap(), Some(s_c.parse().unwrap()));
            let b1 = BorrowedChildName::parse("c:a").unwrap();
            let s_c_b = &format!("{s_c}:{s_b}");
            let b2 = BorrowedChildName::parse(&s_c_b).unwrap();

            let mut set = HashSet::new();
            set.insert(n1.clone());
            assert!(set.contains(&n1));
            assert!(set.contains(b1));
            assert!(!set.contains(&n2));
            assert!(!set.contains(b2));
            set.insert(n2.clone());
            assert!(set.contains(&n1));
            assert!(set.contains(b1));
            assert!(set.contains(&n2));
            assert!(set.contains(b2));
        }
    }
}
