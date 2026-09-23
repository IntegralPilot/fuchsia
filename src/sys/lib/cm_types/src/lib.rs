// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A crate containing common Component Manager types used in Component Manifests
//! (`.cml` files and binary `.cm` files). These types come with `serde` serialization
//! and deserialization implementations that perform the required validation.

use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use flyweights::FlyStr;
use serde::{Deserialize, Serialize, de, ser};
use std::borrow::Borrow;
use std::ffi::CString;
use std::fmt::{self, Display};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::LazyLock;
use std::{cmp, iter};
use thiserror::Error;

/// A default base URL from which to parse relative component URL
/// components.
static DEFAULT_BASE_URL: LazyLock<url::Url> =
    LazyLock::new(|| url::Url::parse("relative:///").unwrap());

/// Generate `impl From` for two trivial enums with identical values, allowing
/// converting to/from each other.
/// This is useful if you have a FIDL-generated enum and a hand-rolled
/// one that contain the same values.
/// # Arguments
///
/// * `$a`, `$b` - The enums to generate `impl From` for. Order doesn't matter because
///     implementation will be generated for both. Enums should be trivial.
/// * `id` - Exhaustive list of all enum values.
/// # Examples
///
/// ```
/// mod a {
///     #[derive(Debug, PartialEq, Eq)]
///     pub enum Streetlight {
///         Green,
///         Yellow,
///         Red,
///     }
/// }
///
/// mod b {
///     #[derive(Debug, PartialEq, Eq)]
///     pub enum Streetlight {
///         Green,
///         Yellow,
///         Red,
///     }
/// }
///
/// symmetrical_enums!(a::Streetlight, b::Streetlight, Green, Yellow, Red);
///
/// assert_eq!(a::Streetlight::Green, b::Streetlight::Green.into());
/// assert_eq!(b::Streetlight::Green, a::Streetlight::Green.into());
/// ```
#[macro_export]
macro_rules! symmetrical_enums {
    ($a:ty , $b:ty, $($id: ident),*) => {
        impl From<$a> for $b {
            fn from(input: $a) -> Self {
                match input {
                    $( <$a>::$id => <$b>::$id, )*
                }
            }
        }

        impl From<$b> for $a {
            fn from(input: $b) -> Self {
                match input {
                    $( <$b>::$id => <$a>::$id, )*
                }
            }
        }
    };
}

/// The error representing a failure to parse a type from string.
#[derive(Serialize, Clone, Deserialize, Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    /// The string did not match a valid value.
    #[error("invalid value")]
    InvalidValue,
    /// The string did not match a valid absolute or relative component URL
    #[error("invalid URL: {details}")]
    InvalidComponentUrl { details: String },
    /// The string was empty.
    #[error("empty")]
    Empty,
    /// The string was too long.
    #[error("too long")]
    TooLong,
    /// A required leading slash was missing.
    #[error("no leading slash")]
    NoLeadingSlash,
    /// The path segment is invalid.
    #[error("invalid path segment")]
    InvalidSegment,
}

pub const MAX_NAME_LENGTH: usize = name::MAX_NAME_LENGTH;
pub const MAX_LONG_NAME_LENGTH: usize = 1024;
pub const MAX_PATH_LENGTH: usize = fio::MAX_PATH_LENGTH as usize;
pub const MAX_URL_LENGTH: usize = 4096;

/// This asks for the maximum possible rights that the parent connection will allow; this will
/// include the writable and executable rights if the parent connection has them, but won't fail if
/// it doesn't.
pub const FLAGS_MAX_POSSIBLE_RIGHTS: fio::Flags = fio::PERM_READABLE
    .union(fio::Flags::PERM_INHERIT_WRITE)
    .union(fio::Flags::PERM_INHERIT_EXECUTE);

/// A name that can refer to a component, collection, or other entity in the
/// Component Manifest. Its length is bounded to `MAX_NAME_LENGTH`.
pub type Name = BoundedName<MAX_NAME_LENGTH>;
/// A `Name` with a higher string capacity of `MAX_LONG_NAME_LENGTH`.
pub type LongName = BoundedName<MAX_LONG_NAME_LENGTH>;

/// A `BoundedName` is a `Name` that can have a max length of `N` bytes.
#[derive(Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoundedName<const N: usize>(FlyStr);

impl Name {
    #[inline]
    pub fn to_long(self) -> LongName {
        BoundedName(self.0)
    }
}

impl<const N: usize> BoundedName<N> {
    /// Creates a `BoundedName` from a `&str` slice, returning an `Err` if the string
    /// fails validation. The string must be non-empty, no more than `N`
    /// characters in length, and consist of one or more of the
    /// following characters: `A-Z`, `a-z`, `0-9`, `_`, `.`, `-`. It may not start
    /// with `.` or `-`.
    pub fn new(s: impl AsRef<str>) -> Result<Self, ParseError> {
        let s = s.as_ref();
        validate_name::<N>(s)?;
        Ok(Self(FlyStr::new(s)))
    }

    /// Private variant of [`BoundedName::new`] that does not perform correctness checks.
    /// For efficiency when the caller is sure `s` is a valid [`BoundedName`].
    fn new_unchecked<S: AsRef<str> + ?Sized>(s: &S) -> Self {
        Self(FlyStr::new(s.as_ref()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<const N: usize> AsRef<str> for BoundedName<N> {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize> AsRef<BoundedBorrowedName<N>> for BoundedName<N> {
    #[inline]
    fn as_ref(&self) -> &BoundedBorrowedName<N> {
        BoundedBorrowedName::<N>::new_unchecked(self)
    }
}

impl<const N: usize> AsRef<BoundedName<N>> for BoundedName<N> {
    #[inline]
    fn as_ref(&self) -> &BoundedName<N> {
        self
    }
}

impl<const N: usize> Borrow<str> for BoundedName<N> {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> Deref for BoundedName<N> {
    type Target = BoundedBorrowedName<N>;

    #[inline]
    fn deref(&self) -> &BoundedBorrowedName<N> {
        BoundedBorrowedName::new_unchecked(self.0.as_str())
    }
}

impl<const N: usize> Borrow<BoundedBorrowedName<N>> for BoundedName<N> {
    #[inline]
    fn borrow(&self) -> &BoundedBorrowedName<N> {
        self.deref()
    }
}

impl<const N: usize> From<BoundedName<N>> for FlyStr {
    #[inline]
    fn from(o: BoundedName<N>) -> Self {
        o.0
    }
}

impl<'a, const N: usize> From<&'a BoundedName<N>> for &'a FlyStr {
    #[inline]
    fn from(o: &'a BoundedName<N>) -> Self {
        &o.0
    }
}

impl<const N: usize> PartialEq<&str> for BoundedName<N> {
    #[inline]
    fn eq(&self, o: &&str) -> bool {
        &*self.0 == *o
    }
}

impl<const N: usize> PartialEq<String> for BoundedName<N> {
    #[inline]
    fn eq(&self, o: &String) -> bool {
        &*self.0 == *o
    }
}

impl<const N: usize> PartialEq<BoundedBorrowedName<N>> for BoundedName<N> {
    #[inline]
    fn eq(&self, o: &BoundedBorrowedName<N>) -> bool {
        &self.0 == &o.0
    }
}

impl<const N: usize> Hash for BoundedName<N> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state)
    }
}

impl<const N: usize> fmt::Display for BoundedName<N> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <FlyStr as fmt::Display>::fmt(&self.0, f)
    }
}

