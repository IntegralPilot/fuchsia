// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builder::build_from_env_like;
use crate::env::EnvLike;
use crate::errors::{BuildError, UsageError};
use crate::logger::Logger;
use crate::name::Name;
use crate::schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const HOST_TEST_BINARY_OPTION: &str = "host_test_binary";
const OUTPUT_DIRECTORY_OPTION: &str = "output_directory";
const BINARY_OPTION: &str = "binary";
const TEST_CONFIG_FILE_PATTERN: &str = "test_config_file";
const RESOLVED_HOST_TEST_ARGS_EXPECT: &str =
    "TestConfig is validated prior to calling resolved_host_test_args";

/// Parameters describing a test to be run.
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct TestConfig {
    /// Path for the executable that this program should invoke to run the test.
    pub host_test_binary: PathBuf,

    /// Arguments to pass to `host_test_binary` to run the test. Substrings in curly braces are
    /// replaced with the value of the test parameter named in the braces. For example
    /// "--out={output_directory}" produces a single argument "--out=<foo>" where <foo> is the
    /// value of the `output_directory` test parameter. In addition, the substring
    /// "{test_config_file}" is replaced by the path to the file containing the test parameters
    /// in JSON format.
    ///
    /// If `host_test_args` is not supplied, the host test binary is invoked with two parameters:
    /// the path of the JSON file containing test parameters, and the path of the output directory.
    /// This is equivalent to a `host_test_args` value of
    /// ["{test_config_file}", "{output_directory}"].
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub host_test_args: Vec<String>,

    /// Path for test output.
    pub output_directory: PathBuf,

    // Processors to be applied to the output.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_processors: Vec<OutputProcessor>,

    /// Pass-through test parameters that are not known to this tool.
    #[serde(flatten)]
    pub unknown: HashMap<String, Value>,
}

impl TestConfig {
    /// Creates a new `TestConfig` from an `EnvLike`.
    pub fn from_env_like<E: EnvLike, L: Logger>(
        env_like: &E,
        schema: Schema,
        logger: &mut L,
    ) -> Result<Self, BuildError> {
        let to_return: Self = build_from_env_like(env_like, schema, logger)?;
        to_return.validate()?;
        Ok(to_return)
    }

    /// Returns the resolved host test binary path. The path is resolved by performing the
    /// parameter substitutions of parameter names surrounded with curly braces.
    pub fn resolved_host_test_binary(&self) -> PathBuf {
        PathBuf::from(
            self.format_arg(
                self.host_test_binary.to_str().expect("host test binary not valid UTF-8"),
                &PathBuf::new(),
            )
            .into_owned(),
        )
    }

