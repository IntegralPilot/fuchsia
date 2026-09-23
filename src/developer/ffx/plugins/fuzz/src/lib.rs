// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::reader::{CommandReader, ShellReader};
use crate::shell::Shell;
use anyhow::Result;
use ffx_config::EnvironmentContext;
use ffx_fuzz_args::{FuzzCommand, Session};
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use fuchsia_fuzzctl_fdomain::{StdioSink, Writer};
use target_holders::RemoteControlProxyHolder;

mod autocomplete;
mod fuzzer;
mod options;
mod reader;
mod shell;

/// The `ffx fuzz` plugin.
///
/// This plugin is designed to connect to the `fuzz-manager` on a target device, and use it to
/// further connect to a fuzzer instance. It enables sending a sequence of commands to fuzzer to
/// perform various actions such as fuzzing, input minimization, corpus compaction, etc. It also
/// receives and displays fuzzer outputs
///
/// This plugin uses a number of async futures which may safely run concurrently, but not in
/// parallel. This assumption is fulfilled by `ffx` itself: See //src/developer/ffx/src/main.rs,
/// in which `main` has an attribute of `#[fuchsia_async::run_singlethreaded]`.
#[derive(FfxTool)]
// TODO(https://fxbug.dev/403625605): Once fuzz is OK, remove the experimental check.
#[check(AvailabilityFlag("fuzzing"))]
pub struct FuzzTool {
    remote_control: RemoteControlProxyHolder,
    #[command]
    cmd: FuzzCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(FuzzTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for FuzzTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        fuzz(self.remote_control, self.cmd, &self.context).await.map_err(Into::into)
    }
}

async fn fuzz(
    rc_holder: RemoteControlProxyHolder,
    command: FuzzCommand,
    context: &EnvironmentContext,
) -> Result<()> {
    let session = command.as_session();
    let (is_tty, muted, use_colors) = match session {
        Session::Interactive(_) => (true, false, true),
        Session::Quiet(_) => (false, true, false),
        Session::Verbose(_) => (false, false, false),
    };
    let mut writer = Writer::new(StdioSink { is_tty });
    let rc = (*rc_holder).clone();
    writer.mute(muted);
    writer.use_colors(use_colors);
    match session {
        Session::Interactive(json_file) => {
            Shell::new(json_file, rc, ShellReader::new(), &writer).run(context).await
        }
        Session::Quiet(commands) => {
            Shell::new(None, rc, CommandReader::new(commands), &writer).run(context).await
        }
        Session::Verbose(commands) => {
            Shell::new(None, rc, CommandReader::new(commands), &writer).run(context).await
        }
    }
}
