// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stubs for tracking unimplemented code paths.
//!
//! This crate provides macros and utilities to track stubbed implementations
//! and surface them in Inspect for diagnostics.

use flyweights::FlyByteStr;
use fuchsia_inspect::{ArrayProperty, Inspector};
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::panic::Location;
use std::sync::{Arc, LazyLock};

static STUB_COUNTS: LazyLock<Mutex<HashMap<Invocation, Counts>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static CONTEXT_NAME_CALLBACK: Mutex<Option<Arc<dyn Fn() -> FlyByteStr + Send + Sync>>> =
    Mutex::new(None);

/// Tracks a stubbed implementation.
///
/// This macro records that a stub was encountered so that it may be surfaced in inspect.
/// The first time a particular stub is encountered, a log message will be emitted.
///
/// Example:
/// ```
/// track_stub!(TODO("https://fxbug.dev/12345"), "my component is not implemented");
/// ```
#[macro_export]
macro_rules! track_stub {
    (TODO($bug_url:literal), $message:expr, $flags:expr $(,)?) => {{
        $crate::__track_stub_inner(
            $crate::bug_ref!($bug_url),
            $message,
            Some($flags.into()),
            std::panic::Location::caller(),
        );
    }};
    (TODO($bug_url:literal), $message:expr $(,)?) => {{
        $crate::__track_stub_inner(
            $crate::bug_ref!($bug_url),
            $message,
            None,
            std::panic::Location::caller(),
        );
    }};
}

/// Tracks a stubbed implementation with a specified log level.
///
/// This macro records that a stub was encountered so that it may be surfaced in inspect.
/// The first time a particular stub is encountered, a log message will be emitted at the
/// specified level.
///
/// Example:
/// ```
/// track_stub_log!(log::Level::Warn, TODO("https://fxbug.dev/12345"), "my component is not implemented");
/// ```
#[macro_export]
macro_rules! track_stub_log {
    ($level:expr, TODO($bug_url:literal), $message:expr, $flags:expr $(,)?) => {{
        $crate::__track_stub_inner_with_level(
            $level,
            $crate::bug_ref!($bug_url),
            $message,
            Some($flags.into()),
            std::panic::Location::caller(),
        );
    }};
    ($level:expr, TODO($bug_url:literal), $message:expr $(,)?) => {{
        $crate::__track_stub_inner_with_level(
            $level,
            $crate::bug_ref!($bug_url),
            $message,
            None,
            std::panic::Location::caller(),
        );
    }};
}

// This is the struct we'll actually store in the HashMap of
// invocations. It needs to contain an owned String for lifetime
// purposes.
#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Invocation {
    location: &'static Location<'static>,
    message: String,
    bug: BugRef,
}

// This trait allows us to look up in the invocation HashMap with
// either a borrowed message or an owned message.
trait InvocationLookup {
    fn location(&self) -> &'static Location<'static>;
    fn message(&self) -> &str;
    fn bug(&self) -> BugRef;
}

impl Hash for dyn InvocationLookup + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.location().hash(state);
        self.message().hash(state);
        self.bug().hash(state);
    }
}

impl PartialEq for dyn InvocationLookup + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.location() == other.location()
            && self.message() == other.message()
            && self.bug() == other.bug()
    }
}

impl Eq for dyn InvocationLookup + '_ {}

impl InvocationLookup for Invocation {
    fn location(&self) -> &'static Location<'static> {
        self.location
    }

    fn message(&self) -> &str {
        &self.message
    }

    fn bug(&self) -> BugRef {
        self.bug
    }
}

impl<'a> std::borrow::Borrow<dyn InvocationLookup + 'a> for Invocation {
    fn borrow(&self) -> &(dyn InvocationLookup + 'a) {
        self
    }
}

