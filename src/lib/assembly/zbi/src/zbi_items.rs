// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! These are structures for creating the binary data for ZBI Items.

use anyhow::{Result, bail};
use zerocopy::IntoBytes;

pub fn new_platform_id(
    vid: u32,
    pid: u32,
    board_name: impl Into<String>,
) -> Result<zbi::PlatformId> {
    let board_name: String = board_name.into();
    if board_name.as_bytes().len() > 32 {
        bail!("board_name is too long (max 32 bytes): {}", board_name.as_bytes().len());
    }
    let mut platform_id = zbi::PlatformId { vid, pid, board_name: [0u8; 32] };
    let _ = board_name.as_bytes().write_to_prefix(&mut platform_id.board_name);
    Ok(platform_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_creation() {
        let platform_id = new_platform_id(23, 42, "some board").unwrap();
        let platform_id_bytes = platform_id.as_bytes();

        #[rustfmt::skip]
        assert_eq!(
            [
                // vid
                23u8, 0, 0, 0,

                //pid
                42, 0, 0, 0,

                //"some board"
                b's', b'o', b'm', b'e', b' ', b'b', b'o', b'a', b'r', b'd',

                //padding
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ],
            platform_id_bytes
        );
    }

    #[test]
    fn test_fails_on_long_name() {
        let result = new_platform_id(0, 0, "this is far, far, too long of a name");
        assert!(result.is_err());
    }
}
