// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types implementing the [flyweight pattern](https://en.wikipedia.org/wiki/Flyweight_pattern)
//! for reusing string allocations.
//!
//! # Features
//!
//! * Supports UTF-8 strings with [`FlyStr`] and UTF-8-ish bytestrings with [`FlyByteStr`].
//! * Easy drop-in replacement for immutable strings without wiring up any additional function args.
//! * Accessing the underlying string values has overhead similar to a `Box<str>`.
//! * Cheap to clone.
//! * Strings in the cache are freed when the last reference to them is dropped.
//! * Heap allocations are avoided when possible with small string optimizations (SSO).
//! * Hashing a flyweight and comparing it for equality against another flyweight are both O(1)
//!   operations.
//!
//! # Tradeoffs
//!
//! This was originally written for [Fuchsia](https://fuchsia.dev) at a time when popular options
//! didn't fit the needs of adding caching to an existing long-running multithreaded system service
//! that holds an unbounded number of user-controlled strings. The above features are suited to this
//! use case, but there are many (many) alternative [string caching crates] to choose from if you
//! have different constraints.
//!
//! [string caching crates]: https://crates.io/search?q=intern

#![warn(missing_docs, clippy::all)]

mod raw;

use ahash::AHashSet;
use bstr::{BStr, BString};
use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::Mutex;

#[cfg(feature = "serde")]
use serde::de::{Deserializer, Visitor};
#[cfg(feature = "serde")]
use serde::ser::Serializer;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// The global string cache for `FlyStr`.
///
/// If a live `FlyStr` contains an `Storage`, the `Storage` must also be in this cache and it must
/// have a refcount of >= 2.
static CACHE: std::sync::LazyLock<Mutex<AHashSet<Storage>>> =
    std::sync::LazyLock::new(|| Mutex::new(AHashSet::new()));

/// Wrapper type for stored strings that lets us query the cache without an owned value.
#[repr(transparent)]
struct Storage(NonNull<raw::Payload>);

// SAFETY: FlyStr storage is always safe to send across threads.
unsafe impl Send for Storage {}
// SAFETY: FlyStr storage is always safe to share across threads.
unsafe impl Sync for Storage {}

impl PartialEq for Storage {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Storage {}

impl Hash for Storage {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state)
    }
}

impl Storage {
    #[inline]
    fn inc_ref(&self) -> usize {
        // SAFETY: `Storage` always points to a valid `Payload`.
        unsafe { raw::Payload::inc_ref(self.0.as_ptr()) }
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: `Storage` always points to a valid `Payload`.
        unsafe { &*raw::Payload::bytes(self.0.as_ptr()) }
    }
}

impl Borrow<[u8]> for Storage {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// An immutable string type which only stores a single copy of each string allocated. Internally
/// represented as a shared pointer to the backing allocation. Occupies a single pointer width.
///
/// # Small strings
///
/// Very short strings are stored inline in the pointer with bit-tagging, so no allocations are
/// performed.
///
/// # Performance
///
/// It's slower to construct than a regular `String` but trades that for reduced standing memory
/// usage by deduplicating strings. `PartialEq` and `Hash` are implemented on the underlying pointer
/// value rather than the pointed-to data for faster equality comparisons and indexing, which is
/// sound by virtue of the type guaranteeing that only one `FlyStr` pointer value will exist at
/// any time for a given string's contents.
///
/// As with any performance optimization, you should only use this type if you can measure the
/// benefit it provides to your program. Pay careful attention to creating `FlyStr`s in hot paths
/// as it may regress runtime performance.
///
/// # Allocation lifecycle
///
/// Intended for long-running system services with user-provided values, `FlyStr`s are removed from
/// the global cache when the last reference to them is dropped. While this incurs some overhead
/// it is important to prevent the value cache from becoming a denial-of-service vector.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct FlyStr(RawRepr);

#[cfg(feature = "json_schema")]
impl schemars::JsonSchema for FlyStr {
    fn schema_name() -> String {
        str::schema_name()
    }

    fn json_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        str::json_schema(generator)
    }

    fn is_referenceable() -> bool {
        false
    }
}

static_assertions::assert_eq_size!(FlyStr, usize);

// `Borrow` requires that:
//
// > Eq, Ord and Hash must be equivalent for borrowed and owned values: x.borrow() == y.borrow()
// > should give the same result as x == y.
//
// The current implementation of `FlyStr` cannot satisfy `Borrow<str>` because the O(1) hashing
// feature means that `s.hash() != s.borrow().hash()`.
static_assertions::assert_not_impl_any!(FlyStr: Borrow<str>);

impl FlyStr {
    /// Create a `FlyStr`, allocating it in the cache if the value is not already cached.
    ///
    /// # Performance
    ///
    /// Creating an instance of this type for strings longer than the maximum inline size requires
    /// accessing the global cache of strings, which involves taking a lock. When multiple threads
    /// are allocating lots of strings there may be contention. Each string allocated is hashed for
    /// lookup in the cache.
    #[inline]
    pub fn new(s: impl AsRef<str> + Into<String>) -> Self {
        Self(RawRepr::new_str(s))
    }

    /// Returns the underlying string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: Every FlyStr is constructed from valid UTF-8 bytes.
        unsafe { std::str::from_utf8_unchecked(self.0.as_bytes()) }
    }
}

