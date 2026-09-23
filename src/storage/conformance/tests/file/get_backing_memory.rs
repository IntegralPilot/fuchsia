// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn file_get_readable_memory_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_backing_memory {
        return;
    }

    for file_flags in harness.file_rights.combinations_containing(fio::Rights::READ_BYTES) {
        // Should be able to get a readable VMO in default, exact, and private sharing modes.
        for sharing_mode in
            [fio::VmoFlags::empty(), fio::VmoFlags::SHARED_BUFFER, fio::VmoFlags::PRIVATE_CLONE]
        {
            let file = file(TEST_FILE, TEST_FILE_CONTENTS.to_owned());
            let (vmo, _) = create_file_and_get_backing_memory(
                file,
                &harness,
                file_flags,
                fio::VmoFlags::READ | sharing_mode,
            )
            .await
            .expect("Failed to create file and obtain VMO");

            // Ensure that the returned VMO's rights are consistent with the expected flags.
            validate_vmo_rights(&vmo, fio::VmoFlags::READ);

            let size = vmo.get_content_size().expect("Failed to get vmo content size");

            // Check contents of buffer.
            let mut data = vec![0; size as usize];
            let () = vmo.read(&mut data, 0).expect("VMO read failed");
            assert_eq!(&data, TEST_FILE_CONTENTS);
        }
    }
}

#[fuchsia::test]
async fn file_get_readable_memory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_backing_memory {
        return;
    }

    for file_flags in harness.file_rights.combinations_without(fio::Rights::READ_BYTES) {
        let file = file(TEST_FILE, TEST_FILE_CONTENTS.to_owned());
        assert_eq!(
            create_file_and_get_backing_memory(file, &harness, file_flags, fio::VmoFlags::READ)
                .await
                .expect_err("Error was expected"),
            zx::Status::ACCESS_DENIED
        );
    }
}

#[fuchsia::test]
async fn file_get_writable_memory_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_backing_memory {
        return;
    }
    // Writable VMOs currently require private sharing mode.
    const VMO_FLAGS: fio::VmoFlags =
        fio::VmoFlags::empty().union(fio::VmoFlags::WRITE).union(fio::VmoFlags::PRIVATE_CLONE);

    for file_flags in harness.file_rights.combinations_containing(fio::Rights::WRITE_BYTES) {
        let file = file(TEST_FILE, TEST_FILE_CONTENTS.to_owned());
        let (vmo, _) = create_file_and_get_backing_memory(file, &harness, file_flags, VMO_FLAGS)
            .await
            .expect("Failed to create file and obtain VMO");

        // Ensure that the returned VMO's rights are consistent with the expected flags.
        validate_vmo_rights(&vmo, VMO_FLAGS);

        // Ensure that we can actually write to the VMO.
        let () = vmo.write("bbbbb".as_bytes(), 0).expect("vmo write failed");
    }
}

#[fuchsia::test]
async fn file_get_writable_memory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_backing_memory {
        return;
    }
    const VMO_FLAGS: fio::VmoFlags =
        fio::VmoFlags::empty().union(fio::VmoFlags::WRITE).union(fio::VmoFlags::PRIVATE_CLONE);

    for file_flags in harness.file_rights.combinations_without(fio::Rights::WRITE_BYTES) {
        let file = file(TEST_FILE, TEST_FILE_CONTENTS.to_owned());
        assert_eq!(
            create_file_and_get_backing_memory(file, &harness, file_flags, VMO_FLAGS)
                .await
                .expect_err("Error was expected"),
            zx::Status::ACCESS_DENIED
        );
    }
}

#[fuchsia::test]
async fn file_get_executable_memory_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_executable_file {
        return;
    }

    // We should be able to get an executable VMO in default, exact, and private sharing modes. Note
    // that the fuchsia.io interface requires the connection to have OPEN_RIGHT_READABLE in addition
    // to OPEN_RIGHT_EXECUTABLE if passing VmoFlags::EXECUTE to the GetBackingMemory method.
    for sharing_mode in
        [fio::VmoFlags::empty(), fio::VmoFlags::SHARED_BUFFER, fio::VmoFlags::PRIVATE_CLONE]
    {
        let file = executable_file(TEST_FILE);
        let vmo_flags = fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | sharing_mode;
        let (vmo, _) = create_file_and_get_backing_memory(
            file,
            &harness,
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
            vmo_flags,
        )
        .await
        .expect("Failed to create file and obtain VMO");
        // Ensure that the returned VMO's rights are consistent with the expected flags.
        validate_vmo_rights(&vmo, vmo_flags);
    }
}

#[fuchsia::test]
async fn file_get_executable_memory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_executable_file {
        return;
    }
    // We should fail to get the backing memory if the connection lacks execute rights.
    for file_flags in harness.executable_file_rights.combinations_without(fio::Rights::EXECUTE) {
        let file = executable_file(TEST_FILE);
        assert_eq!(
            create_file_and_get_backing_memory(file, &harness, file_flags, fio::VmoFlags::EXECUTE)
                .await
                .expect_err("Error was expected"),
            zx::Status::ACCESS_DENIED
        );
    }
}

// Ensure that passing VmoFlags::SHARED_BUFFER to GetBackingMemory returns a buffer that's
// shared with the underlying file.
#[fuchsia::test]
async fn file_get_backing_memory_shared_buffer() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_get_backing_memory || !harness.config.supports_mutable_file {
        return;
    }

    let file = file(TEST_FILE, TEST_FILE_CONTENTS.to_owned());
    let (vmo, (_, vmo_file)) = create_file_and_get_backing_memory(
        file,
        &harness,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
        fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER,
    )
    .await
    .expect("Failed to create file and obtain VMO");

    // Write to the file and it should show up in the VMO.
    fuchsia_fs::file::write(&vmo_file, "foo").await.expect("write failed");

    assert_eq!(&vmo.read_to_vec::<u8>(0, 3).expect("read_to_vec failed"), b"foo");
}
