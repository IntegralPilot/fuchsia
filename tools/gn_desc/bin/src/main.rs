// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use args::Args;
use gn_graph::Graph;
use humansize::format_size;
use std::time::Instant;

mod args;
mod commands;
mod display;

fn main() -> Result<(), anyhow::Error> {
    let args: Args = argh::from_env();

    // Read all the Targets into a single map.
    if args.verbose {
        print!("parsing {}...", &args.file);

        let file_metadata = std::fs::metadata(&args.file)?;
        println!(" ({})", format_size(file_metadata.len(), humansize::WINDOWS));
    }

    let start_time = Instant::now();

    // Use the library's optimized parsing implementation for reading the file
    let all_targets = gn_json::parse_file(&args.file)?;

    if args.verbose {
        println!(
            "    found {} total targets in {:.3} seconds",
            all_targets.len(),
            start_time.elapsed().as_secs_f32()
        );
    }

    if args.verbose {
        println!("constructing build graph...");
    }
    // Create a graph of the targets
    let start_time = Instant::now();
    let graph = Graph::create_from(all_targets)?;
    if args.verbose {
        println!(
            "    added {} total target nodes with {} edges in {:.3} seconds",
            graph.targets().len(),
            graph.edges_count(),
            start_time.elapsed().as_secs_f32()
        );
    }
    let selected_nodes = args.select.select_from(&graph)?;
    for target in selected_nodes {
        args.select.perform_command(target, &graph)?;
    }
    Ok(())
}