    /// Returns the resolved host test arguments. Arguments are resolved by performing the
    /// parameter substitutions of parameter names surrounded with curly braces.
    pub fn resolved_host_test_args<'a>(
        &'a self,
        test_config_file_path: &'a PathBuf,
    ) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        if self.host_test_args.is_empty() {
            // Default host test arguments are <path to JSON test params> <path to output dir>.
            return Box::new(
                [
                    Cow::from(
                        test_config_file_path.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
                    ),
                    Cow::from(
                        self.output_directory.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
                    ),
                ]
                .into_iter(),
            );
        }

        Box::new(
            self.host_test_args
                .iter()
                .map(|arg| self.format_arg(arg.as_str(), test_config_file_path)),
        )
    }

    /// Validates `self`.
    fn validate(&self) -> Result<(), UsageError> {
        let host_test_binary_as_str =
            self.host_test_binary.to_str().expect("host test binary not valid UTF-8");
        self.validate_arg(host_test_binary_as_str)?;
        validate_binary_path(
            &self.resolved_host_test_binary(),
            Name::from_str(HOST_TEST_BINARY_OPTION),
        )?;

        for arg in &self.host_test_args {
            self.validate_arg(arg)?;
        }

        for output_processor in &self.output_processors {
            // Output processors disabled due to `use_if_defined` aren't invoked, so they
            // don't need to be validated and are likely invalid.
            if !output_processor
                .use_if_defined
                .iter()
                .all(|p| self.parameter_is_defined(p.as_str()))
            {
                continue;
            }

            validate_binary_path(
                &output_processor.resolved_binary(&self),
                Name::from_str(BINARY_OPTION),
            )?;

            for arg in &output_processor.args {
                self.validate_arg(arg)?;
            }
        }

        Ok(())
    }

    /// Validates a host test or output processor argument by ensuring that any substitutions
    /// refer to valid test parameters.
    fn validate_arg(&self, arg: &str) -> Result<(), UsageError> {
        let mut remaining_arg = arg;

        while let Some(open) = remaining_arg.find('{') {
            if let Some(close) = remaining_arg[open..].find('}') {
                self.validate_pattern(&remaining_arg[open + 1..open + close], arg)?;
                remaining_arg = &remaining_arg[open + close + 1..];
            } else {
                return Err(UsageError::UnterminatedArgPattern(String::from(arg)));
            }
        }

        Ok(())
    }

    /// Validates that `pattern` refers to a valid test or output processor parameter. `arg` is
    /// the argument containing the pattern and is used to construct an informative error.
    fn validate_pattern(&self, pattern: &str, arg: &str) -> Result<(), UsageError> {
        if self.parameter_is_defined(pattern) {
            Ok(())
        } else {
            Err(UsageError::UnknownArgPattern {
                unknown_parameter: String::from(pattern),
                pattern: String::from(arg),
            })
        }
    }

    /// Determines whether `parameter` is defined.
    pub fn parameter_is_defined(&self, parameter: &str) -> bool {
        match parameter {
            HOST_TEST_BINARY_OPTION | OUTPUT_DIRECTORY_OPTION | TEST_CONFIG_FILE_PATTERN => true,
            _ if self.unknown.contains_key(parameter) => true,
            _ => false,
        }
    }

    /// Formats a host test or output processor argument by performing substitutions. This
    /// function assumes that `self` has been validated.
    fn format_arg<'a>(&'a self, arg: &'a str, test_config_file_path: &PathBuf) -> Cow<'a, str> {
        if !arg.contains('{') {
            return Cow::from(arg);
        }

        let mut remaining_arg = arg;
        let mut result = String::new();

        while let Some(open) = remaining_arg.find('{') {
            let close =
                open + remaining_arg[open..].find('}').expect(RESOLVED_HOST_TEST_ARGS_EXPECT);
            result.push_str(&remaining_arg[..open]);
            result.push_str(
                self.value_for_pattern(&remaining_arg[open + 1..close], test_config_file_path),
            );
            remaining_arg = &remaining_arg[close + 1..];
        }

        result.push_str(&remaining_arg);

        result.into()
    }

    /// Returns the string value for a substitution pattern. `arg` is the argument containing
    /// the pattern. This function assumes that `self` has been validated.
    fn value_for_pattern<'a>(
        &'a self,
        pattern: &str,
        test_config_file_path: &'a PathBuf,
    ) -> &'a str {
        match pattern {
            HOST_TEST_BINARY_OPTION => {
                self.host_test_binary.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT)
            }
            OUTPUT_DIRECTORY_OPTION => {
                self.output_directory.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT)
            }
            TEST_CONFIG_FILE_PATTERN => {
                test_config_file_path.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT)
            }
            _ => self
                .unknown
                .get(pattern)
                .map(|value| match value {
                    // This special case avoids the quotes produced by `value.to_string()`.
                    Value::String(s) => s.as_str(),
                    _ => (value.as_str()).expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
                })
                .expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
        }
    }
}