impl Default for FlyStr {
    #[inline]
    fn default() -> Self {
        Self::new("")
    }
}

impl From<&'_ str> for FlyStr {
    #[inline]
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<&'_ String> for FlyStr {
    #[inline]
    fn from(s: &String) -> Self {
        Self::new(&**s)
    }
}

impl From<String> for FlyStr {
    #[inline]
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<Box<str>> for FlyStr {
    #[inline]
    fn from(s: Box<str>) -> Self {
        Self::new(s)
    }
}

impl From<&Box<str>> for FlyStr {
    #[inline]
    fn from(s: &Box<str>) -> Self {
        Self::new(&**s)
    }
}

impl TryFrom<FlyByteStr> for FlyStr {
    type Error = std::str::Utf8Error;

    #[inline]
    fn try_from(b: FlyByteStr) -> Result<FlyStr, Self::Error> {
        // The internals of both FlyStr and FlyByteStr are the same, but it's only sound to return
        // a FlyStr if the RawRepr contains/points to valid UTF-8.
        std::str::from_utf8(b.as_bytes())?;
        Ok(FlyStr(b.0))
    }
}

impl From<FlyStr> for String {
    #[inline]
    fn from(s: FlyStr) -> String {
        s.as_str().to_owned()
    }
}

impl From<&'_ FlyStr> for String {
    #[inline]
    fn from(s: &FlyStr) -> String {
        s.as_str().to_owned()
    }
}

impl Deref for FlyStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for FlyStr {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialOrd for FlyStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FlyStr {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialEq<str> for FlyStr {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&'_ str> for FlyStr {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for FlyStr {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        self.as_str() == &**other
    }
}

impl PartialEq<FlyByteStr> for FlyStr {
    #[inline]
    fn eq(&self, other: &FlyByteStr) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<&'_ FlyByteStr> for FlyStr {
    #[inline]
    fn eq(&self, other: &&FlyByteStr) -> bool {
        self.0 == other.0
    }
}

impl PartialOrd<str> for FlyStr {
    #[inline]
    fn partial_cmp(&self, other: &str) -> Option<std::cmp::Ordering> {
        self.as_str().partial_cmp(other)
    }
}

impl PartialOrd<&str> for FlyStr {
    #[inline]
    fn partial_cmp(&self, other: &&str) -> Option<std::cmp::Ordering> {
        self.as_str().partial_cmp(*other)
    }
}

impl PartialOrd<FlyByteStr> for FlyStr {
    #[inline]
    fn partial_cmp(&self, other: &FlyByteStr) -> Option<std::cmp::Ordering> {
        BStr::new(self.as_str()).partial_cmp(other.as_bstr())
    }
}

impl PartialOrd<&'_ FlyByteStr> for FlyStr {
    #[inline]
    fn partial_cmp(&self, other: &&FlyByteStr) -> Option<std::cmp::Ordering> {
        BStr::new(self.as_str()).partial_cmp(other.as_bstr())
    }
}

impl Debug for FlyStr {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Debug::fmt(self.as_str(), f)
    }
}

impl Display for FlyStr {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(self.as_str(), f)
    }
}

#[cfg(feature = "serde")]
impl Serialize for FlyStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'d> Deserialize<'d> for FlyStr {
    fn deserialize<D: Deserializer<'d>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(FlyStrVisitor)
    }
}

#[cfg(feature = "serde")]
struct FlyStrVisitor;

#[cfg(feature = "serde")]
impl Visitor<'_> for FlyStrVisitor {
    type Value = FlyStr;
    fn expecting(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("a string")
    }

    fn visit_borrowed_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into())
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into())
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into())
    }
}

macro_rules! new_raw_repr {
    ($borrowed_bytes:expr) => {
        if $borrowed_bytes.len() <= MAX_INLINE_SIZE {
            RawRepr::new_inline($borrowed_bytes)
        } else {
            let mut cache = CACHE.lock().unwrap();
            if let Some(existing) = cache.get($borrowed_bytes) {
                RawRepr::from_storage(existing)
            } else {
                RawRepr::new_for_storage(&mut cache, $borrowed_bytes)
            }
        }
    };
}

/// An immutable bytestring type which only stores a single copy of each string allocated.
/// Internally represented as a shared pointer to the backing allocation. Occupies a single pointer width.
///
/// # Small strings
///
/// Very short strings are stored inline in the pointer with bit-tagging, so no allocations are
/// performed.
///
/// # Performance
///
/// It's slower to construct than a regular `BString` but trades that for reduced standing memory
/// usage by deduplicating strings. `PartialEq` and `Hash` are implemented on the underlying pointer
/// value rather than the pointed-to data for faster equality comparisons and indexing, which is
/// sound by virtue of the type guaranteeing that only one `FlyByteStr` pointer value will exist at
/// any time for a given string's contents.
///
/// As with any performance optimization, you should only use this type if you can measure the
/// benefit it provides to your program. Pay careful attention to creating `FlyByteStr`s in hot
/// paths as it may regress runtime performance.
///
/// # Allocation lifecycle
///
/// Intended for long-running system services with user-provided values, `FlyByteStr`s are removed
/// from the global cache when the last reference to them is dropped. While this incurs some
/// overhead it is important to prevent the value cache from becoming a denial-of-service vector.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct FlyByteStr(RawRepr);

