// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use component_debug::cli::{GraphFilter, GraphOrientation, ListFilter};
use component_debug::config::RawConfigEntry;
use component_debug::explore::DashNamespaceLayout;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use moniker::Moniker;

#[derive(FromArgs, PartialEq, Debug)]
#[argh(
    name = "component",
    description = "Discover and manage components. Functionally equivalent to `ffx component`."
)]
pub struct ComponentArgs {
    #[argh(subcommand)]
    pub subcommand: ComponentSubcommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum ComponentSubcommand {
    Capability(CapabilityArgs),
    List(ListArgs),
    Graph(GraphArgs),
    Show(ShowArgs),
    Create(CreateArgs),
    Destroy(DestroyArgs),
    Resolve(ResolveArgs),
    Run(RunArgs),
    Start(StartArgs),
    Stop(StopArgs),
    Reload(ReloadArgs),
    Doctor(DoctorArgs),
    Copy(CopyArgs),
    Route(RouteArgs),
    Storage(StorageArgs),
    Collection(CollectionArgs),
    Explore(ExploreArgs),
    Config(ConfigArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "show", description = "Same as `ffx component show`")]
pub struct ShowArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "create", description = "Same as `ffx component create`")]
pub struct CreateArgs {
    #[argh(positional)]
    pub moniker: Moniker,

    #[argh(positional)]
    pub url: AbsoluteComponentUrl,

    #[argh(option)]
    /// provide a configuration override to the component being run. Requires
    /// `mutability: [ "parent" ]` on the configuration field. Specified in the format
    /// `KEY=VALUE` where `VALUE` is a JSON string which can be resolved as the correct type of
    /// configuration value.
    pub config: Vec<RawConfigEntry>,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "resolve", description = "Same as `ffx component resolve`")]
pub struct ResolveArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "destroy", description = "Same as `ffx component destroy`")]
pub struct DestroyArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "start", description = "Same as `ffx component start`")]
pub struct StartArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "stop", description = "Same as `ffx component stop`")]
pub struct StopArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "reload", description = "Same as `ffx component reload`")]
pub struct ReloadArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "doctor", description = "Same as `ffx component doctor`")]
pub struct DoctorArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "capability", description = "Same as `ffx component capability`")]
pub struct CapabilityArgs {
    #[argh(positional)]
    pub capability_name: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "list", description = "Same as `ffx component list`")]
pub struct ListArgs {
    #[argh(option, long = "only", short = 'o')]
    /// filter the instance list by a criteria: running, stopped, ancestors:<component_name>, descendants:<component_name>, or relatives:<component_name>
    pub filter: Option<ListFilter>,

    #[argh(switch, long = "verbose", short = 'v')]
    /// show detailed information about each instance
    pub verbose: bool,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "graph", description = "Same as `ffx component graph`")]
pub struct GraphArgs {
    #[argh(option, long = "only", short = 'o')]
    /// filter the instance list by a criteria: ancestor, descendant, relative
    pub filter: Option<GraphFilter>,

    #[argh(option, long = "orientation", short = 'r', default = "GraphOrientation::TopToBottom")]
    /// changes the visual orientation of the graph's nodes.
    /// Allowed values are "lefttoright"/"lr" and "toptobottom"/"tb".
    pub orientation: GraphOrientation,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "run", description = "Same as `ffx component run`")]
pub struct RunArgs {
    #[argh(positional)]
    pub moniker: Moniker,

    #[argh(positional)]
    pub url: AbsoluteComponentUrl,

    #[argh(switch, short = 'r')]
    /// destroy and recreate the component instance if it already exists
    pub recreate: bool,

    #[argh(switch)]
    /// connect stdin, stdout, and stderr to the component (requires component
    /// to be in a collection with single_run durability)
    pub connect_stdio: bool,

    #[argh(option)]
    /// provide a configuration override to the component being run. Requires
    /// `mutability: [ "parent" ]` on the configuration field. Specified in the format
    /// `KEY=VALUE` where `VALUE` is a JSON string which can be resolved as the correct type of
    /// configuration value.
    pub config: Vec<RawConfigEntry>,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "route", description = "Same as `ffx component route`")]
