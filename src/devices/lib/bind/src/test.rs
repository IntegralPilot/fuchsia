// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compiler::{SymbolTable, SymbolicInstructionInfo, compiler};
use crate::debugger::device_specification::DeviceSpecification;
use crate::debugger::offline_debugger::{self, debug_from_device_specification};
use crate::errors::UserError;
use crate::parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;
use valico::json_schema;

const BIND_SCHEMA: &str = include_str!("../tests_schema.json");
const COMPOSITE_SCHEMA: &str = include_str!("../composite_tests_schema.json");

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ExpectedResult {
    Match,
    Abort,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct BindSpec {
    name: String,
    expected: ExpectedResult,
    device: HashMap<String, String>,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct CompositeParentSpec {
    #[serde(alias = "node")]
    pub parent: String,
    pub tests: Vec<BindSpec>,
}

#[derive(Deserialize, Debug)]
enum TestSpec {
    Bind(Vec<BindSpec>),
    CompositeBind(Vec<CompositeParentSpec>),
}

struct TestSuite {
    specs: TestSpec,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum TestError {
    BindParserError(parser::common::BindParserError),
    DeviceSpecParserError(parser::common::BindParserError),
    DebuggerError(offline_debugger::DebuggerError),
    CompilerError(compiler::CompilerError),
    InvalidSchema,
    JsonParserError(String),
    CompositeParentMissing(String),
    // The JSON validator unfortunately doesn't produce useful error messages.
    InvalidJsonError,
}

pub fn run(rules: &str, libraries: &[String], tests: &str) -> Result<bool, TestError> {
    TestSuite::try_from(tests).and_then(|t| t.run(rules, libraries))
}

impl TestSuite {
    fn run(&self, rules: &str, libraries: &[String]) -> Result<bool, TestError> {
        match &self.specs {
            TestSpec::Bind(test_specs) => {
                let bind_rules =
                    compiler::compile_bind(rules, libraries, false, false, false, false)
                        .map_err(TestError::CompilerError)?;
                run_bind_test_specs(test_specs, &bind_rules.symbol_table, &bind_rules.instructions)
            }
            TestSpec::CompositeBind(test_specs) => {
                run_composite_bind_test_specs(test_specs, rules, libraries)
            }
        }
    }
}

impl TryFrom<&str> for TestSuite {
    type Error = TestError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        // Try the non-composite bind test spec.
        let bind_specs = parse_bind_spec(input);
        if bind_specs.is_ok() {
            return bind_specs;
        }

        // Try the composite bind test spec.
        let composite_specs: Vec<CompositeParentSpec> =
            serde_json::from_value(validate_spec(input, &COMPOSITE_SCHEMA)?)
                .map_err(|e| TestError::JsonParserError(format!("{}", e)))?;
        Ok(TestSuite { specs: TestSpec::CompositeBind(composite_specs) })
    }
}

fn parse_bind_spec(input: &str) -> Result<TestSuite, TestError> {
    let bind_specs = serde_json::from_value::<Vec<BindSpec>>(validate_spec(input, &BIND_SCHEMA)?)
        .map_err(|e| TestError::JsonParserError(format!("{}", e)))?;
    Ok(TestSuite { specs: TestSpec::Bind(bind_specs) })
}

fn validate_spec(input: &str, schema: &str) -> Result<serde_json::Value, TestError> {
    let schema =
        serde_json::from_str(&schema).map_err(|e| TestError::JsonParserError(format!("{}", e)))?;
    let mut scope = json_schema::Scope::new();
    let compiled_schema =
        scope.compile_and_return(schema, false).map_err(|_| TestError::InvalidSchema)?;

    let spec =
        serde_json::from_str(input).map_err(|e| TestError::JsonParserError(format!("{}", e)))?;

    let res = compiled_schema.validate(&spec);
    if !res.is_strictly_valid() {
        return Err(TestError::InvalidJsonError);
    }

    Ok(spec)
}

fn run_bind_test_specs<'a>(
    specs: &Vec<BindSpec>,
    symbol_table: &SymbolTable,
    instructions: &Vec<SymbolicInstructionInfo<'a>>,
) -> Result<bool, TestError> {
    println!("[----------]");
    for test in specs {
        let mut device_specification = DeviceSpecification::new();
        println!("[ RUN      ] {}", test.name);
        for (key, value) in &test.device {
            let result = device_specification
                .add_property(&key, &value)
                .map_err(TestError::DeviceSpecParserError);
            if let Err(e) = result {
                println!("Failed to add specification {key}:{value}");
                return Err(e);
            }
        }

        let result =
            debug_from_device_specification(symbol_table, instructions, device_specification)
                .map_err(TestError::DebuggerError)?;
        match (&test.expected, result) {
            (ExpectedResult::Match, false) | (ExpectedResult::Abort, true) => {
                println!("[  FAILED  ] {}", test.name);
                println!("[----------]");
                return Ok(false);
            }
            _ => (),
        }
        println!("[       OK ] {}", test.name);
    }
    println!("[  PASSED  ]");
    println!("[----------]");
    Ok(true)
}

fn run_composite_bind_test_specs(
    specs: &Vec<CompositeParentSpec>,
    rules: &str,
    libraries: &[String],
) -> Result<bool, TestError> {
    let composite_bind = compiler::compile_bind_composite(rules, libraries, false, false, false)
        .map_err(TestError::CompilerError)?;

    // Map composite bind rules by parent name.
    let mut parent_map: HashMap<String, Vec<SymbolicInstructionInfo>> = HashMap::new();
    parent_map
        .insert(composite_bind.primary_parent.name, composite_bind.primary_parent.instructions);
    for parent in composite_bind.additional_parents {
        parent_map.insert(parent.name, parent.instructions);
    }

    for parent in composite_bind.optional_parents {
        parent_map.insert(parent.name, parent.instructions);
    }

    println!("[==========]");
    for parent_spec in specs {
        println!("[ RUN      ] Test for composite parent {}", parent_spec.parent);
        if !parent_map.contains_key(&parent_spec.parent) {
            println!("[  FAILED  ] {}", parent_spec.parent);
            println!("[==========]");
            return Err(TestError::CompositeParentMissing(parent_spec.parent.clone()));
        }

        if !run_bind_test_specs(
            &parent_spec.tests,
            &composite_bind.symbol_table,
            parent_map.get(&parent_spec.parent).unwrap(),
        )? {
            println!("[  FAILED  ] {}", parent_spec.parent);
            println!("[==========]");
            return Ok(false);
        }
        println!("[       OK ] {}", parent_spec.parent);
    }
    println!("[  SUCCESS  ] ");
    println!("[==========]");
    Ok(true)
}

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", UserError::from(self.clone()))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn parse_one_test() {
        let TestSuite { specs } = TestSuite::try_from(
            r#"
[
    {
        "name": "A test",
        "expected": "match",
        "device": {
            "key": "value"
        }
    }
]"#,
        )
        .unwrap();

        assert_matches!(specs, TestSpec::Bind(_));

        if let TestSpec::Bind(specs) = specs {
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].name, "A test".to_string());
            assert_eq!(specs[0].expected, ExpectedResult::Match);

            let mut expected_device = HashMap::new();
            expected_device.insert("key".to_string(), "value".to_string());
            assert_eq!(specs[0].device, expected_device);
        }
    }

    #[test]
    fn parse_two_tests() {
        let TestSuite { specs } = TestSuite::try_from(
            r#"
    [
        {
            "name": "A test",
            "expected": "match",
            "device": {
                "key": "value"
            }
        },
        {
            "name": "A second test",
            "expected": "abort",
            "device": {
                "key1": "value1",
                "key2": "value2"
            }
        }
    ]"#,
        )
        .unwrap();

        assert_matches!(specs, TestSpec::Bind(_));
        if let TestSpec::Bind(specs) = specs {
            assert_eq!(specs.len(), 2);
            assert_eq!(specs[0].name, "A test".to_string());
            assert_eq!(specs[0].expected, ExpectedResult::Match);
            assert_eq!(specs[1].name, "A second test".to_string());
            assert_eq!(specs[1].expected, ExpectedResult::Abort);

            let mut expected_device = HashMap::new();
            expected_device.insert("key".to_string(), "value".to_string());
            assert_eq!(specs[0].device, expected_device);

            let mut expected_device2 = HashMap::new();
            expected_device2.insert("key1".to_string(), "value1".to_string());
            expected_device2.insert("key2".to_string(), "value2".to_string());
            assert_eq!(specs[1].device, expected_device2);
        }
    }

    #[test]
    fn parse_one_composite_parent() {
        let TestSuite { specs } = TestSuite::try_from(
            r#"
            [
                {
                    "node": "honeycreeper",
                    "tests": [
                        {
                            "name": "tanager",
                            "expected": "match",
                            "device": {
                                "grassquit": "blue-black",
                                "flowerpiercer": "moustached"
                            }
                        }
                    ]
                }
            ]"#,
        )
        .unwrap();

        let mut expected_device = HashMap::new();
        expected_device.insert("grassquit".to_string(), "blue-black".to_string());
        expected_device.insert("flowerpiercer".to_string(), "moustached".to_string());

        let expected_specs = CompositeParentSpec {
            parent: "honeycreeper".to_string(),
            tests: vec![BindSpec {
                name: "tanager".to_string(),
                expected: ExpectedResult::Match,
                device: expected_device,
            }],
        };

        assert_matches!(specs, TestSpec::CompositeBind(_));
        if let TestSpec::CompositeBind(parent_specs) = specs {
            assert_eq!(1, parent_specs.len());
            assert_eq!(expected_specs, parent_specs[0]);
        }
    }

    #[test]
    fn parse_multiple_composite_parent() {
        let TestSuite { specs } = TestSuite::try_from(
            r#"
            [
                {
                    "node": "honeycreeper",
                    "tests": [
                        {
                            "name": "tanager",
                            "expected": "match",
                            "device": {
                                "grassquit": "blue-black",
                                "flowerpiercer": "moustached"
                            }
                        }
                    ]
                },
                {
                    "node": "ground-roller",
                    "tests": [
                        {
                            "name": "kingfisher",
                            "expected": "match",
                            "device": {
                                "bee-eater": "little"
                            }
                        }
                    ]
                }
            ]"#,
        )
        .unwrap();

        let mut expected_device_1 = HashMap::new();
        expected_device_1.insert("grassquit".to_string(), "blue-black".to_string());
        expected_device_1.insert("flowerpiercer".to_string(), "moustached".to_string());

        let mut expected_device_2 = HashMap::new();
        expected_device_2.insert("bee-eater".to_string(), "little".to_string());

        let expected_specs = [
            CompositeParentSpec {
                parent: "honeycreeper".to_string(),
                tests: vec![BindSpec {
                    name: "tanager".to_string(),
                    expected: ExpectedResult::Match,
                    device: expected_device_1,
                }],
            },
            CompositeParentSpec {
                parent: "ground-roller".to_string(),
                tests: vec![BindSpec {
                    name: "kingfisher".to_string(),
                    expected: ExpectedResult::Match,
                    device: expected_device_2,
                }],
            },
        ];

        assert_matches!(specs, TestSpec::CompositeBind(_));
        if let TestSpec::CompositeBind(parent_specs) = specs {
            assert_eq!(expected_specs.len(), parent_specs.len());
            for i in 0..expected_specs.len() {
                assert_eq!(expected_specs[i], parent_specs[i]);
            }
        }
    }

    #[test]
    fn parse_json_failure() {
        match TestSuite::try_from("this can't be parsed") {
            Err(TestError::JsonParserError(_)) => (),
            _ => panic!("Expected a JsonParserError"),
        }
    }

    #[test]
    fn parse_invalid_json() {
        match TestSuite::try_from(r#"{ "valid json": "invalid to spec" }"#) {
            Err(TestError::InvalidJsonError) => (),
            _ => panic!("Expected a InvalidJsonError"),
        };
    }

    #[test]
    fn test_missing_parent() {
        let test_suite = TestSuite::try_from(
            r#"
            [
                {
                    "node": "tyrant",
                    "tests": [
                        {
                            "name": "tyrannulet",
                            "expected": "match",
                            "device": {
                                "water-tyrant": "pied"
                            }
                        }
                    ]
                }
            ]"#,
        )
        .unwrap();

        let composite_bind_rules = "composite flycatcher;
            primary parent \"pewee\" {
                fuchsia.BIND_PROTOCOL == 1;
            }
            parent \"phoebe\" {
                fuchsia.BIND_PROTOCOL == 2;
            }";

        assert_eq!(
            Err(TestError::CompositeParentMissing("tyrant".to_string())),
            test_suite.run(composite_bind_rules, &[])
        );
    }
}
