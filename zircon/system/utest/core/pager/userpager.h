// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_PAGER_USERPAGER_H_
#define ZIRCON_SYSTEM_UTEST_CORE_PAGER_USERPAGER_H_

#include <lib/fit/function.h>
#include <lib/zx/event.h>
#include <lib/zx/pager.h>
#include <lib/zx/port.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/port.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>

#include <fbl/intrusive_double_list.h>

#include "test_thread.h"

namespace pager_tests {

class UserPager;

// This class is thread-safe and operations may be called concurrently, from the main test thread
// and from additional instances of TestThread that the main thread spawns.
// Some member functions will serialize behind a mutex, so callers should take care that this is
// expected behavior. At the time of writing this comment, this was the case.
//  - Some tests call these functions only from the main test thread, in which case the lock is
//  essentially a no-op.
//  - Some tests that spawn TestThread instances are already serialized due to tight
//  synchronization between blocking on page requests and resolving them.
//  - Other tests that expect concurrent execution do not care about a particular order in
//  which threads run, so it does not matter whether they were serialized behind a userspace lock
//  (this mutex) or a lock in the kernel.
class Vmo : public fbl::DoublyLinkedListable<std::unique_ptr<Vmo>> {
 public:
  ~Vmo() {
    std::lock_guard guard(mutex_);
    if (size_ != 0) {
      zx::vmar::root_self()->unmap(base_addr_, size_);
    }
  }

  static std::unique_ptr<Vmo> Create(zx::vmo vmo, uint64_t size, uint64_t key) {
    ZX_ASSERT(size % zx_system_get_page_size() == 0);
    zx_vaddr_t addr = 0;
    if (size != 0) {
      zx_status_t status =
          zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &addr);
      if (status != ZX_OK) {
        fprintf(stderr, "vmar map failed with %s\n", zx_status_get_string(status));
        return nullptr;
      }
    }

    return std::unique_ptr<Vmo>(new Vmo(std::move(vmo), size, addr, key));
  }

  // Resizes the vmo.
  bool Resize(uint64_t new_page_count);

  // Generates this vmo contents at the specified offset.
  void GenerateBufferContents(void* dest_buffer, uint64_t page_count,
                              uint64_t paged_vmo_page_offset) const;

  // Validates this vmo's content in the specified pages using a mapped vmar.
  bool CheckVmar(uint64_t page_offset, uint64_t page_count, const void* expected = nullptr) const;
  // Validates this vmo's content in the specified pages using vmo_read.
  bool CheckVmo(uint64_t page_offset, uint64_t page_count, const void* expected = nullptr) const;

  // Commits the specified pages in this vmo.
  bool Commit(uint64_t page_offset, uint64_t page_count) const {
    return OpRange(ZX_VMO_OP_COMMIT, page_offset, page_count);
  }

  bool Prefetch(uint64_t page_offset, uint64_t page_count) const {
    return OpRange(ZX_VMO_OP_PREFETCH, page_offset, page_count);
  }

  std::unique_ptr<Vmo> Clone(uint32_t options = ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE |
                                                ZX_VMO_CHILD_RESIZABLE) const {
    // Lock is held to read size_.
    std::lock_guard guard(mutex_);
    return CloneLocked(0, size_, options);
  }

  std::unique_ptr<Vmo> Clone(uint64_t offset, uint64_t size,
                             uint32_t options = ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE |
                                                ZX_VMO_CHILD_RESIZABLE,
                             uint32_t map_perms = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE) const {
    // Hold the lock for cloning to prevent a Resize from sneaking in mid-operation.
    std::lock_guard guard(mutex_);
    return CloneLocked(offset, size, options, map_perms);
  }

  std::unique_ptr<Vmo> CloneLocked(uint64_t offset, uint64_t size,
                                   uint32_t options = ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE |
                                                      ZX_VMO_CHILD_RESIZABLE,
                                   uint32_t map_perms = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE) const
      __TA_REQUIRES(&mutex_);

  size_t PollNumChildren(size_t expected_children) const;

  bool PollPopulatedBytes(size_t expected_bytes) const;

  uint64_t size() const {
    std::lock_guard guard(mutex_);
    return size_;
  }
  uintptr_t base_addr() const {
    std::lock_guard guard(mutex_);
    return base_addr_;
  }
  const zx::vmo& vmo() const { return vmo_; }
  uint64_t key() const { return key_; }

