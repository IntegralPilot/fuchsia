// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{AnyRef, ContextSpanned};
use std::fmt::{self, Display, Formatter};
use std::{slice, vec};

/// Represents either a single value, or multiple values of T.
/// Useful for differentiating between an array of length 1 and a single value.
#[derive(Debug, Clone, Ord, PartialOrd, Eq)]
pub enum OneOrMany<T> {
    /// A single instance of T.
    One(T),
    /// One or more instances of T.
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// Returns `true` if this `OneOrMany<T>` is a `Many` value.
    pub fn is_many(&self) -> bool {
        match self {
            Self::One(_) => false,
            Self::Many(_) => true,
        }
    }

    /// Returns an [`Iter`] over the values of `OneOrMany<T>`.
    pub fn iter(&self) -> Iter<'_, T> {
        match self {
            Self::One(item) => Iter { inner_one: Some(item), inner_many: None },
            Self::Many(items) => Iter { inner_one: None, inner_many: Some(items.iter()) },
        }
    }

    /// Returns the number of values in this `OneOrMany<T>`.
    pub fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(v) => v.len(),
        }
    }

    /// Returns a `OneOrMany<&T>` that references this `OneOrMany<T>`.
    pub fn as_ref<S>(&self) -> OneOrMany<&S>
    where
        T: AsRef<S>,
        S: ?Sized,
    {
        match self {
            Self::One(o) => OneOrMany::<&S>::One(o.as_ref()),
            Self::Many(v) => OneOrMany::<&S>::Many(v.iter().map(|o| o.as_ref()).collect()),
        }
    }
}

impl<T> OneOrMany<T>
where
    T: Ord + Clone,
{
    /// Canonicalizes self by:
    /// * Transforming to ::One() if is_many() but len() == 1
    /// * Sorting items if is_many()
    pub fn canonicalize(&mut self) {
        let mut replace_with = None;
        match self {
            OneOrMany::One(_) => {}
            OneOrMany::Many(many) => {
                if many.len() == 1 {
                    replace_with = Some(many.first().unwrap().clone());
                } else {
                    many.sort();
                }
            }
        }
        if let Some(t) = replace_with {
            *self = OneOrMany::One(t);
        }
    }
}

impl<T> OneOrMany<T>
where
    T: Ord + Clone,
{
    /// Canonicalizes self by:
    /// * Transforming to ::One() if is_many() but len() == 1
    /// * Sorting items if is_many()
    pub fn canonicalize_context(&mut self) {
        let mut replace_with = None;
        match self {
            OneOrMany::One(_) => {}
            OneOrMany::Many(many) => {
                if many.len() == 1 {
                    replace_with = Some(many.first().unwrap().clone());
                } else {
                    many.sort();
                }
            }
        }
        if let Some(t) = replace_with {
            *self = OneOrMany::One(t);
        }
    }
}

impl<T> OneOrMany<T>
where
    T: PartialEq,
{
    /// Returns true if this `OneOrMany<T>` contains the given element.
    pub fn contains(&self, e: &T) -> bool {
        match self {
            Self::One(item) => item == e,
            Self::Many(items) => items.contains(e),
        }
    }
}

impl<T> PartialEq for OneOrMany<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.into_iter())
    }
}

impl<'a, T> IntoIterator for &'a OneOrMany<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        match self {
            OneOrMany::One(item) => IntoIter { inner_one: Some(item), inner_many: None },
            OneOrMany::Many(items) => {
                IntoIter { inner_one: None, inner_many: Some(items.into_iter()) }
            }
        }
    }
}

impl<T> FromIterator<T> for OneOrMany<T> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T> + Sized,
    {
        let mut iter = iter.into_iter();
        if let Some(first) = iter.next() {
            let rest: Vec<_> = iter.collect();
            if rest.is_empty() {
                Self::One(first)
            } else {
                let mut out = vec![first];
                out.extend(rest);
                Self::Many(out)
            }
        } else {
            Self::Many(vec![])
        }
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(item: T) -> Self {
        Self::One(item)
    }
}

impl<'a, T: Display> Display for OneOrMany<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            OneOrMany::One(item) => Display::fmt(item, f),
            OneOrMany::Many(items) => {
                let mut iter = items.iter();
                if let Some(first_item) = iter.next() {
                    Display::fmt(first_item, f)?;
                }
                for item in iter {
                    f.write_str(", ")?;
                    Display::fmt(item, f)?;
                }
                Ok(())
            }
        }
    }
}

