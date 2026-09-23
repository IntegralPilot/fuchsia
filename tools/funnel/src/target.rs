// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use addr::TargetAddr;
use anyhow::Result;
use std::io::{BufRead, Write};
use thiserror::Error;

use crate::errors::IntoExitCode;

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct TargetInfo {
    pub(crate) nodename: String,
    pub(crate) addresses: Vec<TargetAddr>,
    pub(crate) mode: TargetMode,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum TargetMode {
    Fastboot,
    #[default]
    Product,
}

impl std::fmt::Display for TargetInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let addrs = self.addresses.iter().map(|x| format!("{}", x)).collect::<Vec<_>>().join(",");
        write!(f, "{}\t{}", self.nodename, addrs)
    }
}

#[derive(Debug, Error)]
pub enum ChooseTargetError {
    #[error(
        "No valid targets discovered.\nEnsure your Target is in Product mode or Fastboot Over TCP"
    )]
    NoValidTargets,

    #[error("Internal error: list of targets was length 1, but the `first` one was None.")]
    InvalidState,

    #[error("Specified target ({}) does not exist", .0)]
    DoesNotExist(String),

    #[error("Invalid choice: {}", .0)]
    OutOfRangeChoice(usize),

    #[error("Choice {} was not an unsigned integer", .0)]
    InvalidChoice(String),

    #[error("Could not get target {} from collection", .0)]
    CollectionError(usize),

    #[error("{}", .0)]
    IoError(#[from] std::io::Error),
}

impl IntoExitCode for ChooseTargetError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::NoValidTargets => 80,
            Self::InvalidState => 81,
            Self::DoesNotExist(_) => 82,
            Self::OutOfRangeChoice(_) => 83,
            Self::InvalidChoice(_) => 84,
            Self::CollectionError(_) => 85,
            Self::IoError(e) => e.raw_os_error().unwrap_or(86),
        }
    }
}

pub async fn choose_target<R, W>(
    input: &mut R,
    output: &mut W,
    targets: Vec<TargetInfo>,
    def: Option<String>,
) -> Result<TargetInfo, ChooseTargetError>
where
    R: BufRead,
    W: Write,
{
    // If they specified a target...
    if let Some(t) = def {
        let filtered_targets =
            targets.iter().filter(|x| x.nodename == t).cloned().collect::<Vec<TargetInfo>>();
        let found_target = filtered_targets.first();
        if found_target.is_none() {
            return Err(ChooseTargetError::DoesNotExist(t));
        }
        return Ok(found_target.cloned().unwrap());
    }

    match targets.len() {
        0 => return Err(ChooseTargetError::NoValidTargets),
        1 => {
            let first = targets.first();
            match first {
                Some(f) => Ok(f.clone()),
                None => Err(ChooseTargetError::InvalidState),
            }
        }
        _ => {
            // Okay there is more than one target available and they haven't
            // specified a target to use, lets prompt them for one
            prompt_for_target(input, output, targets).await
        }
    }
}

async fn prompt_for_target<R, W>(
    mut input: R,
    mut output: W,
    targets: Vec<TargetInfo>,
) -> Result<TargetInfo, ChooseTargetError>
where
    R: BufRead,
    W: Write,
{
    writeln!(output, "Multiple devices detected. Please choose the target by number:")?;
    for (i, t) in targets.iter().enumerate() {
        let name = t.clone().nodename;
        writeln!(output, "  {i}: {name}")?;
    }
    write!(output, "Choice: ")?;
    output.flush()?;

    let mut choice = String::new();
    input.read_line(&mut choice)?;
    let idx: usize = choice.trim().parse().map_err(|_| ChooseTargetError::InvalidChoice(choice))?;
    if idx >= targets.len() {
        return Err(ChooseTargetError::OutOfRangeChoice(idx));
    }

    targets.get(idx).ok_or(ChooseTargetError::CollectionError(idx)).cloned()
}

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[fuchsia::test]
    async fn test_prompt_for_first_target() -> Result<()> {
        let input = b"0";
        let output = Vec::new();
        let targets = vec![
            TargetInfo { nodename: "cytherera".to_string(), ..Default::default() },
            TargetInfo { nodename: "alecto".to_string(), ..Default::default() },
        ];

        let res = prompt_for_target(&input[..], output, targets).await?;
        assert_eq!(res, TargetInfo { nodename: "cytherera".to_string(), ..Default::default() });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_prompt_for_last_target() -> Result<()> {
        let input = b"1";
        let output = Vec::new();
        let targets = vec![
            TargetInfo { nodename: "cytherera".to_string(), ..Default::default() },
            TargetInfo { nodename: "alecto".to_string(), ..Default::default() },
        ];

        let res = prompt_for_target(&input[..], output, targets).await?;
        assert_eq!(res, TargetInfo { nodename: "alecto".to_string(), ..Default::default() });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_prompt_for_negative_target() -> Result<()> {
        let input = b"-1";
        let output = Vec::new();
        let targets = vec![
            TargetInfo { nodename: "cytherera".to_string(), ..Default::default() },
            TargetInfo { nodename: "alecto".to_string(), ..Default::default() },
        ];

        let res = prompt_for_target(&input[..], output, targets).await;
        assert!(res.is_err());
        assert_eq!(format!("{}", res.unwrap_err()), "Choice -1 was not an unsigned integer");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_prompt_for_target_not_usize_error() -> Result<()> {
        let input = b"asdf";
        let output = Vec::new();
        let targets = vec![
            TargetInfo { nodename: "cytherera".to_string(), ..Default::default() },
            TargetInfo { nodename: "alecto".to_string(), ..Default::default() },
        ];

        let res = prompt_for_target(&input[..], output, targets).await;
        assert!(res.is_err());
        assert_eq!(format!("{}", res.unwrap_err()), "Choice asdf was not an unsigned integer");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_prompt_for_target_out_of_range_error() -> Result<()> {
        let input = b"2";
        let output = Vec::new();
        let targets = vec![
            TargetInfo { nodename: "cytherera".to_string(), ..Default::default() },
            TargetInfo { nodename: "alecto".to_string(), ..Default::default() },
        ];

        let res = prompt_for_target(&input[..], output, targets).await;
        assert!(res.is_err());
        assert_eq!(format!("{}", res.unwrap_err()), "Invalid choice: 2");
        Ok(())
    }
}
