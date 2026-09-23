// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use bitflags::bitflags;
use dm_verity::{
    DmVerityError, DmVerityTargetOptionalParams, DmVerityTargetParams, HashAlgorithm,
    MerkleVerifier,
};
use linux_uapi::DM_UUID_LEN;
use mundane::hash::{Digest, Hasher, Sha256, Sha512};
use starnix_core::device::DeviceMode;
use starnix_core::device::kobject::{Device, DeviceMetadata};
use starnix_core::fs::sysfs::{BlockDeviceInfo, build_block_device_directory};
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt, ProtectionFlags};
use starnix_core::security;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::buffers::{InputBuffer, VecOutputBuffer};
use starnix_core::vfs::{
    FileHandle, FileObject, FileObjectState, FileOps, FsString, NamespaceNode, OutputBuffer,
    fileops_impl_dataless, fileops_impl_noop_sync, fileops_impl_seekable, fileops_impl_seekless,
};
use starnix_ext::map_ext::EntryExt;
use starnix_logging::{log_trace, track_stub};
use starnix_sync::{DeviceMapperRegistryDevicesLock, DmDeviceStateLock, LockDepMutex};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::auth::CAP_SYS_ADMIN;
use starnix_uapi::device_id::{DEVICE_MAPPER_MAJOR, DeviceId, LOOP_MAJOR};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserCString, UserRef};
use starnix_uapi::{
    DM_ACTIVE_PRESENT_FLAG, DM_BUFFER_FULL_FLAG, DM_DEV_ARM_POLL, DM_DEV_CREATE, DM_DEV_REMOVE,
    DM_DEV_RENAME, DM_DEV_SET_GEOMETRY, DM_DEV_STATUS, DM_DEV_SUSPEND, DM_DEV_WAIT,
    DM_GET_TARGET_VERSION, DM_IMA_MEASUREMENT_FLAG, DM_INACTIVE_PRESENT_FLAG, DM_LIST_DEVICES,
    DM_LIST_VERSIONS, DM_MAX_TYPE_NAME, DM_NAME_LEN, DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID,
    DM_NAME_LIST_FLAG_HAS_UUID, DM_READONLY_FLAG, DM_REMOVE_ALL, DM_STATUS_TABLE_FLAG,
    DM_SUSPEND_FLAG, DM_TABLE_CLEAR, DM_TABLE_DEPS, DM_TABLE_LOAD, DM_TABLE_STATUS, DM_TARGET_MSG,
    DM_UEVENT_GENERATED_FLAG, DM_VERSION, DM_VERSION_MAJOR, DM_VERSION_MINOR,
    DM_VERSION_PATCHLEVEL, errno, error, uapi,
};
use std::collections::btree_map::{BTreeMap, Entry};
use std::ops::Sub;
use std::sync::Arc;

const SECTOR_SIZE: u64 = 512;
// The value of the data_size field in the output dm_ioctl struct when no data is returned as per
// Linux 6.6.15.
const DATA_SIZE: u32 = 305;
// Observed version values for the dm-verity target as per Linux 6.6.15.
const DM_VERITY_VERSION_MAJOR: u32 = 1;
const DM_VERITY_VERSION_MINOR: u32 = 9;
const DM_VERITY_VERSION_PATCHLEVEL: u32 = 0;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct DeviceMapperFlags: u32 {
        const ACTIVE_PRESENT = DM_ACTIVE_PRESENT_FLAG;
        const INACTIVE_PRESENT = DM_INACTIVE_PRESENT_FLAG;
        const READONLY = DM_READONLY_FLAG;
        const SUSPEND = DM_SUSPEND_FLAG;
        const UEVENT_GENERATED = DM_UEVENT_GENERATED_FLAG;
        const BUFFER_FULL = DM_BUFFER_FULL_FLAG;
        const STATUS_TABLE = DM_STATUS_TABLE_FLAG;
        const IMA_MEASUREMENT = DM_IMA_MEASUREMENT_FLAG;
        const DM_NAME_LIST_HAS_UUID = DM_NAME_LIST_FLAG_HAS_UUID;
        const DM_NAME_LIST_NO_UUID = DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID;
    }
}