impl<const N: usize> FromStr for BoundedName<N> {
    type Err = ParseError;

    #[inline]
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::new(name)
    }
}

impl<const N: usize> From<&BoundedBorrowedName<N>> for BoundedName<N> {
    #[inline]
    fn from(o: &BoundedBorrowedName<N>) -> Self {
        Self(o.0.into())
    }
}

impl<const N: usize> From<BoundedName<N>> for String {
    #[inline]
    fn from(name: BoundedName<N>) -> String {
        name.0.into()
    }
}

impl From<Name> for LongName {
    #[inline]
    fn from(name: Name) -> Self {
        Self(name.0)
    }
}

/// Unowned variant of [`Name`]. [`Name`] for more details.
pub type BorrowedName = BoundedBorrowedName<MAX_NAME_LENGTH>;
/// Unowned variant of [`LongName`]. [`LongName`] for more details.
pub type BorrowedLongName = BoundedBorrowedName<MAX_LONG_NAME_LENGTH>;

/// Like [`BoundedName`], except it holds a string slice rather than an allocated string. For
/// example, the [`Path`] API uses this to return path segments without making an allocation.
#[derive(Serialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BoundedBorrowedName<const N: usize>(str);

impl BorrowedName {
    #[inline]
    pub fn to_long(&self) -> &BorrowedLongName {
        // SAFETY: `BorrowedName` and `BorrowedLongName` share the same representation.
        // Furthermore, every `BorrowedName` is a valid `BorrowedLongName`. Therefore, this
        // typecast is safe.
        unsafe { &*(self as *const BorrowedName as *const BorrowedLongName) }
    }
}

impl<const N: usize> BoundedBorrowedName<N> {
    /// Creates a `BoundedBorrowedName` from a `&str` slice, which obeys the same
    /// rules as `BoundedName`.
    pub fn new<S: AsRef<str> + ?Sized>(s: &S) -> Result<&Self, ParseError> {
        validate_name::<N>(s.as_ref())?;
        Ok(Self::new_unchecked(s))
    }

    /// Private variant of [`BoundedBorrowedName::new`] that does not perform correctness checks.
    /// For efficiency when the caller is sure `s` is a valid [`BoundedName`].
    fn new_unchecked<S: AsRef<str> + ?Sized>(s: &S) -> &Self {
        // SAFETY: `&str` is the transparent representation of `BorrowedName`. This function is
        // private, and it is only called from places that are certain the `&str` matches the
        // `BorrowedName` requirements. Therefore, this typecast is safe.
        unsafe { &*(s.as_ref() as *const str as *const Self) }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

fn validate_name<const N: usize>(name: &str) -> Result<(), ParseError> {
    if name.is_empty() {
        return Err(ParseError::Empty);
    }
    if name.len() > N {
        return Err(ParseError::TooLong);
    }
    let mut char_iter = name.chars();
    let first_char = char_iter.next().unwrap();
    if !first_char.is_ascii_alphanumeric() && first_char != '_' {
        return Err(ParseError::InvalidValue);
    }
    let valid_fn = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    if !char_iter.all(valid_fn) {
        return Err(ParseError::InvalidValue);
    }
    Ok(())
}

impl<const N: usize> ToOwned for BoundedBorrowedName<N> {
    type Owned = BoundedName<N>;

    fn to_owned(&self) -> Self::Owned {
        BoundedName::<N>::new_unchecked(&self.0)
    }
}

impl<const N: usize> AsRef<str> for BoundedBorrowedName<N> {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> Borrow<str> for BoundedBorrowedName<N> {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> Borrow<str> for &BoundedBorrowedName<N> {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<'a, const N: usize> From<&'a BoundedBorrowedName<N>> for &'a str {
    #[inline]
    fn from(o: &'a BoundedBorrowedName<N>) -> Self {
        &o.0
    }
}

impl<const N: usize> PartialEq<&str> for BoundedBorrowedName<N> {
    #[inline]
    fn eq(&self, o: &&str) -> bool {
        &self.0 == *o
    }
}

impl<const N: usize> PartialEq<String> for BoundedBorrowedName<N> {
    #[inline]
    fn eq(&self, o: &String) -> bool {
        &self.0 == &*o
    }
}

impl<const N: usize> PartialEq<BoundedName<N>> for BoundedBorrowedName<N> {
    #[inline]
    fn eq(&self, o: &BoundedName<N>) -> bool {
        self.0 == *o.0
    }
}

impl<const N: usize> Hash for BoundedBorrowedName<N> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

impl<const N: usize> fmt::Display for BoundedBorrowedName<N> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <str as fmt::Display>::fmt(&self.0, f)
    }
}

impl<'a> From<&'a BorrowedName> for &'a BorrowedLongName {
    #[inline]
    fn from(name: &'a BorrowedName) -> Self {
        name.to_long()
    }
}

impl<'de, const N: usize> de::Deserialize<'de> for BoundedName<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor<const N: usize>;

        impl<'de, const N: usize> de::Visitor<'de> for Visitor<N> {
            type Value = BoundedName<{ N }>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&format!(
                    "a non-empty string no more than {} characters in length, \
                    consisting of [A-Za-z0-9_.-] and starting with [A-Za-z0-9_]",
                    N
                ))
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue => E::invalid_value(
                        de::Unexpected::Str(s),
                        &"a name that consists of [A-Za-z0-9_.-] and starts with [A-Za-z0-9_]",
                    ),
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &format!("a non-empty name no more than {} characters in length", N)
                            .as_str(),
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

impl IterablePath for Name {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        iter::once(self as &BorrowedName)
    }
}

impl IterablePath for &Name {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        iter::once(*self as &BorrowedName)
    }
}

/// [NamespacePath] is the same as [Path] but accepts `"/"` (which is also a valid namespace
/// path).
///
/// Note that while `"/"` is accepted, `"."` (which is synonymous in fuchsia.io) is rejected.
#[derive(Eq, Ord, PartialOrd, PartialEq, Hash, Clone)]
pub struct NamespacePath(RelativePath);

impl NamespacePath {
    /// Like [Path::new] but `path` may be `/`.
    pub fn new(path: impl AsRef<str>) -> Result<Self, ParseError> {
        let path = path.as_ref();
        if path.is_empty() {
            return Err(ParseError::Empty);
        }
        if path == "." {
            return Err(ParseError::InvalidValue);
        }
        if !path.starts_with('/') {
            return Err(ParseError::NoLeadingSlash);
        }
        if path.len() > MAX_PATH_LENGTH {
            return Err(ParseError::TooLong);
        }
        if path == "/" {
            Ok(Self(RelativePath::dot()))
        } else {
            let path: RelativePath = path[1..].parse()?;
            if path.is_dot() {
                // "/." is not a valid NamespacePath
                return Err(ParseError::InvalidSegment);
            }
            Ok(Self(path))
        }
    }

    /// Returns the [NamespacePath] for `"/"`.
    pub fn root() -> Self {
        Self(RelativePath::dot())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_dot()
    }

