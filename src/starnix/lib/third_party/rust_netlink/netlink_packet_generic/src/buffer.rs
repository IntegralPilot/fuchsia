// SPDX-License-Identifier: MIT

//! Buffer definition of generic netlink packet
use crate::constants::GENL_HDRLEN;
use crate::header::GenlHeader;
use crate::message::GenlMessage;
use netlink_packet_utils::{DecodeError, Parseable, ParseableParametrized};
use std::fmt::Debug;

buffer!(GenlBuffer(GENL_HDRLEN) {
    cmd: (u8, 0),
    version: (u8, 1),
    payload: (slice, GENL_HDRLEN..),
});

impl<'a, F, T> ParseableParametrized<GenlBuffer<&'a T>, u16> for GenlMessage<F>
where
    F: ParseableParametrized<[u8], GenlHeader> + Debug,
    T: AsRef<[u8]> + ?Sized,
    F::Error: Into<DecodeError>,
{
    type Error = DecodeError;
    fn parse_with_param(buf: &GenlBuffer<&'a T>, message_type: u16) -> Result<Self, DecodeError> {
        let header = GenlHeader::parse(buf)?;
        let payload_buf = buf.payload();
        Ok(GenlMessage::new(
            header,
            F::parse_with_param(payload_buf, header).map_err(|err| err.into())?,
            message_type,
        ))
    }
}