static_assertions::assert_eq_size!(FlyByteStr, usize);

// `Borrow` requires that:
//
// > Eq, Ord and Hash must be equivalent for borrowed and owned values: x.borrow() == y.borrow()
// > should give the same result as x == y.
//
// The current implementation of `FlyByteStr` cannot satisfy `Borrow<str>` because the O(1) hashing
// feature means that `s.hash() != s.borrow().hash()`.
static_assertions::assert_not_impl_any!(FlyByteStr: Borrow<str>);

impl FlyByteStr {
    /// Create a `FlyByteStr`, allocating it in the cache if the value is not already cached.
    ///
    /// # Performance
    ///
    /// Creating an instance of this type for strings longer than the maximum inline size requires
    /// accessing the global cache of strings, which involves taking a lock. When multiple threads
    /// are allocating lots of strings there may be contention. Each string allocated is hashed for
    /// lookup in the cache.
    pub fn new(s: impl AsRef<[u8]> + Into<Vec<u8>>) -> Self {
        Self(RawRepr::new(s))
    }

    /// Returns the underlying bytestring slice.
    #[inline]
    pub fn as_bstr(&self) -> &BStr {
        BStr::new(self.0.as_bytes())
    }

    /// Returns the underlying byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Default for FlyByteStr {
    #[inline]
    fn default() -> Self {
        Self::new(b"")
    }
}

impl From<&'_ [u8]> for FlyByteStr {
    #[inline]
    fn from(s: &[u8]) -> Self {
        Self::new(s)
    }
}

impl<const N: usize> From<[u8; N]> for FlyByteStr {
    #[inline]
    fn from(s: [u8; N]) -> Self {
        Self::new(s)
    }
}

impl From<&'_ BStr> for FlyByteStr {
    #[inline]
    fn from(s: &BStr) -> Self {
        let bytes: &[u8] = s.as_ref();
        Self(new_raw_repr!(bytes))
    }
}

impl From<&'_ str> for FlyByteStr {
    #[inline]
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<&'_ Vec<u8>> for FlyByteStr {
    #[inline]
    fn from(s: &Vec<u8>) -> Self {
        Self(new_raw_repr!(&s[..]))
    }
}

impl From<&'_ String> for FlyByteStr {
    #[inline]
    fn from(s: &String) -> Self {
        Self::new(&**s)
    }
}

impl From<Vec<u8>> for FlyByteStr {
    #[inline]
    fn from(s: Vec<u8>) -> Self {
        Self::new(s)
    }
}

impl From<String> for FlyByteStr {
    #[inline]
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<BString> for FlyByteStr {
    #[inline]
    fn from(s: BString) -> Self {
        Self::new(s)
    }
}

impl From<Box<[u8]>> for FlyByteStr {
    #[inline]
    fn from(s: Box<[u8]>) -> Self {
        Self::new(s)
    }
}

impl From<Box<str>> for FlyByteStr {
    #[inline]
    fn from(s: Box<str>) -> Self {
        Self(new_raw_repr!(s.as_bytes()))
    }
}

impl From<&'_ Box<[u8]>> for FlyByteStr {
    #[inline]
    fn from(s: &'_ Box<[u8]>) -> Self {
        Self(new_raw_repr!(&**s))
    }
}

impl From<&Box<str>> for FlyByteStr {
    #[inline]
    fn from(s: &Box<str>) -> Self {
        Self::new(&**s)
    }
}

impl From<FlyByteStr> for BString {
    #[inline]
    fn from(s: FlyByteStr) -> BString {
        s.as_bstr().to_owned()
    }
}

impl From<FlyByteStr> for Vec<u8> {
    #[inline]
    fn from(s: FlyByteStr) -> Vec<u8> {
        s.as_bytes().to_owned()
    }
}

impl From<FlyStr> for FlyByteStr {
    #[inline]
    fn from(s: FlyStr) -> FlyByteStr {
        Self(s.0)
    }
}

impl TryInto<String> for FlyByteStr {
    type Error = std::string::FromUtf8Error;

    #[inline]
    fn try_into(self) -> Result<String, Self::Error> {
        String::from_utf8(self.into())
    }
}

impl Deref for FlyByteStr {
    type Target = BStr;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_bstr()
    }
}

impl AsRef<BStr> for FlyByteStr {
    #[inline]
    fn as_ref(&self) -> &BStr {
        self.as_bstr()
    }
}

impl PartialOrd for FlyByteStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FlyByteStr {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bstr().cmp(other.as_bstr())
    }
}

impl PartialEq<[u8]> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl PartialEq<BStr> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &BStr) -> bool {
        self.as_bytes() == other
    }
}

impl PartialEq<str> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&'_ [u8]> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_bytes() == *other
    }
}

impl PartialEq<&'_ BStr> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &&BStr) -> bool {
        self.as_bstr() == *other
    }
}

impl PartialEq<&'_ str> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<String> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<FlyStr> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &FlyStr) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<&'_ FlyStr> for FlyByteStr {
    #[inline]
    fn eq(&self, other: &&FlyStr) -> bool {
        self.0 == other.0
    }
}

