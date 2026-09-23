// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fidl_fuchsia_dash as fdash;

#[derive(FromArgs, PartialEq, Debug)]
#[argh(name = "package", description = "Interact with the packaging system.")]
pub struct PackageArgs {
    #[argh(subcommand)]
    pub subcommand: PackageSubcommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum PackageSubcommand {
    Explore(ExploreArgs),
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "explore", description = "Same as `ffx target package explore`")]
pub struct ExploreArgs {
    #[argh(positional)]
    /// the package URL to resolve. If `subpackages` is empty the resolved package directory will
    /// be loaded into the shell's namespace at `/pkg`.
    pub url: String,

    #[argh(option, long = "subpackage")]
    /// the chain of subpackages, if any, of `url` to resolve, in resolution order.
    /// If `subpackages` is not empty, the package directory of the final subpackage will be
    /// loaded into the shell's namespace at `/pkg`.
    pub subpackages: Vec<String>,

    #[argh(option)]
    /// list of URLs of tools packages to include in the shell environment.
    /// the PATH variable will be updated to include binaries from these tools packages.
    /// repeat `--tools url` for each package to be included.
    /// The path preference is given by command line order.
    pub tools: Vec<String>,

    #[argh(option, short = 'c', long = "command")]
    /// execute a command instead of reading from stdin.
    /// the exit code of the command will be forwarded to the host.
    pub command: Option<String>,

    #[argh(option, default = "fdash::FuchsiaPkgResolver::Full", from_str_fn(parse_resolver))]
    /// the resolver to use when resolving package URLs with scheme "fuchsia-pkg".
    /// Possible values are "base" and "full". Defaults to "full".
    pub fuchsia_pkg_resolver: fdash::FuchsiaPkgResolver,
}

fn parse_resolver(flag: &str) -> Result<fdash::FuchsiaPkgResolver, String> {
    Ok(match flag {
        "base" => fdash::FuchsiaPkgResolver::Base,
        "full" => fdash::FuchsiaPkgResolver::Full,
        _ => return Err("supported fuchsia-pkg resolvers are: 'base' and 'full'".into()),
    })
}
