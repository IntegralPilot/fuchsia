// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::CreationMode;
use crate::directory::DirectoryOptions;
use crate::file::FileOptions;
use crate::node::NodeOptions;
use crate::service::ServiceOptions;
use crate::symlink::SymlinkOptions;
use flex_fuchsia_io as fio;
use zx_status::Status;

/// Extends fio::Flags and fio::OpenFlags
pub trait ProtocolsExt: ToFileOptions + ToNodeOptions + Send + Sync + 'static {
    /// True if the directory protocol is allowed.
    fn is_dir_allowed(&self) -> bool;

    /// True if the file protocol is allowed.
    fn is_file_allowed(&self) -> bool;

    /// True if the symlink protocol is allowed.
    fn is_symlink_allowed(&self) -> bool;

    /// The creation mode for the connection.
    fn creation_mode(&self) -> CreationMode;

    /// The rights for the connection.  If None, it means the connection is not for a node based
    /// protocol.  If the connection is supposed to use the same rights as the parent connection,
    /// the rights should have been populated.
    fn rights(&self) -> Option<fio::Operations>;

    /// Convert to directory options.  Returns an error if the request does not permit a directory.
    fn to_directory_options(&self) -> Result<DirectoryOptions, Status>;

    /// Convert to symlink options.  Returns an error if the request does not permit a symlink.
    fn to_symlink_options(&self) -> Result<SymlinkOptions, Status>;

    /// Convert to service options.  Returns an error if the request is not valid for a service.
    fn to_service_options(&self) -> Result<ServiceOptions, Status>;

    /// True if the file should be in append mode.
    fn is_append(&self) -> bool;

    /// True if the file should be truncated.
    fn is_truncate(&self) -> bool;

    /// If creating an object, Whether to create a directory.
    fn create_directory(&self) -> bool;

    /// True if the protocol should be a limited node connection.
    fn is_node(&self) -> bool;

    /// True if creating as an unnamed temporary object.
    fn create_unnamed_temporary_in_directory_path(&self) -> bool;
}

impl ProtocolsExt for fio::OpenFlags {
    fn is_dir_allowed(&self) -> bool {
        !self.contains(fio::OpenFlags::NOT_DIRECTORY)
    }

    fn is_file_allowed(&self) -> bool {
        !self.contains(fio::OpenFlags::DIRECTORY)
    }

    fn is_symlink_allowed(&self) -> bool {
        !self.contains(fio::OpenFlags::DIRECTORY)
    }

    fn creation_mode(&self) -> CreationMode {
        if self.contains(fio::OpenFlags::CREATE) {
            if self.contains(fio::OpenFlags::CREATE_IF_ABSENT) {
                CreationMode::Always
            } else {
                CreationMode::AllowExisting
            }
        } else {
            CreationMode::Never
        }
    }

    fn rights(&self) -> Option<fio::Operations> {
        if self.contains(fio::OpenFlags::CLONE_SAME_RIGHTS) {
            None
        } else {
            let mut rights = fio::Operations::GET_ATTRIBUTES
                | fio::Operations::CONNECT
                | fio::Operations::TRAVERSE;
            if self.contains(fio::OpenFlags::RIGHT_READABLE) {
                rights |= fio::R_STAR_DIR;
            }
            if self.contains(fio::OpenFlags::RIGHT_WRITABLE) {
                rights |= fio::W_STAR_DIR;
            }
            if self.contains(fio::OpenFlags::RIGHT_EXECUTABLE) {
                rights |= fio::X_STAR_DIR;
            }
            Some(rights)
        }
    }