pub fn device_mapper_init(kernel: &Kernel) -> Result<(), Errno> {
    kernel.device_registry.register_major(
        "device-mapper".into(),
        DeviceMode::Block,
        DEVICE_MAPPER_MAJOR,
        get_or_create_dm_device,
    )?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct DeviceMapperRegistry {
    devices: LockDepMutex<BTreeMap<u32, Arc<DmDevice>>, DeviceMapperRegistryDevicesLock>,
}

impl DeviceMapperRegistry {
    /// Looks up a dm-device based on strictly one of the following: name, uuid, or dev number.
    fn get(&self, io: &uapi::dm_ioctl) -> Result<Arc<DmDevice>, Errno> {
        if io.name != [0; DM_NAME_LEN as usize] {
            if io.uuid != [0; DM_UUID_LEN as usize] || io.dev > 0 {
                return error!(EINVAL);
            } else {
                return self.get_by_name(io.name);
            }
        }
        if io.uuid != [0; DM_UUID_LEN as usize] {
            if io.dev > 0 {
                return error!(EINVAL);
            } else {
                return self.get_by_uuid(io.uuid);
            }
        }
        let dev_minor = ((io.dev >> 12 & 0xffffff00) | (io.dev & 0xff)) as u32;
        self.devices.lock().get(&dev_minor).ok_or_else(|| errno!(ENODEV)).cloned()
    }

    fn get_by_name(
        &self,
        name: [std::ffi::c_char; DM_NAME_LEN as usize],
    ) -> Result<Arc<DmDevice>, Errno> {
        let devices = self.devices.lock();
        let entry = devices.iter().find(|(_, device)| {
            let state = device.state.lock();
            state.name == name
        });
        if let Some((_, device)) = entry { Ok(device.clone()) } else { error!(ENODEV) }
    }

    fn get_by_uuid(
        &self,
        uuid: [std::ffi::c_char; DM_UUID_LEN as usize],
    ) -> Result<Arc<DmDevice>, Errno> {
        let devices = self.devices.lock();
        let entry = devices.iter().find(|(_, device)| {
            let state = device.state.lock();
            state.uuid == uuid
        });
        if let Some((_, device)) = entry { Ok(device.clone()) } else { error!(ENODEV) }
    }

    fn get_or_create_by_minor(
        &self,
        current_task: &CurrentTask,
        minor: u32,
    ) -> Result<Arc<DmDevice>, Errno> {
        self.devices
            .lock()
            .entry(minor)
            .or_insert_with_fallible(|| DmDevice::new(current_task, minor))
            .cloned()
    }

    /// Finds a free minor number in the DeviceMapperRegistry. Returns that minor number along with
    /// a new DmDevice associated with that minor number.
    fn find(&self, current_task: &CurrentTask) -> Result<Arc<DmDevice>, Errno> {
        let mut devices = self.devices.lock();
        for minor in 0..u32::MAX {
            match devices.entry(minor) {
                Entry::Vacant(e) => {
                    let device = DmDevice::new(current_task, minor)?;
                    e.insert(device.clone());
                    return Ok(device);
                }
                Entry::Occupied(_) => {}
            }
        }
        error!(ENODEV)
    }

    /// Removes `device` from both the Device and DeviceMapper registries.
    fn remove(
        &self,
        current_task: &CurrentTask,
        devices: &mut BTreeMap<u32, Arc<DmDevice>>,
        minor: u32,
        k_device: &Option<Device>,
    ) -> Result<(), Errno> {
        devices.remove(&minor).ok_or_else(|| errno!(ENODEV))?;
        let kernel = current_task.kernel();
        let registry = &kernel.device_registry;
        if let Some(dev) = &k_device {
            registry.remove_device(current_task, dev.clone());
        } else {
            return error!(EINVAL);
        }
        Ok(())
    }
}
#[derive(Debug, Default)]
pub struct DmDevice {
    number: DeviceId,
    state: LockDepMutex<DmDeviceState, DmDeviceStateLock>,
}

impl DmDevice {
    fn new(current_task: &CurrentTask, minor: u32) -> Result<Arc<Self>, Errno> {
        let kernel = current_task.kernel();
        let registry = &kernel.device_registry;
        let dm_device_name = FsString::from(format!("dm-{minor}"));
        let virtual_block_class = registry.objects.virtual_block_class();
        let device = Arc::new(Self {
            number: DeviceId::new(DEVICE_MAPPER_MAJOR, minor),
            ..Default::default()
        });
        let device_weak = Arc::<DmDevice>::downgrade(&device);
        let k_device = registry.add_device(
            current_task.kernel(),
            dm_device_name.as_ref(),
            DeviceMetadata::new(
                dm_device_name.clone(),
                DeviceId::new(DEVICE_MAPPER_MAJOR, minor),
                DeviceMode::Block,
            )
            .with_devtype("disk"),
            virtual_block_class,
            |device, dir| build_block_device_directory(device, device_weak, dir),
        )?;
        {
            let mut state = device.state.lock();
            state.set_k_device(k_device);
        }
        Ok(device)
    }

    fn create_file_ops(self: &Arc<Self>) -> Box<dyn FileOps> {
        let mut state = self.state.lock();
        state.open_count += 1;
        Box::new(DmDeviceFile { device: self.clone() })
    }
}

impl BlockDeviceInfo for DmDevice {
    fn size(&self) -> Result<usize, Errno> {
        let state = self.state.lock();
        if !state.suspended {
            if let Some(active_table) = &state.active_table {
                Ok(active_table.size())
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }
}
struct DmDeviceFile {
    device: Arc<DmDevice>,
}

impl FileOps for DmDeviceFile {
    fileops_impl_seekable!();

    // Writes aren't supported for these files, no need to sync the data.
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(ENOTSUP)
    }

    fn read(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let device = &self.device;
        let mut state = device.state.lock();
        if state.suspended {
            track_stub!(TODO("https://fxbug.dev/338241090"), "Defer io for suspended devices.");
            return Ok(0);
        }
        if let Some(active_table) = &mut state.active_table {
            let mut bytes_read = 0;
            let to_read = std::cmp::min(
                data.available(),
                active_table.size().checked_sub(offset).ok_or_else(|| errno!(EINVAL))?,
            );
            let mut buffer = VecOutputBuffer::new(to_read);
            if active_table.targets.len() > 1 {
                track_stub!(
                    TODO("https://fxbug.dev/339701082"),
                    "Support reads for multiple targets."
                );
                return error!(ENOTSUP);
            }
            let target = &mut active_table.targets[0];
            let start = (target.sector_start * SECTOR_SIZE) as usize;
            debug_assert!(start == 0);
            let size = (target.length * SECTOR_SIZE) as usize;
            if offset >= start && offset < start + size {
                match &mut target.target_type {
                    TargetType::Verity(verity_target) => {
                        if to_read % verity_target.params.hash_block_size as usize != 0 {
                            return error!(EINVAL);
                        }
                        let read = verity_target.block_device.ops().read(
                            &verity_target.block_device,
                            current_task,
                            offset - start,
                            &mut buffer,
                        )?;
                        bytes_read += read;
                        if let Err(_) = verity_target.verifier.verify(offset - start, buffer.data())
                        {
                            verity_target.corrupted = true;
                            return error!(EINVAL);
                        }
                    }
                    TargetType::Error => {
                        return error!(EIO);
                    }
                }
            }
            let read = data.write_all(buffer.data())?;
            debug_assert!(read == bytes_read);
            Ok(bytes_read)
        } else {
            Ok(0)
        }
    }

    fn get_memory(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        let device = &self.device;
        let state = device.state.lock();
        if state.suspended {
            track_stub!(TODO("https://fxbug.dev/338241090"), "Defer io for suspended devices.");
            return error!(EINVAL);
        }
        if let Some(active_table) = &state.active_table {
            if active_table.targets.len() > 1 {
                track_stub!(
                    TODO("https://fxbug.dev/339701082"),
                    "Support pager-backed vmos for multiple targets."
                );
                return error!(ENOTSUP);
            }
            match &active_table.targets[0].target_type {
                TargetType::Verity(verity_target) => verity_target.block_device.ops().get_memory(
                    &verity_target.block_device,
                    current_task,
                    length,
                    prot,
                ),
                TargetType::Error => {
                    error!(EIO)
                }
            }
        } else {
            error!(EINVAL)
        }
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let device = &self.device;
        let mut state = device.state.lock();
        if !state.suspended {
            if let Some(active_table) = &mut state.active_table {
                if active_table.targets.len() == 1 {
                    let target = &mut active_table.targets[0];
                    if let TargetType::Verity(args) = &mut target.target_type {
                        return args.block_device.ops().ioctl(
                            &*args.block_device,
                            current_task,
                            request,
                            arg,
                        );
                    }
                }
            }
        }

        error!(ENOTTY)
    }

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        let mut state = self.device.state.lock();
        state.open_count -= 1;
    }
}
#[derive(Debug)]
struct DmDeviceState {
    version: [u32; 3],
    target_count: u32,
    open_count: u64,
    name: [std::ffi::c_char; 128],
    uuid: [std::ffi::c_char; 129],
    active_table: Option<DmDeviceTable>,
    inactive_table: Option<DmDeviceTable>,
    flags: DeviceMapperFlags,
    suspended: bool,
    k_device: Option<Device>,
}

impl Default for DmDeviceState {
    fn default() -> Self {
        DmDeviceState {
            version: [0; 3],
            name: [0 as std::ffi::c_char; 128],
            uuid: [0 as std::ffi::c_char; 129],
            target_count: 0,
            open_count: 0,
            active_table: None,
            inactive_table: None,
            flags: DeviceMapperFlags::empty(),
            suspended: false,
            k_device: None,
        }
    }
}

impl DmDeviceState {
    fn set_version(&mut self) {
        self.version = [DM_VERSION_MAJOR, DM_VERSION_MINOR, DM_VERSION_PATCHLEVEL];
    }

    fn set_name(&mut self, name: [std::ffi::c_char; 128]) {
        self.name = name;
    }

    fn set_uuid(&mut self, uuid: [std::ffi::c_char; 129]) {
        self.uuid = uuid;
    }

    fn set_inactive_table(&mut self, inactive_table: DmDeviceTable) {
        self.inactive_table = Some(inactive_table);
    }

    fn resume(&mut self) {
        if let Some(inactive_table) = self.inactive_table.take() {
            self.active_table = Some(inactive_table);
        }
        self.suspended = false;
    }

    fn remove(&mut self) {
        self.active_table.take();
        self.suspended = false;
    }

    fn set_target_count(&mut self, target_count: u32) {
        self.target_count = target_count;
    }

    fn get_target_count(&self) -> u32 {
        if let Some(_) = self.active_table { self.target_count } else { 0 }
    }

    fn add_flags(&mut self, flags: DeviceMapperFlags) {
        self.flags |= flags;
    }

    fn get_flags(&self) -> DeviceMapperFlags {
        let mut flags = DeviceMapperFlags::empty();
        if let Some(active_table) = &self.active_table {
            flags |= DeviceMapperFlags::ACTIVE_PRESENT;
            if active_table.readonly {
                flags |= DeviceMapperFlags::READONLY;
            }
        }
        if let Some(_) = &self.inactive_table {
            flags |= DeviceMapperFlags::INACTIVE_PRESENT;
        }
        if self.suspended {
            flags |= DeviceMapperFlags::SUSPEND;
        }
        flags
    }

    fn suspend(&mut self) {
        self.suspended = true;
    }

    fn set_k_device(&mut self, k_device: Device) {
        self.k_device = Some(k_device);
    }
}
#[derive(Debug, Clone)]
struct DmDeviceTarget {
    sector_start: u64,
    length: u64,
    status: i32,
    name: [std::ffi::c_char; DM_MAX_TYPE_NAME as usize],
    target_type: TargetType,
}

#[derive(Debug, Default, Clone)]
pub struct DmDeviceTable {
    targets: Vec<DmDeviceTarget>,
    readonly: bool,
}

impl DmDeviceTable {
    fn size(&self) -> usize {
        let mut size = 0;
        for target in &self.targets {
            size += (SECTOR_SIZE * target.length) as usize;
        }
        size
    }
}

struct DeviceMapper {
    registry: Arc<DeviceMapperRegistry>,
}

impl DeviceMapper {
    pub fn new(registry: Arc<DeviceMapperRegistry>) -> Self {
        Self { registry: registry }
    }
}

#[derive(Debug, Clone)]
enum TargetType {
    Verity(Box<VerityTarget>),
    Error,
}

#[derive(Debug, Clone)]
struct VerityTarget {
    verifier: MerkleVerifier,
    params: DmVerityTargetParams,
    block_device: FileHandle,
    corrupted: bool,
}

impl VerityTarget {
    fn create_and_verify(
        params: DmVerityTargetParams,
        block_device: FileHandle,
        leaf_hashes: Box<[u8]>,
    ) -> Result<Self, Errno> {
        let verifier = dm_verity::create_verifier(&params, leaf_hashes).map_err(|e| match &e {
            DmVerityError::RootHashMismatch => {
                starnix_logging::log_warn!(
                    "Merkle tree root hash mismatch. Expected: {}",
                    params.root_digest
                );
                errno!(EINVAL)
            }
            DmVerityError::UnsupportedAlgorithm(algorithm) => {
                starnix_logging::log_warn!(
                    "dm-verity verifier creation failed for unsupported algorithm: {}",
                    algorithm
                );
                errno!(ENOTSUP)
            }
            _ => {
                starnix_logging::log_warn!("dm-verity verifier creation failed: {:?}", e);
                errno!(EINVAL)
            }
        })?;
        Ok(Self { verifier, params, block_device, corrupted: false })
    }

    fn parameter_string(&self) -> String {
        let params = &self.params;
        let base_string = format!(
            "{} {} {} {} {} {} {} {} {} {}",
            params.version,
            params.block_device_path,
            params.hash_device_path,
            params.data_block_size,
            params.hash_block_size,
            params.num_data_blocks,
            params.hash_start_block,
            params.hash_algorithm.as_str(),
            params.root_digest,
            params.salt
        );
        let mut optional_arg_count = 0;
        let mut optional_string = String::new();
        if params.optional_params.ignore_zero_blocks {
            optional_arg_count += 1;
            optional_string.push_str(" ignore_zero_blocks");
        }
        if params.optional_params.restart_on_corruption {
            optional_arg_count += 1;
            optional_string.push_str(" restart on corruption");
        }
        if optional_arg_count > 0 {
            format!("{base_string} {optional_arg_count}{optional_string}")
        } else {
            base_string
        }
    }
}

// Returns the FileHandle and minor number of the device found at `device path` formatted as
// either /dev/loop# of MAJOR:MINOR

fn open_device(
    current_task: &CurrentTask,
    device_path: String,
) -> Result<(u64, FileHandle), Errno> {
    let device_path_vec: Vec<&str> = device_path.split(":").collect();
    if device_path_vec.len() == 1 {
        let dev = current_task.open_file(device_path.as_str().into(), OpenFlags::RDONLY)?;
        let loop_device_vec: Vec<&str> = device_path.split("loop").collect();
        let minor = loop_device_vec[1].parse::<u64>().unwrap();
        Ok((minor, dev))
    } else {
        let minor = device_path_vec[1].parse::<u64>().unwrap();
        let dev = current_task
            .open_file(format!("/dev/loop{minor}").as_str().into(), OpenFlags::RDONLY)?;
        Ok((minor, dev))
    }
}

fn size_of_merkle_tree_preceding_leaf_nodes(
    leaf_nodes_size: u64,
    hash_size: u64,
    hash_block_size: u64,
) -> u64 {
    let mut total_size = 0;
    let mut data_size = leaf_nodes_size;
    while data_size > hash_block_size {
        let num_hashes = data_size.div_ceil(hash_block_size);
        let hashes_per_block = hash_block_size.div_ceil(hash_size);
        let hash_blocks = num_hashes.div_ceil(hashes_per_block);
        data_size = hash_blocks * hash_block_size;
        total_size += data_size;
    }
    total_size
}

// Parse the parameter string into a TargetType.

fn parse_parameter_string(
    current_task: &CurrentTask,
    target_type: &str,
    parameter_str: String,
) -> Result<TargetType, Errno> {
    match target_type {
        "verity" => {
            let v: Vec<&str> = parameter_str.split(" ").collect();
            let hash_algorithm = v[7].parse::<HashAlgorithm>().map_err(|_| errno!(ENOTSUP))?;
            let mut params = DmVerityTargetParams {
                version: String::from(v[0]),
                block_device_path: String::from(v[1]),
                hash_device_path: String::from(v[2]),
                data_block_size: v[3].parse::<u64>().unwrap(),
                hash_block_size: v[4].parse::<u64>().unwrap(),
                num_data_blocks: v[5].parse::<u64>().unwrap(),
                hash_start_block: v[6].parse::<u64>().unwrap(),
                hash_algorithm,
                root_digest: String::from(v[8]),
                salt: String::from(v[9]),
                optional_params: DmVerityTargetOptionalParams::default(),
            };

            if v.len() > 10 {
                let num_optional_args = v[10].parse::<u64>().unwrap();
                if num_optional_args > 2 {
                    return error!(ENOTSUP);
                }
                for i in 0..num_optional_args {
                    if v[11 + i as usize] == "ignore_zero_blocks" {
                        params.optional_params.ignore_zero_blocks = true;
                    } else if v[11 + i as usize] == "restart_on_corruption" {
                        track_stub!(
                            TODO("https://fxbug.dev/338243823"),
                            "Support restart on corruption."
                        );
                        params.optional_params.restart_on_corruption = true;
                    } else {
                        return error!(ENOTSUP);
                    }
                }
            }

            let (minor, block_device) = open_device(current_task, params.block_device_path)?;
            params.block_device_path = format!("{LOOP_MAJOR}:{minor}");

            let (minor, hash_device) = if params.hash_device_path == params.block_device_path {
                (minor, block_device.clone())
            } else {
                open_device(current_task, params.hash_device_path)?
            };
            params.hash_device_path = format!("{LOOP_MAJOR}:{minor}");

            let hash_size: u64 = match params.hash_algorithm {
                HashAlgorithm::Sha256 => <Sha256 as Hasher>::Digest::DIGEST_LEN as u64,
                HashAlgorithm::Sha512 => <Sha512 as Hasher>::Digest::DIGEST_LEN as u64,
            };

            debug_assert!(params.hash_block_size > 0);
            let data_size = params.num_data_blocks * params.data_block_size;
            let num_hashes = data_size.div_ceil(params.hash_block_size);
            let hashes_per_block = params.hash_block_size.div_ceil(hash_size);
            let hash_blocks = num_hashes.div_ceil(hashes_per_block);
            let leaf_nodes_size = hash_blocks * params.hash_block_size;
            let mut buffer = VecOutputBuffer::new(leaf_nodes_size as usize);
            let offset = params.hash_start_block * params.hash_block_size
                + size_of_merkle_tree_preceding_leaf_nodes(
                    leaf_nodes_size,
                    hash_size,
                    params.hash_block_size,
                );
            let bytes_read =
                hash_device.ops().read(&hash_device, current_task, offset as usize, &mut buffer)?;
            debug_assert!(bytes_read == leaf_nodes_size as usize);

            let leaf_hashes: Vec<u8> = buffer.into();
            let verity_target = VerityTarget::create_and_verify(
                params,
                block_device,
                leaf_hashes.into_boxed_slice(),
            )?;
            Ok(TargetType::Verity(Box::new(verity_target)))
        }
        "error" => Ok(TargetType::Error),
        _ => error!(ENOTSUP),
    }
}

fn check_version_compatibility(major: u32, minor: u32) -> Result<(), Errno> {
    // The version field of the input dm-ioctl struct should represent the version of the interface
    // that the client was compiled with. The major number must match the kernel's, the minor
    // number is backwards compatible, and the patchlevel is forwards and backwards compatible.
    if major != DM_VERSION_MAJOR || minor > DM_VERSION_MINOR {
        return error!(EINVAL);
    }
    return Ok(());
}

impl FileOps for DeviceMapper {
    fileops_impl_seekless!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        security::check_task_capable(current_task, CAP_SYS_ADMIN).map_err(|_| errno!(EACCES))?;
        let user_info = UserRef::<uapi::dm_ioctl>::from(arg);
        let info_addr: starnix_uapi::user_address::UserAddress = user_info.addr();
        let info = current_task.read_object(user_info)?;
        let flags = DeviceMapperFlags::from_bits_truncate(info.flags);
        match request {
            DM_DEV_CREATE => {
                // Expect name and version to be set. This should not fail if uuid is not set.
                if info.name == [0; DM_NAME_LEN as usize] || info.version == [0; 3] {
                    return error!(EINVAL);
                }
                let dm_device = self.registry.find(current_task)?;
                let mut state = dm_device.state.lock();
                check_version_compatibility(info.version[0], info.version[1])?;
                state.set_version();
                state.set_name(info.name);
                state.set_uuid(info.uuid);
                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: DATA_SIZE,
                    data_start: 0,
                    ..Default::default()
                };
                log_trace!("DM_DEV_CREATE returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_TABLE_LOAD => {
                let mut start_addr = (info_addr + info.data_start)?;
                let mut num_targets = 0;
                let dm_device = self.registry.get(&info)?;
                let mut state = dm_device.state.lock();
                let mut table = DmDeviceTable { ..Default::default() };
                if flags.contains(DeviceMapperFlags::READONLY) {
                    table.readonly = true;
                    state.add_flags(DeviceMapperFlags::READONLY);
                }
                if info.target_count > 1 {
                    track_stub!(TODO("https://fxbug.dev/339701082"), "Support multiple targets.");
                    return error!(ENOTSUP);
                }
                track_stub!(
                    TODO("https://fxbug.dev/338245544"),
                    "Make sure targets are contiguous and non-overlapping"
                );
                while num_targets < info.target_count {
                    let target_ref = UserRef::<uapi::dm_target_spec>::new(start_addr);
                    let target = current_task.read_object(target_ref)?;
                    let parameter_cstring =
                        UserCString::new(current_task, target_ref.next()?.addr());
                    let parameters = current_task.read_c_string_to_vec(
                        parameter_cstring,
                        target.next as usize - std::mem::size_of::<uapi::dm_target_spec>(),
                    )?;
                    let target_type_addr = start_addr
                        .checked_add(
                            2 * std::mem::size_of::<u64>()
                                + std::mem::size_of::<u32>()
                                + std::mem::size_of::<i32>(),
                        )
                        .ok_or_else(|| errno!(EINVAL))?;
                    let target_type_cstring = UserCString::new(current_task, target_type_addr);
                    let target_type = current_task
                        .read_c_string_to_vec(target_type_cstring, DM_MAX_TYPE_NAME as usize)?;

                    let device_target = DmDeviceTarget {
                        sector_start: target.sector_start,
                        length: target.length,
                        status: target.status,
                        name: target.target_type,
                        target_type: parse_parameter_string(
                            current_task,
                            &target_type.to_string(),
                            parameters.to_string(),
                        )?,
                    };
                    table.targets.push(device_target);
                    num_targets += 1;
                    debug_assert!(target.next % 8 == 0);
                    start_addr = start_addr
                        .checked_add(target.next as usize)
                        .ok_or_else(|| errno!(EINVAL))?;
                }

                // Update the metadata of the dm device
                state.set_inactive_table(table);
                state.set_target_count(num_targets);
                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: DATA_SIZE,
                    data_start: 0,
                    flags: state.get_flags().bits(),
                    target_count: state.get_target_count(),
                    ..Default::default()
                };
                log_trace!("DM_TABLE_LOAD returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_DEV_SUSPEND => {
                let dm_device = self.registry.get(&info)?;
                let mut state = dm_device.state.lock();
                if flags.contains(DeviceMapperFlags::SUSPEND) {
                    state.suspend();
                } else {
                    state.resume();
                }
                let mut out_flags = state.get_flags();
                if !flags.contains(DeviceMapperFlags::SUSPEND) {
                    out_flags |= DeviceMapperFlags::UEVENT_GENERATED;
                }
                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: DATA_SIZE,
                    data_start: 0,
                    flags: out_flags.bits(),
                    target_count: state.get_target_count(),
                    ..Default::default()
                };
                log_trace!("DM_DEV_SUSPEND returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_DEV_STATUS => {
                let dm_device = self.registry.get(&info)?;
                let state = dm_device.state.lock();
                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: DATA_SIZE,
                    data_start: 0,
                    flags: state.get_flags().bits(),
                    target_count: state.get_target_count(),
                    ..Default::default()
                };
                log_trace!("DM_DEV_STATUS returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_DEV_REMOVE => {
                let dm_device = self.registry.get(&info)?;
                let mut devices = self.registry.devices.lock();
                let mut state = dm_device.state.lock();
                if state.open_count > 0 {
                    return error!(ENOTSUP);
                }
                self.registry.remove(
                    current_task,
                    &mut devices,
                    dm_device.number.minor(),
                    &state.k_device,
                )?;
                state.remove();
                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: DATA_SIZE,
                    data_start: 0,
                    flags: (state.get_flags() | DeviceMapperFlags::UEVENT_GENERATED).bits(),
                    target_count: state.get_target_count(),
                    ..Default::default()
                };
                log_trace!("DM_DEV_REMOVE returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_LIST_DEVICES => {
                if flags.contains(DeviceMapperFlags::DM_NAME_LIST_HAS_UUID)
                    || flags.contains(DeviceMapperFlags::DM_NAME_LIST_NO_UUID)
                {
                    return error!(ENOTSUP);
                }
                let mut name_list_addr = user_info.next()?.addr();
                let mut total_size = std::mem::size_of::<uapi::dm_ioctl>() as u32;
                for (_, device) in self.registry.devices.lock().iter() {
                    let state = device.state.lock();
                    let dm_name_list = UserRef::<uapi::dm_name_list>::new(name_list_addr);
                    let name = state.name.iter().map(|v| *v as u8).collect::<Vec<u8>>();
                    let name_c_str = std::ffi::CStr::from_bytes_until_nul(name.as_slice())
                        .map_err(|_| errno!(EINVAL))?;
                    let mut name_vec_with_nul = name_c_str.to_bytes_with_nul().to_vec();
                    let mut size = (std::mem::size_of::<uapi::dm_name_list>() - 4
                        + name_vec_with_nul.len()) as u32;
                    let mut padding = 0;
                    if size % 8 != 0 {
                        padding = 8 - (size % 8);
                        size += padding;
                    };
                    // For the event_nr and flags.
                    size += 8;
                    let name_list = uapi::dm_name_list {
                        dev: device.number.bits(),
                        next: size,
                        ..Default::default()
                    };
                    if total_size + size > info.data_size {
                        let i = uapi::dm_ioctl {
                            data_size: DATA_SIZE,
                            data_start: std::mem::size_of::<uapi::dm_ioctl>() as u32,
                            flags: DeviceMapperFlags::BUFFER_FULL.bits(),
                            ..Default::default()
                        };
                        log_trace!("DM_LIST_DEVICES returned dm_ioctl: {:?}", i);
                        current_task.write_object(user_info, &i)?;
                        return Ok(SUCCESS);
                    }
                    total_size += size;
                    let mut name_addr = dm_name_list.next()?.addr();
                    name_addr = name_addr.sub(4 as usize)?;
                    current_task.write_object(dm_name_list, &name_list)?;
                    name_vec_with_nul.extend(vec![0; padding as usize + 8]);
                    current_task.write_memory(name_addr, name_vec_with_nul.as_slice())?;
                    name_list_addr =
                        name_list_addr.checked_add(size as usize).ok_or_else(|| errno!(EINVAL))?;
                }
                let i = uapi::dm_ioctl {
                    data_size: total_size,
                    data_start: std::mem::size_of::<uapi::dm_ioctl>() as u32,
                    ..Default::default()
                };
                log_trace!("DM_LIST_DEVICE returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_LIST_VERSIONS => {
                let version_list_addr = user_info.next()?.addr();
                let dm_versions_list = UserRef::<uapi::dm_target_versions>::new(version_list_addr);
                let name_c_str =
                    std::ffi::CString::new(String::from("verity")).map_err(|_| errno!(EINVAL))?;
                let mut name_vec_with_nul = name_c_str.as_bytes_with_nul().to_vec();
                let mut size = (std::mem::size_of::<uapi::dm_target_versions>()
                    + name_vec_with_nul.len()) as u32;
                let mut padding = 0;
                if size % 8 != 0 {
                    padding = 8 - (size % 8);
                    size += padding;
                };
                name_vec_with_nul.extend(vec![0; padding as usize]);
                if std::mem::size_of::<uapi::dm_ioctl>() as u32 + size > info.data_size {
                    let i = uapi::dm_ioctl {
                        data_size: DATA_SIZE,
                        data_start: 0,
                        flags: DeviceMapperFlags::BUFFER_FULL.bits(),
                        ..Default::default()
                    };
                    log_trace!("DM_LIST_VERSIONS returned dm_ioctl: {:?}", i);
                    current_task.write_object(user_info, &i)?;
                    return Ok(SUCCESS);
                }
                let target_versions = uapi::dm_target_versions {
                    next: size,
                    version: [
                        DM_VERITY_VERSION_MAJOR,
                        DM_VERITY_VERSION_MINOR,
                        DM_VERITY_VERSION_PATCHLEVEL,
                    ],
                    ..Default::default()
                };
                let name_addr = dm_versions_list.next()?.addr();
                current_task.write_object(dm_versions_list, &target_versions)?;
                current_task.write_memory(name_addr, name_vec_with_nul.as_slice())?;

                let i = uapi::dm_ioctl {
                    data_size: std::mem::size_of::<uapi::dm_ioctl>() as u32 + size,
                    data_start: std::mem::size_of::<uapi::dm_ioctl>() as u32,
                    ..Default::default()
                };
                log_trace!("DM_LIST_VERSIONS returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            DM_TABLE_STATUS => {
                let dm_device = self.registry.get(&info)?;
                let state = dm_device.state.lock();
                let mut total_data_size = 0;
                let mut data_padding = 0;
                let mut target_spec_addr = user_info.next()?.addr();
                let mut out_flags = DeviceMapperFlags::empty();
                let space_for_data = info.data_size as usize
                    - std::cmp::min(info.data_size as usize, std::mem::size_of::<uapi::dm_ioctl>());
                if let Some(active_table) = &state.active_table {
                    for target in &active_table.targets {
                        let target_spec_info =
                            UserRef::<uapi::dm_target_spec>::new(target_spec_addr);
                        let mut data_size = std::mem::size_of::<uapi::dm_target_spec>();
                        if total_data_size + data_size - data_padding >= space_for_data {
                            out_flags |= DeviceMapperFlags::BUFFER_FULL;
                            break;
                        }
                        let data_addr = target_spec_info.next()?.addr();
                        if flags.contains(DeviceMapperFlags::STATUS_TABLE) {
                            match &target.target_type {
                                TargetType::Verity(args) => {
                                    let param_str = std::ffi::CString::new(args.parameter_string())
                                        .map_err(|_| errno!(EINVAL))?;
                                    let mut args_bytes = param_str.into_bytes_with_nul();
                                    if args_bytes.len() % 8 != 0 {
                                        data_padding = 8 - args_bytes.len() % 8;
                                        args_bytes.extend(vec![0 as u8; data_padding]);
                                    }
                                    data_size += args_bytes.len();
                                    if total_data_size + data_size - data_padding >= space_for_data
                                    {
                                        out_flags |= DeviceMapperFlags::BUFFER_FULL;
                                        break;
                                    }
                                    current_task.write_memory(data_addr, args_bytes.as_slice())?;
                                }
                                TargetType::Error => {
                                    // Error target has no parameters.
                                }
                            }
                        } else if flags.contains(DeviceMapperFlags::IMA_MEASUREMENT) {
                            // Linux 6.6.15 does not currently support IMA for dm-verity.
                            return error!(ENOTSUP);
                        } else {
                            match &target.target_type {
                                TargetType::Verity(args) => {
                                    let status = if args.corrupted { "C" } else { "V" };
                                    let status_c_str = std::ffi::CString::new(String::from(status))
                                        .map_err(|_| errno!(EINVAL))?;
                                    let mut status_bytes = status_c_str.into_bytes_with_nul();
                                    if status_bytes.len() % 8 != 0 {
                                        data_padding = 8 - status_bytes.len() % 8;
                                        status_bytes.extend(vec![0 as u8; data_padding]);
                                    }
                                    data_size += status_bytes.len();
                                    if total_data_size + data_size - data_padding >= space_for_data
                                    {
                                        out_flags |= DeviceMapperFlags::BUFFER_FULL;
                                        break;
                                    }
                                    current_task
                                        .write_memory(data_addr, status_bytes.as_slice())?;
                                }
                                TargetType::Error => {
                                    // Status for error target is not used much but we can provide something.
                                }
                            }
                        }
                        total_data_size += data_size;
                        let target_spec = uapi::dm_target_spec {
                            sector_start: target.sector_start,
                            length: target.length,
                            status: target.status,
                            target_type: target.name,
                            next: total_data_size as u32,
                        };
                        current_task.write_object(target_spec_info, &target_spec)?;
                        target_spec_addr = target_spec_addr
                            .checked_add(data_size)
                            .ok_or_else(|| errno!(EINVAL))?;
                    }
                } else {
                    let i = uapi::dm_ioctl {
                        name: state.name,
                        version: state.version,
                        uuid: state.uuid,
                        dev: dm_device.number.bits(),
                        data_size: DATA_SIZE,
                        data_start: std::mem::size_of::<uapi::dm_ioctl>() as u32,
                        flags: state.get_flags().bits(),
                        ..Default::default()
                    };
                    log_trace!("DM_TABLE_STATUS returned dm_ioctl: {:?}", i);
                    current_task.write_object(user_info, &i)?;
                    return Ok(SUCCESS);
                }
                // Linux removes the size of the data padding when calculating the data size field
                // returned.
                let total_size = if out_flags.contains(DeviceMapperFlags::BUFFER_FULL) {
                    DATA_SIZE
                } else {
                    (total_data_size + std::mem::size_of::<uapi::dm_ioctl>() - data_padding) as u32
                };
                out_flags |= state.get_flags();

                let i = uapi::dm_ioctl {
                    name: state.name,
                    version: state.version,
                    uuid: state.uuid,
                    dev: dm_device.number.bits(),
                    data_size: total_size,
                    data_start: std::mem::size_of::<uapi::dm_ioctl>() as u32,
                    flags: out_flags.bits(),
                    ..Default::default()
                };
                log_trace!("DM_TABLE_STATUS returned dm_ioctl: {:?}", i);
                current_task.write_object(user_info, &i)?;
                Ok(SUCCESS)
            }
            // These dm ioctls are not used by Android
            DM_VERSION
            | DM_DEV_RENAME
            | DM_DEV_WAIT
            | DM_TABLE_CLEAR
            | DM_TABLE_DEPS
            | DM_REMOVE_ALL
            | DM_TARGET_MSG
            | DM_DEV_SET_GEOMETRY
            | DM_DEV_ARM_POLL
            | DM_GET_TARGET_VERSION => return error!(ENOTSUP),
            _ => error!(ENOTTY),
        }
    }
}

pub fn create_device_mapper(
    current_task: &CurrentTask,
    _id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    Ok(Box::new(DeviceMapper::new(current_task.kernel().expando.get::<DeviceMapperRegistry>())))
}

fn get_or_create_dm_device(
    current_task: &CurrentTask,
    id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    Ok(current_task
        .kernel()
        .expando
        .get::<DeviceMapperRegistry>()
        .get_or_create_by_minor(current_task, id.minor())?
        .create_file_ops())
}