  // Set the limit of the range that may be automatically supplied to this VMO from the
  // UserPager::StartTaggedPageFaultHandler thread. Changing this limit is intended to allow for
  // tests that will re-inspect supplied regions and want to be resilient against eviction, but do
  // not want the page fault handler from spuriously succeeding the test by accidentally handling
  // requests it is not supposed to. To ensure a lack of races this method must be called *before*
  // initially supplying the range that you then want to have auto supplied.
  void SetPageFaultSupplyLimit(uint64_t pages_limit) {
    std::lock_guard guard(mutex_);
    page_fault_supply_limit_ = pages_limit * zx_system_get_page_size();
  }

  // Retrieve the current page fault supply limit in pages.
  uint64_t GetPageFaultSupplyLimit() const {
    std::lock_guard guard(mutex_);
    return page_fault_supply_limit_;
  }

 private:
  Vmo(zx::vmo vmo, uint64_t size, uint64_t base_addr, uint64_t key)
      : size_(size), base_addr_(base_addr), vmo_(std::move(vmo)), key_(key) {}

  bool OpRange(uint32_t op, uint64_t page_offset, uint64_t page_count) const;

  // Use this mutex to protect state as sparingly as possible; the primary objective of this lock is
  // to prevent data races.
  //  - Do not hold it on paths that might block on page requests, because the UserPager might need
  //  the lock to resolve the page requests too, and we will deadlock.
  //  - Do not hold it over long critical sections as it might defeat the intended concurrency of
  //  test threads by serializing on this mutex instead.
  mutable std::mutex mutex_;

  // These are set in the ctor, but can be changed by Vmo::Resize.
  // The region described by this range remains mapped for the lifetime of the object, and we
  // are responsible for unmapping it on destruction.
  uint64_t size_ __TA_GUARDED(&mutex_);
  uintptr_t base_addr_ __TA_GUARDED(&mutex_);

  uint64_t page_fault_supply_limit_ __TA_GUARDED(&mutex_) = UINT64_MAX;

  // vmo_ and key_ are set in the ctor.
  const zx::vmo vmo_;
  // This value is used for both the port packet key and to populate the contents of supplied pages.
  const uint64_t key_;
};

// This class is not thread-safe and is only expected to be accessed from the main test thread.
class UserPager {
 public:
  UserPager();
  ~UserPager();

  // Initialzies the UserPager.
  bool Init();
  //  Closes the pager handle.
  void ClosePagerHandle() { pager_.reset(); }
  // Closes the pager's port handle.
  void ClosePortHandle() { port_.reset(); }
  // Returns a reference to the pager's port handle.
  const zx::port& port() const { return port_; }

  // Creates a new paged vmo.
  bool CreateVmo(uint64_t num_pages, Vmo** vmo_out);
  // Creates a new paged vmo with the provided create |options|.
  bool CreateVmoWithOptions(uint64_t num_pages, uint32_t options, Vmo** vmo_out);
  // Create a VMO of type ZX_VMO_UNBOUNDED. The resulting Vmo only has limited support and cannot
  // be accessed via its mapping. Additional |options| may also be specified, as long as they do not
  // conflict with ZX_VMO_UNBOUNDED.
  bool CreateUnboundedVmo(uint64_t initial_stream_size, uint32_t options, Vmo** vmo_out);
  // Detaches the paged vmo.
  bool DetachVmo(Vmo* vmo);
  // Destroys the paged vmo.
  void ReleaseVmo(Vmo* vmo);

