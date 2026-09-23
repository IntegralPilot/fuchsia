// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_dash as fdash;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "explore",
    description = "Resolves a package and then spawns a shell with said package loaded into the namespace at /pkg.",
    example = "To explore the update package interactively:

> ffx target package explore 'fuchsia-pkg://fuchsia.com/update'
$ ls
svc
pkg
$ exit
Connection to terminal closed

To run a command directly from the command line:
> ffx target package explore 'fuchsia-pkg://fuchsia.com/update' -c 'printenv'
PATH=/.dash/tools/debug-dash-launcher
PWD=/
",
    note = "The environment contains the following directories:
* /.dash    User-added and built-in dash tools
* /pkg      The package directory of the resolved package
* /svc      Protocols required by the dash shell

If additional binaries are provided via --tools, they will be loaded into .dash/tools/<pkg>/<binary>
The path is set so that they can be run by name. The path preference is in the command line order
of the --tools arguments, ending with the built-in dash tools (/.dash/tools/debug-dash-launcher).

--tools URLs may be package or binary URLs. Note that collisions can occur if different URLs have
the same package and binary names. An error, `NonUniqueBinaryName`, is returned if a binary name
collision occurs.
"
)]
pub struct ExploreCommand {
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
        _ => return Err("supported fuchisa-pkg resolvers are: 'base' and 'full'".into()),
    })
}
