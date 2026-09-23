// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use anyhow::format_err;
use wlan_common::mac;
use zerocopy::Ref;

pub fn set_more_data(buffer: &mut [u8]) -> Result<(), Error> {
    let (frame_ctrl, _) = Ref::<&mut [u8], mac::FrameControl>::from_prefix(buffer)
        .map_err(|_| format_err!("could not parse frame control header"))?;
    let frame_ctrl = Ref::into_mut(frame_ctrl);
    frame_ctrl.set_more_data(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_data() {
        let mut buffer = vec![
            // Mgmt header
            0b00001000, 0b00000010, // Frame Control
            0, 0, // Duration
            3, 3, 3, 3, 3, 3, // addr1
            2, 2, 2, 2, 2, 2, // addr2
            1, 1, 1, 1, 1, 1, // addr3
            0x10, 0, // Sequence Control
            0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
            0, 0, 0, // OUI
            0x12, 0x34, // Protocol ID
            // Data
            1, 2, 3, 4, 5,
        ];
        set_more_data(&mut buffer[..]).expect("expected set more data OK");
        assert_eq!(
            &[
                // Mgmt header
                0b00001000, 0b00100010, // Frame Control
                0, 0, // Duration
                3, 3, 3, 3, 3, 3, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                1, 1, 1, 1, 1, 1, // addr3
                0x10, 0, // Sequence Control
                0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
                0, 0, 0, // OUI
                0x12, 0x34, // Protocol ID
                // Data
                1, 2, 3, 4, 5,
            ][..],
            &buffer[..]
        );
    }
}
