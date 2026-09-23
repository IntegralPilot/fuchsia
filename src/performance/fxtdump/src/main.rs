// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{ArgsInfo, FromArgs};
use fxt::session::SessionParser;
use prettytable::{Table, row};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

#[derive(FromArgs)]
/// Print information about an FXT file.
struct Args {
    #[argh(subcommand)]
    command: FxtDump,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum FxtDump {
    Dump(Dump),
    CategoryStats(CategoryStats),
    Fidl(FidlMessages),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "dump",
    description = "Dump a human readable view of the trace",
    example = "fx fxtdump dump trace.fxt"
)]
pub struct Dump {
    #[argh(positional)]
    pub input: String,
    /// if set, immediately abort when encountering an invalid trace event.
    #[argh(switch)]
    /// if set, immediately abort when encountering an invalid trace event.
    pub strict: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "category_stats",
    description = "Print information about the count and total size of each category",
    example = "fx fxtdump category_stats trace.fxt"
)]
pub struct CategoryStats {
    #[argh(positional)]
    pub input: String,
    #[argh(switch)]
    /// if set, include a breakdown of the stats by name as well
    pub detailed: bool,

    /// if set, only include the top `limit` entries
    #[argh(option)]
    pub limit: Option<usize>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "fidl",
    description = "Print information about the FIDL messages in the trace",
    example = "fx fxtdump fidl trace.fxt"
)]
pub struct FidlMessages {
    #[argh(positional)]
    pub input: String,
}

fn format_size(size: usize) -> String {
    if size > 1024 { format!("{} KiB", size / 1024) } else { format!("{} bytes", size) }
}

fn dump<R: std::io::Read>(fxt: &mut SessionParser<R>, strict: bool) {
    let mut offset = 0;
    while let Some(record) = fxt.next() {
        match record {
            Ok(trace_record) => {
                println!("{:#010x}: {:?}", offset, trace_record);
            }
            Err(parse_err) => {
                if strict {
                    eprintln!("ERROR {:#}", parse_err);
                    return;
                } else {
                    eprintln!("WARNING {:#}", parse_err);
                }
            }
        }
        offset += fxt.parsed_bytes().len();
    }
}

#[derive(Default)]
struct Stats {
    count: usize,
    size_bytes: usize,
}