impl PartialOrd<str> for FlyByteStr {
    #[inline]
    fn partial_cmp(&self, other: &str) -> Option<std::cmp::Ordering> {
        self.as_bstr().partial_cmp(other)
    }
}

impl PartialOrd<&str> for FlyByteStr {
    #[inline]
    fn partial_cmp(&self, other: &&str) -> Option<std::cmp::Ordering> {
        self.as_bstr().partial_cmp(other)
    }
}

impl PartialOrd<FlyStr> for FlyByteStr {
    #[inline]
    fn partial_cmp(&self, other: &FlyStr) -> Option<std::cmp::Ordering> {
        self.as_bstr().partial_cmp(other.as_str())
    }
}

impl Debug for FlyByteStr {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Debug::fmt(self.as_bstr(), f)
    }
}

impl Display for FlyByteStr {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(self.as_bstr(), f)
    }
}

#[cfg(feature = "serde")]
impl Serialize for FlyByteStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.as_bytes())
    }
}

#[cfg(feature = "serde")]
impl<'d> Deserialize<'d> for FlyByteStr {
    fn deserialize<D: Deserializer<'d>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_bytes(FlyByteStrVisitor)
    }
}

#[cfg(feature = "serde")]
struct FlyByteStrVisitor;

#[cfg(feature = "serde")]
impl<'de> Visitor<'de> for FlyByteStrVisitor {
    type Value = FlyByteStr;
    fn expecting(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("a string, a bytestring, or a sequence of bytes")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(FlyByteStr::from(v))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(FlyByteStr::from(v))
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(FlyByteStr::from(v))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut bytes = vec![];
        while let Some(b) = seq.next_element::<u8>()? {
            bytes.push(b);
        }
        Ok(FlyByteStr::from(bytes))
    }
}

#[repr(C)] // Guarantee predictable field ordering.
union RawRepr {
    /// Strings longer than MAX_INLINE_SIZE are allocated using `raw::Payload`, which have a thin
    /// pointer representation. This means that `heap` variants of `Storage` will always have the
    /// pointer contents aligned and this variant will never have its least significant bit set.
    ///
    /// We store a `NonNull` so we can have guaranteed pointer layout.
    heap: NonNull<raw::Payload>,

    /// Strings shorter than or equal in length to MAX_INLINE_SIZE are stored in this union variant.
    /// The first byte is reserved for the size of the inline string, and the remaining bytes are
    /// used for the string itself. The first byte has its least significant bit set to 1 to
    /// distinguish inline strings from heap-allocated ones, and the size is stored in the remaining
    /// 7 bits.
    inline: InlineRepr,
}

// The inline variant should not cause us to occupy more space than the heap variant alone.
static_assertions::assert_eq_size!(NonNull<raw::Payload>, RawRepr);

// Alignment of the payload pointers must be >1 in order to have space for the mask bit at the
// bottom.
static_assertions::const_assert!(std::mem::align_of::<raw::Payload>() > 1);

// The short string optimization makes little-endian layout assumptions with the first byte being
// the least significant.
static_assertions::assert_type_eq_all!(byteorder::NativeEndian, byteorder::LittleEndian);

/// An enum with an actual discriminant that allows us to limit the reach of unsafe code in the
/// implementation without affecting the stored size of `RawRepr`.
enum SafeRepr<'a> {
    Heap(NonNull<raw::Payload>),
    Inline(&'a InlineRepr),
}

// SAFETY: FlyStr can be dropped from any thread.
unsafe impl Send for RawRepr {}
// SAFETY: FlyStr has an immutable public API.
unsafe impl Sync for RawRepr {}

impl RawRepr {
    fn new_str(s: impl AsRef<str>) -> Self {
        let borrowed = s.as_ref();
        new_raw_repr!(borrowed.as_bytes())
    }

    fn new(s: impl AsRef<[u8]>) -> Self {
        let borrowed = s.as_ref();
        new_raw_repr!(borrowed)
    }

    #[inline]
    fn new_inline(s: &[u8]) -> Self {
        assert!(s.len() <= MAX_INLINE_SIZE);
        let new = Self {
            inline: InlineRepr::new(s),
        };
        assert!(
            new.is_inline(),
            "least significant bit must be 1 for inline strings"
        );
        new
    }

    #[inline]
    fn from_storage(storage: &Storage) -> Self {
        if storage.inc_ref() == 0 {
            // Another thread is trying to lock the cache and free this string. They already
            // released their refcount, so give it back to them. This will prevent this thread
            // and other threads from attempting to free the string if they drop the refcount back
            // down.
            storage.inc_ref();
        }
        Self { heap: storage.0 }
    }

    #[inline]
    fn new_for_storage(cache: &mut AHashSet<Storage>, bytes: &[u8]) -> Self {
        assert!(bytes.len() > MAX_INLINE_SIZE);
        // `Payload::alloc` starts the refcount at 1.
        let new_storage = raw::Payload::alloc(bytes);

        let for_cache = Storage(new_storage);
        let new = Self { heap: new_storage };
        assert!(
            !new.is_inline(),
            "least significant bit must be 0 for heap strings"
        );
        cache.insert(for_cache);
        new
    }

    #[inline]
    fn is_inline(&self) -> bool {
        // SAFETY: it is always OK to interpret a pointer as byte array as long as we don't expect
        // to retain provenance.
        (unsafe { self.inline.masked_len } & 1) == 1
    }

