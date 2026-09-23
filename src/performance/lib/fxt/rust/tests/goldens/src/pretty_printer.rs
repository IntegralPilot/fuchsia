// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;

/// pretty-print the contents of an fxt file for golden testing
#[derive(Debug, argh::FromArgs)]
struct Options {
    /// path to an fxt file
    #[argh(option)]
    input: PathBuf,

    /// path to write a text file to
    #[argh(option)]
    output: PathBuf,
}

fn main() {
    let Options { input, output } = argh::from_env();
    let trace_session_file = std::fs::File::open(input).unwrap();
    let trace_session = fxt::SessionParser::new(std::io::BufReader::new(trace_session_file))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    std::fs::write(output, format!("{trace_session:#?}")).unwrap();
}
