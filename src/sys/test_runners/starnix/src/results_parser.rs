// Copyright 2023 The Fuchsia Authors.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::iter::Peekable;

use crate::helpers::TestType;
use anyhow::{Error, anyhow};
use gtest_runner_lib::parser::{
    Failure, IndividualTestOutput, IndividualTestOutputStatus, TestOutput, TestSuiteOutput,
};
use xml::reader::{EventReader, XmlEvent};

pub fn parse_results(test_type: TestType, contents: &str) -> Result<TestOutput, Error> {
    match test_type {
        TestType::Gtest | TestType::Gunit | TestType::SyscallGtest => {
            serde_json::from_str(contents).map_err(|e| anyhow!("JSON parsing error: {e}"))
        }
        TestType::GtestXmlOutput => {
            let mut errors = vec![];
            let mut iter = EventReader::new(contents.as_bytes())
                .into_iter()
                .filter(|p| match p {
                    Ok(
                        XmlEvent::Whitespace(..) | XmlEvent::Doctype { .. } | XmlEvent::Comment(..),
                    ) => false,
                    _ => true,
                })
                .map_while(|r| match r {
                    Ok(event) => Some(event),
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                })
                .peekable();
            parse_xml_test_output(&mut iter)
                .map_err(|e| anyhow!("Parsing error: {e} errors from XML parser: {errors:?}"))
        }
        _ => panic!("Do not know how to parse results for test type."),
    }
}

fn parse_xml_test_output<I>(iter: &mut Peekable<I>) -> Result<TestOutput, Error>
where
    I: Iterator<Item = XmlEvent>,
{
    match iter.next() {
        Some(XmlEvent::StartDocument { .. }) => {}
        _ => {
            return Err(anyhow!("Expected document start event"));
        }
    }
    if let Some(XmlEvent::StartElement { name, .. }) = iter.next() {
        if name.borrow().local_name != "testsuites" {
            return Err(anyhow!("Expected <testsuites> saw ${name}"));
        }

        Ok(TestOutput { testsuites: parse_xml_testsuites(iter)? })
    } else {
        Err(anyhow!("Expected <testsuites>"))
    }
}

fn parse_xml_testsuites<I>(iter: &mut Peekable<I>) -> Result<Vec<TestSuiteOutput>, Error>
where
    I: Iterator<Item = XmlEvent>,
{
    let mut suites = vec![];
    loop {
        if let Some(XmlEvent::StartElement { name, attributes, .. }) = iter.next() {
            if name.borrow().local_name != "testsuite" {
                return Err(anyhow!("Expected <testsuite> got {name}"));
            }
            let mut output = TestSuiteOutput::default();
            for a in attributes {
                match a.name.local_name.as_str() {
                    "name" => {
                        output.name = a.value.to_string();
                    }
                    "tests" => {
                        output.tests = a
                            .value
                            .parse::<usize>()
                            .map_err(|e| anyhow!("Unable to parse number of tests: {e}"))?;
                    }
                    "failures" => {
                        output.failures = a
                            .value
                            .parse::<usize>()
                            .map_err(|e| anyhow!("Unable to parse number of failures: {e}"))?;
                    }
                    "time" => {
                        output.time = a.value.to_string();
                    }
                    _ => {}
                }
            }
            output.testsuite = parse_xml_testsuite(iter)?;
            suites.push(output);
        } else {
            return Ok(suites);
        }
    }
}

fn parse_xml_testsuite<I>(iter: &mut Peekable<I>) -> Result<Vec<IndividualTestOutput>, Error>
where
    I: Iterator<Item = XmlEvent>,
{
    let mut outputs = vec![];
    loop {
        match iter.peek() {
            Some(XmlEvent::StartElement { .. }) => {
                outputs.push(parse_xml_testcase(iter)?);
            }
            Some(XmlEvent::EndElement { .. }) => {
                iter.next();
                break;
            }
            _ => {
                return Err(anyhow!("Expected <testcase> or </testsuite>"));
            }
        }
    }
    Ok(outputs)
}

fn parse_xml_testcase<I>(iter: &mut Peekable<I>) -> Result<IndividualTestOutput, Error>
where
    I: Iterator<Item = XmlEvent>,
{
    if let Some(XmlEvent::StartElement { name, attributes, .. }) = iter.next() {
        if name.local_name != "testcase" {
            return Err(anyhow!("Expected <testcase> saw {name}"));
        }

        let mut output = IndividualTestOutput::default();

        // The XML format does not include a "result" field. Pretend that it's completed.
        output.result = "COMPLETED".to_string();

        for a in attributes {
            match a.name.local_name.as_str() {
                "name" => {
                    output.name = a.value.to_string();
                }
                "time" => {
                    output.time = a.value.to_string();
                }
                "status" => {
                    output.status = match a.value.as_str() {
                        "run" => IndividualTestOutputStatus::Run,
                        _ => IndividualTestOutputStatus::NotRun,
                    }
                }
                _ => {}
            }
        }
        loop {
            match iter.peek() {
                Some(XmlEvent::EndElement { .. }) => {
                    iter.next();
                    break;
                }
                Some(XmlEvent::StartElement { .. }) => {
                    output.failures = Some(parse_xml_failures(iter)?);
                }
                _ => {
                    return Err(anyhow!("Expected <failure> or end of <testcase>"));
                }
            }
        }
        Ok(output)
    } else {
        Err(anyhow!("Expected <testcase>"))
    }
}