    #[inline]
    fn project(&self) -> SafeRepr<'_> {
        if self.is_inline() {
            // SAFETY: Just checked that this is the inline variant.
            SafeRepr::Inline(unsafe { &self.inline })
        } else {
            // SAFETY: Just checked that this is the heap variant.
            SafeRepr::Heap(unsafe { self.heap })
        }
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        match self.project() {
            // SAFETY: FlyStr owns the payload stored as a NonNull, it is live as long as `FlyStr`.
            SafeRepr::Heap(ptr) => unsafe { &*raw::Payload::bytes(ptr.as_ptr()) },
            SafeRepr::Inline(i) => i.as_bytes(),
        }
    }
}

impl PartialEq for RawRepr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: it is always OK to interpret a pointer as a byte array as long as we don't expect
        // to retain provenance.
        let lhs = unsafe { &self.inline };
        // SAFETY: it is always OK to interpret a pointer as a byte array as long as we don't expect
        // to retain provenance.
        let rhs = unsafe { &other.inline };
        lhs.eq(rhs)
    }
}
impl Eq for RawRepr {}

impl Hash for RawRepr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        // SAFETY: it is always OK to interpret a pointer as a byte array as long as we don't expect
        // to retain provenance.
        let this = unsafe { &self.inline };
        this.hash(h);
    }
}

impl Clone for RawRepr {
    fn clone(&self) -> Self {
        match self.project() {
            SafeRepr::Heap(ptr) => {
                // SAFETY: We own this payload, we know it's live because we are.
                unsafe {
                    raw::Payload::inc_ref(ptr.as_ptr());
                }
                Self { heap: ptr }
            }
            SafeRepr::Inline(&inline) => Self { inline },
        }
    }
}

