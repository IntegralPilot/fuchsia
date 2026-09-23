// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::{stderr, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use anyhow::{Context, Result};
use argh::FromArgs;
use compare::{AbiSurface, CompatibilityProblems};
use flyweights::FlyStr;
use itertools::Itertools;
use maplit as _;

mod compare;
mod convert;
mod ir;

/// Evaluate ABI compatibility between two Fuchsia platform versions.
#[derive(FromArgs)]
struct Args {
    /// path to write a report to.
    #[argh(option)]
    out: PathBuf,

    /// if any errors are found print them to stderr and return an exit status.
    #[argh(switch, short = 'e')]
    enforce: bool,

    /// follow protocol endpoints to evaluate tear-off protocols and their associated types.
    #[argh(switch, short = 't')]
    tear_off: bool,

    /// treat discoverable context contradictions as errors
    #[argh(switch)]
    fatal_contradictions: bool,

    /// paths to a JSON files with ABI surface definitions.
    #[argh(positional)]
    json: Vec<PathBuf>,
}

pub struct Configuration {
    pub tear_off: bool,
    pub fatal_contradictions: bool,
}

impl Configuration {
    fn new(args: &Args) -> Self {
        Self { tear_off: args.tear_off, fatal_contradictions: args.fatal_contradictions }
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self { tear_off: true, fatal_contradictions: false }
    }
}

#[derive(Clone, Copy, Debug, PartialOrd, PartialEq, Eq, Hash)]
pub enum Scope {
    Platform,
    External,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Platform => write!(f, "platform"),
            Self::External => write!(f, "external"),
        }
    }
}

#[derive(Clone, Debug, PartialOrd, PartialEq, Eq, Hash)]
pub struct Version {
    scope: Scope,
    version: FlyStr,
}

impl Version {
    pub fn new(version: &str) -> Self {
        Self::from_api_levels(version.split(","))
    }
    pub fn from_api_levels(api_levels: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let api_levels: Vec<_> = api_levels.into_iter().collect();
        assert!(!api_levels.is_empty());
        if api_levels.len() == 1 {
            let api_level = api_levels[0].as_ref();
            assert!(api_level == "NEXT" || api_level == "HEAD" || !api_level.chars().any(|c| !c.is_digit(10)),
                "External API levels must either be HEAD, NEXT or a single decimal integer. Got: {api_level:?}");
            Self { scope: Scope::External, version: FlyStr::new(api_level) }
        } else {
            assert!(
                api_levels.iter().any(|l| l.as_ref() == "HEAD"),
                "Platform API levels must include HEAD."
            );
            Self {
                scope: Scope::Platform,
                version: FlyStr::new(api_levels.iter().map(|s| s.as_ref()).join(",")),
            }
        }
    }
    pub fn scope(&self) -> Scope {
        self.scope
    }
    pub fn api_level(&self) -> &str {
        self.version.as_str()
    }
}

fn main() -> Result<ExitCode> {
    let args: Args = argh::from_env();
    let config = Configuration::new(&args);

    let irs: Result<Vec<Rc<ir::IR>>> = args
        .json
        .iter()
        .map(|path| ir::IR::load(path).context(format!("loading {path:?}")))
        .collect();
    let irs = irs?;

    let abi_surfaces: Result<Vec<AbiSurface>> = irs
        .into_iter()
        .map(|ir: Rc<ir::IR>| -> Result<AbiSurface> {
            convert::convert_abi_surface(ir).context(format!("converting IR"))
        })
        .collect();
    let abi_surfaces = abi_surfaces?;

    let mut problems = CompatibilityProblems::default();

    for (i, first) in abi_surfaces.iter().enumerate() {
        for second in abi_surfaces.iter().skip(i + 1) {
            problems.append(compare::compatible([first, second], &config)?);
        }
    }

    let has_errors;
    {
        let mut out = File::create(args.out).unwrap();

        let (errors, warnings) = problems.into_errors_and_warnings();

        has_errors = errors.len() > 0;

        for inc in errors {
            writeln!(out, "{}", inc)?;
            if args.enforce {
                writeln!(stderr(), "{}", inc)?;
            }
        }
        for inc in warnings {
            writeln!(out, "{}", inc)?;
        }
    }

    // Exiting rather than returning saves a lot of time in unnecessary Drop implementations.
    if args.enforce && has_errors {
        std::process::exit(1);
    } else {
        std::process::exit(0);
    }
}
