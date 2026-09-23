// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use ext4_lib::parser::{Parser as ExtParser, XattrMap as ExtXattrMap};
use ext4_lib::readers::VmoReader;
use ext4_lib::structs::{EntryType, INode, ROOT_INODE_NUM};
use once_cell::sync::OnceCell;
use starnix_core::mm::ProtectionFlags;
use starnix_core::mm::memory::MemoryObject;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    CacheMode, DEFAULT_BYTES_PER_BLOCK, DirectoryEntryType, DirentSink, FileObject, FileOps,
    FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsNode, FsNodeFlags,
    FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, FsString, MemoryRegularFile, SeekTarget,
    SymlinkTarget, XattrOp, XattrStorage, default_seek, fileops_impl_directory,
    fileops_impl_noop_sync, fs_node_impl_dir_readonly, fs_node_impl_not_dir, fs_node_impl_symlink,
    fs_node_impl_xattr_delegate,
};
use starnix_logging::{impossible_error, track_stub};

use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::mount_flags::FileSystemFlags;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{EXT4_SUPER_MAGIC, errno, error, ino_t, off_t, statfs};
use std::sync::Arc;
use std::sync::atomic::Ordering;

mod pager;

use pager::{Pager, PagerExtent};

pub struct ExtFilesystem {
    parser: ExtParser,
    pager: Arc<Pager>,
}

impl FileSystemOps for ExtFilesystem {
    fn name(&self) -> &'static FsStr {
        "ext4".into()
    }

    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(EXT4_SUPER_MAGIC))
    }
}

struct ExtNode {
    inode_num: u32,
    inode: INode,
    xattrs: ExtXattrMap,
}

impl ExtFilesystem {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let mut open_flags = OpenFlags::RDWR;
        let mut prot_flags = ProtectionFlags::READ | ProtectionFlags::WRITE | ProtectionFlags::EXEC;
        if options.flags.load(Ordering::Relaxed).contains(FileSystemFlags::RDONLY) {
            open_flags = OpenFlags::RDONLY;
            prot_flags ^= ProtectionFlags::WRITE;
        }

        let source_device = current_task.open_file(options.source.as_ref(), open_flags)?;

        // Note that we *require* get_memory to work here for performance reasons.  Fallback to
        // FIDL-based read/write API is not an option.
        let memory = source_device.get_memory(current_task, None, prot_flags)?;
        let pager_vmo = memory
            .as_vmo()
            .ok_or_else(|| errno!(EINVAL))?
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(impossible_error)?;
        let parser_vmo = Arc::new(
            memory
                .as_vmo()
                .ok_or_else(|| errno!(EINVAL))?
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(impossible_error)?,
        );
        let parser = ExtParser::new(Box::new(VmoReader::new(parser_vmo)));
        let pager =
            Arc::new(Pager::new(pager_vmo, parser.block_size().map_err(|e| errno!(EIO, e))?)?);
        let fs = Self { parser, pager };
        let ops = ExtDirectory { inner: Arc::new(ExtNode::new(&fs, ROOT_INODE_NUM)?) };
        let fs = FileSystem::new(
            current_task.kernel(),
            CacheMode::Cached(current_task.kernel().fs_cache_config()),
            fs,
            options,
        )?;
        fs.create_root(ROOT_INODE_NUM as ino_t, ops);
        Ok(fs)
    }
}

impl ExtNode {
    fn new(fs: &ExtFilesystem, inode_num: u32) -> Result<ExtNode, Errno> {
        let inode = fs.parser.inode(inode_num).map_err(|e| errno!(EIO, e))?;
        let xattrs = fs.parser.inode_xattrs(inode_num).unwrap_or_default();
        Ok(ExtNode { inode_num, inode, xattrs })
    }
}

impl XattrStorage for ExtNode {
    fn list_xattrs(&self) -> Result<Vec<FsString>, Errno> {
        Ok(self.xattrs.keys().map(|k| k.clone().into()).collect())
    }

