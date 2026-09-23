// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fscrypt_shared::*;
use linux_uapi::{
    FS_IOC_ADD_ENCRYPTION_KEY, FS_IOC_REMOVE_ENCRYPTION_KEY, FS_IOC_SET_ENCRYPTION_POLICY,
    FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_POLICY_FLAGS_PAD_16, fscrypt_policy_v2,
};
use std::os::fd::AsRawFd;
use zerocopy::{FromBytes, IntoBytes};

const FSCRYPT_MODE_AES_256_XTS: u8 = 1;
const FSCRYPT_MODE_AES_256_CTS: u8 = 4;

#[derive(FromArgs)]
/// Top-level args for this binary.
struct Args {
    /// uid to set this child process to
    #[argh(option)]
    uid: u32,
    /// the specific ioctl or io command this binary should call
    #[argh(subcommand)]
    command: FscryptSubcommand,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Determines which fscrypt ioctl or io command this binary should call.
#[argh(subcommand)]
enum FscryptSubcommand {
    Add(AddSubcommand),
    Set(SetSubcommand),
    Remove(RemoveSubcommand),
    Read(ReadSubcommand),
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "add")]
/// Contains the arguments for the Add subcommand.
struct AddSubcommand {
    #[argh(option)]
    /// the wrapping key the user wants to add
    key: Option<String>,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Contains the arguments for the Set subcommand.
#[argh(subcommand, name = "set")]
struct SetSubcommand {
    #[argh(option)]
    /// identifier of the wrapping key used to encrypt this directory
    identifier: String,
    /// whether or not the SET_ENCRYPTION_POLICY ioctl should fail
    #[argh(option)]
    should_fail: bool,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Contains the arguments for the Remove subcommand.
#[argh(subcommand, name = "remove")]
struct RemoveSubcommand {
    #[argh(option)]
    /// identifier of the wrapping key the user wants to remove
    identifier: String,
    /// whether or not the REMOVE_ENCRYPTION_KEY ioctl should fail
    #[argh(option)]
    should_fail: bool,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "read")]
/// Contains the arguments for the Read subcommand.
struct ReadSubcommand {
    #[argh(option)]
    /// whether or not the encrypted directory is locked
    locked: bool,
}

fn main() {
    let Args { uid, command } = argh::from_env();
    let ret = unsafe { libc::setuid(uid) };
    assert!(ret == 0, "set uid failed: {:?}", std::io::Error::last_os_error());
    match command {
        FscryptSubcommand::Add(AddSubcommand { key }) => {
            let data_dir = std::fs::File::open(&std::env::var("MUTABLE_STORAGE").unwrap())
                .expect("open failed");
            let key_spec = fscrypt_key_specifier {
                type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER,
                ..Default::default()
            };

            let wrapping_key = if let Some(encoded_key) = key {
                let wrapping_key: [u8; 64] =
                    hex::decode(encoded_key).expect("hex decode failed").try_into().unwrap();
                wrapping_key
            } else {
                let mut random_vector: [u8; 64] = [0; 64];
                for i in 0..random_vector.len() {
                    let rand_u8: u8 = rand::random();
                    random_vector[i] = rand_u8;
                }
                random_vector
            };

            let arg =
                fscrypt_add_key_arg { key_spec: key_spec, raw_size: 64, ..Default::default() };
            let mut arg_vec = arg.as_bytes().to_vec();
            arg_vec.extend(wrapping_key);

            let ret = unsafe {
                libc::ioctl(
                    data_dir.as_raw_fd(),
                    FS_IOC_ADD_ENCRYPTION_KEY.try_into().unwrap(),
                    arg_vec.as_ptr(),
                )
            };
            assert!(
                ret == 0,
                "add encryption key ioctl failed: {:?}",
                std::io::Error::last_os_error()
            );
            let (arg_struct_bytes, _) =
                arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
            let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
            let identifier = unsafe { arg_struct.key_spec.u.identifier.value };
            let output = FscryptOutput { identifier };
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        FscryptSubcommand::Set(SetSubcommand { identifier, should_fail }) => {
            let dir_path =
                std::path::Path::new(&std::env::var("MUTABLE_STORAGE").unwrap()).join("my_dir");
            let dir = std::fs::File::open(dir_path).expect("open failed");
            let identifier: [u8; 16] =
                hex::decode(identifier).expect("hex decode failed").try_into().unwrap();
            let ret = unsafe {
                let policy = fscrypt_policy_v2 {
                    version: 2,
                    contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS,
                    filenames_encryption_mode: FSCRYPT_MODE_AES_256_CTS,
                    flags: FSCRYPT_POLICY_FLAGS_PAD_16 as u8,
                    master_key_identifier: identifier,
                    ..Default::default()
                };
                libc::ioctl(
                    dir.as_raw_fd(),
                    FS_IOC_SET_ENCRYPTION_POLICY.try_into().unwrap(),
                    &policy,
                )
            };
            if should_fail {
                assert!(
                    ret != 0,
                    "set encryption policy ioctl should have failed: {:?}",
                    std::io::Error::last_os_error()
                );
            } else {
                assert!(
                    ret == 0,
                    "set encryption policy ioctl failed: {:?}",
                    std::io::Error::last_os_error()
                );
            }
        }
        FscryptSubcommand::Remove(RemoveSubcommand { identifier, should_fail }) => {
            let data_dir = std::fs::File::open(&std::env::var("MUTABLE_STORAGE").unwrap())
                .expect("open failed");
            let identifier: [u8; 16] =
                hex::decode(identifier).expect("hex decode failed").try_into().unwrap();
            let mut key_spec = fscrypt_key_specifier {
                type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER,
                ..Default::default()
            };
            key_spec.u.identifier.value = identifier;
            let remove_arg = fscrypt_remove_key_arg { key_spec: key_spec, ..Default::default() };

            let ret = unsafe {
                libc::ioctl(
                    data_dir.as_raw_fd(),
                    FS_IOC_REMOVE_ENCRYPTION_KEY.try_into().unwrap(),
                    &remove_arg,
                )
            };
            if should_fail {
                assert!(
                    ret != 0,
                    "remove encryption key ioctl should have failed: {:?}",
                    std::io::Error::last_os_error()
                );
            } else {
                assert!(
                    ret == 0,
                    "remove encryption key ioctl failed: {:?}",
                    std::io::Error::last_os_error()
                );
            }
        }
        FscryptSubcommand::Read(ReadSubcommand { locked }) => {
            let dir_path =
                std::path::Path::new(&std::env::var("MUTABLE_STORAGE").unwrap()).join("my_dir");
            let entries = std::fs::read_dir(dir_path).expect("readdir failed");
            let mut count = 0;
            for entry in entries {
                let entry = entry.expect("invalid entry");
                if locked {
                    assert!(entry.file_name() != "subdir");
                } else {
                    assert!(entry.file_name() == "subdir");
                }
                count += 1;
            }
            assert_eq!(count, 1);
        }
    }
}
