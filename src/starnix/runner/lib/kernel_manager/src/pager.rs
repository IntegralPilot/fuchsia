// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This supports paging for ext4 files.  `zx_pager_supply_pages` requires is to transfer pages to
//! the target, hence the need for a transfer VMO.  This also uses a static zeroed VMO to transfer
//! pages that should be zeroed.

use fidl_fuchsia_starnix_runner as fstarnixrunner;
use futures::TryStreamExt;
use starnix_logging::{log_debug, log_error, log_warn, with_zx_name};
use starnix_sync::{LockDepMutex, PagerFilesByInodeLock, PagerFilesystemsLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use zx::sys::zx_page_request_command_t::{ZX_PAGER_VMO_COMPLETE, ZX_PAGER_VMO_READ};

// N.B. At time of writing, no particular science has gone into picking these numbers; tweaking
// these numbers might or might not give us better performance.
const PAGER_THREADS: usize = 1;
const TRANSFER_VMO_SIZE: u64 = 1 * 1024 * 1024;
const ZERO_VMO_SIZE: u64 = 1 * 1024 * 1024;

/// Tracing category used to trace the pager.
const CATEGORY_STARNIX_PAGER: &'static str = "starnix:pager";

pub async fn run_pager(
    pager_request: fstarnixrunner::ManagerCreatePagerRequest,
    pager: Arc<Pager>,
) {
    let fstarnixrunner::ManagerCreatePagerRequest {
        backing_vmo: Some(backing_vmo),
        block_size: Some(block_size),
        pager: Some(pager_server),
        ..
    } = pager_request
    else {
        log_error!("Invalid create pager request");
        return;
    };

    let filesystem = Arc::new(match Filesystem::new(pager.clone(), backing_vmo, block_size) {
        Ok(filesystem) => filesystem,
        Err(error) => {
            log_error!("Unable to register filesystem {error}");
            return;
        }
    });

    pager.add_filesystem(filesystem.clone());

    let mut stream = pager_server.into_stream();
    'outer: while let Ok(Some(event)) = stream.try_next().await {
        match event {
            fstarnixrunner::PagerRequest::RegisterFile {
                payload:
                    fstarnixrunner::PagerRegisterFileRequest {
                        name: Some(name),
                        inode_num: Some(inode_num),
                        size: Some(size),
                        extents: Some(extents),
                        ..
                    },
                responder,
                ..
            } => {
                fuchsia_trace::instant!(
                    CATEGORY_STARNIX_PAGER,
                    "file_register",
                    fuchsia_trace::Scope::Thread
                );
                match filesystem.register(
                    &name,
                    inode_num,
                    size,
                    extents
                        .iter()
                        .map(|e| PagerExtent {
                            logical: e.logical_start..e.logical_end,
                            physical_block: e.physical_block,
                        })
                        .collect(),
                ) {
                    Ok(vmo) => {
                        match responder.send(Ok(fstarnixrunner::PagerRegisterFileResponse {
                            vmo: Some(vmo),
                            ..Default::default()
                        })) {
                            Ok(_) => {}
                            Err(e) => {
                                log_error!("Error sending pager response {:?}", e);
                                break 'outer;
                            }
                        }
                    }
                    Err(e) => match responder.send(Err(e.into_raw())) {
                        Ok(_) => {}
                        Err(e) => {
                            log_error!("Error sending pager error response {:?}", e);
                            break 'outer;
                        }
                    },
                };
            }
            fstarnixrunner::PagerRequest::RegisterFile { .. } => {
                log_error!("Invalid RegisterFile request");
                break 'outer;
            }
            _ => {}
        }
    }
    pager.remove_filesystem(&*filesystem);
}

/// A simple pager implementation. One pager can serve multiple filesystems.
pub struct Pager {
    pager: zx::Pager,
    port: zx::Port,
    zero_vmo: zx::Vmo,
    next_filesystem_id: AtomicU32,
    filesystems: LockDepMutex<HashMap<u32, Arc<Filesystem>>, PagerFilesystemsLock>,
}

