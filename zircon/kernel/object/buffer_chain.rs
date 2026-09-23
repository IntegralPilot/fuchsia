// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! BufferChain is a list of buffers allocated from the PMM.
//!
//! It's designed for use with channel messages. Pages backing a BufferChain are marked as
//! vm_page_state::IPC.
//!
//! The BufferChain object itself lives *inside* its first buffer. Here's what it looks like:
//!
//!   +--------------------------------+     +--------------------------------+
//!   | page                           |     | page                           |
//!   |+------------------------------+|     |+------------------------------+|
//!   || Buffer                       |+---->|| Buffer                       ||
//!   || raw_data:   +---------------+||     || raw_data:    +--------------+||
//!   ||             |  BufferChain  |||     ||              | message data |||
//!   || reserved -> | ~~~~~~~~~~~~~ |||     || reserved = 0 |  continued   |||
//!   ||             |  message data |||     ||              |              |||
//!   ||             +---------------+||     ||              +--------------+||
//!   |+------------------------------+|     |+------------------------------+|
//!   +--------------------------------+     +--------------------------------+
//!
//! BufferChain does not dynamically allocate. An initial Alloc() call allocates a list of
//! pages that will be used to build the BufferChain. This allocation can exceed the size
//! actually used by the buffer chain and the excess buffers can be freed with a call to
//! FreeUnusedBuffers(). The motivation for sometimes allocating more than needed and later
//! freeing is that the number of needed buffers is sometimes initially unknown and it is
//! presumed to be more efficient to do a single allocation than multiple allocations.
//!
//! BufferChain uses a private PageCache to improve performance under load by
//! avoiding contention on the PMM. The page cache is tunable by the kernel
//! command line parameter kernel.bufferchain.reserve-pages.

use crate::page_cache::{PageCache, PageList};
use crate::user_copy::{UserInPtr, UserOutPtr};
use crate::vm::page::{VmPage, VmPageDoublyLinkedList};
use crate::vm::page_state::VmPageState;
use crate::vm::physmap;
use boot_options::BootOptions;
use core::mem::{self, MaybeUninit, size_of};
use core::pin::Pin;
use core::ptr::{self, NonNull};
use core::{cmp, slice};
use fbl::{Canary, SinglyLinkedList, SinglyLinkedListContainable, SinglyLinkedListNode};
use lazy_init::LazyInit;
use page_bindings::vm_page_state;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop, stack_pin_init};
use zr::static_assert;
use zx_status::Status;

static PAGE_CACHE: LazyInit<PageCache> = LazyInit::uninit();

fn page_cache() -> &'static PageCache {
    &PAGE_CACHE
}

fn initialize_page_cache(_level: init::LkInitLevel) {
    let reserve_pages = BootOptions::get().bufferchain_reserve_pages as usize;
    let cache =
        PageCache::new(reserve_pages).expect("failed to initialize buffer chain page cache");
    // SAFETY: Single-threaded initialization during early boot before concurrency.
    unsafe {
        PAGE_CACHE.init(cache);
    }
}

// Initialize the cache after the percpu data structures are initialized.
init::lk_init_hook!(
    buffer_chain_page_cache_init,
    initialize_page_cache,
    init::LK_INIT_LEVEL_KERNEL
);

/// Size of a buffer backing `BufferChain`.
const BUFFER_SIZE: usize = page::SIZE;

/// Size of the header fields in a `Buffer`.
const BUFFER_FIELDS_SIZE: usize = 16;

/// `RAW_DATA_SIZE` is the maximum number of bytes that can fit in a single `Buffer`.
///
/// However, not every `Buffer` in a `BufferChain` can store this many bytes. The first `Buffer`
/// in a `BufferChain` is special because it also stores the chain itself. See also
/// `CONTIGUOUS_SIZE`.
const RAW_DATA_SIZE: usize = BUFFER_SIZE - BUFFER_FIELDS_SIZE;

/// A single fixed-size buffer within a `BufferChain`, occupying an entire physical page.
#[repr(C)]
#[derive(SinglyLinkedListContainable)]
struct Buffer {
    #[sll_node]
    node: SinglyLinkedListNode<Buffer>,
    canary: Canary<{ fbl::magic(b"BUFC") }>,
    reserved: u32,
    raw_data: MaybeUninit<[u8; RAW_DATA_SIZE]>,
}