  // Populates the specified pages with autogenerated content. |src_page_offset| is used
  // to offset where in the temporary vmo the content is generated.
  bool SupplyPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count,
                   uint64_t src_page_offset = 0);
  // Populates the specified pages with the content in |src| starting at |src_page_offset|.
  bool SupplyPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count, zx::vmo src,
                   uint64_t src_page_offset = 0);

  // Signals failure to populate pages in the specified range.
  bool FailPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count,
                 zx_status_t error_status = ZX_ERR_IO);

  // Signals that pages in the specified range can be marked dirty.
  bool DirtyPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count);

  // Queries dirty ranges of pages in the specified range and verifies that they match the ones
  // provided in |dirty_ranges_to_verify|. The number of entries in |dirty_ranges_to_verify| is
  // passed in with |num_dirty_ranges_to_verify|.
  bool VerifyDirtyRanges(Vmo* paged_vmo, zx_vmo_dirty_range_t* dirty_ranges_to_verify,
                         size_t num_dirty_ranges_to_verify);
  // Queries pager vmo stats, and returns whether the |paged_vmo| has been modified since the last
  // query.
  bool VerifyModified(Vmo* paged_vmo);

  // Begins and ends writeback on pages in the specified range.
  bool WritebackBeginPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count);
  bool WritebackBeginZeroPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count);
  bool WritebackEndPages(Vmo* vmo, uint64_t page_offset, uint64_t page_count);

  // Checks if there is a request for the range [page_offset, length). Will
  // wait until |deadline|.
  bool WaitForPageRead(Vmo* vmo, uint64_t page_offset, uint64_t page_count,
                       zx_instant_mono_t deadline);
  bool WaitForPageDirty(Vmo* vmo, uint64_t page_offset, uint64_t page_count,
                        zx_instant_mono_t deadline);
  bool WaitForPageComplete(uint64_t key, zx_instant_mono_t deadline);

  // Gets the first page read request. Blocks until |deadline|.
  bool GetPageReadRequest(Vmo* vmo, zx_instant_mono_t deadline, uint64_t* page_offset,
                          uint64_t* page_count);
  // Gets the first page dirty request. Blocks until |deadline|.
  bool GetPageDirtyRequest(Vmo* vmo, zx_instant_mono_t deadline, uint64_t* page_offset,
                           uint64_t* page_count);
  // Gets the first page request with |command|. Blocks until |deadline|.
  bool GetPageRequest(Vmo* vmo, uint16_t command, zx_instant_mono_t deadline, uint64_t* page_offset,
                      uint64_t* page_count);

  // Starts a thread to handle any page faults. Faulted in pages are initialized with the default
  // page tagged data as per SupplyPages. This function is not thread safe, and should only be
  // called once. After starting a pager thread it is an error to create or destroy VMOs, as this
  // could lead to data races.
  // The individual VMOs can, optionally, have the maximum offset of a fault that will be handled
  // through their respective SetPageFaultSupplyLimit methods. Any page request outside these limits
  // will be dropped and ignored, and cannot be retrieved through any of the GetPageRequest or
  // similar methods.
  bool StartTaggedPageFaultHandler();

  const zx::pager& pager() const { return pager_; }

 private:
  bool WaitForPageRequest(uint16_t command, Vmo* vmo, uint64_t page_offset, uint64_t page_count,
                          zx_instant_mono_t deadline);
  bool WaitForRequest(Vmo* vmo, uint64_t key, const zx_packet_page_request_t& request,
                      zx_instant_mono_t deadline);
  bool WaitForRequest(Vmo* vmo, fit::function<bool(const zx_port_packet_t& packet)> cmp_fn,
                      zx_instant_mono_t deadline);
  void PageFaultHandler();
  bool VerifyDirtyRangesHelper(Vmo* paged_vmo, zx_vmo_dirty_range_t* dirty_ranges_to_verify,
                               size_t num_dirty_ranges_to_verify, zx_vmo_dirty_range_t* ranges_buf,
                               size_t ranges_buf_size, uint64_t* num_ranges_buf);

  bool CreateVmoInternal(uint64_t byte_size, uint32_t options, Vmo** vmo_out);

  void OvertimeHandler();
  void DumpRequestsLocked() __TA_REQUIRES(pager_mutex_);
  void DumpVmosLocked() __TA_REQUIRES(pager_mutex_);

  zx::pager pager_;
  zx::port port_;
  static constexpr uint64_t kShutdownKey = 1;
  uint64_t next_key_ = kShutdownKey + 1;

  // Lock to guard modifications to vmos_ and requests_ so the OvertimeHandler can safely inspect
  // them.
  mutable std::mutex pager_mutex_;

  fbl::DoublyLinkedList<std::unique_ptr<Vmo>> vmos_ __TA_GUARDED(pager_mutex_);

  typedef struct request : fbl::DoublyLinkedListable<std::unique_ptr<struct request>> {
    zx_port_packet_t req;
  } request_t;

  fbl::DoublyLinkedList<std::unique_ptr<request_t>> requests_ __TA_GUARDED(pager_mutex_);

  zx::event shutdown_event_;
  TestThread pager_thread_;
  zx::event overtime_event_;
  TestThread timeout_thread_;
};

inline bool check_buffer_data(Vmo* vmo, uint64_t offset, uint64_t len, const void* data,
                              bool check_vmar) {
  return check_vmar ? vmo->CheckVmar(offset, len, data) : vmo->CheckVmo(offset, len, data);
}

inline bool check_buffer(Vmo* vmo, uint64_t offset, uint64_t len, bool check_vmar) {
  return check_vmar ? vmo->CheckVmar(offset, len) : vmo->CheckVmo(offset, len);
}

#define VMO_VMAR_TEST(test_name, fn_name)           \
  void fn_name(bool);                               \
  TEST(test_name, fn_name##Vmar) { fn_name(true); } \
  TEST(test_name, fn_name##Vmo) { fn_name(false); } \
  void fn_name(bool check_vmar)

}  // namespace pager_tests

#endif  // ZIRCON_SYSTEM_UTEST_CORE_PAGER_USERPAGER_H_
