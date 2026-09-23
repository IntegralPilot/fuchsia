// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use remotevol_linux_test_util::{
    LINK_FILE_NAME, MASTER_ENCRYPTION_KEY, TARGET_FILE_NAME, add_encryption_key_with_key_bytes,
};
fn main() {
    let root_dir = std::fs::File::open("/data").expect("open failed");
    let dir_path = std::path::Path::new("/data").join("my_dir");
    // Find path for encrypted symlink and then read it
    let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
    let mut encrypted_target_path = None;
    for entry in entries {
        let entry = entry.expect("invalid entry");
        if entry.file_type().unwrap().is_symlink() {
            encrypted_target_path =
                Some(std::fs::read_link(entry.path()).expect("read_link failed"));
            break;
        }
    }
    assert!(encrypted_target_path.is_some());
    assert!(encrypted_target_path.unwrap() != dir_path.join(TARGET_FILE_NAME));

    // Now unlock the symlink and check that the target path has changed
    let (ret, _) = add_encryption_key_with_key_bytes(&root_dir, &MASTER_ENCRYPTION_KEY);
    assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
    let plaintext_target_path =
        std::fs::read_link(dir_path.join(LINK_FILE_NAME)).expect("read_link failed");
    assert_eq!(plaintext_target_path, dir_path.join(TARGET_FILE_NAME));
}
