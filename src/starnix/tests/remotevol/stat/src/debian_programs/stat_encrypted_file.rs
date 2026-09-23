// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use remotevol_linux_test_util::{MASTER_ENCRYPTION_KEY, add_encryption_key_with_key_bytes};
fn main() {
    let root_dir = std::fs::File::open("/data").expect("open failed");
    let dir_path = std::path::Path::new("/data").join("my_dir");
    // Find path for encrypted file and then read it
    let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
    let mut encrypted_target_path = None;
    for entry in entries {
        let entry = entry.expect("invalid entry");
        if entry.file_type().unwrap().is_file() {
            encrypted_target_path = Some(entry.path());
            let _ = std::fs::metadata(entry.path()).expect("metadata failed");
            break;
        }
    }
    assert!(encrypted_target_path.is_some());

    // Now unlock the file and see if metadata still succeeds on the encrypted path
    let (ret, _) = add_encryption_key_with_key_bytes(&root_dir, &MASTER_ENCRYPTION_KEY);
    assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
    let _ = std::fs::metadata(encrypted_target_path.unwrap())
        .expect_err("metadata succeeded on the encrypted file path");
}