impl Pager {
    pub fn new() -> Result<Self, Errno> {
        Ok(Self {
            pager: zx::Pager::create(zx::PagerOptions::empty()).map_err(|error| {
                log_error!(error:?; "Pager::create failed");
                errno!(EINVAL)
            })?,
            port: zx::Port::create(),
            zero_vmo: with_zx_name(
                zx::Vmo::create(ZERO_VMO_SIZE).map_err(|_| errno!(EINVAL))?,
                b"starnix:ext4",
            ),
            next_filesystem_id: AtomicU32::new(1),
            filesystems: Default::default(),
        })
    }

    /// Starts the pager threads.
    pub fn start_threads(self: &Arc<Self>) {
        for i in 0..PAGER_THREADS {
            let this = self.clone();
            let _ = std::thread::Builder::new().name(format!("pager-{}", i)).spawn(move || {
                this.run_pager_thread();
            });
        }
    }

    /// Dedicated thread responsible for listening on port and supplying pages as needed.
    /// More than one pager thread can be running concurrently.
    pub fn run_pager_thread(&self) {
        let transfer_vmo = with_zx_name(
            zx::Vmo::create(TRANSFER_VMO_SIZE).expect("unable to create transfer vmo"),
            b"starnix:ext4",
        );
        let transfer_vmo_addr = fuchsia_runtime::vmar_root_self()
            .map(
                0,
                &transfer_vmo,
                0,
                TRANSFER_VMO_SIZE as usize,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::ALLOW_FAULTS,
            )
            .expect("unable to map transfer vmo");
        scopeguard::defer!({
            // SAFETY: We mapped the VMO above.
            let _ = unsafe {
                fuchsia_runtime::vmar_root_self()
                    .unmap(transfer_vmo_addr, TRANSFER_VMO_SIZE as usize)
            };
        });
        let split_key = |key: u64| -> (u32, u32) { ((key >> 32) as u32, key as u32) };
        loop {
            match self.port.wait(zx::MonotonicInstant::INFINITE) {
                Ok(packet) => {
                    match packet.contents() {
                        zx::PacketContents::Pager(contents)
                            if contents.command() == ZX_PAGER_VMO_READ =>
                        {
                            fuchsia_trace::duration!(CATEGORY_STARNIX_PAGER, "vmo_read");
                            let (filesystem_num, inode_num) = split_key(packet.key());
                            let filesystem = self
                                .filesystems
                                .lock()
                                .get(&filesystem_num)
                                .cloned()
                                .expect("Unexpected packet key");
                            filesystem.receive_pager_packet(
                                inode_num,
                                contents,
                                &transfer_vmo,
                                transfer_vmo_addr,
                            );
                        }
                        zx::PacketContents::Pager(contents)
                            if contents.command() == ZX_PAGER_VMO_COMPLETE =>
                        {
                            fuchsia_trace::duration!(CATEGORY_STARNIX_PAGER, "vmo_complete");
                            // We don't care about this command, but we will receive them and we
                            // don't want to log them as unexpected.
                        }
                        zx::PacketContents::SignalOne(signals)
                            if signals.observed().contains(zx::Signals::VMO_ZERO_CHILDREN) =>
                        {
                            fuchsia_trace::duration!(
                                CATEGORY_STARNIX_PAGER,
                                "signal_zero_children"
                            );
                            let (filesystem_num, inode_num) = split_key(packet.key());
                            // We may get VMO_ZERO_CHILDREN notifications for
                            // files within a filesystem after it is unmounted,
                            // ignore those.
                            let filesystem = self.filesystems.lock().get(&filesystem_num).cloned();
                            if let Some(filesystem) = filesystem {
                                filesystem.on_zero_children(inode_num).expect("on_zero_children");
                            }
                        }
                        zx::PacketContents::User(_) => break,
                        _ => log_error!("Unexpected port packet: {:?}", packet.contents()),
                    }
                }
                Err(error) => log_error!(error:?; "Port::wait failed"),
            }
        }
        log_debug!("Pager thread terminating");
    }