    /// Splits the path according to `"/"`.
    pub fn split(&self) -> Vec<&BorrowedName> {
        self.0.split()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.to_string())
    }

    /// Returns a path that represents the parent directory of this one, or None if this is a
    /// root dir.
    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(|p| Self(p))
    }

    /// Returns whether `prefix` is a prefix of `self` in terms of path segments.
    ///
    /// For example:
    /// ```
    /// Path("/pkg/data").has_prefix("/pkg") == true
    /// Path("/pkg_data").has_prefix("/pkg") == false
    /// ```
    pub fn has_prefix(&self, prefix: &Self) -> bool {
        let my_segments = self.split();
        let prefix_segments = prefix.split();
        if prefix_segments.len() > my_segments.len() {
            return false;
        }
        prefix_segments.into_iter().zip(my_segments.into_iter()).all(|(a, b)| a == b)
    }

    /// The last path segment, or None.
    pub fn basename(&self) -> Option<&BorrowedName> {
        self.0.basename()
    }

    pub fn pop_front(&mut self) -> Option<Name> {
        self.0.pop_front()
    }

    pub fn into_relative(self) -> RelativePath {
        self.0
    }
}

impl IterablePath for NamespacePath {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        self.0.iter_segments()
    }
}

impl serde::ser::Serialize for NamespacePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl TryFrom<CString> for NamespacePath {
    type Error = ParseError;

    fn try_from(path: CString) -> Result<Self, ParseError> {
        Self::new(path.into_string().map_err(|_| ParseError::InvalidValue)?)
    }
}

impl From<NamespacePath> for CString {
    fn from(path: NamespacePath) -> Self {
        // SAFETY: in `Path::new` we already verified that there are no
        // embedded NULs.
        unsafe { CString::from_vec_unchecked(path.to_string().as_bytes().to_owned()) }
    }
}

impl From<NamespacePath> for String {
    fn from(path: NamespacePath) -> Self {
        path.to_string()
    }
}

impl FromStr for NamespacePath {
    type Err = ParseError;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::new(path)
    }
}

impl fmt::Debug for NamespacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for NamespacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.0.is_dot() { write!(f, "/{}", self.0) } else { write!(f, "/") }
    }
}

/// A path type used throughout Component Framework, along with its variants [NamespacePath] and
/// [RelativePath]. Examples of use:
///
/// - [NamespacePath]: Namespace paths
/// - [Path]: Outgoing paths and namespace paths that can't be "/"
/// - [RelativePath]: Dictionary paths
///
/// [Path] obeys the following constraints:
///
/// - Is a [fuchsia.io.Path](https://fuchsia.dev/reference/fidl/fuchsia.io#Directory.Open).
/// - Begins with `/`.
/// - Is not `.`.
/// - Contains at least one path segment (just `/` is disallowed).
/// - Each path segment is a [Name]. (This is strictly more constrained than a fuchsia.io
///   path segment.)
#[derive(Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Path(RelativePath);

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.0)
    }
}

impl ser::Serialize for Path {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl Path {
    /// Creates a [`Path`] from a [`String`], returning an `Err` if the string fails validation.
    /// The string must be non-empty, no more than [`MAX_PATH_LENGTH`] bytes in length, start with
    /// a leading `/`, not be exactly `/` or `.`, and each segment must be a valid [`Name`]. As a
    /// result, [`Path`]s are always valid [`NamespacePath`]s.
    pub fn new(path: impl AsRef<str>) -> Result<Self, ParseError> {
        let path = path.as_ref();
        if path.is_empty() {
            return Err(ParseError::Empty);
        }
        if path == "/" || path == "." {
            return Err(ParseError::InvalidValue);
        }
        if !path.starts_with('/') {
            return Err(ParseError::NoLeadingSlash);
        }
        if path.len() > MAX_PATH_LENGTH {
            return Err(ParseError::TooLong);
        }
        let path: RelativePath = path[1..].parse()?;
        if path.is_dot() {
            // "/." is not a valid Path
            return Err(ParseError::InvalidSegment);
        }
        Ok(Self(path))
    }

    /// Splits the path according to "/".
    pub fn split(&self) -> Vec<&BorrowedName> {
        self.0.split()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.to_string())
    }

    /// Returns a path that represents the parent directory of this one. Returns [NamespacePath]
    /// instead of [Path] because the parent could be the root dir.
    pub fn parent(&self) -> NamespacePath {
        let p = self.0.parent().expect("can't be root");
        NamespacePath(p)
    }

    pub fn basename(&self) -> &BorrowedName {
        self.0.basename().expect("can't be root")
    }

    // Attaches the path `other` to the end of `self`. Returns `true` on success, and false
    // if the resulting path's length would exceed `MAX_PATH_LENGTH`.
    #[must_use]
    pub fn extend(&mut self, other: RelativePath) -> bool {
        let rep: FlyStr = if !other.is_dot() {
            format!("{}/{}", self.0.rep, other.rep).into()
        } else {
            // Nothing to do.
            return true;
        };
        // Account for leading /
        if rep.len() > MAX_PATH_LENGTH - 1 {
            return false;
        }
        self.0.rep = rep;
        true
    }

    // Attaches `segment` to the end of `self`. Returns `true` on success, and false
    // if the resulting path's length would exceed `MAX_PATH_LENGTH`.
    #[must_use]
    pub fn push(&mut self, segment: Name) -> bool {
        let rep: FlyStr = format!("{}/{}", self.0.rep, segment).into();
        // Account for leading /
        if rep.len() > MAX_PATH_LENGTH - 1 {
            return false;
        }
        self.0.rep = rep;
        true
    }
}

impl IterablePath for Path {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        Box::new(self.0.iter_segments())
    }
}

impl From<Path> for NamespacePath {
    fn from(value: Path) -> Self {
        Self(value.0)
    }
}

impl FromStr for Path {
    type Err = ParseError;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::new(path)
    }
}

impl TryFrom<CString> for Path {
    type Error = ParseError;

    fn try_from(path: CString) -> Result<Self, ParseError> {
        Self::new(path.into_string().map_err(|_| ParseError::InvalidValue)?)
    }
}

impl From<Path> for CString {
    fn from(path: Path) -> Self {
        // SAFETY: in `Path::new` we already verified that there are no
        // embedded NULs.
        unsafe { CString::from_vec_unchecked(path.to_string().as_bytes().to_owned()) }
    }
}

impl From<Path> for String {
    fn from(path: Path) -> String {
        path.to_string()
    }
}

impl<'de> de::Deserialize<'de> for Path {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Path;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH characters \
                     in length, with a leading `/`, and containing no \
                     empty path segments",
                )
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue
                    | ParseError::InvalidSegment
                    | ParseError::NoLeadingSlash => E::invalid_value(
                        de::Unexpected::Str(s),
                        &"a path with leading `/` and non-empty segments, where each segment is no \
                        more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., \
                        and cannot contain embedded NULs",
                    ),
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &"a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH bytes \
                        in length",
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

/// Same as [Path] except the path does not begin with `/`.
#[derive(Eq, Ord, PartialOrd, PartialEq, Hash, Clone)]
pub struct RelativePath {
    rep: FlyStr,
}

impl RelativePath {
    /// Like [Path::new] but `path` must not begin with `/` and may be `.`.
    pub fn new(path: impl AsRef<str>) -> Result<Self, ParseError> {
        let path: &str = path.as_ref();
        if path == "." {
            return Ok(Self::dot());
        }
        if path.is_empty() {
            return Err(ParseError::Empty);
        }
        if path.len() > MAX_PATH_LENGTH {
            return Err(ParseError::TooLong);
        }
        path.split('/').try_for_each(|s| {
            Name::new(s).map(|_| ()).map_err(|e| match e {
                ParseError::Empty => ParseError::InvalidValue,
                _ => ParseError::InvalidSegment,
            })
        })?;
        Ok(Self { rep: path.into() })
    }

