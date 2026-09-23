// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use zx_types::zx_koid_t;

use crate::debug_agent::DebugAgentSocket;

/// Builds command lines for `zxdb`.
pub struct CommandBuilder {
    zxdb_path: PathBuf,
    args: Vec<OsString>,
}

impl CommandBuilder {
    /// Create a command builder for zxdb at the given path.
    pub fn new(zxdb_path: PathBuf) -> Self {
        Self { zxdb_path, args: vec![] }
    }

    /// Build the `Command` for running zxdb.
    pub fn build(&self) -> Command {
        let mut command = Command::new(self.zxdb_path.clone());
        command.args(&self.args);
        command
    }

    /// The currently collected arguments.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// Add the `--unix-connect` argument for the given `socket`.
    pub fn connect(&mut self, socket: &DebugAgentSocket) {
        self.args.push(OsString::from("--unix-connect"));
        self.args.push(socket.unix_socket_path().into());
    }

    pub fn push_str(&mut self, arg: &str) {
        self.args.push(OsString::from(arg));
    }

    pub fn extend(&mut self, args: &Vec<String>) {
        self.args.extend(args.iter().map(|s| s.into()));
    }

    pub fn attach(&mut self, attach: &str) {
        self.args.push(OsString::from("--attach"));
        self.args.push(OsString::from(attach));
    }

    pub fn attach_each(&mut self, attach: &Vec<String>) {
        for attach in attach.iter() {
            self.attach(attach);
        }
    }

    pub fn attach_koid(&mut self, koid: zx_koid_t) {
        self.attach(&format!("{koid}"));
    }

    pub fn attach_job_koid(&mut self, koid: zx_koid_t) {
        self.attach(&format!("--job {koid}"));
    }

    pub fn break_at(&mut self, location: &str) {
        self.execute(&format!("break {location}"));
    }

    pub fn execute(&mut self, execute: &str) {
        self.args.push(OsString::from("--execute"));
        self.args.push(OsString::from(execute));
    }

    pub fn execute_each(&mut self, execute: &Vec<String>) {
        for execute in execute.iter() {
            self.execute(execute);
        }
    }
}