    fn create_pager_vmo(&self, key: u64, size: u64) -> Result<zx::Vmo, zx::Status> {
        self.pager.create_vmo(zx::VmoOptions::RESIZABLE, &self.port, key, size)
    }

    fn allocate_filesystem_id(&self) -> u32 {
        self.next_filesystem_id.fetch_add(1, Ordering::Relaxed)
    }

    fn add_filesystem(&self, filesystem: Arc<Filesystem>) {
        self.filesystems.lock().insert(filesystem.id(), filesystem);
    }

    fn remove_filesystem(&self, filesystem: &Filesystem) {
        self.filesystems.lock().remove(&filesystem.id());
    }

    /// Terminates (asynchronously) the pager threads.
    pub fn terminate(&self) {
        let up = zx::UserPacket::from_u8_array([0; 32]);
        let packet = zx::Packet::from_user_packet(0, 0, up);
        for _ in 0..PAGER_THREADS {
            self.port.queue(&packet).unwrap();
        }
    }
}

/// Filesystem registered with the pager.
pub struct Filesystem {
    pager: Arc<Pager>,
    backing_vmo: zx::Vmo,
    block_size: u64,
    files_by_inode: LockDepMutex<HashMap<u32, Arc<PagedFile>>, PagerFilesByInodeLock>,
    id: u32,
}

impl Filesystem {
    /// Returns a new filesystem.  `block_size` shouldn't be too big (which might cause overflows) and it
    /// should be a power of 2.
    pub fn new(pager: Arc<Pager>, backing_vmo: zx::Vmo, block_size: u64) -> Result<Self, Errno> {
        if block_size > 1024 * 1024 || !block_size.is_power_of_two() {
            return error!(EINVAL, "Bad block size {block_size}");
        }
        let id = pager.allocate_filesystem_id();
        Ok(Self { pager, backing_vmo, block_size, files_by_inode: Default::default(), id })
    }

    /// Registers the file with the pager.  Returns a child VMO.  `extents` should be sorted.
    pub fn register(
        &self,
        name: &str,
        inode_num: u32,
        size: u64,
        extents: Box<[PagerExtent]>,
    ) -> Result<zx::Vmo, zx::Status> {
        let (file, did_create) = {
            match self.files_by_inode.lock().entry(inode_num) {
                Entry::Occupied(o) => (o.get().clone(), false),
                Entry::Vacant(v) => (
                    v.insert(Arc::new(PagedFile {
                        vmo: self
                            .pager
                            .create_pager_vmo(self.port_key_for_inode(inode_num), size)?,
                        extents,
                    }))
                    .clone(),
                    true,
                ),
            }
        };
        let child_vmo = file.vmo.create_child(zx::VmoChildOptions::REFERENCE, 0, 0);
        if did_create {
            let set_up_vmo = |vmo| -> Result<(), zx::Status> {
                self.watch_for_zero_children(vmo, inode_num)?;
                vmo.set_name(&zx::Name::new_lossy(&format!("ext4!{}", name)))?;
                Ok(())
            };

            if let Err(e) = set_up_vmo(&file.vmo) {
                self.files_by_inode.lock().remove(&inode_num);
                return Err(e);
            }
        }
        child_vmo
    }