// This struct is never to be stored, but is constructed for lookup
// purposes on the invocations map. Looking up using a borrowed string
// for 'message' saves an allocation if the key is already in the map,
// which could be significant if a client is opening a stub file very
// frequently.
struct InvocationKey<'a> {
    location: &'static Location<'static>,
    message: &'a str,
    bug: BugRef,
}

impl<'a> InvocationLookup for InvocationKey<'a> {
    fn location(&self) -> &'static Location<'static> {
        self.location
    }

    fn message(&self) -> &str {
        self.message
    }

    fn bug(&self) -> BugRef {
        self.bug
    }
}

#[derive(Default)]
struct Counts {
    by_flags: HashMap<Option<u64>, u64>,
    contexts_seen: HashSet<FlyByteStr>,
}

#[doc(hidden)]
#[inline]
pub fn __track_stub_inner(
    bug: BugRef,
    message: &str,
    flags: Option<u64>,
    location: &'static Location<'static>,
) -> u64 {
    __track_stub_inner_with_level(log::Level::Debug, bug, message, flags, location)
}

#[doc(hidden)]
#[inline]
pub fn __track_stub_inner_with_level(
    level: log::Level,
    bug: BugRef,
    message: &str,
    flags: Option<u64>,
    location: &'static Location<'static>,
) -> u64 {
    let current_context = {
        let cb = CONTEXT_NAME_CALLBACK.lock().clone();
        cb.as_ref().map(|cb| cb())
    };

    let mut counts = STUB_COUNTS.lock();
    let key = InvocationKey { location, message, bug };

    if let Some(message_counts) = counts.get_mut(&key as &dyn InvocationLookup) {
        let context_count = message_counts.by_flags.entry(flags).or_default();
        if let Some(ref current_context) = current_context {
            message_counts.contexts_seen.insert(current_context.clone());
        }
        if *context_count == 0 {
            match flags {
                Some(flags) => {
                    log::log!(level, tag = "track_stub", location:%; "{bug} {message}: 0x{flags:x}");
                }
                None => {
                    log::log!(level, tag = "track_stub", location:%; "{bug} {message}");
                }
            }
        }
        *context_count += 1;
        return *context_count;
    }

    match flags {
        Some(flags) => {
            log::log!(level, tag = "track_stub", location:%; "{bug} {message}: 0x{flags:x}");
        }
        None => {
            log::log!(level, tag = "track_stub", location:%; "{bug} {message}");
        }
    }

    let mut message_counts = Counts::default();
    if let Some(current_context) = current_context {
        message_counts.contexts_seen.insert(current_context);
    }
    message_counts.by_flags.insert(flags, 1);
    counts.insert(Invocation { location, message: String::from(message), bug }, message_counts);
    1
}

/// Provide a callback to retrieve the current context name, for example the name of the current
/// Starnix process.
pub fn register_context_name_callback(cb: impl Fn() -> FlyByteStr + Send + Sync + 'static) {
    *CONTEXT_NAME_CALLBACK.lock() = Some(Arc::new(cb));
}

/// Returns a future that resolves to an `Inspector` containing stub information.
///
/// This function can be used to create a lazy node in inspect that exposes the locations
/// where stubs have been tracked.
pub fn track_stub_lazy_node_callback() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> {
    Box::pin(async {
        let inspector = Inspector::default();
        for (Invocation { location, message, bug }, context_counts) in STUB_COUNTS.lock().iter() {
            inspector.root().atomic_update(|root| {
                root.record_child(message, |message_node| {
                    message_node.record_string("file", location.file());
                    message_node.record_uint("line", location.line().into());
                    message_node.record_string("bug", bug.to_string());

                    if !context_counts.contexts_seen.is_empty() {
                        let mut contexts =
                            context_counts.contexts_seen.iter().cloned().collect::<Vec<_>>();
                        contexts.sort();
                        let contexts_prop =
                            message_node.create_string_array("contexts", contexts.len());
                        for (i, context) in contexts.iter().enumerate() {
                            contexts_prop.set(i, context.to_string());
                        }
                        message_node.record(contexts_prop);
                    }

                    // Make a copy of the map so we can mutate it while recording values.
                    let mut context_counts = context_counts.by_flags.clone();

                    if let Some(no_context_count) = context_counts.remove(&None) {
                        // If the track_stub callsite doesn't provide any context,
                        // record the count as a property on the node without an intermediate.
                        message_node.record_uint("count", no_context_count);
                    }

                    if !context_counts.is_empty() {
                        message_node.record_child("counts", |counts_node| {
                            for (context, count) in context_counts {
                                if let Some(c) = context {
                                    counts_node.record_uint(format!("0x{c:x}"), count);
                                }
                            }
                        });
                    }
                });
            });
        }
        Ok(inspector)
    })
}