    /// Checks flags provided for a new directory connection.  Returns directory options (cleaning
    /// up some ambiguities) or an error, in case new new connection flags are not permitting the
    /// connection to be opened.
    ///
    /// Changing this function can be dangerous!  Flags operations may have security implications.
    fn to_directory_options(&self) -> Result<DirectoryOptions, Status> {
        assert!(!self.intersects(fio::OpenFlags::NODE_REFERENCE));

        let mut flags = *self;

        if flags.intersects(fio::OpenFlags::DIRECTORY) {
            flags &= !fio::OpenFlags::DIRECTORY;
        }

        if flags.intersects(fio::OpenFlags::NOT_DIRECTORY) {
            return Err(Status::NOT_FILE);
        }

        // Parent connection must check the POSIX flags in `check_child_connection_flags`, so if any
        // are still present, we expand their respective rights and remove any remaining flags.
        if flags.intersects(fio::OpenFlags::POSIX_EXECUTABLE) {
            flags |= fio::OpenFlags::RIGHT_EXECUTABLE;
        }
        if flags.intersects(fio::OpenFlags::POSIX_WRITABLE) {
            flags |= fio::OpenFlags::RIGHT_WRITABLE;
        }
        flags &= !(fio::OpenFlags::POSIX_WRITABLE | fio::OpenFlags::POSIX_EXECUTABLE);

        let allowed_flags = fio::OpenFlags::DESCRIBE
            | fio::OpenFlags::CREATE
            | fio::OpenFlags::CREATE_IF_ABSENT
            | fio::OpenFlags::DIRECTORY
            | fio::OpenFlags::RIGHT_READABLE
            | fio::OpenFlags::RIGHT_WRITABLE
            | fio::OpenFlags::RIGHT_EXECUTABLE;

        let prohibited_flags = fio::OpenFlags::APPEND | fio::OpenFlags::TRUNCATE;

        if flags.intersects(prohibited_flags) {
            return Err(Status::INVALID_ARGS);
        }

        if flags.intersects(!allowed_flags) {
            return Err(Status::NOT_SUPPORTED);
        }

        // Map io1 OpenFlags::RIGHT_* flags to the corresponding set of io2 rights. Using Open1
        // requires GET_ATTRIBUTES, as this was previously an privileged operation.
        let mut rights = fio::Rights::GET_ATTRIBUTES | fio::Rights::TRAVERSE;
        if flags.contains(fio::OpenFlags::RIGHT_READABLE) {
            rights |= fio::R_STAR_DIR;
        }
        if flags.contains(fio::OpenFlags::RIGHT_WRITABLE) {
            rights |= fio::W_STAR_DIR;
        }
        if flags.contains(fio::OpenFlags::RIGHT_EXECUTABLE) {
            rights |= fio::X_STAR_DIR;
        }

        Ok(DirectoryOptions { rights })
    }

    fn to_symlink_options(&self) -> Result<SymlinkOptions, Status> {
        if self.intersects(fio::OpenFlags::DIRECTORY) {
            return Err(Status::NOT_DIR);
        }

        // We allow write and executable access because the client might not know this is a symbolic
        // link and they want to open the target of the link with write or executable rights.
        let optional = fio::OpenFlags::NOT_DIRECTORY
            | fio::OpenFlags::DESCRIBE
            | fio::OpenFlags::RIGHT_WRITABLE
            | fio::OpenFlags::RIGHT_EXECUTABLE;

        if *self & !optional != fio::OpenFlags::RIGHT_READABLE {
            return Err(Status::INVALID_ARGS);
        }

        let mut rights = fio::Operations::GET_ATTRIBUTES;
        if self.contains(fio::OpenFlags::RIGHT_READABLE) {
            rights |= fio::Operations::READ_BYTES;
        }
        if self.contains(fio::OpenFlags::RIGHT_WRITABLE) {
            rights |= fio::Operations::WRITE_BYTES | fio::Operations::UPDATE_ATTRIBUTES;
        }
        if self.contains(fio::OpenFlags::RIGHT_EXECUTABLE) {
            rights |= fio::Operations::EXECUTE;
        }

        Ok(SymlinkOptions { rights })
    }

    fn to_service_options(&self) -> Result<ServiceOptions, Status> {
        if self.intersects(fio::OpenFlags::DIRECTORY) {
            return Err(Status::NOT_DIR);
        }

        if self.intersects(!fio::OpenFlags::DESCRIBE.union(fio::OpenFlags::NOT_DIRECTORY)) {
            return Err(Status::INVALID_ARGS);
        }

        Ok(ServiceOptions)
    }

