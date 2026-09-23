// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

use crate::args::{
    Args, Command, GcCommand, GetHashCommand, OpenCommand, PkgStatusCommand, RepoAddCommand,
    RepoAddFileCommand, RepoAddSubCommand, RepoAddUrlCommand, RepoCommand, RepoRemoveCommand,
    RepoShowCommand, RepoSubCommand, ResolveCommand, RuleClearCommand, RuleCommand,
    RuleDumpDynamicCommand, RuleListCommand, RuleReplaceCommand, RuleReplaceFileCommand,
    RuleReplaceJsonCommand, RuleReplaceSubCommand, RuleSubCommand,
};
use anyhow::{Context as _, format_err};
use fetch_url::fetch_url;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_ext as pkg;
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fidl_fuchsia_pkg_rewrite::EngineMarker;
use fidl_fuchsia_pkg_rewrite_ext::{Rule as RewriteRule, RuleConfig, do_transaction};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_url::RepositoryUrl;
use futures::stream::TryStreamExt;
use std::fs::File;
use std::io;
use std::process::exit;

mod args;

pub fn main() -> Result<(), anyhow::Error> {
    let mut executor = fasync::LocalExecutorBuilder::new().build();
    let Args { command } = argh::from_env();
    exit(executor.run_singlethreaded(main_helper(command))?)
}

