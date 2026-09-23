// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::vm::page::{VmPage, VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_state::VmPageState;
use crate::vm::{physmap, pmm};
use core::cmp::min;
use core::convert::Infallible;
use core::ffi::c_char;
use core::mem::{MaybeUninit, align_of, size_of};
use core::pin::Pin;
use core::ptr::NonNull;
use core::slice::{from_raw_parts, from_raw_parts_mut};
use fbl::{DoublyLinkedList, DoublyLinkedListContainable, DoublyLinkedListNode};
use page;
use page_bindings::vm_page_state;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop, stack_pin_init};
use zr::static_assert;
use zx_status::Status;

// Total amount of memory occupied by MBuf objects.
define_kcounter!(MBUF_TOTAL_BYTES_COUNT, "mbuf.total_bytes", Sum);

/// An MBuf is a small fixed-size chainable memory buffer.
#[repr(C)]
#[derive(DoublyLinkedListContainable)]
struct MBuf {
    #[dll_node]
    node: DoublyLinkedListNode<MBuf>,

    /// Length of the valid `data` in this buffer. Writes can append more to `data` and increment
    /// this length.
    len: u32,

    /// `pkt_len` is set to the total number of bytes in a packet when a socket is in
    /// `ZX_SOCKET_DATAGRAM` mode. A `pkt_len` of 0 means this `MBuf` is part of the body of a
    /// packet.
    ///
    /// Always 0 in `ZX_SOCKET_STREAM` mode.
    pkt_len: u32,

    /// Back-pointer to the `vm_page_t` this `MBuf` was allocated from. Recording this is just an
    /// optimization as it should always be the case that:
    /// `Pmm::Node().PaddrToPage(physmap_to_paddr(this)) == page_`
    page: VmPagePtr,

    /// The data field is left uninitialized as the caller is going to immediately overwrite with
    /// the payload, and is trusted to not access any uninitialized portions.
    /// TODO: maybe union data with char* blocks for large messages
    data: MaybeUninit<[u8; MBuf::PAYLOAD_SIZE]>,
}

impl MBuf {
    // 16 for the linked list 16 for the explicit fields.
    pub const HEADER_SIZE: usize = 32;
    pub const PAYLOAD_SIZE: usize = page::SIZE - Self::HEADER_SIZE;

    /// Calculate the number of MBuf objects needed to store a payload of the given size.
    pub const fn num_buffers_for_payload(payload: usize) -> usize {
        payload.div_ceil(Self::PAYLOAD_SIZE)
    }

    /// Returns number of bytes of free space in this MBuf.
    pub fn available_space(&self) -> usize {
        Self::PAYLOAD_SIZE - (self.len as usize)
    }

    /// Returns a slice of valid initialized data starting at `offset` up to `self.len`, typed as
    /// `c_char` for user memory copy operations.
    pub fn read(&self, offset: usize) -> &[c_char] {
        let len = self.len as usize;
        if offset >= len {
            return &[];
        }
        let valid_len = len - offset;
        // SAFETY: `self.data` has length `PAYLOAD_SIZE >= len >= offset + valid_len`.
        // The memory from `0..len` has been initialized by prior writes.
        // `c_char` and `u8` have identical size (1 byte), alignment (1 byte), and all bit patterns
        // are valid.
        unsafe {
            let ptr = self.data.as_ptr().cast::<c_char>().add(offset);
            from_raw_parts(ptr, valid_len)
        }
    }

    /// Returns a mutable slice of uninitialized capacity starting after current `len` up to
    /// `max_len` (capped by `available_space()`), typed as `MaybeUninit<c_char>` for user copies.
    pub fn extend(&mut self, max_len: usize) -> &mut [MaybeUninit<c_char>] {
        let copy_len = min(self.available_space(), max_len);
        let offset = self.len as usize;
        // SAFETY: `offset + copy_len <= PAYLOAD_SIZE`.
        // `MaybeUninit<c_char>` and `u8` have identical size (1 byte) and alignment (1 byte).
        unsafe {
            let ptr = self.data.as_mut_ptr().cast::<MaybeUninit<c_char>>().add(offset);
            from_raw_parts_mut(ptr, copy_len)
        }
    }

    /// Copies up to `len - *pos` bytes from user pointer `src` at `*pos` into this `MBuf`,
    /// advancing `*pos` and `self.len` by the number of bytes copied.
    pub fn write_from_user(
        &mut self,
        src: UserInPtr<c_char>,
        pos: &mut usize,
        len: usize,
    ) -> Result<(), Status> {
        let dst_slice = self.extend(len - *pos);
        let copy_len = dst_slice.len();
        src.byte_offset(*pos as isize).copy_slice_from_user(dst_slice)?;
        *pos += copy_len;
        self.len += copy_len as u32;
        Ok(())
    }

    /// Initializes an uninitialized `MBuf` in the physical map of the given `page`.
    ///
    /// Returns a pointer to the initialized `MBuf`.
    ///
    /// # Safety
    ///
    /// `page_raw` must be a valid pointer to an allocated `VmPage` whose physical memory is mapped
    /// in the physmap, and the caller must possess conceptual ownership of the page to set its
    /// state.
    unsafe fn init_in_page(page_raw: *mut VmPage) -> *mut MBuf {
        // SAFETY: Caller guarantees `page_raw` is a valid pointer to an allocated `VmPage` and
        // possesses conceptual ownership of it. `buf_ptr` points to the mapped physical memory
        // of the allocated page and is initialized without dropping.
        unsafe {
            let page = &*page_raw;
            page.set_state(VmPageState(vm_page_state::IPC));
            let paddr = page.paddr();
            let buf_ptr = physmap::paddr_to_physmap(paddr).0 as *mut MBuf;
            let page_ptr = VmPagePtr::new(NonNull::from(page));

            buf_ptr.write(MBuf {
                node: DoublyLinkedListNode::new(),
                len: 0,
                pkt_len: 0,
                page: page_ptr,
                data: MaybeUninit::uninit(),
            });

            buf_ptr
        }
    }
}

