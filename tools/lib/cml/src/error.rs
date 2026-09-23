// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_fidl_validator::error::ErrorList;
use cm_types::ParseError;
use fidl_fuchsia_component_decl as fdecl;
use std::path::Path;
use std::str::Utf8Error;
use std::sync::Arc;
use std::{error, fmt, io};

/// The location in the file where an error was detected.
#[derive(PartialEq, Clone, Debug, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Location {
    /// One-based line number of the error.
    pub line: usize,

    /// One-based column number of the error.
    pub column: usize,
}

/// Enum type that can represent any error encountered by a cml operation.
#[derive(Debug)]
pub enum Error {
    DuplicateRights(String),
    InvalidArgs(String),
    Io(io::Error),
    FidlEncoding(fidl::Error),
    Merge {
        err: String,
        origin: Option<Arc<std::path::Path>>,
    },
    MissingRights(String),
    Parse {
        err: String,
        location: Option<Location>,
        filename: Option<String>,
    },
    Validate {
        err: String,
        filename: Option<String>,
    },
    ValidateContext {
        err: String,
        origin: Option<Arc<std::path::Path>>,
    },
    ValidateContexts {
        err: String,
        origins: Vec<Arc<std::path::Path>>,
    },
    FidlValidator {
        errs: ErrorList,
    },
    Internal(String),
    Utf8(Utf8Error),
    /// A restricted feature was used without opting-in.
    RestrictedFeature(String),
}

impl error::Error for Error {}

impl Error {
    pub fn invalid_args(err: impl Into<String>) -> Self {
        Self::InvalidArgs(err.into())
    }

    pub fn parse(
        err: impl fmt::Display,
        location: Option<Location>,
        filename: Option<&Path>,
    ) -> Self {
        Self::Parse {
            err: err.to_string(),
            location,
            filename: filename.map(|f| f.to_string_lossy().into_owned()),
        }
    }

    pub fn merge(err: impl fmt::Display, origin: Option<Arc<Path>>) -> Self {
        Self::Merge { err: err.to_string(), origin }
    }

    pub fn validate(err: impl fmt::Display) -> Self {
        Self::Validate { err: err.to_string(), filename: None }
    }

    pub fn validate_context(err: impl fmt::Display, origin: Option<Arc<std::path::Path>>) -> Self {
        Self::ValidateContext { err: err.to_string(), origin }
    }
    pub fn validate_contexts(err: impl fmt::Display, origins: Vec<Arc<std::path::Path>>) -> Self {
        Self::ValidateContexts { err: err.to_string(), origins }
    }

    pub fn fidl_validator(errs: ErrorList) -> Self {
        Self::FidlValidator { errs }
    }

    pub fn duplicate_rights(err: impl Into<String>) -> Self {
        Self::DuplicateRights(err.into())
    }

    pub fn missing_rights(err: impl Into<String>) -> Self {
        Self::MissingRights(err.into())
    }

    pub fn internal(err: impl Into<String>) -> Self {
        Self::Internal(err.into())
    }

    pub fn json5(err: json5format::Error, file: Option<&Path>) -> Self {
        match err {
            json5format::Error::Configuration(errstr) => Error::Internal(errstr),
            json5format::Error::Parse(location, errstr) => match location {
                Some(location) => Error::parse(
                    errstr,
                    Some(Location { line: location.line, column: location.col }),
                    file,
                ),
                None => Error::parse(errstr, None, file),
            },
            json5format::Error::Internal(location, errstr) => match location {
                Some(location) => Error::Internal(format!("{}: {}", location, errstr)),
                None => Error::Internal(errstr),
            },
            json5format::Error::TestFailure(location, errstr) => match location {
                Some(location) => {
                    Error::Internal(format!("{}: Test failure: {}", location, errstr))
                }
                None => Error::Internal(format!("Test failure: {}", errstr)),
            },
        }
    }

