// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use argh::{ArgsInfo, FromArgs};
use async_fs as afs;
use async_trait::async_trait;
use buf_read_ext::BufReadExt as _;
use crossterm::tty::IsTty;
use errors::ffx_bail;
use fdomain_client::fidl::Proxy;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use fidl_codec_fdomain::library as lib;
use fuchsia_async as fasync;
use futures::AsyncReadExt;
use futures::channel::oneshot::channel as oneshot;
use futures::future::{Either, FutureExt, select};
use futures::io::AllowStdIo;
use playground::interpreter::Interpreter;
use playground::value::Value;
use std::fs::File;
use std::io::{self, BufReader, stdin};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use target_holders::RemoteControlProxyHolder;
use vfs::directory::helper::DirectlyMutable;

mod analytics;
mod cf_fs;
mod host_fs;
mod presentation;
mod repl;
mod strict_mutex;
mod term_util;
mod toolbox_fs;

use presentation::display_result;
use toolbox_fs::toolbox_directory;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "playground", description = "Directly invoke FIDL services")]
pub struct PlaygroundCommand {
    /// a command to run. If passed, the playground will run that one command,
    /// display the output, and return immediately.
    #[argh(option, short = 'c')]
    pub command: Option<String>,

    /// a file to run.
    #[argh(positional)]
    pub file: Option<String>,
}

#[derive(FfxTool)]
pub struct PlaygroundTool {
    #[command]
    cmd: PlaygroundCommand,
    rcs_proxy: RemoteControlProxyHolder,
}

#[async_trait(?Send)]
impl FfxMain for PlaygroundTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        exec_playground(self.rcs_proxy, self.cmd).await?;
        Ok(())
    }
}

