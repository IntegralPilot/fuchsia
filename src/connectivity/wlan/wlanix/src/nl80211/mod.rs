// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use netlink_packet_generic::{GenlFamily, GenlHeader};
use netlink_packet_utils::nla::NlasIterator;
use netlink_packet_utils::traits::{Emitable, ParseableParametrized};
use netlink_packet_utils::{DecodeError, Parseable};
use std::convert::TryInto;

mod attr;
mod cmd;
#[allow(unused)]
mod constants;
mod nested;

pub use attr::*;
pub use cmd::*;

#[derive(Debug)]
pub struct Nl80211 {
    pub cmd: Nl80211Cmd,
    pub attrs: Vec<Nl80211Attr>,
}

impl GenlFamily for Nl80211 {
    fn family_name() -> &'static str {
        "nl80211"
    }

    fn command(&self) -> u8 {
        self.cmd as u8
    }

    fn version(&self) -> u8 {
        1
    }
}

impl Emitable for Nl80211 {
    fn emit(&self, buffer: &mut [u8]) {
        self.attrs.as_slice().emit(buffer)
    }

    fn buffer_len(&self) -> usize {
        self.attrs.as_slice().buffer_len()
    }
}

impl ParseableParametrized<[u8], GenlHeader> for Nl80211 {
    type Error = DecodeError;
    fn parse_with_param(buf: &[u8], header: GenlHeader) -> Result<Self, DecodeError> {
        let cmd = header.cmd.try_into()?;
        let attrs = NlasIterator::new(buf)
            .map(|nla| nla.map_err(DecodeError::from).and_then(|nla| Nl80211Attr::parse(&nla)))
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse NL80211 attributes")?;
        Ok(Self { cmd, attrs })
    }
}
