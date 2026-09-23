// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::{Nla, NlaBuffer};
use netlink_packet_utils::parsers::parse_u32;
use netlink_packet_utils::{DecodeError, Parseable};

use crate::nl80211::constants::*;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211SchedScanPlanAttr {
    Interval(u32),
    Iterations(u32),
}

impl Nla for Nl80211SchedScanPlanAttr {
    fn value_len(&self) -> usize {
        use Nl80211SchedScanPlanAttr::*;
        match self {
            Interval(_) | Iterations(_) => std::mem::size_of::<u32>(),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211SchedScanPlanAttr::*;
        match self {
            Interval(_) => NL80211_SCHED_SCAN_PLAN_INTERVAL,
            Iterations(_) => NL80211_SCHED_SCAN_PLAN_ITERATIONS,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211SchedScanPlanAttr::*;
        match self {
            Interval(val) => NativeEndian::write_u32(buffer, *val),
            Iterations(val) => NativeEndian::write_u32(buffer, *val),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for Nl80211SchedScanPlanAttr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            NL80211_SCHED_SCAN_PLAN_INTERVAL => Self::Interval(
                parse_u32(payload).context("Invalid NL80211_SCHED_SCAN_PLAN_INTERVAL value")?,
            ),
            NL80211_SCHED_SCAN_PLAN_ITERATIONS => Self::Iterations(
                parse_u32(payload).context("Invalid NL80211_SCHED_SCAN_PLAN_ITERATIONS value")?,
            ),
            other => {
                return Err(DecodeError::from(format!(
                    "Unhandled NL80211_ATTR_SCHED_SCAN_PLANS attribute: {other}"
                )));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netlink_packet_utils::Emitable;
    use test_case::test_case;

    #[test_case(
        Nl80211SchedScanPlanAttr::Interval(100),
        vec![8, 0, NL80211_SCHED_SCAN_PLAN_INTERVAL as u8, 0, 100, 0, 0, 0] ;
        "interval"
    )]
    #[test_case(
        Nl80211SchedScanPlanAttr::Iterations(5),
        vec![8, 0, NL80211_SCHED_SCAN_PLAN_ITERATIONS as u8, 0, 5, 0, 0, 0] ;
        "iterations"
    )]
    fn emit_and_parse_test(attr: Nl80211SchedScanPlanAttr, bytes: Vec<u8>) {
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);
        assert_eq!(buffer, bytes);

        let parsed_attr =
            Nl80211SchedScanPlanAttr::parse(&NlaBuffer::new(&bytes).unwrap()).unwrap();
        assert_eq!(parsed_attr, attr);
    }
}
