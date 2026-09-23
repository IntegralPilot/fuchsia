// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_filter_ext as filter_ext;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar_inline = r#"
rules = { SOI ~ (rule ~ ";")+ ~ EOI }

rule = {
     action ~
     direction ~
     proto ~
     devclass ~
     src ~
     dst ~
     log ~
     state
}

nat_rules = { SOI ~ (nat ~ ";")+ ~ EOI }

nat = {
     "nat" ~
     proto ~
     "from" ~ subnet ~
     "->"  ~
     "from" ~ ipaddr
}

rdr_rules = { SOI ~ (rdr ~ ";")+ ~ EOI }

rdr = {
     "rdr" ~
     proto ~
     "to" ~ ipaddr ~ port_range ~
     "->" ~
     "to" ~ ipaddr ~ port_range
}

action = { pass | drop | dropreset }
  pass = { "pass" }
  drop = { "drop" }
  dropreset = { "dropreset" }

direction = { incoming | outgoing }
  incoming = { "in" }
  outgoing = { "out" }

proto = { ("proto" ~ (tcp | udp | icmp))? }
  tcp = { "tcp" }
  udp = { "udp" }
  icmp = { "icmp" }

devclass = { ("devclass" ~ (virt | ethernet | wlan | ppp | bridge | ap | lowpan))? }
  virt = { "virt" }
  ethernet = { "ethernet" }
  wlan = { "wlan" }
  ppp = { "ppp" }
  bridge = { "bridge" }
  ap = { "ap" }
  lowpan = { "lowpan" }

src = { ("from" ~ invertible_subnet? ~ port_range?)? }
dst = { ("to" ~ invertible_subnet? ~ port_range?)? }

invertible_subnet = { not? ~ subnet }
  not = { "!" }

subnet = ${ ipaddr ~ "/" ~ prefix_len }

ipaddr = { ipv4addr | ipv6addr }
  ipv4addr = @{ ASCII_DIGIT{1,3} ~ ("." ~ ASCII_DIGIT{1,3}){3} }
  ipv6addr = @{ (ASCII_HEX_DIGIT | ":")+ }

prefix_len = @{ ASCII_DIGIT+ }

port_range = { port ~ port_num | range ~ port_num ~ ":" ~ port_num }
  port = { "port" }
  range = { "range" }
  port_num = @{ ASCII_DIGIT+ }

log = { ("log")? }

state = { (state_adj ~ "state")? }
  state_adj = { "keep" | "no" }

WHITESPACE = _{ " " }
"#]

pub struct FilterRuleParser;

#[derive(Debug, PartialEq)]
pub enum Error {
    // Boxing `pest::error::Error<...>` because it's >184 bytes as of writing.
    Pest(Box<pest::error::Error<Rule>>),
    Addr(std::net::AddrParseError),
    Num(std::num::ParseIntError),
    Invalid(InvalidReason),
    RoutineNotProvided(crate::parser::Direction),
    Fidl(filter_ext::FidlConversionError),
}

#[derive(Debug, PartialEq)]
pub enum InvalidReason {
    MixedIPVersions,
    InvalidPortRange,
    PortRangeLengthMismatch,
}

impl std::fmt::Display for InvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MixedIPVersions => write!(f, "mixed IP versions"),
            Self::InvalidPortRange => write!(f, "invalid port range"),
            Self::PortRangeLengthMismatch => write!(f, "port range length mismatch"),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pest(e) => write!(f, "pest error: {}", e),
            Self::Addr(e) => std::fmt::Display::fmt(e, f),
            Self::Num(e) => std::fmt::Display::fmt(e, f),
            Self::Invalid(e) => write!(f, "invalid: {}", e),
            Self::RoutineNotProvided(e) => write!(f, "expected routine for direction: {:?}", e),
            Self::Fidl(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for Error {}