    fn get_xattr(&self, name: &FsStr) -> Result<FsString, Errno> {
        self.xattrs.get(&**name).map(|a| a.clone().into()).ok_or_else(|| errno!(ENODATA))
    }

    fn set_xattr(&self, _name: &FsStr, _value: &FsStr, _op: XattrOp) -> Result<(), Errno> {
        error!(ENOSYS)
    }
    fn remove_xattr(&self, _name: &FsStr) -> Result<(), Errno> {
        error!(ENOSYS)
    }
}

struct ExtDirectory {
    inner: Arc<ExtNode>,
}

impl FsNodeOps for ExtDirectory {
    fs_node_impl_dir_readonly!();
    fs_node_impl_xattr_delegate!(self, self.inner);

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(ExtDirFileObject { inner: self.inner.clone() }))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let fs = node.fs();
        let fs_ops = fs.downcast_ops::<ExtFilesystem>().unwrap();
        let dir_entries =
            fs_ops.parser.entries_from_inode(&self.inner.inode).map_err(|e| errno!(EIO, e))?;
        let entry = dir_entries
            .iter()
            .find(|e| e.name_bytes() == name)
            .ok_or_else(|| errno!(ENOENT, name))?;
        let ext_node = ExtNode::new(fs_ops, entry.e2d_ino.into())?;
        let inode_num = ext_node.inode_num as ino_t;
        fs.get_or_create_node(inode_num, || {
            let entry_type = EntryType::from_u8(entry.e2d_type).map_err(|e| errno!(EIO, e))?;
            let mode = FileMode::from_bits(ext_node.inode.e2di_mode.into());

            let uid = get_uid_from_node(&ext_node);
            let gid = get_gid_from_node(&ext_node);
            let owner = FsCred { uid, gid };

            let size = get_size_from_node(&ext_node, &mode);
            let blocks = get_blocks_from_node(&ext_node);
            let nlink = ext_node.inode.e2di_nlink.into();

            let ops: Box<dyn FsNodeOps> = match entry_type {
                EntryType::RegularFile => Box::new(ExtFile::new(ext_node, name.to_owned())),
                EntryType::Directory => Box::new(ExtDirectory { inner: Arc::new(ext_node) }),
                EntryType::SymLink => Box::new(ExtSymlink { inner: ext_node }),
                EntryType::Unknown => {
                    track_stub!(TODO("https://fxbug.dev/322873719"), "ext4 unknown entry type");
                    Box::new(ExtFile::new(ext_node, name.to_owned()))
                }
                EntryType::CharacterDevice => {
                    track_stub!(TODO("https://fxbug.dev/322874445"), "ext4 character device");
                    Box::new(ExtFile::new(ext_node, name.to_owned()))
                }
                EntryType::BlockDevice => {
                    track_stub!(TODO("https://fxbug.dev/322874062"), "ext4 block device");
                    Box::new(ExtFile::new(ext_node, name.to_owned()))
                }
                EntryType::FIFO => {
                    track_stub!(TODO("https://fxbug.dev/322874249"), "ext4 fifo");
                    Box::new(ExtFile::new(ext_node, name.to_owned()))
                }
                EntryType::Socket => {
                    track_stub!(TODO("https://fxbug.dev/322874081"), "ext4 socket");
                    Box::new(ExtFile::new(ext_node, name.to_owned()))
                }
            };

            let child = FsNode::new_uncached(
                inode_num,
                ops,
                &fs,
                FsNodeInfo { mode, uid: owner.uid, gid: owner.gid, ..Default::default() },
                FsNodeFlags::empty(),
            );
            child.update_info(|info| {
                info.size = size as usize;
                info.link_count = nlink;
                info.blksize = DEFAULT_BYTES_PER_BLOCK;
                info.blocks = blocks as usize;
            });
            Ok(child)
        })
    }
}

fn merge_low_high_16(low: u32, high: u32) -> u32 {
    low | (high << 16)
}

fn merge_low_high_32(low: u64, high: u64) -> u64 {
    low | (high << 32)
}