static_assert!(size_of::<Buffer>() == BUFFER_SIZE);

impl Buffer {
    /// Returns a raw pointer to the usable data payload in this buffer.
    fn data(&self) -> *const u8 {
        self.canary.assert();
        // SAFETY: `self.raw_data` has length `RAW_DATA_SIZE` and `reserved <= RAW_DATA_SIZE`.
        unsafe { self.raw_data.as_ptr().cast::<u8>().add(self.reserved as usize) }
    }

    /// Returns a mutable raw pointer to the usable data payload in this buffer.
    fn data_mut(&mut self) -> *mut u8 {
        self.canary.assert();
        // SAFETY: `self.raw_data` has length `RAW_DATA_SIZE` and `reserved <= RAW_DATA_SIZE`.
        unsafe { self.raw_data.as_mut_ptr().cast::<u8>().add(self.reserved as usize) }
    }

    /// Returns the capacity in bytes of the usable data payload in this buffer.
    fn size(&self) -> usize {
        RAW_DATA_SIZE - (self.reserved as usize)
    }

    /// Returns a mutable slice of the usable data payload in this buffer as maybe-uninitialized
    /// bytes.
    fn as_mut_slice(&mut self) -> &mut [MaybeUninit<u8>] {
        self.canary.assert();
        // SAFETY: `self.raw_data` has length `RAW_DATA_SIZE` and `reserved <= RAW_DATA_SIZE`.
        unsafe {
            slice::from_raw_parts_mut(
                self.raw_data.as_mut_ptr().cast::<MaybeUninit<u8>>().add(self.reserved as usize),
                self.size(),
            )
        }
    }

    /// Initializes an uninitialized `Buffer` in the physical map of the given `page`.
    ///
    /// # Safety
    ///
    /// `page_raw` must be a valid pointer to an allocated `VmPage` with `ALLOC` state.
    unsafe fn init_in_page(page_raw: NonNull<VmPage>, reserved: u32) -> NonNull<Buffer> {
        // SAFETY: Caller guarantees `page_raw` is a valid allocated VmPage.
        let page = unsafe { page_raw.as_ref() };
        debug_assert_eq!(page.state(), VmPageState(vm_page_state::ALLOC));

        // SAFETY: `page` is owned and in the `ALLOC` state.
        unsafe { page.set_state(VmPageState(vm_page_state::IPC)) };

        let va =
            ptr::with_exposed_provenance_mut::<Buffer>(physmap::paddr_to_physmap(page.paddr()).0);

        // SAFETY: `va` is mapped in the kernel physmap for this physical page.
        unsafe {
            va.write(Buffer {
                node: SinglyLinkedListNode::new(),
                canary: Canary::new(),
                reserved,
                raw_data: MaybeUninit::uninit(),
            });
            NonNull::new_unchecked(va)
        }
    }

    /// Returns a slice of the usable data payload in this buffer as maybe-uninitialized bytes.
    #[cfg(ktest)]
    fn as_slice(&self) -> &[MaybeUninit<u8>] {
        self.canary.assert();
        // SAFETY: `self.raw_data` has length `RAW_DATA_SIZE` and `reserved <= RAW_DATA_SIZE`.
        unsafe {
            slice::from_raw_parts(
                self.raw_data.as_ptr().cast::<MaybeUninit<u8>>().add(self.reserved as usize),
                self.size(),
            )
        }
    }
}