/// Creates a `BugRef` from a URL literal.
///
/// This macro will cause a compilation error if the provided literal is not a valid Fuchsia bug URL.
#[macro_export]
macro_rules! bug_ref {
    ($bug_url:literal) => {{
        // Assign the value to a const to ensure we get compile-time validation of the URL.
        const __REF: $crate::BugRef = match $crate::BugRef::from_str($bug_url) {
            Some(b) => b,
            None => panic!("bug references must have the form `https://fxbug.dev/123456789`"),
        };
        __REF
    }};
}

/// Represents a reference to a Fuchsia bug.
///
/// This struct is used to ensure that stubs are tracked against a valid bug.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BugRef {
    number: u64,
}

impl BugRef {
    #[doc(hidden)] // use bug_ref!() instead
    pub const fn from_str(url: &'static str) -> Option<Self> {
        let expected_prefix = b"https://fxbug.dev/";
        let url = str::as_bytes(url);

        if url.len() < expected_prefix.len() {
            return None;
        }
        let (scheme_and_domain, number_str) = url.split_at(expected_prefix.len());
        if number_str.is_empty() {
            return None;
        }

        // The standard library doesn't seem to have a const string or slice equality function.
        {
            let mut i = 0;
            while i < scheme_and_domain.len() {
                if scheme_and_domain[i] != expected_prefix[i] {
                    return None;
                }
                i += 1;
            }
        }

        // The standard library doesn't seem to have a const base 10 string parser.
        let mut number = 0;
        {
            let mut i = 0;
            while i < number_str.len() {
                number *= 10;
                number += match number_str[i] {
                    b'0' => 0,
                    b'1' => 1,
                    b'2' => 2,
                    b'3' => 3,
                    b'4' => 4,
                    b'5' => 5,
                    b'6' => 6,
                    b'7' => 7,
                    b'8' => 8,
                    b'9' => 9,
                    _ => return None,
                };
                i += 1;
            }
        }

        if number != 0 { Some(Self { number }) } else { None }
    }
}

impl From<NonZeroU64> for BugRef {
    /// Converts a `NonZeroU64` into a `BugRef`.
    fn from(value: NonZeroU64) -> Self {
        Self { number: value.get() }
    }
}

impl Into<NonZeroU64> for BugRef {
    /// Converts a `BugRef` into a `NonZeroU64`.
    fn into(self) -> NonZeroU64 {
        NonZeroU64::new(self.number).unwrap()
    }
}

impl std::fmt::Display for BugRef {
    /// Formats the `BugRef` as a URL string.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "https://fxbug.dev/{}", self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;

    #[test]
    fn valid_url_parses() {
        assert_eq!(BugRef::from_str("https://fxbug.dev/1234567890").unwrap().number, 1234567890);
    }

    #[test]
    fn missing_prefix_fails() {
        assert_eq!(BugRef::from_str("1234567890"), None);
    }

    #[test]
    fn missing_number_fails() {
        assert_eq!(BugRef::from_str("https://fxbug.dev/"), None);
    }