    fn is_append(&self) -> bool {
        self.contains(fio::OpenFlags::APPEND)
    }

    fn is_truncate(&self) -> bool {
        self.contains(fio::OpenFlags::TRUNCATE)
    }

    fn create_directory(&self) -> bool {
        self.contains(fio::OpenFlags::DIRECTORY)
    }

    fn is_node(&self) -> bool {
        self.contains(fio::OpenFlags::NODE_REFERENCE)
    }

    fn create_unnamed_temporary_in_directory_path(&self) -> bool {
        false
    }
}

impl ProtocolsExt for fio::Flags {
    fn is_dir_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_DIRECTORY)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn is_file_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_FILE)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn is_symlink_allowed(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_SYMLINK)
            || self.intersection(fio::MASK_KNOWN_PROTOCOLS).is_empty()
    }

    fn creation_mode(&self) -> CreationMode {
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        {
            if self.contains(fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY) {
                if self.contains(fio::Flags::FLAG_MUST_CREATE) {
                    return CreationMode::UnlinkableUnnamedTemporary;
                }
                return CreationMode::UnnamedTemporary;
            }
        }
        if self.contains(fio::Flags::FLAG_MUST_CREATE) {
            CreationMode::Always
        } else if self.contains(fio::Flags::FLAG_MAYBE_CREATE) {
            CreationMode::AllowExisting
        } else {
            CreationMode::Never
        }
    }

    fn rights(&self) -> Option<fio::Operations> {
        Some(flags_to_rights(self))
    }

    fn to_directory_options(&self) -> Result<DirectoryOptions, Status> {
        // Verify protocols.
        if !self.is_dir_allowed() {
            return Err(if self.is_file_allowed() { Status::NOT_FILE } else { Status::WRONG_TYPE });
        }

        // Expand the POSIX flags to their respective rights. This is done with the assumption that
        // the POSIX flags would have been validated prior to calling this. E.g. in the vfs
        // connection later.
        let mut updated_flags = *self;
        if updated_flags.contains(fio::Flags::PERM_INHERIT_WRITE) {
            updated_flags |=
                fio::Flags::from_bits_truncate(fio::INHERITED_WRITE_PERMISSIONS.bits());
        }
        if updated_flags.contains(fio::Flags::PERM_INHERIT_EXECUTE) {
            updated_flags |= fio::Flags::PERM_EXECUTE;
        }

        // Verify that there are no file-related flags.
        if updated_flags.intersects(fio::Flags::FILE_APPEND | fio::Flags::FILE_TRUNCATE) {
            return Err(Status::INVALID_ARGS);
        }

        Ok(DirectoryOptions { rights: flags_to_rights(&updated_flags) })
    }

    fn to_symlink_options(&self) -> Result<SymlinkOptions, Status> {
        if !self.is_symlink_allowed() {
            return Err(Status::WRONG_TYPE);
        }

        // If is_symlink_allowed() returned true, there must be rights.
        let rights = self.rights().unwrap();
        if !rights.contains(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::INVALID_ARGS);
        }
        Ok(SymlinkOptions { rights })
    }

    fn to_service_options(&self) -> Result<ServiceOptions, Status> {
        // This should not be called if fio::Flags::PROTOCOL_NODE was set (`to_node_options` would
        // be called instead).
        assert!(!self.contains(fio::Flags::PROTOCOL_NODE));
        if !self
            .intersection(fio::MASK_KNOWN_PROTOCOLS)
            .difference(fio::Flags::PROTOCOL_SERVICE)
            .is_empty()
        {
            return if self.is_dir_allowed() {
                Err(Status::NOT_DIR)
            } else if self.is_file_allowed() {
                Err(Status::NOT_FILE)
            } else {
                Err(Status::WRONG_TYPE)
            };
        }

        if !self.difference(fio::Flags::PROTOCOL_SERVICE).is_empty() {
            return Err(Status::INVALID_ARGS);
        }

        Ok(ServiceOptions)
    }

    fn is_append(&self) -> bool {
        self.contains(fio::Flags::FILE_APPEND)
    }

    fn is_truncate(&self) -> bool {
        self.contains(fio::Flags::FILE_TRUNCATE)
    }

    fn create_directory(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_DIRECTORY)
    }

    fn is_node(&self) -> bool {
        self.contains(fio::Flags::PROTOCOL_NODE)
    }

    fn create_unnamed_temporary_in_directory_path(&self) -> bool {
        self.creation_mode() == CreationMode::UnnamedTemporary
            || self.creation_mode() == CreationMode::UnlinkableUnnamedTemporary
    }
}