static_assert!(size_of::<MBuf>() == page::SIZE);
static_assert!(align_of::<MBuf>() == 8);

/// Helper function to allocate `num` `MBuf` buffers into a `DoublyLinkedList`.
///
/// If allocation of any buffer fails, the error is returned and `bufs` is left unmodified.
fn alloc_mbufs(num: usize, bufs: &mut DoublyLinkedList<*mut MBuf>) -> Result<(), Status> {
    if num == 0 {
        return Ok(());
    }

    stack_pin_init!(let pages = VmPageDoublyLinkedList::new());
    pmm::alloc_pages(num, 0, pages.as_mut())?;

    // SAFETY: `pages` is pinned on stack; obtaining mutable reference to pop pages is safe.
    let pages = unsafe { pages.get_unchecked_mut() };
    while let Some(page_raw) = pages.pop_front() {
        // SAFETY: `page_raw` was popped from `pages` allocated by `pmm::alloc_pages` and is valid.
        // The resulting `buf_ptr` is newly initialized and not currently in any list.
        unsafe {
            let buf_ptr = MBuf::init_in_page(page_raw.as_ptr());
            bufs.push_back_raw(buf_ptr);
        }
    }

    MBUF_TOTAL_BYTES_COUNT.add((num * size_of::<MBuf>()) as i64);
    Ok(())
}

/// Helper function to free all `MBuf` buffers in a `DoublyLinkedList`.
fn free_mbufs(bufs: &mut DoublyLinkedList<*mut MBuf>) {
    if bufs.is_empty() {
        return;
    }

    stack_pin_init!(let pages = VmPageDoublyLinkedList::new());
    // SAFETY: `pages` is pinned on stack; obtaining mutable reference to the list is safe.
    let pages_list = unsafe { pages.as_mut().get_unchecked_mut() };

    let mut count = 0usize;
    while let Some(buf_ptr) = bufs.pop_front() {
        // SAFETY: `buf_ptr` was popped from `bufs` and points to a valid, initialized `MBuf`
        // allocated from PMM whose backing page is not in any list.
        unsafe {
            let page_ptr = (*buf_ptr).page;
            pages_list.push_back_raw(page_ptr.as_non_null());
        }
        count += 1;
    }

    MBUF_TOTAL_BYTES_COUNT.add(-((count * size_of::<MBuf>()) as i64));

    // SAFETY: All pages in `pages` were allocated from PMM and are being returned to PMM.
    unsafe {
        pmm::free_list(pages);
    }
}

/// An RAII guard for a temporary linked list of `MBuf` pointers.
///
/// If dropped without being disarmed, any `MBuf`s currently in the list are automatically freed to
/// PMM via `free_mbufs`. This ensures that temporary allocations during writes or buffers collected
/// during reads are never leaked on error or early return.
struct MBufListGuard<'a> {
    list: Option<&'a mut DoublyLinkedList<*mut MBuf>>,
}

impl<'a> MBufListGuard<'a> {
    /// Creates an armed guard wrapping a pinned `list`.
    fn new(list: Pin<&'a mut DoublyLinkedList<*mut MBuf>>) -> Self {
        // SAFETY: `list` is pinned on the caller's stack and will not be moved for lifetime `'a`.
        Self { list: Some(unsafe { list.get_unchecked_mut() }) }
    }

    /// Pushes an unlinked `MBuf` to the back of the list.
    fn push(&mut self, buf: *mut MBuf) {
        // SAFETY: `buf` is a valid, unlinked `MBuf` allocated from PMM.
        unsafe {
            self.as_mut().push_back_raw(buf);
        }
    }

    /// Returns a mutable reference to the underlying list.
    fn as_mut(&mut self) -> &mut DoublyLinkedList<*mut MBuf> {
        self.list.as_deref_mut().expect("MBufListGuard already disarmed")
    }

    /// Disarms the guard, returning the underlying list so its buffers will not be freed on drop.
    fn disarm(mut self) -> &'a mut DoublyLinkedList<*mut MBuf> {
        self.list.take().expect("MBufListGuard already disarmed")
    }
}

impl Drop for MBufListGuard<'_> {
    fn drop(&mut self) {
        if let Some(list) = self.list.take() {
            free_mbufs(list);
        }
    }
}

/// MBufChain is a container for storing a stream of bytes or a sequence of datagrams.
///
/// It's designed to back sockets and channels. Don't simultaneously store stream data and datagrams
/// in a single instance.
#[pin_data(PinnedDrop)]
pub struct MBufChain {
    /// The MBufs are placed in a doubly linked list so that both the front and back of the list can
    /// be manipulated and to allow for efficiently splicing lists into each other.
    ///
    /// The active buffers that make up this chain. buffers.front() + read_cursor_off is the read
    /// cursor. buffers.back() is the write cursor.
    #[pin]
    buffers: DoublyLinkedList<*mut MBuf>,
    /// The byte offset of the read cursor in next MBuf.
    read_cursor_off: usize,
    size: usize,
}

impl MBufChain {
    /// Although the maximum size of the data in an MBuf is a kernel implementation detail, it is
    /// visible to user space. To avoid unintentionally changing it when modifying other data
    /// structures round up the currently chosen size (256KiB) to the next multiple of the MBuf
    /// payload size.
    pub const MAX_SIZE: usize = (256usize * 1024).div_ceil(MBuf::PAYLOAD_SIZE) * MBuf::PAYLOAD_SIZE;

    /// Constructs a new `MBufChain`.
    pub fn new() -> impl PinInit<Self, Infallible> {
        pin_init!(Self {
            buffers <- DoublyLinkedList::<*mut MBuf>::new(),
            read_cursor_off: 0,
            size: 0,
        })
    }

