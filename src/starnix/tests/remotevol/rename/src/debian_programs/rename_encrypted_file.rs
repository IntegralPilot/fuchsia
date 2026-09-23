// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

fn main() {
    let dir_path = std::path::Path::new("/data").join("rename_dir");
    let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
    let mut encrypted_file_path = None;
    for entry in entries {
        let entry = entry.expect("invalid entry");
        if entry.file_type().unwrap().is_file() {
            encrypted_file_path = Some(entry.path());
            break;
        }
    }
    let encrypted_file_path = encrypted_file_path.expect("encrypted file not found in readdir");
    let err = std::fs::rename(&encrypted_file_path, dir_path.join("renamed.txt"))
        .expect_err("rename on locked encrypted directory should fail with ENOKEY");
    assert_eq!(err.raw_os_error(), Some(libc::ENOKEY));
}
