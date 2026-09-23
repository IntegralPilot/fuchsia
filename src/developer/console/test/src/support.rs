// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A multi-faceted support program to help testing developer console.

use std::str::FromStr;

use argh::FromArgs;
use futures::StreamExt as _;

/// developer-console support binary.
#[derive(FromArgs)]
struct Args {
    /// the mode to run the support component.
    #[argh(positional, default = "Mode::SayHello")]
    mode: Mode,
}

enum Mode {
    ServeDirs,
    SayHello,
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "serve-dirs" => Ok(Mode::ServeDirs),
            "say-hello" => Ok(Mode::SayHello),
            _ => Err(format!("unknown mode: {}", s)),
        }
    }
}

fn main() {
    let Args { mode } = argh::from_env();
    match mode {
        Mode::ServeDirs => serve_dirs(),
        Mode::SayHello => say_hello(),
    }
}

fn serve_dirs() {
    fuchsia_async::LocalExecutorBuilder::new().build().run_singlethreaded(serve_dirs_inner())
}

async fn serve_dirs_inner() {
    let pkg = vfs::remote::remote_dir(
        fuchsia_fs::directory::open_in_namespace(
            "/pkg",
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
        )
        .expect("open"),
    );
    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.add_entry_at("boot", pkg)
        .add_entry_at("foo", vfs::pseudo_directory!())
        .add_entry_at("root-ssl-certificates", vfs::pseudo_directory!())
        .take_and_serve_directory_handle()
        .expect("failed to serve")
        .collect::<()>()
        .await;
}

fn say_hello() {
    println!("hello world");
}