    /// Writes `len` bytes of stream data from `src`.
    ///
    /// Returns an error on failure, although some data may still have been written, in which case
    /// `written` is set with the amount.
    ///
    /// Returns `(res, written)` indicating the operation status and the number of bytes written.
    pub fn write_stream(
        self: Pin<&mut Self>,
        src: UserInPtr<c_char>,
        mut len: usize,
    ) -> (Result<(), Status>, usize) {
        // SAFETY: We hold `Pin<&mut Self>` and obtain `&mut Self` to perform buffer writes without
        // moving `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        // Cap len by the max we are allowed to write.
        len = min(Self::MAX_SIZE - this.size, len);
        if len == 0 {
            return (Err(Status::SHOULD_WAIT), 0);
        }

        let avail = this.buffers.back().map_or(0, |b| b.available_space());
        let num_buffers = MBuf::num_buffers_for_payload(len.saturating_sub(avail));

        stack_pin_init!(let bufs = DoublyLinkedList::<*mut MBuf>::new());
        let mut bufs_guard = MBufListGuard::new(bufs);

        if alloc_mbufs(num_buffers, bufs_guard.as_mut()).is_err() {
            return (Err(Status::SHOULD_WAIT), 0);
        }

        let mut pos = 0usize;
        let mut tail_written = 0usize;

        let tail = this.buffers.back_mut().filter(|b| b.available_space() > 0);
        let bufs = tail
            .into_iter()
            .map(|b| (b, true))
            .chain(bufs_guard.as_mut().iter_mut().map(|b| (b, false)));

        for (buf, is_tail) in bufs {
            let res = buf.write_from_user(src, &mut pos, len);
            if is_tail {
                this.size += pos;
                tail_written = pos;
            }
            if let Err(err) = res {
                // TODO(https://fxbug.dev/42109418): Note that although we set |written| for the
                // benefit of the socket dispatcher updating signals, ultimately we're not
                // indicating to the caller that data added so far in previous copies was written
                // successfully. This means the caller may try to re-send the same data again,
                // leading to duplicate data. Consider changing the socket dispatcher to forward
                // this partial write information to the caller, or consider not committing any of
                // the new data until we can ensure success, or consider putting the socket in a
                // state where it can't succeed a subsequent write.
                return (Err(err), tail_written);
            }
        }

        this.buffers.splice(bufs_guard.disarm());
        this.size += pos - tail_written;
        (Ok(()), pos)
    }

    /// Writes a datagram of `len` bytes from `src`.
    ///
    /// This operation is atomic in that either the entire datagram is written successfully or the
    /// chain is unmodified.
    ///
    /// Writing a zero-length datagram is an error.
    ///
    /// Returns an error on failure, although some data may still have been written, in which case
    /// `written` is set with the amount.
    ///
    /// Returns `(res, written)` indicating the operation status and the number of bytes written.
    pub fn write_datagram(
        self: Pin<&mut Self>,
        src: UserInPtr<c_char>,
        len: usize,
    ) -> (Result<(), Status>, usize) {
        // SAFETY: We hold `Pin<&mut Self>` and obtain `&mut Self` to perform buffer writes without
        // moving `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        if len == 0 {
            return (Err(Status::INVALID_ARGS), 0);
        }
        if len > Self::MAX_SIZE {
            return (Err(Status::OUT_OF_RANGE), 0);
        }
        if Self::MAX_SIZE - this.size < len {
            return (Err(Status::SHOULD_WAIT), 0);
        }

        let num_buffers = MBuf::num_buffers_for_payload(len);

        stack_pin_init!(let bufs = DoublyLinkedList::<*mut MBuf>::new());
        let mut bufs_guard = MBufListGuard::new(bufs);

        if alloc_mbufs(num_buffers, bufs_guard.as_mut()).is_err() {
            return (Err(Status::SHOULD_WAIT), 0);
        }

        let mut pos = 0usize;
        for buf in bufs_guard.as_mut().iter_mut() {
            if let Err(err) = buf.write_from_user(src, &mut pos, len) {
                return (Err(err), 0);
            }
        }

        bufs_guard.as_mut().front_mut().unwrap().pkt_len = len as u32;