pub async fn exec_playground(
    remote_proxy: RemoteControlProxyHolder,
    command: PlaygroundCommand,
) -> Result<()> {
    if !stdin().is_tty() {
        ffx_bail!(
            "Playground must be used from a real TTY.\n\
                   Playground is not stable enough for automation tasks, \
                     and is not designed to be suitable for them.\n\
                   If you'd like to do extensive scripting, consider Fuchsia Controller instead.\n\
                   https://fuchsia.dev/fuchsia-src/development/tools/fuchsia-controller/getting-started-in-tree"
        );
    }

    let mut lib_namespace = lib::Namespace::new();

    let Ok(fuchsia_dir) = std::env::var("FUCHSIA_DIR") else {
        ffx_bail!("FUCHSIA_DIR environment variable is not set")
    };
    let fuchsia_dir = fuchsia_dir.trim();
    let build_dir_file: PathBuf = [fuchsia_dir, ".fx-build-dir"].iter().collect();

    let root = match std::fs::read_to_string(&build_dir_file) {
        Ok(root) => root,
        Err(e) => ffx_bail!("Could not read {}: {e}", build_dir_file.display()),
    };
    let root = root.trim();

    let all_fidl_json: PathBuf = [fuchsia_dir, root, "all_fidl_json.txt"].iter().collect();

    let file = match File::open(&all_fidl_json) {
        Ok(file) => file,
        Err(e) => ffx_bail!("Could not open {}: {e}", all_fidl_json.display()),
    };

    let mut reader = BufReader::new(file);
    let mut lines = reader.lending_lines();
    while let Some(line) = lines.next() {
        let line = match line {
            Ok(line) => line,
            Err(e) => ffx_bail!("Error reading {}, {e}", all_fidl_json.display()),
        };
        let line = line.trim();
        let path: PathBuf = [fuchsia_dir, root, line].iter().collect();
        let json_file = match std::fs::read_to_string(&path) {
            Ok(json_file) => json_file,
            Err(e) => {
                eprintln!("Skipping {line}: read error: {e}");
                continue;
            }
        };
        if let Err(x) = lib_namespace.load(&json_file) {
            eprintln!("Skipping {line}: {x}");
        }
    }

    let remote_proxy = Arc::new(remote_proxy);
    let query = rcs::root_realm_query(&remote_proxy, std::time::Duration::from_secs(5)).await?;
    let toolbox = toolbox_directory(&*remote_proxy, &query).await?;
    let cf_root = cf_fs::CFDirectory::new_root(query);
    let fs_root_simple = vfs::directory::immutable::simple();
    fs_root_simple.add_entry("host", host_fs::HostDirectory::new("/"))?;
    fs_root_simple.add_entry("toolbox", toolbox)?;
    fs_root_simple.add_entry("cf", cf_root)?;

    let root_dir_client = vfs::directory::serve_read_only(
        Arc::clone(&fs_root_simple),
        vfs::execution_scope::ExecutionScope::new(remote_proxy.domain()),
    )
    .into_client_end()
    .unwrap();

    let (interpreter, runner) = Interpreter::new(lib_namespace, root_dir_client.into()).await;
    fasync::Task::spawn(runner).detach();

    let (quit_sender, mut quit_receiver) = oneshot();
    let quit_sender = Arc::new(Mutex::new(Some(quit_sender)));
    {
        let quit_sender = Arc::clone(&quit_sender);
        interpreter.add_command("quit", move |_, _| {
            if let Some(quit_sender) = quit_sender.lock().unwrap().take() {
                let _ = quit_sender.send(());
            }

            async move { Ok(Value::Null) }
        });
    }
    {
        let remote_proxy = Arc::clone(&remote_proxy);
        fasync::Task::spawn(async move {
            let _ = remote_proxy.on_closed().await;
            if let Some(quit_sender) = quit_sender.lock().unwrap().take() {
                eprintln!("Connection lost");
                let _ = quit_sender.send(());
            }
        })
        .detach();
    }

    let mut text = String::new();
    if let Some(cmd) = command.command {
        if command.file.is_some() {
            Err(anyhow!("Cannot specify a command and a file at the same time"))
        } else {
            let res = interpreter.run(cmd.as_str()).await;
            analytics::emit_playground_cmd_event(res.is_ok(), "argument").await;
            display_result(&mut AllowStdIo::new(&io::stdout()), res, &interpreter).await?;
            Ok(())
        }
    } else if let Some(file) = command.file {
        afs::File::open(&file).await?.read_to_string(&mut text).await?;
        let res = interpreter.run(text.as_str()).await;
        analytics::emit_playground_cmd_event(res.is_ok(), "script").await;
        display_result(&mut AllowStdIo::new(&io::stdout()), res, &interpreter).await?;
        Ok(())
    } else {
        let node_name = remote_proxy
            .identify_host()
            .await?
            .map_err(|e| anyhow!("Could not identify host: {:?}", e))?
            .nodename
            .unwrap_or_else(|| "<unknown>".to_owned());
        let node_name = node_name.escape_default().to_string();
        let repl = Arc::new(repl::Repl::new()?);
        let interpreter = Arc::new(interpreter);

        let completer = {
            let interpreter = Arc::clone(&interpreter);
            move |cmd, pos| {
                let interpreter = Arc::clone(&interpreter);
                async move { interpreter.complete(cmd, pos).await }
            }
        };

        while let Either::Left((line, _)) = select(
            repl.get_cmd(&format!("\x1b[1;92m{} \x1b[1;97m➤\x1b[0m", node_name), &completer)
                .boxed(),
            &mut quit_receiver,
        )
        .await
        {
            let repl = Arc::clone(&repl);
            let interpreter = Arc::clone(&interpreter);
            if let Some(line) = line? {
                fasync::Task::local(async move {
                    let res = interpreter.run(line.as_str()).await;
                    analytics::emit_playground_cmd_event(res.is_ok(), "interactive").await;
                    display_result(&mut repl::ReplWriter::new(&*repl), res, &interpreter)
                        .await
                        .unwrap();
                })
                .detach();
            } else {
                break;
            }
        }
        Ok(())
    }
}
