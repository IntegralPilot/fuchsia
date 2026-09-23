// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryAccessorExt;
use crate::task::CurrentTask;
use mundane::hash::{Digest, Hasher, Sha256, Sha512};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use starnix_logging::track_stub;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{
    FS_VERITY_HASH_ALG_SHA256, FS_VERITY_HASH_ALG_SHA512, errno, error, fsverity_descriptor,
    fsverity_enable_arg, fsverity_read_metadata_arg,
};
use zerocopy::IntoBytes;

#[derive(Copy, Clone, Debug, Eq, FromPrimitive, PartialEq)]
enum HashAlgorithm {
    SHA256 = FS_VERITY_HASH_ALG_SHA256 as isize,
    SHA512 = FS_VERITY_HASH_ALG_SHA512 as isize,
}

/// Create a new fsverity_descriptor from the ioctl `enable_verity`.
/// Copies salt and signature from the given user tasks memory space.
fn fsverity_descriptor_from_enable_arg(
    current_task: &CurrentTask,
    filesystem_block_size: u32,
    src: &fsverity_enable_arg,
) -> Result<fsverity_descriptor, Errno> {
    if src.version != 1 {
        return error!(EINVAL);
    }
    // block_size must be a power of 2 in [1024,min(page_size, filesystem_block_size)].
    if src.block_size.count_ones() != 1
        || src.block_size < 1024
        || src.block_size > std::cmp::min(4096, filesystem_block_size)
    {
        return error!(EINVAL);
    }
    if src.hash_algorithm != FS_VERITY_HASH_ALG_SHA256
        && src.hash_algorithm != FS_VERITY_HASH_ALG_SHA512
    {
        return error!(ENOTSUP);
    }
    if src.salt_size > 32 {
        return error!(EINVAL);
    }
    if src.sig_size > 0 {
        // TODO(https://fxbug.dev/302620572) Under linux, signatures are supported up to 16128 bytes.
        // We don't support these currently.
        track_stub!(TODO("https://fxbug.dev/302620572"), "fsverity_enable signature");
        return error!(ENOTSUP);
    }
    let salt =
        current_task.read_memory_to_vec(UserAddress::from(src.salt_ptr), src.salt_size as usize)?;
    let mut desc = fsverity_descriptor {
        version: 1,
        hash_algorithm: HashAlgorithm::from_u32(src.hash_algorithm).ok_or_else(|| errno!(EINVAL))?
            as u8,
        salt_size: salt.len() as u8,
        log_blocksize: (u32::BITS - 1 - src.block_size.leading_zeros()) as u8,
        ..Default::default()
    };
    desc.salt[..src.salt_size as usize].clone_from_slice(salt.as_slice());
    Ok(desc)
}

/// An fsverity 'measurement' is a hash of a descriptor that contains the root hash.
/// This ensures that the result captures file size, hash type and other relevant parameters.
fn fsverity_measurement(descriptor: &fsverity_descriptor) -> Result<Box<[u8]>, Errno> {
    match descriptor.hash_algorithm.into() {
        FS_VERITY_HASH_ALG_SHA256 => {
            let mut hasher = Sha256::default();
            if descriptor.salt_size > 0 {
                hasher.update(&descriptor.salt[..descriptor.salt_size as usize]);
            }
            hasher.update(descriptor.as_bytes());
            Ok(Box::new(hasher.finish().bytes()))
        }
        FS_VERITY_HASH_ALG_SHA512 => {
            let mut hasher = Sha512::default();
            if descriptor.salt_size > 0 {
                hasher.update(&descriptor.salt[..descriptor.salt_size as usize]);
            }
            hasher.update(descriptor.as_bytes());
            Ok(Box::new(hasher.finish().bytes()))
        }
        _ => error!(EINVAL),
    }
}

#[derive(Debug, FromPrimitive)]
enum MetadataType {
    MerkleTree = 1,
    Descriptor = 2,
    Signature = 3,
}

/// Per-file fsverity state stored in FsNode.
#[derive(Debug, Default)]
pub enum FsVerityState {
    /// This file does not use fsverity.
    #[default]
    None,
    /// The state when building the merkle-tree for this file.
    ///
    /// When in the building state, the file is not writable and further attempts to
    /// ENABLE_VERITY will fail with EBUSY. It is possible for merkle-tree building to fail
    /// (e.g. if the file is too large) but it is not possible to cancel this state manually.
    Building,
    /// This file uses fsverity.
    ///
    /// Files in this mode can never be opened as writable.
    /// There is no way to disable fsverity once enabled outside of deleting the file.
    FsVerity,
}

impl FsVerityState {
    /// Returns an appropriate error if state is not writable.
    pub fn check_writable(&self) -> Result<(), Errno> {
        match self {
            FsVerityState::None => Ok(()),
            FsVerityState::Building => error!(ETXTBSY),
            FsVerityState::FsVerity => error!(EACCES),
        }
    }
}

pub mod ioctl {

