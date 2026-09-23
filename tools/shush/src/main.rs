// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod allow;
mod api;
mod bugspec;
mod command_ext;
mod fix;
mod issues;
mod lint;
mod mock;
mod owners;
mod rollout;
mod span;

use anyhow::{Result, anyhow, bail};
use argh::FromArgs;
use rustfix::Filter;

use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::api::Api;

const DEFAULT_ROLLOUT_PATH: &str = "./rollout.json";
const DEFAULT_MAX_CC_USERS: usize = 3;
const DEFAULT_HOLDING_COMPONENT: &str = "LanguagePlatforms>Rust";

#[derive(Debug, FromArgs)]
/// Silence rustc and clippy lints with allow attributes and autofixes
struct Args {
    #[argh(subcommand)]
    action: Action,
    /// path to a binary for API calls
    #[argh(option)]
    api: Option<PathBuf>,
    /// mock API issue creation calls
    #[argh(switch)]
    mock: bool,
    /// print details of created issues to the command line
    #[argh(switch)]
    verbose: bool,
    /// print API calls to the command line
    #[argh(switch)]
    log_api: bool,
}

impl Args {
    pub fn api(&self) -> Result<Box<dyn Api>> {
        let api = self.api.as_ref().map(|path| {
            Box::new(bugspec::Bugspec::new(path.clone(), self.log_api)) as Box<dyn Api>
        });

        if self.mock {
            Ok(Box::new(mock::Mock::new(self.log_api, api)))
        } else {
            Ok(api.ok_or_else(|| anyhow!("--api is required when shush is not mocked"))?)
        }
    }
}

#[allow(clippy::large_enum_variant, reason = "mass allow for https://fxbug.dev/381896734")]
#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum Action {
    Lint(Lint),
    Rollout(Rollout),
}

/// address existing lints
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "lint")]
struct Lint {
    /// how to address the lints
    #[argh(subcommand)]
    action: LintAction,
    /// path to the root dir of the fuchsia source tree
    #[argh(option)]
    fuchsia_dir: Option<PathBuf>,
    /// don't modify source files
    #[argh(switch)]
    dryrun: bool,
    /// modify files even if there are local uncommitted changes
    #[argh(switch)]
    force: bool,
    /// lint (or category) to deal with e.g. clippy::needless_return
    #[argh(option)]
    lint: Vec<String>,
    /// file containing json lints (uses stdin if not given)
    #[argh(positional)]
    lint_file: Option<PathBuf>,
}

impl Lint {
    pub fn change_to_fuchsia_root(&self) -> Result<PathBuf> {
        if let Some(fuchsia_dir) =
            self.fuchsia_dir.clone().or_else(|| env::var("FUCHSIA_DIR").ok().map(Into::into))
        {
            env::set_current_dir(&fuchsia_dir.canonicalize()?)?;
            Ok(fuchsia_dir)
        } else {
            Ok(std::env::current_dir()?.canonicalize()?)
        }
    }

    pub fn try_get_filter(&self) -> Result<&[String]> {
        if self.lint.is_empty() {
            Err(anyhow!("Must filter on at least one lint or category with '--lint'"))
        } else {
            Ok(&self.lint)
        }
    }

    pub fn read_lints(&self) -> Box<dyn BufRead> {
        if let Some(ref f) = self.lint_file {
            Box::new(BufReader::new(File::open(f).unwrap()))
        } else {
            Box::new(BufReader::new(io::stdin()))
        }
    }
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum LintAction {
    Fix(Fix),
    Allow(Allow),
}

/// use rustfix to auto-fix the lints
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "fix")]
struct Fix {
    /// which suggestions to apply
    #[argh(option, default = "Filter::MachineApplicableOnly", from_str_fn(filter_from_str))]
    suggestions: Filter,
    /// use this solution if there are multiple possible solutions.
    #[argh(option)]
    solution_message: Option<String>,
}

fn filter_from_str(s: &str) -> Result<Filter, String> {
    match s {
        "everything" => Ok(Filter::Everything),
        "machine-applicable" => Ok(Filter::MachineApplicableOnly),
        _ => Err("expected `everything` or `machine-applicable`".to_string()),
    }
}

/// add allow attributes
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "allow")]
struct Allow {
    /// a reason to provide for allows as an alternative to filing issues
    #[argh(option)]
    reason: Option<String>,
    /// the ref to link to on codesearch
    #[argh(option)]
    codesearch_ref: Option<String>,
    /// path to an issue description template containing "INSERT_DETAILS_HERE"
    #[argh(option)]
    template: Option<PathBuf>,
    /// the issue to mark created issues as blocking
    #[argh(option)]
    blocking_issue: Option<String>,
    /// the maximum number of additional users to CC on created issues
    #[argh(option)]
    max_cc_users: Option<usize>,
    /// the holding component to place newly-created bugs into (default "LanguagePlatforms>Rust")
    #[argh(option)]
    holding_component: Option<String>,
    /// the path to the rollout file
    #[argh(option)]
    rollout: Option<PathBuf>,
}

