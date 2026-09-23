// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use remotevol_linux_test_util::{
    MASTER_ENCRYPTION_KEY, add_encryption_key_with_key_bytes, fscrypt_add_key_arg,
    set_encryption_policy,
};
use zerocopy::FromBytes;

fn main() {
    let root_dir = std::fs::File::open("/data").expect("open failed");
    let dir_path = std::path::Path::new("/data").join("my_dir");
    std::fs::create_dir_all(dir_path.clone()).unwrap();
    let dir = std::fs::File::open(dir_path.clone()).unwrap();
    let (ret, arg_vec) = add_encryption_key_with_key_bytes(&root_dir, &MASTER_ENCRYPTION_KEY);
    assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
    let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
    let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
    let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
    assert!(ret == 0, "set encryption policy ioctl failed: {:?}", std::io::Error::last_os_error());
    {
        let _ = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&dir_path.clone().join("file.txt"))
            .unwrap();
    }
}