    fn receive_pager_packet(
        &self,
        inode_num: u32,
        contents: zx::PagerPacket,
        transfer_vmo: &zx::Vmo,
        transfer_vmo_addr: usize,
    ) {
        let Some(file) = self.files_by_inode.lock().get(&inode_num).cloned() else {
            return;
        };

        let requested_range = contents.range();

        // Align the read to 128 KiB slots (readahead).
        const ALIGNMENT: u64 = 128 * 1024;
        let readahead_start = (requested_range.start / ALIGNMENT) * ALIGNMENT;
        let mut readahead_end = requested_range.end.next_multiple_of(ALIGNMENT);

        // Clamp to VMO size to avoid supplying pages out of bounds.
        let vmo_size = match file.vmo.get_size() {
            Ok(size) => size,
            Err(status) => {
                log_error!("Failed to get VMO size: {:?}", status);
                let _ = self.pager.pager.op_range(
                    zx::PagerOp::Fail(zx::Status::IO),
                    &file.vmo,
                    contents.range(),
                );
                return;
            }
        };
        readahead_end = std::cmp::min(readahead_end, vmo_size);

        if readahead_end <= readahead_start {
            return;
        }

        let start_block = (readahead_start / self.block_size) as u32;
        let mut ix = file.extents.partition_point(|e| e.logical.end <= start_block);

        // SAFETY: We know that `transfer_vmo` is mapped (and initialized) for `TRANSFER_VMO_SIZE`
        // bytes and `len` must be less than or equal to that.
        let buf = unsafe {
            std::slice::from_raw_parts_mut(transfer_vmo_addr as *mut u8, TRANSFER_VMO_SIZE as usize)
        };

        let mut current_offset = readahead_start;
        let mut supply_helper =
            SupplyHelper::new(transfer_vmo, buf, &file.vmo, current_offset, &*self.pager);

        while ix < file.extents.len() && current_offset < readahead_end {
            let extent = &file.extents[ix];

            let logical_start = extent.logical.start as u64 * self.block_size;

            // Deal with holes.
            if current_offset < logical_start {
                if let Err(e) = supply_helper.zero(logical_start - current_offset) {
                    supply_helper.fail_to(readahead_end, e);
                    return;
                }
                current_offset = logical_start;
            }

            let end = std::cmp::min(extent.logical.end as u64 * self.block_size, readahead_end);

            while current_offset < end {
                let phys_offset =
                    extent.physical_block * self.block_size + current_offset - logical_start;

                match supply_helper.fill_buf(|buf| {
                    let amount = std::cmp::min(buf.len() as u64, end - current_offset) as usize;
                    self.backing_vmo.read(&mut buf[..amount], phys_offset)?;
                    Ok(amount)
                }) {
                    Ok(amount) => {
                        // We don't need the pages in the backing VMO any more.  Don't worry about
                        // errors; this is purely a hint.
                        let _ = self.backing_vmo.op_range(
                            zx::VmoOp::DONT_NEED,
                            phys_offset,
                            amount as u64,
                        );
                        current_offset += amount as u64;
                    }
                    Err(e) => {
                        supply_helper.fail_to(readahead_end, e);
                        return;
                    }
                }
            }

            ix += 1;
        }

        if let Err(e) = supply_helper.finish(readahead_end) {
            supply_helper.fail_to(readahead_end, e);
        }
    }

    fn watch_for_zero_children(&self, vmo: &zx::Vmo, inode_num: u32) -> Result<(), zx::Status> {
        vmo.wait_async(
            &self.pager.port,
            self.port_key_for_inode(inode_num),
            zx::Signals::VMO_ZERO_CHILDREN,
            zx::WaitAsyncOpts::empty(),
        )
    }

    fn on_zero_children(&self, inode_num: u32) -> Result<(), Errno> {
        let mut files = self.files_by_inode.lock();
        let file = files.entry(inode_num);
        if let Entry::Occupied(o) = file {
            let vmo = &o.get().vmo;
            match vmo.info() {
                Ok(info) => {
                    if info.num_children == 0 {
                        // This is a true signal, so we can remove this entry.
                        o.remove();
                    } else {
                        // This shouldn't fail, and there's not much we can do
                        // if it does.
                        if let Err(error) = self.watch_for_zero_children(vmo, inode_num) {
                            log_error!(
                                error:?;
                                "watch_for_zero_children failed"
                            );
                        }
                    }
                }
                Err(error) => log_error!(error:?; "Vmo::info failed"),
            }
        }
        Ok(())
    }

