// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::subcommands::composite::args::CompositeCommand;
use super::subcommands::disable::args::DisableCommand;
use super::subcommands::doctor::args::DoctorCommand;
use super::subcommands::dump::args::DumpCommand;
use super::subcommands::enable::args::EnableCommand;
use super::subcommands::host::args::HostCommand;
use super::subcommands::list::args::ListCommand;
use super::subcommands::list_composite_node_specs::args::ListCompositeNodeSpecsCommand;
use super::subcommands::list_composites::args::ListCompositesCommand;
use super::subcommands::list_devices::args::ListDevicesCommand;
use super::subcommands::list_hosts::args::ListHostsCommand;
use super::subcommands::node::args::NodeCommand;
use super::subcommands::register::args::RegisterCommand;
use super::subcommands::restart::args::RestartCommand;
use super::subcommands::show::args::ShowCommand;
use super::subcommands::test_node::args::TestNodeCommand;
use argh::{ArgsInfo, FromArgs};

#[derive(Debug, PartialEq)]
pub struct Boxed<T>(pub Box<T>);

impl<T: argh::FromArgs> argh::FromArgs for Boxed<T> {
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {
        T::from_args(command_name, args).map(|t| Boxed(Box::new(t)))
    }
    fn redact_arg_values(
        command_name: &[&str],
        args: &[&str],
    ) -> Result<Vec<String>, argh::EarlyExit> {
        T::redact_arg_values(command_name, args)
    }
}

impl<T: argh::SubCommand> argh::SubCommand for Boxed<T> {
    const COMMAND: &'static argh::CommandInfo = T::COMMAND;
}

impl<T: argh::ArgsInfo> argh::ArgsInfo for Boxed<T> {
    fn get_args_info() -> argh::CommandInfoWithArgs {
        T::get_args_info()
    }
}

#[cfg(not(target_os = "fuchsia"))]
use static_checks_lib::args::StaticChecksCommand;

pub type BoxedDisableCommand = Boxed<DisableCommand>;
pub type BoxedEnableCommand = Boxed<EnableCommand>;
pub type BoxedDoctorCommand = Boxed<DoctorCommand>;
pub type BoxedDumpCommand = Boxed<DumpCommand>;
pub type BoxedListCommand = Boxed<ListCommand>;
pub type BoxedListCompositesCommand = Boxed<ListCompositesCommand>;
pub type BoxedListDevicesCommand = Boxed<ListDevicesCommand>;
pub type BoxedListHostsCommand = Boxed<ListHostsCommand>;
pub type BoxedListCompositeNodeSpecsCommand = Boxed<ListCompositeNodeSpecsCommand>;
pub type BoxedRegisterCommand = Boxed<RegisterCommand>;
pub type BoxedRestartCommand = Boxed<RestartCommand>;
pub type BoxedShowCommand = Boxed<ShowCommand>;
pub type BoxedTestNodeCommand = Boxed<TestNodeCommand>;
pub type BoxedNodeCommand = Boxed<NodeCommand>;
pub type BoxedHostCommand = Boxed<HostCommand>;
pub type BoxedCompositeCommand = Boxed<CompositeCommand>;
#[cfg(not(target_os = "fuchsia"))]
pub type BoxedStaticChecksCommand = Boxed<StaticChecksCommand>;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(name = "driver", description = "Support driver development workflows")]
pub struct DriverCommand {
    /// if this exists, the user will be prompted for a component to select.
    #[argh(switch, short = 's', long = "select")]
    pub select: bool,

    #[argh(subcommand)]
    pub subcommand: DriverSubCommand,
}

#[cfg(target_os = "fuchsia")]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum DriverSubCommand {
    Disable(BoxedDisableCommand),
    Enable(BoxedEnableCommand),
    Doctor(BoxedDoctorCommand),
    Dump(BoxedDumpCommand),
    List(BoxedListCommand),
    ListComposites(BoxedListCompositesCommand),
    ListDevices(BoxedListDevicesCommand),
    ListHosts(BoxedListHostsCommand),
    ListCompositeNodeSpecs(BoxedListCompositeNodeSpecsCommand),
    Register(BoxedRegisterCommand),
    Restart(BoxedRestartCommand),
    Show(BoxedShowCommand),
    TestNode(BoxedTestNodeCommand),
    // New and improved driver commands.
    Node(BoxedNodeCommand),
    Host(BoxedHostCommand),
    Composite(BoxedCompositeCommand),
}

#[cfg(not(target_os = "fuchsia"))]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum DriverSubCommand {
    Disable(BoxedDisableCommand),
    Enable(BoxedEnableCommand),
    Doctor(BoxedDoctorCommand),
    Dump(BoxedDumpCommand),
    List(BoxedListCommand),
    ListComposites(BoxedListCompositesCommand),
    ListDevices(BoxedListDevicesCommand),
    ListHosts(BoxedListHostsCommand),
    ListCompositeNodeSpecs(BoxedListCompositeNodeSpecsCommand),
    Register(BoxedRegisterCommand),
    Restart(BoxedRestartCommand),
    StaticChecks(BoxedStaticChecksCommand),
    Show(BoxedShowCommand),
    TestNode(BoxedTestNodeCommand),
    // New and improved driver commands.
    Node(BoxedNodeCommand),
    Host(BoxedHostCommand),
    Composite(BoxedCompositeCommand),
}