async fn main_helper(command: Command) -> Result<i32, anyhow::Error> {
    match command {
        Command::Resolve(ResolveCommand { pkg_url, verbose }) => {
            let resolver = connect_to_protocol::<fpkg::PackageResolverMarker>()
                .context("Failed to connect to resolver service")?;
            println!("resolving {pkg_url}");

            let (dir, dir_server_end) = fidl::endpoints::create_proxy();

            let _: fpkg::ResolutionContext = resolver
                .resolve(&pkg_url, dir_server_end)
                .await?
                .map_err(fidl_fuchsia_pkg_ext::ResolveError::from)
                .with_context(|| format!("Failed to resolve {pkg_url}"))?;

            if verbose {
                println!("package contents:");
                let mut stream =
                    fuchsia_fs::directory::readdir_recursive(&dir, /*timeout=*/ None);
                while let Some(entry) = stream.try_next().await? {
                    println!("/{}", entry.name);
                }
            }

            Ok(0)
        }
        Command::GetHash(GetHashCommand { pkg_url }) => {
            let resolver = connect_to_protocol::<fpkg::PackageResolverMarker>()
                .context("Failed to connect to resolver service")?;
            let blob_id =
                resolver.get_hash(&fpkg::PackageUrl { url: pkg_url }).await?.map_err(|i| {
                    format_err!(
                        "Failed to get package hash with error: {}",
                        zx::Status::err_from_raw(i)
                    )
                })?;
            println!("{}", pkg::BlobId::from(blob_id));
            Ok(0)
        }
        Command::PkgStatus(PkgStatusCommand { pkg_url: _ }) => {
            anyhow::bail!("`pkgctl pkg-status` is being deleted, https://fxbug.dev/552675412");
        }
        Command::Open(OpenCommand { meta_far_blob_id: _ }) => {
            anyhow::bail!("`pkgctl open` is being deleted, https://fxbug.dev/552670958");
        }
        Command::Repo(RepoCommand { verbose, subcommand }) => {
            let repo_manager = connect_to_protocol::<fpkg::RepositoryManagerMarker>()
                .context("Failed to connect to resolver service")?;

            match subcommand {
                None => {
                    if !verbose {
                        // with no arguments, list available repos
                        let repos = fetch_repos(repo_manager).await?;

                        let mut urls =
                            repos.into_iter().map(|r| r.repo_url().to_string()).collect::<Vec<_>>();
                        urls.sort_unstable();
                        urls.into_iter().for_each(|url| println!("{url}"));
                    } else {
                        let repos = fetch_repos(repo_manager).await?;

                        let s = serde_json::to_string_pretty(&repos).expect("valid json");
                        println!("{s}");
                    }
                    Ok(0)
                }
                Some(RepoSubCommand::Add(RepoAddCommand { subcommand })) => {
                    match subcommand {
                        RepoAddSubCommand::File(RepoAddFileCommand { persist, name, file }) => {
                            let mut repo: pkg::RepositoryConfig =
                                serde_json::from_reader(io::BufReader::new(File::open(file)?))?;
                            // If a name is specified via the command line, override the
                            // automatically derived name.
                            if let Some(n) = name {
                                repo = pkg::RepositoryConfigBuilder::from(repo)
                                    .repo_url(RepositoryUrl::parse_host(n)?)
                                    .build();
                            }
                            // The storage type can be overridden to persistent via the
                            // command line.
                            if persist {
                                repo = pkg::RepositoryConfigBuilder::from(repo)
                                    .repo_storage_type(pkg::RepositoryStorageType::Persistent)
                                    .build();
                            }

                            let res = repo_manager.add(&repo.into()).await?;
                            let () = res.map_err(zx::Status::err_from_raw)?;
                        }
                        RepoAddSubCommand::Url(RepoAddUrlCommand { persist, name, repo_url }) => {
                            let res = fetch_url(repo_url, None, vec![]).await?;
                            let mut repo: pkg::RepositoryConfig = serde_json::from_slice(&res)?;
                            // If a name is specified via the command line, override the
                            // automatically derived name.
                            if let Some(n) = name {
                                repo = pkg::RepositoryConfigBuilder::from(repo)
                                    .repo_url(RepositoryUrl::parse_host(n)?)
                                    .build();
                            }
                            // The storage type can be overridden to persistent via the
                            // command line.
                            if persist {
                                repo = pkg::RepositoryConfigBuilder::from(repo)
                                    .repo_storage_type(pkg::RepositoryStorageType::Persistent)
                                    .build();
                            }

                            let res = repo_manager.add(&repo.into()).await?;
                            let () = res.map_err(zx::Status::err_from_raw)?;
                        }
                    }

                    Ok(0)
                }

                Some(RepoSubCommand::Remove(RepoRemoveCommand { repo_url })) => {
                    let res = repo_manager.remove(&repo_url).await?;
                    let () = res.map_err(zx::Status::err_from_raw)?;

                    Ok(0)
                }

                Some(RepoSubCommand::Show(RepoShowCommand { repo_url })) => {
                    let repos = fetch_repos(repo_manager).await?;
                    for repo in repos.into_iter() {
                        if repo.repo_url().to_string() == repo_url {
                            let s = serde_json::to_string_pretty(&repo).expect("valid json");
                            println!("{s}");
                            return Ok(0);
                        }
                    }

                    println!("Package repository not found: {repo_url:?}");
                    Ok(1)
                }
            }
        }
        Command::Rule(RuleCommand { subcommand }) => {
            let engine = connect_to_protocol::<EngineMarker>()
                .context("Failed to connect to rewrite engine service")?;

            match subcommand {
                RuleSubCommand::List(RuleListCommand {}) => {
                    let (iter, iter_server_end) = fidl::endpoints::create_proxy();
                    engine.list(iter_server_end)?;

                    let mut rules = Vec::new();
                    loop {
                        let more = iter.next().await?;
                        if more.is_empty() {
                            break;
                        }
                        rules.extend(more);
                    }
                    let rules = rules.into_iter().map(|rule| rule.try_into()).collect::<Result<
                        Vec<RewriteRule>,
                        _,
                    >>(
                    )?;

                    for rule in rules {
                        println!("{rule:#?}");
                    }
                }
                RuleSubCommand::Clear(RuleClearCommand {}) => {
                    do_transaction(&engine, |transaction| async move {
                        transaction.reset_all()?;
                        Ok(transaction)
                    })
                    .await?;
                }
                RuleSubCommand::DumpDynamic(RuleDumpDynamicCommand {}) => {
                    let (transaction, transaction_server_end) = fidl::endpoints::create_proxy();
                    let () = engine.start_edit_transaction(transaction_server_end)?;
                    let (iter, iter_server_end) = fidl::endpoints::create_proxy();
                    transaction.list_dynamic(iter_server_end)?;
                    let mut rules = Vec::new();
                    loop {
                        let more = iter.next().await?;
                        if more.is_empty() {
                            break;
                        }
                        rules.extend(more);
                    }
                    let rules = rules.into_iter().map(|rule| rule.try_into()).collect::<Result<
                        Vec<RewriteRule>,
                        _,
                    >>(
                    )?;
                    let rule_configs = RuleConfig::Version1(rules);
                    let dynamic_rules = serde_json::to_string_pretty(&rule_configs)?;
                    println!("{dynamic_rules}");
                }
                RuleSubCommand::Replace(RuleReplaceCommand { subcommand }) => {
                    let RuleConfig::Version1(ref rules) = match subcommand {
                        RuleReplaceSubCommand::File(RuleReplaceFileCommand { file }) => {
                            serde_json::from_reader(io::BufReader::new(File::open(file)?))?
                        }
                        RuleReplaceSubCommand::Json(RuleReplaceJsonCommand { config }) => config,
                    };

                    do_transaction(&engine, |transaction| {
                        async move {
                            transaction.reset_all()?;
                            // add() inserts rules as highest priority, so iterate over our
                            // prioritized list of rules so they end up in the right order.
                            for rule in rules.iter().rev() {
                                let () = transaction.add(rule.clone()).await?;
                            }
                            Ok(transaction)
                        }
                    })
                    .await?;
                }
            }

            Ok(0)
        }
        Command::Gc(GcCommand {}) => {
            let space_manager = connect_to_protocol::<fpkg_gc::ManagerMarker>()
                .context("Failed to connect to space manager service")?;
            space_manager
                .gc()
                .await?
                .map_err(|err| format_err!("Garbage collection failed with error: {:?}", err))
                .map(|_| 0i32)
        }
    }
}

async fn fetch_repos(
    repo_manager: fpkg::RepositoryManagerProxy,
) -> Result<Vec<pkg::RepositoryConfig>, anyhow::Error> {
    let (iter, server_end) = fidl::endpoints::create_proxy();
    repo_manager.list(server_end)?;
    let mut repos = vec![];

    loop {
        let chunk = iter.next().await?;
        if chunk.is_empty() {
            break;
        }
        repos.extend(chunk);
    }

    repos
        .into_iter()
        .map(|repo| pkg::RepositoryConfig::try_from(repo).map_err(anyhow::Error::from))
        .collect()
}