    pub fn with_origin(self, origin: Arc<std::path::Path>) -> Self {
        match self {
            Error::ValidateContext { err, .. } => {
                Error::ValidateContext { err, origin: Some(origin) }
            }
            _ => Error::ValidateContext { err: self.to_string(), origin: Some(origin) },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            Error::DuplicateRights(err) => write!(f, "Duplicate rights: {}", err),
            Error::InvalidArgs(err) => write!(f, "Invalid args: {}", err),
            Error::Io(err) => write!(f, "IO error: {}", err),
            Error::FidlEncoding(err) => write!(f, "Fidl encoding error: {}", err),
            Error::Merge { err, origin } => {
                let mut prefix = String::new();

                if let Some(origin) = origin {
                    prefix.push_str(&format!("{:?}:", origin));
                }

                if !prefix.is_empty() {
                    write!(f, "Error merging at {} {}", prefix, err)
                } else {
                    write!(f, "{}", err)
                }
            }
            Error::MissingRights(err) => write!(f, "Missing rights: {}", err),
            Error::Parse { err, location, filename } => {
                let mut prefix = String::new();
                if let Some(filename) = filename {
                    prefix.push_str(&format!("{}:", filename));
                }
                if let Some(location) = location {
                    // Check for a syntax error generated by pest. These error messages have
                    // the line and column number embedded in them, so we don't want to
                    // duplicate that.
                    //
                    // TODO: If serde_json5 had an error type for json5 syntax errors, we wouldn't
                    // need to parse the string like this.
                    if !err.starts_with(" -->") {
                        prefix.push_str(&format!("{}:{}:", location.line, location.column));
                    }
                }
                if !prefix.is_empty() {
                    write!(f, "Error at {} {}", prefix, err)
                } else {
                    write!(f, "{}", err)
                }
            }
            Error::Validate { err, filename } => {
                let mut prefix = String::new();
                if let Some(filename) = filename {
                    prefix.push_str(&format!("{}:", filename));
                }
                if !prefix.is_empty() {
                    write!(f, "Error at {} {}", prefix, err)
                } else {
                    write!(f, "{}", err)
                }
            }
            Error::ValidateContext { err, origin } => {
                let mut prefix = String::new();

                if let Some(origin) = origin {
                    prefix.push_str(&format!("{:?}:", origin));
                }

                if !prefix.is_empty() {
                    write!(f, "Error at {} {}", prefix, err)
                } else {
                    write!(f, "{}", err)
                }
            }
            Error::ValidateContexts { err, origins } => {
                let mut prefix = String::new();

                for origin in origins {
                    if !prefix.is_empty() {
                        prefix.push_str(", and ");
                    }

                    prefix.push_str(&format!("{:?}:", origin));
                }

                if !prefix.is_empty() {
                    write!(f, "Error at {} {}", prefix, err)
                } else {
                    write!(f, "{}", err)
                }
            }
            Error::FidlValidator { errs } => format_multiple_fidl_validator_errors(errs, f),
            Error::Internal(err) => write!(f, "Internal error: {}", err),
            Error::Utf8(err) => write!(f, "UTF8 error: {}", err),
            Error::RestrictedFeature(feature) => write!(
                f,
                "Use of restricted feature \"{}\". To opt-in, see https://fuchsia.dev/go/components/restricted-features",
                feature
            ),
        }
    }
}

