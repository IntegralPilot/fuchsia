// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};

/// A struct that can be json serialized to the fuchsiaperf.json format.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(into = "JsonFuchsiaPerfBenchmarkResult", try_from = "JsonFuchsiaPerfBenchmarkResult")]
pub struct FuchsiaPerfBenchmarkResult {
    pub label: String,
    pub test_suite: String,
    pub unit: Unit,
    pub direction: Direction,
    pub values: Vec<f64>,
}

/// The unit of a benchmark result.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Unit {
    #[serde(rename = "ns")]
    #[serde(alias = "nanoseconds")]
    Nanoseconds,
    #[serde(rename = "ms")]
    #[serde(alias = "milliseconds")]
    Milliseconds,
    #[serde(rename = "bytes")]
    Bytes,
    #[serde(rename = "bytes/second")]
    BytesPerSecond,
    #[serde(rename = "frames/second")]
    FramesPerSecond,
    #[serde(rename = "percent")]
    Percent,
    #[serde(rename = "count")]
    Count,
    #[serde(rename = "W")]
    Watts,
}

impl std::fmt::Display for Unit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unit::Nanoseconds => write!(f, "ns"),
            Unit::Milliseconds => write!(f, "ms"),
            Unit::Bytes => write!(f, "bytes"),
            Unit::BytesPerSecond => write!(f, "bytes/second"),
            Unit::FramesPerSecond => write!(f, "frames/second"),
            Unit::Percent => write!(f, "percent"),
            Unit::Count => write!(f, "count"),
            Unit::Watts => write!(f, "W"),
        }
    }
}

impl std::str::FromStr for Unit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ns" | "nanoseconds" => Ok(Unit::Nanoseconds),
            "ms" | "milliseconds" => Ok(Unit::Milliseconds),
            "bytes" => Ok(Unit::Bytes),
            "bytes/second" => Ok(Unit::BytesPerSecond),
            "frames/second" => Ok(Unit::FramesPerSecond),
            "percent" => Ok(Unit::Percent),
            "count" => Ok(Unit::Count),
            "W" => Ok(Unit::Watts),
            _ => Err(format!("Invalid benchmark unit: {}", s)),
        }
    }
}

/// The direction of a benchmark result. Controls which kinds of changes are considered
/// regressions or improvements.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Direction {
    #[serde(rename = "biggerIsBetter")]
    BiggerBetter,
    #[serde(rename = "smallerIsBetter")]
    SmallerBetter,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::BiggerBetter => write!(f, "biggerIsBetter"),
            Direction::SmallerBetter => write!(f, "smallerIsBetter"),
        }
    }
}

impl std::str::FromStr for Direction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "biggerIsBetter" => Ok(Direction::BiggerBetter),
            "smallerIsBetter" => Ok(Direction::SmallerBetter),
            _ => Err(format!("Invalid benchmark direction: {}", s)),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct JsonFuchsiaPerfBenchmarkResult {
    label: String,
    test_suite: String,
    unit: String,
    values: Vec<f64>,
}

impl From<FuchsiaPerfBenchmarkResult> for JsonFuchsiaPerfBenchmarkResult {
    fn from(result: FuchsiaPerfBenchmarkResult) -> Self {
        let unit = format!("{}_{}", result.unit, result.direction);
        JsonFuchsiaPerfBenchmarkResult {
            label: result.label,
            test_suite: result.test_suite,
            unit,
            values: result.values,
        }
    }
}

impl TryFrom<JsonFuchsiaPerfBenchmarkResult> for FuchsiaPerfBenchmarkResult {
    type Error = String;

    fn try_from(json: JsonFuchsiaPerfBenchmarkResult) -> Result<Self, Self::Error> {
        let (unit, direction) = if let Some((u, d)) = json.unit.rsplit_once('_') {
            (u.parse()?, d.parse()?)
        } else {
            let u: Unit = json.unit.parse()?;
            let d = match u {
                Unit::Nanoseconds
                | Unit::Milliseconds
                | Unit::Bytes
                | Unit::Count
                | Unit::Watts => Direction::SmallerBetter,
                Unit::BytesPerSecond | Unit::FramesPerSecond | Unit::Percent => {
                    Direction::BiggerBetter
                }
            };
            (u, d)
        };

        Ok(FuchsiaPerfBenchmarkResult {
            label: json.label,
            test_suite: json.test_suite,
            unit,
            direction,
            values: json.values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_serialization_with_direction() {
        let result = FuchsiaPerfBenchmarkResult {
            label: "test_label".to_string(),
            test_suite: "test_suite".to_string(),
            unit: Unit::Nanoseconds,
            direction: Direction::SmallerBetter,
            values: vec![1.0, 2.0],
        };

        let json = serde_json::to_value(&result).unwrap();
        let expected = json!({
            "label": "test_label",
            "test_suite": "test_suite",
            "unit": "ns_smallerIsBetter",
            "values": [1.0, 2.0]
        });

        assert_eq!(json, expected);
    }

    #[test]
    fn test_serialization_with_bigger_better() {
        let result = FuchsiaPerfBenchmarkResult {
            label: "test_label".to_string(),
            test_suite: "test_suite".to_string(),
            unit: Unit::BytesPerSecond,
            direction: Direction::BiggerBetter,
            values: vec![1.0, 2.0],
        };

        let json = serde_json::to_value(&result).unwrap();
        let expected = json!({
            "label": "test_label",
            "test_suite": "test_suite",
            "unit": "bytes/second_biggerIsBetter",
            "values": [1.0, 2.0]
        });

        assert_eq!(json, expected);
    }

    #[test]
    fn test_deserialization_with_direction() {
        let json = json!({
            "label": "test_label",
            "test_suite": "test_suite",
            "unit": "ns_smallerIsBetter",
            "values": [1.0, 2.0]
        });

        let result: FuchsiaPerfBenchmarkResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.unit, Unit::Nanoseconds);
        assert_eq!(result.direction, Direction::SmallerBetter);
    }

    #[test]
    fn test_deserialization_without_direction() {
        let json = json!({
            "label": "test_label",
            "test_suite": "test_suite",
            "unit": "ns",
            "values": [1.0, 2.0]
        });

        let result: FuchsiaPerfBenchmarkResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.unit, Unit::Nanoseconds);
        assert_eq!(result.direction, Direction::SmallerBetter);
    }
}
