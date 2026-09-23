// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, anyhow};
use argh::{ArgsInfo, FromArgs};
use bitflags::bitflags;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    short = 'l',
    example = "To list targets in short form:

    $ ffx target list --format s
    fe80::4415:3606:fb52:e2bc%zx-f80ff974f283 pecan-guru-clerk-rhyme

To list targets with only their addresses:

    $ ffx target list --format a
    fe80::4415:3606:fb52:e2bc%zx-f80ff974f283",
    description = "List all targets",
    note = "List all targets.

In daemon mode (default), this lists targets that the daemon currently has in memory. This includes
manually added targets. The daemon also proactively discovers targets as
they come online. Use `ffx target list` to always get the latest list
of targets. Targets are sorted by name.

In direct mode, this lists targets discovered by searching the local network (mDNS) and USB connections.

The default target is marked with a '*' next to the node name. The table
has the following columns:

    NAME = The name of the target.
    SERIAL = The serial number of the target.
    TYPE = The product type of the target.
    STATE = The high-level state of the target.
    ADDRS/IP = The discovered and known addresses of the target.
    RCS = Indicates if the Remote Control Service is running on the target.

The NAME column shows the target's advertised name. When the target is
in early boot state such as fastboot, the NAME column may be `<unknown>` with
a STATE being `fastboot` and a SERIAL attribute.

By default, the `list` command outputs in a tabular format. To override
the format, pass `--format` and can take the following options: 'simple|s'
, 'tabular|table|tab|t', 'addresses|addrs|addr|a', 'name-only|n', 'json|JSON|j' or
'serials'.

By default, Zedboot discovery is disabled.  To enable discovery of Zedboot
targets run:

    $ ffx config set discovery.zedboot.enabled true
",
    error_code(
        2,
        "If a nodename is supplied, an error code of 2 will be returned \
               if the nodename cannot be resolved"
    )
)]
pub struct ListCommand {
    #[argh(positional)]
    pub nodename: Option<String>,

    #[argh(option, short = 'f', default = "Format::Tabular")]
    /// determines the output format for the list operation
    pub format: Format,

    #[argh(
        switch,
        description = "do not return IPv4 addresses (deprecated, use --allow-addrs/--deny-addrs)"
    )]
    pub no_ipv4: bool,

    #[argh(
        switch,
        description = "do not return IPv6 addresses (deprecated, use --allow-addrs/--deny-addrs)"
    )]
    pub no_ipv6: bool,

    #[argh(option, default = "AddressTypes::all()")]
    /// list of address types to show in the output. Value is a comma-separated
    /// list of address types. Use 'ipv4', 'ip4', or '4' for IPv4 addresses,
    /// 'ipv6', 'ip6' or '6' for IPv6 addresses, 'usb' for USB addresses (e.g.
    /// "usb:cid:4"), and 'vsock' for VSOCK addresses (e.g. "vsock:cid:4"). You
    /// can also use "all" to accept any addresses, which is the default.
    /// --allow-addrs applies before --deny-addrs.
    pub allow_addrs: AddressTypes,

    #[argh(option, default = "AddressTypes::empty()")]
    /// list of address types to show in the output. Value is a comma-separated
    /// list of address types. Use 'ipv4', 'ip4', or '4' for IPv4 addresses,
    /// 'ipv6', 'ip6' or '6' for IPv6 addresses, 'usb' for USB addresses (e.g.
    /// "usb:cid:4"), and 'vsock' for VSOCK addresses (e.g. "vsock:cid:4"). You
    /// can also use "none" to accept any addresses, which is the default.
    /// --allow-addrs applies before --deny-addrs.
    pub deny_addrs: AddressTypes,

    #[argh(switch, description = "do not connect to targets (direct mode only)")]
    pub no_probe: bool,

    #[argh(switch, description = "do not do mDNS discovery (direct mode only)")]
    pub no_mdns: bool,

    #[argh(switch, description = "do not do USB discovery (direct mode only)")]
    pub no_usb: bool,
}

impl ListCommand {
    pub fn address_types(&self) -> AddressTypes {
        let mut ret = self.allow_addrs.difference(self.deny_addrs);

        if self.no_ipv4 {
            ret.remove(AddressTypes::IPV4)
        }

        if self.no_ipv6 {
            ret.remove(AddressTypes::IPV6)
        }

        ret
    }
}

impl Default for ListCommand {
    fn default() -> Self {
        ListCommand {
            nodename: None,
            format: Format::Tabular,
            no_ipv4: false,
            no_ipv6: false,
            allow_addrs: AddressTypes::all(),
            deny_addrs: AddressTypes::empty(),
            no_probe: false,
            no_mdns: false,
            no_usb: false,
        }
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct AddressTypes: u8 {
        const IPV4  = 0x01;
        const IPV6  = 0x02;
        const IP    = AddressTypes::IPV4.bits() | AddressTypes::IPV6.bits();
        const USB   = 0x04;
        const VSOCK = 0x08;
    }
}

impl Default for AddressTypes {
    fn default() -> Self {
        AddressTypes::all()
    }
}

impl std::str::FromStr for AddressTypes {
    type Err = Error;

    fn from_str(s: &str) -> Result<AddressTypes> {
        let mut ret = AddressTypes::empty();
        for item in s.split(',').map(|x| x.trim()) {
            match item {
                "ip" => ret.insert(AddressTypes::IP),
                "ipv4" | "ip4" | "4" => ret.insert(AddressTypes::IPV4),
                "ipv6" | "ip6" | "6" => ret.insert(AddressTypes::IPV6),
                "usb" => ret.insert(AddressTypes::USB),
                "vsock" => ret.insert(AddressTypes::VSOCK),
                _ => {
                    return Err(anyhow!(
                        "expected 'ip', 'ipv4', 'ip4', '4' 'ipv6', 'ip6', '6' 'usb', 'vsock', 'none', or 'all'"
                    ));
                }
            }
        }
        Ok(ret)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Format {
    #[default]
    Tabular,
    Simple,
    Addresses,
    AddressesWithLexicalScope,
    Serials,
    NameOnly,
    Json,
}

impl std::str::FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Format> {
        match s {
            "tabular" | "table" | "tab" | "t" => Ok(Format::Tabular),
            "simple" | "s" => Ok(Format::Simple),
            "addresses" | "a" | "addr" | "addrs" => Ok(Format::Addresses),
            "addresses-with-scope" => Ok(Format::AddressesWithLexicalScope),
            "serials" => Ok(Format::Serials),
            "name-only" | "n" => Ok(Format::NameOnly),
            "json" | "JSON" | "j" => Ok(Format::Json),
            _ => Err(anyhow!("expected 'tabular', 'simple', 'addresses', 'serials', or 'json'")),
        }
    }
}