fn format_multiple_fidl_validator_errors(e: &ErrorList, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    // Some errors are caught by `cm_fidl_validator` but not caught by `cml` validation.
    //
    // Our strategy is:
    //
    // - If a `cm_fidl_validator` error can be easily transformed back to be relevant in the context
    //   of `cml`, do that. For example, we should transform the FIDL declaration names
    //   to corresponding cml names.
    //
    // - Otherwise, we consider that a bug and we should add corresponding validation in `cml`.
    //   As such, we will surface this kind of errors as `Internal` as an indication.
    //   That is represented by the `_` match arm here.
    //
    use cm_fidl_validator::error::Error as CmFidlError;
    let mut found_internal_errors = false;
    for e in e.errs.iter() {
        match e {
            CmFidlError::DifferentAvailabilityInAggregation(availability_list) => {
                // Format the availability in `cml` syntax.
                let comma_separated = availability_list
                    .0
                    .iter()
                    .map(|s| match s {
                        fdecl::Availability::Required => "\"required\"".to_string(),
                        fdecl::Availability::Optional => "\"optional\"".to_string(),
                        fdecl::Availability::SameAsTarget => "\"same_as_target\"".to_string(),
                        fdecl::Availability::Transitional => "\"transitional\"".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                write!(
                    f,
                    "All sources that feed into an aggregation operation should have the same availability. "
                )?;
                write!(f, "Got [ {comma_separated} ].")?;
            }
            _ => {
                write!(f, "Internal error: {}\n", e)?;
                found_internal_errors = true;
            }
        }
    }

    if found_internal_errors {
        write!(
            f,
            "This reflects error(s) in the `.cml` file. \
Unfortunately, for some of them, cmc cannot provide more details at this time.
Please file a bug at https://bugs.fuchsia.dev/p/fuchsia/issues/entry?template=ComponentFramework \
with the cml in question, so we can work on better error reporting."
        )?;
    }

    Ok(())
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<Utf8Error> for Error {
    fn from(err: Utf8Error) -> Self {
        Error::Utf8(err)
    }
}

impl From<serde_json::error::Error> for Error {
    fn from(err: serde_json::error::Error) -> Self {
        use serde_json::error::Category;
        match err.classify() {
            Category::Io | Category::Eof => Error::Io(err.into()),
            Category::Syntax => {
                let line = err.line();
                let column = err.column();
                Error::parse(err, Some(Location { line, column }), None)
            }
            Category::Data => Error::validate(err),
        }
    }
}

impl From<fidl::Error> for Error {
    fn from(err: fidl::Error) -> Self {
        Error::FidlEncoding(err)
    }
}

impl From<ParseError> for Error {
    fn from(err: ParseError) -> Self {
        match err {
            ParseError::InvalidValue => Self::internal("invalid value"),
            ParseError::InvalidComponentUrl { details } => {
                Self::internal(&format!("invalid component url: {details}"))
            }
            ParseError::TooLong => Self::internal("too long"),
            ParseError::Empty => Self::internal("empty"),
            ParseError::InvalidSegment => Self::internal("invalid path segment"),
            ParseError::NoLeadingSlash => Self::internal("missing leading slash"),
        }
    }
}

impl TryFrom<serde_json5::Error> for Location {
    type Error = &'static str;
    fn try_from(e: serde_json5::Error) -> Result<Self, Self::Error> {
        match e {
            serde_json5::Error::Message { location: Some(l), .. } => {
                Ok(Location { line: l.line, column: l.column })
            }
            _ => Err("location unavailable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::format_err;
    use assert_matches::assert_matches;
    use cm_types as cm;

    #[test]
    fn test_syntax_error_message() {
        let result = serde_json::from_str::<cm::Name>("foo").map_err(Error::from);
        assert_matches!(result, Err(Error::Parse { .. }));
    }

    #[test]
    fn test_validation_error_message() {
        let result = serde_json::from_str::<cm::Name>("\"foo$\"").map_err(Error::from);
        assert_matches!(result, Err(Error::Validate { .. }));
    }

    #[test]
    fn test_parse_error() {
        let result = Error::parse(format_err!("oops"), None, None);
        assert_eq!(format!("{}", result), "oops");

        let result = Error::parse(format_err!("oops"), Some(Location { line: 2, column: 3 }), None);
        assert_eq!(format!("{}", result), "Error at 2:3: oops");

        let result = Error::parse(
            format_err!("oops"),
            Some(Location { line: 2, column: 3 }),
            Some(&Path::new("test.cml")),
        );
        assert_eq!(format!("{}", result), "Error at test.cml:2:3: oops");

        let result = Error::parse(
            format_err!(" --> pest error"),
            Some(Location { line: 42, column: 42 }),
            Some(&Path::new("test.cml")),
        );
        assert_eq!(format!("{}", result), "Error at test.cml:  --> pest error");
    }

    #[test]
    fn test_validation_error() {
        let mut result = Error::validate(format_err!("oops"));
        assert_eq!(format!("{}", result), "oops");

        if let Error::Validate { filename, .. } = &mut result {
            *filename = Some("test.cml".to_string());
        }
        assert_eq!(format!("{}", result), "Error at test.cml: oops");
    }

    #[test]
    fn test_format_multiple_fidl_validator_errors() {
        use cm_fidl_validator::error::{AvailabilityList, Error as CmFidlError};

        let error = Error::FidlValidator {
            errs: ErrorList {
                errs: vec![CmFidlError::DifferentAvailabilityInAggregation(AvailabilityList(
                    vec![fdecl::Availability::Required, fdecl::Availability::Optional],
                ))],
            },
        };
        assert_eq!(
            format!("{error}"),
            "All sources that feed into an aggregation operation should \
            have the same availability. Got [ \"required\", \"optional\" ]."
        );
    }
}