impl Allow {
    fn load_template(&self) -> Result<Option<String>> {
        Ok(self.template.as_ref().map(fs::read_to_string).transpose()?)
    }

    pub fn rollout_path(&self) -> &Path {
        self.rollout.as_deref().unwrap_or_else(|| Path::new(DEFAULT_ROLLOUT_PATH))
    }
}

/// roll out lints generated by allow
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "rollout")]
struct Rollout {
    /// the path to the rollout file
    #[argh(option)]
    rollout: Option<PathBuf>,
}

impl Rollout {
    pub fn rollout_path(&self) -> &Path {
        self.rollout.as_deref().unwrap_or_else(|| Path::new(DEFAULT_ROLLOUT_PATH))
    }
}

fn check_clean() -> Result<()> {
    let git_status =
        Command::new("jiri").args(["runp", "git", "status", "--porcelain"]).output()?;

    if !git_status.status.success() || !git_status.stdout.is_empty() {
        bail!("The current directory is dirty, pass the --force flag or commit the local changes");
    }

    Ok(())
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    match args.action {
        Action::Lint(ref lint_args) => {
            if !(lint_args.dryrun || lint_args.force) {
                check_clean()?;
            }

            if lint_args.dryrun && !args.mock {
                bail!("dry runs require a mocked API");
            }

            match &lint_args.action {
                LintAction::Fix(f) => fix::fix(
                    &mut lint_args.read_lints(),
                    lint_args.try_get_filter()?,
                    f.suggestions,
                    f.solution_message.as_deref(),
                    lint_args.dryrun,
                ),
                LintAction::Allow(allow_args) => {
                    let followup = if let Some(reason) = &allow_args.reason {
                        anyhow::ensure!(
                            allow_args.codesearch_ref.is_none(),
                            "can't specify an allow reason and a codesearch ref"
                        );
                        anyhow::ensure!(
                            allow_args.template.is_none(),
                            "can't specify an allow reason and a template"
                        );
                        anyhow::ensure!(
                            allow_args.blocking_issue.is_none(),
                            "can't specify an allow reason and a blocking issue"
                        );
                        anyhow::ensure!(
                            allow_args.blocking_issue.is_none(),
                            "can't specify an allow reason and a blocking issue"
                        );
                        anyhow::ensure!(
                            allow_args.max_cc_users.is_none(),
                            "can't specify an allow reason and max cc users"
                        );
                        anyhow::ensure!(
                            allow_args.holding_component.is_none(),
                            "can't specify an allow reason and a holding component"
                        );
                        anyhow::ensure!(
                            allow_args.rollout.is_none(),
                            "can't specify an allow reason and a rollout path"
                        );
                        allow::AllowFollowup::Reason(reason.clone())
                    } else {
                        let issue_template = issues::IssueTemplate::new(
                            &lint_args.lint,
                            allow_args.codesearch_ref.as_deref(),
                            allow_args.load_template()?,
                            allow_args.blocking_issue.as_deref(),
                            allow_args.max_cc_users.unwrap_or(DEFAULT_MAX_CC_USERS),
                        );

                        let rollout_path = allow_args.rollout_path();
                        if rollout_path.exists() {
                            return Err(anyhow!(
                                "The rollout path {} already exists, delete it or specify an alternate path.",
                                rollout_path.to_str().unwrap_or("<non-utf8 path>"),
                            ));
                        }

                        allow::AllowFollowup::FileIssues {
                            api: args.api()?,
                            issue_template,
                            rollout_path,
                            holding_component_name: allow_args
                                .holding_component
                                .as_deref()
                                .unwrap_or(DEFAULT_HOLDING_COMPONENT),
                        }
                    };

                    allow::allow(
                        &mut lint_args.read_lints(),
                        lint_args.try_get_filter()?,
                        &lint_args.change_to_fuchsia_root()?,
                        followup,
                        lint_args.dryrun,
                        args.verbose,
                    )
                }
            }
        }
        Action::Rollout(ref rollout_args) => {
            let mut api = args.api()?;

            let rollout_path = rollout_args.rollout_path();
            if !rollout_path.exists() {
                return Err(anyhow!(
                    "The rollout path {} does not exist, run shush allow to generate a rollout file.",
                    rollout_path.to_str().unwrap_or("<non-utf8 path>"),
                ));
            }

            rollout::rollout(&mut *api, rollout_path, args.verbose)
        }
    }
}
