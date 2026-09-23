// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod fscrypt_test {
    use fscrypt_shared::{
        FscryptOutput, fscrypt_add_key_arg, fscrypt_key_specifier, fscrypt_remove_key_arg,
    };
    use linux_uapi::{
        FS_IOC_ADD_ENCRYPTION_KEY, FS_IOC_REMOVE_ENCRYPTION_KEY, FS_IOC_SET_ENCRYPTION_POLICY,
        FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_MODE_AES_256_CTS, FSCRYPT_MODE_AES_256_HCTR2,
        FSCRYPT_MODE_AES_256_XTS, FSCRYPT_POLICY_FLAGS_PAD_16, fscrypt_policy_v2,
    };
    use serial_test::serial;
    use std::env::VarError;
    use std::ffi::OsString;
    use std::os::fd::AsRawFd;
    use zerocopy::{FromBytes, IntoBytes};

    fn add_encryption_key(root_dir: &std::fs::File) -> (i32, Vec<u8>) {
        let key_spec =
            fscrypt_key_specifier { type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, ..Default::default() };
        let arg = fscrypt_add_key_arg { key_spec: key_spec, raw_size: 64, ..Default::default() };
        let mut arg_vec = arg.as_bytes().to_vec();
        let mut random_vector: [u8; 64] = [0; 64];
        for i in 0..64 {
            let rand_u8: u8 = rand::random();
            random_vector[i] = rand_u8;
        }
        arg_vec.extend(random_vector);

        let ret = unsafe {
            libc::ioctl(
                root_dir.as_raw_fd(),
                FS_IOC_ADD_ENCRYPTION_KEY.try_into().unwrap(),
                arg_vec.as_mut_ptr(),
            )
        };
        (ret, arg_vec)
    }

    fn add_encryption_key_with_key_bytes(
        root_dir: &std::fs::File,
        raw_key: &[u8],
    ) -> (i32, Vec<u8>) {
        let key_spec =
            fscrypt_key_specifier { type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, ..Default::default() };
        let arg = fscrypt_add_key_arg { key_spec: key_spec, raw_size: 64, ..Default::default() };
        let mut arg_vec = arg.as_bytes().to_vec();
        arg_vec.extend(raw_key);

        let ret = unsafe {
            libc::ioctl(
                root_dir.as_raw_fd(),
                FS_IOC_ADD_ENCRYPTION_KEY.try_into().unwrap(),
                arg_vec.as_mut_ptr(),
            )
        };
        (ret, arg_vec)
    }

    fn set_encryption_policy(dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
        let ret = unsafe {
            let policy = fscrypt_policy_v2 {
                version: 2,
                contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS as u8,
                filenames_encryption_mode: FSCRYPT_MODE_AES_256_CTS as u8,
                flags: FSCRYPT_POLICY_FLAGS_PAD_16 as u8,
                master_key_identifier: identifier,
                ..Default::default()
            };
            libc::ioctl(dir.as_raw_fd(), FS_IOC_SET_ENCRYPTION_POLICY.try_into().unwrap(), &policy)
        };
        ret
    }

    fn set_encryption_policy_hctr2(dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
        let ret = unsafe {
            let policy = fscrypt_policy_v2 {
                version: 2,
                contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS as u8,
                filenames_encryption_mode: FSCRYPT_MODE_AES_256_HCTR2 as u8,
                flags: FSCRYPT_POLICY_FLAGS_PAD_16 as u8,
                master_key_identifier: identifier,
                ..Default::default()
            };
            libc::ioctl(dir.as_raw_fd(), FS_IOC_SET_ENCRYPTION_POLICY.try_into().unwrap(), &policy)
        };
        ret
    }

    fn remove_encryption_key(root_dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
        let ret = unsafe {
            let mut key_spec = fscrypt_key_specifier {
                type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER,
                ..Default::default()
            };
            key_spec.u.identifier.value = identifier;
            let remove_arg = fscrypt_remove_key_arg { key_spec: key_spec, ..Default::default() };

            libc::ioctl(
                root_dir.as_raw_fd(),
                FS_IOC_REMOVE_ENCRYPTION_KEY.try_into().unwrap(),
                &remove_arg,
            )
        };
        ret
    }

    fn get_root_path() -> Option<String> {
        // TODO(https://fxbug.dev/317285180): Once host syscall tests can be run in CQ with root
        // access, define MUTABLE_STORAGE for host tests.
        match std::env::var("MUTABLE_STORAGE") {
            Ok(root_path) => Some(root_path),
            Err(e) if e == VarError::NotPresent => None,
            Err(e) => panic!("fetching env var MUTABLE_STORAGE failed with {:?}", e),
        }
    }

    #[test]
    #[serial]
    fn remove_key_that_was_never_added() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::random();
            random_vector[i] = rand_u8;
        }
        let ret = remove_encryption_key(&root_dir, random_vector);
        assert!(
            ret != 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
    }

    fn get_child_binary_path() -> std::path::PathBuf {
        std::path::PathBuf::from("data/fscrypt_test")
    }

    #[test]
    #[serial]
    fn remove_key_added_by_different_user_non_root() {
        let Some(_) = get_root_path() else { return };
        let child_binary_path = get_child_binary_path();

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("add encryption key failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "true",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        // Cleanup
        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
    }

    #[test]
    #[serial]
    fn remove_key_added_by_different_user_root() {
        let Some(_) = get_root_path() else { return };
        let child_binary_path = get_child_binary_path();

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("add encryption key failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);
        let root_dir =
            std::fs::File::open(&std::env::var("MUTABLE_STORAGE").unwrap()).expect("open failed");
        let ret = remove_encryption_key(&root_dir, fscrypt_output.identifier);

        assert!(
            ret != 0,
            "remove encryption key ioctl should have failed: {:?}",
            std::io::Error::last_os_error()
        );

        // Cleanup
        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn user_reads_directory_unlocked_by_different_user() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let child_binary_path = get_child_binary_path();

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "false",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::create_dir(dir_path.join("subdir")).unwrap();

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "read", "--locked", "false"])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "read", "--locked", "true"])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn readdir_encrypted_directory_name() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::create_dir_all(dir_path.clone().join("subdir/subsubdir"))
            .expect("failed to create subdir");

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        let mut encrypted_dir_name = OsString::new();
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            encrypted_dir_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::read_dir(dir_path.clone().join("subdir")).expect_err(
            "should not be able to readdir locked encrypted directory
                    with its plaintext filename",
        );

        let entries = std::fs::read_dir(dir_path.join(encrypted_dir_name))
            .expect("readdir of encrypted subdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subsubdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn read_file_contents_from_handle_created_before_remove_encryption_key() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::write(dir_path.clone().join("file.txt"), "file_contents")
            .expect("create or write failed on file.txt");
        let file = std::fs::File::open(dir_path.clone().join("file.txt")).unwrap();
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };

        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let buf = std::fs::read(dir_path.join("file.txt")).expect("read file failed");
        assert_eq!(buf.as_bytes(), "file_contents".as_bytes());

        drop(file);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::read(dir_path.join("file.txt"))
            .expect_err("should not be able to read locked file");
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn readdir_locked_directory() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::create_dir(dir_path.clone().join("subdir")).expect("failed to create subdir");
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() == "subdir");
            count += 1;
        }
        assert_eq!(count, 1);

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn set_encryption_policy_with_fake_identifier_non_root() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::random();
            random_vector[i] = rand_u8;
        }

        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(random_vector),
            ])
            .output()
            .expect("set encryption policy failed");

        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn set_encryption_policy_with_fake_identifier_root() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let mut random_vector: [u8; 16] = [0; 16];
        for i in 0..16 {
            let rand_u8: u8 = rand::random();
            random_vector[i] = rand_u8;
        }
        let ret = set_encryption_policy(&dir, random_vector);
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn hard_link_encrypted_file_into_unencrypted_dir() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let unencrypted_dir_path = std::path::Path::new(&root_path).join("unencrypted");
        std::fs::create_dir_all(unencrypted_dir_path.clone()).unwrap();

        let encrypted_dir_path = std::path::Path::new(&root_path).join("encrypted");
        std::fs::create_dir_all(encrypted_dir_path.clone()).unwrap();

        let dir = std::fs::File::open(encrypted_dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::File::create_new(encrypted_dir_path.clone().join("file"))
            .expect("failed to create file in encrypted directory");

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(encrypted_dir_path.clone()).expect("readdir failed");
        let mut encrypted_file_name = OsString::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            encrypted_file_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::hard_link(
            encrypted_dir_path.join(encrypted_file_name),
            unencrypted_dir_path.clone().join("file"),
        )
        .expect("failed to hard link an encrypted file into an unencrypted directory ");

        let _ =
            std::fs::metadata(unencrypted_dir_path.clone().join("file")).expect("metadata failed");
        std::fs::File::open(unencrypted_dir_path.clone().join("file"))
            .expect_err("opening a locked encrypted file should fail");

        std::fs::remove_dir_all(unencrypted_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[serial]
    fn hard_link_unencrypted_file_into_encrypted_dir() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let unencrypted_dir_path = std::path::Path::new(&root_path).join("unencrypted");
        std::fs::create_dir_all(unencrypted_dir_path.clone()).unwrap();
        std::fs::File::create_new(unencrypted_dir_path.clone().join("file"))
            .expect("failed to create file in unencrypted directory");

        let encrypted_dir_path = std::path::Path::new(&root_path).join("encrypted");
        std::fs::create_dir_all(encrypted_dir_path.clone()).unwrap();

        let dir = std::fs::File::open(encrypted_dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::hard_link(
            unencrypted_dir_path.join("file"),
            encrypted_dir_path.clone().join("subdir"),
        )
        .expect_err(
            "hard linking an unencrypted file into an encrypted directory should fail with EXDEV",
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(unencrypted_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[serial]
    fn rename_unencrypted_file_into_encrypted_dir() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let unencrypted_dir_path = std::path::Path::new(&root_path).join("rename_unencrypted");
        std::fs::create_dir_all(unencrypted_dir_path.clone()).unwrap();
        std::fs::File::create_new(unencrypted_dir_path.clone().join("file"))
            .expect("failed to create file in unencrypted directory");

        let encrypted_dir_path = std::path::Path::new(&root_path).join("rename_encrypted");
        std::fs::create_dir_all(encrypted_dir_path.clone()).unwrap();

        let dir = std::fs::File::open(encrypted_dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = set_encryption_policy(&dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let err =
            std::fs::rename(unencrypted_dir_path.join("file"), encrypted_dir_path.join("file"))
                .expect_err("renaming unencrypted file into encrypted dir should fail with EXDEV");
        assert_eq!(err.raw_os_error(), Some(libc::EXDEV));

        let ret =
            remove_encryption_key(&root_dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(unencrypted_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[serial]
    fn rename_encrypted_file_into_unencrypted_dir() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let unencrypted_dir_path = std::path::Path::new(&root_path).join("rename_unencrypted_2");
        std::fs::create_dir_all(unencrypted_dir_path.clone()).unwrap();

        let encrypted_dir_path = std::path::Path::new(&root_path).join("rename_encrypted_2");
        std::fs::create_dir_all(encrypted_dir_path.clone()).unwrap();

        let dir = std::fs::File::open(encrypted_dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = set_encryption_policy(&dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::File::create_new(encrypted_dir_path.clone().join("file"))
            .expect("failed to create file in encrypted directory");

        let err =
            std::fs::rename(encrypted_dir_path.join("file"), unencrypted_dir_path.join("file"))
                .expect_err("renaming encrypted file into unencrypted dir should fail with EXDEV");
        assert_eq!(err.raw_os_error(), Some(libc::EXDEV));

        let ret =
            remove_encryption_key(&root_dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(unencrypted_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn hard_link_encrypted_file_into_second_dir_encrypted_with_same_policy() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let encrypted_1_dir_path = std::path::Path::new(&root_path).join("encrypted_1");
        std::fs::create_dir_all(encrypted_1_dir_path.clone()).unwrap();

        let encrypted_2_dir_path = std::path::Path::new(&root_path).join("encrypted_2");
        std::fs::create_dir_all(encrypted_2_dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, arg_struct_raw_key) =
            arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let dir_1 = std::fs::File::open(encrypted_1_dir_path.clone()).unwrap();
        let dir_2 = std::fs::File::open(encrypted_2_dir_path.clone()).unwrap();

        let ret =
            unsafe { set_encryption_policy(&dir_1, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::File::create_new(encrypted_1_dir_path.clone().join("file"))
            .expect("failed to create file in unencrypted directory");

        let ret =
            unsafe { set_encryption_policy(&dir_2, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::hard_link(
            encrypted_1_dir_path.join("file"),
            encrypted_2_dir_path.clone().join("subdir"),
        )
        .expect_err(
            "hard linking between two directories encrypted with the same policy should fail if
                they are locked",
        );

        let (ret, _) = add_encryption_key_with_key_bytes(&root_dir, arg_struct_raw_key);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());

        std::fs::hard_link(
            encrypted_1_dir_path.join("file"),
            encrypted_2_dir_path.clone().join("subdir"),
        )
        .expect(
            "failed to hard link a file from an encrypted directory into another directory
                encrypted with the same policy",
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(encrypted_1_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_2_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn rename_in_locked_encrypted_directory_fails() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let encrypted_dir_path = std::path::Path::new(&root_path).join("rename_locked_encrypted");
        std::fs::create_dir_all(encrypted_dir_path.clone()).unwrap();

        let dir = std::fs::File::open(encrypted_dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = set_encryption_policy(&dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::File::create_new(encrypted_dir_path.clone().join("file"))
            .expect("failed to create file in encrypted directory");

        let ret =
            remove_encryption_key(&root_dir, unsafe { arg_struct_1.key_spec.u.identifier.value });
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let err = std::fs::rename(
            encrypted_dir_path.join("file"),
            encrypted_dir_path.join("file_renamed"),
        )
        .expect_err("renaming inside locked encrypted directory should fail with ENOKEY");
        assert_eq!(err.raw_os_error(), Some(libc::ENOKEY));

        std::fs::remove_dir_all(encrypted_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[serial]
    fn hard_link_encrypted_file_into_second_dir_encrypted_with_different_policy() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let encrypted_1_dir_path = std::path::Path::new(&root_path).join("encrypted_1");
        std::fs::create_dir_all(encrypted_1_dir_path.clone()).unwrap();

        let encrypted_2_dir_path = std::path::Path::new(&root_path).join("encrypted_2");
        std::fs::create_dir_all(encrypted_2_dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let dir_1 = std::fs::File::open(encrypted_1_dir_path.clone()).unwrap();
        let dir_2 = std::fs::File::open(encrypted_2_dir_path.clone()).unwrap();

        let ret =
            unsafe { set_encryption_policy(&dir_1, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::File::create_new(encrypted_1_dir_path.clone().join("file"))
            .expect("failed to create file in unencrypted directory");

        let ret = unsafe {
            set_encryption_policy_hctr2(&dir_2, arg_struct_1.key_spec.u.identifier.value)
        };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::hard_link(
            encrypted_1_dir_path.join("file"),
            encrypted_2_dir_path.clone().join("subdir"),
        )
        .expect(
            "hard linking a file in encrypted directory into a second encrypted directory with a
                different policy should fail",
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(encrypted_1_dir_path).expect("failed to remove unencrypted dir");
        std::fs::remove_dir_all(encrypted_2_dir_path).expect("failed to remove encrypted dir");
    }

    #[test]
    #[serial]
    fn set_encryption_policy_on_directory_encrypted_with_different_policy() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_1 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct_1.key_spec.u.identifier.value) };

        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct_2 = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct_2.key_spec.u.identifier.value) };

        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        // Cleanup
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_1.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct_2.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn set_encryption_policy_on_file() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let file =
            std::fs::File::create_new(dir_path.join("file.txt")).expect("create file failed");
        let ret = unsafe { set_encryption_policy(&file, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn set_encryption_policy_on_non_empty_directory() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        std::fs::create_dir(dir_path.join("subdir")).expect("create dir failed");
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret != 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn set_encryption_key_on_directory_owned_by_different_user() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let child_binary_path = get_child_binary_path();

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn set_encryption_policy_with_encryption_key_added_by_different_user() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let child_binary_path = get_child_binary_path();

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add"])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        std::os::unix::fs::chown(dir_path.clone(), Some(2000), Some(2000)).expect("chown failed");
        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "set",
                "--should-fail",
                "true",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn stat_encrypted_symlink() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let target_path = dir_path.clone().join("foo.txt");
        let link = dir_path.clone().join("blah.txt");
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&target_path)
                .unwrap();
        }
        std::os::unix::fs::symlink(target_path, &link).expect("failed to create symlink");
        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        for entry in entries {
            let entry = entry.expect("invalid entry");
            if entry.file_type().unwrap().is_symlink() {
                let encrypted_target_path =
                    std::fs::read_link(entry.path()).expect("read_link failed");
                let encrypted_target_len = encrypted_target_path.as_path().to_str().unwrap().len();
                let symlink_met = std::fs::symlink_metadata(entry.path())
                    .expect("failed to read metadata of symlink");
                assert_eq!(encrypted_target_len as u64, symlink_met.len());
                std::fs::metadata(encrypted_target_path)
                    .expect_err("there should be no existing directory at this gibberish path");
            }
        }
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn create_symlink_in_locked_directory_fails() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let target_path = dir_path.clone().join("foo.txt");
        let link = dir_path.clone().join("blah.txt");
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&target_path)
                .unwrap();
        }

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::os::unix::fs::symlink(target_path, &link)
            .expect_err("creating a symlink in locked directory should fail");
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn stat_locked_file() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();
        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, arg_struct_raw_key) =
            arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(dir_path.clone().join("foo.txt"))
                .unwrap();
        }
        drop(dir);

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut encrypted_file_name = None;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            if entry.file_type().unwrap().is_file() {
                eprintln!("file name is {:?}", entry.file_name());
                encrypted_file_name = Some(entry.file_name());
                break;
            }
        }

        let _ = std::fs::metadata(dir_path.clone().join(encrypted_file_name.clone().unwrap()))
            .expect("metadata failed");

        let (ret, _) = add_encryption_key_with_key_bytes(&root_dir, arg_struct_raw_key);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());

        std::fs::metadata(dir_path.clone().join(encrypted_file_name.unwrap()))
            .expect_err("metadata should fail on an encrypted path for an unlocked directory");
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn one_user_adds_the_same_encryption_key_twice() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();

        let child_binary_path = get_child_binary_path();
        let wrapping_key: [u8; 64] = [2; 64];
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);
        let child_binary_path = get_child_binary_path();

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn different_user_add_the_same_encryption_key() {
        let Some(root_path) = get_root_path() else { return };
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let child_binary_path = get_child_binary_path();
        let wrapping_key: [u8; 64] = [2; 64];
        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "1000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        eprintln!("std err is {:?}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{:#?}", output.status);
        let child_binary_path = get_child_binary_path();

        let output = std::process::Command::new(child_binary_path)
            .args(["--uid", "2000", "add", "--key", &hex::encode(wrapping_key)])
            .output()
            .expect("set encryption policy failed");
        let output_str = String::from_utf8_lossy(&output.stdout);
        let fscrypt_output: FscryptOutput = serde_json::from_str(&output_str).unwrap();
        assert!(output.status.success(), "{:#?}", output.status);

        let child_binary_path = get_child_binary_path();
        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "set",
                "--should-fail",
                "false",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
            ])
            .output()
            .expect("set encryption policy failed");
        assert!(output.status.success(), "{:#?}", output.status);

        std::fs::create_dir(dir_path.join("subdir")).unwrap();
        drop(dir);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "1000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() == "subdir");
            count += 1;
        }
        assert_eq!(count, 1);

        let child_binary_path = get_child_binary_path();
        let output = std::process::Command::new(child_binary_path)
            .args([
                "--uid",
                "2000",
                "remove",
                "--identifier",
                &hex::encode(fscrypt_output.identifier),
                "--should-fail",
                "false",
            ])
            .output()
            .expect("remove encryption key failed");
        assert!(output.status.success(), "{:#?}", output.status);
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            assert!(entry.file_name() != "subdir");
            count += 1;
        }
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[serial]
    fn root_sets_encryption_policy_on_a_directory_it_does_not_own() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        std::os::unix::fs::chown(dir_path.clone(), Some(1000), Some(1000)).expect("chown failed");
        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        std::fs::remove_dir_all(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn unlink_locked_empty_encrypted_directory() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        std::fs::create_dir(dir_path.join("subdir")).unwrap();
        drop(dir);

        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut encrypted_dir_name = OsString::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            encrypted_dir_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::remove_dir(dir_path.clone().join(encrypted_dir_name)).expect("remove dir failed");
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for _ in entries {
            count += 1;
        }
        assert_eq!(count, 0);
        std::fs::remove_dir(dir_path).expect("failed to remove my_dir");
    }

    #[test]
    #[ignore] // TODO(https://fxbug.dev/359885449) use expectations
    #[serial]
    // TODO(https://fxbug.dev/358420498) Purge the key manager on FS_IOC_REMOVE_ENCRYPTION_KEY
    fn unlink_locked_encrypted_file() {
        let Some(root_path) = get_root_path() else { return };
        let root_dir = std::fs::File::open(&root_path).expect("open failed");
        let dir_path = std::path::Path::new(&root_path).join("my_dir");
        std::fs::create_dir_all(dir_path.clone()).unwrap();
        let dir = std::fs::File::open(dir_path.clone()).unwrap();

        let (ret, arg_vec) = add_encryption_key(&root_dir);
        assert!(ret == 0, "add encryption key ioctl failed: {:?}", std::io::Error::last_os_error());
        let (arg_struct_bytes, _) = arg_vec.split_at(std::mem::size_of::<fscrypt_add_key_arg>());
        let arg_struct = fscrypt_add_key_arg::read_from_bytes(arg_struct_bytes).unwrap();

        let ret = unsafe { set_encryption_policy(&dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "set encryption policy ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        {
            let _ = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(dir_path.clone().join("foo.txt"))
                .unwrap();
        }

        drop(dir);
        let ret =
            unsafe { remove_encryption_key(&root_dir, arg_struct.key_spec.u.identifier.value) };
        assert!(
            ret == 0,
            "remove encryption key ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );

        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut encrypted_file_name = OsString::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.expect("invalid entry");
            encrypted_file_name = entry.file_name();
            count += 1;
        }
        assert_eq!(count, 1);

        std::fs::remove_file(dir_path.clone().join(encrypted_file_name))
            .expect("remove file failed");
        let entries = std::fs::read_dir(dir_path.clone()).expect("readdir failed");
        let mut count = 0;
        for _ in entries {
            count += 1;
        }
        assert_eq!(count, 0);
        std::fs::remove_dir(dir_path).expect("failed to remove my_dir");
    }
}