fn get_uid_from_node(ext_node: &ExtNode) -> u32 {
    let uid_lower: u32 = ext_node.inode.e2di_uid.into();
    let uid_upper: u32 = ext_node.inode.e2di_uid_high.into();
    merge_low_high_16(uid_lower, uid_upper)
}

fn get_gid_from_node(ext_node: &ExtNode) -> u32 {
    let gid_lower: u32 = ext_node.inode.e2di_gid.into();
    let gid_upper: u32 = ext_node.inode.e2di_gid_high.into();
    merge_low_high_16(gid_lower, gid_upper)
}

fn get_size_from_node(ext_node: &ExtNode, mode: &FileMode) -> u64 {
    if mode.is_reg() {
        let size_lower: u64 = ext_node.inode.e2di_size.into();
        let size_upper: u64 = ext_node.inode.e2di_size_high.into();
        merge_low_high_32(size_lower, size_upper)
    } else {
        ext_node.inode.e2di_size.into()
    }
}

fn get_blocks_from_node(ext_node: &ExtNode) -> u64 {
    let blocks_lower: u64 = ext_node.inode.e2di_nblock.into();
    let blocks_upper: u64 = ext_node.inode.e2di_nblock_high.into();
    merge_low_high_32(blocks_lower, blocks_upper)
}

struct ExtFile {
    inner: ExtNode,
    name: FsString,

    // The VMO here will be a child of the main VMO that the pager holds.  We want to keep it here
    // so that whilst ExtFile remains resident, we hold a child reference to the main VMO which
    // will prevent the pager from dropping the VMO (and any data we might have paged-in).
    memory: OnceCell<Arc<MemoryObject>>,
}

impl ExtFile {
    fn new(inner: ExtNode, name: FsString) -> Self {
        ExtFile { inner, name, memory: OnceCell::new() }
    }
}

impl FsNodeOps for ExtFile {
    fs_node_impl_not_dir!();
    fs_node_impl_xattr_delegate!(self, self.inner);

    fn create_file_ops(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let fs = node.fs();
        let fs_ops = fs.downcast_ops::<ExtFilesystem>().unwrap();
        let inode_num = self.inner.inode_num;
        let memory = self.memory.get_or_try_init(|| {
            let (file_size, extents) = fs_ops
                .parser
                .read_extents(self.inner.inode_num)
                .map_err(|e| errno!(EINVAL, format!("failed to read extents: {e}")))?;
            // The extents should be sorted which we rely on later.
            let mut pager_extents = Vec::with_capacity(extents.len());
            let mut last_block = 0;
            for e in extents {
                let pager_extent = PagerExtent::from(e);
                if pager_extent.logical.start < last_block {
                    return error!(EIO, "Bad extent");
                }
                last_block = pager_extent.logical.end;
                pager_extents.push(pager_extent);
            }
            Ok(Arc::new(MemoryObject::from(
                fs_ops
                    .pager
                    .register(self.name.as_ref(), inode_num, file_size, &pager_extents)
                    .map_err(|e| errno!(EINVAL, e))?,
            )))
        })?;

        // TODO(https://fxbug.dev/42080696) returned memory shouldn't be writeable
        Ok(Box::new(MemoryRegularFile::new(memory.clone())))
    }
}

impl From<ext4_lib::structs::Extent> for PagerExtent {
    fn from(e: ext4_lib::structs::Extent) -> Self {
        let block_count: u16 = e.e_len.into();
        let start = e.e_blk.into();
        Self { logical: start..start + block_count as u32, physical_block: e.target_block_num() }
    }
}

struct ExtSymlink {
    inner: ExtNode,
}

impl FsNodeOps for ExtSymlink {
    fs_node_impl_symlink!();
    fs_node_impl_xattr_delegate!(self, self.inner);

    fn readlink(&self, node: &FsNode, _current_task: &CurrentTask) -> Result<SymlinkTarget, Errno> {
        let fs = node.fs();
        let fs_ops = fs.downcast_ops::<ExtFilesystem>().unwrap();
        let data = fs_ops.parser.read_data(self.inner.inode_num).map_err(|e| errno!(EIO, e))?;
        Ok(SymlinkTarget::Path(data.into()))
    }
}