/// A processor to be applied to the output.
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct OutputProcessor {
    /// Path for the executable that implements the output processor.
    pub binary: PathBuf,

    /// Arguments to pass to `binary` to run the processor. Substrings in curly braces are
    /// replaced with the value of the test parameter named in the braces. For example
    /// "--out={output_directory}" produces a single argument "--out=<foo>" where <foo> is the
    /// value of the `output_directory` test parameter. In addition, the substring
    /// "{test_config_file}" is replaced by the path to the file containing the test parameters
    /// in JSON format.
    ///
    /// If `host_test_args` is not supplied, the host test binary is invoked with two parameters:
    /// the path of the JSON file containing test parameters, and the path of the output directory.
    /// This is equivalent to a `host_test_args` value of
    /// ["{test_config_file}", "{output_directory}"].
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Whether the output processor should be invoked when the test run succeeds.
    #[serde(default)]
    pub use_on_success: bool,

    /// Whether the output processor should be invoked when the test run fails.
    #[serde(default)]
    pub use_on_failure: bool,

    /// Test parameters that must defined in order to invoke this output processor. If any
    /// parameters in the list are not defined, the invocation of this output processor is
    /// skipped without complaint.
    #[serde(default)]
    pub use_if_defined: Vec<String>,
}

impl OutputProcessor {
    pub fn resolved_binary(&self, test_config: &TestConfig) -> PathBuf {
        PathBuf::from(
            test_config
                .format_arg(
                    self.binary.to_str().expect("host test binary not valid UTF-8"),
                    &PathBuf::new(),
                )
                .into_owned(),
        )
    }

    /// Returns the output processors arguments. Arguments are resolved by performing the
    /// parameter substitutions of parameter names surrounded with curly braces.
    pub fn resolved_args<'a>(
        &'a self,
        test_config: &'a TestConfig,
        test_config_file_path: &'a PathBuf,
    ) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        if self.args.is_empty() {
            // Default host test arguments are <path to JSON test params> <path to output dir>.
            return Box::new(
                [
                    Cow::from(
                        test_config_file_path.to_str().expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
                    ),
                    Cow::from(
                        test_config
                            .output_directory
                            .to_str()
                            .expect(RESOLVED_HOST_TEST_ARGS_EXPECT),
                    ),
                ]
                .into_iter(),
            );
        }

        Box::new(
            self.args.iter().map(|arg| test_config.format_arg(arg.as_str(), test_config_file_path)),
        )
    }
}