pub trait ToFileOptions: Send + 'static {
    fn to_file_options(&self) -> Result<FileOptions, Status>;
}

impl ToFileOptions for fio::OpenFlags {
    fn to_file_options(&self) -> Result<FileOptions, Status> {
        assert!(!self.intersects(fio::OpenFlags::NODE_REFERENCE));

        if self.contains(fio::OpenFlags::DIRECTORY) {
            return Err(Status::NOT_DIR);
        }

        // Verify allowed operations/flags this node supports.
        let flags_without_rights = self.difference(
            fio::OpenFlags::RIGHT_READABLE
                | fio::OpenFlags::RIGHT_WRITABLE
                | fio::OpenFlags::RIGHT_EXECUTABLE,
        );
        const ALLOWED_FLAGS: fio::OpenFlags = fio::OpenFlags::DESCRIBE
            .union(fio::OpenFlags::CREATE)
            .union(fio::OpenFlags::CREATE_IF_ABSENT)
            .union(fio::OpenFlags::APPEND)
            .union(fio::OpenFlags::TRUNCATE)
            .union(fio::OpenFlags::POSIX_WRITABLE)
            .union(fio::OpenFlags::POSIX_EXECUTABLE)
            .union(fio::OpenFlags::NOT_DIRECTORY);
        if flags_without_rights.intersects(!ALLOWED_FLAGS) {
            return Err(Status::NOT_SUPPORTED);
        }

        // Disallow invalid flag combinations.
        let mut prohibited_flags = fio::OpenFlags::empty();
        if !self.intersects(fio::OpenFlags::RIGHT_WRITABLE) {
            prohibited_flags |= fio::OpenFlags::TRUNCATE
        }
        if self.intersects(prohibited_flags) {
            return Err(Status::INVALID_ARGS);
        }

        Ok(FileOptions {
            rights: {
                let mut rights = fio::Operations::GET_ATTRIBUTES;
                if self.contains(fio::OpenFlags::RIGHT_READABLE) {
                    rights |= fio::Operations::READ_BYTES;
                }
                if self.contains(fio::OpenFlags::RIGHT_WRITABLE) {
                    rights |= fio::Operations::WRITE_BYTES | fio::Operations::UPDATE_ATTRIBUTES;
                }
                if self.contains(fio::OpenFlags::RIGHT_EXECUTABLE) {
                    rights |= fio::Operations::EXECUTE;
                }
                rights
            },
            is_append: self.contains(fio::OpenFlags::APPEND),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            is_linkable: true,
        })
    }
}

impl ToFileOptions for fio::Flags {
    fn to_file_options(&self) -> Result<FileOptions, Status> {
        // Verify protocols.
        if !self.is_file_allowed() {
            if self.is_dir_allowed() && !self.is_symlink_allowed() {
                return Err(Status::NOT_DIR);
            } else {
                return Err(Status::WRONG_TYPE);
            }
        }

        // Verify prohibited flags and disallow invalid flag combinations.
        if self.contains(fio::Flags::FILE_TRUNCATE) && !self.contains(fio::Flags::PERM_WRITE_BYTES)
        {
            return Err(Status::INVALID_ARGS);
        }

        // Used to remove any non-file flags.
        const ALLOWED_RIGHTS: fio::Operations = fio::Operations::empty()
            .union(fio::Operations::GET_ATTRIBUTES)
            .union(fio::Operations::READ_BYTES)
            .union(fio::Operations::WRITE_BYTES)
            .union(fio::Operations::UPDATE_ATTRIBUTES)
            .union(fio::Operations::EXECUTE);

        Ok(FileOptions {
            rights: flags_to_rights(self).intersection(ALLOWED_RIGHTS),
            is_append: self.contains(fio::Flags::FILE_APPEND),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            is_linkable: !self.contains(
                fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY | fio::FLAG_TEMPORARY_AS_NOT_LINKABLE,
            ),
        })
    }
}