    pub fn dot() -> Self {
        Self { rep: ".".into() }
    }

    pub fn is_dot(&self) -> bool {
        self.rep == "."
    }

    pub fn parent(&self) -> Option<Self> {
        if self.is_dot() {
            None
        } else {
            match self.rep.rfind('/') {
                Some(idx) => Some(Self::new(&self.rep[0..idx]).unwrap()),
                None => Some(Self::dot()),
            }
        }
    }

    pub fn split(&self) -> Vec<&BorrowedName> {
        if self.is_dot() {
            vec![]
        } else {
            self.rep.split('/').map(|s| BorrowedName::new_unchecked(s)).collect()
        }
    }

    pub fn len(&self) -> usize {
        if self.is_dot() { 0 } else { self.rep.split('/').count() }
    }

    pub fn basename(&self) -> Option<&BorrowedName> {
        if self.is_dot() {
            None
        } else {
            match self.rep.rfind('/') {
                Some(idx) => Some(BorrowedName::new_unchecked(&self.rep[idx + 1..])),
                None => Some(BorrowedName::new_unchecked(&self.rep)),
            }
        }
    }

    pub fn to_path_buf(&self) -> PathBuf {
        if self.is_dot() { PathBuf::new() } else { PathBuf::from(self.to_string()) }
    }

    // Attaches the path `other` to the end of `self`. Returns `true` on success, and false
    // if the resulting path's length would exceed `MAX_PATH_LENGTH`.
    #[must_use]
    pub fn extend(&mut self, other: Self) -> bool {
        let rep = if self.is_dot() {
            other.rep
        } else if !other.is_dot() {
            format!("{}/{}", self.rep, other.rep).into()
        } else {
            // Nothing to do.
            return true;
        };
        if rep.len() > MAX_PATH_LENGTH {
            return false;
        }
        self.rep = rep;
        true
    }

    // Attaches `segment` to the end of `self`. Returns `true` on success, and false
    // if the resulting path's length would exceed `MAX_PATH_LENGTH`.
    #[must_use]
    pub fn push(&mut self, segment: Name) -> bool {
        let rep: FlyStr = if self.is_dot() {
            format!("{segment}").into()
        } else {
            format!("{}/{}", self.rep, segment).into()
        };
        if rep.len() > MAX_PATH_LENGTH {
            return false;
        }
        self.rep = rep;
        true
    }

    pub fn pop_front(&mut self) -> Option<Name> {
        if self.is_dot() {
            None
        } else {
            let (rep, front) = match self.rep.find('/') {
                Some(idx) => {
                    let rep = self.rep[idx + 1..].into();
                    let front = Name::new_unchecked(&self.rep[0..idx]);
                    (rep, front)
                }
                None => (".".into(), Name::new_unchecked(&self.rep)),
            };
            self.rep = rep;
            Some(front)
        }
    }
}

impl Default for RelativePath {
    fn default() -> Self {
        Self::dot()
    }
}

impl IterablePath for RelativePath {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        Box::new(self.split().into_iter())
    }
}

impl FromStr for RelativePath {
    type Err = ParseError;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::new(path)
    }
}

impl From<RelativePath> for String {
    fn from(path: RelativePath) -> String {
        path.to_string()
    }
}

impl From<Vec<Name>> for RelativePath {
    fn from(segments: Vec<Name>) -> Self {
        if segments.is_empty() {
            Self::dot()
        } else {
            Self { rep: segments.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("/").into() }
        }
    }
}

impl From<Vec<&BorrowedName>> for RelativePath {
    fn from(segments: Vec<&BorrowedName>) -> Self {
        if segments.is_empty() {
            Self::dot()
        } else {
            Self {
                rep: segments.into_iter().map(|s| s.as_str()).collect::<Vec<_>>().join("/").into(),
            }
        }
    }
}

impl fmt::Debug for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rep)
    }
}

impl ser::Serialize for RelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> de::Deserialize<'de> for RelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = RelativePath;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH characters \
                     in length, not starting with `/`, and containing no empty path segments",
                )
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue
                    | ParseError::InvalidSegment
                    | ParseError::NoLeadingSlash => E::invalid_value(
                        de::Unexpected::Str(s),
                        &"a path with no leading `/` and non-empty segments",
                    ),
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &"a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH characters \
                        in length",
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

/// Path that separates the dirname and basename as different variables
/// (referencing type). Convenient for / path representations that split the
/// dirname and basename, like Fuchsia component decl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowedSeparatedPath<'a> {
    pub dirname: &'a RelativePath,
    pub basename: &'a Name,
}

impl BorrowedSeparatedPath<'_> {
    /// Converts this [BorrowedSeparatedPath] to the owned type.
    pub fn to_owned(&self) -> SeparatedPath {
        SeparatedPath { dirname: self.dirname.clone(), basename: self.basename.clone() }
    }
}

impl fmt::Display for BorrowedSeparatedPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.dirname.is_dot() {
            write!(f, "{}/{}", self.dirname, self.basename)
        } else {
            write!(f, "{}", self.basename)
        }
    }
}

impl IterablePath for BorrowedSeparatedPath<'_> {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        Box::new(self.dirname.iter_segments().chain(iter::once(self.basename as &BorrowedName)))
    }
}

/// Path that separates the dirname and basename as different variables (owned
/// type). Convenient for path representations that split the dirname and
/// basename, like Fuchsia component decl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeparatedPath {
    pub dirname: RelativePath,
    pub basename: Name,
}

impl SeparatedPath {
    /// Obtains a reference to this [SeparatedPath] as the borrowed type.
    pub fn as_ref(&self) -> BorrowedSeparatedPath<'_> {
        BorrowedSeparatedPath { dirname: &self.dirname, basename: &self.basename }
    }
}

impl IterablePath for SeparatedPath {
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send {
        Box::new(self.dirname.iter_segments().chain(iter::once(&self.basename as &BorrowedName)))
    }
}

impl fmt::Display for SeparatedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.dirname.is_dot() {
            write!(f, "{}/{}", self.dirname, self.basename)
        } else {
            write!(f, "{}", self.basename)
        }
    }
}

/// Trait implemented by path types that provides an API to iterate over path segments.
pub trait IterablePath: Clone + Send + Sync {
    /// Returns a double-sided iterator over the segments in this path.
    fn iter_segments(&self) -> impl DoubleEndedIterator<Item = &BorrowedName> + Send;
}

/// A component URL. The URL is validated, but represented as a string to avoid
/// normalization and retain the original representation.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Url(FlyStr);

impl Url {
    /// Creates a `Url` from a `&str` slice, returning an `Err` if the string fails
    /// validation. The string must be non-empty, no more than 4096 characters
    /// in length, and be a valid URL. See the [`url`](../../url/index.html) crate.
    pub fn new(url: impl AsRef<str> + Into<String>) -> Result<Self, ParseError> {
        Self::validate(url.as_ref())?;
        Ok(Self(FlyStr::new(url)))
    }