    fn port_key_for_inode(&self, inode_num: u32) -> u64 {
        (self.id as u64) << 32 | inode_num as u64
    }

    fn id(&self) -> u32 {
        self.id
    }
}

/// Per file state needed by the pager.
struct PagedFile {
    /// The main VMO.  We always hand out children of this VMO.
    vmo: zx::Vmo,

    /// The extents for the file, which will be sorted and not overlapping.  There can be holes i.e.
    /// zeroed ranges within the file.
    extents: Box<[PagerExtent]>,
}

/// A single extent.
pub struct PagerExtent {
    pub logical: Range<u32>,
    pub physical_block: u64,
}

/// SupplyHelper exists to make dealing with misalignment easier.
struct SupplyHelper<'a> {
    transfer_vmo: &'a zx::Vmo,
    buffer: &'a mut [u8],
    target_vmo: &'a zx::Vmo,
    offset: u64,
    pager: &'a Pager,
    page_size: u64,
    buf_len: usize,
}

impl<'a> SupplyHelper<'a> {
    fn new(
        transfer_vmo: &'a zx::Vmo,
        buffer: &'a mut [u8],
        target_vmo: &'a zx::Vmo,
        offset: u64,
        pager: &'a Pager,
    ) -> Self {
        Self {
            transfer_vmo,
            buffer,
            target_vmo,
            offset,
            pager,
            page_size: *starnix_core::mm::PAGE_SIZE,
            buf_len: 0,
        }
    }

    /// Zeroes `len` bytes.
    fn zero(&mut self, mut len: u64) -> Result<(), zx::Status> {
        let unaligned = self.buf_len as u64 % self.page_size;
        if unaligned > 0 {
            let amount = std::cmp::min(self.page_size - unaligned, len);
            self.buffer[self.buf_len..self.buf_len + amount as usize].fill(0);
            self.buf_len += amount as usize;
            len -= amount;
            self.supply_pages()?;
        }
        // Zero whole pages by supplying pages from the zero VMO.
        while len >= self.page_size {
            let amount =
                if len >= ZERO_VMO_SIZE { ZERO_VMO_SIZE } else { len - len % self.page_size };
            self.pager.pager.supply_pages(
                self.target_vmo,
                self.offset..self.offset + amount,
                &self.pager.zero_vmo,
                0,
            )?;
            self.offset += amount;
            len -= amount;
        }
        // And now the remaining partial page...
        self.buffer[self.buf_len..self.buf_len + len as usize].fill(0);
        self.buf_len += len as usize;
        Ok(())
    }

    /// Flushes whole pages.
    fn supply_pages(&mut self) -> Result<(), zx::Status> {
        if self.buf_len as u64 >= self.page_size {
            let len = self.buf_len - self.buf_len % self.page_size as usize;
            self.pager.pager.supply_pages(
                self.target_vmo,
                self.offset..self.offset + len as u64,
                self.transfer_vmo,
                0,
            )?;
            // Move any remaining data to the beginning of the buffer.
            self.buffer.copy_within(len..self.buf_len, 0);
            self.buf_len -= len;
            self.offset += len as u64;
        }
        Ok(())
    }

    /// Fills the buffer by calling the provided callback.  Returns the amount of data filled.
    fn fill_buf(
        &mut self,
        f: impl FnOnce(&mut [u8]) -> Result<usize, zx::Status>,
    ) -> Result<usize, zx::Status> {
        let amount = f(&mut self.buffer[self.buf_len..])?;
        self.buf_len += amount;
        self.supply_pages()?;
        Ok(amount)
    }