struct ExtDirFileObject {
    inner: Arc<ExtNode>,
}

impl FileOps for ExtDirFileObject {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        Ok(default_seek(current_offset, target, || error!(EINVAL))?)
    }

    fn readdir(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        let fs = file.node().fs();
        let fs_ops = fs.downcast_ops::<ExtFilesystem>().unwrap();
        let dir_entries =
            fs_ops.parser.entries_from_inode(&self.inner.inode).map_err(|e| errno!(EIO, e))?;

        if sink.offset() as usize >= dir_entries.len() {
            return Ok(());
        }

        for entry in dir_entries[(sink.offset() as usize)..].iter() {
            let inode_num = entry.e2d_ino.into();
            let entry_type = directory_entry_type(
                EntryType::from_u8(entry.e2d_type).map_err(|e| errno!(EIO, e))?,
            );
            sink.add(inode_num, sink.offset() + 1, entry_type, entry.name_bytes().into())?;
        }
        Ok(())
    }
}

fn directory_entry_type(entry_type: EntryType) -> DirectoryEntryType {
    match entry_type {
        EntryType::Unknown => DirectoryEntryType::UNKNOWN,
        EntryType::RegularFile => DirectoryEntryType::REG,
        EntryType::Directory => DirectoryEntryType::DIR,
        EntryType::CharacterDevice => DirectoryEntryType::CHR,
        EntryType::BlockDevice => DirectoryEntryType::BLK,
        EntryType::FIFO => DirectoryEntryType::FIFO,
        EntryType::Socket => DirectoryEntryType::SOCK,
        EntryType::SymLink => DirectoryEntryType::LNK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ext4_lib::structs::INode;
    use starnix_uapi::file_mode::mode;
    use zerocopy::FromBytes;
    use zerocopy::byteorder::little_endian::{U16 as LE16, U32 as LE32};

    fn default_inode() -> INode {
        let zero = vec![0; 160];
        INode::read_from_bytes(&zero).expect("failed to read from bytes")
    }

    fn create_test_ext_node(inode: INode) -> ExtNode {
        ExtNode { inode_num: 1, inode, xattrs: ExtXattrMap::default() }
    }

    #[test]
    fn test_get_uid_from_node() {
        let mut inode = default_inode();
        inode.e2di_uid = LE16::new(1001);
        inode.e2di_uid_high = LE16::new(1);
        let node = create_test_ext_node(inode);
        assert_eq!(get_uid_from_node(&node), (1 << 16) | 1001);
    }

    #[test]
    fn test_get_gid_from_node() {
        let mut inode = default_inode();
        inode.e2di_gid = LE16::new(1002);
        inode.e2di_gid_high = LE16::new(2);
        let node = create_test_ext_node(inode);
        assert_eq!(get_gid_from_node(&node), (2 << 16) | 1002);
    }

    #[test]
    fn test_get_size_from_node() {
        // Test with a regular file.
        let mut inode = default_inode();
        inode.e2di_size = LE32::new(0x12345678);
        inode.e2di_size_high = LE32::new(0x9);
        let node = create_test_ext_node(inode);
        let mode = mode!(IFREG, 0o777);
        assert_eq!(get_size_from_node(&node, &mode), (0x9 << 32) | 0x12345678);

        // Test with a directory, where size_high should be ignored.
        let mode = mode!(IFDIR, 0o777);
        assert_eq!(get_size_from_node(&node, &mode), 0x12345678);
    }

    #[test]
    fn test_get_blocks_from_node() {
        let mut inode = default_inode();
        inode.e2di_nblock = LE32::new(0xABCDE);
        inode.e2di_nblock_high = LE16::new(0x3);
        let node = create_test_ext_node(inode);
        assert_eq!(get_blocks_from_node(&node), (0x3 << 32) | 0xABCDE);
    }
}
