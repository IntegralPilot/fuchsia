// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42055130): Consider enabling globally.
#![deny(unused_crate_dependencies)]

use anyhow::Result;
use argh::FromArgs;
use package_tool::{
    PackageArchiveAddCommand, PackageArchiveCreateCommand, PackageArchiveExtractCommand,
    PackageBuildCommand, RepoCreateCommand, RepoPMListCommand, RepoPublishCommand,
    cmd_package_archive_add, cmd_package_archive_create, cmd_package_archive_extract,
    cmd_package_build, cmd_repo_create, cmd_repo_package_manifest_list, cmd_repo_publish,
};

struct BoxedRepoPublishCommand(Box<RepoPublishCommand>);

impl FromArgs for BoxedRepoPublishCommand {
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {
        RepoPublishCommand::from_args(command_name, args).map(|c| Self(Box::new(c)))
    }
}

impl argh::SubCommand for BoxedRepoPublishCommand {
    const COMMAND: &'static argh::CommandInfo = <RepoPublishCommand as argh::SubCommand>::COMMAND;
}

/// Package manipulation tool
#[derive(FromArgs)]
struct Command {
    #[argh(subcommand)]
    subcommands: SubCommands,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCommands {
    Package(PackageCommand),
    Repository(RepoCommand),
}

/// Package subcommands
#[derive(FromArgs)]
#[argh(subcommand, name = "package")]
struct PackageCommand {
    #[argh(subcommand)]
    subcommands: PackageSubCommands,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum PackageSubCommands {
    Archive(PackageArchiveCommand),
    Build(PackageBuildCommand),
}

/// Package Archive subcommands
#[derive(FromArgs)]
#[argh(subcommand, name = "archive")]
struct PackageArchiveCommand {
    #[argh(subcommand)]
    subcommands: PackageArchiveSubCommands,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum PackageArchiveSubCommands {
    Add(PackageArchiveAddCommand),
    Create(PackageArchiveCreateCommand),
    Extract(PackageArchiveExtractCommand),
}

/// Repository subcommands
#[derive(FromArgs)]
#[argh(subcommand, name = "repository")]
struct RepoCommand {
    #[argh(subcommand)]
    subcommands: RepoSubCommands,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum RepoSubCommands {
    Create(RepoCreateCommand),
    Publish(BoxedRepoPublishCommand),
    PMList(RepoPMListCommand),
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let cmd: Command = argh::from_env();
    match cmd.subcommands {
        SubCommands::Package(cmd) => match cmd.subcommands {
            PackageSubCommands::Archive(cmd) => match cmd.subcommands {
                PackageArchiveSubCommands::Add(cmd) => cmd_package_archive_add(cmd).await,
                PackageArchiveSubCommands::Create(cmd) => cmd_package_archive_create(cmd).await,
                PackageArchiveSubCommands::Extract(cmd) => cmd_package_archive_extract(cmd).await,
            },
            PackageSubCommands::Build(cmd) => cmd_package_build(cmd).await,
        },
        SubCommands::Repository(cmd) => match cmd.subcommands {
            RepoSubCommands::Create(cmd) => cmd_repo_create(cmd).await,
            RepoSubCommands::Publish(cmd) => cmd_repo_publish(*cmd.0).await,
            RepoSubCommands::PMList(cmd) => cmd_repo_package_manifest_list(cmd).await,
        },
    }
}
