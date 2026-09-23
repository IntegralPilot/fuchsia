// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declarative macros and zero-allocation matcher predicates for VMO digest categorization.

#[inline]
fn is_hex_str(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[inline]
fn is_digits_str(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[inline]
pub(crate) fn is_exact_hex(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(is_hex_str)
}

#[inline]
pub(crate) fn is_exact_digits(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(is_digits_str)
}

#[inline]
pub(crate) fn is_starts_with_digits_delim(s: &str, prefix: &str, delim: &str) -> bool {
    if let Some(rest) = s.strip_prefix(prefix) {
        if let Some(pos) = rest.find(delim) {
            return rest[..pos].bytes().all(|b| b.is_ascii_digit());
        }
    }
    false
}

/// Evaluates a DSL matcher against a candidate string `$val`.
///
/// Supported matcher forms:
/// - `exact("str")`: Start-and-end-anchored literal match (`$val == "str"`).
/// - `exact("prefix-", hex())`: Start-and-end-anchored match where `$val` begins with `"prefix-"`
///   and the remainder consists of 1 or more lowercase hex characters (`[0-9a-f]+`).
/// - `exact("prefix:", digits())`: Start-and-end-anchored match where `$val` begins with
///   `"prefix:"` and the remainder consists of 1 or more ASCII digits (`[0-9]+`).
/// - `starts_with("str")`: Open-ended prefix match (`$val.starts_with("str")`).
/// - `starts_with("prefix", digits(), ":")`: Open-ended match where `$val` begins with `"prefix"`,
///   followed by 0 or more ASCII digits, followed by `":"`, and arbitrary trailing characters.
/// - `contains("substr")`: Substring match (`$val.contains("substr")`).
/// - `or(matcher1, matcher2, ...)`: Evaluates matchers with logical OR short-circuiting.
macro_rules! eval_matcher {
    ($val:expr, exact($s:expr)) => {
        $val == $s
    };
    ($val:expr, exact($prefix:expr, hex())) => {
        $crate::macros::is_exact_hex($val, $prefix)
    };
    ($val:expr, exact($prefix:expr, digits())) => {
        $crate::macros::is_exact_digits($val, $prefix)
    };
    ($val:expr, starts_with($s:expr)) => {
        $val.starts_with($s)
    };
    ($val:expr, starts_with($prefix:expr, digits(), $delim:expr)) => {
        $crate::macros::is_starts_with_digits_delim($val, $prefix, $delim)
    };
    ($val:expr, contains($s:expr)) => {
        $val.contains($s)
    };
    ($val:expr, or($($m_ident:ident($($m_args:tt)*)),* $(,)?)) => {
        false $( || $crate::macros::eval_matcher!($val, $m_ident($($m_args)*)) )*
    };
}
pub(crate) use eval_matcher;

/// Declares VMO digest categories, their static string and `ZXName` representations, and a fast
/// matching function.
///
/// Syntax:
/// ```ignore
/// vmo_digests! {
///     (CategoryVariant, "[display-name]", <matcher>),
///     ...
/// }
/// ```
macro_rules! vmo_digests {
    ($( ($variant:ident, $str_name:expr, $($matcher:tt)+) ),* $(,)?) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        enum VmoDigestCategory {
            $( $variant, )*
        }

        impl VmoDigestCategory {
            #[inline]
            const fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $str_name, )*
                }
            }

            #[inline]
            const fn as_zxname(&self) -> &'static $crate::ZXName {
                match self {
                    $(
                        Self::$variant => {
                            const VAL: &$crate::ZXName = &$crate::ZXName::from_bytes_lossy($str_name.as_bytes());
                            VAL
                        }
                    )*
                }
            }
        }

        // We use custom string matching to be faster than using full regular expressions.
        #[inline]
        fn match_vmo_digest(name_str: &str) -> Option<VmoDigestCategory> {
            $(
                if $crate::macros::eval_matcher!(name_str, $($matcher)+) {
                    return Some(VmoDigestCategory::$variant);
                }
            )*
            None
        }
    };
}
pub(crate) use vmo_digests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZXName;

    #[test]
    fn test_is_hex_str() {
        assert!(!is_hex_str(""));
        assert!(is_hex_str("0123456789abcdef"));
        assert!(is_hex_str("0"));
        assert!(is_hex_str("f"));
        assert!(!is_hex_str("ABC"));
        assert!(!is_hex_str("123g"));
        assert!(!is_hex_str("0123 456"));
    }

    #[test]
    fn test_is_exact_hex() {
        assert!(is_exact_hex("blob-1234", "blob-"));
        assert!(is_exact_hex("blob-abcdef0123456789", "blob-"));
        assert!(!is_exact_hex("blob-", "blob-"));
        assert!(!is_exact_hex("blob-ABC", "blob-"));
        assert!(!is_exact_hex("blob-123g", "blob-"));
        assert!(!is_exact_hex("inactive-blob-123", "blob-"));
        assert!(is_exact_hex("inactive-blob-123", "inactive-blob-"));
        assert!(!is_exact_hex("blob-1234-trailing", "blob-"));
    }

    #[test]
    fn test_is_digits_str() {
        assert!(!is_digits_str(""));
        assert!(is_digits_str("0"));
        assert!(is_digits_str("1234567890"));
        assert!(!is_digits_str("123a"));
        assert!(!is_digits_str("a123"));
        assert!(!is_digits_str(" 123"));
    }

    #[test]
    fn test_is_exact_digits() {
        assert!(!is_exact_digits("restricted_state_vmo:", "restricted_state_vmo:"));
        assert!(is_exact_digits("restricted_state_vmo:0", "restricted_state_vmo:"));
        assert!(is_exact_digits("restricted_state_vmo:12345", "restricted_state_vmo:"));
        assert!(!is_exact_digits("restricted_state_vmo:abc", "restricted_state_vmo:"));
        assert!(!is_exact_digits("restricted_state_vmo:12a", "restricted_state_vmo:"));
        assert!(!is_exact_digits("restricted_state_vmo:123-trailing", "restricted_state_vmo:"));
        assert!(!is_exact_digits("other_vmo:12345", "restricted_state_vmo:"));
    }

    #[test]
    fn test_is_starts_with_digits_delim() {
        assert!(is_starts_with_digits_delim("data:", "data", ":"));
        assert!(is_starts_with_digits_delim("data0:", "data", ":"));
        assert!(is_starts_with_digits_delim("data123:foo", "data", ":"));
        assert!(is_starts_with_digits_delim("data:bar", "data", ":"));
        assert!(!is_starts_with_digits_delim("data_foo:", "data", ":"));
        assert!(!is_starts_with_digits_delim("data", "data", ":"));
        assert!(!is_starts_with_digits_delim("data123", "data", ":"));
        assert!(!is_starts_with_digits_delim("dataa:foo", "data", ":"));
        assert!(is_starts_with_digits_delim("bss:", "bss", ":"));
        assert!(is_starts_with_digits_delim("bss99:", "bss", ":"));
        assert!(is_starts_with_digits_delim("bss456:bar", "bss", ":"));
        assert!(!is_starts_with_digits_delim("bss_foo:", "bss", ":"));
        assert!(!is_starts_with_digits_delim("bss", "bss", ":"));
    }

    #[test]
    fn test_eval_matcher_macro() {
        assert!(eval_matcher!("hello", exact("hello")));
        assert!(!eval_matcher!("hello world", exact("hello")));

        assert!(eval_matcher!("blob-1234", exact("blob-", hex())));
        assert!(!eval_matcher!("blob-", exact("blob-", hex())));
        assert!(!eval_matcher!("blob-1234-trailing", exact("blob-", hex())));

        assert!(eval_matcher!(
            "restricted_state_vmo:123",
            exact("restricted_state_vmo:", digits())
        ));
        assert!(!eval_matcher!("restricted_state_vmo:", exact("restricted_state_vmo:", digits())));
        assert!(!eval_matcher!(
            "restricted_state_vmo:abc",
            exact("restricted_state_vmo:", digits())
        ));

        assert!(eval_matcher!("hello world", starts_with("hello")));
        assert!(!eval_matcher!("world hello", starts_with("hello")));

        assert!(eval_matcher!("data123:abc", starts_with("data", digits(), ":")));
        assert!(eval_matcher!("data:abc", starts_with("data", digits(), ":")));
        assert!(!eval_matcher!("data_foo:abc", starts_with("data", digits(), ":")));

        assert!(eval_matcher!("libfoo.so.1", contains(".so")));
        assert!(!eval_matcher!("libfoo.dll", contains(".so")));

        assert!(eval_matcher!("bootfs:bin", or(exact("bootfs"), starts_with("bootfs:"))));
        assert!(eval_matcher!("bootfs", or(exact("bootfs"), starts_with("bootfs:"))));
        assert!(!eval_matcher!("bootfs_other", or(exact("bootfs"), starts_with("bootfs:"))));

        // Test multi-alternative and trailing commas
        assert!(eval_matcher!("bar", or(exact("foo"), exact("bar"), exact("baz"),)));
        assert!(!eval_matcher!("other", or(exact("foo"), exact("bar"), exact("baz"))));
        assert!(eval_matcher!("single", or(exact("single"))));
        assert!(!eval_matcher!("anything", or()));
    }

    mod test_macro_expansion {
        use super::*;
        vmo_digests! {
            (TestA, "[test-a]", exact("a")),
            (TestB, "[test-b]", or(starts_with("b:"), contains("b_mid"))),
            (TestC, "[test-c]", exact("c-", hex())),
            (TestD, "[test-d]", exact("d:", digits())),
        }

        #[test]
        fn test_generated_methods() {
            assert_eq!(VmoDigestCategory::TestA.as_str(), "[test-a]");
            assert_eq!(VmoDigestCategory::TestB.as_str(), "[test-b]");
            assert_eq!(VmoDigestCategory::TestC.as_str(), "[test-c]");
            assert_eq!(VmoDigestCategory::TestD.as_str(), "[test-d]");

            assert_eq!(
                VmoDigestCategory::TestA.as_zxname(),
                &ZXName::from_string_lossy("[test-a]")
            );
            assert_eq!(
                VmoDigestCategory::TestB.as_zxname(),
                &ZXName::from_string_lossy("[test-b]")
            );
            assert_eq!(
                VmoDigestCategory::TestC.as_zxname(),
                &ZXName::from_string_lossy("[test-c]")
            );
            assert_eq!(
                VmoDigestCategory::TestD.as_zxname(),
                &ZXName::from_string_lossy("[test-d]")
            );

            assert_eq!(match_vmo_digest("a"), Some(VmoDigestCategory::TestA));
            assert_eq!(match_vmo_digest("b:123"), Some(VmoDigestCategory::TestB));
            assert_eq!(match_vmo_digest("pre_b_mid_post"), Some(VmoDigestCategory::TestB));
            assert_eq!(match_vmo_digest("c-deadbeef"), Some(VmoDigestCategory::TestC));
            assert_eq!(match_vmo_digest("c-nonhex"), None);
            assert_eq!(match_vmo_digest("d:12345"), Some(VmoDigestCategory::TestD));
            assert_eq!(match_vmo_digest("d:"), None);
            assert_eq!(match_vmo_digest("unmatched"), None);
        }
    }
}