pub struct RouteArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID of target component. Partial matches allowed.
    pub target: String,

    #[argh(positional)]
    /// optional filter of comma-separated capability names or <decl type>:<capability> pairs.
    /// If provided, <decl type> is one of `use` or `expose`, otherwise it will match both used
    /// and exposed capabilities. Partial matches on <capability> are allowed.
    ///
    /// Examples:
    /// - fuchsia.pkg.FontResolver
    /// - use:fuchsia.pkg.FontResolver
    /// - expose:fuchsia.fonts.Provider
    /// - fuchsia.pkg.FontResolver,expose:fuchsia.fonts.Provider
    /// - fuchsia.pkg
    pub filter: Option<String>,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "copy", description = "Same as `ffx component copy`")]
pub struct CopyArgs {
    #[argh(positional)]
    pub paths: Vec<String>,
    /// show detailed information about copy action
    #[argh(switch, short = 'v')]
    pub verbose: bool,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "storage", description = "Same as `ffx component storage`")]
pub struct StorageArgs {
    #[argh(subcommand)]
    pub subcommand: StorageSubcommand,

    #[argh(option, default = "String::from(\"/core\")")]
    /// the moniker of the storage provider component.
    /// Defaults to "/core"
    pub provider: String,

    #[argh(option, default = "String::from(\"data\")")]
    /// the capability name of the storage to use.
    /// Examples: "data", "cache", "tmp"
    /// Defaults to "data"
    pub capability: String,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum StorageSubcommand {
    Copy(StorageCopyArgs),
    Delete(StorageDeleteArgs),
    List(StorageListArgs),
    MakeDirectory(StorageMakeDirectoryArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list", description = "Same as `ffx component storage list`")]
pub struct StorageListArgs {
    #[argh(positional)]
    pub path: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "make-directory",
    description = "Same as `ffx component storage make-directory`"
)]
pub struct StorageMakeDirectoryArgs {
    #[argh(positional)]
    pub path: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "copy", description = "Same as `ffx component storage copy`")]
pub struct StorageCopyArgs {
    #[argh(positional)]
    pub source_path: String,

    #[argh(positional)]
    pub destination_path: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "delete", description = "Same as `ffx component storage delete`")]
pub struct StorageDeleteArgs {
    #[argh(positional)]
    pub path: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "collection", description = "Same as `ffx component collection`")]
pub struct CollectionArgs {
    #[argh(subcommand)]
    pub subcommand: CollectionSubcommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum CollectionSubcommand {
    List(CollectionListArgs),
    Show(CollectionShowArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list", description = "Same as `ffx component collection list`")]
pub struct CollectionListArgs {
    #[argh(positional)]
    pub path: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "show", description = "Same as `ffx component collection show`")]
pub struct CollectionShowArgs {
    #[argh(positional)]
    pub query: String,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "explore", description = "Same as `ffx component explore`")]
pub struct ExploreArgs {
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

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "config", description = "Same as `ffx component config`")]
pub struct ConfigArgs {
    #[argh(subcommand)]
    pub subcommand: ConfigSubcommand,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum ConfigSubcommand {
    Set(SetArgs),
    Unset(UnsetArgs),
    List(ConfigListArgs),
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "set", description = "Same as `ffx component config set`")]
pub struct SetArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
    #[argh(positional)]
    /// key-value pairs of the configuration capability to be overridden. Takes
    /// the form 'key="value"'.
    pub key_values: Vec<String>,
    #[argh(switch, short = 'r')]
    /// if enabled, component instance will be immediately reloaded so overrides
    /// take effect.
    pub reload: bool,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "unset", description = "Same as `ffx component config unset`")]
pub struct UnsetArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed. If
    /// empty, unsets structured config overrides for all components.
    pub query: Option<String>,
    #[argh(switch, short = 'r')]
    /// if enabled, component instance will be immediately reloaded so overrides
    /// take effect.
    pub reload: bool,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "list", description = "Same as `ffx component config list`")]
pub struct ConfigListArgs {
    #[argh(positional)]
    /// component URL, moniker or instance ID. Partial matches allowed.
    pub query: String,
}
