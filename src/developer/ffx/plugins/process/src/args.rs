// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "process", description = "Processes related commands")]
pub struct ProcessCommand {
    #[argh(subcommand)]
    pub arg: Args,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum Args {
    List(ListArg),
    Tree(TreeArg),
    Filter(FilterArg),
    GenerateFuchsiaMap(GenerateFuchsiaMapArg),
    Kill(KillArg),
    StackTrace(StackTraceArg),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "list",
    description = "outputs a list containing the name and koid of all processes"
)]
pub struct ListArg {
    #[argh(
        switch,
        description = "outputs all processes and the kernel objects owned by each of them"
    )]
    pub verbose: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "tree", description = "outputs the tree of all tasks in the system")]
pub struct TreeArg {
    #[argh(switch, description = "include threads")]
    pub threads: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "filter",
    description = "outputs information about the processes that correspond to the koids input"
)]
pub struct FilterArg {
    #[argh(positional, description = "process koids")]
    pub process_koids: Vec<u64>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "generate-fuchsia-map",
    description = "outputs the json required to generate a map of all processes and channels"
)]
pub struct GenerateFuchsiaMapArg {}

#[derive(PartialEq, Debug)]
pub enum Task {
    Koid(u64),
    ProcessName(String),
}

fn parse_task(arg: &str) -> Result<Task, String> {
    Ok(if let Ok(koid) = arg.parse::<u64>() {
        Task::Koid(koid)
    } else {
        Task::ProcessName(String::from(arg))
    })
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "kill",
    description = "Attempts to kill a process by it's KOID or process name."
)]
pub struct KillArg {
    #[argh(positional, from_str_fn(parse_task), description = "koid or name process to kill.")]
    pub task_to_kill: Task,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "stack_trace",
    description = "Attempts to get a strack trace a process by it's KOID or process name."
)]
pub struct StackTraceArg {
    #[argh(
        positional,
        from_str_fn(parse_task),
        description = "koid or name of process to get stack trace of."
    )]
    pub task: Task,
}