        // Successfully built the packet mbufs. Splice into this.buffers.
        this.buffers.splice(bufs_guard.disarm());
        this.size += len;
        (Ok(()), len)
    }

    /// Reads up to `len` bytes of stream data from the chain into `dst` (no boundaries).
    ///
    /// The actual number of bytes read is returned in `actual`, and this can be non-zero even if
    /// the read itself is an error.
    ///
    /// Returns `(res, actual)` indicating the operation status and number of bytes read.
    pub fn read_stream(
        self: Pin<&mut Self>,
        dst: UserOutPtr<c_char>,
        len: usize,
    ) -> (Result<(), Status>, usize) {
        // SAFETY: We hold `Pin<&mut Self>` and obtain `&mut Self` to read and remove buffers from
        // `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        if this.size == 0 || len == 0 {
            return (Ok(()), 0);
        }

        let mut pos = 0usize;
        let mut read_off = this.read_cursor_off;

        stack_pin_init!(let free_list = DoublyLinkedList::<*mut MBuf>::new());
        let mut free_list = MBufListGuard::new(free_list);

        let res = (|| {
            while pos < len
                && let Some(front) = this.buffers.front()
            {
                let slice = front.read(read_off);
                let copy_len = min(slice.len(), len - pos);

                dst.byte_offset(pos as isize).copy_slice_to_user(&slice[..copy_len])?;

                pos += copy_len;
                read_off += copy_len;
                this.size -= copy_len;

                if read_off == front.len as usize {
                    if let Some(buf) = this.buffers.pop_front() {
                        free_list.push(buf);
                    }
                    read_off = 0;
                }
            }
            Ok(())
        })();

        // Record the fact that some data might have been read, even if the overall operation is
        // considered a failure.
        this.read_cursor_off = read_off;
        (res, pos)
    }

    /// Reads at most one datagram from the chain into `dst`.
    ///
    /// If `len` is too small to read a complete datagram, a partial datagram is returned and its
    /// remaining bytes are discarded.
    ///
    /// The actual number of bytes read is returned in `actual`, and this can be non-zero even if
    /// the read itself is an error.
    ///
    /// Returns an error on failure. If an error occurs while copying a datagram to `dst`, the
    /// datagram is dropped.
    ///
    /// Returns `(res, actual)` indicating the operation status and number of bytes read.
    pub fn read_datagram(
        self: Pin<&mut Self>,
        dst: UserOutPtr<c_char>,
        mut len: usize,
    ) -> (Result<(), Status>, usize) {
        // SAFETY: We hold `Pin<&mut Self>` and obtain `&mut Self` to read and remove buffers from
        // `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        if this.size == 0 || len == 0 {
            return (Ok(()), 0);
        }

        len = min(len, this.buffers.front().unwrap().pkt_len as usize);
        let mut pos = 0usize;

        stack_pin_init!(let free_list = DoublyLinkedList::<*mut MBuf>::new());
        let mut free_list = MBufListGuard::new(free_list);

        let res = (|| {
            while pos < len
                && let Some(front) = this.buffers.front()
            {
                let slice = front.read(0);
                let copy_len = min(slice.len(), len - pos);

                let copy_res = dst.byte_offset(pos as isize).copy_slice_to_user(&slice[..copy_len]);

                // In datagram mode, each visited buffer is popped and discarded completely.
                this.size -= front.len as usize;
                if let Some(buf) = this.buffers.pop_front() {
                    free_list.push(buf);
                }

                copy_res?;
                pos += copy_len;
            }
            Ok(())
        })();

        // Drain any leftover mbufs in the datagram packet if we're consuming data, even if we fail
        // to read bytes.
        while let Some(front) = this.buffers.front()
            && front.pkt_len == 0
        {
            this.size -= front.len as usize;
            if let Some(buf) = this.buffers.pop_front() {
                free_list.push(buf);
            }
        }

        (res, pos)
    }

    /// Same as `read_stream()`/`read_datagram()` but leaves the bytes in the chain instead of
    /// consuming them, even if an error occurs.
    ///
    /// Peeks up to `len` bytes of stream data from the chain into `dst` without consuming them.
    ///
    /// Returns `(res, actual)` indicating the operation status and number of bytes peeked.
    pub fn peek_stream(&self, dst: UserOutPtr<c_char>, len: usize) -> (Result<(), Status>, usize) {
        if self.size == 0 || len == 0 {
            return (Ok(()), 0);
        }

        let mut pos = 0usize;
        let mut read_off = self.read_cursor_off;

        let res = (|| {
            for buf in self.buffers.iter() {
                if pos >= len {
                    break;
                }
                let slice = buf.read(read_off);
                let copy_len = min(slice.len(), len - pos);

                dst.byte_offset(pos as isize).copy_slice_to_user(&slice[..copy_len])?;

                pos += copy_len;
                read_off = 0;
            }
            Ok(())
        })();

        (res, pos)
    }

    /// Peeks at most one datagram from the chain into `dst` without consuming it.
    ///
    /// Returns `(res, actual)` indicating the operation status and number of bytes peeked.
    pub fn peek_datagram(
        &self,
        dst: UserOutPtr<c_char>,
        len: usize,
    ) -> (Result<(), Status>, usize) {
        if self.size == 0 || len == 0 {
            return (Ok(()), 0);
        }

        let len = min(len, self.buffers.front().unwrap().pkt_len as usize);
        self.peek_stream(dst, len)
    }

    /// Returns number of bytes stored in the chain.
    pub fn stream_size(&self) -> usize {
        self.size
    }

    /// Returns number of bytes stored in the first datagram, or 0 if empty.
    pub fn datagram_size(&self) -> usize {
        if let Some(front) = self.buffers.front() { front.pkt_len as usize } else { 0 }
    }

    /// Returns true if chain is full.
    pub fn is_full(&self) -> bool {
        self.size >= Self::MAX_SIZE
    }

    /// Returns true if chain is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[pinned_drop]
impl PinnedDrop for MBufChain {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: `self` is being dropped and will not be accessed again. Obtaining `&mut Self`
        // allows freeing remaining buffers.
        let this = unsafe { self.get_unchecked_mut() };
        free_mbufs(&mut this.buffers);
    }
}

