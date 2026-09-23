// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common utilities for implementing file nodes and connections.

use crate::file::FileOptions;
use flex_fuchsia_io as fio;
use zx_status::Status;

/// Validate that the requested flags for a new connection are valid. This includes permission
/// handling, only allowing certain operations.
///
/// Changing this function can be dangerous! There are security implications.
pub fn new_connection_validate_options(
    options: &FileOptions,
    readable: bool,
    writable: bool,
    executable: bool,
) -> Result<(), Status> {
    // Nodes supporting both W+X rights are not supported.
    debug_assert!(!(writable && executable));

    // Validate permissions.
    if !readable && options.rights.contains(fio::Operations::READ_BYTES) {
        return Err(Status::ACCESS_DENIED);
    }
    if !writable && options.rights.contains(fio::Operations::WRITE_BYTES) {
        return Err(Status::ACCESS_DENIED);
    }
    if !executable && options.rights.contains(fio::Operations::EXECUTE) {
        return Err(Status::ACCESS_DENIED);
    }

    Ok(())
}

/// Converts the set of validated VMO flags to their respective zx::Rights.
#[cfg(target_os = "fuchsia")]
pub fn vmo_flags_to_rights(vmo_flags: fio::VmoFlags) -> fidl::Rights {
    // Map VMO flags to their respective rights.
    let mut rights = fidl::Rights::NONE;
    if vmo_flags.contains(fio::VmoFlags::READ) {
        rights |= fidl::Rights::READ;
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE) {
        rights |= fidl::Rights::WRITE;
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE) {
        rights |= fidl::Rights::EXECUTE;
    }

    rights
}