    use crate::mm::{MemoryAccessor, MemoryAccessorExt};
    use crate::task::CurrentTask;
    use crate::vfs::fsverity::{
        FsVerityState, HashAlgorithm, MetadataType, fsverity_descriptor_from_enable_arg,
        fsverity_enable_arg, fsverity_measurement, fsverity_read_metadata_arg,
    };
    use crate::vfs::{FileObject, FileWriteGuardMode};
    use num_traits::FromPrimitive;

    use starnix_syscalls::{SUCCESS, SyscallResult};
    use starnix_uapi::errors::Errno;
    use starnix_uapi::user_address::{UserAddress, UserRef};
    use starnix_uapi::{errno, error, uapi};
    use zerocopy::IntoBytes;

    /// ioctl handler for FS_IOC_ENABLE_VERITY.
    pub fn enable(
        task: &CurrentTask,
        arg: UserAddress,
        file: &FileObject,
    ) -> Result<SyscallResult, Errno> {
        if file.can_write() {
            return error!(ETXTBSY);
        }
        let block_size = file.name.entry.node.fs().statfs(task)?.f_bsize as u32;
        // Nb: Lock order is important here.
        let args: fsverity_enable_arg = task.read_object(arg.into())?;
        let mut descriptor = fsverity_descriptor_from_enable_arg(task, block_size, &args)?;
        descriptor.data_size = file.node().fetch_and_refresh_info(task)?.size as u64;
        // The "Exec" writeguard mode means 'no writers'.
        let _mapping = file.to_mapping(Some(FileWriteGuardMode::ExecMapping))?;
        {
            let mut fsverity = file.node().fsverity.lock();
            match *fsverity {
                FsVerityState::Building => return error!(EBUSY),
                FsVerityState::FsVerity => return error!(EEXIST),
                FsVerityState::None => *fsverity = FsVerityState::Building,
            }
        }
        let result = file.node().enable_fsverity(task, &descriptor);
        *file.node().fsverity.lock() =
            if result.is_ok() { FsVerityState::FsVerity } else { FsVerityState::None };
        result.map(|()| SUCCESS)
    }

    /// ioctl handler for FS_IOC_MEASURE_VERITY.
    pub fn measure(
        task: &CurrentTask,
        arg: UserAddress,
        file: &FileObject,
    ) -> Result<SyscallResult, Errno> {
        let header_ref = UserRef::<uapi::fsverity_digest>::new(arg);
        let digest_addr = header_ref.next()?.addr();
        let header = task.read_object(header_ref.clone())?;
        match &*file.node().fsverity.lock() {
            FsVerityState::FsVerity => {
                let block_size = file.name.entry.node.fs().statfs(task)?.f_bsize as u32;
                if !block_size.is_power_of_two() {
                    return error!(EINVAL);
                }
                let descriptor =
                    file.node().ops().get_fsverity_descriptor(block_size.ilog2() as u8)?;
                let digest_algorithm = HashAlgorithm::from_u8(descriptor.hash_algorithm)
                    .ok_or_else(|| errno!(EINVAL))?;
                let required_size = match digest_algorithm {
                    HashAlgorithm::SHA256 => 32,
                    HashAlgorithm::SHA512 => 64,
                };
                if (header.digest_size as usize) < required_size {
                    return error!(EOVERFLOW);
                }
                let output_header = uapi::fsverity_digest {
                    digest_algorithm: descriptor.hash_algorithm as u16,
                    digest_size: required_size as u16,
                    ..Default::default()
                };
                task.write_object(header_ref, &output_header)?;
                task.write_memory(digest_addr, &fsverity_measurement(&descriptor)?)?;
                Ok(SUCCESS)
            }
            _ => error!(ENODATA),
        }
    }

    /// ioctl handler for FS_IOC_READ_VERITY_METADATA.
    pub fn read_metadata(
        task: &CurrentTask,
        arg: UserAddress,
        file: &FileObject,
    ) -> Result<SyscallResult, Errno> {
        let arg: fsverity_read_metadata_arg = task.read_object(arg.into())?;
        match &*file.node().fsverity.lock() {
            FsVerityState::FsVerity => {
                match MetadataType::from_u64(arg.metadata_type).ok_or_else(|| errno!(EINVAL))? {
                    MetadataType::MerkleTree => {
                        error!(EOPNOTSUPP)
                    }
                    MetadataType::Descriptor => {
                        // TODO(b/314182708): Remove hardcoding of blocksize
                        let descriptor = file.node().ops().get_fsverity_descriptor(12)?;
                        task.write_memory(
                            UserAddress::from(arg.buf_ptr).into(),
                            &descriptor.as_bytes()
                                [arg.offset as usize..(arg.offset + arg.length) as usize],
                        )?;
                        Ok(SUCCESS)
                    }
                    MetadataType::Signature => {
                        error!(EOPNOTSUPP)
                    }
                }
            }
            _ => error!(ENODATA),
        }
    }
}