/// `BufferChain` is a list of buffers allocated from the PMM, designed for channel messages.
///
/// The `BufferChain` object itself lives *inside* its first buffer and contains intrusive lists
/// (`used_pages`, `unused_pages`) that track physical memory pages backing the buffer chain.
/// Because intrusive list heads contain self-referential pointers, `BufferChain` is pinned
/// (`#[pin_data(PinnedDrop)]`) and cannot be moved in memory. It has no independent heap or
/// stack allocation; its storage is located directly within the `raw_data` payload of the first
/// physical page `Buffer`.
// Take care when adding fields as BufferChain lives inside the first buffer of buffers_.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct BufferChain {
    buffers: SinglyLinkedList<NonNull<Buffer>>,
    // Iterator pointing to the last valid element in the buffer.
    buffer_tail: Option<NonNull<Buffer>>,
    // Position of the next byte to write in the Buffer pointed to by |buffer_tail|.
    buffer_offset: usize,
    // used_pages is a list of VmPage descriptors for the pages that back buffers.
    #[pin]
    used_pages: VmPageDoublyLinkedList,
    // unused_pages is a list of VmPage descriptors for pages that have been allocated but have
    // not yet been used. These pages will be migrated to used_pages once they are needed for
    // buffers.
    #[pin]
    unused_pages: VmPageDoublyLinkedList,
}

// SAFETY: `BufferChain` owns its backing memory and can be transferred across threads.
unsafe impl Send for BufferChain {}
// SAFETY: Concurrent reads to const methods are safe.
unsafe impl Sync for BufferChain {}

/// Size of `BufferChain` which lives inside the first buffer.
const BUFFER_CHAIN_SIZE: usize = size_of::<BufferChain>();

/// `CONTIGUOUS_SIZE` is the number of bytes guaranteed to be stored contiguously in any buffer.
pub const CONTIGUOUS_SIZE: usize = RAW_DATA_SIZE - BUFFER_CHAIN_SIZE;

#[pinned_drop]
impl PinnedDrop for BufferChain {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: We have pinned mutable access during drop.
        let this = unsafe { self.get_unchecked_mut() };
        debug_assert!(this.used_pages.is_empty());
        debug_assert!(this.unused_pages.is_empty());
    }
}

/// Trait abstracting reading from either user memory or kernel memory for append operations.
// |PTR_IN| is a user_in_ptr-like type.
trait SourceReader {
    fn copy_to(&mut self, dst: &mut [MaybeUninit<u8>]) -> Result<(), Status>;
}

/// Adapter for copying from userspace pointers.
struct UserSource(UserInPtr<u8>);

impl SourceReader for UserSource {
    fn copy_to(&mut self, dst: &mut [MaybeUninit<u8>]) -> Result<(), Status> {
        let len = dst.len();
        self.0.copy_slice_from_user(dst)?;
        self.0 = self.0.byte_offset(len as isize);
        Ok(())
    }
}

/// Adapter for copying from kernel slices.
// Makes a &[u8] look like a SourceReader.
// Sometimes we need to copy data from kernel space. KernelSource allows us to implement the copy
// logic once for both &[u8] and UserInPtr<u8>.
struct KernelSource<'a>(&'a [u8]);

impl SourceReader for KernelSource<'_> {
    fn copy_to(&mut self, dst: &mut [MaybeUninit<u8>]) -> Result<(), Status> {
        let len = dst.len();
        let src = &self.0[..len];
        // SAFETY: `dst` has length `len` and `src` has length `len`.
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr().cast::<u8>(), len);
        }
        self.0 = &self.0[len..];
        Ok(())
    }
}

impl BufferChain {
    /// Creates a `BufferChain` with enough buffers to store `size` bytes.
    ///
    /// The chain is initialized in-place inside the first buffer. It must be freed with
    /// `BufferChain::free`.
    ///
    /// Returns `Err(Status)` on error.
    pub fn alloc(size: usize) -> Result<NonNull<Self>, Status> {
        let size = size.checked_add(BUFFER_CHAIN_SIZE).ok_or(Status::OUT_OF_RANGE)?;
        let num_buffers = size.div_ceil(RAW_DATA_SIZE);

        // Allocate a list of pages. If `init_in_pages` fails, `temp_pages` automatically frees
        // any remaining pages back to the PMM upon drop.
        stack_pin_init!(let temp_pages = PageList::new());
        page_cache().alloc(num_buffers, temp_pages.as_mut())?;

        // SAFETY: `temp_pages` contains the allocated pages from the page cache.
        let chain = unsafe {
            let pages = temp_pages.get_unchecked_mut();
            Self::init_in_pages(pages.list_mut())?
        };
        Ok(chain)
    }

