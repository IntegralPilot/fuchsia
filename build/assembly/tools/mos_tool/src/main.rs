// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use argh::FromArgs;
use assembly_artifact_cache::{Artifact, MOSClient, MOSIdentifier, mos};
use assembly_util::sanitize_for_mos_apis;
use gcs::client::Client as GcsClient;
use pbms::{AuthFlowChoice, handle_new_access_token};
use structured_ui;

/// A tool for interacting with the MOS service.
#[derive(FromArgs, Debug)]
struct MosToolArgs {
    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
enum Command {
    GetArtifactReleaseInfo(GetArtifactReleaseInfoArgs),
    GetPbReleaseInfo(GetPbReleaseInfoArgs),
    Interpolate(InterpolateArgs),
}

/// Get release information for a single artifact.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "get-artifact-release-info")]
struct GetArtifactReleaseInfoArgs {
    /// the MOS ID of the artifact (e.g. "mos://fuchsia/boards/x64@1.2.3")
    #[argh(positional)]
    mos_id: String,
}

/// Get release information for a product bundle.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "get-pb-release-info")]
struct GetPbReleaseInfoArgs {
    /// the name of the product bundle.
    #[argh(positional)]
    name: String,
    /// the version of the product bundle.
    #[argh(positional)]
    version: String,
}

/// Interpolate between two artifact versions.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "interpolate")]
struct InterpolateArgs {
    /// the starting MOS ID.
    #[argh(positional)]
    from_mos_id: String,
    /// the ending MOS ID.
    #[argh(positional)]
    to_mos_id: String,
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: MosToolArgs = argh::from_env();

    let gcs_client = GcsClient::initial().context("Failed to initialize GCS client")?;
    let token = handle_new_access_token(&AuthFlowChoice::Default, &structured_ui::MockUi::new())
        .await
        .context("Failed to get new access token")?;
    gcs_client.set_access_token(token).await;
    let client = MOSClient::new(gcs_client.clone());

    match args.command {
        Command::GetArtifactReleaseInfo(args) => {
            let artifact = parse_mos_id(&args.mos_id)?;
            let result = client.get_artifact_release_info(&artifact).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::GetPbReleaseInfo(args) => {
            let name = sanitize_and_report("name", args.name);
            let version = sanitize_and_report("version", args.version);
            let result = client.get_pb_release_info(name, version).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Interpolate(args) => {
            let from = parse_mos_id(&args.from_mos_id)?;
            let to = parse_mos_id(&args.to_mos_id)?;
            let result = client.interpolate(&from, &to).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}

fn parse_mos_id(s: &str) -> Result<MOSIdentifier> {
    match mos::parse_mos_artifact(s)? {
        Some(Artifact::MOS(id)) => Ok(sanitize_mos_id(s, id)),
        _ => anyhow::bail!("Invalid MOS ID: {}", s),
    }
}

fn sanitize_mos_id(original_str: &str, mut id: MOSIdentifier) -> MOSIdentifier {
    let original = id.id();
    id.repository = sanitize_for_mos_apis(&id.repository);
    id.name = sanitize_for_mos_apis(&id.name);
    id.version = sanitize_for_mos_apis(&id.version);
    let sanitized = id.id();
    if original != sanitized {
        eprintln!("Sanitized MOS ID from '{}' to '{}'", original_str, sanitized);
    }
    id
}

fn sanitize_and_report(arg_name: &str, val: String) -> String {
    let sanitized = sanitize_for_mos_apis(&val);
    if sanitized != val {
        eprintln!("Sanitized {} from '{}' to '{}'", arg_name, val, sanitized);
    }
    sanitized
}