impl ToFileOptions for FileOptions {
    fn to_file_options(&self) -> Result<FileOptions, Status> {
        Ok(*self)
    }
}

pub trait ToNodeOptions: Send + 'static {
    fn to_node_options(&self, dirent_type: fio::DirentType) -> Result<NodeOptions, Status>;
}

impl ToNodeOptions for fio::OpenFlags {
    fn to_node_options(&self, dirent_type: fio::DirentType) -> Result<NodeOptions, Status> {
        // Strictly, we shouldn't allow rights to be specified with NODE_REFERENCE, but there's a
        // CTS pkgdir test that asserts these flags work and fixing that is painful so we preserve
        // old behaviour (which permitted these flags).
        let allowed_rights =
            fio::OPEN_RIGHTS | fio::OpenFlags::POSIX_WRITABLE | fio::OpenFlags::POSIX_EXECUTABLE;
        if self.intersects(!(fio::OPEN_FLAGS_ALLOWED_WITH_NODE_REFERENCE | allowed_rights)) {
            Err(Status::INVALID_ARGS)
        } else if self.contains(fio::OpenFlags::DIRECTORY)
            && dirent_type != fio::DirentType::Directory
        {
            Err(Status::NOT_DIR)
        } else {
            Ok(NodeOptions { rights: fio::Operations::GET_ATTRIBUTES })
        }
    }
}

impl ToNodeOptions for fio::Flags {
    fn to_node_options(&self, dirent_type: fio::DirentType) -> Result<NodeOptions, Status> {
        // Strictly, we shouldn't allow rights to be specified with PROTOCOL_NODE, but there's a
        // CTS pkgdir test that asserts these flags work and fixing that is painful so we preserve
        // old behaviour (which permitted these flags).
        const ALLOWED_FLAGS: fio::Flags = fio::Flags::FLAG_SEND_REPRESENTATION
            .union(fio::MASK_KNOWN_PERMISSIONS)
            .union(fio::MASK_KNOWN_PROTOCOLS);

        if self.intersects(!ALLOWED_FLAGS) {
            return Err(Status::INVALID_ARGS);
        }

        // If other `PROTOCOL_*` were were specified along with `PROTOCOL_NODE`, verify that the
        // target node supports it.
        if self.intersects(fio::MASK_KNOWN_PROTOCOLS.difference(fio::Flags::PROTOCOL_NODE)) {
            if dirent_type == fio::DirentType::Directory {
                if !self.intersects(fio::Flags::PROTOCOL_DIRECTORY) {
                    if self.intersects(fio::Flags::PROTOCOL_FILE) {
                        return Err(Status::NOT_FILE);
                    } else {
                        return Err(Status::WRONG_TYPE);
                    }
                }
            } else if dirent_type == fio::DirentType::File {
                if !self.intersects(fio::Flags::PROTOCOL_FILE) {
                    if self.intersects(fio::Flags::PROTOCOL_DIRECTORY) {
                        return Err(Status::NOT_DIR);
                    } else {
                        return Err(Status::WRONG_TYPE);
                    }
                }
            } else if dirent_type == fio::DirentType::Symlink {
                if !self.intersects(fio::Flags::PROTOCOL_SYMLINK) {
                    return Err(Status::WRONG_TYPE);
                }
            }
        }

        Ok(NodeOptions {
            rights: flags_to_rights(self).intersection(fio::Operations::GET_ATTRIBUTES),
        })
    }
}

impl ToNodeOptions for NodeOptions {
    fn to_node_options(&self, _dirent_type: fio::DirentType) -> Result<NodeOptions, Status> {
        Ok(*self)
    }
}

fn flags_to_rights(flags: &fio::Flags) -> fio::Rights {
    fio::Rights::from_bits_truncate(flags.bits())
}
