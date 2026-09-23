// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_ARCH_VM_ASPACE_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_ARCH_VM_ASPACE_H_

#include <lib/fit/function.h>
#include <sys/types.h>
#include <zircon/types.h>

#include <fbl/macros.h>
#include <vm/page.h>

using arch_mmu_flags_t = uint8_t;
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_CACHED = (0u << 0);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_UNCACHED = (1u << 0);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_UNCACHED_DEVICE =
    (2u << 0);  // Only exists on some arches, otherwise UNCACHED
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_WRITE_COMBINING =
    (3u << 0);  // Only exists on some arches, otherwise UNCACHED
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_CACHE_MASK = (3u << 0);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_PERM_USER = (1u << 2);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_PERM_READ = (1u << 3);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_PERM_WRITE = (1u << 4);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_PERM_EXECUTE = (1u << 5);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_PERM_RWX_MASK =
    (ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE);
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_NS = (1u << 6);  // NON-SECURE
static constexpr arch_mmu_flags_t ARCH_MMU_FLAG_INVALID =
    (1u << 7);  // Indicates that flags are not specified

static constexpr arch_mmu_flags_t ARCH_ASPACE_FLAG_KERNEL = (1u << 0);
static constexpr arch_mmu_flags_t ARCH_ASPACE_FLAG_GUEST = (1u << 1);

constexpr bool arch_mmu_flags_uncached(arch_mmu_flags_t mmu_flags) {
  return (mmu_flags & (ARCH_MMU_FLAG_UNCACHED | ARCH_MMU_FLAG_UNCACHED_DEVICE)) != 0;
}

// per arch base class api to encapsulate the mmu routines on an aspace
//
// Beyond construction/destruction lifetimes users of this object must ensure that none of the
// main methods are called before calling Init or after calling Destroy. Doing so is allowed to
// cause a panic.
// Aside from Init and Destroy, the main methods are all thread-safe.
class ArchVmAspaceInterface {
 public:
  ArchVmAspaceInterface() = default;
  virtual ~ArchVmAspaceInterface() = default;

  // Function pointer to allocate a single page that the mmu routine uses to allocate
  // page tables.
  using page_alloc_fn_t = zx_status_t (*)(uint alloc_flags, vm_page** p, paddr_t* pa);

  // The Init* methods are used to initialize the ArchVmAspace. The method that should be used
  // is dependent on the type of address space being created.
  //
  // `Init`: This is used to create a regular address space with no special features. In
  //    architectures that do not support unified address spaces, it is also used to create
  //    shared and restricted address spaces. However, when unified address spaces are
  //    supported, the shared and restricted address spaces should be created with `InitShared`
  //    and `InitRestricted`.
  //
  // `InitShared`: This is used to create a shared address space, whose contents can be
  //    accessed from multiple unified address spaces. These address spaces have a statically
  //    initialized top level page.
  //
  // `InitRestricted`: This is used to create a restricted address space, whose contents can be
  //    accessed from a single unified address space.
  //
  // `InitUnified`: This is used to create a unified address space. This type of address space
  //    owns no mappings of its own; rather, it is composed of a shared address space and a
  //    restricted address space. As a result, it expects `InitShared` to have been called
  //    on the shared address space, and expects `InitRestricted` to have been called on the
  //    restricted address space.
  virtual zx_status_t Init() = 0;
  virtual zx_status_t InitShared() = 0;
  virtual zx_status_t InitRestricted() = 0;
  virtual zx_status_t InitUnified(ArchVmAspaceInterface& shared,
                                  ArchVmAspaceInterface& restricted) = 0;

  // This method puts the instance into read-only mode and asserts that it contains no mappings.
  //
  // Note, this method may be a no-op on some architectures.  See https://fxbug.dev/42159319.
  //
  // It is an error to call this method on an instance that contains mappings.  Once called,
  // subsequent operations that modify the page table will trigger a panic.
  //
  // The purpose of this method is to help enforce lifecycle and state transitions of VmAspace and
  // ArchVmAspaceInterface.
  virtual void DisableUpdates() = 0;

  // Destroy expects the aspace to be fully unmapped, as any mapped regions indicate incomplete
  // cleanup at the higher layers. Note that this does not apply to unified aspaces, which may
  // still contain some mappings when Destroy() is called.
  //
  // It is safe to call Destroy even if Init, InitShared, InitRestricted, or InitUnified failed.
  // Once destroy has been called it is a user error to call any of the other methods on the aspace,
  // unless specifically stated otherwise, and doing so may cause a panic.
  virtual zx_status_t Destroy() = 0;

  // main methods
  // Map a physically contiguous region into the virtual address space. This is allowed to use any
  // page size the architecture allows given the from the input parameters.
  virtual zx_status_t MapContiguous(vaddr_t vaddr, paddr_t paddr, size_t count,
                                    arch_mmu_flags_t mmu_flags) = 0;

  // Map the given array of pages into the virtual address space starting at
  // |vaddr|, in the order they appear in |phys|.
  //
  // If any address in the range [vaddr, vaddr + count * kPageSize) is already
  // mapped when this is called, |existing_action| controls the behavior used:
  //  - |Skip| - Skip updating any existing mappings.
  //  - |Error| - Existing mappings result in a ZX_ERR_ALREADY_EXISTS error.
  //  - |Upgrade| - Upgrade any existing mappings, meaning a read-only mapping
  //                can be converted to read-write, or the mapping can have its
  //                paddr changed.
  //
  // On error none of the provided pages will be mapped. In the case of |Upgrade| the state of any
  // previous mappings is undefined, and could either still be present or be unmapped.
  enum class ExistingEntryAction : uint8_t {
    Skip,
    Error,
    Upgrade,
  };
  virtual zx_status_t Map(vaddr_t vaddr, paddr_t* phys, size_t count, arch_mmu_flags_t mmu_flags,
                          ExistingEntryAction existing_action) = 0;