/// Validate flags passed to `get_backing_memory` against the underlying connection flags.
/// Returns Ok() if the flags were validated, and an Error(Status) otherwise.
///
/// Changing this function can be dangerous! Flags operations may have security implications.
#[cfg(target_os = "fuchsia")]
pub fn get_backing_memory_validate_flags(
    vmo_flags: fio::VmoFlags,
    options: FileOptions,
) -> Result<(), Status> {
    // Disallow inconsistent flag combination.
    if vmo_flags.contains(fio::VmoFlags::PRIVATE_CLONE)
        && vmo_flags.contains(fio::VmoFlags::SHARED_BUFFER)
    {
        return Err(Status::INVALID_ARGS);
    }

    // Ensure the requested rights in vmo_flags do not exceed those of the underlying connection.
    if vmo_flags.contains(fio::VmoFlags::READ)
        && !options.rights.intersects(fio::Operations::READ_BYTES)
    {
        return Err(Status::ACCESS_DENIED);
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE)
        && !options.rights.intersects(fio::Operations::WRITE_BYTES)
    {
        return Err(Status::ACCESS_DENIED);
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE)
        && !options.rights.intersects(fio::Operations::EXECUTE)
    {
        return Err(Status::ACCESS_DENIED);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::new_connection_validate_options;
    use crate::file::FileOptions;
    use crate::protocols::ToFileOptions;
    use crate::test_utils::build_flag_combinations;

    use assert_matches::assert_matches;
    use flex_fuchsia_io as fio;
    use zx_status::Status;

    fn options_to_rights(options: FileOptions) -> (bool, bool, bool) {
        return (
            options.rights.intersects(fio::Operations::READ_BYTES),
            options.rights.intersects(fio::Operations::WRITE_BYTES),
            options.rights.intersects(fio::Operations::EXECUTE),
        );
    }

    fn ncvf(
        flags: impl ToFileOptions,
        readable: bool,
        writable: bool,
        executable: bool,
    ) -> Result<FileOptions, Status> {
        let options = flags.to_file_options()?;
        new_connection_validate_options(&options, readable, writable, executable)?;
        Ok(options)
    }

    #[test]
    fn new_connection_validate_flags_create() {
        for open_flags in build_flag_combinations(
            fio::Flags::FLAG_MAYBE_CREATE,
            fio::Flags::PERM_READ_BYTES
                | fio::Flags::PERM_WRITE_BYTES
                | fio::Flags::FLAG_MUST_CREATE,
        ) {
            let options = open_flags.to_file_options().unwrap();
            let (readable, writable, executable) = options_to_rights(options);
            assert_matches!(ncvf(options, readable, writable, executable), Ok(_));
        }
    }

    #[test]
    fn new_connection_validate_flags_truncate() {
        assert_matches!(
            ncvf(fio::Flags::PERM_READ_BYTES | fio::Flags::FILE_TRUNCATE, true, true, false),
            Err(Status::INVALID_ARGS)
        );
        assert_matches!(
            ncvf(fio::Flags::PERM_WRITE_BYTES | fio::Flags::FILE_TRUNCATE, true, true, false),
            Ok(_)
        );
        assert_matches!(
            ncvf(fio::Flags::PERM_READ_BYTES | fio::Flags::FILE_TRUNCATE, true, false, false),
            Err(Status::INVALID_ARGS)
        );
    }

    #[test]
    fn new_connection_validate_flags_append() {
        assert_matches!(
            ncvf(fio::Flags::PERM_READ_BYTES | fio::Flags::FILE_APPEND, true, false, false),
            Ok(_)
        );
        assert_matches!(
            ncvf(fio::Flags::PERM_READ_BYTES | fio::Flags::FILE_APPEND, true, true, false),
            Ok(_)
        );
        assert_matches!(
            ncvf(fio::Flags::PERM_READ_BYTES | fio::Flags::FILE_APPEND, true, true, false),
            Ok(_)
        );
    }

    #[test]
    fn new_connection_validate_flags_open_rights() {
        for open_flags in build_flag_combinations(
            fio::Flags::empty(),
            fio::Flags::PERM_READ_BYTES | fio::Flags::PERM_READ_BYTES | fio::Flags::PERM_EXECUTE,
        ) {
            let options = open_flags.to_file_options().unwrap();
            let (readable, writable, executable) = options_to_rights(options);

            // Ensure all combinations are valid except when both writable and executable are set,
            // as this combination is disallowed.
            if !(writable && executable) {
                assert_matches!(ncvf(options, readable, writable, executable), Ok(_));
            }

            // Ensure we report ACCESS_DENIED if open_flags exceeds the supported connection rights.
            if readable && !(writable && executable) {
                assert_eq!(ncvf(options, false, writable, executable), Err(Status::ACCESS_DENIED));
            }
            if writable {
                assert_eq!(ncvf(options, readable, false, executable), Err(Status::ACCESS_DENIED));
            }
            if executable {
                assert_eq!(ncvf(options, readable, writable, false), Err(Status::ACCESS_DENIED));
            }
        }
    }

    #[cfg(target_os = "fuchsia")]
    mod vmo_tests {
        use super::super::{get_backing_memory_validate_flags, vmo_flags_to_rights};
        use super::*;

        fn rights_to_vmo_flags(readable: bool, writable: bool, executable: bool) -> fio::VmoFlags {
            return if readable { fio::VmoFlags::READ } else { fio::VmoFlags::empty() }
                | if writable { fio::VmoFlags::WRITE } else { fio::VmoFlags::empty() }
                | if executable { fio::VmoFlags::EXECUTE } else { fio::VmoFlags::empty() };
        }

        /// Validates that the passed VMO flags are correctly mapped to their respective Rights.
        #[test]
        fn test_vmo_flags_to_rights() {
            for vmo_flags in build_flag_combinations(
                fio::VmoFlags::empty(),
                fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::EXECUTE,
            ) {
                let rights: fidl::Rights = vmo_flags_to_rights(vmo_flags);
                assert_eq!(
                    vmo_flags.contains(fio::VmoFlags::READ),
                    rights.contains(fidl::Rights::READ)
                );
                assert_eq!(
                    vmo_flags.contains(fio::VmoFlags::WRITE),
                    rights.contains(fidl::Rights::WRITE)
                );
                assert_eq!(
                    vmo_flags.contains(fio::VmoFlags::EXECUTE),
                    rights.contains(fidl::Rights::EXECUTE)
                );
            }
        }

        #[test]
        fn get_backing_memory_validate_flags_invalid() {
            // Cannot specify both PRIVATE and EXACT at the same time, since they conflict.
            assert_eq!(
                get_backing_memory_validate_flags(
                    fio::VmoFlags::PRIVATE_CLONE | fio::VmoFlags::SHARED_BUFFER,
                    fio::Flags::empty().to_file_options().unwrap()
                ),
                Err(Status::INVALID_ARGS)
            );
        }

        /// Ensure that the check passes if we request the same or less rights
        /// than the connection has.
        #[test]
        fn get_backing_memory_validate_flags_less_rights() {
            for open_flags in build_flag_combinations(
                fio::Flags::empty(),
                fio::Flags::PERM_READ_BYTES
                    | fio::Flags::PERM_WRITE_BYTES
                    | fio::Flags::PERM_EXECUTE,
            ) {
                let options = open_flags.to_file_options().unwrap();
                let (readable, writable, executable) = options_to_rights(options);
                let vmo_flags = rights_to_vmo_flags(readable, writable, executable);

                // Ensure that we can open the VMO with the same rights as the connection.
                get_backing_memory_validate_flags(vmo_flags, options)
                    .expect("Failed to validate flags");

                // Ensure that we can also open the VMO with *less* rights than the connection has.
                if readable {
                    let vmo_flags = rights_to_vmo_flags(false, writable, executable);
                    get_backing_memory_validate_flags(vmo_flags, options)
                        .expect("Failed to validate flags");
                }
                if writable {
                    let vmo_flags = rights_to_vmo_flags(readable, false, executable);
                    get_backing_memory_validate_flags(vmo_flags, options)
                        .expect("Failed to validate flags");
                }
                if executable {
                    let vmo_flags = rights_to_vmo_flags(readable, writable, false);
                    get_backing_memory_validate_flags(vmo_flags, options)
                        .expect("Failed to validate flags");
                }
            }
        }

        /// Ensure that vmo_flags cannot exceed rights of connection_flags.
        #[test]
        fn get_backing_memory_validate_flags_more_rights() {
            for open_flags in build_flag_combinations(
                fio::Flags::empty(),
                fio::Flags::PERM_READ_BYTES
                    | fio::Flags::PERM_WRITE_BYTES
                    | fio::Flags::PERM_EXECUTE,
            ) {
                let options = open_flags.to_file_options().unwrap();
                // Ensure we cannot return a VMO with more rights than the connection itself has.
                let (readable, writable, executable) = options_to_rights(options);
                if !readable {
                    let vmo_flags = rights_to_vmo_flags(true, writable, executable);
                    assert_eq!(
                        get_backing_memory_validate_flags(vmo_flags, options),
                        Err(Status::ACCESS_DENIED)
                    );
                }
                if !writable {
                    let vmo_flags = rights_to_vmo_flags(readable, true, false);
                    assert_eq!(
                        get_backing_memory_validate_flags(vmo_flags, options),
                        Err(Status::ACCESS_DENIED)
                    );
                }
                if !executable {
                    let vmo_flags = rights_to_vmo_flags(readable, false, true);
                    assert_eq!(
                        get_backing_memory_validate_flags(vmo_flags, options),
                        Err(Status::ACCESS_DENIED)
                    );
                }
            }
        }
    }
}
