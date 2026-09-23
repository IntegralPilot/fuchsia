// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! implementation of the Fletcher64 checksum
//! https://en.wikipedia.org/wiki/Fletcher%27s_checksum
//! This approach was select because it is extremely fast but still sensitive to data changes.
//! The "typical" crypto algorithms are either more complex (slower to run),
//! or CRC based which is designed to be fast in hardware implementations, but is slower
//! than Fletcher when implemented in software.
//!
//! The implementation of Fletcher64 is only 10 lines, so it is easy to use vs. the overhead of
//! third_party dependencies.

use byteorder::{ByteOrder, LittleEndian};
use std::fs::File;
use std::io::{Error, Read};
use std::path::Path;
use std::time::Instant;

const BUF_SIZE: usize = 8192;

/// Fletcher64 implementation for checksums.
/// Implements the std::io::Writer trait to make
/// checksumming files easy.
pub struct FletcherHasher {
    last: u64,
}

impl FletcherHasher {
    pub fn new() -> Self {
        FletcherHasher { last: 0 }
    }

    pub fn finish(&self) -> u64 {
        self.last
    }

    /// Writes bytes to the the checksum. The
    /// buffer must has a length which is a multiple of 4.
    pub fn write(&mut self, buf: &[u8]) {
        assert!(buf.len() % 4 == 0);
        let mut lo = self.last as u32;
        let mut hi = (self.last >> 32) as u32;
        for chunk in buf.chunks(4) {
            lo = lo.wrapping_add(LittleEndian::read_u32(chunk));
            hi = hi.wrapping_add(lo);
        }
        self.last = (hi as u64) << 32 | lo as u64
    }
}

/// Returns the Fletcher64 checksum for the contents of
/// the specified file.
pub fn get_file_hash(file: &Path) -> Result<u64, Error> {
    let start = Instant::now();
    let mut hasher = FletcherHasher::new();
    let mut reader = File::open(file)?;
    let mut buf: [u8; BUF_SIZE] = [0; BUF_SIZE];

    loop {
        let len = reader.read(&mut buf)?;
        if len == 0 {
            break;
        }
        let extra = len % 4;
        // we need to write out blocks of data that are multiples
        // of 4, so check the length and pad with 0 if needed.
        if extra != 0 {
            hasher.write(&buf[0..len - extra]);
            let mut rest: [u8; 4] = [0; 4];
            rest[..extra].clone_from_slice(&buf[len - extra..len]);
            hasher.write(&rest);
        } else {
            hasher.write(&buf[0..len]);
        }
    }
    let new_hash = hasher.finish();
    let elapsed = start.elapsed().as_millis();
    log::debug!("File hash for {file:?} calculated in {elapsed:?}");
    Ok(new_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_fletcher() {
        let bytes: Vec<u8> = vec![];
        let mut hasher = FletcherHasher::new();
        hasher.write(&bytes);
        assert_eq!(0, hasher.finish());
    }
    #[test]
    fn test_hello_world_fletcher() {
        let bytes: &[u8] = "Hello World!".as_bytes();
        let mut hasher = FletcherHasher::new();
        hasher.write(&bytes);
        assert_eq!(0x4557dd28fd27f229, hasher.finish());
    }

    #[test]
    fn test_hello_world_from_file_fletcher() {
        let bytes: &[u8] = "Hello World!".as_bytes();
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let hellofile = temp_dir.path().join("hello.dat");
        std::fs::write(&hellofile, bytes).expect("create hello file");

        let val = get_file_hash(&hellofile).expect("hashing hello file");
        assert_eq!(0x4557dd28fd27f229, val);
    }

    #[test]
    fn test_hello_world_odd_bytes_from_file_fletcher() {
        let bytes: &[u8] = "Hello World!\n".as_bytes();
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let hellofile = temp_dir.path().join("hello.dat");
        std::fs::write(&hellofile, bytes).expect("create hello file");

        let val = get_file_hash(&hellofile).expect("hashing hello file");
        assert_eq!(0x427FCF5BFD27F233, val);
    }
}