    /// Zeroes out to at least `end`, then pads to a page boundary and supplies those pages.
    fn finish(&mut self, mut end: u64) -> Result<(), zx::Status> {
        let byte_offset = self.offset + self.buf_len as u64;
        end = std::cmp::max(end, byte_offset);
        end = end + self.page_size - 1;
        end -= end % self.page_size;
        self.zero(end - byte_offset)
    }

    /// Fails the request up to the given offset with `error`.
    fn fail_to(&mut self, end: u64, error: zx::Status) {
        if self.offset < end {
            log_warn!(error:?; "Failing page-in, range: {:?}", self.offset..end);
            // The pager is fussy about what errors we can return here, so we always return IO.
            match self.pager.pager.op_range(
                zx::PagerOp::Fail(zx::Status::IO),
                self.target_vmo,
                self.offset..end,
            ) {
                Ok(()) => {}
                Err(error) => log_error!(error:?; "Failed to report error"),
            }
            self.offset = end;
            self.buf_len = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Filesystem, Pager, PagerExtent};

    use std::sync::Arc;
    use std::time::Duration;

    #[::fuchsia::test]
    async fn test_pager() {
        let backing_vmo = zx::Vmo::create(1 * 1024 * 1024).expect("Vmo::craete failed");
        let backing_vmo_clone =
            backing_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("failed handle dup");

        let pager = Arc::new(Pager::new().expect("Pager::new failed"));
        let filesystem = Arc::new(
            Filesystem::new(pager.clone(), backing_vmo_clone, 1024)
                .expect("Filesystem::new failed"),
        );

        pager.add_filesystem(filesystem.clone());

        {
            pager.start_threads();

            // With no extent, we expect it to return zeroed data.
            let vmo = filesystem.register("a".into(), 1, 5, Box::new([])).expect("register failed");

            let mut buf = vec![1; 5];
            vmo.read(&mut buf, 0).expect("read failed");

            assert_eq!(&buf, &[0; 5]);

            // A single extent:
            let vmo = filesystem
                .register(
                    "b".into(),
                    2,
                    5,
                    Box::new([PagerExtent { logical: 0..1, physical_block: 0 }]),
                )
                .expect("register failed");
            backing_vmo.write(b"hello", 0).expect("write failed");
            vmo.read(&mut buf, 0).expect("read failed");

            assert_eq!(&buf, b"hello");

            // A file with sparse ranges: 6 sparse, 1 extent, 5 more sparse, 1 extent, 4 sparse + a
            // bit.
            let file_size = (6 + 1 + 5 + 4) * 1024 + 100;
            let vmo = filesystem
                .register(
                    "c".into(),
                    3,
                    file_size,
                    Box::new([
                        PagerExtent { logical: 6..7, physical_block: 0 },
                        PagerExtent { logical: 12..13, physical_block: 1 },
                    ]),
                )
                .expect("register failed");
            backing_vmo.write(b"there", 1024).expect("write failed");
            let mut buf = vec![1; file_size as usize];
            vmo.read(&mut buf, 0).expect("read failed");

            let mut expected = vec![0; file_size as usize];
            expected[6 * 1024..6 * 1024 + 5].copy_from_slice(b"hello");
            expected[12 * 1024..12 * 1024 + 5].copy_from_slice(b"there");
            assert_eq!(&buf, &expected);

            // Use the same file, but initiate a read that starts after the first extent.
            let vmo = filesystem
                .register(
                    "d".into(),
                    4,
                    file_size,
                    Box::new([
                        PagerExtent { logical: 6..7, physical_block: 0 },
                        PagerExtent { logical: 12..13, physical_block: 1 },
                    ]),
                )
                .expect("register failed");

            let offset = 9000;
            let mut buf = vec![1; (file_size - offset) as usize];
            vmo.read(&mut buf, offset).expect("read failed");

            assert_eq!(&buf, &expected[offset as usize..]);
        }

        // After dropping all VMOs, we expect the pager to clean up.
        loop {
            if filesystem.files_by_inode.lock().is_empty() {
                break;
            }
            // The pager is running on different threads, hence:
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
