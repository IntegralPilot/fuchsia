// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Hash, Eq, Error)]
pub enum NumberError {
    #[error("Number contains control characters")]
    ControlCharacters,
    #[error("Number contains internal quotes")]
    InternalQuotes,
    #[error("Number is not enclosed in delimiting quotes")]
    NotQuoted,
}

/// The fuchsia.bluetooth.hfp library representation of a Number.
pub type FidlNumber = String;
/// A phone number.  Clients should generally use `as_non_at_string` and
/// `from_non_at_string` to work with these, which add and remove delimiting
/// quotes.  As AT commands require these quotes to be in place around numbers,
/// when generating and parsing AT commands, clients should use `from_at_string`
/// and `as_at_string`, which maintain the quotes.
#[derive(Debug, Clone, PartialEq, Hash, Default, Eq)]
pub struct Number(String);

fn is_quoted(s: &str) -> bool {
    s.starts_with('"') && s.ends_with('"') && s.len() >= 2
}

impl Number {
    /// Format value indicating no changes on the number presentation are required.
    /// See HFP v1.8, Section 4.34.2.
    const NUMBER_FORMAT: i64 = 129;

    /// Returns the numeric representation of the Number's format as specified in HFP v1.8,
    /// Section 4.34.2.
    pub fn type_(&self) -> i64 {
        Number::NUMBER_FORMAT
    }

    /// Converts the Number to a String, stripping quotes from the beginning and end.
    pub fn to_non_at_string(&self) -> String {
        if is_quoted(&self.0) {
            let string = self.0.clone();
            let mut chars = string.chars();
            let _front_must_exist = chars.next();
            let _back_must_exist = chars.next_back();
            String::from(chars.as_str())
        } else {
            self.0.clone()
        }
    }

    /// Converts the Number to a String to be used in AT commands, leaving the delimiting quotes in
    /// place.
    pub fn to_at_string(&self) -> String {
        self.0.clone()
    }

    pub fn from_non_at_string(s: &str) -> Result<Self, NumberError> {
        if is_quoted(s) {
            Self::from_at_string(s)
        } else {
            let quoted = format!("\"{}\"", s);
            Self::from_at_string(&quoted)
        }
    }

    /// Converts a String to a Number, from an AT command, leaving the delimiting quotes in place.
    /// Returns an error if the string contains ASCII control characters or internal quotes.
    pub fn from_at_string(s: &str) -> Result<Self, NumberError> {
        if s.chars().any(|c| c.is_ascii_control()) {
            return Err(NumberError::ControlCharacters);
        }

        // It is valid for the phone number to be completely empty/omitted in HFP/AT.
        if s.is_empty() {
            return Ok(Self(String::from(s)));
        }
        if !is_quoted(s) {
            return Err(NumberError::NotQuoted);
        }

        let inner_s = &s[1..s.len() - 1];

        if inner_s.contains('"') {
            return Err(NumberError::InternalQuotes);
        }

        Ok(Self(String::from(s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn number_type_in_valid_range() {
        let number = Number(String::from("\"1234567\""));
        // type values must be in range 128-175.
        assert!(number.type_() >= 128);
        assert!(number.type_() <= 175);
    }

    #[fuchsia::test]
    fn number_str_delimiters() {
        // Convert str to Number
        {
            let actual_number = Number::from_non_at_string("1234567").unwrap();
            let expected_number = Number(String::from("\"1234567\""));
            assert_eq!(actual_number, expected_number);
        }

        // Convert Number to str
        {
            let actual_string = Number(String::from("\"1234567\"")).to_non_at_string();
            let expected_string = String::from("1234567");
            assert_eq!(actual_string, expected_string);
        }

        // Convert str to Number with redundant quotes
        {
            let actual_number = Number::from_non_at_string("\"1234567\"").unwrap();
            let expected_number = Number(String::from("\"1234567\""));
            assert_eq!(actual_number, expected_number);
        }

        // Convert AT command str to Number
        {
            let actual_number = Number::from_at_string("\"1234567\"").unwrap();
            let expected_number = Number(String::from("\"1234567\""));
            assert_eq!(actual_number, expected_number);
        }

        // Convert Number to AT command str
        {
            let actual_string = Number(String::from("\"1234567\"")).to_at_string();
            let expected_string = String::from("\"1234567\"");
            assert_eq!(actual_string, expected_string);
        }
    }

    #[fuchsia::test]
    fn number_validation_success() {
        assert!(Number::from_non_at_string("").is_ok());
        assert!(Number::from_non_at_string("123456").is_ok());
        assert!(Number::from_non_at_string("\"123456\"").is_ok());
        assert!(Number::from_at_string("").is_ok());
        assert!(Number::from_at_string("\"123456\"").is_ok());
    }

    #[fuchsia::test]
    fn number_validation_error() {
        // Control characters are rejected
        assert!(matches!(
            Number::from_non_at_string("123\r\n456"),
            Err(NumberError::ControlCharacters)
        ));
        assert!(matches!(
            Number::from_at_string("\"123\0456\""),
            Err(NumberError::ControlCharacters)
        ));

        // Not quoted is rejected for from_at_string
        assert!(matches!(Number::from_at_string("123456"), Err(NumberError::NotQuoted)));

        // Internal quotes are rejected
        assert!(matches!(Number::from_non_at_string("123\"456"), Err(NumberError::InternalQuotes)));
        assert!(matches!(
            Number::from_non_at_string("\"123\"456\""),
            Err(NumberError::InternalQuotes)
        ));
        assert!(matches!(Number::from_at_string("\"123\"456\""), Err(NumberError::InternalQuotes)));
    }
}
