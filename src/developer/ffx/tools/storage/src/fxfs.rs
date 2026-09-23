// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_fxfs::DebugProxy;
use ffx_writer::SimpleWriter;
use fho::{Error, Result};
use zx_status::Status;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "compact",
    example = "ffx storage fxfs compact",
    description = "Forces a (blocking) compaction of all layer files."
)]
pub struct CompactSubCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "delete_profile",
    example = "ffx storage fxfs delete_profile",
    description = "Deletes a profile from a named unlocked volume. Fails during active profile \
        record and/or replay."
)]
pub struct DeleteProfileSubCommand {
    #[argh(positional)]
    volume: String,
    #[argh(positional)]
    profile: String,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "record_and_replay_profile",
    example = "ffx storage fxfs record_and_replay_profile --volume data startup 60 ",
    description = "Starts recording a for a named unlocked volume to run for a limited number of \
        time. If a profile exists on the volume with the given name, then it will also begin \
        replaying it. If no volume is given, then all unlocked volumes are activated for recording \
        and replay. Fails during active profile recording and/or replay."
)]
pub struct RecordAndReplayProfileSubCommand {
    #[argh(positional)]
    profile: String,
    #[argh(positional)]
    duration_secs: u32,
    #[argh(option, short = 'v')]
    /// the volume to affect instead of all unlocked volumes.
    volume: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "replay_xor_record_profile",
    example = "ffx storage fxfs replay_xor_record_profile --volume data startup 60 ",
    description = "Replays a profile for a named unlocked volume if one exists, otherwise it \
        starts recording one. Fails during active profile recording and/or replay."
)]
pub struct ReplayXorRecordProfileSubCommand {
    #[argh(positional)]
    profile: String,
    #[argh(positional)]
    duration_secs: u32,
    #[argh(option, short = 'v')]
    /// the volume to affect.
    volume: String,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "stop_profile",
    example = "ffx storage fxfs stop_profile",
    description = "Blocks while stopping all profile recording and/or replay activity."
)]
pub struct StopProfileSubCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum FxfsSubCommand {
    Compact(CompactSubCommand),
    DeleteProfile(DeleteProfileSubCommand),
    RecordAndReplayProfile(RecordAndReplayProfileSubCommand),
    ReplayXorRecordProfile(ReplayXorRecordProfileSubCommand),
    StopProfile(StopProfileSubCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "fxfs", description = "Interact with fxfs instances.")]
pub struct FxfsCommand {
    #[argh(subcommand)]
    subcommand: FxfsSubCommand,
}

pub async fn handle_cmd(
    cmd: FxfsCommand,
    _writer: SimpleWriter,
    fxfs_proxy: DebugProxy,
) -> Result<()> {
    match cmd.subcommand {
        FxfsSubCommand::Compact(_) => {
            fxfs_proxy
                .compact()
                .await
                .map_err(|e| Error::User(e.into()))?
                .map_err(|e| Error::ExitWithCode(e))?;
        }
        FxfsSubCommand::DeleteProfile(args) => {
            fxfs_proxy
                .delete_profile(&args.volume, &args.profile)
                .await
                .map_err(|e| Error::User(e.into()))?
                .map_err(|e| Error::ExitWithCode(e))?;
        }
        FxfsSubCommand::RecordAndReplayProfile(args) => {
            fxfs_proxy
                .record_and_replay_profile(
                    args.volume.as_ref().map(|s| s.as_str()),
                    &args.profile,
                    args.duration_secs,
                )
                .await
                .map_err(|e| Error::User(e.into()))?
                .map_err(|e| Error::User(Status::err_from_raw(e).into()))?;
        }
        FxfsSubCommand::ReplayXorRecordProfile(args) => {
            fxfs_proxy
                .replay_xor_record_profile(&args.volume, &args.profile, args.duration_secs)
                .await
                .map_err(|e| Error::User(e.into()))?
                .map_err(|e| Error::User(Status::err_from_raw(e).into()))?;
        }
        FxfsSubCommand::StopProfile(_) => {
            fxfs_proxy
                .stop_profile_tasks()
                .await
                .map_err(|e| Error::User(e.into()))?
                .map_err(|e| Error::ExitWithCode(e))?;
        }
    };
    Ok(())
}