    #[test]
    fn short_prefixes_fail() {
        assert_eq!(BugRef::from_str("b/1234567890"), None);
        assert_eq!(BugRef::from_str("fxb/1234567890"), None);
        assert_eq!(BugRef::from_str("fxbug.dev/1234567890"), None);
    }

    #[test]
    fn invalid_characters_fail() {
        assert_eq!(BugRef::from_str("https://fxbug.dev/123a45"), None);
    }

    #[test]
    fn zero_bug_number_fails() {
        assert_eq!(BugRef::from_str("https://fxbug.dev/0"), None);
    }

    #[fuchsia::test]
    async fn test_track_stub() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_child("stubs", track_stub_lazy_node_callback);

        let call_stub = || {
            track_stub!(TODO("https://fxbug.dev/1"), "test stub");
            std::line!() as u64 - 1
        };

        let file = std::panic::Location::caller().file();
        let line = call_stub();

        assert_data_tree!(inspector, root: {
            stubs: {
                "test stub": {
                    bug: "https://fxbug.dev/1",
                    count: 1u64,
                    file: file,
                    line: line,
                }
            }
        });

        call_stub();
        assert_data_tree!(inspector, root: {
            stubs: {
                "test stub": {
                    bug: "https://fxbug.dev/1",
                    count: 2u64,
                    file: file,
                    line: line,
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_track_stub_different_callsites() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_child("stubs", track_stub_lazy_node_callback);

        let loc1 = std::panic::Location::caller();
        track_stub!(TODO("https://fxbug.dev/1"), "stub 1");
        let loc2 = std::panic::Location::caller();
        track_stub!(TODO("https://fxbug.dev/2"), "stub 2");

        assert_data_tree!(inspector, root: {
            stubs: {
                "stub 1": {
                    bug: "https://fxbug.dev/1",
                    count: 1u64,
                    file: loc1.file(),
                    line: (loc1.line() + 1) as u64,
                },
                "stub 2": {
                    bug: "https://fxbug.dev/2",
                    count: 1u64,
                    file: loc2.file(),
                    line: (loc2.line() + 1) as u64,
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_track_stub_with_flags() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_child("stubs", track_stub_lazy_node_callback);

        let call_stub = |flags: u64| {
            track_stub!(TODO("https://fxbug.dev/3"), "stub with flags", flags);
            std::line!() - 1
        };

        let file = std::panic::Location::caller().file();
        let line = call_stub(0x1);
        call_stub(0x2);
        call_stub(0x1);

        assert_data_tree!(inspector, root: {
            stubs: {
                "stub with flags": {
                    bug: "https://fxbug.dev/3",
                    file: file,
                    line: line as u64,
                    counts: {
                        "0x1": 2u64,
                        "0x2": 1u64,
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_track_stub_with_context() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_child("stubs", track_stub_lazy_node_callback);

        let current_context = std::sync::Arc::new(Mutex::new("SHOULD NOT SHOW UP"));
        let context_clone = current_context.clone();
        register_context_name_callback(move || FlyByteStr::from(*context_clone.lock()));

        let call_stub_with_context = |context| {
            *current_context.lock() = context;
            track_stub!(TODO("https://fxbug.dev/4"), "stub with context");
        };
        let line = std::line!() as u64 - 2;

        call_stub_with_context("context1");
        call_stub_with_context("context2");

        assert_data_tree!(inspector, root: {
            stubs: {
                "stub with context": {
                    bug: "https://fxbug.dev/4",
                    count: 2u64,
                    file: std::file!(),
                    line: line,
                    contexts: vec!["context1", "context2"]
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_track_stub_reentrant_context_callback() {
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered_clone = entered.clone();
        register_context_name_callback(move || {
            if !entered_clone.swap(true, std::sync::atomic::Ordering::SeqCst) {
                track_stub!(TODO("https://fxbug.dev/9991"), "nested stub");
            }
            FlyByteStr::from("reentrant_ctx")
        });

        track_stub!(TODO("https://fxbug.dev/9992"), "outer stub");
    }
}
