// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use component_debug_fdomain::explore::DashNamespaceLayout;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "explore",
    description = "Spawns a shell scoped to a component instance.",
    example = "To explore the Archivist instance interactively:

> ffx component explore /bootstrap/archivist
$ ls
exposed
ns
out
runtime
svc
$ exit
Connection to terminal closed

To run a command directly from the command line:
> ffx component explore /bootstrap/archivist -c 'printenv'
PATH=/.dash/tools/debug-dash-launcher
PWD=/
",
    note = "When --layout=nested (the default), the environment contains the following
directories of the explored instance:
* /ns       The namespace of the instance
* /exposed  The capabilities exposed by the instance
* /out      The outgoing directory of the instance, if it is running
* /runtime  The runtime directory of the instance, if it is running

The environment also contains the following directories, irrespective of the explored instance:
* /.dash    User-added and built-in dash tools
* /svc      Protocols required by the dash shell

When --layout=namespace, the contents of the /ns dir above are placed at / with two protocols
(fuchsia.process.Launcher and fuchsia.process.Resolver) overlayed into /svc. In this mode, the
exposed, out, and runtime directories will not be accessible.

With --tools, tools in the specified package will be loaded into /.dash/tools/<pkg>/<binary>
The path is set so that they can be run by name. The path preference is in the command line order
of the --tools arguments, with any built-in tools taking lowest priority.

--tools URLs may be package or binary URLs. If a package URL is given, tool executables are expected
to be in a bin/ dir within the package. If a specific tool URL is given, append the tool path
to the package URL. For example: --tools fuchsia-pkg://fuchsia.com/my_package#bin/my_tool. Note that
naming collisions can occur if multiple packages share a package or binary name. An error,
`NonUniqueBinaryName`, is returned if a binary name collision occurs.
"
)]

pub struct ExploreComponentCommand {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,

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

    #[argh(
        option,
        short = 'l',
        long = "layout",
        default = "DashNamespaceLayout::NestAllInstanceDirs"
    )]
    /// changes the namespace layout that is created for the shell.
    /// nested: nests all instance directories under subdirs (default)
    /// namespace: sets the instance namespace as the root (works better for tools)
    pub ns_layout: DashNamespaceLayout,
}
