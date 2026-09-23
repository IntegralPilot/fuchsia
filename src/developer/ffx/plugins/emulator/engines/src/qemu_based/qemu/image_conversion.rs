// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Image conversion utilities for ffx emulator screenshots.

use fho::{Result, bug, user_error};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Converts a PPM P6 file to a PNG file.
///
/// Reads `ppm_source` to memory and converts its contents, then saves the PNG
/// file to `png_destination`.
///
/// `ppm_source` and `png_destination` may be the same.
pub fn convert_ppm_to_png(ppm_source: &Path, png_destination: &Path) -> Result<()> {
    let ppm_bytes = std::fs::read(ppm_source)
        .map_err(|e| user_error!("Failed to read PPM data from {:?}: {}", ppm_source, e))?;

    // Structural parsing: Separate header and payload into a validated PpmImage.
    let ppm_image = parse_ppm_p6(ppm_bytes)?;

    // Format conversion: Encode the validated pixel data as PNG.
    let png_file =
        File::create(png_destination).map_err(|e| bug!("Failed to create PNG file: {e}"))?;

    // We use BufWriter because the png crate writes data in chunks. Buffering these
    // writes reduces the number of system calls and improves performance for
    // potentially large image files.
    let mut encoder =
        png::Encoder::new(BufWriter::new(png_file), ppm_image.width, ppm_image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| bug!("Failed to write PNG header: {e}"))?;
    writer
        .write_image_data(&ppm_image.pixel_data)
        .map_err(|e| user_error!("Failed to encode PNG data: {e}"))?;

    Ok(())
}

/// A validated PPM (Portable Pixel Map) P6 image with its header properties and pixel data.
///
/// References:
/// * [the PPM Format Specifications][ppm-spec] - 07 November 2025.
///
/// [ppm-spec]: https://netpbm.sourceforge.net/doc/ppm.html
struct PpmImage {
    width: u32,
    height: u32,
    pixel_data: Vec<u8>,
}

/// Parses raw PPM bytes into a structured PpmImage.
fn parse_ppm_p6(bytes: Vec<u8>) -> Result<PpmImage> {
    // A "magic number" identifies the file type. For a PPM P6 file, it is the characters "P6".
    let (magic, position) =
        get_next_ppm_token(&bytes, 0).ok_or_else(|| user_error!("Empty or invalid PPM file"))?;
    if magic != "P6" {
        return Err(user_error!(
            "Unsupported PPM format from emulator: {}. Only P6 is supported.",
            magic
        ));
    }

    // Following the magic number are the image dimensions (width and height) and the
    // maximum color value (Maxval), each as ASCII characters in decimal, separated by
    // whitespace.
    let (width_str, position) = get_next_ppm_token(&bytes, position)
        .ok_or_else(|| user_error!("Missing width in PPM header"))?;
    let width: u32 = width_str.parse().map_err(|e| user_error!("Invalid width in PPM: {e}"))?;
    if width == 0 {
        return Err(user_error!("Invalid width in PPM: width cannot be zero."));
    }

    let (height_str, position) = get_next_ppm_token(&bytes, position)
        .ok_or_else(|| user_error!("Missing height in PPM header"))?;
    let height: u32 = height_str.parse().map_err(|e| user_error!("Invalid height in PPM: {e}"))?;
    if height == 0 {
        return Err(user_error!("Invalid height in PPM: height cannot be zero."));
    }

    let (max_val_str, mut position) = get_next_ppm_token(&bytes, position)
        .ok_or_else(|| user_error!("Missing max value in PPM header"))?;
    let max_val: u32 =
        max_val_str.parse().map_err(|e| user_error!("Invalid max value in PPM: {e}"))?;

    // QEMU's screendump typically uses 8-bit output (max_val = 255).
    if max_val != 255 {
        return Err(user_error!(
            "Unsupported PPM parameters: max_val={}. Only 8-bit PPM is supported.",
            max_val
        ));
    }

    // According to the spec, after the Maxval, there is exactly one whitespace character
    // (usually a newline) before the raster (pixel data).
    if position < bytes.len() && (bytes[position] as char).is_ascii_whitespace() {
        position += 1;
    } else {
        return Err(user_error!("Missing mandatory whitespace after Maxval in PPM header"));
    }

    let pixel_data = &bytes[position..];
    // Validation: Verify we have the correct amount of pixel data (RGB = 3 bytes per pixel)
    let expected_size = (width * height * 3) as usize;
    if pixel_data.len() != expected_size {
        return Err(user_error!(
            "Incomplete or malformed PPM data: expected {} ({}x{}x3) bytes, found {}",
            expected_size,
            width,
            height,
            pixel_data.len()
        ));
    }

    Ok(PpmImage { width, height, pixel_data: pixel_data.to_vec() })
}

