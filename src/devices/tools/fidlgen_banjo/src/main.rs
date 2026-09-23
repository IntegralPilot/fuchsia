// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use clap::{Parser, ValueEnum};
use fidl_ir_lib::fidl::*;
use fidlgen_banjo_lib::backends::*;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

#[derive(Debug, ValueEnum, Clone)]
#[clap(rename_all = "snake_case")]
enum BackendName {
    C,
    Cpp,
    CppInternal,
    CppMock,
    Rust,
}

#[derive(Parser, Debug)]
#[command(name = "fidlgen_banjo")]
struct Flags {
    #[arg(short = 'i', long = "ir")]
    ir: PathBuf,

    #[arg(short = 'b', long = "backend", value_enum, ignore_case = true)]
    backend: BackendName,

    #[arg(short = 'o', long = "output")]
    output: PathBuf,
}

fn main() -> Result<(), Error> {
    let flags = Flags::parse();
    let mut output = File::create(flags.output)?;
    let mut backend: Box<dyn Backend<'_, _>> = match flags.backend {
        BackendName::C => Box::new(CBackend::new(&mut output)),
        BackendName::Cpp => Box::new(CppBackend::new(&mut output)),
        BackendName::CppInternal => Box::new(CppInternalBackend::new(&mut output)),
        BackendName::CppMock => Box::new(CppMockBackend::new(&mut output)),
        BackendName::Rust => Box::new(RustBackend::new(&mut output)),
    };
    let mut ir: FidlIr = serde_json::from_reader(BufReader::new(File::open(flags.ir)?))?;
    ir.build()?;
    backend.codegen(ir)
}
