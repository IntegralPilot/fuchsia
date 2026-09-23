// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgValue, FromArgs};
use std::fmt;

#[cfg(not(target_os = "fuchsia"))]
use ffx_core::ffx_command;

#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GuestType {
    Debian,
    Zircon,
}

impl FromArgValue for GuestType {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "debian" => Ok(Self::Debian),
            "zircon" => Ok(Self::Zircon),
            _ => Err(format!(
                "Unrecognized guest type \"{}\". Supported guest types are: \
                \"debian\", \"zircon\".",
                value
            )),
        }
    }
}

impl fmt::Display for GuestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            GuestType::Debian => write!(f, "debian"),
            GuestType::Zircon => write!(f, "zircon"),
        }
    }
}

impl GuestType {
    pub fn moniker(&self) -> &str {
        match self {
            GuestType::Debian => "/core/debian-guest-manager",
            GuestType::Zircon => "/core/zircon-guest-manager",
        }
    }

    pub fn guest_manager_interface(&self) -> &str {
        match *self {
            GuestType::Zircon => "fuchsia.virtualization.ZirconGuestManager",
            GuestType::Debian => "fuchsia.virtualization.DebianGuestManager",
        }
    }

    pub fn gn_target_label(self) -> &'static str {
        match self {
            GuestType::Zircon => "//src/virtualization/bundles:zircon",
            GuestType::Debian => "//src/virtualization/bundles:debian",
        }
    }

    pub fn gn_core_shard_label(&self) -> &'static str {
        match self {
            GuestType::Zircon => "//src/virtualization/bundles:zircon_core_shards",
            GuestType::Debian => "//src/virtualization/bundles:debian_core_shards",
        }
    }

    pub fn package_url(&self) -> &'static str {
        match self {
            GuestType::Zircon => "fuchsia-pkg://fuchsia.com/zircon_guest#meta/zircon_guest.cm",
            GuestType::Debian => "fuchsia-pkg://fuchsia.com/debian_guest#meta/debian_guest.cm",
        }
    }

    pub fn all_guests() -> Vec<GuestType> {
        vec![GuestType::Debian, GuestType::Zircon]
    }
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Top-level command.
pub struct GuestOptions {
    #[argh(subcommand)]
    pub nested: SubCommands,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommands {
    Attach(crate::attach_args::AttachArgs),
    Launch(crate::launch_args::LaunchArgs),
    Stop(crate::stop_args::StopArgs),
    Balloon(crate::balloon_args::BalloonArgs),
    List(crate::list_args::ListArgs),
    Socat(crate::socat_args::SocatArgs),
    VsockPerf(crate::vsockperf_args::VsockPerfArgs),
    Mem(crate::mem_args::MemArgs),
}

pub mod mem_args {
    use super::*;
    /// Interact with the guest virtio-mem. Usage: guest mem sub-command [ request-plugged or stats]
    #[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
    #[argh(subcommand, name = "mem")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct MemArgs {
        #[argh(subcommand)]
        pub mem_cmd: MemCommands,
    }
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    #[argh(subcommand)]
    pub enum MemCommands {
        RequestPluggedMem(RequestPluggedMem),
        StatsMem(StatsMem),
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Modify the requested amount of the dynamically plugged memory. Usage: guest mem request-plugged guest-type size
    #[argh(subcommand, name = "request-plugged")]
    pub struct RequestPluggedMem {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
        #[argh(positional)]
        /// target amount of memory to be dynamically plugged
        pub size: u64,
    }
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// See the stats of a guest's virtio-mem. Usage: guest mem stats guest-type
    #[argh(subcommand, name = "stats")]
    pub struct StatsMem {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
    }
}

pub mod balloon_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Interact with the guest memory balloon. Usage: guest balloon sub-command guest-type ...
    #[argh(subcommand, name = "balloon")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct BalloonArgs {
        #[argh(subcommand)]
        pub balloon_cmd: BalloonCommands,
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    #[argh(subcommand)]
    pub enum BalloonCommands {
        Set(BalloonSet),
        Stats(BalloonStats),
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Modify the size of a memory balloon. Usage: guest balloon set guest-type num-pages
    #[argh(subcommand, name = "set")]
    pub struct BalloonSet {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
        #[argh(positional)]
        /// number of pages guest balloon will have after use.
        pub num_pages: u32,
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// See the stats of a guest's memory balloon. Usage: guest balloon stats guest-type
    #[argh(subcommand, name = "stats")]
    pub struct BalloonStats {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
    }
}

pub mod list_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// List available guest environments.
    #[argh(subcommand, name = "list")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct ListArgs {
        #[argh(positional)]
        /// optional guest type to get detailed information about
        pub guest_type: Option<GuestType>,
    }
}