/// Extracts the next token from a PPM header, skipping whitespaces and comments.
fn get_next_ppm_token(bytes: &[u8], mut position: usize) -> Option<(String, usize)> {
    loop {
        // Skip whitespace characters (blanks, TABs, CRs, LFs).
        while position < bytes.len() && (bytes[position] as char).is_ascii_whitespace() {
            position += 1;
        }
        // Skip comments as defined in the PBM spec.
        //
        // The PPM Format specification (Section "the Format") mentions that comments are
        // defined in the same way as PBM (Portable Bit Map).
        //
        // The PBM spec defines comments as follows: "Before the whitespace character that
        // delimits the raster, any characters from a "#" to but not including the next
        // carriage return or newline character, or end of file, is a comment and is ignored."
        //
        // References:
        // * [the PBM Format Specifications][pbm-spec] - 07 November 2025.
        //
        // [pbm-spec]: https://netpbm.sourceforge.net/doc/pbm.html
        if position < bytes.len() && bytes[position] == b'#' {
            while position < bytes.len() && bytes[position] != b'\n' && bytes[position] != b'\r' {
                position += 1;
            }
        } else {
            break;
        }
    }
    let start = position;
    // Collect non-whitespace characters until the next whitespace.
    while position < bytes.len() && !(bytes[position] as char).is_ascii_whitespace() {
        position += 1;
    }
    if start == position {
        None
    } else {
        Some((String::from_utf8_lossy(&bytes[start..position]).into_owned(), position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

    // Corresponds to the files in `engines/test_data`.
    const TEST_PPM: &[u8] = include_bytes!("../../../test_data/screenshot_input.ppm");
    const GOLDEN_PNG: &[u8] = include_bytes!("../../../test_data/screenshot_golden.png");

    #[fuchsia::test]
    fn test_get_next_ppm_token_basic() {
        let bytes = b"P6 1024 768 255";
        let (token, position) = get_next_ppm_token(bytes, 0).unwrap();
        assert_eq!(token, "P6");
        let (token, position) = get_next_ppm_token(bytes, position).unwrap();
        assert_eq!(token, "1024");
        let (token, position) = get_next_ppm_token(bytes, position).unwrap();
        assert_eq!(token, "768");
        let (token, _) = get_next_ppm_token(bytes, position).unwrap();
        assert_eq!(token, "255");
    }

    #[fuchsia::test]
    fn test_get_next_ppm_token_with_comments_and_mixed_whitespace() {
        let bytes_with_comments = b"P6 # Magic\n 1024\t# Width\r\n 768 # Height\n 255 # Maxval\n";
        let (token, position) = get_next_ppm_token(bytes_with_comments, 0).unwrap();
        assert_eq!(token, "P6");
        let (token, position) = get_next_ppm_token(bytes_with_comments, position).unwrap();
        assert_eq!(token, "1024");
        let (token, position) = get_next_ppm_token(bytes_with_comments, position).unwrap();
        assert_eq!(token, "768");
        let (token, _) = get_next_ppm_token(bytes_with_comments, position).unwrap();
        assert_eq!(token, "255");
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_basic() {
        // Valid 2x1 image
        let mut data = b"P6 2 1 255\n".to_vec();
        data.extend_from_slice(&[255, 0, 0, 0, 255, 0]); // Red pixel, Green pixel
        let image = parse_ppm_p6(data).expect("Should parse valid PPM");
        assert_eq!(image.width, 2);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixel_data, vec![255, 0, 0, 0, 255, 0]);
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_wrong_magic() {
        assert!(parse_ppm_p6(b"P3 1 1 255\n...".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_invalid_width() {
        assert!(parse_ppm_p6(b"P6 0 1 255\n".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_invalid_height() {
        assert!(parse_ppm_p6(b"P6 1 0 255\n".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_unsupported_maxval() {
        assert!(parse_ppm_p6(b"P6 1 1 100\n".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_missing_whitespace_after_maxval() {
        assert!(parse_ppm_p6(b"P6 1 1 255X...".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_incomplete_data() {
        assert!(parse_ppm_p6(b"P6 1 1 255\nRG".to_vec()).is_err());
    }

    #[fuchsia::test]
    fn test_parse_ppm_p6_real_data() {
        let image = parse_ppm_p6(TEST_PPM.to_vec()).expect("should parse real QEMU ppm");
        assert_eq!(image.width, 1280);
        assert_eq!(image.height, 800);
        assert_eq!(image.pixel_data.len(), 1280 * 800 * 3);
    }

    #[fuchsia::test]
    fn test_convert_ppm_to_png_verification() {
        let temp = tempdir().expect("tempdir");
        let ppm_input = temp.path().join("input.ppm");
        let png_output = temp.path().join("output.png");

        std::fs::write(&ppm_input, TEST_PPM).expect("failed to write test ppm");

        convert_ppm_to_png(&ppm_input, &png_output).expect("conversion failed");

        assert!(png_output.exists());
        let metadata = std::fs::metadata(&png_output).unwrap();
        assert!(metadata.len() > 0);

        // Basic PNG signature check.
        // PNG Specification 1.2: Section 3.1. PNG file signature.
        // https://www.libpng.org/pub/png/spec/1.2/PNG-Structure.html#PNG-file-signature
        let mut file = File::open(png_output).unwrap();
        let mut signature = [0u8; 8];
        file.read_exact(&mut signature).unwrap();
        assert_eq!(signature, [137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[fuchsia::test]
    fn test_convert_ppm_to_png_same_path() {
        let temp = tempdir().expect("tempdir");
        let same_path = temp.path().join("image_to_convert.png");

        // Prepare the source file at the same path
        std::fs::write(&same_path, TEST_PPM).expect("failed to write test ppm");

        // Convert where source and destination are the same
        convert_ppm_to_png(&same_path, &same_path).expect("conversion failed at same path");

        assert!(same_path.exists());

        // Basic PNG signature check.
        // PNG Specification 1.2: Section 3.1. PNG file signature.
        // https://www.libpng.org/pub/png/spec/1.2/PNG-Structure.html#PNG-file-signature
        let mut file = File::open(same_path).unwrap();
        let mut signature = [0u8; 8];
        file.read_exact(&mut signature).unwrap();
        assert_eq!(signature, [137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[fuchsia::test]
    fn test_convert_ppm_to_png_against_golden() {
        let temp = tempdir().expect("tempdir");
        let ppm_input = temp.path().join("input.ppm");
        let png_output = temp.path().join("output.png");

        std::fs::write(&ppm_input, TEST_PPM).expect("failed to write test ppm");

        // Perform the conversion
        convert_ppm_to_png(&ppm_input, &png_output).expect("conversion failed");

        // Read the generated PNG file
        let generated_bytes = std::fs::read(png_output).expect("failed to read generated png");

        // Decode the generated PNG to get width, height, and pixels
        let generated_decoder = png::Decoder::new(std::io::Cursor::new(generated_bytes));
        let mut generated_reader =
            generated_decoder.read_info().expect("failed to read generated PNG info");
        let mut generated_buf = vec![0; generated_reader.output_buffer_size().unwrap()];
        let generated_info = generated_reader
            .next_frame(&mut generated_buf)
            .expect("failed to decode generated PNG frame");
        let generated_pixels = &generated_buf[..generated_info.buffer_size()];

        // Decode the golden PNG to get width, height, and pixels
        let golden_decoder = png::Decoder::new(std::io::Cursor::new(GOLDEN_PNG));
        let mut golden_reader = golden_decoder.read_info().expect("failed to read golden PNG info");
        let mut golden_buf = vec![0; golden_reader.output_buffer_size().unwrap()];
        let golden_info =
            golden_reader.next_frame(&mut golden_buf).expect("failed to decode golden PNG frame");
        let golden_pixels = &golden_buf[..golden_info.buffer_size()];

        // Compare the decoded properties and pixel data.
        assert_eq!(
            generated_info.width, golden_info.width,
            "The width of the generated PNG file does not match the golden image from test_data."
        );
        assert_eq!(
            generated_info.height, golden_info.height,
            "The height of the generated PNG file does not match the golden image from test_data."
        );
        assert_eq!(
            generated_info.color_type, golden_info.color_type,
            "The color type of the generated PNG file does not match the golden image from test_data."
        );
        assert_eq!(
            generated_info.bit_depth, golden_info.bit_depth,
            "The bit depth of the generated PNG file does not match the golden image from test_data."
        );
        assert_eq!(
            generated_pixels, golden_pixels,
            "The decoded pixels of the generated PNG file do not match the golden image from test_data."
        );
    }
}