    /// Verifies the given string is a valid absolute or relative component URL.
    pub fn validate(url_str: &str) -> Result<(), ParseError> {
        if url_str.is_empty() {
            return Err(ParseError::Empty);
        }
        if url_str.len() > MAX_URL_LENGTH {
            return Err(ParseError::TooLong);
        }
        match url::Url::parse(url_str).map(|url| (url, false)).or_else(|err| {
            if err == url::ParseError::RelativeUrlWithoutBase {
                DEFAULT_BASE_URL.join(url_str).map(|url| (url, true))
            } else {
                Err(err)
            }
        }) {
            Ok((url, is_relative)) => {
                let mut path = url.path();
                if path.starts_with('/') {
                    path = &path[1..];
                }
                if is_relative && url.fragment().is_none() {
                    // TODO(https://fxbug.dev/42070831): Fragments should be optional
                    // for relative path URLs.
                    //
                    // Historically, a component URL string without a scheme
                    // was considered invalid, unless it was only a fragment.
                    // Subpackages allow a relative path URL, and by current
                    // definition they require a fragment. By declaring a
                    // relative path without a fragment "invalid", we can avoid
                    // breaking tests that expect a path-only string to be
                    // invalid. Sadly this appears to be a behavior of the
                    // public API.
                    return Err(ParseError::InvalidComponentUrl {
                        details: "Relative URL has no resource fragment.".to_string(),
                    });
                }
                if url.host_str().unwrap_or("").is_empty()
                    && path.is_empty()
                    && url.fragment().is_none()
                {
                    return Err(ParseError::InvalidComponentUrl {
                        details: "URL is missing either `host`, `path`, and/or `resource`."
                            .to_string(),
                    });
                }
            }
            Err(err) => {
                return Err(ParseError::InvalidComponentUrl {
                    details: format!("Malformed URL: {err:?}."),
                });
            }
        }
        // Use the unparsed URL string so that the original format is preserved.
        Ok(())
    }

    pub fn is_relative(&self) -> bool {
        matches!(url::Url::parse(&self.0), Err(url::ParseError::RelativeUrlWithoutBase))
    }

    pub fn scheme(&self) -> Option<String> {
        url::Url::parse(&self.0).ok().map(|u| u.scheme().into())
    }

    pub fn resource(&self) -> Option<String> {
        url::Url::parse(&self.0).ok().map(|u| u.fragment().map(str::to_string)).flatten()
    }

    pub fn as_str(&self) -> &str {
        &*self.0
    }
}

impl FromStr for Url {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::new(url)
    }
}

impl From<Url> for String {
    fn from(url: Url) -> String {
        url.0.into()
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl ser::Serialize for Url {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> de::Deserialize<'de> for Url {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Url;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a non-empty URL no more than 4096 characters in length")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidComponentUrl { details: _ } => {
                        E::invalid_value(de::Unexpected::Str(s), &"a valid URL")
                    }
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &"a non-empty URL no more than 4096 characters in length",
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

impl PartialEq<&str> for Url {
    fn eq(&self, o: &&str) -> bool {
        &*self.0 == *o
    }
}

impl PartialEq<String> for Url {
    fn eq(&self, o: &String) -> bool {
        &*self.0 == *o
    }
}

/// A URL scheme.
#[derive(Serialize, Clone, Debug, Eq, Hash, PartialEq)]
pub struct UrlScheme(FlyStr);

impl UrlScheme {
    /// Creates a `UrlScheme` from a `String`, returning an `Err` if the string fails
    /// validation. The string must be non-empty and no more than 100 characters
    /// in length. It must start with a lowercase ASCII letter (a-z),
    /// and contain only lowercase ASCII letters, digits, `+`, `-`, and `.`.
    pub fn new(url_scheme: impl AsRef<str> + Into<String>) -> Result<Self, ParseError> {
        Self::validate(url_scheme.as_ref())?;
        Ok(UrlScheme(FlyStr::new(url_scheme)))
    }

    /// Validates `url_scheme` but does not construct a new `UrlScheme` object.
    /// See [`UrlScheme::new`] for validation details.
    pub fn validate(url_scheme: &str) -> Result<(), ParseError> {
        if url_scheme.is_empty() {
            return Err(ParseError::Empty);
        }
        if url_scheme.len() > MAX_NAME_LENGTH {
            return Err(ParseError::TooLong);
        }
        let mut iter = url_scheme.chars();
        let first_char = iter.next().unwrap();
        if !first_char.is_ascii_lowercase() {
            return Err(ParseError::InvalidValue);
        }
        if let Some(_) = iter.find(|&c| {
            !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '.' && c != '+' && c != '-'
        }) {
            return Err(ParseError::InvalidValue);
        }
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &*self.0
    }
}

impl fmt::Display for UrlScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for UrlScheme {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl From<UrlScheme> for String {
    fn from(u: UrlScheme) -> String {
        u.0.into()
    }
}

impl<'de> de::Deserialize<'de> for UrlScheme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UrlScheme;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a non-empty URL scheme no more than 100 characters in length")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue => {
                        E::invalid_value(de::Unexpected::Str(s), &"a valid URL scheme")
                    }
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &"a non-empty URL scheme no more than 100 characters in length",
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

/// The duration of child components in a collection. See [`Durability`].
///
/// [`Durability`]: ../../fidl_fuchsia_sys2/enum.Durability.html
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    Transient,
    /// An instance is started on creation and exists until it stops.
    SingleRun,
}

symmetrical_enums!(Durability, fdecl::Durability, Transient, SingleRun);

/// A component instance's startup mode. See [`StartupMode`].
///
/// [`StartupMode`]: ../../fidl_fuchsia_sys2/enum.StartupMode.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    Lazy,
    Eager,
}

impl StartupMode {
    pub fn is_lazy(&self) -> bool {
        matches!(self, StartupMode::Lazy)
    }
}

symmetrical_enums!(StartupMode, fdecl::StartupMode, Lazy, Eager);

impl Default for StartupMode {
    fn default() -> Self {
        Self::Lazy
    }
}

/// A component instance's recovery policy. See [`OnTerminate`].
///
/// [`OnTerminate`]: ../../fidl_fuchsia_sys2/enum.OnTerminate.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnTerminate {
    None,
    Reboot,
}

symmetrical_enums!(OnTerminate, fdecl::OnTerminate, None, Reboot);

impl Default for OnTerminate {
    fn default() -> Self {
        Self::None
    }
}

/// The kinds of offers that can target components in a given collection. See
/// [`AllowedOffers`].
///
/// [`AllowedOffers`]: ../../fidl_fuchsia_sys2/enum.AllowedOffers.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AllowedOffers {
    StaticOnly,
    StaticAndDynamic,
}

symmetrical_enums!(AllowedOffers, fdecl::AllowedOffers, StaticOnly, StaticAndDynamic);

impl Default for AllowedOffers {
    fn default() -> Self {
        Self::StaticOnly
    }
}

/// Offered dependency type. See [`DependencyType`].
///
/// [`DependencyType`]: ../../fidl_fuchsia_sys2/enum.DependencyType.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyType {
    Strong,
    Weak,
}

symmetrical_enums!(DependencyType, fdecl::DependencyType, Strong, Weak);

impl Default for DependencyType {
    fn default() -> Self {
        Self::Strong
    }
}

