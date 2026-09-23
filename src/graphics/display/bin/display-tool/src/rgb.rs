// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::{fmt, str};

#[derive(Debug, PartialEq)]
pub enum ParseRgbError {
    UnexpectedCharacter,
    IncorrectSize(usize),
}

impl fmt::Display for ParseRgbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseRgbError::UnexpectedCharacter => write!(f, "Unexpected character"),
            ParseRgbError::IncorrectSize(size) => write!(f, "Incorrect size {}", size),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rgb888 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl str::FromStr for Rgb888 {
    type Err = ParseRgbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.chars().all(|x| x.is_ascii_hexdigit()) {
            return Err(ParseRgbError::UnexpectedCharacter);
        }
        if s.len() != 6 {
            return Err(ParseRgbError::IncorrectSize(s.len()));
        }
        Ok(Rgb888 {
            r: u8::from_str_radix(&s[0..2], 16).unwrap(),
            g: u8::from_str_radix(&s[2..4], 16).unwrap(),
            b: u8::from_str_radix(&s[4..6], 16).unwrap(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use googletest::{expect_that, gtest, matchers};
    use std::str::FromStr;

    #[gtest]
    #[fuchsia::test]
    fn rgb_from_str_invalid() {
        expect_that!(
            &Rgb888::from_str("zz00ff"),
            matchers::err(matchers::eq(&ParseRgbError::UnexpectedCharacter))
        );
        expect_that!(
            &Rgb888::from_str("0000vv"),
            matchers::err(matchers::eq(&ParseRgbError::UnexpectedCharacter))
        );
        expect_that!(
            &Rgb888::from_str("0x010101"),
            matchers::err(matchers::eq(&ParseRgbError::UnexpectedCharacter))
        );

        expect_that!(
            &Rgb888::from_str(""),
            matchers::err(matchers::eq(&ParseRgbError::IncorrectSize(0))),
        );
        expect_that!(
            &Rgb888::from_str("10101"),
            matchers::err(matchers::eq(&ParseRgbError::IncorrectSize(5)))
        );
        expect_that!(
            &Rgb888::from_str("1010111"),
            matchers::err(matchers::eq(&ParseRgbError::IncorrectSize(7)))
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn rgb_from_str_valid() {
        expect_that!(
            &Rgb888::from_str("ef0000"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0xef, g: 0x00, b: 0x00 }))
        );
        expect_that!(
            &Rgb888::from_str("00ab00"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0x00, g: 0xab, b: 0x00 }))
        );
        expect_that!(
            &Rgb888::from_str("0000cd"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0x00, g: 0x00, b: 0xcd }))
        );
        expect_that!(
            &Rgb888::from_str("012345"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0x01, g: 0x23, b: 0x45 }))
        );
        expect_that!(
            &Rgb888::from_str("000000"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0x00, g: 0x00, b: 0x00 }))
        );
        expect_that!(
            &Rgb888::from_str("ffffff"),
            matchers::ok(matchers::eq(&Rgb888 { r: 0xff, g: 0xff, b: 0xff }))
        );
    }
}
