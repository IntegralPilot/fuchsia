// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::ser::SerializeSeq as _;
use std::str::FromStr;

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Expectations {
    #[serde(rename = "actions")]
    pub expectations: Vec<Expectation>,
    pub cases_to_run: CasesToRun,
}

pub const ALL_CASES_LABEL: &str = "All";
const WITH_ERR_LOGS_CASES_LABEL: &str = "WithErrLogs";
const NO_ERR_LOGS_CASES_LABEL: &str = "NoErrLogs";

#[derive(serde::Deserialize, serde::Serialize, Debug, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CasesToRun {
    WithErrLogs,
    NoErrLogs,
    All,
}

impl FromStr for CasesToRun {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            NO_ERR_LOGS_CASES_LABEL => Ok(CasesToRun::NoErrLogs),
            WITH_ERR_LOGS_CASES_LABEL => Ok(CasesToRun::WithErrLogs),
            ALL_CASES_LABEL => Ok(CasesToRun::All),
            _ => Err(anyhow::anyhow!("Invalid CasesToRun {}", s)),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
pub struct Matchers {
    #[serde(deserialize_with = "deserialize_glob_vec", serialize_with = "serialize_glob_vec")]
    pub matchers: Vec<glob::Pattern>,
}

fn deserialize_glob_vec<'de, D>(deserializer: D) -> Result<Vec<glob::Pattern>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let strings: Vec<String> = serde::Deserialize::deserialize(deserializer)?;
    strings.into_iter().map(|s| s.parse().map_err(serde::de::Error::custom)).collect()
}

fn serialize_glob_vec<S>(globs: &[glob::Pattern], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut seq = serializer.serialize_seq(Some(globs.len()))?;
    for glob in globs {
        seq.serialize_element(glob.as_str())?;
    }
    seq.end()
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Expectation {
    ExpectFailure(Matchers),
    ExpectPass(Matchers),
    Skip(Matchers),
    ExpectFailureWithErrLogs(Matchers),
    ExpectPassWithErrLogs(Matchers),
}

impl Expectation {
    pub fn matchers(&self) -> &[glob::Pattern] {
        match self {
            Expectation::ExpectFailure(matchers)
            | Expectation::ExpectPass(matchers)
            | Expectation::Skip(matchers)
            | Expectation::ExpectFailureWithErrLogs(matchers)
            | Expectation::ExpectPassWithErrLogs(matchers) => {
                let Matchers { matchers } = matchers;
                matchers
            }
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Include {
    #[serde(rename = "include")]
    pub path: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
#[serde(untagged)]
pub enum UnmergedExpectation {
    Include(Include),
    Expectation(Expectation),
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct UnmergedExpectations {
    #[serde(rename = "actions")]
    pub expectations: Vec<UnmergedExpectation>,
}

#[cfg(test)]
mod tests {
    use crate::{CasesToRun, ALL_CASES_LABEL, NO_ERR_LOGS_CASES_LABEL, WITH_ERR_LOGS_CASES_LABEL};
    use std::str::FromStr;

    #[test]
    fn cases_to_run_from_str() {
        assert_eq!(CasesToRun::from_str(ALL_CASES_LABEL).unwrap(), CasesToRun::All);
        assert_eq!(
            CasesToRun::from_str(WITH_ERR_LOGS_CASES_LABEL).unwrap(),
            CasesToRun::WithErrLogs
        );
        assert_eq!(CasesToRun::from_str(NO_ERR_LOGS_CASES_LABEL).unwrap(), CasesToRun::NoErrLogs);
        assert!(CasesToRun::from_str("Garbage").is_err());
    }
}
