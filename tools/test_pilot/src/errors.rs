// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::name::Name;
use serde_json::Value;
use std::io;
use std::path::PathBuf;
use test_pilot_lib::test_output::TestOutputError;
use thiserror::Error;
use valico::json_schema::validators::ValidationState;

/// Error encountered while executing test binary
#[derive(Debug, Error)]
pub enum TestRunError {
    #[error("Error launching test binary '{path:?}': {source}")]
    Spawn {
        path: std::ffi::OsString,
        #[source]
        source: io::Error,
    },

    #[error("Error reading stdout: {0}")]
    StdoutRead(#[source] io::Error),

    #[error("Error reading stderr: {0}")]
    StderrRead(#[source] io::Error),

    #[error("Error writing stdout: {0}")]
    StdoutWrite(#[source] io::Error),

    #[error("Error writing stderr: {0}")]
    StderrWrite(#[source] io::Error),

    #[error("Error creating output file {path}")]
    FailedToCreateFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Output operation failed: {0}")]
    OutputOperationFailed(#[from] TestOutputError),
}

/// Error encountered validating config
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("Schema file {path} could not be opened for reading: {source}")]
    FailedToOpenSchema {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failure attempting to read schema {path}: {source}")]
    FailedToReadSchema {
        path: PathBuf,
        #[source]
        source: serde_json5::Error,
    },

    #[error("Failure attempting to parse schema {path}: {source}")]
    FailedToParseSchema {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Schema not well-formed: {0}")]
    InvalidSchema(String),

    #[error("Failed to parse test configuration as aggregated JSON value: {0}")]
    FailedToParse(#[from] serde_json::Error),

    #[error("Include file {path} could not be opened for reading: {source}")]
    FailedToOpenInclude {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("Failure attempting to parse include {path}: {source}")]
    FailedToParseInclude {
        path: PathBuf,
        #[source]
        source: serde_json5::Error,
    },

    #[error("Incorrect usage: {0}")]
    IncorrectUsage(#[from] UsageError),

    #[error("Multiple validation errors: {0:?}")]
    ValidationMultiple(Vec<BuildError>),

    // The valico error here is deliberately formatted instead of being included explicitly.
    // Neither valico errors nor validation_state are cloneable, creating problems when we
    // want to capture them in errors. Also, valico errors are not std::error::Error.
    #[error("Unclassified schema error, please file a bug to classify {0}")]
    UnclassifiedSchemaError(String),

    #[error("Unclassified schema state, please file a bug to classify {0:?}")]
    UnclassifiedSchemaState(Box<ValidationState>),
}

/// Error encountered processing command line arguments, environment variables, or
/// included JSON files.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum UsageError {
    #[error("Unexpected positional argument '{0}'")]
    UnexpectedPositionalArgument(String),

    #[error("Argument '{0}' missing required value")]
    MissingValue(Name),

    #[error("Parameter name '{0}' does not appear in the test config schema")]
    UnrecognizedParameter(Name),

    #[error("Option '{option}' has unexpected value {got}")]
    UnexpectedOptionValue { option: Name, got: Value },

    #[error("Invalid 'strict' value: {0}. 'strict' can only be set to true")]
    InvalidStrictValue(String),

    #[error("Included path {0} does not exist")]
    IncludedPathDoesNotExist(PathBuf),

    #[error("Included path {0} cannot be read (user lacks permission?)")]
    IncludedPathUnreadable(PathBuf),

    #[error("Included path {0} is not a file")]
    IncludedPathIsNotAFile(PathBuf),

    #[error("Parameter '{0}' is required, but was not in the schema")]
    UnknownRequiredParameter(Name),

    #[error("Parameter '{0}' is required, but was not defined")]
    MissingRequiredParameter(Name),

    #[error("Parameter '{0}' is prohibited, but was not in the schema")]
    UnknownProhibitedParameter(Name),

    #[error("Parameter '{0}' is prohibited, but was defined")]
    DefinedProhibitedParameter(Name),

    #[error("Expected viable parameter names for option '{option}', got {got}")]
    InvalidParameterName { option: Name, got: Name },

    #[error("Unterminated pattern in argument {0}")]
    UnterminatedArgPattern(String),

    #[error("Unknown parameter {unknown_parameter} referenced in argument pattern {pattern}")]
    UnknownArgPattern { unknown_parameter: String, pattern: String },

    #[error("Parameter {0} was assigned more than one value in strict mode")]
    ParamAlreadyStrictlyAssigned(Name),

    #[error("Wrong type: expected type {expected}, got \"{got}\" for parameter {parameter}")]
    TypeMismatch { expected: String, got: String, parameter: Name },

    #[error(
        "Parameters of type object (such as {0}) are not allowed in command lines or environment variables"
    )]
    ObjectNotAllowed(Name),

    #[error("Parameter '{0}' is required by the schema, but was not defined")]
    MissingParameterRequiredBySchema(String),

    #[error("Type of parameter {parameter} disagrees with schema, {detail}")]
    SchemaTypeMismatch { parameter: String, detail: String },

    #[error(
        "Parameters of type array must have simple items in command lines or environment variables"
    )]
    ArrayOfComplexTypeNotAllowed(Name),

    #[error("Expected executable file path for option '{option}', got nonexistent {path:?}")]
    BinaryDoesNotExist { option: Name, path: PathBuf },

    #[error("Expected executable file path for option '{option}', got directory {path:?}")]
    BinaryIsNotAFile { option: Name, path: PathBuf },

    #[error("Expected executable file path for option '{option}', got unreadable {path:?}")]
    BinaryUnreadable { option: Name, path: PathBuf },

    #[error(
        "Assignment to parameter '{parameter}' references undefined environment variable {var_name_value:?}"
    )]
    FromEnvUndefined { parameter: Name, var_name_value: Value },

    #[error(
        "Assignment to parameter '{parameter}' uses non-string value {var_name_value:?} in from_env/try_from_env"
    )]
    FromEnvNotString { parameter: Name, var_name_value: Value },
}
