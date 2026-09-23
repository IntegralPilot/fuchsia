// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::schema::Schema;
use heck::ToSnakeCase;
use serde::Deserialize;

/// Represents an option or parameter name.
#[derive(Debug, PartialEq, Eq, Hash, Deserialize, Clone, PartialOrd, Ord)]
pub enum Name {
    // Options
    #[serde(rename = "schema")]
    Schema,

    #[serde(rename = "debug")]
    Debug,

    #[serde(rename = "strict")]
    Strict,

    #[serde(rename = "include")]
    Include,

    #[serde(rename = "require")]
    Require,

    #[serde(rename = "prohibit")]
    Prohibit,

    // Parameter name
    #[serde(untagged)]
    Parameter(String),
}

impl Name {
    /// Create a `Name` from a `&str` in lower snake case.
    pub fn from_str(s: &str) -> Self {
        match s {
            "schema" => Self::Schema,
            "debug" => Self::Debug,
            "strict" => Self::Strict,
            "include" => Self::Include,
            "require" => Self::Require,
            "prohibit" => Self::Prohibit,
            _ => Self::Parameter(String::from(s)),
        }
    }

    /// Returns the `String` representation of this `Name`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Schema => "schema",
            Self::Debug => "debug",
            Self::Strict => "strict",
            Self::Include => "include",
            Self::Require => "require",
            Self::Prohibit => "prohibit",
            Self::Parameter(parameter) => parameter,
        }
    }

    /// Determines whether this `Name` is a viable parameter name without consulting the schema. A
    /// parameter name is viable if:
    /// 1) it's not one of the reserved option names
    /// 2) it starts with a lower-case ascii letter or underscore
    /// 3) it otherwise consists of lower-case ascii letters, ascii digits and underscores.
    pub fn is_viable_parameter_name(&self) -> bool {
        match self {
            Self::Schema
            | Self::Debug
            | Self::Strict
            | Self::Include
            | Self::Require
            | Self::Prohibit => false,
            Self::Parameter(parameter) => {
                let mut iter = parameter.chars();
                let first = iter.next().expect("value strings are at least 1 character");
                (first.is_ascii_lowercase() || first == '_')
                    && iter.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            }
        }
    }

    /// Determines whether this `Name` is handled by `add_name_and_value`.
    pub fn can_be_added(&self, schema: &Schema) -> bool {
        match self {
            Self::Schema | Self::Debug => false,
            Self::Strict | Self::Include | Self::Require | Self::Prohibit => true,
            Self::Parameter(_) => schema.properties.contains_key(self),
        }
    }

    /// Implements equivalent of `&str::starts_with`, which determines whether `self` has
    /// the specified prefix.
    pub fn starts_with(&self, prefix: &str) -> bool {
        match self {
            Self::Parameter(parameter) => parameter.starts_with(prefix),
            _ => false,
        }
    }

    /// Implements equivalent of `&str::strip_prefix`, which removes the specified prefix from
    /// `self`. Returns `None` if `self` doesn't have the specified prefix.
    pub fn strip_prefix(&self, prefix: &str) -> Option<Name> {
        match self {
            Self::Parameter(parameter) => {
                parameter.strip_prefix(prefix).map(|stripped| Self::from(String::from(stripped)))
            }
            _ => None,
        }
    }

    /// Converts a command line argument name to the equivalent `Name`. Specifically, the
    /// argument name is converted to lower_snake_case.
    pub fn from_arg_name(arg_name: &str) -> Self {
        Self::from(arg_name.to_snake_case())
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        match s.as_str() {
            "schema" => Self::Schema,
            "debug" => Self::Debug,
            "strict" => Self::Strict,
            "include" => Self::Include,
            "require" => Self::Require,
            "prohibit" => Self::Prohibit,
            _ => Self::Parameter(s),
        }
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_viable_parameter_name() {
        assert_eq!(false, Name::from_str("schema").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("debug").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("strict").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("include").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("require").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("prohibit").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("3").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("3_foo").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("Foo").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("!foo").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("foo!").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("fooBAR").is_viable_parameter_name());
        assert_eq!(false, Name::from_str("foo_BAR").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("foo").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("foo3").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("_foo").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("foo_").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("foo_bar").is_viable_parameter_name());
        assert_eq!(true, Name::from_str("foo_3").is_viable_parameter_name());
    }
}
