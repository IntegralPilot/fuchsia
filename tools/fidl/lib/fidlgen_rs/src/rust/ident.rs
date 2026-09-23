// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;
use std::sync::LazyLock;

use fidl_ir::Ident;

use crate::SplitIdent;

pub trait RustIdent: SplitIdent {
    fn camel(&self) -> String {
        let mut result = String::new();

        for piece in self.split() {
            let mut chars = piece.chars();
            result.push(chars.next().unwrap().to_ascii_uppercase());
            result.extend(chars.map(|c| c.to_ascii_lowercase()));
        }

        if is_rust_keyword(&result) {
            result.push('_');
        }

        result
    }

    fn snake(&self) -> String {
        let mut result = String::new();

        for piece in self.split() {
            if !result.is_empty() {
                result.push('_');
            }
            result.extend(piece.chars().map(|c| c.to_ascii_lowercase()));
        }

        if is_rust_keyword(&result) {
            result.push('_');
        }

        result
    }

    fn screaming_snake(&self) -> String {
        let mut result = String::new();

        for piece in self.split() {
            if !result.is_empty() {
                result.push('_');
            }
            result.extend(piece.chars().map(|c| c.to_ascii_uppercase()));
        }

        if is_rust_keyword(&result) {
            result.push('_');
        }

        result
    }
}

impl RustIdent for Ident {}

pub fn is_rust_keyword(name: &str) -> bool {
    RUST_KEYWORDS.contains(name)
}

static RUST_KEYWORDS: LazyLock<HashSet<String>> =
    LazyLock::new(|| RUST_KEYWORDS_LIST.iter().map(|k| k.to_string()).collect());

const RUST_KEYWORDS_LIST: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "macro_rules",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "pub",
    "priv",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
];

#[cfg(test)]
mod tests {
    use fidl_ir::Ident;

    use super::*;

    const TEST_CASES: &[&str] = &[
        "foo_bar",
        "foo__bar",
        "FooBar",
        "fooBar",
        "FOOBar",
        "__foo_bar",
        "foo123bar",
        "foO123bar",
        "foo_123bar",
        "FOO123Bar",
        "FOO123bar",
    ];

    #[test]
    fn split() {
        const EXPECTEDS: [&[&str]; TEST_CASES.len()] = [
            &["foo", "bar"],
            &["foo", "bar"],
            &["Foo", "Bar"],
            &["foo", "Bar"],
            &["FOO", "Bar"],
            &["foo", "bar"],
            &["foo123bar"],
            &["fo", "O123bar"],
            &["foo", "123bar"],
            &["FOO123", "Bar"],
            &["FOO123bar"],
        ];

        for (case, expected) in TEST_CASES.iter().zip(EXPECTEDS.iter()) {
            assert_eq!(
                &Ident::from_str(case).split().collect::<Vec<_>>(),
                expected,
                "{case} did not split correctly",
            );
        }
    }

    #[test]
    fn snake() {
        const EXPECTEDS: [&str; TEST_CASES.len()] = [
            "foo_bar",
            "foo_bar",
            "foo_bar",
            "foo_bar",
            "foo_bar",
            "foo_bar",
            "foo123bar",
            "fo_o123bar",
            "foo_123bar",
            "foo123_bar",
            "foo123bar",
        ];

        for (case, expected) in TEST_CASES.iter().zip(EXPECTEDS.iter()) {
            assert_eq!(
                &Ident::from_str(case).snake(),
                expected,
                "{case} was not transformed to snake case correctly",
            );
        }
    }

    #[test]
    fn camel() {
        const EXPECTEDS: [&str; TEST_CASES.len()] = [
            "FooBar",
            "FooBar",
            "FooBar",
            "FooBar",
            "FooBar",
            "FooBar",
            "Foo123bar",
            "FoO123bar",
            "Foo123bar",
            "Foo123Bar",
            "Foo123bar",
        ];

        for (case, expected) in TEST_CASES.iter().zip(EXPECTEDS.iter()) {
            assert_eq!(
                &Ident::from_str(case).camel(),
                expected,
                "{case} was not transformed to camel case correctly",
            );
        }
    }

    #[test]
    fn screaming_snake() {
        const EXPECTEDS: [&str; TEST_CASES.len()] = [
            "FOO_BAR",
            "FOO_BAR",
            "FOO_BAR",
            "FOO_BAR",
            "FOO_BAR",
            "FOO_BAR",
            "FOO123BAR",
            "FO_O123BAR",
            "FOO_123BAR",
            "FOO123_BAR",
            "FOO123BAR",
        ];

        for (case, expected) in TEST_CASES.iter().zip(EXPECTEDS.iter()) {
            assert_eq!(
                &Ident::from_str(case).screaming_snake(),
                expected,
                "{case} was not transformed to screaming snake case correctly",
            );
        }
    }
}