fn category_stats<R: std::io::Read>(
    fxt: &mut SessionParser<R>,
    detailed: bool,
    limit: Option<usize>,
) {
    let mut category_name_counts = HashMap::<String, HashMap<String, Stats>>::new();

    while let Some(record) = fxt.next() {
        match record {
            Ok(trace_record) => {
                let (category, name) = match trace_record {
                    fxt::TraceRecord::Event(event_record) => {
                        (event_record.category.as_str().into(), event_record.name.as_str().into())
                    }
                    fxt::TraceRecord::Scheduling(_scheduling_record) => {
                        ("kernel_sched".into(), "kernel:sched".into())
                    }
                    fxt::TraceRecord::Blob(blob_record) => {
                        (blob_record.name.as_str().into(), blob_record.name.as_str().into())
                    }
                    fxt::TraceRecord::LargeBlob(large_blob_record) => (
                        large_blob_record.category.as_str().into(),
                        large_blob_record.name.as_str().into(),
                    ),
                    fxt::TraceRecord::KernelObj(_kernel_obj_record) => {
                        ("kernel:meta".into(), "kernel:meta".into())
                    }
                    fxt::TraceRecord::UserspaceObj(_user_obj_record) => {
                        ("user:meta".into(), "user:meta".into())
                    }
                    fxt::TraceRecord::Log(_log_record) => ("log".into(), "log".into()),
                    fxt::TraceRecord::ProviderEvent { .. } => {
                        ("metadata".into(), "metadata".into())
                    }
                    fxt::TraceRecord::Profiler(_) => todo!(),
                };
                let name_map = category_name_counts.entry(category).or_insert(HashMap::new());
                let stats = name_map.entry(name).or_insert(Stats::default());
                stats.count += 1;
                stats.size_bytes += fxt.parsed_bytes().len();
            }
            Err(parse_err) => eprintln!("WARNING {:#}", parse_err),
        }
    }
    if detailed {
        let mut category_stats: Vec<_> = category_name_counts
            .into_iter()
            .map(|(category, name_map)| {
                let total_stats = name_map.values().fold(Stats::default(), |s1, s2| Stats {
                    count: s1.count + s2.count,
                    size_bytes: s1.size_bytes + s2.size_bytes,
                });
                let mut name_stats: Vec<_> = name_map.into_iter().collect();
                name_stats.sort_by(|a, b| b.1.size_bytes.cmp(&a.1.size_bytes));
                (category, total_stats, name_stats)
            })
            .collect();

        category_stats.sort_by(|a, b| b.1.size_bytes.cmp(&a.1.size_bytes));

        let num_categories = limit.unwrap_or(category_stats.len());
        for (category, total_stats, name_stats) in category_stats.into_iter().take(num_categories) {
            let mut table = Table::new();
            table.add_row(row![
                format!("Name (Category: {})", category),
                format!("Size  (Total: {})", format_size(total_stats.size_bytes)),
                format!("Count (Total: {})", total_stats.count)
            ]);
            let mut sorted_name_stats: Vec<_> = name_stats.into_iter().collect();
            sorted_name_stats.sort_by(|a, b| b.1.size_bytes.cmp(&a.1.size_bytes));
            let num_names = limit.unwrap_or(sorted_name_stats.len());
            for (name, stats) in sorted_name_stats.into_iter().take(num_names) {
                table.add_row(row![name, format_size(stats.size_bytes), stats.count]);
            }
            table.printstd();
        }
    } else {
        let mut flattened_counts: Vec<_> = category_name_counts
            .into_iter()
            .map(|(name, stats)| {
                (
                    name,
                    stats.into_iter().fold(Stats::default(), |s1, (_, s2)| Stats {
                        count: s1.count + s2.count,
                        size_bytes: s1.size_bytes + s2.size_bytes,
                    }),
                )
            })
            .collect();
        flattened_counts.sort_by(|a, b| b.1.size_bytes.cmp(&a.1.size_bytes));

        let mut table = Table::new();
        table.add_row(row!["Category", "Size", "Count"]);

        let num_categories = limit.unwrap_or(flattened_counts.len());
        for (category, Stats { count, size_bytes }) in
            flattened_counts.into_iter().take(num_categories)
        {
            table.add_row(row![category, format_size(size_bytes), count]);
        }
        table.printstd();
    }
}

fn fidl_messages<R: std::io::Read>(fxt: &mut SessionParser<R>) {
    let mut name_counts = std::collections::HashMap::new();
    for record in fxt {
        match record {
            Ok(fxt::TraceRecord::Event(fxt::EventRecord {
                payload: fxt::EventPayload::DurationBegin,
                category,
                name,
                ..
            })) => {
                if category == "kernel:ipc" && name != "ChannelMessage" {
                    *name_counts.entry(name.as_str().to_string()).or_insert(0) += 1;
                }
            }
            Ok(_) => {}
            Err(parse_err) => eprintln!("WARNING {:#}", parse_err),
        }
    }

    let mut counts: Vec<_> = name_counts.into_iter().collect();
    counts.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    for (name, count) in counts {
        println!("{} {}", count, name);
    }
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    match args.command {
        FxtDump::CategoryStats(CategoryStats { input, detailed, limit }) => {
            let file = File::open(input)?;
            let mut reader = BufReader::new(file);
            let mut session = SessionParser::new(&mut reader);
            category_stats(&mut session, detailed, limit)
        }
        FxtDump::Dump(Dump { input, strict }) => {
            let file = File::open(input)?;
            let mut reader = BufReader::new(file);
            let mut session = SessionParser::new(&mut reader);
            dump(&mut session, strict)
        }
        FxtDump::Fidl(FidlMessages { input }) => {
            let file = File::open(input)?;
            let mut reader = BufReader::new(file);
            let mut session = SessionParser::new(&mut reader);
            fidl_messages(&mut session)
        }
    }

    Ok(())
}