/// Capability availability. See [`Availability`].
///
/// [`Availability`]: ../../fidl_fuchsia_sys2/enum.Availability.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Required,
    Optional,
    SameAsTarget,
    Transitional,
}

symmetrical_enums!(
    Availability,
    fdecl::Availability,
    Required,
    Optional,
    SameAsTarget,
    Transitional
);

impl Display for Availability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Availability::Required => write!(f, "Required"),
            Availability::Optional => write!(f, "Optional"),
            Availability::SameAsTarget => write!(f, "SameAsTarget"),
            Availability::Transitional => write!(f, "Transitional"),
        }
    }
}

// TODO(cgonyeo): remove this once we've soft migrated to the availability field being required.
impl Default for Availability {
    fn default() -> Self {
        Self::Required
    }
}

impl PartialOrd for Availability {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        match (*self, *other) {
            (Availability::Transitional, Availability::Optional)
            | (Availability::Transitional, Availability::Required)
            | (Availability::Optional, Availability::Required) => Some(cmp::Ordering::Less),
            (Availability::Optional, Availability::Transitional)
            | (Availability::Required, Availability::Transitional)
            | (Availability::Required, Availability::Optional) => Some(cmp::Ordering::Greater),
            (Availability::Required, Availability::Required)
            | (Availability::Optional, Availability::Optional)
            | (Availability::Transitional, Availability::Transitional)
            | (Availability::SameAsTarget, Availability::SameAsTarget) => {
                Some(cmp::Ordering::Equal)
            }
            (Availability::SameAsTarget, _) | (_, Availability::SameAsTarget) => None,
        }
    }
}

/// Specifies when the framework will open the protocol from the provider
/// component's outgoing directory when someone requests the capability. See
/// [`DeliveryType`].
///
/// [`DeliveryType`]: ../../fidl_fuchsia_component_decl/enum.DeliveryType.html
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Copy)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryType {
    Immediate,
    OnReadable,
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl TryFrom<fdecl::DeliveryType> for DeliveryType {
    type Error = fdecl::DeliveryType;

    fn try_from(value: fdecl::DeliveryType) -> Result<Self, Self::Error> {
        match value {
            fdecl::DeliveryType::Immediate => Ok(DeliveryType::Immediate),
            fdecl::DeliveryType::OnReadable => Ok(DeliveryType::OnReadable),
            fdecl::DeliveryTypeUnknown!() => Err(value),
        }
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl From<DeliveryType> for fdecl::DeliveryType {
    fn from(value: DeliveryType) -> Self {
        match value {
            DeliveryType::Immediate => fdecl::DeliveryType::Immediate,
            DeliveryType::OnReadable => fdecl::DeliveryType::OnReadable,
        }
    }
}

impl Display for DeliveryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeliveryType::Immediate => write!(f, "Immediate"),
            DeliveryType::OnReadable => write!(f, "OnReadable"),
        }
    }
}

impl Default for DeliveryType {
    fn default() -> Self {
        Self::Immediate
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageId {
    StaticInstanceId,
    StaticInstanceIdOrMoniker,
}

symmetrical_enums!(StorageId, fdecl::StorageId, StaticInstanceId, StaticInstanceIdOrMoniker);

/// We can't link the fuchsia-runtime crate because it's target side only, but we don't really
/// need to -- its HandleType is pretty much just a thin wrapper over `u8`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandleType(u8);

impl Serialize for HandleType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl From<HandleType> for u8 {
    fn from(h: HandleType) -> Self {
        h.0
    }
}

impl From<u8> for HandleType {
    fn from(h: u8) -> Self {
        Self(h)
    }
}

#[cfg(target_os = "fuchsia")]
impl From<fuchsia_runtime::HandleType> for HandleType {
    fn from(h: fuchsia_runtime::HandleType) -> Self {
        (h as u8).into()
    }
}

impl From<HandleType> for Name {
    fn from(numbered_handle: HandleType) -> Self {
        let numbered_handle: u8 = numbered_handle.into();
        let numbered_handle = format!("{numbered_handle:x}");
        Self::new(numbered_handle).expect("numbered_handle is a valid dictionary key")
    }
}

impl fmt::Display for HandleType {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

const HANDLE_TYPE_EXPECT_STR: &str = "a uint8 from zircon/processargs.h";

impl<'de> de::Deserialize<'de> for HandleType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = HandleType;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(HANDLE_TYPE_EXPECT_STR)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let v = v.try_into().map_err(|_| {
                    de::Error::invalid_value(de::Unexpected::Unsigned(v), &HANDLE_TYPE_EXPECT_STR)
                })?;
                Ok(HandleType(v))
            }
        }
        deserializer.deserialize_u64(Visitor)
    }
}

/// A namespace entry type. Identical to the type in the `namespace` crate but can be used in
/// contexts where that crate is not included.
#[derive(Debug)]
pub struct NamespaceEntry {
    pub path: NamespacePath,
    pub directory: fidl::endpoints::ClientEnd<fio::DirectoryMarker>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde_json::json;
    use std::collections::HashSet;
    use std::iter::repeat;

    macro_rules! expect_ok {
        ($type_:ty, $($input:tt)+) => {
            assert_matches!(
                serde_json::from_str::<$type_>(&json!($($input)*).to_string()),
                Ok(_)
            );
        };
    }

    macro_rules! expect_ok_no_serialize {
        ($type_:ty, $($input:tt)+) => {
            assert_matches!(
                ($($input)*).parse::<$type_>(),
                Ok(_)
            );
        };
    }

    macro_rules! expect_err_no_serialize {
        ($type_:ty, $err:pat, $($input:tt)+) => {
            assert_matches!(
                ($($input)*).parse::<$type_>(),
                Err($err)
            );
        };
    }

    macro_rules! expect_err {
        ($type_:ty, $err:pat, $($input:tt)+) => {
            assert_matches!(
                ($($input)*).parse::<$type_>(),
                Err($err)
            );
            assert_matches!(
                serde_json::from_str::<$type_>(&json!($($input)*).to_string()),
                Err(_)
            );
        };
    }

    #[test]
    fn test_valid_name() {
        expect_ok!(Name, "foo");
        expect_ok!(Name, "Foo");
        expect_ok!(Name, "O123._-");
        expect_ok!(Name, "_O123._-");
        expect_ok!(Name, repeat("x").take(255).collect::<String>());
    }

    #[test]
    fn test_invalid_name() {
        expect_err!(Name, ParseError::Empty, "");
        expect_err!(Name, ParseError::InvalidValue, "-");
        expect_err!(Name, ParseError::InvalidValue, ".");
        expect_err!(Name, ParseError::InvalidValue, "@&%^");
        expect_err!(Name, ParseError::TooLong, repeat("x").take(256).collect::<String>());
    }

    #[test]
    fn test_valid_path() {
        expect_ok!(Path, "/foo");
        expect_ok!(Path, "/foo/bar");
        expect_ok!(Path, format!("/{}", repeat("x").take(100).collect::<String>()).as_str());
        // 2047 * 2 characters per repeat = 4094
        expect_ok!(Path, repeat("/x").take(2047).collect::<String>().as_str());
    }

