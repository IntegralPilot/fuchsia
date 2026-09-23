// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::workloads::{
    BURST_IO_PREALLOC_SIZE, RandomArgs, SequentialArgs, TransferArgs, WorkloadSubcommand,
    get_effective_op_size,
};
use anyhow::Result;
use argh::FromArgs;

#[derive(FromArgs, Debug)]
/// Dynamic Storage IO Stressor Harness
struct TopLevelArgs {
    /// duration to run workloads in seconds (global, default: 10s)
    #[argh(option, default = "10")]
    duration_secs: u64,

    #[argh(subcommand)]
    subcommand: WorkloadSubcommand,
}

#[derive(FromArgs, Debug)]
/// Fake parent for standalone subcommand parsing
struct FakeParent {
    #[argh(subcommand)]
    subcommand: WorkloadSubcommand,
}

pub fn parse_chained_workloads(args: &[String]) -> Result<(u64, Vec<WorkloadSubcommand>)> {
    if args.is_empty() || args.len() == 1 {
        anyhow::bail!(
            "Error: No workload subcommand specified! (e.g. 'fx test io-stress-test -- random')"
        );
    }

    let chunks: Vec<&[String]> =
        args[1..].split(|arg| arg == "+").filter(|chunk| !chunk.is_empty()).collect();

    if chunks.is_empty() {
        anyhow::bail!(
            "Error: No workload subcommand specified! (e.g. 'fx test io-stress-test -- random')"
        );
    }

    let first_refs: Vec<&str> = chunks[0].iter().map(|s| s.as_str()).collect();
    let top_args = TopLevelArgs::from_args(&["io_stress"], &first_refs)
        .map_err(|err| anyhow::anyhow!("Failed to parse first subcommand: {:?}", err))?;

    let duration_secs = top_args.duration_secs;
    let mut subcommands = vec![top_args.subcommand];

    for chunk in &chunks[1..] {
        let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
        let parent = FakeParent::from_args(&["workload"], &refs)
            .map_err(|err| anyhow::anyhow!("Failed to parse chained subcommand: {:?}", err))?;
        subcommands.push(parent.subcommand);
    }

    Ok((duration_secs, subcommands))
}

pub fn validate_workloads(subcommands: &[WorkloadSubcommand]) -> Result<()> {
    for sub in subcommands {
        match sub {
            WorkloadSubcommand::Random(RandomArgs { op_size_bytes, file_size_bytes, .. })
            | WorkloadSubcommand::Sequential(SequentialArgs {
                op_size_bytes,
                file_size_bytes,
                ..
            })
            | WorkloadSubcommand::Transfer(TransferArgs {
                op_size_bytes, file_size_bytes, ..
            }) => {
                let op_size = get_effective_op_size(*op_size_bytes);
                if op_size > *file_size_bytes {
                    anyhow::bail!(
                        "Error: {} operation size ({} B) cannot exceed file size ({} B)!",
                        sub.name(),
                        op_size,
                        file_size_bytes
                    );
                }
            }
            WorkloadSubcommand::Burst(args) => {
                let op_size = get_effective_op_size(args.op_size_bytes);
                if op_size > BURST_IO_PREALLOC_SIZE {
                    anyhow::bail!(
                        "Error: Burst op size ({} B) cannot exceed pre-allocated size ({} B)!",
                        op_size,
                        BURST_IO_PREALLOC_SIZE
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chained_workloads_single() {
        let args = vec![
            "io_stress".to_string(),
            "random".to_string(),
            "--op-size-bytes".to_string(),
            "1024".to_string(),
        ];
        let (duration, subcmds) = parse_chained_workloads(&args).unwrap();
        assert_eq!(duration, 10);
        assert_eq!(subcmds.len(), 1);
        match &subcmds[0] {
            WorkloadSubcommand::Random(a) => assert_eq!(a.op_size_bytes, 1024),
            _ => panic!("Expected Random subcommand"),
        }
    }

    #[test]
    fn test_parse_chained_workloads_chained() {
        let args = vec![
            "io_stress".to_string(),
            "--duration-secs".to_string(),
            "20".to_string(),
            "random".to_string(),
            "+".to_string(),
            "sequential".to_string(),
            "--op-size-bytes".to_string(),
            "2048".to_string(),
        ];
        let (duration, subcmds) = parse_chained_workloads(&args).unwrap();
        assert_eq!(duration, 20);
        assert_eq!(subcmds.len(), 2);
        match &subcmds[0] {
            WorkloadSubcommand::Random(_) => {}
            _ => panic!("Expected Random subcommand"),
        }
        match &subcmds[1] {
            WorkloadSubcommand::Sequential(a) => assert_eq!(a.op_size_bytes, 2048),
            _ => panic!("Expected Sequential subcommand"),
        }
    }

    #[test]
    fn test_parse_chained_workloads_invalid() {
        let args = vec!["io_stress".to_string()];
        assert!(parse_chained_workloads(&args).is_err());

        let args = vec!["io_stress".to_string(), "+".to_string()];
        assert!(parse_chained_workloads(&args).is_err());
    }
}
