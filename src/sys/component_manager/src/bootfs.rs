// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_boot::BootfsFileVmo;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_kernel as fkernel;
use fuchsia_async as fasync;
use fuchsia_bootfs::{
    BootfsParser, BootfsParserError, zbi_bootfs_is_aligned, zbi_bootfs_page_align,
};
use fuchsia_component::client;
use fuchsia_runtime::{HandleInfo, HandleType, take_startup_handle};
use log::info;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use thiserror::Error;
use vfs::ToObjectRequest;
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo;
use vfs::tree_builder::{self, TreeBuilder};
use zx::Resource;

// Used to create executable VMOs.
const BOOTFS_VMEX_NAME: &str = "bootfs_vmex";

const BOOTFS_VMO_NAME: &str = "bootfs";

// If passed as a kernel handle, this gets relocated into a '/boot/log' directory.
const KERNEL_CRASHLOG_NAME: &str = "crashlog";
const LAST_PANIC_FILEPATH: &str = "log/last-panic.txt";

// Kernel startup VMOs are published beneath '/boot/kernel'. The VFS is relative
// to '/boot', so we only need to prepend paths under that.
const KERNEL_VMO_SUBDIRECTORY: &str = "kernel/";

// Bootfs will sequentially number files and directories starting with this value.
// This is a self contained immutable filesystem, so we only need to ensure that
// there are no internal collisions.
const FIRST_INODE_VALUE: u64 = 1;

// Packages in bootfs can contain both executable and read-only files. For example,
// 'pkg/my_package/bin' should be executable but 'pkg/my_package/foo' should not.
const BOOTFS_PACKAGE_PREFIX: &str = "pkg";

// Packages are allowed to contain executable binaries, libraries, tests, and modern packaged drivers.
const BOOTFS_EXECUTABLE_PACKAGE_DIRECTORIES: &[&str] = &["bin", "lib", "driver", "test"];

// The root filesystem is highly restricted. Only core bootstrap binaries and shared libraries get execute rights.
// Unpackaged drivers and tests are no longer supported and are stripped of execute rights.
const BOOTFS_EXECUTABLE_ROOT_DIRECTORIES: &[&str] = &["bin", "lib", "blob", "test"];

