// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::stage_file;
use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;

type Result<T> = std::result::Result<T, FfxFastbootError>;

use byteorder::{ByteOrder, LittleEndian};
use ffx_fastboot_interface::fastboot_interface::{FastbootInterface, UploadProgress};
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};
use tokio::sync::mpsc::Sender;

const PAGE_SIZE: u32 = 4096;
const BOOT_MAGIC: &str = "ANDROID!";
const BOOT_SIZE: usize = 8;
const V4_HEADER_SIZE: u32 = 1580;

fn copy<R: Read, W: Write>(mut reader: BufReader<R>, writer: &mut BufWriter<W>) -> Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        let length = buffer.len();
        if length == 0 {
            return Ok(());
        }
        writer.write_all(buffer)?;
        reader.consume(length);
    }
}

async fn get_boot_image<F: FileResolver + Sync>(
    file_resolver: &mut F,
    zbi: &String,
    vbmeta: &Option<String>,
    temp_dir: &TempDir,
) -> Result<(PathBuf, bool)> {
    let zbi_path = PathBuf::from(file_resolver.get_file(zbi).await?);

    let has_android_header = {
        let mut file = File::open(&zbi_path)?;
        let mut magic = [0u8; BOOT_SIZE];
        match file.read_exact(&mut magic) {
            Ok(()) => &magic == BOOT_MAGIC.as_bytes(),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => false,
            Err(e) => return Err(FfxFastbootError::from(e)),
        }
    };

    if has_android_header {
        return Ok((zbi_path, true));
    }

    match vbmeta {
        None => Ok((zbi_path, false)),
        Some(v) => {
            // if vbmeta exists, concat the two into a single boot image file
            let v_path = file_resolver.get_file(v).await?;
            let mut path = PathBuf::new();
            path.push(temp_dir.path());
            path.push("boot_image.bin");
            let mut outfile = BufWriter::new(File::create(&path)?);
            let zbi_file = BufReader::new(File::open(&zbi_path)?);
            let vbmeta_file = BufReader::new(File::open(&v_path)?);
            copy(zbi_file, &mut outfile)?;
            outfile.flush()?;
            copy(vbmeta_file, &mut outfile)?;
            outfile.flush()?;
            Ok((path, false))
        }
    }
}