/// Immutable iterator over a `OneOrMany`.
/// This `struct` is created by [`OneOrMany::iter`].
///
/// [`OneOrMany::iter`]: struct.OneOrMany.html#method.iter
pub struct Iter<'a, T> {
    inner_one: Option<&'a T>,
    inner_many: Option<slice::Iter<'a, T>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.inner_one.take() {
            Some(item)
        } else if let Some(iter) = &mut self.inner_many {
            iter.next()
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if let Some(_) = self.inner_one {
            (1, Some(1))
        } else if let Some(iter) = &self.inner_many {
            iter.size_hint()
        } else {
            (0, Some(0))
        }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}

/// An iterator that moves out of a `OneOrMany`.
/// This `struct` is created by the `into_iter` method on [`OneOrMany`] (provided by the [`IntoIterator`] trait).
///
/// [`OneOrMany`]: struct.OneOrMany.html
/// [`IntoIterator`]: https://doc.rust-lang.org/std/iter/trait.IntoIterator.html
pub struct IntoIter<T> {
    inner_one: Option<T>,
    inner_many: Option<vec::IntoIter<T>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.inner_one.take() {
            Some(item)
        } else if let Some(iter) = &mut self.inner_many {
            iter.next()
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if let Some(_) = self.inner_one {
            (1, Some(1))
        } else if let Some(iter) = &self.inner_many {
            iter.size_hint()
        } else {
            (0, Some(0))
        }
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}

pub(crate) fn always_one_context<T>(
    o: Option<ContextSpanned<OneOrMany<T>>>,
) -> Option<ContextSpanned<T>> {
    o.map(|spanned| {
        let single_val = match spanned.value {
            OneOrMany::One(val) => val,
            OneOrMany::Many(_) => panic!("many is impossible"),
        };

        ContextSpanned { value: single_val, origin: spanned.origin }
    })
}

pub(crate) fn one_or_many_from_context<'a, T>(
    from: &'a ContextSpanned<OneOrMany<T>>,
) -> ContextSpanned<OneOrMany<AnyRef<'a>>>
where
    AnyRef<'a>: From<&'a T>,
    T: 'a,
{
    let origin = from.origin.clone();

    let converted_value = match &from.value {
        OneOrMany::One(r) => OneOrMany::One(r.into()),
        OneOrMany::Many(v) => {
            let converted_vec = v.iter().map(|r| r.into()).collect();
            OneOrMany::Many(converted_vec)
        }
    };

    ContextSpanned { value: converted_value, origin }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_iter_one() {
        let v = OneOrMany::One(34);
        let mut iter = v.iter();
        assert_matches!(iter.next(), Some(&34));
        assert_matches!(iter.next(), None);
    }

    #[test]
    fn test_iter_many() {
        let v = OneOrMany::Many(vec![1, 2, 3]);
        let mut iter = v.iter();
        assert_matches!(iter.next(), Some(&1));
        assert_matches!(iter.next(), Some(&2));
        assert_matches!(iter.next(), Some(&3));
        assert_matches!(iter.next(), None);
    }

    #[test]
    fn test_is_many() {
        let v = OneOrMany::One(34);
        assert_eq!(v.is_many(), false);

        let v = OneOrMany::Many(vec![1, 2, 3]);
        assert_eq!(v.is_many(), true);
    }

    #[test]
    fn test_from_iter() {
        let o: OneOrMany<i64> = [34].into_iter().collect();
        assert_eq!(o, OneOrMany::One(34));

        let o: OneOrMany<i64> = [1, 2, 3].into_iter().collect();
        assert_eq!(o, OneOrMany::Many(vec![1, 2, 3]));

        let o: OneOrMany<i64> = [].into_iter().collect();
        assert_eq!(o, OneOrMany::Many(vec![]));
    }

    #[test]
    fn test_display() {
        let val = 34;
        let v = OneOrMany::One(val);
        assert_eq!(v.to_string(), "34");

        let val = vec![1, 2, 3];
        let v = OneOrMany::Many(val);
        assert_eq!(v.to_string(), "1, 2, 3");
    }
}
