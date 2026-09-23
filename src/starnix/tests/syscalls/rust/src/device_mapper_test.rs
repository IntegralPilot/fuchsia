// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod device_mapper_test {
    use linux_uapi::{
        DM_ACTIVE_PRESENT_FLAG, DM_BUFFER_FULL_FLAG, DM_DEV_CREATE, DM_DEV_REMOVE, DM_DEV_STATUS,
        DM_DEV_SUSPEND, DM_INACTIVE_PRESENT_FLAG, DM_LIST_DEVICES, DM_LIST_VERSIONS,
        DM_MAX_TYPE_NAME, DM_NAME_LEN, DM_READONLY_FLAG, DM_STATUS_TABLE_FLAG, DM_SUSPEND_FLAG,
        DM_TABLE_LOAD, DM_TABLE_STATUS, DM_UEVENT_GENERATED_FLAG, DM_UUID_LEN, DM_VERSION_MAJOR,
        DM_VERSION_MINOR, DM_VERSION_PATCHLEVEL, LOOP_CONFIGURE, LOOP_CTL_GET_FREE, dm_ioctl,
    };
    use serial_test::serial;
    use std::collections::HashSet;
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use test_case::test_case;
    use zerocopy::{FromBytes, IntoBytes};

    const DM_VERSION0: u32 = 4;
    const DM_VERSION1: u32 = 0;
    const DM_VERSION2: u32 = 0;
    const SECTOR_SIZE: u64 = 512;
    const LOOP_MAJOR: u32 = 7;
    const DATA_BLOCK_SIZE: usize = 4096;
    const HASH_BLOCK_SIZE: usize = 4096;
    const SHA256_ALGORITHM: &str = "sha256";
    const SHA512_ALGORITHM: &str = "sha512";

    // Based on observable Linux kernel behavior from Kernel: Linux 6.6.15-2rodete2-amd64
    const DM_VERITY_VERSION_MAJOR: u32 = 1;
    const DM_VERITY_VERSION_MINOR: u32 = 9;
    const DM_VERITY_VERSION_PATCHLEVEL: u32 = 0;

    fn init_io(io: &mut dm_ioctl, name: Vec<std::ffi::c_char>) {
        io.version[0] = DM_VERSION0;
        io.version[1] = DM_VERSION1;
        io.version[2] = DM_VERSION2;
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_start = 0;
        if name.len() > 0 {
            io.name[0..name.len()].copy_from_slice(&name);
        }
    }

    fn check_io(io: &dm_ioctl, name: [std::ffi::c_char; 128], uuid: [std::ffi::c_char; 129]) {
        assert_eq!(io.name, name);
        assert_eq!(io.uuid, uuid);
        assert_eq!(io.version[0], DM_VERSION_MAJOR);
        assert_eq!(io.version[1], DM_VERSION_MINOR);
        assert_eq!(io.version[2], DM_VERSION_PATCHLEVEL);
        // Based on observable Linux kernel behavior from Kernel: Linux 6.6.15-2rodete2-amd64.
        assert_eq!(io.data_size, 305);
        assert_eq!(io.data_start, 0);
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VerityTargetState {
        // A valid dm-verity target. Both the Merkle tree and the data blocks are valid, and the
        // root hash matches. Tests that normal reads succeed.
        Valid,
        // The hash tree and root hash matches, but does not match the data blocks. The load should
        // succeed, but reads should fail. Tests read-time failure.
        CorruptedTreeWithMatchingRoot,
        // The hash tree is valid, but the root hash passed at load time does not match the tree.
        // The load should fail immediately. Tests load-time failure.
        ValidTreeWithMismatchedRoot,
    }

    fn create_verity_target_two_loop_devices_internal(
        state: VerityTargetState,
        hash_algorithm: &str,
    ) -> (String, linux_uapi::dm_target_spec, Vec<u8>) {
        let mut ext4_image_file =
            OpenOptions::new().read(true).open("data/device_mapper_ext4.img").unwrap();

        let mut ext4_image = vec![];
        ext4_image_file.read_to_end(&mut ext4_image).expect("failed to read ext4 image");
        let image_size = ext4_image.len();

        let hashtree_file = match state {
            VerityTargetState::Valid | VerityTargetState::ValidTreeWithMismatchedRoot => {
                OpenOptions::new()
                    .read(true)
                    .open(format!("data/hashtree_{hash_algorithm}.txt"))
                    .unwrap()
            }
            VerityTargetState::CorruptedTreeWithMatchingRoot => OpenOptions::new()
                .read(true)
                .open(format!("data/corrupted_hashtree_{hash_algorithm}.txt"))
                .unwrap(),
        };

        let root_hash = match state {
            VerityTargetState::Valid => {
                let mut root_hash_file = OpenOptions::new()
                    .read(true)
                    .open(format!("data/root_hash_{hash_algorithm}.txt"))
                    .unwrap();
                let mut hash = String::new();
                root_hash_file.read_to_string(&mut hash).expect("failed to read the root_hash");
                hash.trim().to_string()
            }
            VerityTargetState::CorruptedTreeWithMatchingRoot => {
                let mut root_hash_file = OpenOptions::new()
                    .read(true)
                    .open(format!("data/corrupted_root_hash_{hash_algorithm}.txt"))
                    .unwrap();
                let mut hash = String::new();
                root_hash_file
                    .read_to_string(&mut hash)
                    .expect("failed to read the corrupted_tree_valid_root_hash");
                hash.trim().to_string()
            }
            VerityTargetState::ValidTreeWithMismatchedRoot => {
                if hash_algorithm == SHA256_ALGORITHM { "0".repeat(64) } else { "0".repeat(128) }
            }
        };

        let loop_control_device =
            OpenOptions::new().read(true).write(true).open("/dev/loop-control").unwrap();
        let image_device_num = unsafe {
            libc::ioctl(loop_control_device.as_raw_fd(), LOOP_CTL_GET_FREE.try_into().unwrap())
        };
        assert!(
            image_device_num >= 0,
            "loop ctl get free ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let image_loop_device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&format!("/dev/loop{image_device_num}"))
            .unwrap();

        let config = linux_uapi::loop_config {
            fd: ext4_image_file.as_raw_fd() as u32,
            block_size: 4096,
            ..Default::default()
        };
        let ret = unsafe {
            libc::ioctl(image_loop_device.as_raw_fd(), LOOP_CONFIGURE.try_into().unwrap(), &config)
        };
        assert!(ret == 0, "loop configure ioctl failed: {:?}", std::io::Error::last_os_error());

        let hashtree_device_num = unsafe {
            libc::ioctl(loop_control_device.as_raw_fd(), LOOP_CTL_GET_FREE.try_into().unwrap())
        };
        assert!(
            hashtree_device_num >= 0,
            "loop ctl get free ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let hashtree_loop_device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&format!("/dev/loop{hashtree_device_num}"))
            .unwrap();

        let config = linux_uapi::loop_config {
            fd: hashtree_file.as_raw_fd() as u32,
            block_size: 4096,
            ..Default::default()
        };
        let ret = unsafe {
            libc::ioctl(
                hashtree_loop_device.as_raw_fd(),
                LOOP_CONFIGURE.try_into().unwrap(),
                &config,
            )
        };
        assert!(ret == 0, "loop configure ioctl failed: {:?}", std::io::Error::last_os_error());

        let parameter_string = format!(
            "1 {LOOP_MAJOR}:{image_device_num} {LOOP_MAJOR}:{hashtree_device_num} {DATA_BLOCK_SIZE} {HASH_BLOCK_SIZE} {:?} 0 {hash_algorithm} {root_hash} ffffffffffffffff 1 ignore_zero_blocks",
            image_size / DATA_BLOCK_SIZE
        );
        let target_type_cstr = std::ffi::CString::new("verity").unwrap();
        let target_type = target_type_cstr
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let mut padding = 0;
        if (parameter_string.len() + 1 + std::mem::size_of::<linux_uapi::dm_target_spec>()) % 8 != 0
        {
            padding = 8
                - ((parameter_string.len()
                    + 1
                    + std::mem::size_of::<linux_uapi::dm_target_spec>())
                    % 8);
        }

        let mut target_spec = linux_uapi::dm_target_spec {
            sector_start: 0,
            length: (image_size / 512) as u64,
            next: (parameter_string.len()
                + 1
                + std::mem::size_of::<linux_uapi::dm_target_spec>()
                + padding) as u32,
            ..Default::default()
        };

        target_spec.target_type[..target_type.len()].copy_from_slice(&target_type);

        let mut target_vec = target_spec.as_bytes().to_vec();
        target_vec
            .extend(std::ffi::CString::new(parameter_string.clone()).unwrap().as_bytes_with_nul());

        for _ in 0..padding {
            target_vec.push(0);
        }

        (parameter_string, target_spec, target_vec)
    }

    fn create_verity_target_two_loop_devices_sha256(
        state: VerityTargetState,
    ) -> (String, linux_uapi::dm_target_spec, Vec<u8>) {
        create_verity_target_two_loop_devices_internal(state, SHA256_ALGORITHM)
    }

    fn create_verity_target_two_loop_devices_sha512(
        state: VerityTargetState,
    ) -> (String, linux_uapi::dm_target_spec, Vec<u8>) {
        create_verity_target_two_loop_devices_internal(state, SHA512_ALGORITHM)
    }

    fn create_verity_target_shared_loop_device(
        state: VerityTargetState,
    ) -> (String, linux_uapi::dm_target_spec, Vec<u8>) {
        let mut ext4_image_with_hashtree_file = match state {
            VerityTargetState::Valid | VerityTargetState::ValidTreeWithMismatchedRoot => {
                OpenOptions::new()
                    .read(true)
                    .open(format!("data/valid_image_with_hashtree_{SHA256_ALGORITHM}.txt"))
                    .unwrap()
            }
            VerityTargetState::CorruptedTreeWithMatchingRoot => OpenOptions::new()
                .read(true)
                .open(format!("data/valid_image_with_corrupted_hashtree_{SHA256_ALGORITHM}.txt"))
                .unwrap(),
        };

        let mut ext4_image_with_hashtree = vec![];
        ext4_image_with_hashtree_file
            .read_to_end(&mut ext4_image_with_hashtree)
            .expect("failed to read ext4 image");
        // The hashtree has is 4096 bytes.
        let image_size = ext4_image_with_hashtree.len() - 4096;

        let root_hash = match state {
            VerityTargetState::Valid => {
                let mut root_hash_file = OpenOptions::new()
                    .read(true)
                    .open(format!("data/root_hash_{SHA256_ALGORITHM}.txt"))
                    .unwrap();
                let mut root_hash = String::new();
                let _ = root_hash_file
                    .read_to_string(&mut root_hash)
                    .expect("failed to read the root_hash");
                root_hash.trim().to_string()
            }
            VerityTargetState::CorruptedTreeWithMatchingRoot => {
                let mut root_hash_file = OpenOptions::new()
                    .read(true)
                    .open(format!("data/corrupted_root_hash_{SHA256_ALGORITHM}.txt"))
                    .unwrap();
                let mut hash = String::new();
                root_hash_file
                    .read_to_string(&mut hash)
                    .expect("failed to read the corrupted_tree_valid_root_hash");
                hash.trim().to_string()
            }
            VerityTargetState::ValidTreeWithMismatchedRoot => "0".repeat(64),
        };

        let loop_control_device =
            OpenOptions::new().read(true).write(true).open("/dev/loop-control").unwrap();
        let device_num = unsafe {
            libc::ioctl(loop_control_device.as_raw_fd(), LOOP_CTL_GET_FREE.try_into().unwrap())
        };
        assert!(
            device_num >= 0,
            "loop ctl get free ioctl failed: {:?}",
            std::io::Error::last_os_error()
        );
        let image_and_hashtree_loop_device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&format!("/dev/loop{device_num}"))
            .unwrap();

        let config = linux_uapi::loop_config {
            fd: ext4_image_with_hashtree_file.as_raw_fd() as u32,
            block_size: 4096,
            ..Default::default()
        };
        let ret = unsafe {
            libc::ioctl(
                image_and_hashtree_loop_device.as_raw_fd(),
                LOOP_CONFIGURE.try_into().unwrap(),
                &config,
            )
        };
        assert!(ret == 0, "loop configure ioctl failed: {:?}", std::io::Error::last_os_error());

        let parameter_string = format!(
            "1 {LOOP_MAJOR}:{device_num} {LOOP_MAJOR}:{device_num} {DATA_BLOCK_SIZE} {HASH_BLOCK_SIZE} {:?} {:?} {SHA256_ALGORITHM} {root_hash} ffffffffffffffff 1 ignore_zero_blocks",
            image_size / DATA_BLOCK_SIZE,
            image_size / HASH_BLOCK_SIZE
        );
        let target_type_cstr = std::ffi::CString::new("verity").unwrap();
        let target_type = target_type_cstr
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        let mut padding = 0;
        if (parameter_string.len() + 1 + std::mem::size_of::<linux_uapi::dm_target_spec>()) % 8 != 0
        {
            padding = 8
                - ((parameter_string.len()
                    + 1
                    + std::mem::size_of::<linux_uapi::dm_target_spec>())
                    % 8);
        }

        let mut target_spec = linux_uapi::dm_target_spec {
            sector_start: 0,
            length: (image_size / 512) as u64,
            next: (parameter_string.len()
                + 1
                + std::mem::size_of::<linux_uapi::dm_target_spec>()
                + padding) as u32,
            ..Default::default()
        };

        target_spec.target_type[..target_type.len()].copy_from_slice(&target_type);

        let mut target_vec = target_spec.as_bytes().to_vec();
        target_vec
            .extend(std::ffi::CString::new(parameter_string.clone()).unwrap().as_bytes_with_nul());

        for _ in 0..padding {
            target_vec.push(b'\0');
        }

        (parameter_string, target_spec, target_vec)
    }

    // TODO(https://fxbug.dev/317285180) panic if this fails once running as root for baseline
    fn open_dm_control() -> Option<std::fs::File> {
        match OpenOptions::new().read(true).write(true).open("/dev/mapper/control") {
            Ok(file) => Some(file),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => None,
                _ => panic!("open /dev/mapper/control failed with {:?}", e),
            },
        }
    }

    #[test]
    #[serial]
    fn create_dm_device() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let mut expected_name = [0; DM_NAME_LEN as usize];
        let mut expected_uuid = [0; DM_UUID_LEN as usize];
        expected_name[0..name_slice.len()].copy_from_slice(&name_slice);
        expected_uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        check_io(&io, expected_name, expected_uuid);
        assert_eq!(io.flags, 0);
        assert_eq!(io.target_count, 0);
        assert_eq!(io.open_count, 0);

        // Cleanup -- remove all created dm-devices.
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn create_dm_device_no_name() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };

        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, vec![]);
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn create_dm_device_no_version() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);
        io.version = [0; 3];

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn dm_dev_status() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let mut expected_name = [0; DM_NAME_LEN as usize];
        let mut expected_uuid = [0; DM_UUID_LEN as usize];
        expected_name[0..name_slice.len()].copy_from_slice(&name_slice);
        expected_uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dev = io.dev;

        // Only one of name, uuid, and dev should be set in the input dm_ioctl.
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.dev = dev;
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.dev = dev;
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.dev = dev;
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        check_io(&io, expected_name, expected_uuid);
        assert_eq!(io.flags, 0);
        assert_eq!(io.target_count, 0);
        assert_eq!(io.open_count, 0);

        // Cleanup -- remove all created dm-devices.
        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test_case(create_verity_target_shared_loop_device)]
    #[test_case(create_verity_target_two_loop_devices_sha256)]
    #[test_case(create_verity_target_two_loop_devices_sha512)]
    #[serial]
    fn read_loaded_and_resumed_dm_device(
        create_verity_target: fn(
            VerityTargetState,
        ) -> (String, linux_uapi::dm_target_spec, Vec<u8>),
    ) {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let mut expected_name = [0; DM_NAME_LEN as usize];
        let mut expected_uuid = [0; DM_UUID_LEN as usize];
        expected_name[0..name_slice.len()].copy_from_slice(&name_slice);
        expected_uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, target_spec, target_vec) = create_verity_target(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());
        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(io_struct.flags, DM_INACTIVE_PRESENT_FLAG);
        assert_eq!(io_struct.target_count, 0);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        check_io(&io, expected_name, expected_uuid);
        assert_eq!(io.flags, DM_INACTIVE_PRESENT_FLAG);
        assert_eq!(io.target_count, 0);
        assert_eq!(io.open_count, 0);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_READONLY_FLAG | DM_ACTIVE_PRESENT_FLAG | DM_UEVENT_GENERATED_FLAG);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_STATUS.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        check_io(&io, expected_name, expected_uuid);
        assert_eq!(io.flags, DM_READONLY_FLAG | DM_ACTIVE_PRESENT_FLAG);
        assert_eq!(io.target_count, 1);
        assert_eq!(io.open_count, 0);

        // Perform verified read
        {
            let path = format!("/dev/dm-{:?}", dm_minor);
            let mut dm_device = OpenOptions::new().read(true).open(&path).unwrap();

            let mut buf = vec![0; (target_spec.length * SECTOR_SIZE) as usize];
            let bytes_read = dm_device.read(&mut buf).expect("failed to read the dm-device");
            assert_eq!(bytes_read, (target_spec.length * SECTOR_SIZE) as usize);
        }

        // Do cleanup after the test
        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io_remove.flags, DM_UEVENT_GENERATED_FLAG);
    }

    #[test]
    #[serial]
    fn remove_dm_device_in_scope() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let dm_device =
            OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();

        // DM_DEV_REMOVE fails with an open device handle.
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());

        drop(dm_device);
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn remove_dm_device_cached_handle_in_scope() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        let dm_device =
            OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();

        let dm_device_2 =
            OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();
        drop(dm_device);

        // DM_DEV_REMOVE fails with an open device handle.
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret != 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());

        drop(dm_device_2);
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test_case(create_verity_target_shared_loop_device)]
    #[test_case(create_verity_target_two_loop_devices_sha256)]
    #[test_case(create_verity_target_two_loop_devices_sha512)]
    #[serial]
    fn read_corrupted_dm_verity_device(
        create_verity_target: fn(
            VerityTargetState,
        ) -> (String, linux_uapi::dm_target_spec, Vec<u8>),
    ) {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, target_spec, target_vec) =
            create_verity_target(VerityTargetState::CorruptedTreeWithMatchingRoot);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());

        // Perform verified read
        {
            let mut dm_device =
                OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();
            let mut buf = vec![0; (target_spec.length * SECTOR_SIZE) as usize];
            dm_device.read(&mut buf).expect_err("unexpectedly passed reading dm-device");
        }

        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test_case(create_verity_target_shared_loop_device)]
    #[test_case(create_verity_target_two_loop_devices_sha256)]
    #[test_case(create_verity_target_two_loop_devices_sha512)]
    #[serial]
    fn test_dm_table_load_invalid_root_hash(
        create_verity_target: fn(
            VerityTargetState,
        ) -> (String, linux_uapi::dm_target_spec, Vec<u8>),
    ) {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-invalid-root").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (_, _target_spec, target_vec) =
            create_verity_target(VerityTargetState::ValidTreeWithMismatchedRoot);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(
            ret != 0,
            "dm table load succeeded but should have failed due to invalid root hash"
        );
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap();
        assert_eq!(errno, libc::EINVAL);

        // Clean up
        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn read_dm_device_no_active_table() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, target_spec, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.flags |= DM_SUSPEND_FLAG;
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_INACTIVE_PRESENT_FLAG | DM_SUSPEND_FLAG);

        // Perform verified read
        {
            let mut dm_device =
                OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();
            let mut buf = vec![0; (target_spec.length * SECTOR_SIZE) as usize];
            let bytes_read = dm_device.read(&mut buf).expect("failed to read the dm-device");
            assert_eq!(bytes_read, 0);
        }

        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io_remove.flags, DM_UEVENT_GENERATED_FLAG | DM_INACTIVE_PRESENT_FLAG);
    }

    #[test]
    #[serial]
    fn suspend_and_resume_dm_device() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        // Suspend an inactive table.
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.flags |= DM_SUSPEND_FLAG;
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_INACTIVE_PRESENT_FLAG | DM_SUSPEND_FLAG);

        // Activate the table.
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_ACTIVE_PRESENT_FLAG | DM_READONLY_FLAG | DM_UEVENT_GENERATED_FLAG);

        // Suspending an active table leaves it in the active table slot.
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.flags |= DM_SUSPEND_FLAG;
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_ACTIVE_PRESENT_FLAG | DM_SUSPEND_FLAG | DM_READONLY_FLAG);

        // While the first table is active and suspended, load a second table.
        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());
        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(
            io_struct.flags,
            DM_INACTIVE_PRESENT_FLAG | DM_ACTIVE_PRESENT_FLAG | DM_SUSPEND_FLAG | DM_READONLY_FLAG
        );

        // The inactive table now replaces the active table.
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev suspend ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io.flags, DM_ACTIVE_PRESENT_FLAG | DM_READONLY_FLAG | DM_UEVENT_GENERATED_FLAG);

        let mut io_remove: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io_remove, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io_remove)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
        assert_eq!(io_remove.flags, DM_UEVENT_GENERATED_FLAG);
    }

    #[test]
    #[serial]
    fn list_devices_buffer_full() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        // Do not create space for the name, event_nr, or flags following the dm_name_list.
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + std::mem::size_of::<linux_uapi::dm_name_list>() as u32;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; std::mem::size_of::<linux_uapi::dm_name_list>()]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_LIST_DEVICES.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm list devices ioctl failed: {:?}", std::io::Error::last_os_error());
        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(io_struct.flags, DM_BUFFER_FULL_FLAG);

        // Cleanup -- delete all created dm-devices.
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn dm_list_devices() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice_1 = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice_1.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dev_1 = io.dev;

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        let name = std::ffi::CString::new("test-device-2").unwrap();
        let name_slice_2 = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'b'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice_2.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dev_2 = io.dev;

        let mut expected_devs: HashSet<(String, u64)> = HashSet::new();
        expected_devs.insert((String::from("test-device-1"), dev_1));
        expected_devs.insert((String::from("test-device-2"), dev_2));

        let mut observed_devs: HashSet<(String, u64)> = HashSet::new();

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        let per_device_metadata_size =
            std::mem::size_of::<linux_uapi::dm_name_list>() as u32 + DM_NAME_LEN;
        let per_device_aligned_metadata_size =
            per_device_metadata_size + (8 - per_device_metadata_size % 8);
        // 4 because Linux has 2 dm-devices already on the system.
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 4 * per_device_aligned_metadata_size;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; 4 * per_device_aligned_metadata_size as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_LIST_DEVICES.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm list devices ioctl failed: {:?}", std::io::Error::last_os_error());

        let (io_struct_bytes, data_bytes) =
            io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        let mut next = 0;
        let mut data_size = io_struct.data_size as usize
            - std::cmp::min(
                io_struct.data_size as usize,
                std::mem::size_of::<linux_uapi::dm_ioctl>(),
            );
        while data_size > 0 {
            let name_list = linux_uapi::dm_name_list::read_from_bytes(
                &data_bytes[next..next + std::mem::size_of::<linux_uapi::dm_name_list>()],
            )
            .unwrap();
            let name_list_bytes = name_list.as_bytes();
            // Based on Linux behavior -- the kernel starts writing the name of the device in the
            // padding of the dm_name_list struct.
            let name_prefix = &name_list_bytes[name_list_bytes.len() - 4..];
            let name_suffix = std::ffi::CStr::from_bytes_until_nul(
                &data_bytes[next + std::mem::size_of::<linux_uapi::dm_name_list>()..],
            )
            .unwrap()
            .to_bytes();
            let name = format!(
                "{}{}",
                String::from_utf8(name_prefix.to_vec()).unwrap(),
                String::from_utf8(name_suffix.to_vec()).unwrap()
            );
            assert!(name.len() <= DM_NAME_LEN as usize);
            if name.contains("test-device") {
                observed_devs.insert((name, name_list.dev));
            }
            data_size -= name_list.next as usize;
            if name_list.next == 0 {
                break;
            }
            next += name_list.next as usize;
        }
        assert_eq!(observed_devs, expected_devs);

        // Cleanup -- delete all created dm-devices.
        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice_1);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice_2);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn list_versions_buffer_full() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        // No space allocated for the target type following the dm_target_versions struct.
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + std::mem::size_of::<linux_uapi::dm_target_versions>() as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; std::mem::size_of::<linux_uapi::dm_target_versions>()]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_LIST_VERSIONS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm list versions ioctl failed: {:?}", std::io::Error::last_os_error());
        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(io_struct.flags, DM_BUFFER_FULL_FLAG);
    }

    #[test]
    #[serial]
    fn dm_list_versions() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };

        let mut observed_devs: HashSet<(String, [u32; 3])> = HashSet::new();

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, vec![]);
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        let per_device_metadata_size =
            std::mem::size_of::<linux_uapi::dm_target_versions>() as u32 + DM_MAX_TYPE_NAME;
        let per_device_aligned_metadata_size =
            per_device_metadata_size + (8 - per_device_metadata_size % 8);
        // 4 because Linux returns versions for 4 different target types.
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 4 * per_device_aligned_metadata_size;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; 4 * per_device_aligned_metadata_size as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_LIST_VERSIONS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm list versions ioctl failed: {:?}", std::io::Error::last_os_error());
        let (io_struct_bytes, data_bytes) =
            io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        let mut next = 0;
        let mut data_size = io_struct.data_size as usize
            - std::cmp::min(
                io_struct.data_size as usize,
                std::mem::size_of::<linux_uapi::dm_ioctl>(),
            );
        while data_size > 0 {
            let target_versions = linux_uapi::dm_target_versions::read_from_bytes(
                &data_bytes[next..next + std::mem::size_of::<linux_uapi::dm_target_versions>()],
            )
            .unwrap();
            let name_cstr = std::ffi::CStr::from_bytes_until_nul(
                &data_bytes[next + std::mem::size_of::<linux_uapi::dm_target_versions>()..],
            )
            .unwrap()
            .to_bytes();
            let name = String::from_utf8(name_cstr.to_vec()).unwrap();
            assert!(name.len() <= DM_MAX_TYPE_NAME as usize);
            observed_devs.insert((name, target_versions.version));
            data_size -= target_versions.next as usize;
            if target_versions.next == 0 {
                break;
            }
            next += target_versions.next as usize;
        }
        assert!(observed_devs.contains(&(
            String::from("verity"),
            [DM_VERITY_VERSION_MAJOR, DM_VERITY_VERSION_MINOR, DM_VERITY_VERSION_PATCHLEVEL],
        )));
    }

    #[test]
    #[serial]
    fn dm_table_status_buffer_full() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev resume ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        // No space for the status string following the dm_target_spec struct.
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + std::mem::size_of::<linux_uapi::dm_target_spec>() as u32;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; std::mem::size_of::<linux_uapi::dm_target_spec>() as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_TABLE_STATUS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm table status ioctl failed: {:?}", std::io::Error::last_os_error());

        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(
            io_struct.flags,
            DM_BUFFER_FULL_FLAG | DM_ACTIVE_PRESENT_FLAG | DM_READONLY_FLAG
        );

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn dm_table_status_no_active_table() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (_, _, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as u32;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_TABLE_STATUS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm table status ioctl failed: {:?}", std::io::Error::last_os_error());
        // DM_TABLE_STATUS will return no target metadata if there is no active table.
        let (io_struct_bytes, _) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        assert_eq!(io_struct.data_size, 305);
        assert_eq!(io_struct.data_start, std::mem::size_of::<linux_uapi::dm_ioctl>() as u32);
        assert_eq!(io_struct.flags, DM_INACTIVE_PRESENT_FLAG);

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test_case(create_verity_target_shared_loop_device)]
    #[test_case(create_verity_target_two_loop_devices_sha256)]
    #[test_case(create_verity_target_two_loop_devices_sha512)]
    #[serial]
    fn dm_table_status_corrupt(
        create_verity_target: fn(
            VerityTargetState,
        ) -> (String, linux_uapi::dm_target_spec, Vec<u8>),
    ) {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());
        let dm_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;

        let (_, target_spec, target_vec) =
            create_verity_target(VerityTargetState::CorruptedTreeWithMatchingRoot);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev resume ioctl failed: {:?}", std::io::Error::last_os_error());

        // Ensure read of corrupted device fails.
        {
            let mut dm_device =
                OpenOptions::new().read(true).open(&format!("/dev/dm-{:?}", dm_minor)).unwrap();
            let mut buf = vec![0; (target_spec.length * SECTOR_SIZE) as usize];
            let _bytes_read =
                dm_device.read(&mut buf).expect_err("succeeded to read the corrupt dm-device");
        }

        let mut observed_table_statuses: HashSet<(u64, u64, i32, String, Option<String>)> =
            HashSet::new();

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as u32;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_TABLE_STATUS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm table status ioctl failed: {:?}", std::io::Error::last_os_error());

        let (io_struct_bytes, rest) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        let mut next = 0;
        let mut data_size = io_struct.data_size as usize
            - std::cmp::min(
                io_struct.data_size as usize,
                std::mem::size_of::<linux_uapi::dm_ioctl>(),
            );
        while data_size > 0 {
            let target_specs = linux_uapi::dm_target_spec::read_from_bytes(
                &rest[next..next + std::mem::size_of::<linux_uapi::dm_target_spec>()],
            )
            .unwrap();
            let data = if target_specs.next as usize
                > next + std::mem::size_of::<linux_uapi::dm_target_spec>()
            {
                let data_c_str = std::ffi::CStr::from_bytes_until_nul(
                    &rest[next + std::mem::size_of::<linux_uapi::dm_target_spec>()
                        ..target_specs.next as usize],
                )
                .unwrap()
                .to_bytes();
                Some(String::from_utf8(data_c_str.to_vec()).unwrap())
            } else {
                None
            };
            let target_type =
                target_specs.target_type.iter().map(|v| *v as u8).collect::<Vec<u8>>();
            let target_type_c_str = std::ffi::CStr::from_bytes_until_nul(target_type.as_bytes())
                .unwrap()
                .to_bytes()
                .to_vec();
            observed_table_statuses.insert((
                target_specs.sector_start,
                target_specs.length,
                target_specs.status,
                String::from_utf8(target_type_c_str).unwrap(),
                data,
            ));
            if target_specs.next as usize - next > data_size {
                break;
            }
            data_size -= target_specs.next as usize - next;
            next = target_specs.next as usize;
        }
        assert!(observed_table_statuses.contains(&(
            target_spec.sector_start,
            target_spec.length,
            target_spec.status,
            String::from("verity"),
            Some(String::from("C")),
        )));

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn dm_table_status_valid() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (_, target_spec, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev resume ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut observed_table_statuses: HashSet<(u64, u64, i32, String, Option<String>)> =
            HashSet::new();

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as u32;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![0; 2 * std::mem::size_of::<linux_uapi::dm_target_spec>() as usize]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_TABLE_STATUS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm table status ioctl failed: {:?}", std::io::Error::last_os_error());

        let (io_struct_bytes, rest) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        let mut next = 0;
        let mut data_size = io_struct.data_size as usize
            - std::cmp::min(
                io_struct.data_size as usize,
                std::mem::size_of::<linux_uapi::dm_ioctl>(),
            );
        while data_size > 0 {
            let target_specs = linux_uapi::dm_target_spec::read_from_bytes(
                &rest[next..next + std::mem::size_of::<linux_uapi::dm_target_spec>()],
            )
            .unwrap();
            let data = if target_specs.next as usize
                > next + std::mem::size_of::<linux_uapi::dm_target_spec>()
            {
                let data_c_str = std::ffi::CStr::from_bytes_until_nul(
                    &rest[next + std::mem::size_of::<linux_uapi::dm_target_spec>()
                        ..target_specs.next as usize],
                )
                .unwrap()
                .to_bytes();
                Some(String::from_utf8(data_c_str.to_vec()).unwrap())
            } else {
                None
            };
            let target_type =
                target_specs.target_type.iter().map(|v| *v as u8).collect::<Vec<u8>>();
            let target_type_c_str = std::ffi::CStr::from_bytes_until_nul(target_type.as_bytes())
                .unwrap()
                .to_bytes()
                .to_vec();
            observed_table_statuses.insert((
                target_specs.sector_start,
                target_specs.length,
                target_specs.status,
                String::from_utf8(target_type_c_str).unwrap(),
                data,
            ));
            if target_specs.next as usize - next > data_size {
                break;
            }
            data_size -= target_specs.next as usize - next;
            next = target_specs.next as usize;
        }
        assert!(observed_table_statuses.contains(&(
            target_spec.sector_start,
            target_spec.length,
            target_spec.status,
            String::from("verity"),
            Some(String::from("V")),
        )));

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }

    #[test]
    #[serial]
    fn dm_table_status_flag() {
        let Some(dm_control) = open_dm_control() else {
            return;
        };
        let mut io = linux_uapi::dm_ioctl { ..Default::default() };

        let name = std::ffi::CString::new("test-device-1").unwrap();
        let name_slice = name
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();
        let uuid = std::ffi::CString::new(vec![b'a'; 37]).unwrap();
        let uuid_slice = uuid
            .as_bytes_with_nul()
            .iter()
            .map(|v| *v as std::ffi::c_char)
            .collect::<Vec<std::ffi::c_char>>();

        init_io(&mut io, name_slice.clone());
        io.uuid[0..uuid_slice.len()].copy_from_slice(&uuid_slice);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_CREATE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev create ioctl failed: {:?}", std::io::Error::last_os_error());

        let (param_str, target_spec, target_vec) =
            create_verity_target_two_loop_devices_sha256(VerityTargetState::Valid);

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.target_count = 1;
        io.flags |= DM_READONLY_FLAG;
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = (std::mem::size_of::<linux_uapi::dm_ioctl>() + target_vec.len()) as u32;

        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(target_vec);

        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_TABLE_LOAD.try_into().unwrap(), io_vec.as_ptr())
        };
        assert!(ret == 0, "dm table load ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut io = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_SUSPEND.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev resume ioctl failed: {:?}", std::io::Error::last_os_error());

        let mut observed_table_statuses: HashSet<(u64, u64, i32, String, Option<String>)> =
            HashSet::new();

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice.clone());
        io.data_start = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32;
        io.data_size = std::mem::size_of::<linux_uapi::dm_ioctl>() as u32
            + 2 * (std::mem::size_of::<linux_uapi::dm_target_spec>() + param_str.len() + 1) as u32;
        io.flags |= DM_STATUS_TABLE_FLAG;
        let mut io_vec = io.as_bytes().to_vec();
        io_vec.extend(vec![
            0;
            2 * (std::mem::size_of::<linux_uapi::dm_target_spec>()
                + param_str.len()
                + 1)
        ]);

        let ret = unsafe {
            libc::ioctl(
                dm_control.as_raw_fd(),
                DM_TABLE_STATUS.try_into().unwrap(),
                io_vec.as_ptr(),
            )
        };
        assert!(ret == 0, "dm table status ioctl failed: {:?}", std::io::Error::last_os_error());

        let (io_struct_bytes, rest) = io_vec.split_at(std::mem::size_of::<linux_uapi::dm_ioctl>());
        let io_struct = linux_uapi::dm_ioctl::read_from_bytes(io_struct_bytes).unwrap();
        let mut next = 0;
        let mut data_size = io_struct.data_size as usize
            - std::cmp::min(
                io_struct.data_size as usize,
                std::mem::size_of::<linux_uapi::dm_ioctl>(),
            );
        while data_size > 0 {
            let target_specs = linux_uapi::dm_target_spec::read_from_bytes(
                &rest[next..next + std::mem::size_of::<linux_uapi::dm_target_spec>()],
            )
            .unwrap();
            let data = if target_specs.next as usize
                > next + std::mem::size_of::<linux_uapi::dm_target_spec>()
            {
                let data_c_str = std::ffi::CStr::from_bytes_until_nul(
                    &rest[next + std::mem::size_of::<linux_uapi::dm_target_spec>()
                        ..target_specs.next as usize],
                )
                .unwrap()
                .to_bytes();
                Some(String::from_utf8(data_c_str.to_vec()).unwrap())
            } else {
                None
            };
            let target_type =
                target_specs.target_type.iter().map(|v| *v as u8).collect::<Vec<u8>>();
            let target_type_c_str = std::ffi::CStr::from_bytes_until_nul(target_type.as_bytes())
                .unwrap()
                .to_bytes()
                .to_vec();
            observed_table_statuses.insert((
                target_specs.sector_start,
                target_specs.length,
                target_specs.status,
                String::from_utf8(target_type_c_str).unwrap(),
                data,
            ));
            if target_specs.next as usize - next > data_size {
                break;
            }
            data_size -= target_specs.next as usize - next;
            next = target_specs.next as usize;
        }
        assert!(observed_table_statuses.contains(&(
            target_spec.sector_start,
            target_spec.length,
            target_spec.status,
            String::from("verity"),
            Some(param_str),
        )));

        let mut io: dm_ioctl = linux_uapi::dm_ioctl { ..Default::default() };
        init_io(&mut io, name_slice);
        let ret = unsafe {
            libc::ioctl(dm_control.as_raw_fd(), DM_DEV_REMOVE.try_into().unwrap(), &mut io)
        };
        assert!(ret == 0, "dm dev remove ioctl failed: {:?}", std::io::Error::last_os_error());
    }
}