pub async fn boot<F: FileResolver + Sync, T: FastbootInterface>(
    messenger: Sender<UploadProgress>,
    file_resolver: &mut F,
    zbi: String,
    vbmeta: Option<String>,
    fastboot_interface: &mut T,
) -> Result<()> {
    let temp_dir = tempdir()?;
    let (boot_image, has_android_header) =
        get_boot_image(file_resolver, &zbi, &vbmeta, &temp_dir).await?;

    let path = if has_android_header {
        boot_image
    } else {
        let page_mask: u32 = PAGE_SIZE - 1;
        let kernal_size: u32 = metadata(&boot_image)?.len().try_into()?;
        let kernal_actual: u32 = (kernal_size + page_mask) & (!page_mask);

        let mut path = PathBuf::new();
        path.push(temp_dir.path());
        path.push("bootimg.bin");

        let mut outfile = BufWriter::new(File::create(&path)?);

        let mut header: [u8; 4096] = [0u8; 4096];
        header[0..BOOT_SIZE].copy_from_slice(&BOOT_MAGIC.as_bytes()[..]);
        LittleEndian::write_u32(&mut header[BOOT_SIZE..BOOT_SIZE + 4], kernal_size);
        LittleEndian::write_u32(&mut header[BOOT_SIZE + 12..BOOT_SIZE + 16], V4_HEADER_SIZE);
        LittleEndian::write_u32(
            &mut header[BOOT_SIZE + 32..BOOT_SIZE + 36],
            4, /* header version*/
        );
        outfile.write_all(&header)?;

        let in_file = BufReader::new(File::open(&boot_image)?);
        copy(in_file, &mut outfile)?;

        // Pad to page size.
        let padding = kernal_actual - kernal_size;
        let padding_bytes: [u8; 4096] = [0u8; 4096];
        outfile.write_all(&padding_bytes[..padding.try_into()?])?;
        outfile.flush()?;

        path
    };

    stage_file(
        messenger,
        file_resolver,
        false, /* resolve */
        path.to_str().ok_or(FfxFastbootError::NonUtf8Path)?,
        fastboot_interface,
    )
    .await?;

    fastboot_interface.boot().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_resolver::test::TestResolver;
    use ffx_fastboot_interface::test::setup;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    #[fuchsia::test]
    async fn test_get_boot_image_ignores_vbmeta_if_android_header_present() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"ANDROID!").unwrap();
        file.write_all(&[0u8; 100]).unwrap();
        file.flush().unwrap();
        let file_path = file.path().to_str().unwrap().to_string();

        let mut vbmeta = NamedTempFile::new().unwrap();
        vbmeta.write_all(b"vbmeta_content").unwrap();
        vbmeta.flush().unwrap();
        let vbmeta_path = vbmeta.path().to_str().unwrap().to_string();

        let temp_dir = tempdir().unwrap();
        let mut resolver = TestResolver::new();
        let (boot_img, has_android) =
            get_boot_image(&mut resolver, &file_path, &Some(vbmeta_path), &temp_dir).await.unwrap();

        assert!(has_android);
        assert_eq!(boot_img, PathBuf::from(&file_path));
    }

    #[fuchsia::test]
    async fn test_get_boot_image_appends_vbmeta_if_no_android_header() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"raw_zbi").unwrap();
        file.flush().unwrap();
        let file_path = file.path().to_str().unwrap().to_string();

        let mut vbmeta = NamedTempFile::new().unwrap();
        vbmeta.write_all(b"vbmeta_content").unwrap();
        vbmeta.flush().unwrap();
        let vbmeta_path = vbmeta.path().to_str().unwrap().to_string();

        let temp_dir = tempdir().unwrap();
        let mut resolver = TestResolver::new();
        let (boot_img, has_android) =
            get_boot_image(&mut resolver, &file_path, &Some(vbmeta_path), &temp_dir).await.unwrap();

        assert!(!has_android);
        assert_ne!(boot_img, PathBuf::from(&file_path));
        assert!(boot_img.ends_with("boot_image.bin"));
    }

    #[fuchsia::test]
    async fn test_boot_appends_header_for_raw_image() {
        let (state, mut proxy) = setup();
        let mut raw_file = NamedTempFile::new().unwrap();
        raw_file.write_all(b"raw-zbi-content").unwrap();
        raw_file.flush().unwrap();
        let raw_path = raw_file.path().to_str().unwrap().to_string();

        let (messenger, _receiver) = mpsc::channel(1);
        let mut resolver = TestResolver::new();
        boot(messenger, &mut resolver, raw_path.clone(), None, &mut proxy).await.unwrap();

        let state = state.lock().unwrap();
        assert_eq!(1, state.boots);
        assert_eq!(1, state.staged_files.len());
        assert_ne!(&state.staged_files[0], &raw_path);
        assert!(state.staged_files[0].ends_with("bootimg.bin"));
    }

    #[fuchsia::test]
    async fn test_boot_does_not_append_header_if_already_present() {
        let (state, mut proxy) = setup();
        let mut android_file = NamedTempFile::new().unwrap();
        android_file.write_all(b"ANDROID!already_formatted_bootimg").unwrap();
        android_file.flush().unwrap();
        let android_path = android_file.path().to_str().unwrap().to_string();

        let (messenger, _receiver) = mpsc::channel(1);
        let mut resolver = TestResolver::new();
        boot(messenger, &mut resolver, android_path.clone(), None, &mut proxy).await.unwrap();

        let state = state.lock().unwrap();
        assert_eq!(1, state.boots);
        assert_eq!(1, state.staged_files.len());
        assert_eq!(&state.staged_files[0], &android_path);
    }
}