/// In-tree kernel unit tests for `MBufChain`.
#[cfg(ktest)]
#[unittest::suite(name = "mbuf_rust")]
mod tests {
    use super::{MBuf, MBufChain, MBufListGuard, alloc_mbufs};
    use crate::user_copy::{UserInPtr, UserOutPtr};
    use crate::user_memory::UserMemory;
    use core::ffi::c_char;
    use core::mem::MaybeUninit;
    use core::pin::Pin;
    use pin_init::stack_pin_init;
    use unittest::{expect_eq, expect_false, expect_ok, expect_true, unwrap_ok};
    use zx_status::Status;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    enum MessageType {
        Stream,
        Datagram,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    enum ReadType {
        Read,
        Peek,
    }

    fn make_user_in_pattern(
        size: usize,
        pattern: impl Fn(usize) -> u8,
    ) -> Option<(UserMemory, UserInPtr<c_char>)> {
        let alloc_size = if size == 0 { 1 } else { size };
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        let mut chunk = [0u8; 512];
        let mut offset = 0;
        while offset < size {
            let to_write = core::cmp::min(chunk.len(), size - offset);
            for (i, b) in chunk[..to_write].iter_mut().enumerate() {
                *b = pattern(offset + i);
            }
            mem.vmo_write(&chunk[..to_write], offset as u64).ok()?;
            offset += to_write;
        }
        let ptr = UserInPtr::new(mem.base() as *const c_char);
        Some((mem, ptr))
    }

    fn make_user_in_byte(size: usize, val: u8) -> Option<(UserMemory, UserInPtr<c_char>)> {
        make_user_in_pattern(size, |_| val)
    }

    fn make_user_in(data: &[u8]) -> Option<(UserMemory, UserInPtr<c_char>)> {
        make_user_in_pattern(data.len(), |i| data[i])
    }

    fn make_user_out(size: usize) -> Option<(UserMemory, UserOutPtr<c_char>)> {
        let alloc_size = if size == 0 { 1 } else { size };
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        let ptr = UserOutPtr::new(mem.base() as *mut c_char);
        Some((mem, ptr))
    }

    fn verify_user_mem(mem: &UserMemory, size: usize, pattern: impl Fn(usize) -> u8) -> bool {
        let mut chunk = [MaybeUninit::<u8>::uninit(); 512];
        let mut offset = 0;
        while offset < size {
            let to_read = core::cmp::min(chunk.len(), size - offset);
            let Ok(read_bytes) = mem.vmo_read(&mut chunk[..to_read], offset as u64) else {
                return false;
            };
            for (i, &b) in read_bytes.iter().enumerate() {
                if b != pattern(offset + i) {
                    return false;
                }
            }
            offset += to_read;
        }
        true
    }

    /// Writes a string slice into `chain`.
    ///
    /// Helps eliminate boilerplate code dealing with copying in and out of user memory to make the
    /// test logic more obvious.
    fn write_helper(chain: Pin<&mut MBufChain>, str_data: &str, msg_type: MessageType) -> bool {
        let len = str_data.len();
        let Some((_mem, src)) = make_user_in(str_data.as_bytes()) else { return false };
        let (res, written) = match msg_type {
            MessageType::Datagram => chain.write_datagram(src, len),
            MessageType::Stream => chain.write_stream(src, len),
        };
        res.is_ok() && written == len
    }

    /// Reads or peeks data from `chain`.
    fn read_helper<'a>(
        chain: &mut Pin<&mut MBufChain>,
        buf: &'a mut [MaybeUninit<u8>],
        len: usize,
        msg_type: MessageType,
        read_type: ReadType,
        actual: &mut usize,
    ) -> Result<&'a mut [u8], Status> {
        let (mem, dst) = make_user_out(len).ok_or(Status::NO_MEMORY)?;
        let (res, nread) = match (read_type, msg_type) {
            (ReadType::Read, MessageType::Datagram) => chain.as_mut().read_datagram(dst, len),
            (ReadType::Read, MessageType::Stream) => chain.as_mut().read_stream(dst, len),
            (ReadType::Peek, MessageType::Datagram) => chain.peek_datagram(dst, len),
            (ReadType::Peek, MessageType::Stream) => chain.peek_stream(dst, len),
        };
        *actual = nread;
        res?;
        if nread > 0 {
            let copy_len = core::cmp::min(nread, buf.len());
            mem.vmo_read(&mut buf[..copy_len], 0)
        } else {
            Ok(&mut [])
        }
    }

    /// Tests initial state of MBufChain.
    #[test]
    fn test_initial_state() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let chain = chain_pin.as_ref();
        expect_true!(chain.is_empty());
        expect_false!(chain.is_full());
        expect_eq!(chain.stream_size(), 0);
    }

    /// Tests reading stream when empty.
    #[test]
    fn test_stream_read_empty() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let mut buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut buf,
            1,
            MessageType::Stream,
            ReadType::Read,
            &mut actual
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests reading stream with zero length.
    #[test]
    fn test_stream_read_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "x", MessageType::Stream));

        let mut buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut buf,
            0,
            MessageType::Stream,
            ReadType::Read,
            &mut actual
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests basic stream writing and reading.
    #[test]
    fn test_stream_write_basic() {
        const WRITE_LEN: usize = 1024;
        const NUM_WRITES: usize = 5;

        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        // Call write several times with different buffer contents.
        for i in 0..NUM_WRITES {
            let (_mem, src) = make_user_in_byte(WRITE_LEN, b'A' + (i as u8)).unwrap();
            let (res, written) = chain.as_mut().write_stream(src, WRITE_LEN);
            expect_ok!(res);
            expect_eq!(written, WRITE_LEN);
            expect_false!(chain.is_empty());
            expect_false!(chain.is_full());
            expect_eq!(chain.stream_size(), (i + 1) * WRITE_LEN);
        }

        // Read it all back in one call.
        const TOTAL_LEN: usize = WRITE_LEN * NUM_WRITES;
        expect_eq!(chain.stream_size(), TOTAL_LEN);

        let (mem_out, dst) = make_user_out(TOTAL_LEN).unwrap();
        let (res, actual) = chain.as_mut().read_stream(dst, TOTAL_LEN);
        expect_ok!(res);
        expect_eq!(actual, TOTAL_LEN);
        expect_true!(chain.is_empty());
        expect_false!(chain.is_full());
        expect_eq!(chain.stream_size(), 0);

        // Verify result.
        expect_true!(verify_user_mem(&mem_out, TOTAL_LEN, |offset| {
            b'A' + ((offset / WRITE_LEN) as u8)
        }));
    }

    /// Tests stream writing zero length.
    #[test]
    fn test_stream_write_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let (_mem, src) = make_user_in_byte(1, 0).unwrap();
        let (res, written) = chain.as_mut().write_stream(src, 0);
        // TODO(maniscalco): Is ZX_ERR_SHOULD_WAIT really the right error here in this case?
        expect_true!(res == Err(Status::SHOULD_WAIT));
        expect_eq!(written, 0);
        expect_true!(chain.is_empty());
        expect_false!(chain.is_full());
        expect_eq!(chain.stream_size(), 0);
    }

    /// Tests stream writing beyond capacity.
    #[test]
    fn test_stream_write_too_much() {
        const WRITE_LEN: usize = 65536;
        let (_mem_in, src) = make_user_in_byte(WRITE_LEN, 0).unwrap();

        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        // Fill the chain until it refuses to take any more.
        let mut total_written = 0usize;
        while !chain.is_full() {
            let (res, written) = chain.as_mut().write_stream(src, WRITE_LEN);
            if res.is_err() {
                break;
            }
            total_written += written;
        }

        expect_false!(chain.is_empty());
        expect_true!(chain.is_full());
        expect_eq!(total_written, chain.stream_size());

        // Read it all back out and see we get back the same number of bytes we wrote.
        let (_mem_out, dst) = make_user_out(WRITE_LEN).unwrap();
        let mut total_read = 0usize;
        while !chain.is_empty() {
            let (res, bytes_read) = chain.as_mut().read_stream(dst, WRITE_LEN);
            if res.is_err() || bytes_read == 0 {
                break;
            }
            total_read += bytes_read;
        }

        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
        expect_eq!(total_written, total_read);
    }

    /// Tests stream peeking data.
    #[test]
    fn test_stream_peek() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "abc", MessageType::Stream));
        expect_true!(write_helper(chain.as_mut(), "123", MessageType::Stream));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 10];
        let mut actual = 0;

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"a");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            3,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            4,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc1");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            6,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc123");

        expect_eq!(chain.stream_size(), 6);
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            6,
            MessageType::Stream,
            ReadType::Read,
            &mut actual,
        ));
        expect_true!(bytes == b"abc123");
    }

    /// Tests stream peeking when empty.
    #[test]
    fn test_stream_peek_empty() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests stream peeking zero length.
    #[test]
    fn test_stream_peek_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "a", MessageType::Stream));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            0,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests stream peeking with underflow.
    #[test]
    fn test_stream_peek_underflow() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        expect_true!(write_helper(chain.as_mut(), "abc", MessageType::Stream));
        let mut read_buf = [MaybeUninit::<u8>::uninit(); 10];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            10,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");

        expect_true!(write_helper(chain.as_mut(), "123", MessageType::Stream));
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            10,
            MessageType::Stream,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc123");
    }

    /// Tests datagram reading when empty.
    #[test]
    fn test_datagram_read_empty() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
        expect_true!(chain.is_empty());
    }

    /// Tests datagram reading zero length.
    #[test]
    fn test_datagram_read_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "x", MessageType::Datagram));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            0,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
        expect_false!(chain.is_empty());
    }

    /// Tests datagram reading with a small buffer.
    #[test]
    fn test_datagram_read_buffer_too_small() {
        const WRITE_LEN: usize = 32;
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        let (_mem_a, src_a) = make_user_in_byte(WRITE_LEN, b'A').unwrap();
        let (res_a, written_a) = chain.as_mut().write_datagram(src_a, WRITE_LEN);
        expect_ok!(res_a);
        expect_eq!(written_a, WRITE_LEN);
        expect_eq!(chain.stream_size(), WRITE_LEN);
        expect_false!(chain.is_empty());

        let (_mem_b, src_b) = make_user_in_byte(WRITE_LEN, b'B').unwrap();
        let (res_b, written_b) = chain.as_mut().write_datagram(src_b, WRITE_LEN);
        expect_ok!(res_b);
        expect_eq!(written_b, WRITE_LEN);
        expect_eq!(chain.stream_size(), 2 * WRITE_LEN);
        expect_false!(chain.is_empty());

        let mut read_buf = [MaybeUninit::<u8>::uninit(); WRITE_LEN];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(actual, 1);
        expect_eq!(bytes[0], b'A');
        expect_false!(chain.is_empty());

        expect_eq!(chain.stream_size(), WRITE_LEN);
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            WRITE_LEN,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(actual, WRITE_LEN);
        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
        expect_true!(bytes.iter().all(|&b| b == b'B'));
    }

    /// Tests basic datagram writing and reading.
    #[test]
    fn test_datagram_write_basic() {
        const NUM_DATAGRAMS: usize = 100;

        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        let mut total_written = 0;

        // Write a series of datagrams with different sizes.
        for i in 1..=NUM_DATAGRAMS {
            let (_mem, src) = make_user_in_byte(i, i as u8).unwrap();
            let (res, written) = chain.as_mut().write_datagram(src, i);
            expect_ok!(res);
            expect_eq!(written, i);
            total_written += written;
            expect_false!(chain.is_empty());
            expect_false!(chain.is_full());
        }

        // Verify size() returns correctly
        expect_eq!(chain.datagram_size(), 1);
        expect_eq!(chain.stream_size(), total_written);

        // Read them back and verify their contents.
        for i in 1..=NUM_DATAGRAMS {
            expect_eq!(chain.datagram_size(), i);
            let mut read_buf = [MaybeUninit::<u8>::uninit(); 100];
            let mut actual = 0;
            let bytes = unwrap_ok!(read_helper(
                &mut chain,
                &mut read_buf[..i],
                i,
                MessageType::Datagram,
                ReadType::Read,
                &mut actual,
            ));
            expect_eq!(actual, i);
            expect_true!(bytes.iter().all(|&b| b == (i as u8)));
        }

        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
    }

    /// Tests datagram writing zero length.
    #[test]
    fn test_datagram_write_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let (_mem, src) = make_user_in_byte(1, 0).unwrap();
        let (res, written) = chain.as_mut().write_datagram(src, 0);
        expect_true!(res == Err(Status::INVALID_ARGS));
        expect_eq!(written, 0);
        expect_true!(chain.is_empty());
        expect_false!(chain.is_full());
        expect_eq!(chain.datagram_size(), 0);
        expect_eq!(chain.stream_size(), 0);
    }

    /// Tests datagram writing beyond capacity.
    #[test]
    fn test_datagram_write_too_much() {
        const WRITE_LEN: usize = 65536;
        let (_mem_in, src) = make_user_in_byte(WRITE_LEN, 0).unwrap();

        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        // Fill the chain until it refuses to take any more.
        let mut num_datagrams_written = 0;
        while !chain.is_full() {
            let (res, written) = chain.as_mut().write_datagram(src, WRITE_LEN);
            if res.is_err() {
                break;
            }
            num_datagrams_written += 1;
            expect_eq!(written, WRITE_LEN);
        }

        expect_false!(chain.is_empty());
        expect_eq!(chain.stream_size(), WRITE_LEN * num_datagrams_written);

        // Read it all back out and see that there's none left over.
        let (_mem_out, dst) = make_user_out(WRITE_LEN).unwrap();
        let mut num_datagrams_read = 0;
        while !chain.is_empty() {
            let (res, actual) = chain.as_mut().read_datagram(dst, WRITE_LEN);
            if res.is_err() || actual == 0 {
                break;
            }
            num_datagrams_read += 1;
        }

        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
        expect_eq!(num_datagrams_written, num_datagrams_read);
    }

    /// Tests datagram buffer reuse.
    #[test]
    fn test_datagram_reuse_mbuf() {
        let large_write = MBuf::PAYLOAD_SIZE + 10;

        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        // Write two datagrams.
        let (_mem_a, a_src) = make_user_in_byte(1, b'a').unwrap();
        let (res_a, written_a) = chain.as_mut().write_datagram(a_src, 1);
        expect_ok!(res_a);
        expect_eq!(written_a, 1);
        let (_mem_b, b_src) = make_user_in_byte(1, b'b').unwrap();
        let (res_b, written_b) = chain.as_mut().write_datagram(b_src, 1);
        expect_ok!(res_b);
        expect_eq!(written_b, 1);

        // Now read them both out.
        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(bytes[0], b'a');
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(bytes[0], b'b');

        // Now write a large datagram that spans two buffers.
        let (_mem_c, c_src) = make_user_in_byte(large_write, b'c').unwrap();
        let (res_c, written_c) = chain.as_mut().write_datagram(c_src, large_write);
        expect_ok!(res_c);
        expect_eq!(written_c, large_write);

        // Write in a second small datagram.
        let (_mem_d, d_src) = make_user_in_byte(1, b'd').unwrap();
        let (res_d, written_d) = chain.as_mut().write_datagram(d_src, 1);
        expect_ok!(res_d);
        expect_eq!(written_d, 1);

        // Do a short read to consume the first datagram.
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(bytes[0], b'c');

        // Reading again should give us the second datagram we wrote, as the remaining of the first
        // should have been discarded.
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_eq!(bytes[0], b'd');

        // At this point the socket should be empty.
        expect_true!(chain.is_empty());
    }

    /// Tests writing a datagram packet larger than the mbuf's capacity.
    #[test]
    fn test_datagram_write_huge_packet() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        let huge_packet_size = MBufChain::MAX_SIZE + 1;
        let (_mem, src) = make_user_in_byte(1, 0).unwrap();
        let (res, _written) = chain.as_mut().write_datagram(src, huge_packet_size);
        expect_true!(res == Err(Status::OUT_OF_RANGE));
    }

    /// Tests datagram peeking.
    #[test]
    fn test_datagram_peek() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "abc", MessageType::Datagram));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 10];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"a");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            3,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");

        // Make sure peeking didn't affect an actual read.
        expect_eq!(chain.stream_size(), 3);
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            3,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");
    }

    /// Tests datagram peeking empty.
    #[test]
    fn test_datagram_peek_empty() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            1,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests datagram peeking zero length.
    #[test]
    fn test_datagram_peek_zero() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "a", MessageType::Datagram));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 1];
        let mut actual = 0;
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            0,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_eq!(actual, 0);
        expect_true!(bytes.is_empty());
    }

    /// Tests datagram peeking underflow.
    #[test]
    fn test_datagram_peek_underflow() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();
        expect_true!(write_helper(chain.as_mut(), "abc", MessageType::Datagram));
        expect_true!(write_helper(chain.as_mut(), "123", MessageType::Datagram));

        let mut read_buf = [MaybeUninit::<u8>::uninit(); 10];
        let mut actual = 0;

        // Datagram peeks should not return more than a single message.
        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            10,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            3,
            MessageType::Datagram,
            ReadType::Read,
            &mut actual,
        ));
        expect_true!(bytes == b"abc");

        let bytes = unwrap_ok!(read_helper(
            &mut chain,
            &mut read_buf,
            10,
            MessageType::Datagram,
            ReadType::Peek,
            &mut actual,
        ));
        expect_true!(bytes == b"123");
    }

    /// Tests multi-buffer datagram partial read discarding trailing continuation buffers.
    #[test]
    fn test_datagram_read_multibuffer_discard() {
        const LARGE_PAYLOAD: usize = MBuf::PAYLOAD_SIZE * 2 + 50;
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        let (_mem_in, src) = make_user_in_byte(LARGE_PAYLOAD, b'Z').unwrap();
        let (res, written) = chain.as_mut().write_datagram(src, LARGE_PAYLOAD);
        expect_ok!(res);
        expect_eq!(written, LARGE_PAYLOAD);
        expect_eq!(chain.stream_size(), LARGE_PAYLOAD);
        expect_eq!(chain.datagram_size(), LARGE_PAYLOAD);

        // Read only enough to span into the second buffer, leaving the third buffer untouched.
        const READ_LEN: usize = MBuf::PAYLOAD_SIZE + 20;
        let (mem_out, dst) = make_user_out(READ_LEN).unwrap();
        let (res_r, actual) = chain.as_mut().read_datagram(dst, READ_LEN);
        expect_ok!(res_r);
        expect_eq!(actual, READ_LEN);
        expect_true!(verify_user_mem(&mem_out, READ_LEN, |_| b'Z'));

        // All remaining bytes (including continuation buffer 3) should have been discarded.
        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
        expect_eq!(chain.datagram_size(), 0);
    }

    /// Tests interleaved partial stream reads across MBuf page boundaries.
    #[test]
    fn test_stream_read_interleaved_chunks() {
        const TOTAL_BYTES: usize = MBuf::PAYLOAD_SIZE * 3;
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        let (_mem_in, src) = make_user_in_pattern(TOTAL_BYTES, |i| (i % 251) as u8).unwrap();
        let (res, written) = chain.as_mut().write_stream(src, TOTAL_BYTES);
        expect_ok!(res);
        expect_eq!(written, TOTAL_BYTES);
        expect_eq!(chain.stream_size(), TOTAL_BYTES);

        let mut total_read = 0usize;
        let chunk_sizes = [100, MBuf::PAYLOAD_SIZE - 50, 500, MBuf::PAYLOAD_SIZE, 1000];

        for &chunk in &chunk_sizes {
            if total_read >= TOTAL_BYTES {
                break;
            }
            let to_read = core::cmp::min(chunk, TOTAL_BYTES - total_read);
            let (mem_out, dst) = make_user_out(to_read).unwrap();
            let (res_r, actual) = chain.as_mut().read_stream(dst, to_read);
            expect_ok!(res_r);
            expect_eq!(actual, to_read);
            expect_true!(verify_user_mem(&mem_out, actual, |i| { ((total_read + i) % 251) as u8 }));
            total_read += actual;
            expect_eq!(chain.stream_size(), TOTAL_BYTES - total_read);
        }

        // Read the remaining bytes.
        if total_read < TOTAL_BYTES {
            let rem = TOTAL_BYTES - total_read;
            let (mem_rem, rem_dst) = make_user_out(rem).unwrap();
            let (res_rem, actual) = chain.as_mut().read_stream(rem_dst, rem);
            expect_ok!(res_rem);
            expect_eq!(actual, rem);
            expect_true!(verify_user_mem(&mem_rem, actual, |i| { ((total_read + i) % 251) as u8 }));
        }

        expect_true!(chain.is_empty());
        expect_eq!(chain.stream_size(), 0);
    }

    /// Tests helper calculations for MBuf sizing and buffer requirements.
    #[test]
    fn test_mbuf_calculations() {
        expect_eq!(MBuf::num_buffers_for_payload(0), 0);
        expect_eq!(MBuf::num_buffers_for_payload(1), 1);
        expect_eq!(MBuf::num_buffers_for_payload(MBuf::PAYLOAD_SIZE), 1);
        expect_eq!(MBuf::num_buffers_for_payload(MBuf::PAYLOAD_SIZE + 1), 2);
        expect_eq!(MBuf::num_buffers_for_payload(2 * MBuf::PAYLOAD_SIZE), 2);
        expect_eq!(MBuf::num_buffers_for_payload(2 * MBuf::PAYLOAD_SIZE + 1), 3);

        expect_true!(MBufChain::MAX_SIZE >= 256 * 1024);
        expect_eq!(MBufChain::MAX_SIZE % MBuf::PAYLOAD_SIZE, 0);
    }

    /// Tests datagram write failure when exceeding chain capacity.
    #[test]
    fn test_datagram_write_should_wait_boundary() {
        stack_pin_init!(let chain_pin = MBufChain::new());
        let mut chain = chain_pin.as_mut();

        // Fill close to maximum capacity.
        const FILL_SIZE: usize = MBufChain::MAX_SIZE - 100;
        let (_mem, src) = make_user_in_byte(FILL_SIZE, b'K').unwrap();
        let (res, written) = chain.as_mut().write_datagram(src, FILL_SIZE);
        expect_ok!(res);
        expect_eq!(written, FILL_SIZE);
        expect_eq!(chain.stream_size(), FILL_SIZE);

        // Attempting to write a 101-byte datagram exceeds MAX_SIZE.
        let (_overflow_mem, overflow_src) = make_user_in_byte(101, b'V').unwrap();
        let (res_overflow, overflow_written) = chain.as_mut().write_datagram(overflow_src, 101);
        expect_true!(res_overflow == Err(Status::SHOULD_WAIT));
        expect_eq!(overflow_written, 0);
        expect_eq!(chain.stream_size(), FILL_SIZE);
    }

    /// Tests that MBuf allocation, insertion, removal, and freeing preserves node state.
    #[test]
    fn test_mbuf_alloc_free_node_lifecycle() {
        stack_pin_init!(let bufs = fbl::DoublyLinkedList::<*mut MBuf>::new());
        let mut bufs = MBufListGuard::new(bufs);

        expect_ok!(alloc_mbufs(10, bufs.as_mut()));

        stack_pin_init!(let other = fbl::DoublyLinkedList::<*mut MBuf>::new());
        let mut other = MBufListGuard::new(other);

        while let Some(buf) = bufs.as_mut().pop_front() {
            other.push(buf);
        }

        expect_true!(bufs.as_mut().is_empty());
        expect_false!(other.as_mut().is_empty());
    }

    /// Tests allocating and freeing MBufs using alloc_mbufs and free_mbufs helpers.
    #[test]
    fn test_alloc_free_mbufs() {
        stack_pin_init!(let bufs = fbl::DoublyLinkedList::<*mut MBuf>::new());
        let mut bufs = MBufListGuard::new(bufs);

        expect_ok!(alloc_mbufs(5, bufs.as_mut()));
        expect_false!(bufs.as_mut().is_empty());
    }
}