impl Drop for RawRepr {
    fn drop(&mut self) {
        if !self.is_inline() {
            // SAFETY: We checked above that this is the heap repr.
            let heap = unsafe { self.heap };

            // Decrementing the refcount before locking the cache causes the following failure mode:
            //
            // 1. We drop the refcount to 0.
            // 2. Another thread finds the string we're about to drop and increments the refcount
            //    back up to 1.
            // 3. That thread drops its refcount, and also sees the refcount drop to 0.
            // 4. That thread locks the cache, removes the value, and drops the payload. This leaves
            //    us with a dangling pointer to the dropped payload.
            // 5. We lock the cache and go to look up our value in the cache. Our payload pointer is
            //    dangling now, but we don't know that. If we try to read through our dangling
            //    pointer, we cause UB.
            //
            // To account for this failure mode and still optimistically drop our refcount, we
            // modify the procedure slightly:
            //
            // 1. We drop the refcount to 0.
            // 2. Another thread finds the string we're about to drop and increments the refcount
            //    back up to 1. It notices that the refcount incremented from 0 to 1, and so knows
            //    that our thread will try to drop it. While still holding the cache lock, that
            //    thread increments the refcount again from 1 to 2. This "gives back" the refcount
            //    to our thread.
            // 3. That thread drops its refcount, and sees the refcount drop to 1. It won't try to
            //    drop the payload this time.
            // 4. We lock the cache, and decrement the refcount a second time. If it decremented
            //    from 0 or 1, then we know that no other threads are currently holding references
            //    to it and we can safely drop it ourselves.

            // SAFETY: The payload is live.
            let prev_refcount = unsafe { raw::Payload::dec_ref(heap.as_ptr()) };

            // If we held the final refcount outside of the cache, try to remove the string.
            if prev_refcount == 1 {
                let mut cache = CACHE.lock().unwrap();

                let current_refcount = unsafe { raw::Payload::dec_ref(heap.as_ptr()) };
                if current_refcount <= 1 {
                    // If the refcount was still 0 after acquiring the cache lock, no other thread
                    // looked up this payload between optimistically decrementing the refcount and
                    // now. If the refcount was 1, then another thread did, but dropped its refcount
                    // before we got the cache lock. Either way, we can safely remove the string
                    // from the cache and free it.

                    let bytes = unsafe { &*raw::Payload::bytes(heap.as_ptr()) };
                    assert!(
                        cache.remove(bytes),
                        "cache did not contain bytes, but this thread didn't remove them yet",
                    );

                    // Get out of the critical section as soon as possible
                    drop(cache);

                    // SAFETY: The payload is live.
                    unsafe { raw::Payload::dealloc(heap.as_ptr()) };
                } else {
                    // Another thread looked up this payload, made a reference to it, and gave our
                    // refcount back to us for a minmium refcount of 2. We re-removed our refcount,
                    // giving a minimum of one. This means it's no longer our responsibility to
                    // deallocate the string, so we ended up needlessly locking the cache.
                }
            }
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq)]
#[repr(C)] // Preserve field ordering.
struct InlineRepr {
    /// The first byte, which corresponds to the LSB of a pointer in the other variant.
    ///
    /// When the first bit is `1` the rest of this byte stores the length of the inline string.
    masked_len: u8,
    /// Inline string contents.
    contents: [u8; MAX_INLINE_SIZE],
}

/// We can store small strings up to 1 byte less than the size of the pointer to the heap-allocated
/// string.
const MAX_INLINE_SIZE: usize = std::mem::size_of::<NonNull<raw::Payload>>() - 1;

// Guard rail to make sure we never end up with an incorrect inline size encoding. Ensure that
// MAX_INLINE_SIZE is always smaller than the maximum size we can represent in a byte with the LSB
// reserved.
static_assertions::const_assert!((u8::MAX >> 1) as usize >= MAX_INLINE_SIZE);

impl InlineRepr {
    #[inline]
    fn new(s: &[u8]) -> Self {
        assert!(s.len() <= MAX_INLINE_SIZE);

        // Set the first byte to the length of the inline string with LSB masked to 1.
        let masked_len = ((s.len() as u8) << 1) | 1;

        let mut contents = [0u8; MAX_INLINE_SIZE];
        contents[..s.len()].copy_from_slice(s);

        Self {
            masked_len,
            contents,
        }
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        let len = self.masked_len >> 1;
        &self.contents[..len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{const_assert, const_assert_eq};
    use std::collections::BTreeSet;
    use test_case::test_case;

    // These tests all manipulate the process-global cache in the parent module. On target devices
    // we run each test case in its own process, so the test cases can't pollute each other. On
    // host, we run tests with a process for each suite (which is the Rust upstream default), and
    // we need to manually isolate the tests from each other.
    #[cfg(not(target_os = "fuchsia"))]
    use serial_test::serial;

    fn reset_global_cache() {
        // We still want subsequent tests to be able to run if one in the same process panics.
        match CACHE.lock() {
            Ok(mut c) => *c = AHashSet::new(),
            Err(e) => *e.into_inner() = AHashSet::new(),
        }
    }
    fn num_strings_in_global_cache() -> usize {
        CACHE.lock().unwrap().len()
    }

    impl RawRepr {
        fn refcount(&self) -> Option<usize> {
            match self.project() {
                SafeRepr::Heap(ptr) => {
                    // SAFETY: The payload is live as long as the repr is live.
                    let count = unsafe { raw::Payload::refcount(ptr.as_ptr()) };
                    Some(count)
                }
                SafeRepr::Inline(_) => None,
            }
        }
    }

    const SHORT_STRING: &str = "hello";
    const_assert!(SHORT_STRING.len() < MAX_INLINE_SIZE);

    const MAX_LEN_SHORT_STRING: &str = "hello!!";
    const_assert_eq!(MAX_LEN_SHORT_STRING.len(), MAX_INLINE_SIZE);

    const MIN_LEN_LONG_STRING: &str = "hello!!!";
    const_assert_eq!(MIN_LEN_LONG_STRING.len(), MAX_INLINE_SIZE + 1);

    const LONG_STRING: &str = "hello, world!!!!!!!!!!!!!!!!!!!!";
    const_assert!(LONG_STRING.len() > MAX_INLINE_SIZE);

    const SHORT_NON_UTF8: &[u8] = b"\xF0\x28\x8C\x28";
    const_assert!(SHORT_NON_UTF8.len() < MAX_INLINE_SIZE);

    const LONG_NON_UTF8: &[u8] = b"\xF0\x28\x8C\x28\xF0\x28\x8C\x28";
    const_assert!(LONG_NON_UTF8.len() > MAX_INLINE_SIZE);

    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn string_formatting_is_equivalent_to_str(original: &str) {
        reset_global_cache();

        let cached = FlyStr::new(original);
        assert_eq!(format!("{original}"), format!("{cached}"));
        assert_eq!(format!("{original:?}"), format!("{cached:?}"));

        let cached = FlyByteStr::new(original);
        assert_eq!(format!("{original}"), format!("{cached}"));
        assert_eq!(format!("{original:?}"), format!("{cached:?}"));
    }

    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn string_equality_works(contents: &str) {
        reset_global_cache();

        let cached = FlyStr::new(contents);
        let bytes_cached = FlyByteStr::new(contents);
        assert_eq!(cached, cached.clone(), "must be equal to itself");
        assert_eq!(cached, contents, "must be equal to the original");
        assert_eq!(
            cached,
            contents.to_owned(),
            "must be equal to an owned copy of the original"
        );
        assert_eq!(cached, bytes_cached);

        // test inequality too
        assert_ne!(cached, "goodbye");
        assert_ne!(bytes_cached, "goodbye");
    }

    #[test_case("", SHORT_STRING ; "empty and short string")]
    #[test_case(SHORT_STRING, MAX_LEN_SHORT_STRING ; "two short strings")]
    #[test_case(MAX_LEN_SHORT_STRING, MIN_LEN_LONG_STRING ; "short and long strings")]
    #[test_case(MIN_LEN_LONG_STRING, LONG_STRING ; "barely long and long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn string_comparison_works(lesser_contents: &str, greater_contents: &str) {
        reset_global_cache();

        let lesser = FlyStr::new(lesser_contents);
        let lesser_bytes = FlyByteStr::from(lesser_contents);
        let greater = FlyStr::new(greater_contents);
        let greater_bytes = FlyByteStr::from(greater_contents);

        // lesser as method receiver
        assert!(lesser < greater);
        assert!(lesser < greater_bytes);
        assert!(lesser_bytes < greater);
        assert!(lesser_bytes < greater_bytes);
        assert!(lesser <= greater);
        assert!(lesser <= greater_bytes);
        assert!(lesser_bytes <= greater);
        assert!(lesser_bytes <= greater_bytes);

        // greater as method receiver
        assert!(greater > lesser);
        assert!(greater > lesser_bytes);
        assert!(greater_bytes > lesser);
        assert!(greater >= lesser);
        assert!(greater >= lesser_bytes);
        assert!(greater_bytes >= lesser);
        assert!(greater_bytes >= lesser_bytes);
    }

    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn no_allocations_for_short_strings(contents: &str) {
        reset_global_cache();
        assert_eq!(num_strings_in_global_cache(), 0);

        let original = FlyStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(original.0.refcount(), None);

        let cloned = original.clone();
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(cloned.0.refcount(), None);

        let deduped = FlyStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(deduped.0.refcount(), None);
    }

    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn no_allocations_for_short_bytestrings(contents: &str) {
        reset_global_cache();
        assert_eq!(num_strings_in_global_cache(), 0);

        let original = FlyByteStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(original.0.refcount(), None);

        let cloned = original.clone();
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(cloned.0.refcount(), None);

        let deduped = FlyByteStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 0);
        assert_eq!(deduped.0.refcount(), None);
    }

    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn only_one_copy_allocated_for_long_strings(contents: &str) {
        reset_global_cache();

        assert_eq!(num_strings_in_global_cache(), 0);

        let original = FlyStr::new(contents);
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "only one string allocated"
        );
        assert_eq!(original.0.refcount(), Some(1), "one copy on stack");

