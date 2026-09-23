// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A ffx plugin for bisecting product bundles.

#![deny(missing_docs)]

use anyhow::{Context, Result};
use assembly_artifact_cache::{ArtifactCache, MOSClient};
use assembly_config_schema as _;
use assembly_container as _;
use assembly_platform_artifacts as _;
use assembly_util::{sanitize_for_mos_apis, shorten_path};
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_config::EnvironmentContext;
use ffx_product_bundle_bisect_args::BisectCommand;
use ffx_writer::{SimpleWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use gcs::client::Client as GcsClient;
use pbms::handle_new_access_token;
use std::fs;
use structured_ui;

mod assembly;
mod controller;
mod dimension;
mod mos;
mod search_space;
mod strategy;
mod testing;

use assembly::assemble;
use controller::Controller;
use mos::get_search_space;
use search_space::SearchSpace;
use testing::{prompt_for_manual_test, run_automated_test};

/// The ffx tool for bisecting product bundles.
#[derive(FfxTool)]
pub struct ProductBisectTool {
    /// The command line arguments for the bisect tool.
    #[command]
    pub cmd: BisectCommand,

    /// The ffx environment context.
    env_context: EnvironmentContext,
}

fho::embedded_plugin!(ProductBisectTool);

use gcs::error::GcsError;

/// Initiate PB Bisection.
#[async_trait(?Send)]
impl FfxMain for ProductBisectTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    /// Main entry point for the `ffx product bisect` tool.
    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        writer.line("")?;
        let cmd = sanitize_cmd(self.cmd);

        let setup_result = setup_core(&cmd, &self.env_context).await;
        let (plan_home, client, cache) = match setup_result {
            Ok(result) => result,
            Err(e) => {
                if let Some(GcsError::NeedNewRefreshToken) = e.downcast_ref::<GcsError>() {
                    writer.line("Authentication expired. Please run `ffx auth login`.")?;
                }
                return Err(e.into());
            }
        };

        if let Err(e) =
            run_bisection(cmd, &mut writer, &self.env_context, cache, client, plan_home).await
        {
            if let Some(GcsError::NeedNewRefreshToken) = e.downcast_ref::<GcsError>() {
                writer.line("Authentication expired. Please run `ffx auth login`.")?;
            }
            return Err(e.into());
        }

        Ok(())
    }
}

/// Sanitize the inputs so that they can be used as directory names and for MOS queries.
fn sanitize_cmd(mut cmd: BisectCommand) -> BisectCommand {
    // Sanitize the inputs so that they can be used as directory names and for MOS queries.
    cmd.name = sanitize_for_mos_apis(&cmd.name);
    cmd.from_success = sanitize_for_mos_apis(&cmd.from_success);
    cmd.to_failure = sanitize_for_mos_apis(&cmd.to_failure);
    cmd
}

/// Setup the environment for bisection, returning the plan home path,
/// MOS client, and artifact cache.
async fn setup_core(
    cmd: &BisectCommand,
    env_context: &EnvironmentContext,
) -> Result<(Utf8PathBuf, MOSClient, ArtifactCache)> {
    // Create a directory for storing plan status and search results.
    let home = std::env::home_dir().context("Could not find home directory")?;
    let fuchsia_home = Utf8PathBuf::from_path_buf(home.join(".fuchsia"))
        .map_err(|p| anyhow::anyhow!("Path is not valid UTF-8: {}", p.display()))?;
    let bisect_home = fuchsia_home.join("bisect");
    let plan_home =
        bisect_home.join(&cmd.name).join(format!("{}_to_{}", cmd.from_success, cmd.to_failure));
    fs::create_dir_all(&plan_home).with_context(|| {
        format!("Failed to create plan directory at {}", shorten_path(&plan_home))
    })?;

    // Create a MOS client. Reuse the setup logic from the GCS library.
    let gcs_client = GcsClient::initial().context("Failed to initialize GCS client")?;
    let token = handle_new_access_token(&cmd.auth, &structured_ui::MockUi::new())
        .await
        .context("Failed to get new access token")?;
    gcs_client.set_access_token(token).await;
    let client = MOSClient::new(gcs_client.clone());

    // Create an artifact cache for reusing assembly artifacts.
    let build_dir = env_context
        .build_dir()
        .map(|p| {
            Utf8PathBuf::from_path_buf(p.to_path_buf()).map_err(|build_dir| {
                anyhow::anyhow!("Failed to parse build_dir as utf8: {}", build_dir.display())
            })
        })
        .transpose()?;
    let cache = ArtifactCache::new(build_dir, gcs_client.clone())
        .context("Failed to create artifact cache")?;

    Ok((plan_home, client, cache))
}

