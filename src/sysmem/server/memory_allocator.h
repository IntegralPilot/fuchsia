// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_MEMORY_ALLOCATOR_H_
#define SRC_SYSMEM_SERVER_MEMORY_ALLOCATOR_H_

#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/bti.h>
#include <lib/zx/vmo.h>

#include <map>

#include "protected_ranges.h"

class SnapshotAnnotationRegister;
class SysmemMetrics;

namespace sysmem_service {

class MemoryAllocator {
 public:
  // Some sub-classes take this interface as a constructor param, which
  // enables a fake in tests where we don't have a real zx::bti etc.
  class Owner {
   public:
    [[nodiscard]] virtual inspect::Node* heap_node() = 0;
    [[nodiscard]] virtual const zx::bti& bti() = 0;
    [[nodiscard]] virtual zx::result<zx::vmo> CreatePhysicalVmo(uint64_t base, uint64_t size) = 0;
    // Should be called after every delete that makes the allocator empty.
    virtual void CheckForUnbind() {}
    [[nodiscard]] virtual std::optional<SnapshotAnnotationRegister>&
    snapshot_annotation_register() = 0;
    [[nodiscard]] virtual SysmemMetrics& metrics() = 0;
    [[nodiscard]] virtual protected_ranges::ProtectedRangesCoreControl&
    protected_ranges_core_control(const fuchsia_sysmem2::Heap& heap) {
      // Avoid requiring unrelated tests to implement.
      ZX_PANIC("protected_ranges_core_control() not implemented by subclass");
    }
    [[nodiscard]] virtual bool protected_ranges_disable_dynamic() const { return false; }
    virtual void OnAllocationFailure() {}
  };

  explicit MemoryAllocator(fuchsia_hardware_sysmem::HeapProperties properties);

  virtual ~MemoryAllocator();

  // size - the needed size in bytes, rounded up to a page boundaryß
  //
  // min_physical_alignment - the minimum alignment of the physical base address of the allocation;
  // a MemoryAllocator that isn't allocating physically contiguous memory can either completely
  // ignore this parameter, or fail with ZX_ERR_NOT_SUPPORTED if this has_value()
  //
  // settings - allocators may observe settings if needed, but are discouraged from looking at
  // settings unless really needed; settings.buffer_settings.size_bytes is the logical size in bytes
  // not rounded up to a page boundary, so most allocators will only care about `size` (first
  // parameter).
  //
  // name - a name that can be useful for debugging
  //
  // buffer_collection_id + buffer_index - The buffer_collection_id and buffer_index are retrievable
  // from a sysmem-provided VMO using GetVmoInfo(). Because the sysmem-provided VMO may be a child
  // (or grandchild) of the allocated VMO, the koid of parent_vmo is not generally retrievable from
  // a sysmem-provide VMO derived from parent_vmo. Allocators which don't need to correlate a
  // sysmem-provided VMO to a parent_vmo can ignore buffer_collection_id and buffer_index.
  //
  // parent_vmo - return the allocated VMO
  //
  // returns zx_status_t - the error may be useful for debugging, but the error code indicated to
  // sysmem clients will be sanitized to ZX_ERR_NO_MEMORY.
  virtual zx_status_t Allocate(uint64_t size, std::optional<uint64_t> min_physical_alignment,
                               const fuchsia_sysmem2::SingleBufferSettings& settings,
                               std::optional<std::string> name, uint64_t buffer_collection_id,
                               uint32_t buffer_index, zx::vmo* parent_vmo) = 0;

  // This also should clean up any tracking of buffer_collection_id + buffer_index (the specific
  // combination), as zero sysmem-provided VMOs exist at this point, so it's impossible for any
  // sysmem-provided VMO to show up with the parent_vmo's buffer_collection_id + buffer_index.
  //
  // This call takes ownership of parent_vmo, and should close parent_vmo so that the memory used by
  // parent_vmo can be freed/reclaimed/recycled.
  virtual void Delete(zx::vmo parent_vmo) = 0;

  virtual zx_status_t GetPhysicalMemoryInfo(uint64_t* base, uint64_t* size) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  const fuchsia_hardware_sysmem::HeapProperties& heap_properties() const {
    return heap_properties_;
  }

  // These avoid the possibility of trying to use a sysmem-configured secure
  // heap before the TEE has told the HW to make the physical range
  // secure/protected.  The default SetReady() implementation panics, and the
  // default is_ready() just returns true.
  virtual void set_ready();
  virtual void clear_ready();
  virtual bool is_ready();

  void AddDestroyCallback(intptr_t key, fit::callback<void()> callback);
  void RemoveDestroyCallback(intptr_t key);

  // Returns true if there are no outstanding allocations, or if the allocator only allocates fully
  // independent VMOs that fully own their own memory separate from any tracking in sysmem.
  // Allocators must be empty before they're deleted.
  virtual bool is_empty() = 0;

  uint64_t id() const { return id_; }

  virtual bool is_already_cleared_on_allocate() { return false; }

 public:
  std::map<intptr_t, fit::callback<void()>> destroy_callbacks_;

 private:
  // This is a unique ID for the allocator on this system.
  uint64_t id_{};
  fuchsia_hardware_sysmem::HeapProperties heap_properties_;
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_MEMORY_ALLOCATOR_H_