    /// Frees `chain` and all its backing buffers and pages.
    ///
    /// # Safety
    ///
    /// `chain` must be a valid, uniquely owned pointer returned from `BufferChain::alloc`.
    pub unsafe fn free(mut chain: NonNull<Self>) {
        // SAFETY: Caller guarantees `chain` is valid and uniquely owned.
        let this = unsafe { chain.as_mut() };
        // Remove the buffers and vm_page_t's from the chain *before* destroying it.
        let mut buffers = mem::replace(&mut this.buffers, SinglyLinkedList::new());

        stack_pin_init!(let all_pages = PageList::new());
        // SAFETY: `all_pages` is pinned on the stack; mutating list does not move `all_pages`.
        let pages = unsafe { all_pages.as_mut().get_unchecked_mut() };
        pages.splice(&mut this.used_pages);
        pages.splice(&mut this.unused_pages);

        // Now both `this.used_pages` and `this.unused_pages` are completely empty.
        // SAFETY: `chain` points to initialized `BufferChain`.
        unsafe {
            ptr::drop_in_place(chain.as_ptr());
        }

        while let Some(buf) = buffers.pop_front() {
            // SAFETY: `buf` points to an initialized `Buffer` allocated in page.
            unsafe {
                ptr::drop_in_place(buf.as_ptr());
            }
        }

        page_cache().free(all_pages);
    }

    /// Initializes `BufferChain` in place inside the first page of `pages`.
    ///
    /// # Safety
    ///
    /// `pages` must contain valid allocated pages with `ALLOC` state.
    unsafe fn init_in_pages(pages: &mut VmPageDoublyLinkedList) -> Result<NonNull<Self>, Status> {
        let first_page = pages.pop_front().ok_or(Status::NO_MEMORY)?;
        // SAFETY: `first_page` was popped from `pages` and is a valid allocated page.
        let first_buffer = unsafe { Buffer::init_in_page(first_page, BUFFER_CHAIN_SIZE as u32) };

        // SAFETY: `first_buffer` is valid and contains space for `BufferChain`.
        let chain_ptr = unsafe { (*first_buffer.as_ptr()).raw_data.as_mut_ptr().cast::<Self>() };
        let init = pin_init!(Self {
            buffers: {
                let mut list = SinglyLinkedList::new();
                // SAFETY: `first_buffer` is an unlinked Buffer pointer.
                unsafe { list.push_front_raw(first_buffer) };
                list
            },
            buffer_tail: Some(first_buffer),
            buffer_offset: 0,
            used_pages <- VmPageDoublyLinkedList::new(),
            unused_pages <- VmPageDoublyLinkedList::new(),
        });

        // SAFETY: `chain_ptr` is properly aligned and sized inside the first buffer.
        if unsafe { init.__pinned_init(chain_ptr) }.is_err() {
            // SAFETY: Push `first_page` back onto `pages` so caller can free it.
            unsafe {
                pages.push_front_raw(first_page);
            }
            return Err(Status::NO_MEMORY);
        }

        // SAFETY: `chain_ptr` is now initialized, `first_page` was allocated for this chain,
        // and `chain_ptr` is non-null.
        unsafe {
            let chain = &mut *chain_ptr;
            chain.used_pages.push_back_raw(first_page);
            chain.unused_pages.splice(pages);
            Ok(NonNull::new_unchecked(chain_ptr))
        }
    }

    /// Takes an unused page, initializes a new `Buffer` in it, and appends it to `buffers`.
    fn add_next_buffer(&mut self) -> Result<(), Status> {
        let page_raw = self.unused_pages.pop_front().ok_or(Status::OUT_OF_RANGE)?;
        let tail = self.buffer_tail.ok_or(Status::OUT_OF_RANGE)?;
        // SAFETY: `page_raw` was popped from `unused_pages` and is a valid page.
        unsafe {
            self.used_pages.push_back_raw(page_raw);
            let va = Buffer::init_in_page(page_raw, 0);
            self.buffers.insert_after_raw(tail.as_ptr(), va);
            self.buffer_tail = Some(va);
        }
        Ok(())
    }