    #[test]
    fn test_invalid_path() {
        expect_err!(Path, ParseError::Empty, "");
        expect_err!(Path, ParseError::InvalidValue, "/");
        expect_err!(Path, ParseError::InvalidValue, ".");
        expect_err!(Path, ParseError::NoLeadingSlash, "foo");
        expect_err!(Path, ParseError::NoLeadingSlash, "foo/");
        expect_err!(Path, ParseError::InvalidValue, "/foo/");
        expect_err!(Path, ParseError::InvalidValue, "/foo//bar");
        expect_err!(Path, ParseError::InvalidSegment, "/fo\0b/bar");
        expect_err!(Path, ParseError::InvalidSegment, "/.");
        expect_err!(Path, ParseError::InvalidSegment, "/foo/.");
        expect_err!(
            Path,
            ParseError::InvalidSegment,
            format!("/{}", repeat("x").take(256).collect::<String>()).as_str()
        );
        // 2048 * 2 characters per repeat = 4096
        expect_err!(
            Path,
            ParseError::TooLong,
            repeat("/x").take(2048).collect::<String>().as_str()
        );
    }

    #[test]
    fn test_name_hash() {
        {
            let n1 = Name::new("a").unwrap();
            let s_b = repeat("b").take(255).collect::<String>();
            let n2 = Name::new(&s_b).unwrap();
            let b1 = BorrowedName::new("a").unwrap();
            let b2 = BorrowedName::new(&s_b).unwrap();

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
            let n1 = LongName::new("a").unwrap();
            let s_b = repeat("b").take(1024).collect::<String>();
            let n2 = LongName::new(&s_b).unwrap();
            let b1 = BorrowedLongName::new("a").unwrap();
            let b2 = BorrowedLongName::new(&s_b).unwrap();

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

    // Keep in sync with test_relative_path_methods()
    #[test]
    fn test_path_methods() {
        let dot = RelativePath::dot();
        let prefix = Path::new("/some/path").unwrap();
        let suffix = RelativePath::new("another/path").unwrap();
        let segment = Name::new("segment").unwrap();

        let mut path = prefix.clone();
        assert!(path.extend(suffix.clone()));
        assert_eq!(path, "/some/path/another/path".parse().unwrap());
        assert_eq!(
            path.split(),
            [
                BorrowedName::new("some").unwrap(),
                BorrowedName::new("path").unwrap(),
                BorrowedName::new("another").unwrap(),
                BorrowedName::new("path").unwrap(),
            ]
        );

        let mut path = prefix.clone();
        assert!(path.extend(dot.clone()));
        assert_eq!(path, "/some/path".parse().unwrap());

        let mut path = prefix.clone();
        assert!(path.push(segment.clone()));
        assert_eq!(path, "/some/path/segment".parse().unwrap());
        assert!(path.push(segment.clone()));
        assert_eq!(path, "/some/path/segment/segment".parse().unwrap());
        assert_eq!(
            path.split(),
            [
                BorrowedName::new("some").unwrap(),
                BorrowedName::new("path").unwrap(),
                BorrowedName::new("segment").unwrap(),
                BorrowedName::new("segment").unwrap(),
            ]
        );

        let long_path =
            Path::new(format!("{}/xx", repeat("/x").take(4092 / 2).collect::<String>())).unwrap();
        let mut path = long_path.clone();
        // One more than the maximum size.
        assert!(!path.push("a".parse().unwrap()));
        assert_eq!(path, long_path);
        assert!(!path.extend("a".parse().unwrap()));
        assert_eq!(path, long_path);
    }

    #[test]
    fn test_valid_namespace_path() {
        expect_ok_no_serialize!(NamespacePath, "/");
        expect_ok_no_serialize!(NamespacePath, "/foo");
        expect_ok_no_serialize!(NamespacePath, "/foo/bar");
        expect_ok_no_serialize!(
            NamespacePath,
            format!("/{}", repeat("x").take(100).collect::<String>()).as_str()
        );
        // 2047 * 2 characters per repeat = 4094
        expect_ok_no_serialize!(
            NamespacePath,
            repeat("/x").take(2047).collect::<String>().as_str()
        );
    }

    #[test]
    fn test_invalid_namespace_path() {
        expect_err_no_serialize!(NamespacePath, ParseError::Empty, "");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidValue, ".");
        expect_err_no_serialize!(NamespacePath, ParseError::NoLeadingSlash, "foo");
        expect_err_no_serialize!(NamespacePath, ParseError::NoLeadingSlash, "foo/");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidValue, "/foo/");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidValue, "/foo//bar");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidSegment, "/fo\0b/bar");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidSegment, "/.");
        expect_err_no_serialize!(NamespacePath, ParseError::InvalidSegment, "/foo/.");
        expect_err_no_serialize!(
            NamespacePath,
            ParseError::InvalidSegment,
            format!("/{}", repeat("x").take(256).collect::<String>()).as_str()
        );
        // 2048 * 2 characters per repeat = 4096
        expect_err_no_serialize!(
            Path,
            ParseError::TooLong,
            repeat("/x").take(2048).collect::<String>().as_str()
        );
    }

    #[test]
    fn test_path_parent_basename() {
        let path = Path::new("/foo").unwrap();
        assert_eq!((path.parent().to_string().as_str(), path.basename().as_str()), ("/", "foo"));
        let path = Path::new("/foo/bar").unwrap();
        assert_eq!((path.parent().to_string().as_str(), path.basename().as_str()), ("/foo", "bar"));
        let path = Path::new("/foo/bar/baz").unwrap();
        assert_eq!(
            (path.parent().to_string().as_str(), path.basename().as_str()),
            ("/foo/bar", "baz")
        );
    }

    #[test]
    fn test_separated_path() {
        fn test_path(path: SeparatedPath, in_expected_segments: Vec<&str>) {
            let expected_segments: Vec<&BorrowedName> =
                in_expected_segments.iter().map(|s| BorrowedName::new(*s).unwrap()).collect();
            let segments: Vec<&BorrowedName> = path.iter_segments().collect();
            assert_eq!(segments, expected_segments);
            let borrowed_path = path.as_ref();
            let segments: Vec<&BorrowedName> = borrowed_path.iter_segments().collect();
            assert_eq!(segments, expected_segments);
            let owned_path = borrowed_path.to_owned();
            assert_eq!(path, owned_path);
            let expected_fmt = in_expected_segments.join("/");
            assert_eq!(format!("{path}"), expected_fmt);
            assert_eq!(format!("{owned_path}"), expected_fmt);
        }
        test_path(
            SeparatedPath { dirname: ".".parse().unwrap(), basename: "foo".parse().unwrap() },
            vec!["foo"],
        );
        test_path(
            SeparatedPath { dirname: "bar".parse().unwrap(), basename: "foo".parse().unwrap() },
            vec!["bar", "foo"],
        );
        test_path(
            SeparatedPath { dirname: "bar/baz".parse().unwrap(), basename: "foo".parse().unwrap() },
            vec!["bar", "baz", "foo"],
        );
    }

    #[test]
    fn test_valid_relative_path() {
        expect_ok!(RelativePath, ".");
        expect_ok!(RelativePath, "foo");
        expect_ok!(RelativePath, "foo/bar");
        expect_ok!(RelativePath, &format!("x{}", repeat("/x").take(2047).collect::<String>()));
    }

    #[test]
    fn test_invalid_relative_path() {
        expect_err!(RelativePath, ParseError::Empty, "");
        expect_err!(RelativePath, ParseError::InvalidValue, "/");
        expect_err!(RelativePath, ParseError::InvalidValue, "/foo");
        expect_err!(RelativePath, ParseError::InvalidValue, "foo/");
        expect_err!(RelativePath, ParseError::InvalidValue, "/foo/");
        expect_err!(RelativePath, ParseError::InvalidValue, "foo//bar");
        expect_err!(RelativePath, ParseError::InvalidSegment, "..");
        expect_err!(RelativePath, ParseError::InvalidSegment, "foo/..");
        expect_err!(
            RelativePath,
            ParseError::TooLong,
            &format!("x{}", repeat("/x").take(2048).collect::<String>())
        );
    }

    // Keep in sync with test_path_methods()
    #[test]
    fn test_relative_path_methods() {
        let dot = RelativePath::dot();
        let prefix = RelativePath::new("some/path").unwrap();
        let suffix = RelativePath::new("another/path").unwrap();
        let segment = Name::new("segment").unwrap();

        let mut path = prefix.clone();
        assert!(path.extend(suffix.clone()));
        assert_eq!(path, "some/path/another/path".parse().unwrap());
        assert_eq!(
            path.split(),
            [
                BorrowedName::new("some").unwrap(),
                BorrowedName::new("path").unwrap(),
                BorrowedName::new("another").unwrap(),
                BorrowedName::new("path").unwrap(),
            ]
        );
        assert_eq!(path.pop_front(), Some(Name::new("some").unwrap()));
        assert_eq!(path.pop_front(), Some(Name::new("path").unwrap()));
        assert_eq!(path.pop_front(), Some(Name::new("another").unwrap()));
        assert_eq!(path.pop_front(), Some(Name::new("path").unwrap()));
        assert_eq!(path.pop_front(), None);

        let mut path = prefix.clone();
        assert!(path.extend(dot.clone()));
        assert_eq!(path, "some/path".parse().unwrap());
        let mut path = dot.clone();
        assert!(path.extend(suffix));
        assert_eq!(path, "another/path".parse().unwrap());
        let mut path = dot.clone();
        assert!(path.extend(dot.clone()));
        assert_eq!(path, RelativePath::dot());

        let mut path = prefix.clone();
        assert!(path.push(segment.clone()));
        assert_eq!(path, "some/path/segment".parse().unwrap());
        assert!(path.push(segment.clone()));
        assert_eq!(path, "some/path/segment/segment".parse().unwrap());
        assert_eq!(
            path.split(),
            [
                BorrowedName::new("some").unwrap(),
                BorrowedName::new("path").unwrap(),
                BorrowedName::new("segment").unwrap(),
                BorrowedName::new("segment").unwrap(),
            ]
        );

        let mut path = dot.clone();
        assert!(path.push(segment.clone()));
        assert_eq!(path, "segment".parse().unwrap());

        let long_path =
            RelativePath::new(format!("{}x", repeat("x/").take(4094 / 2).collect::<String>()))
                .unwrap();
        let mut path = long_path.clone();
        // One more than the maximum size.
        assert!(!path.push("a".parse().unwrap()));
        assert_eq!(path, long_path);
        assert!(!path.extend("a".parse().unwrap()));
        assert_eq!(path, long_path);
    }

    #[test]
    fn test_valid_url() {
        expect_ok!(Url, "a://foo");
        expect_ok!(Url, "#relative-url");
        expect_ok!(Url, &format!("a://{}", repeat("x").take(4092).collect::<String>()));
    }

    #[test]
    fn test_invalid_url() {
        expect_err!(Url, ParseError::Empty, "");
        expect_err!(Url, ParseError::InvalidComponentUrl { .. }, "foo");
        expect_err!(
            Url,
            ParseError::TooLong,
            &format!("a://{}", repeat("x").take(4093).collect::<String>())
        );
    }

    #[test]
    fn test_valid_url_scheme() {
        expect_ok!(UrlScheme, "fuch.sia-pkg+0");
        expect_ok!(UrlScheme, &format!("{}", repeat("f").take(255).collect::<String>()));
    }

    #[test]
    fn test_invalid_url_scheme() {
        expect_err!(UrlScheme, ParseError::Empty, "");
        expect_err!(UrlScheme, ParseError::InvalidValue, "0fuch.sia-pkg+0");
        expect_err!(UrlScheme, ParseError::InvalidValue, "fuchsia_pkg");
        expect_err!(UrlScheme, ParseError::InvalidValue, "FUCHSIA-PKG");
        expect_err!(
            UrlScheme,
            ParseError::TooLong,
            &format!("{}", repeat("f").take(256).collect::<String>())
        );
    }

    #[test]
    fn test_name_error_message() {
        let input = r#"
            "foo$"
        "#;
        let err = serde_json::from_str::<Name>(input).expect_err("must fail");
        assert_eq!(
            err.to_string(),
            "invalid value: string \"foo$\", expected a name \
            that consists of [A-Za-z0-9_.-] and starts with [A-Za-z0-9_] \
            at line 2 column 18"
        );
        assert_eq!(err.line(), 2);
        assert_eq!(err.column(), 18);
    }

    #[test]
    fn test_path_error_message() {
        let input = r#"
            "foo";
        "#;
        let err = serde_json::from_str::<Path>(input).expect_err("must fail");
        assert_eq!(
            err.to_string(),
            "invalid value: string \"foo\", expected a path with leading `/` and non-empty \
            segments, where each segment is no \
            more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., \
            and cannot contain embedded NULs at line 2 column 17"
        );

        assert_eq!(err.line(), 2);
        assert_eq!(err.column(), 17);
    }

    #[test]
    fn test_url_error_message() {
        let input = r#"
            "foo";
        "#;
        let err = serde_json::from_str::<Url>(input).expect_err("must fail");
        assert_eq!(
            err.to_string(),
            "invalid value: string \"foo\", expected a valid URL at line 2 \
             column 17"
        );
        assert_eq!(err.line(), 2);
        assert_eq!(err.column(), 17);
    }

    #[test]
    fn test_url_scheme_error_message() {
        let input = r#"
            "9fuchsia_pkg"
        "#;
        let err = serde_json::from_str::<UrlScheme>(input).expect_err("must fail");
        assert_eq!(
            err.to_string(),
            "invalid value: string \"9fuchsia_pkg\", expected a valid URL scheme at line 2 column 26"
        );
        assert_eq!(err.line(), 2);
        assert_eq!(err.column(), 26);
    }

    #[test]
    fn test_symmetrical_enums() {
        mod a {
            #[derive(Debug, PartialEq, Eq)]
            pub enum Streetlight {
                Green,
                Yellow,
                Red,
            }
        }

        mod b {
            #[derive(Debug, PartialEq, Eq)]
            pub enum Streetlight {
                Green,
                Yellow,
                Red,
            }
        }

        symmetrical_enums!(a::Streetlight, b::Streetlight, Green, Yellow, Red);

        assert_eq!(a::Streetlight::Green, b::Streetlight::Green.into());
        assert_eq!(a::Streetlight::Yellow, b::Streetlight::Yellow.into());
        assert_eq!(a::Streetlight::Red, b::Streetlight::Red.into());
        assert_eq!(b::Streetlight::Green, a::Streetlight::Green.into());
        assert_eq!(b::Streetlight::Yellow, a::Streetlight::Yellow.into());
        assert_eq!(b::Streetlight::Red, a::Streetlight::Red.into());
    }
}