async fn run_bisection<'a>(
    cmd: BisectCommand,
    writer: &'a mut SimpleWriter,
    env_context: &'a EnvironmentContext,
    cache: ArtifactCache,
    mut client: MOSClient,
    plan_home: Utf8PathBuf,
) -> Result<()> {
    use std::cell::RefCell;
    use std::rc::Rc;

    // We wrap the writer in an `Rc<RefCell<...>>` to provide interior
    // mutability. The `Controller` orchestrates the bisection loop,
    // but it also accepts a `test_fn` callback (a future-returning closure)
    // that performs the assembly and testing of each bisection step.
    //
    // Both the `Controller` and the `test_fn` need to print to the console
    // simultaneously to provide the user with ongoing status updates.
    // The `Rc<RefCell>` allows us to safely clone references to the writer
    // so both components can emit output.
    let shared_writer = Rc::new(RefCell::new(writer));

    let plan_file = plan_home.join("plan.json");
    let mut fetch_from_mos = true;
    let mut space = SearchSpace::new(vec![]);

    if plan_file.is_file() {
        let writer_clone = shared_writer.clone();
        let print = move |msg: &str| {
            let _ = writer_clone.borrow_mut().line(msg);
        };
        print(&format!("Found previous bisection at {}", shorten_path(&plan_file)));
        if confirm_action(&mut *shared_writer.borrow_mut())
            .context("Failed to get user confirmation")?
        {
            print("Resuming existing plan...\n");
            fetch_from_mos = false;
        } else {
            print("Deleting previous plan...\n");
            fs::remove_file(&plan_file).context("Failed to delete existing plan")?;
        }
    }

    if fetch_from_mos {
        let fuchsia_dir = find_fuchsia_dir(env_context);

        let writer_clone = shared_writer.clone();
        let mut print_mos = move |msg: &str| {
            let _ = writer_clone.borrow_mut().line(msg);
        };

        space = get_search_space(
            &mut client,
            &cmd.name,
            &cmd.from_success,
            &cmd.to_failure,
            fuchsia_dir.as_deref(),
            cmd.slot,
            &mut print_mos,
        )
        .await?;
    }

    let out_dir = cmd.out_dir.clone().unwrap_or_else(|| plan_home.join("out"));
    let gen_dir = cmd.gen_dir.clone().unwrap_or_else(|| plan_home.join("gen"));
    let script = cmd.script.clone();
    let slot = cmd.slot;
    let pb_name = cmd.name.clone();

    let cache_ref = &cache;

    // Define the callback function that the Controller will invoke
    // for each step in the search space.
    //
    // It is responsible for assembling the specific combination of artifacts
    // and validating the result.
    let test_fn = |combination: std::vec::Vec<assembly_artifact_cache::MOSIdentifier>| {
        let writer_clone1 = shared_writer.clone();
        let mut print1 = move |msg: &str| {
            let _ = writer_clone1.borrow_mut().line(msg);
        };
        let writer_clone2 = shared_writer.clone();
        let mut print2 = move |msg: &str| {
            let _ = writer_clone2.borrow_mut().line(msg);
        };
        let out_dir_clone = out_dir.clone();
        let gen_dir_clone = gen_dir.clone();
        let script_clone = script.clone();
        let pb_name_clone = pb_name.clone();

        async move {
            let pb_path = assemble(
                &combination,
                cache_ref,
                env_context,
                &pb_name_clone,
                &out_dir_clone,
                gen_dir_clone,
                slot,
                &mut print1,
            )
            .await?;

            if let Some(script_path) = &script_clone {
                run_automated_test(script_path, &pb_path, &mut print2).await
            } else {
                prompt_for_manual_test(&pb_path, &mut print2)
            }
        }
    };

    let writer_clone_ctrl = shared_writer.clone();
    let mut print_fn = move |msg: &str| {
        let _ = writer_clone_ctrl.borrow_mut().line(msg);
    };

    let mut controller =
        Controller::new(space, plan_file, slot, test_fn, &mut print_fn, Some(client))?;

    controller.run().await?;

    Ok(())
}

fn confirm_action(writer: &mut SimpleWriter) -> anyhow::Result<bool> {
    use std::io::{self, Write};
    loop {
        writer.write_all(b"Continue? (y/n) ")?;
        writer.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.to_lowercase().trim() {
            "yes" | "y" => return Ok(true),
            "no" | "n" => return Ok(false),
            _ => {
                writer.line("Invalid input. Please enter 'y', 'yes', 'n', or 'no'.")?;
            }
        }
    }
}

/// Resolve the Fuchsia source directory if in an in-tree or configured environment.
fn find_fuchsia_dir(env_context: &EnvironmentContext) -> Option<Utf8PathBuf> {
    env_context.project_root().and_then(Utf8Path::from_path).map(|p| p.to_path_buf()).or_else(
        || {
            env_context
                .build_dir()
                .and_then(Utf8Path::from_path)
                .and_then(|p| p.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::ConfigMap;
    use ffx_config::environment::ExecutableKind;
    use std::path::PathBuf;

    #[test]
    fn test_find_fuchsia_dir_no_context() {
        let ctx = EnvironmentContext::default();
        assert_eq!(find_fuchsia_dir(&ctx), None);
    }

    #[test]
    fn test_find_fuchsia_dir_in_tree_with_build_dir() {
        let ctx = EnvironmentContext::in_tree(
            ExecutableKind::Test,
            PathBuf::from("/fake/fuchsia"),
            Some(PathBuf::from("/fake/fuchsia/out/default")),
            ConfigMap::new(),
            None,
            false,
        )
        .expect("in tree context");
        assert_eq!(find_fuchsia_dir(&ctx), Some(Utf8PathBuf::from("/fake/fuchsia")));
    }

    #[test]
    fn test_find_fuchsia_dir_in_tree_without_build_dir() {
        let ctx = EnvironmentContext::in_tree(
            ExecutableKind::Test,
            PathBuf::from("/fake/fuchsia"),
            None,
            ConfigMap::new(),
            None,
            false,
        )
        .expect("in tree context");
        assert_eq!(find_fuchsia_dir(&ctx), Some(Utf8PathBuf::from("/fake/fuchsia")));
    }
}