    /// Skips the specified number of bytes, so they won't be consumed by `append_user` or
    /// `append_kernel`.
    ///
    /// Assumes that it is called only at the beginning of the buffer chain.
    pub fn skip(self: Pin<&mut Self>, size: usize) {
        // SAFETY: Modifying unpinned field `buffer_offset` does not move `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        debug_assert_eq!(this.buffer_offset, 0);
        debug_assert!(size <= CONTIGUOUS_SIZE);
        this.buffer_offset += size;
    }

    /// Appends |size| bytes from |src| to this chain.
    ///
    /// If there is insufficient remaining space in the buffer chain, ZX_ERR_OUT_OF_RANGE will be
    /// returned.
    pub fn append_user(
        self: Pin<&mut Self>,
        src: UserInPtr<u8>,
        size: usize,
    ) -> Result<(), Status> {
        self.append_common(UserSource(src), size)
    }

    /// Same as Append except |src| can be in kernel space.
    ///
    /// If there is insufficient remaining space in the buffer chain, ZX_ERR_OUT_OF_RANGE will be
    /// returned.
    pub fn append_kernel(self: Pin<&mut Self>, src: &[u8]) -> Result<(), Status> {
        self.append_common(KernelSource(src), src.len())
    }

    /// Copies |size| bytes from this chain starting at offset |src_offset| to |dst|.
    ///
    /// |src_offset| must be in the range [0, CONTIGUOUS_SIZE).
    pub fn copy_out(
        &self,
        mut dst: UserOutPtr<u8>,
        src_offset: usize,
        size: usize,
    ) -> Result<(), Status> {
        debug_assert!(src_offset < self.buffers.front().unwrap().size());
        let mut copy_offset = src_offset;
        let mut rem = size;
        for buf in self.buffers.iter() {
            if rem == 0 {
                break;
            }
            let copy_len = cmp::min(rem, buf.size() - copy_offset);
            // SAFETY: `copy_len` bytes starting at `copy_offset` were written by append.
            let src = unsafe { slice::from_raw_parts(buf.data().add(copy_offset), copy_len) };
            dst.copy_slice_to_user(src)?;
            dst = dst.byte_offset(copy_len as isize);
            rem -= copy_len;
            copy_offset = 0;
        }
        Ok(())
    }

    /// Free unused pages.
    pub fn free_unused_buffers(self: Pin<&mut Self>) {
        // SAFETY: Mutating `unused_pages` does not move `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        stack_pin_init!(let temp_pages = PageList::new());
        // SAFETY: `temp_pages` is pinned on the stack; mutating list does not move `temp_pages`.
        let pages = unsafe { temp_pages.as_mut().get_unchecked_mut() };
        pages.splice(&mut this.unused_pages);
        page_cache().free(temp_pages);
    }

    /// Common implementation for appending bytes from a source reader.
    fn append_common<R: SourceReader>(
        self: Pin<&mut Self>,
        mut src: R,
        size: usize,
    ) -> Result<(), Status> {
        if size == 0 {
            return Ok(());
        }
        // SAFETY: We hold `Pin<&mut Self>` and obtain `&mut Self` to perform buffer writes without
        // moving `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        let mut rem = size;
        while rem > 0 {
            let Some(mut tail) = this.buffer_tail else {
                return Err(Status::OUT_OF_RANGE);
            };
            // SAFETY: `tail` is verified non-null and points to a valid Buffer.
            let buf = unsafe { tail.as_mut() };
            let copy_len = cmp::min(rem, buf.size() - this.buffer_offset);
            let dst = &mut buf.as_mut_slice()[this.buffer_offset..this.buffer_offset + copy_len];
            if let Err(status) = src.copy_to(dst) {
                this.buffer_tail = None;
                return Err(status);
            }

            this.buffer_offset += copy_len;
            rem -= copy_len;

            if rem > 0 {
                if let Err(status) = this.add_next_buffer() {
                    this.buffer_tail = None;
                    return Err(status);
                }
                this.buffer_offset = 0;
            }
        }
        Ok(())
    }