#[derive(Debug, Error)]
pub enum BootfsError {
    #[error("Invalid handle: {handle_type:?}")]
    InvalidHandle { handle_type: HandleType },
    #[error("Failed to duplicate handle: {0}")]
    DuplicateHandle(zx::Status),
    #[error("Failed to access vmex Resource: {0}")]
    Vmex(zx::Status),
    #[error("BootfsParser error: {0}")]
    Parser(#[from] BootfsParserError),
    #[error("Failed to add entry to Bootfs VFS: {0}")]
    AddEntry(#[from] tree_builder::Error),
    #[error("Failed to locate entry for {name}")]
    MissingEntry { name: String },
    #[error("Failed to create an executable VMO: {0}")]
    ExecVmo(zx::Status),
    #[error("VMO operation failed: {0}")]
    Vmo(zx::Status),
    #[error("Failed to get VMO name: {0}")]
    VmoName(zx::Status),
    #[error("Failed to convert numerical value: {0}")]
    ConvertNumber(#[from] std::num::TryFromIntError),
    #[error("Failed to convert string value: {0}")]
    ConvertString(#[from] std::ffi::IntoStringError),
    #[error("Failed to bind Bootfs to Component Manager's namespace: {0}")]
    Namespace(zx::Status),
    #[error("Bootfs entry at offset {0} is not page-aligned")]
    MisalignedOffset(u32),
}

// Transferring data from Bootfs can only be done with page-aligned offsets
// and sizes. It is expected for the VMO offset to be aligned by BootfsParser,
// but the size alignment is not guaranteed.
fn aligned_range(offset: u32, size: u32) -> Result<Range<u64>, BootfsError> {
    if !zbi_bootfs_is_aligned(offset) {
        return Err(BootfsError::MisalignedOffset(offset));
    }
    let aligned_offset: u64 = offset.into();
    let aligned_size: u64 = zbi_bootfs_page_align(size).into();
    return Ok(aligned_offset..(aligned_offset + aligned_size));
}

pub struct BootfsSvc {
    next_inode: u64,
    parser: BootfsParser,
    bootfs: zx::Vmo,
    tree_builder: TreeBuilder,
}

impl BootfsSvc {
    pub fn new() -> Result<Self, BootfsError> {
        let bootfs_handle = take_startup_handle(HandleType::BootfsVmo.into())
            .ok_or(BootfsError::InvalidHandle { handle_type: HandleType::BootfsVmo })?;
        let bootfs = zx::Vmo::from(bootfs_handle);
        Self::new_internal(bootfs)
    }

    fn new_internal(bootfs: zx::Vmo) -> Result<Self, BootfsError> {
        let bootfs_dup = bootfs
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(BootfsError::DuplicateHandle)?
            .into();
        let parser = BootfsParser::create_from_vmo(bootfs_dup)?;
        Ok(Self {
            next_inode: FIRST_INODE_VALUE,
            parser,
            bootfs,
            tree_builder: TreeBuilder::empty_dir(),
        })
    }

    fn get_next_inode(inode: &mut u64) -> u64 {
        let next_inode = *inode;
        *inode += 1;
        next_inode
    }

    fn file_in_executable_directory(path: &Vec<&str>) -> bool {
        // If the first token is 'pkg', the second token can be anything, with the third
        // token needing to be within the list of allowed executable package directories.
        if path.len() > 2 && path[0] == BOOTFS_PACKAGE_PREFIX {
            return BOOTFS_EXECUTABLE_PACKAGE_DIRECTORIES.iter().any(|&dir| path[2] == dir);
        }
        // If the first token is an allowed executable directory, everything beneath it
        // can be marked executable.
        if path.len() > 1 {
            return BOOTFS_EXECUTABLE_ROOT_DIRECTORIES.iter().any(|&dir| path[0] == dir);
        }
        false
    }

    fn create_dir_entry(vmo: zx::Vmo, executable: bool, inode: u64) -> Arc<vmo::VmoFile> {
        vmo::VmoFile::new_with_inode_and_executable(vmo, inode, executable)
    }

    /// Read configs from the parsed bootfs image before the filesystem has been fully initialized.
    /// This is required for configs needed to run the VFS, such as the component manager config
    /// which specifies the number of threads for the executor which the VFS needs to run within.
    /// Path should be relative to '/boot' without a leading forward slash.
    pub fn read_config_from_uninitialized_vfs(
        &self,
        config_path: &str,
    ) -> Result<Vec<u8>, BootfsError> {
        for entry in self.parser.zero_copy_iter() {
            let entry = entry?;
            assert!(entry.payload.is_none()); // Using the zero copy iterator.
            if entry.name == config_path {
                let mut buffer = vec![0; usize::try_from(entry.size)?];
                self.bootfs.read(&mut buffer, entry.offset).map_err(BootfsError::Vmo)?;
                return Ok(buffer);
            }
        }
        Err(BootfsError::MissingEntry { name: config_path.to_string() })
    }

    pub fn ingest_bootfs_vmo_with_system_resource(
        self,
        system: &Option<Resource>,
        bootfs_entries: Vec<BootfsFileVmo>,
    ) -> Result<Self, BootfsError> {
        let system = system
            .as_ref()
            .ok_or(BootfsError::InvalidHandle { handle_type: HandleType::Resource })?;

        let vmex = system
            .create_child(
                zx::ResourceKind::SYSTEM,
                None,
                zx::sys::ZX_RSRC_SYSTEM_VMEX_BASE,
                1,
                BOOTFS_VMEX_NAME.as_bytes(),
            )
            .map_err(BootfsError::Vmex)?;
        self.ingest_bootfs_vmo(vmex, bootfs_entries)
    }

    pub async fn ingest_bootfs_vmo_with_namespace_vmex(
        self,
        bootfs_entries: Vec<BootfsFileVmo>,
    ) -> Result<Self, BootfsError> {
        let vmex_service = client::connect_to_protocol::<fkernel::VmexResourceMarker>()
            .map_err(|_| BootfsError::Vmex(zx::Status::UNAVAILABLE))?;
        let vmex =
            vmex_service.get().await.map_err(|_| BootfsError::Vmex(zx::Status::UNAVAILABLE))?;
        self.ingest_bootfs_vmo(vmex, bootfs_entries)
    }

    // Create a VMO and transfer the entry's data from Bootfs to it. This
    // operation will also decommit the transferred range in the Bootfs image.
    fn create_vmo_from_bootfs(
        &self,
        range: &Range<u64>,
        original_size: u64,
    ) -> Result<zx::Vmo, BootfsError> {
        let aligned_size = range.end - range.start;
        let vmo = zx::Vmo::create(original_size).map_err(BootfsError::Vmo)?;
        vmo.transfer_data(
            zx::TransferDataOptions::empty(),
            0,
            aligned_size,
            &self.bootfs,
            range.start,
        )
        .map_err(BootfsError::Vmo)?;
        Ok(vmo)
    }

    // This function will iterate through all entries parsed from the Bootfs
    // VMO. A VMO is created for each entry, and the content located at the
    // entry's offset in the Bootfs VMO is  transferred to the newly created
    // VMO. The entry VMO corresponds to a new VMO file that gets inserted into
    // the Bootfs VFS. The Bootfs VMO gets decommitted during this process.
    //
    // Note that until https://fxbug.dev/352179816 is landed, Userboot continues
    // to hold onto copy-on-write VMO clones of the Component Manager binary and
    // deps backing this process, so that even when this function creates
    // true-copy VMOs and decommits their regions, the COW VMO will still have
    // its own copy of pages with the original content.
    fn ingest_bootfs_vmo(
        mut self,
        vmex: Resource,
        bootfs_entries: Vec<BootfsFileVmo>,
    ) -> Result<Self, BootfsError> {
        // A map of `seen_ranges` records the range in the Bootfs VMO that has
        // already been processed by a previous entry. `seen_ranges` is checked
        // first to see if a VMO has already been created for a particular range
        // of data.
        //
        // Userboot may provide VMOs through `fuchsia.boot.Userboot` and those VMOs must be used
        // because they describe regions of the big VMO that are no longer accessible.

        let mut seen_ranges = HashMap::new();

        struct VmoWithSize {
            vmo: zx::Vmo,
            content_size: u64,
        }

        for entry in bootfs_entries {
            let vmo_size = entry.contents.get_content_size().map_err(BootfsError::Vmo)? as u32;
            let vmo_range = aligned_range(entry.offset, vmo_size)?;
            seen_ranges.insert(
                vmo_range,
                VmoWithSize { vmo: entry.contents, content_size: vmo_size as u64 },
            );
        }

        for entry in self.parser.zero_copy_iter() {
            let entry = entry?;
            assert!(entry.payload.is_none()); // Using the zero copy iterator.

            let vmo_range = aligned_range(entry.offset.try_into()?, entry.size.try_into()?)?;
            let vmo = match seen_ranges.get(&vmo_range) {
                Some(vmo_with_size) => {
                    assert!(vmo_with_size.content_size == entry.size);
                    vmo_with_size
                        .vmo
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .map_err(BootfsError::DuplicateHandle)?
                }
                None => {
                    let vmo = self.create_vmo_from_bootfs(&vmo_range, entry.size)?;
                    seen_ranges.insert(
                        vmo_range,
                        VmoWithSize {
                            vmo: vmo
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .map_err(BootfsError::DuplicateHandle)?,
                            content_size: entry.size,
                        },
                    );
                    vmo
                }
            };
            if matches!(vmo.get_name(), Ok(s) if s == "") {
                vmo.set_name(&zx::Name::new_lossy(&format!("{BOOTFS_VMO_NAME}:{}", entry.name)))
                    .map_err(BootfsError::Vmo)?;
            }

            // Execution rights are granted based on the directory name (e.g., 'bin', 'lib')
            // whether the file sits at the root or within a structured package (pkg/name/dir/).
            // Files matching these paths receive ZX_RIGHT_EXECUTE, while all other files
            // have WRITE permissions stripped and are kept strictly read-only.
            let path_parts: Vec<&str> = entry.name.split("/").filter(|&x| !x.is_empty()).collect();
            let is_exec = BootfsSvc::file_in_executable_directory(&path_parts);
            let vmo = if is_exec {
                vmo.replace_as_executable(&vmex).map_err(BootfsError::ExecVmo)?
            } else {
                vmo.replace_handle(zx::Rights::VMO_DEFAULT - zx::Rights::WRITE)
                    .map_err(BootfsError::Vmo)?
            };

            let dir_entry = BootfsSvc::create_dir_entry(
                vmo,
                is_exec,
                BootfsSvc::get_next_inode(&mut self.next_inode),
            );
            self.tree_builder.add_entry(&path_parts, dir_entry)?;
        }

        Ok(self)
    }

    // Publish a VMO beneath '/boot/kernel'. Used to publish VDSOs and kernel files.
    pub fn publish_kernel_vmo(mut self, vmo: zx::Vmo) -> Result<Self, BootfsError> {
        let name = vmo.get_name().map_err(BootfsError::VmoName)?.to_string();
        if name.is_empty() {
            // Skip VMOs without names.
            return Ok(self);
        }
        let path = format!("{}{}", KERNEL_VMO_SUBDIRECTORY, name);
        let mut path_parts: Vec<&str> = path.split("/").filter(|&x| !x.is_empty()).collect();

        // There is special handling for the crashlog.
        if path_parts.len() > 1 && path_parts[path_parts.len() - 1] == KERNEL_CRASHLOG_NAME {
            path_parts = LAST_PANIC_FILEPATH.split("/").filter(|&x| !x.is_empty()).collect();
        }

        let vmo_size = vmo.get_size().map_err(BootfsError::Vmo)?;
        if vmo_size == 0 {
            // Skip empty VMOs.
            return Ok(self);
        }

        // If content size is zero, set it to the size of the VMO.
        if vmo.get_content_size().map_err(BootfsError::Vmo)? == 0 {
            vmo.set_content_size(&vmo_size).map_err(BootfsError::Vmo)?;
        }

        let info = vmo.basic_info().map_err(BootfsError::Vmo)?;
        let is_exec = info.rights.contains(zx::Rights::EXECUTE);

        let dir_entry = BootfsSvc::create_dir_entry(
            vmo,
            is_exec,
            BootfsSvc::get_next_inode(&mut self.next_inode),
        );
        self.tree_builder.add_entry(&path_parts, dir_entry)?;

        Ok(self)
    }

    /// Publish all VMOs of a given type provided to this process through its processargs
    /// bootstrap message. An initial index can be provided to skip handles that were already
    /// taken.
    pub fn publish_kernel_vmos(
        mut self,
        handle_type: HandleType,
        first_index: u16,
    ) -> Result<Self, BootfsError> {
        info!(
            "[BootfsSvc] Adding kernel VMOs of type {:?} starting at index {}.",
            handle_type, first_index
        );
        // The first handle may not be at index 0 if we have already taken it previously.
        let mut index = first_index;
        loop {
            let vmo = take_startup_handle(HandleInfo::new(handle_type, index)).map(zx::Vmo::from);
            match vmo {
                Some(vmo) => {
                    index += 1;
                    self = self.publish_kernel_vmo(vmo)?;
                }
                None => break,
            }
        }

        Ok(self)
    }

    pub fn create_and_bind_vfs(mut self) -> Result<(), BootfsError> {
        info!("[BootfsSvc] Finalizing rust bootfs service.");

        let (directory, directory_server_end) = fidl::endpoints::create_endpoints();

        let mut get_inode = |_: &str| -> u64 { BootfsSvc::get_next_inode(&mut self.next_inode) };

        let vfs = self.tree_builder.build_with_inode_generator(&mut get_inode);

        // Run the service with its own executor to avoid reentrancy issues.
        std::thread::spawn(move || {
            const FLAGS: fio::Flags = fio::PERM_READABLE.union(fio::PERM_EXECUTABLE);
            fasync::LocalExecutorBuilder::new().build().run_singlethreaded(async move {
                let scope = ExecutionScope::new();
                FLAGS
                    .to_object_request(directory_server_end)
                    .handle_async(async |object_request| {
                        ImmutableConnection::create(scope.clone(), vfs, FLAGS, object_request).await
                    })
                    .await;
                scope.wait().await;
            });
        });

        let ns = fdio::Namespace::installed().map_err(BootfsError::Namespace)?;
        assert!(
            ns.unbind("/boot").is_err(),
            "No filesystem should already be bound to /boot when BootfsSvc is starting."
        );

        ns.bind("/boot", directory).map_err(BootfsError::Namespace)?;

        info!("[BootfsSvc] Bootfs is ready and is now serving /boot.");

        Ok(())
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use fuchsia_bootfs::BootfsEntry;
    use std::fs::File;
    use std::io::Read;

    // This uses a test bootfs image containing synthetic data from
    // //src/sys/lib/fuchsia-bootfs.
    static BASIC_BOOTFS_UNCOMPRESSED_FILE: &str = "/pkg/data/basic.bootfs.uncompressed";

    // Returns the number of total committed bytes in this VMO.
    fn committed_bytes(bootfs_vmo: &zx::Vmo) -> u64 {
        let info = bootfs_vmo.info().unwrap();
        // Assert the kernel did not make any changes that would have altered
        // committed_change_events (this value does not include user-triggered
        // events) .
        assert_eq!(info.committed_change_events, 0);
        info.committed_bytes
    }

    fn read_file_to_vmo(path: &str) -> zx::Vmo {
        let mut file_buffer = Vec::new();
        File::open(path).unwrap().read_to_end(&mut file_buffer).unwrap();
        let vmo = zx::Vmo::create(file_buffer.len() as u64).unwrap();
        vmo.write(&file_buffer, 0).unwrap();
        vmo
    }

    async fn open_file_to_read(dir: &fio::DirectoryProxy, name: &str) -> fio::FileProxy {
        fuchsia_fs::directory::open_file(&dir, &name, fio::PERM_READABLE).await.unwrap()
    }

    fn parsed_payload(entry: &BootfsEntry) -> String {
        String::from_utf8(entry.payload.clone().unwrap()).unwrap()
    }

    // Test that bootfs entries are all parsed into true copy VMOs and inserted
    // into the BootfsVFS, and that the Bootfs VMO is decommitted.
    #[fuchsia::test]
    async fn bootfs_is_parsed_and_decommitted() {
        let bootfs_vmo = read_file_to_vmo(BASIC_BOOTFS_UNCOMPRESSED_FILE);
        let bootfs_svc = BootfsSvc::new_internal(bootfs_vmo).unwrap();
        let entries = bootfs_svc.parser.iter().map(|e| e.unwrap()).collect::<Vec<BootfsEntry>>();

        // Save a copy of the Bootfs handle so we can check that this region is
        // decommitted later.
        let bootfs_dup =
            bootfs_svc.bootfs.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into();
        // All of the bootfs VMO should be committed.
        assert_eq!(committed_bytes(&bootfs_dup), bootfs_dup.get_size().unwrap());

        bootfs_svc
            .ingest_bootfs_vmo(Resource::from(zx::NullableHandle::invalid()), Vec::new())
            .unwrap()
            .create_and_bind_vfs()
            .unwrap();

        let boot_proxy =
            fuchsia_fs::directory::open_in_namespace("/boot", fio::PERM_READABLE).unwrap();

        // Confirm that each entry from the uncompressed Bootfs is inserted into
        // the BootfsVFS as distinct copies.
        for entry in &entries {
            let file = open_file_to_read(&boot_proxy, &entry.name).await;
            let contents = fuchsia_fs::file::read(&file).await.unwrap();
            assert_eq!(entry.payload.as_ref().unwrap(), &contents);
            // Assert that the VMO handle backing the file is a fully-owned VMO
            // (i.e. it does not have a parent VMO).
            let vmo = file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().unwrap();
            assert_eq!(vmo.info().unwrap().parent_koid, zx::Koid::from_raw(0));
            assert_eq!(
                vmo.info().unwrap().name.to_string(),
                // VMO are reused between files with the same content which can causes a mismatch.
                format!(
                    "bootfs:{}",
                    match entry.name.as_str() {
                        "empty" => "dir/empty",
                        "simple.txt" => "dir/simple-copy.txt",
                        _ => &entry.name,
                    }
                )
            );
        }

        // Confirm that the only committed bytes of the Bootfs VMO is up to the
        // start of the VMO data.
        let data_start = entries.first().unwrap().offset;
        let data_size = bootfs_dup.get_size().unwrap() - data_start;
        assert_eq!(committed_bytes(&bootfs_dup), data_start);

        // Expect the entire data VMO region to be zero-ed out.
        let content = bootfs_dup.read_to_vec::<u8>(data_start, data_size).unwrap();
        assert!(content.iter().all(|b| *b == 0x0));
    }

    // Test that bootfs entries that share the same offset are parsed as
    // distinct VMOs.
    #[fuchsia::test]
    async fn shared_bootfs_offsets() {
        let bootfs_vmo = read_file_to_vmo(BASIC_BOOTFS_UNCOMPRESSED_FILE);
        let bootfs_svc = BootfsSvc::new_internal(bootfs_vmo).unwrap();
        let entries = bootfs_svc.parser.iter().map(|e| e.unwrap()).collect::<Vec<BootfsEntry>>();

        // `entry1` is just an empty file, so the offset for this entry is the
        // same as `entry2`, even if the entry.size of `entry1` is 0 and the
        // entry.size of `entry2` is > 0.
        let entry1 = entries.iter().find(|e| e.name == "dir/empty").unwrap();
        let entry2 = entries.iter().find(|e| e.name == "dir/lorem.txt").unwrap();
        assert_eq!(entry1.offset, entry2.offset);

        // Confirm the payload copied from the bootfs VMO is as expected.
        assert!(parsed_payload(entry1).is_empty());
        assert!(parsed_payload(entry2).starts_with("Lorem ipsum"));

        bootfs_svc
            .ingest_bootfs_vmo(Resource::from(zx::NullableHandle::invalid()), Vec::new())
            .unwrap()
            .create_and_bind_vfs()
            .unwrap();

        let boot_proxy =
            fuchsia_fs::directory::open_in_namespace("/boot", fio::PERM_READABLE).unwrap();

        // Make sure entry1 was translated correctly to the VFS.
        let entry1_file = open_file_to_read(&boot_proxy, &entry1.name).await;
        let entry1_contents = fuchsia_fs::file::read_to_string(&entry1_file).await.unwrap();
        assert_eq!(entry1_contents, parsed_payload(entry1));

        // Make sure entry2 was translated correctly to the VFS.
        let entry2_file = open_file_to_read(&boot_proxy, &entry2.name).await;
        let entry2_contents = fuchsia_fs::file::read_to_string(&entry2_file).await.unwrap();
        assert_eq!(entry2_contents, parsed_payload(entry2));

        // Re-confirm the content of the VFS entries are as expected.
        assert_ne!(entry1_contents, entry2_contents);
        assert!(entry1_contents.is_empty());
        assert!(entry2_contents.starts_with("Lorem ipsum"));
    }

    // Test that a duplicated handle is given to different bootfs entries that
    // point to the same VMO content range.
    #[fuchsia::test]
    async fn shared_bootfs_ranges() {
        let bootfs_vmo = read_file_to_vmo(BASIC_BOOTFS_UNCOMPRESSED_FILE);
        let bootfs_svc = BootfsSvc::new_internal(bootfs_vmo).unwrap();
        let entries = bootfs_svc.parser.iter().map(|e| e.unwrap()).collect::<Vec<BootfsEntry>>();

        // `entry1` and `entry2` share the same parsed VMO offset and size under
        // different named entries.
        let entry1 = entries.iter().find(|e| e.name == "simple.txt").unwrap();
        let entry2 = entries.iter().find(|e| e.name == "dir/simple-copy.txt").unwrap();
        assert_eq!(entry1.offset, entry2.offset);
        assert_eq!(entry1.size, entry2.size);
        assert_eq!(parsed_payload(entry1), parsed_payload(entry2));

        bootfs_svc
            .ingest_bootfs_vmo(Resource::from(zx::NullableHandle::invalid()), Vec::new())
            .unwrap()
            .create_and_bind_vfs()
            .unwrap();

        let boot_proxy =
            fuchsia_fs::directory::open_in_namespace("/boot", fio::PERM_READABLE).unwrap();

        // Make sure entry1 was translated correctly to the VFS.
        let entry1_file = open_file_to_read(&boot_proxy, &entry1.name).await;
        let entry1_contents = fuchsia_fs::file::read_to_string(&entry1_file).await.unwrap();
        assert_eq!(entry1_contents, parsed_payload(entry1));

        // Make sure entry2 was translated correctly to the VFS.
        let entry2_file = open_file_to_read(&boot_proxy, &entry2.name).await;
        let entry2_contents = fuchsia_fs::file::read_to_string(&entry2_file).await.unwrap();
        assert_eq!(entry2_contents, parsed_payload(entry2));

        // Re-confirm the content of the VFS entries are the same.
        assert_eq!(entry1_contents, entry2_contents);

        // Test that both VFS entries share the same memory backing VMO.
        let entry1_vmo =
            entry1_file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().unwrap();
        let entry1_vmo_info = entry1_vmo.info().unwrap();

        let entry2_vmo =
            entry2_file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().unwrap();
        let entry2_vmo_info = entry2_vmo.info().unwrap();

        // Expect the same KOID for both VMO handles.
        assert_eq!(entry1_vmo_info.koid, entry2_vmo_info.koid);
    }

    // Test that bootfs entries that were handed over are used instead of creating new VMOs.
    #[fuchsia::test]
    async fn provided_bootfs_entries() {
        let bootfs_vmo = read_file_to_vmo(BASIC_BOOTFS_UNCOMPRESSED_FILE);
        let bootfs_vmo_handle = bootfs_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let bootfs_svc = BootfsSvc::new_internal(bootfs_vmo).unwrap();
        let entries = bootfs_svc.parser.iter().map(|e| e.unwrap()).collect::<Vec<BootfsEntry>>();

        // `entry1` and `entry2` share the same parsed VMO offset and size under
        // different named entries.
        let entry1 = entries.iter().find(|e| e.name == "simple.txt").unwrap();
        let entry2 = entries.iter().find(|e| e.name == "dir/lorem.txt").unwrap();
        assert_ne!(entry1.offset, entry2.offset);

        // VMO from original bootfs_vmo and transfer bytes.

        let entry1_range = aligned_range(entry1.offset as u32, entry1.size as u32).unwrap();
        let entry1_bootfs_vmo = zx::Vmo::create_with_opts(
            zx::VmoOptions::RESIZABLE,
            entry1_range.end - entry1_range.start,
        )
        .unwrap();
        assert_eq!(entry1_bootfs_vmo.set_size(entry1.size).is_ok(), true);
        assert_eq!(
            entry1_bootfs_vmo
                .transfer_data(
                    zx::TransferDataOptions::empty(),
                    0,
                    entry1_range.end - entry1_range.start,
                    &bootfs_vmo_handle,
                    entry1_range.start
                )
                .is_ok(),
            true
        );
        let entry1_bootfs_vmo_koid = entry1_bootfs_vmo.koid().unwrap();

        let mut userboot_bootfs_entries = Vec::new();
        userboot_bootfs_entries
            .push(BootfsFileVmo { offset: entry1.offset as u32, contents: entry1_bootfs_vmo });

        bootfs_svc
            .ingest_bootfs_vmo(
                Resource::from(zx::NullableHandle::invalid()),
                userboot_bootfs_entries,
            )
            .unwrap()
            .create_and_bind_vfs()
            .unwrap();

        let boot_proxy =
            fuchsia_fs::directory::open_in_namespace("/boot", fio::PERM_READABLE).unwrap();

        // Make sure entry1 was translated correctly to the VFS.
        let entry1_file = open_file_to_read(&boot_proxy, &entry1.name).await;
        let entry1_contents = fuchsia_fs::file::read_to_string(&entry1_file).await.unwrap();
        assert_eq!(entry1_contents, parsed_payload(entry1));

        // Make sure entry2 was translated correctly to the VFS.
        let entry2_file = open_file_to_read(&boot_proxy, &entry2.name).await;
        let entry2_contents = fuchsia_fs::file::read_to_string(&entry2_file).await.unwrap();
        assert_eq!(entry2_contents, parsed_payload(entry2));

        // Re-confirm the content of the VFS entries are the same.
        assert_ne!(entry1_contents, entry2_contents);

        // Test that both VFS entries share the same memory backing VMO.
        let entry1_vmo =
            entry1_file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().unwrap();
        let entry1_vmo_info = entry1_vmo.info().unwrap();

        let entry2_vmo =
            entry2_file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().unwrap();
        let entry2_vmo_info = entry2_vmo.info().unwrap();

        // Expect different backing VMOs for both files.
        assert_ne!(entry1_vmo_info.koid, entry2_vmo_info.koid);

        // Expect same backing VMO for the provided vmo and the vmo handed over.
        assert_eq!(entry1_vmo_info.koid, entry1_bootfs_vmo_koid);
    }
}