/// Validate a binary file path, checking that the file exists and is an executable file.
fn validate_binary_path(path: &PathBuf, option: Name) -> Result<(), UsageError> {
    if !path.exists() {
        return Err(UsageError::BinaryDoesNotExist { option, path: path.clone() });
    }

    if let Ok(metadata) = fs::metadata(&path) {
        if !metadata.is_file() {
            return Err(UsageError::BinaryIsNotAFile { option, path: path.clone() });
        }
    } else {
        return Err(UsageError::BinaryUnreadable { option, path: path.clone() });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::testutils::FakeEnv;
    use crate::logger::NullLogger;
    use assert_matches::assert_matches;
    use serde_json::json;
    use tempfile::NamedTempFile;

    pub fn test_schema() -> Schema {
        Schema::from_value(
            json!({
                "type": "object",
                "properties": {
                    "foo": { "type": "string" },
                    "bar": { "type": "string" },
                    "baz": { "type": "string" },
                    "output_directory": { "type": "string" },
                    "host_test_binary": { "type": "string" },
                    "host_test_args": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "output_processors": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "binary": {
                                    "type": "string",
                                },
                                "args": {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                    },
                                },
                                "use_on_success": {
                                    "type": "boolean",
                                },
                                "use_on_failure": {
                                    "type": "boolean",
                                },
                                "use_if_defined": {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                    },
                                },
                            },
                            "required": [
                                "binary",
                            ],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": [ "output_directory", "host_test_binary" ],
                "additionalProperties": false,
            }
            ),
            PathBuf::new(),
        )
        .expect("test schema is valid")
    }

    /// Asserts that a `Result<TestConfig, BuilderError>` wraps `usage_error`
    #[track_caller]
    fn assert_usage_error(result: Result<TestConfig, BuildError>, usage_error: UsageError) {
        assert_matches!(result, Err(BuildError::IncorrectUsage(e)) if e == usage_error)
    }

    #[test]
    // Tests construction of a `TestConfig` from an environment.
    fn test_config_validate() {
        // Missing required parameter host-test-binary.
        let fake_env = FakeEnv::new("", "");
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        match result {
            Err(BuildError::ValidationMultiple(errors)) => {
                assert_matches!(
                    &errors[0],
                    BuildError::IncorrectUsage(e)
                        if e == &UsageError::MissingParameterRequiredBySchema(
                            String::from(OUTPUT_DIRECTORY_OPTION)
                        )
                );
                assert_matches!(
                    &errors[1],
                    BuildError::IncorrectUsage(e)
                        if e == &UsageError::MissingParameterRequiredBySchema(
                            String::from(HOST_TEST_BINARY_OPTION)
                        )
                );
            }
            other => {
                panic!("expected Err(BuildError::ValidationMultiple(...)), got: {other:?}");
            }
        }

        let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let temp_file_path = temp_file.path().display();

        // Missing required parameter output-directory.
        let fake_env = FakeEnv::new(format!("--host-test-binary={}", temp_file_path).as_str(), "");
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(
            result,
            UsageError::MissingParameterRequiredBySchema(String::from(OUTPUT_DIRECTORY_OPTION)),
        );

        // Valid.
        let fake_env = FakeEnv::new(
            format!("--host-test-binary={} --output-directory=/nonexistent", temp_file_path)
                .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));

        // Missing explicitly required parameter foo.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --output-directory=/nonexistent --require=foo",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(result, UsageError::MissingRequiredParameter(Name::from_str("foo")));

        // Defining explicitly prohibited parameter foo.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --output-directory=/nonexistent --prohibit=foo --foo=bar",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(result, UsageError::DefinedProhibitedParameter(Name::from_str("foo")));

        // Using host-test-args pattern that references an unknown value.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{notathing}} --output-directory=/nonexistent",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(
            result,
            UsageError::UnknownArgPattern {
                unknown_parameter: String::from("notathing"),
                pattern: String::from("{notathing}"),
            },
        );

        // Using an unterminated host-test-args pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo --output-directory=/nonexistent",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(result, UsageError::UnterminatedArgPattern(String::from("{foo")));

        // Valid, using a valid host-test-args pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}} --output-directory=/nonexistent \
                     --foo=bar",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));

        temp_file.close().expect("Failed to close temporary file");
    }

    #[test]
    // Tests `resolved_host_test_args`.
    fn test_test_params_resolved_host_test_args() {
        let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let temp_file_path = temp_file.path().display();

        // Default args.
        let fake_env = FakeEnv::new(
            format!("--host-test-binary={} --output-directory=/nonexistent", temp_file_path)
                .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result.unwrap().resolved_host_test_args(&PathBuf::from("test/params/path")).eq(vec![
                "test/params/path",
                "/nonexistent"
            ]
            .into_iter())
        );

        // Multiple args, no patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1,true,thing \
                     --output-directory=/nonexistent",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1", "true", "thing"].into_iter())
        );

        // One arg, no patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=just_one --output-directory=/nonexistent",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["just_one"].into_iter())
        );

        // Three args, one pattern in first position.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}},true,thing \
                     --output-directory=/nonexistent --foo=1",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1", "true", "thing"].into_iter())
        );

        // Three args, one pattern in second position.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1,{{foo}},thing \
                     --output-directory=/nonexistent --foo=true",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1", "true", "thing"].into_iter())
        );

        // Three args, one pattern in third position.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1,true,{{foo}} \
                     --output-directory=/nonexistent --foo=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1", "true", "thing"].into_iter())
        );

        // Three args, three patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}},{{bar}},{{baz}} \
                     --output-directory=/nonexistent --foo=1 --bar=true --baz=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1", "true", "thing"].into_iter())
        );

        // One arg with initial pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}}truething \
                     --output-directory=/nonexistent --foo=1",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with embedded pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1{{foo}}thing \
                     --output-directory=/nonexistent --foo=true",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with final pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1true{{foo}} \
                     --output-directory=/nonexistent --foo=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with two initial patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}}{{bar}}thing \
                     --output-directory=/nonexistent --foo=1 --bar=true",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with one initial and one final pattern.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}}true{{bar}} \
                     --output-directory=/nonexistent --foo=1 --bar=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with two final patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args=1{{foo}}{{bar}} \
                     --output-directory=/nonexistent --foo=true --bar=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert!(
            result
                .unwrap()
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        // One arg with three patterns.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={} --host-test-args={{foo}}{{bar}}{{baz}} \
                     --output-directory=/nonexistent --foo=1 --bar=true --baz=thing",
                temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        let test_config = result.unwrap();
        assert!(
            test_config
                .resolved_host_test_args(&PathBuf::from("test/params/path"))
                .eq(vec!["1truething"].into_iter())
        );

        assert!(test_config.parameter_is_defined("host_test_binary"));
        assert!(test_config.parameter_is_defined("output_directory"));
        assert!(test_config.parameter_is_defined("foo"));
        assert!(test_config.parameter_is_defined("bar"));
        assert!(test_config.parameter_is_defined("baz"));
        assert!(!test_config.parameter_is_defined("not_defined"));
        assert!(!test_config.parameter_is_defined("host_test_args"));

        temp_file.close().expect("Failed to close temporary file");
    }

    #[test]
    // Tests `resolved_host_test_binary`.
    fn test_resolved_host_test_binary() {
        let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let temp_file_path = temp_file.path().display();

        // Plain binary path without patterns.
        let fake_env = FakeEnv::new(
            format!("--host-test-binary={} --output-directory=/out", temp_file_path).as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.unwrap().resolved_host_test_binary(),
            PathBuf::from(format!("{}", temp_file_path))
        );

        // Pattern substitution with custom parameter.
        let fake_env = FakeEnv::new(
            format!("--host-test-binary={{foo}} --output-directory=/out --foo={}", temp_file_path)
                .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.unwrap().resolved_host_test_binary(),
            PathBuf::from(format!("{}", temp_file_path))
        );

        // Multiple pattern substitutions.
        let mut test_config = TestConfig {
            host_test_binary: PathBuf::from("{foo}/{bar}/{baz}"),
            output_directory: PathBuf::from("/out"),
            ..Default::default()
        };
        test_config.unknown.insert("foo".to_string(), Value::String("bin".to_string()));
        test_config.unknown.insert("bar".to_string(), Value::String("dir".to_string()));
        test_config.unknown.insert("baz".to_string(), Value::String("test_bin".to_string()));
        assert_eq!(test_config.resolved_host_test_binary(), PathBuf::from("bin/dir/test_bin"));

        // Substitution of output_directory.
        let test_config = TestConfig {
            host_test_binary: PathBuf::from("{output_directory}/binary"),
            output_directory: PathBuf::from("/custom_out"),
            ..Default::default()
        };
        assert_eq!(test_config.resolved_host_test_binary(), PathBuf::from("/custom_out/binary"));

        temp_file.close().expect("Failed to close temporary file");
    }

    #[test]
    // Tests `OutputProcessor::resolved_binary`.
    fn test_output_processor_resolved_binary() {
        let test_config = TestConfig {
            host_test_binary: PathBuf::from("/bin/host_test"),
            output_directory: PathBuf::from("/out/dir"),
            unknown: [
                ("processor_dir".to_string(), Value::String("/tools/bin".to_string())),
                ("processor_name".to_string(), Value::String("resummarize".to_string())),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        // Plain binary path.
        let output_processor =
            OutputProcessor { binary: PathBuf::from("/usr/bin/processor"), ..Default::default() };
        assert_eq!(
            output_processor.resolved_binary(&test_config),
            PathBuf::from("/usr/bin/processor")
        );

        // Pattern substitution with custom parameter.
        let output_processor = OutputProcessor {
            binary: PathBuf::from("{processor_dir}/{processor_name}"),
            ..Default::default()
        };
        assert_eq!(
            output_processor.resolved_binary(&test_config),
            PathBuf::from("/tools/bin/resummarize")
        );

        // Built-in parameters {host_test_binary} and {output_directory}.
        let output_processor = OutputProcessor {
            binary: PathBuf::from("{output_directory}/post_process"),
            ..Default::default()
        };
        assert_eq!(
            output_processor.resolved_binary(&test_config),
            PathBuf::from("/out/dir/post_process")
        );

        let output_processor = OutputProcessor {
            binary: PathBuf::from("{host_test_binary}_post"),
            ..Default::default()
        };
        assert_eq!(
            output_processor.resolved_binary(&test_config),
            PathBuf::from("/bin/host_test_post")
        );
    }

    #[test]
    // Tests validation of resolved binaries for host_test_binary and output_processors.
    fn test_validate_resolved_binaries() {
        let binary_temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let binary_temp_file_path = binary_temp_file.path().display().to_string();

        let processor_temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let processor_temp_file_path = processor_temp_file.path().display().to_string();

        // Host test binary with pattern resolving to an existing file passes validation.
        let fake_env = FakeEnv::new(
            format!(
                "--host-test-binary={{foo}} --output-directory=/out --foo={}",
                binary_temp_file_path
            )
            .as_str(),
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_matches!(result, Ok(_));

        // Host test binary with pattern resolving to a non-existent file fails validation.
        let fake_env = FakeEnv::new(
            "--host-test-binary={foo} --output-directory=/out --foo=/nonexistent/binary",
            "",
        );
        let result = TestConfig::from_env_like(&fake_env, test_schema(), &mut NullLogger);
        assert_usage_error(
            result,
            UsageError::BinaryDoesNotExist {
                option: Name::from_str(HOST_TEST_BINARY_OPTION),
                path: PathBuf::from("/nonexistent/binary"),
            },
        );

        // Output processor with resolved binary that exists passes validation.
        let mut test_config = TestConfig {
            host_test_binary: PathBuf::from(&binary_temp_file_path),
            output_directory: PathBuf::from("/out"),
            output_processors: vec![OutputProcessor {
                binary: PathBuf::from("{proc_path}"),
                ..Default::default()
            }],
            ..Default::default()
        };
        test_config
            .unknown
            .insert("proc_path".to_string(), Value::String(processor_temp_file_path.clone()));
        assert_matches!(test_config.validate(), Ok(()));

        // Output processor with resolved binary that does not exist fails validation.
        let mut test_config = TestConfig {
            host_test_binary: PathBuf::from(&binary_temp_file_path),
            output_directory: PathBuf::from("/out"),
            output_processors: vec![OutputProcessor {
                binary: PathBuf::from("{proc_path}"),
                ..Default::default()
            }],
            ..Default::default()
        };
        test_config
            .unknown
            .insert("proc_path".to_string(), Value::String("/nonexistent/processor".to_string()));
        assert_matches!(
            test_config.validate(),
            Err(UsageError::BinaryDoesNotExist { option, path })
                if option == Name::from_str(BINARY_OPTION) && path == PathBuf::from("/nonexistent/processor")
        );

        // Output processor with non-existent binary is skipped if use_if_defined condition is not met.
        let test_config = TestConfig {
            host_test_binary: PathBuf::from(&binary_temp_file_path),
            output_directory: PathBuf::from("/out"),
            output_processors: vec![OutputProcessor {
                binary: PathBuf::from("/nonexistent/processor"),
                use_if_defined: vec!["undefined_parameter".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_matches!(test_config.validate(), Ok(()));

        // Output processor with non-existent binary is validated and fails if use_if_defined condition is met.
        let mut test_config = TestConfig {
            host_test_binary: PathBuf::from(&binary_temp_file_path),
            output_directory: PathBuf::from("/out"),
            output_processors: vec![OutputProcessor {
                binary: PathBuf::from("/nonexistent/processor"),
                use_if_defined: vec!["defined_parameter".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        test_config
            .unknown
            .insert("defined_parameter".to_string(), Value::String("some_value".to_string()));
        assert_matches!(
            test_config.validate(),
            Err(UsageError::BinaryDoesNotExist { option, path })
                if option == Name::from_str(BINARY_OPTION) && path == PathBuf::from("/nonexistent/processor")
        );

        binary_temp_file.close().expect("Failed to close temporary file");
        processor_temp_file.close().expect("Failed to close temporary file");
    }
}