    /// Returns a mutable pointer to the usable data payload in the first buffer.
    pub fn first_buffer_data_mut(self: Pin<&mut Self>) -> *mut u8 {
        // SAFETY: Inspecting buffers list does not move `Self`.
        let this = unsafe { self.get_unchecked_mut() };
        this.buffers.front_mut().unwrap().data_mut()
    }

    /// Returns true if the buffer chain has no buffers.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Returns the number of buffers currently in the chain.
    #[cfg(ktest)]
    pub fn buffer_count(&self) -> usize {
        self.buffers.size_slow()
    }

    /// Returns an iterator over byte slices of each buffer in the chain.
    #[cfg(ktest)]
    pub fn buffers_data(&self) -> impl Iterator<Item = &[MaybeUninit<u8>]> {
        self.buffers.iter().map(Buffer::as_slice)
    }
}

/// In-tree kernel unit tests for `BufferChain`.
#[cfg(ktest)]
#[unittest::suite(name = "buffer_chain_rust")]
mod tests {
    use super::{BufferChain, CONTIGUOUS_SIZE, RAW_DATA_SIZE};
    use crate::user_memory::UserMemory;
    use core::ops::Deref;
    use unittest::{expect_eq, expect_false, expect_ok, expect_true, unwrap_ok};

    fn make_user_in_byte(size: usize, val: u8) -> Option<(UserMemory, UserInPtr<u8>)> {
        let alloc_size = size.max(1);
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        let chunk = [val; 256];
        let mut offset = 0;
        while offset < size {
            let to_write = cmp::min(chunk.len(), size - offset);
            mem.vmo_write(&chunk[..to_write], offset as u64).ok()?;
            offset += to_write;
        }
        let ptr = UserInPtr::new(ptr::with_exposed_provenance::<u8>(mem.base()));
        Some((mem, ptr))
    }

    fn make_user_out(size: usize) -> Option<(UserMemory, UserOutPtr<u8>)> {
        let alloc_size = size.max(1);
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        let ptr = UserOutPtr::new(ptr::with_exposed_provenance_mut::<u8>(mem.base()));
        Some((mem, ptr))
    }

    fn verify_user_mem_byte(mem: &UserMemory, offset: usize, size: usize, expected: u8) -> bool {
        let mut chunk = [MaybeUninit::<u8>::uninit(); 256];
        let mut curr_offset = offset;
        let end = offset + size;
        while curr_offset < end {
            let to_read = cmp::min(chunk.len(), end - curr_offset);
            let Ok(read_bytes) = mem.vmo_read(&mut chunk[..to_read], curr_offset as u64) else {
                return false;
            };
            for &mut b in read_bytes {
                if b != expected {
                    return false;
                }
            }
            curr_offset += to_read;
        }
        true
    }

    struct TestChain(NonNull<BufferChain>);

    impl TestChain {
        fn alloc(size: usize) -> Result<Self, Status> {
            BufferChain::alloc(size).map(Self)
        }

        fn pin(&mut self) -> Pin<&mut BufferChain> {
            // SAFETY: `self.0` is valid and pinned in its first buffer.
            unsafe { Pin::new_unchecked(self.0.as_mut()) }
        }

        fn skip(&mut self, size: usize) {
            self.pin().skip(size);
        }

        fn append_user(&mut self, src: UserInPtr<u8>, size: usize) -> Result<(), Status> {
            self.pin().append_user(src, size)
        }

        fn append_kernel(&mut self, src: &[u8]) -> Result<(), Status> {
            self.pin().append_kernel(src)
        }

        fn free_unused_buffers(&mut self) {
            self.pin().free_unused_buffers();
        }
    }

    impl Deref for TestChain {
        type Target = BufferChain;

        fn deref(&self) -> &Self::Target {
            // SAFETY: `self.0` is non-null and valid.
            unsafe { self.0.as_ref() }
        }
    }

    impl Drop for TestChain {
        fn drop(&mut self) {
            // SAFETY: `self.0` is uniquely owned by `TestChain`.
            unsafe { BufferChain::free(self.0) };
        }
    }

