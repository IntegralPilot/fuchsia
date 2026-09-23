// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use anyhow::{Context, Result};
use zerocopy::byteorder::big_endian::{U32 as BigEndianU32, U64 as BigEndianU64};
use zerocopy::{Immutable, IntoBytes};

const MAGIC: [u8; 4] = *b"AVBf";

// These give the footer-specific version and should be kept in sync with
// libavb.
const VERSION_MAJOR: u32 = 1;
const VERSION_MINOR: u32 = 0;

// The footer to an image or partition used to identify an appended VBMeta
// (rather than as standalone contents).
#[derive(Debug, Immutable, IntoBytes)]
#[repr(C, packed)]
pub(crate) struct Footer {
    magic: [u8; 4],
    version_major: BigEndianU32,
    version_minor: BigEndianU32,
    original_image_size: BigEndianU64,
    vbmeta_offset: BigEndianU64,
    vbmeta_size: BigEndianU64,
    reserved: [u8; 28],
}

/// Appends the provided VBMeta contents to a copy of `image` as a VBMeta
/// footer, written to `destination`. While not spec'd as such, both the
/// appended VBMeta and footer are aligned to an 8 byte boundary to
/// ensure aligned reads of its fields and easy viewing under things like
/// `hexdump`.
pub(crate) fn append_vbmeta_as_footer(
    vbmeta: impl AsRef<[u8]>,
    image: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<()> {
    // Copy the image contents over to the destination.
    let _ = fs::copy(&image, &destination).with_context(|| {
        format!("failed to copy image at {:?} to {:?}", image.as_ref(), destination.as_ref())
    })?;
    let dest_metadata = fs::metadata(&destination)
        .with_context(|| format!("failed to read metadata of image at {:?}", image.as_ref()))?;
    let mut dest_perms = dest_metadata.permissions();
    dest_perms.set_mode(dest_perms.mode() | 0o200); // Ensure writable
    fs::set_permissions(&destination, dest_perms)?;

    // Now we append the copied contents.
    let mut f = OpenOptions::new()
        .append(true)
        .open(&destination)
        .with_context(|| format!("failed to open image at {:?}", destination.as_ref()))?;

    // Per the documentation, pad the VBMeta to an 8 byte boundary.
    let original_image_size = dest_metadata.len();
    let vbmeta_offset = pad_to_8_bytes(&mut f, original_image_size)?;
    f.write_all(vbmeta.as_ref()).context("failed to append VBMeta contents")?;

    // Again per the documentation, pad the footer to an 8 byte boundary.
    let vbmeta_size = vbmeta.as_ref().len() as u64;
    pad_to_8_bytes(&mut f, vbmeta_offset + vbmeta_size)?;
    let footer = Footer {
        magic: MAGIC,
        version_major: VERSION_MAJOR.into(),
        version_minor: VERSION_MINOR.into(),
        original_image_size: original_image_size.into(),
        vbmeta_offset: vbmeta_offset.into(),
        vbmeta_size: vbmeta_size.into(),
        reserved: [0u8; 28],
    };
    f.write_all(footer.as_bytes()).context("failed to append the footer")
}

// Given a file `f` assumed to have a current length of `curr_len`, this pads
// it out to an 8 byte boundary and returns the new size.
fn pad_to_8_bytes(f: &mut File, curr_len: u64) -> Result<u64> {
    let new_len = curr_len.next_multiple_of(8);
    if new_len > curr_len {
        f.set_len(new_len).context("failed to pad out the image")?;
        f.seek(SeekFrom::End(0)).context("failed to seek to the end of the image")?;
    }
    Ok(new_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn appending() {
        // These are not valid VBMeta contents, but that's not actually
        // strictly relevant to `append_vbmeta_as_footer()`.
        const VBMETA_CONTENTS: [u8; 7] = [0xaa, 0x00, 0xbb, 0x00, 0xcc, 0x00, 0xdd];
        const INITIAL_FILE_CONTENTS: [u8; 5] = [0x01, 0x23, 0x45, 0x67, 0x89];

        let mut image = NamedTempFile::new().unwrap();
        image.write_all(&INITIAL_FILE_CONTENTS).unwrap();
        image.as_file_mut().flush().unwrap();

        let outdir = TempDir::new().unwrap();
        let destination = outdir.path().join("appended");
        append_vbmeta_as_footer(&VBMETA_CONTENTS, image.path(), &destination).unwrap();

        #[rustfmt::skip]
        let expected_contents = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0x00, 0x00, 0x00, // INITIAL_FILE_CONTENTS, padded to 8 bytes
            0xaa, 0x00, 0xbb, 0x00, 0xcc, 0x00, 0xdd, 0x00, // VBMETA_CONTENTS, padded to 8 bytes
            0x41, 0x56, 0x42, 0x66, 0x00, 0x00, 0x00, 0x01, // MAGIC, VERSION_MAJOR
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // VERSION_MINOR, ...
            0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, // ...original_file_size = 5, ...
            0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, // ...vbmeta_offset = 8, ...
            0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, // ...vbmeta_size = 7
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // The rest reserved as zero...
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let actual_contents = fs::read(&destination).unwrap();
        assert_eq!(&expected_contents, actual_contents.as_slice());
    }
}
