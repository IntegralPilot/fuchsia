// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This library contains the shared functions used by multiple emulation engines. Any code placed
//! in this library may not depend on any other code within the plugin, with the exception of "args"
//! libraries.

use anyhow::{Result, anyhow};
use nix::sys::socket::{AddressFamily, SockFlag, SockType, VsockAddr, connect, socket};
use rand::{RngExt as _, distr};
use std::fs::File;
use std::io::{BufRead, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

// Provides access to ffx_config properties.
pub mod config;
pub mod process;
pub mod tuntap;

const VSOCK_PORT: u32 = 22;
const VSOCK_CID_BOUNDS: (u32, u32) = (3, u32::MAX); // Inclusive

/// A utility function for checking whether the host OS is MacOS.
pub fn host_is_mac() -> bool {
    std::env::consts::OS == "macos"
}

/// A utility function for splitting a string at a single point and converting it into a tuple.
/// Returns an Err(anyhow) if it can't do the split.
pub fn split_once(text: &str, pattern: &str) -> Result<(String, String)> {
    let splitter: Vec<&str> = text.splitn(2, pattern).collect();
    if splitter.len() != 2 {
        return Err(anyhow!("Invalid split of '{}' on '{}'.", text, pattern));
    }
    let first = splitter[0];
    let second = splitter[1];
    Ok((first.to_string(), second.to_string()))
}

/// A utility function to dump the contents of a file to the terminal.
pub fn dump_log_to_out<W: Write>(log: &PathBuf, out: &mut W) -> Result<()> {
    let mut out_handle = std::io::BufWriter::new(out);
    let mut buf = Vec::with_capacity(64);
    let mut buf_reader = std::io::BufReader::new(File::open(log)?);
    while let Ok(len) = buf_reader.read_until(b'\n', &mut buf) {
        if len == 0 {
            break;
        }
        out_handle.write_all(&buf)?;
        buf.clear();
        out_handle.flush()?;
    }
    Ok(())
}

fn connect_with_cid_port(cid: u32, port: u32) -> std::io::Result<()> {
    let addr = VsockAddr::new(cid, port);
    #[cfg(not(target_os = "macos"))]
    let flags = SockFlag::SOCK_CLOEXEC;
    #[cfg(target_os = "macos")]
    let flags = SockFlag::empty();
    let sock = socket(AddressFamily::Vsock, SockType::Stream, flags, None)?;
    connect(sock.as_raw_fd(), &addr)?;
    Ok(())
}

pub fn find_unused_vsock_cid() -> Result<u32> {
    // With the random generation of CIDs, collision is already highly
    // improbable. Subsequent collisions would be even more so, but better to
    // be defensive with a bit of retry logic.
    const ATTEMPTS: u32 = 3;

    let dist = distr::Uniform::new_inclusive(VSOCK_CID_BOUNDS.0, VSOCK_CID_BOUNDS.1).unwrap();
    let mut cids = rand::rng().sample_iter(dist);

    for _ in 0..ATTEMPTS {
        let cid = cids.next().unwrap();
        match connect_with_cid_port(cid, VSOCK_PORT) {
            // A successful connection indicates the CID is valid and there was even something
            // listening to it inside the VM.
            Ok(_) => continue,
            Err(err) => {
                if err.kind() == ErrorKind::ConnectionReset {
                    // ConnectionReset means the CID was valid, but nothing was listening to the
                    // port in the VM.
                    continue;
                }
                if let Some(os_err) = err.raw_os_error() {
                    match os_err {
                        // ENODEV indicates that the CID is unused.
                        nix::libc::ENODEV => return Ok(cid),
                        nix::libc::EAFNOSUPPORT => {
                            return Err(anyhow!("host does not support vsock"));
                        }
                        _ => {}
                    }
                }
                eprintln!("Unexpected error: {err:?}")
            }
        };
    }
    Err(anyhow!("No free cids found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_once() {
        // Can't split an empty string.
        assert!(split_once("", " ").is_err());

        // Can't split on a character that isn't in the text.
        assert!(split_once("something", " ").is_err());

        // Splitting on an empty pattern returns ("", text).
        // This is strange, but expected and documented.
        let result = split_once("something", "");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("".to_string(), "something".to_string()));

        // This also results in a successful ("", "") when they're both empty.
        let result = split_once("", "");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("".to_string(), "".to_string()));

        // A simple split on a space.
        let result = split_once("A B", " ");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("A".to_string(), "B".to_string()));

        // A simple split on a colon.
        let result = split_once("A:B", ":");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("A".to_string(), "B".to_string()));

        // Splitting when there are multiple separators returns (first, remainder).
        let result = split_once("A:B:C:D:E", ":");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("A".to_string(), "B:C:D:E".to_string()));

        // Splitting on a more complex pattern.
        let result = split_once("A pattern can be just about anything", "pattern");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("A ".to_string(), " can be just about anything".to_string()));

        // When the pattern is the first thing in the text.
        let result = split_once("A pattern", "pattern");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("A ".to_string(), "".to_string()));

        // When the pattern is the last thing in the text.
        let result = split_once("pattern B", "pattern");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("".to_string(), " B".to_string()));

        // When the pattern is the only thing in the text.
        let result = split_once("pattern", "pattern");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("".to_string(), "".to_string()));
    }
}