    /// Tests basic allocation and deallocation of BufferChain.
    #[test]
    fn test_alloc_free_basic() {
        // An empty chain requires one buffer
        let bc = unwrap_ok!(BufferChain::alloc(0));
        let bc_ref = unsafe { bc.as_ref() };
        expect_false!(bc_ref.is_empty());
        expect_eq!(bc_ref.buffer_count(), 1);
        unsafe { BufferChain::free(bc) };

        // One Buffer is enough to hold one byte.
        let bc = unwrap_ok!(BufferChain::alloc(1));
        let bc_ref = unsafe { bc.as_ref() };
        expect_false!(bc_ref.is_empty());
        expect_eq!(bc_ref.buffer_count(), 1);
        unsafe { BufferChain::free(bc) };

        // One Buffer is still enough.
        let bc = unwrap_ok!(BufferChain::alloc(CONTIGUOUS_SIZE));
        let bc_ref = unsafe { bc.as_ref() };
        expect_false!(bc_ref.is_empty());
        expect_eq!(bc_ref.buffer_count(), 1);
        unsafe { BufferChain::free(bc) };

        // Two pages allocated, only one used for the buffer.
        let bc = unwrap_ok!(BufferChain::alloc(CONTIGUOUS_SIZE + 1));
        let bc_ref = unsafe { bc.as_ref() };
        expect_false!(bc_ref.is_empty());
        expect_eq!(bc_ref.buffer_count(), 1);
        unsafe { BufferChain::free(bc) };

        // Several pages allocated, only one used for the buffer.
        let bc = unwrap_ok!(BufferChain::alloc(10000 * RAW_DATA_SIZE));
        let bc_ref = unsafe { bc.as_ref() };
        expect_false!(bc_ref.is_empty());
        expect_eq!(bc_ref.buffer_count(), 1);
        unsafe { BufferChain::free(bc) };
    }

    /// Tests appending data from user memory and copying it back out.
    #[test]
    fn test_append_copy_out() {
        const OFFSET: usize = 24;
        const FIRST_COPY: usize = CONTIGUOUS_SIZE + 8;
        const SECOND_COPY: usize = RAW_DATA_SIZE + 16;
        const SIZE: usize = OFFSET + FIRST_COPY + SECOND_COPY;

        let (_mem_a, mem_a_in) = make_user_in_byte(FIRST_COPY, b'A').unwrap();
        let (_mem_b, mem_b_in) = make_user_in_byte(SECOND_COPY, b'B').unwrap();
        let (mem_out_holder, mem_out) = make_user_out(SIZE).unwrap();

        let mut bc = unwrap_ok!(TestChain::alloc(SIZE));
        expect_eq!(bc.buffer_count(), 1);

        bc.skip(OFFSET);

        // Fill the chain with 'A'.
        expect_ok!(bc.append_user(mem_a_in, FIRST_COPY));

        // Verify it.
        {
            let mut bufs = bc.buffers_data();
            let buf0 = bufs.next().unwrap();
            for &byte in &buf0[OFFSET..CONTIGUOUS_SIZE] {
                // SAFETY: Byte in this range was written by append_user.
                expect_eq!(unsafe { byte.assume_init() }, b'A');
            }
            let buf1 = bufs.next().unwrap();
            for &byte in &buf1[..(OFFSET + FIRST_COPY - CONTIGUOUS_SIZE)] {
                // SAFETY: Byte in this range was written by append_user.
                expect_eq!(unsafe { byte.assume_init() }, b'A');
            }
        }

        // Write a chunk of 'B' straddling all three buffers.
        expect_ok!(bc.append_user(mem_b_in, SECOND_COPY));

        // Verify it.
        {
            let mut bufs = bc.buffers_data();
            let buf0 = bufs.next().unwrap();
            for &byte in &buf0[OFFSET..CONTIGUOUS_SIZE] {
                // SAFETY: Byte in this range was written by append.
                expect_eq!(unsafe { byte.assume_init() }, b'A');
            }
            let buf1 = bufs.next().unwrap();
            for &byte in &buf1[..(OFFSET + FIRST_COPY - CONTIGUOUS_SIZE)] {
                // SAFETY: Byte in this range was written by append.
                expect_eq!(unsafe { byte.assume_init() }, b'A');
            }
            for &byte in &buf1[(OFFSET + FIRST_COPY - CONTIGUOUS_SIZE)..RAW_DATA_SIZE] {
                // SAFETY: Byte in this range was written by append_user.
                expect_eq!(unsafe { byte.assume_init() }, b'B');
            }
            let buf2 = bufs.next().unwrap();
            for &byte in
                &buf2[..(OFFSET + FIRST_COPY + SECOND_COPY - CONTIGUOUS_SIZE - RAW_DATA_SIZE)]
            {
                // SAFETY: Byte in this range was written by append_user.
                expect_eq!(unsafe { byte.assume_init() }, b'B');
            }
            expect_true!(bufs.next().is_none());
        }

        // Copy it all out.
        expect_ok!(bc.copy_out(mem_out, 0, SIZE));

        // Verify it.
        expect_true!(verify_user_mem_byte(&mem_out_holder, OFFSET, FIRST_COPY, b'A'));
        expect_true!(verify_user_mem_byte(&mem_out_holder, OFFSET + FIRST_COPY, SECOND_COPY, b'B'));
    }