  // Options for unmapping the given virtual address range.
  // ArchUnmapOptions::Enlarge controls whether the unmap region can be extended to be larger, or if
  // only the exact region may be unmapped. The unmap region might be extended, even if only
  // temporarily, if large pages need to be split.
  //
  // ArchUnmapOptions::Harvest requests that the accessed bit be harvested, and the page queues
  // updated.
  enum class ArchUnmapOptions : uint8_t {
    None = 0,
    Enlarge = (1u << 0),
    Harvest = (1u << 1),
  };

  virtual zx_status_t Unmap(vaddr_t vaddr, size_t count, ArchUnmapOptions enlarge) = 0;

  // Returns whether or not an unmap might need to enlarge an operation for reasons other than being
  // out of memory. If this returns true, then unmapping a partial large page will fail always
  // require an enlarged operation.
  virtual bool UnmapOnlyEnlargeOnOom() const = 0;

  // Change the page protections on the given virtual address range
  //
  // May return ZX_ERR_NO_MEMORY if the operation requires splitting
  // a large page and the next level page table allocation fails. In
  // this case, mappings in the input range may be a mix of the old and
  // new flags.
  // ArchUnmapOptions controls whether the a larger range than requested is permitted to experience
  // a temporary permissions change. A temporary change may be required if a break-before-make style
  // unmap -> remap of the large page is required.
  virtual zx_status_t Protect(vaddr_t vaddr, size_t count, arch_mmu_flags_t mmu_flags,
                              ArchUnmapOptions enlarge) = 0;

  virtual zx_status_t Query(vaddr_t vaddr, paddr_t* paddr, arch_mmu_flags_t* mmu_flags) = 0;

  virtual vaddr_t PickSpot(vaddr_t base, vaddr_t end, vaddr_t align, size_t size,
                           arch_mmu_flags_t mmu_flags) = 0;

  // For HarvestAccessed Terminal and non-terminal get processed based on the following two
  // controls.
  enum class NonTerminalAction : bool {
    // If a non-terminal entry has no accessed information, unmap and free it. If it has accessed
    // information, just remove the flag.
    FreeUnaccessed,
    // Retain both the non-terminal mappings and any accessed information.
    Retain,
  };
  enum class TerminalAction : bool {
    // If the page is accessed update its age in the page queues, and remove the accessed flag.
    UpdateAgeAndHarvest,
    // If the page is accessed update its age in the page queues, but do not clear the flag.
    UpdateAge,
  };
  // Walks the given range of pages and for any pages that are mapped and have their access bit set
  //  * Tells the page queues it has been accessed via PageQueues::MarkAccessed
  //  * Potentially removes the accessed flag.
  //  * Potentially frees unaccessed page tables.
  virtual zx_status_t HarvestAccessed(vaddr_t vaddr, size_t count,
                                      NonTerminalAction non_terminal_action,
                                      TerminalAction terminal_action) = 0;

  // Marks any pages in the given virtual address range as being accessed.
  virtual zx_status_t MarkAccessed(vaddr_t vaddr, size_t count) = 0;

  // Returns whether or not this aspace might have additional accessed information since the last
  // time this method was called with clear=true. If this returns |false| then, modulo races,
  // HarvestAccessed is defined to not find any set bits and not call PageQueues::MarkAccessed.
  //
  // This is intended for use by the harvester to avoid scanning for any accessed or dirty bits if
  // the aspace has not been accessed at all.
  //
  // Note that restricted and shared ArchVmAspace's will report that they have been accessed if an
  // associated unified ArchVmAspace has been accessed. However, the reverse is not true; the
  // unified ArchVmAspace will not return true if the associated shared/restricted aspaces have been
  // accessed.
  //
  // The |clear| flag controls whether the aspace having been accessed should be cleared or not. Not
  // clearing makes this function const and not modify any state. If |clear| is true then this
  // method is only thread-compatible and must be externally synchronized.
  virtual bool AccessedSinceLastCheck(bool clear) = 0;

  // Physical address of the backing data structure used for translation.
  //
  // This should be treated as an opaque value outside of
  // architecture-specific components.
  virtual paddr_t arch_table_phys() const = 0;
};

// Per arch base class API to encapsulate routines for maintaining icache consistency.
class ArchVmICacheConsistencyManagerInterface {
 public:
  ArchVmICacheConsistencyManagerInterface() = default;
  virtual ~ArchVmICacheConsistencyManagerInterface() = default;

  // Indicate that the given kernel address range may have modified data. The given range is not
  // actually guaranteed to be synced until |Finish| is called. All aliases of the given range are
  // guaranteed to be consistent after |Finish|.
  virtual void SyncAddr(vaddr_t start, size_t len) = 0;

  // Perform any final synchronization operations. This may be used by an implementation to
  // efficiently batch operations, and no addresses should be considered actually synchronized
  // until this returns.
  // This is automatically called on destruction.
  virtual void Finish() = 0;
};

FBL_ENABLE_ENUM_BITS(ArchVmAspaceInterface::ArchUnmapOptions)

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_ARCH_VM_ASPACE_H_