        let cloned = original.clone();
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "cloning just incremented refcount"
        );
        assert_eq!(cloned.0.refcount(), Some(2), "two copies on stack");

        let deduped = FlyStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 1, "new string was deduped");
        assert_eq!(deduped.0.refcount(), Some(3), "three copies on stack");
    }

    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn only_one_copy_allocated_for_long_bytestrings(contents: &str) {
        reset_global_cache();

        assert_eq!(num_strings_in_global_cache(), 0);

        let original = FlyByteStr::new(contents);
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "only one string allocated"
        );
        assert_eq!(original.0.refcount(), Some(1), "one copy on stack");

        let cloned = original.clone();
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "cloning just incremented refcount"
        );
        assert_eq!(cloned.0.refcount(), Some(2), "two copies on stack");

        let deduped = FlyByteStr::new(contents);
        assert_eq!(num_strings_in_global_cache(), 1, "new string was deduped");
        assert_eq!(deduped.0.refcount(), Some(3), "three copies on stack");
    }

    #[test]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn utf8_and_bytestrings_share_the_cache() {
        reset_global_cache();

        assert_eq!(num_strings_in_global_cache(), 0, "cache is empty");

        let _utf8 = FlyStr::from(MIN_LEN_LONG_STRING);
        assert_eq!(num_strings_in_global_cache(), 1, "string was allocated");

        let _bytes = FlyByteStr::from(MIN_LEN_LONG_STRING);
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "bytestring was pulled from cache"
        );
    }

    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn cached_strings_dropped_when_refs_dropped(contents: &str) {
        reset_global_cache();

        let alloced = FlyStr::new(contents);
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "only one string allocated"
        );
        drop(alloced);
        assert_eq!(num_strings_in_global_cache(), 0, "last reference dropped");
    }

    #[test_case(MIN_LEN_LONG_STRING ; "barely long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn cached_bytestrings_dropped_when_refs_dropped(contents: &str) {
        reset_global_cache();

        let alloced = FlyByteStr::new(contents);
        assert_eq!(
            num_strings_in_global_cache(),
            1,
            "only one string allocated"
        );
        drop(alloced);
        assert_eq!(num_strings_in_global_cache(), 0, "last reference dropped");
    }

    #[test_case("", SHORT_STRING ; "empty and short string")]
    #[test_case(SHORT_STRING, MAX_LEN_SHORT_STRING ; "two short strings")]
    #[test_case(SHORT_STRING, LONG_STRING ; "short and long strings")]
    #[test_case(LONG_STRING, MAX_LEN_SHORT_STRING ; "long and max-len-short strings")]
    #[test_case(MIN_LEN_LONG_STRING, LONG_STRING ; "barely long and long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn equality_and_hashing_with_pointer_value_works_correctly(first: &str, second: &str) {
        reset_global_cache();

        let first = FlyStr::new(first);
        let second = FlyStr::new(second);

        let mut set = AHashSet::new();
        set.insert(first.clone());
        assert!(set.contains(&first));
        assert!(!set.contains(&second));

        // re-insert the same cmstring
        set.insert(first);
        assert_eq!(
            set.len(),
            1,
            "set did not grow because the same string was inserted as before"
        );

        set.insert(second.clone());
        assert_eq!(
            set.len(),
            2,
            "inserting a different string must mutate the set"
        );
        assert!(set.contains(&second));

        // re-insert the second cmstring
        set.insert(second);
        assert_eq!(set.len(), 2);
    }

    #[test_case("", SHORT_STRING ; "empty and short string")]
    #[test_case(SHORT_STRING, MAX_LEN_SHORT_STRING ; "two short strings")]
    #[test_case(SHORT_STRING, LONG_STRING ; "short and long strings")]
    #[test_case(LONG_STRING, MAX_LEN_SHORT_STRING ; "long and max-len-short strings")]
    #[test_case(MIN_LEN_LONG_STRING, LONG_STRING ; "barely long and long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn byte_equality_and_hashing_with_pointer_value_works_correctly(first: &str, second: &str) {
        reset_global_cache();

        let first = FlyByteStr::new(first);
        let second = FlyByteStr::new(second);

        let mut set = AHashSet::new();
        set.insert(first.clone());
        assert!(set.contains(&first));
        assert!(!set.contains(&second));

        // re-insert the same string
        set.insert(first);
        assert_eq!(
            set.len(),
            1,
            "set did not grow because the same string was inserted as before"
        );

        set.insert(second.clone());
        assert_eq!(
            set.len(),
            2,
            "inserting a different string must mutate the set"
        );
        assert!(set.contains(&second));

        // re-insert the second string
        set.insert(second);
        assert_eq!(set.len(), 2);
    }

    #[test_case("", SHORT_STRING ; "empty and short string")]
    #[test_case(SHORT_STRING, MAX_LEN_SHORT_STRING ; "two short strings")]
    #[test_case(SHORT_STRING, LONG_STRING ; "short and long strings")]
    #[test_case(LONG_STRING, MAX_LEN_SHORT_STRING ; "long and max-len-short strings")]
    #[test_case(MIN_LEN_LONG_STRING, LONG_STRING ; "barely long and long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn comparison_for_btree_storage_works(first: &str, second: &str) {
        reset_global_cache();

        let first = FlyStr::new(first);
        let second = FlyStr::new(second);

        let mut set = BTreeSet::new();
        set.insert(first.clone());
        assert!(set.contains(&first));
        assert!(!set.contains(&second));

        // re-insert the same cmstring
        set.insert(first);
        assert_eq!(
            set.len(),
            1,
            "set did not grow because the same string was inserted as before"
        );

        set.insert(second.clone());
        assert_eq!(
            set.len(),
            2,
            "inserting a different string must mutate the set"
        );
        assert!(set.contains(&second));

        // re-insert the second cmstring
        set.insert(second);
        assert_eq!(set.len(), 2);
    }

    #[test_case("", SHORT_STRING ; "empty and short string")]
    #[test_case(SHORT_STRING, MAX_LEN_SHORT_STRING ; "two short strings")]
    #[test_case(SHORT_STRING, LONG_STRING ; "short and long strings")]
    #[test_case(LONG_STRING, MAX_LEN_SHORT_STRING ; "long and max-len-short strings")]
    #[test_case(MIN_LEN_LONG_STRING, LONG_STRING ; "barely long and long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn byte_comparison_for_btree_storage_works(first: &str, second: &str) {
        reset_global_cache();

        let first = FlyByteStr::new(first);
        let second = FlyByteStr::new(second);

        let mut set = BTreeSet::new();
        set.insert(first.clone());
        assert!(set.contains(&first));
        assert!(!set.contains(&second));

        // re-insert the same string
        set.insert(first);
        assert_eq!(
            set.len(),
            1,
            "set did not grow because the same string was inserted as before"
        );

        set.insert(second.clone());
        assert_eq!(
            set.len(),
            2,
            "inserting a different string must mutate the set"
        );
        assert!(set.contains(&second));

        // re-insert the second string
        set.insert(second);
        assert_eq!(set.len(), 2);
    }

    #[cfg(feature = "serde")]
    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[test_case(MIN_LEN_LONG_STRING ; "min len long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn serde_works(contents: &str) {
        reset_global_cache();

        let s = FlyStr::new(contents);
        let as_json = serde_json::to_string(&s).unwrap();
        assert_eq!(as_json, format!("\"{contents}\""));
        assert_eq!(s, serde_json::from_str::<FlyStr>(&as_json).unwrap());
    }

    #[cfg(feature = "serde")]
    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[test_case(MIN_LEN_LONG_STRING ; "min len long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn serde_works_bytestring(contents: &str) {
        reset_global_cache();

        let s = FlyByteStr::new(contents);
        let as_json = serde_json::to_string(&s).unwrap();
        assert_eq!(s, serde_json::from_str::<FlyByteStr>(&as_json).unwrap());
    }

    #[test_case(SHORT_NON_UTF8 ; "short non-utf8 bytestring")]
    #[test_case(LONG_NON_UTF8 ; "long non-utf8 bytestring")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn non_utf8_works(contents: &[u8]) {
        reset_global_cache();

        let res: Result<FlyStr, _> = FlyByteStr::from(contents).try_into();
        res.unwrap_err();
    }

    #[test_case("" ; "empty string")]
    #[test_case(SHORT_STRING ; "short strings")]
    #[test_case(MAX_LEN_SHORT_STRING ; "max len short strings")]
    #[test_case(MIN_LEN_LONG_STRING ; "min len long strings")]
    #[test_case(LONG_STRING ; "long strings")]
    #[cfg_attr(not(target_os = "fuchsia"), serial)]
    fn flystr_to_flybytestr_and_back(contents: &str) {
        reset_global_cache();

        let bytestr = FlyByteStr::from(contents);
        let flystr = FlyStr::try_from(bytestr.clone()).unwrap();
        assert_eq!(bytestr, flystr);
        let bytestr2 = FlyByteStr::from(flystr.clone());
        assert_eq!(bytestr, bytestr2);
    }
}