    /// Tests appending data from kernel memory and copying it back out.
    #[test]
    fn test_append_kernel() {
        const SIZE: usize = 8192;
        static KERNEL_DATA: [u8; SIZE] = [0x5A; SIZE];

        let mut bc = unwrap_ok!(TestChain::alloc(SIZE));
        expect_ok!(bc.append_kernel(&KERNEL_DATA));
        expect_eq!(bc.buffer_count(), 3);

        let (mem_out_holder, mem_out) = make_user_out(SIZE).unwrap();
        expect_ok!(bc.copy_out(mem_out, 0, SIZE));

        expect_true!(verify_user_mem_byte(&mem_out_holder, 0, SIZE, 0x5A));
    }

    /// Tests freeing unused pages from the chain.
    #[test]
    fn test_free_unused_pages() {
        const SIZE: usize = 8 * page::SIZE;
        const WRITE_SIZE: usize = CONTIGUOUS_SIZE + 1;

        let (_mem, mem_in) = make_user_in_byte(WRITE_SIZE, 0).unwrap();

        let mut bc = unwrap_ok!(TestChain::alloc(SIZE));
        expect_eq!(bc.buffer_count(), 1);

        expect_ok!(bc.append_user(mem_in, WRITE_SIZE));

        expect_eq!(bc.buffer_count(), 2);
        bc.free_unused_buffers();
        expect_eq!(bc.buffer_count(), 2);
    }

    /// Tests that appending more than allocated returns OUT_OF_RANGE.
    #[test]
    fn test_append_more_than_allocated() {
        const ALLOC_SIZE: usize = 2 * page::SIZE;
        const WRITE_SIZE: usize = 2 * ALLOC_SIZE;

        let (_mem, mem_in) = make_user_in_byte(WRITE_SIZE, 0).unwrap();

        let mut bc = unwrap_ok!(TestChain::alloc(ALLOC_SIZE));
        expect_eq!(bc.buffer_count(), 1);

        let res = bc.append_user(mem_in, WRITE_SIZE);
        expect_true!(matches!(res, Err(Status::OUT_OF_RANGE)));
    }

    /// Tests that append after a failed append also fails.
    #[test]
    fn test_append_after_fail_fails() {
        const ALLOC_SIZE: usize = 2 * page::SIZE;
        const WRITE_SIZE: usize = page::SIZE;

        let (_mem, mem_in) = make_user_in_byte(WRITE_SIZE, 0).unwrap();

        let mut bc = unwrap_ok!(TestChain::alloc(ALLOC_SIZE));
        expect_eq!(bc.buffer_count(), 1);

        let bad_in = UserInPtr::new(ptr::null());
        let res = bc.append_user(bad_in, WRITE_SIZE);
        expect_true!(matches!(res, Err(Status::INVALID_ARGS)));

        let res2 = bc.append_user(mem_in, WRITE_SIZE);
        expect_true!(matches!(res2, Err(Status::OUT_OF_RANGE)));
    }
}