pub mod socat_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Interact with the guest via socat. See the sub-command help for details.
    #[argh(subcommand, name = "socat")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct SocatArgs {
        #[argh(subcommand)]
        pub socat_cmd: SocatCommands,
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    #[argh(subcommand)]
    pub enum SocatCommands {
        Listen(SocatListen),
        Connect(SocatConnect),
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Create a socat connection on the specified port. Usage: guest socat connect guest-type port
    #[argh(subcommand, name = "connect")]
    pub struct SocatConnect {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
        #[argh(positional)]
        /// guest port number to attempt to connect to
        pub guest_port: u32,
    }

    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Listen through socat on the specified port. Usage: guest socat listen guest-type host-port
    #[argh(subcommand, name = "listen")]
    pub struct SocatListen {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
        #[argh(positional)]
        /// host port number to accept incoming guest connections on
        pub host_port: u32,
    }
}

pub mod vsockperf_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Perform a vsock micro benchmark on the target guest. Only Debian is supported.
    #[argh(subcommand, name = "vsock-perf")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct VsockPerfArgs {
        #[argh(positional)]
        /// type of the guest
        pub guest_type: GuestType,
    }
}

pub mod launch_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Launch a guest image. Usage: guest launch guest_type [--cmdline-add <arg>...] [--default-net <bool>] [--memory <memory-size>] [--cpus <num-cpus>] [--virtio-* <bool>]
    #[argh(subcommand, name = "launch")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct LaunchArgs {
        #[argh(positional)]
        /// guest type to launch e.g. 'zircon'.
        pub guest_type: GuestType,
        /// adds provided strings to the existing kernel command line
        #[argh(option)]
        pub cmdline_add: Vec<String>,
        /// enable a default net device
        #[argh(option)]
        pub default_net: Option<bool>,
        /// allocate 'bytes' of memory for the guest
        #[argh(option)]
        pub memory: Option<u64>,
        /// number of virtual cpus available for the guest
        #[argh(option)]
        pub cpus: Option<u8>,
        /// enable virtio-balloon
        #[argh(option)]
        pub virtio_balloon: Option<bool>,
        /// enable virtio-console
        #[argh(option)]
        pub virtio_console: Option<bool>,
        /// enable virtio-gpu and virtio-input
        #[argh(option)]
        pub virtio_gpu: Option<bool>,
        /// enable virtio-rng
        #[argh(option)]
        pub virtio_rng: Option<bool>,
        /// enable virtio-sound
        #[argh(option)]
        pub virtio_sound: Option<bool>,
        /// enable virtio-sound-input
        #[argh(option)]
        pub virtio_sound_input: Option<bool>,
        /// enable virtio-vsock
        #[argh(option)]
        pub virtio_vsock: Option<bool>,
        /// enable virtio-mem to allow dynamically (un)plug memory to the guest
        #[argh(option)]
        pub virtio_mem: Option<bool>,
        /// virtio-mem pluggable region size
        #[argh(option)]
        pub virtio_mem_region_size: Option<u64>,
        /// virtio-mem pluggable region alignment
        #[argh(option)]
        pub virtio_mem_region_alignment: Option<u64>,
        /// virtio-mem pluggable blocksize
        #[argh(option)]
        pub virtio_mem_block_size: Option<u64>,
        /// detach from a guest allowing it to run in the background
        #[argh(switch, short = 'd')]
        pub detach: bool,
    }
}

pub mod stop_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Stop a running guest. Usage: guest stop guest_type [-f]
    #[argh(subcommand, name = "stop")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct StopArgs {
        /// guest type to stop e.g. 'zircon'
        #[argh(positional)]
        pub guest_type: GuestType,
        /// force stop the guest
        #[argh(switch, short = 'f')]
        pub force: bool,
    }
}

pub mod attach_args {
    use super::*;
    #[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
    /// Attach console and serial to a running guest. Usage: guest attach guest_type
    #[argh(subcommand, name = "attach")]
    #[cfg_attr(not(target_os = "fuchsia"), ffx_command())]
    pub struct AttachArgs {
        /// guest type to attach to e.g. 'debian'
        #[argh(positional)]
        pub guest_type: GuestType,
        /// attach via serial instead of virtio-console
        #[argh(switch)]
        pub serial: bool,
    }
}