fn parse_xml_failures<I>(iter: &mut Peekable<I>) -> Result<Vec<Failure>, Error>
where
    I: Iterator<Item = XmlEvent>,
{
    let mut failures = vec![];
    loop {
        if let Some(XmlEvent::StartElement { name, attributes, .. }) = iter.peek() {
            if name.local_name.as_str() != "failure" {
                return Err(anyhow!("Expected <failure> got {name}"));
            }
            let mut failure = Failure::default();
            for a in attributes {
                if a.name.local_name == "message" {
                    failure.failure = a.value.to_string();
                }
            }
            iter.next();
            if let Some(XmlEvent::EndElement { .. }) = iter.next() {
                failures.push(failure);
            } else {
                return Err(anyhow!("Expected </failure>"));
            }
        } else {
            break;
        }
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use crate::helpers::TestType;
    use gtest_runner_lib::parser::*;

    use super::parse_results;

    fn expected_results() -> TestOutput {
        TestOutput {
            testsuites: vec![
                TestSuiteOutput {
                    name: "MathTest".to_string(),
                    tests: 2,
                    failures: 1,
                    disabled: 0,
                    time: "0.015s".to_string(),
                    testsuite: vec![
                        IndividualTestOutput {
                            name: "Addition".to_string(),
                            status: IndividualTestOutputStatus::Run,
                            time: "0.007s".to_string(),
                            failures: Some(vec![
                                Failure {
                                    failure: "Value of: add(1, 1)\n  Actual: 3\n  Expected: 2"
                                        .to_string(),
                                },
                                Failure {
                                    failure: "Value of: add(1, -1)\n  Actual: 1\n  Expected: 0"
                                        .to_string(),
                                },
                            ]),
                            result: "COMPLETED".to_string(),
                        },
                        IndividualTestOutput {
                            name: "Subtraction".to_string(),
                            status: IndividualTestOutputStatus::Run,
                            time: "0.005s".to_string(),
                            failures: None,
                            result: "COMPLETED".to_string(),
                        },
                    ],
                },
                TestSuiteOutput {
                    name: "LogicTest".to_string(),
                    tests: 1,
                    failures: 0,
                    disabled: 0,
                    time: "0.005s".to_string(),
                    testsuite: vec![IndividualTestOutput {
                        name: "NonContradiction".to_string(),
                        status: IndividualTestOutputStatus::Run,
                        time: "0.005s".to_string(),
                        failures: None,
                        result: "COMPLETED".to_string(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn parse_json_results() {
        let results_json = r#"
      {
        "tests": 3,
        "failures": 1,
        "disabled": 0,
        "errors": 0,
        "time": "0.035s",
        "timestamp": "2011-10-31T18:52:42Z",
        "name": "AllTests",
        "testsuites": [
          {
            "name": "MathTest",
            "tests": 2,
            "failures": 1,
            "disabled": 0,
            "errors": 0,
            "time": "0.015s",
            "testsuite": [
              {
                "name": "Addition",
                "status": "RUN",
                "result": "COMPLETED",
                "time": "0.007s",
                "classname": "",
                "failures": [
                  {
                    "failure": "Value of: add(1, 1)\n  Actual: 3\n  Expected: 2",
                    "type": ""
                  },
                  {
                    "failure": "Value of: add(1, -1)\n  Actual: 1\n  Expected: 0",
                    "type": ""
                  }
                ]
              },
              {
                "name": "Subtraction",
                "status": "RUN",
                "result": "COMPLETED",
                "time": "0.005s",
                "classname": ""
              }
            ]
          },
          {
            "name": "LogicTest",
            "tests": 1,
            "failures": 0,
            "disabled": 0,
            "errors": 0,
            "time": "0.005s",
            "testsuite": [
              {
                "name": "NonContradiction",
                "status": "RUN",
                "result": "COMPLETED",
                "time": "0.005s",
                "classname": ""
              }
            ]
          }
        ]
      }
    "#;

        let results = parse_results(TestType::Gtest, results_json).unwrap();
        assert_eq!(results, expected_results());
    }

    #[test]
    fn parse_xml_results() {
        let results_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="AllTests" tests="3" failures="1" disabled="0" errors="0"
            timestamp="2011-10-31T18:52:42Z">
  <testsuite name="MathTest" tests="2" failures="1" disabled="0" errors="0" time="0.015s">
    <testcase name="Addition" status="run" time="0.007s" classname="addition">
      <failure message="Value of: add(1, 1)&#xA;  Actual: 3&#xA;  Expected: 2" type=""></failure>
      <failure message="Value of: add(1, -1)&#xA;  Actual: 1&#xA;  Expected: 0" type=""></failure>
    </testcase>
    <testcase name="Subtraction" status="run" time="0.005s" classname="subtraction" />
  </testsuite>
  <testsuite name="LogicTest" tests="1" failures="0" disabled="0" errors="0" time="0.005s">
    <testcase name="NonContradiction" status="run" time="0.005s" classname="logic_test" />
  </testsuite>
</testsuites>
    "#;

        let results = parse_results(TestType::GtestXmlOutput, results_xml).unwrap();
        assert_eq!(results, expected_results());
    }
}
