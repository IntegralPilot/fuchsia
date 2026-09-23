// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/allocation.h"

#include <inttypes.h>
#include <zircon/assert.h>

#include <new>

#include <efi/boot-services.h>
#include <fbl/algorithm.h>
#include <phys/efi/main.h>
#include <phys/efi/page-size.h>

// The phys Allocation type is supported in EFI via AllocatePages/FreePages.

namespace {

constexpr size_t EfiPageCount(size_t bytes) { return (bytes + kEfiPageSize - 1) / kEfiPageSize; }

}  // namespace

namespace std {

const nothrow_t nothrow;

}  // namespace std

// This is where actual allocation happens.
// The returned object is default-constructed if it fails.
Allocation Allocation::New(fbl::AllocChecker& ac, memalloc::Type type, size_t size,
                           size_t alignment, ktl::optional<uint64_t> min_addr,
                           ktl::optional<uint64_t> max_addr) {
  ZX_ASSERT(!min_addr);
  ZX_ASSERT(!max_addr);

  Allocation alloc;
  alloc.type_ = type;

  // If we need larger alignment, allocate extra pages to ensure we can get it.
  size_t alloc_size = fbl::round_up(size, kEfiPageSize);
  if (alignment > kEfiPageSize) {
    alloc_size += 2 * alignment;
  }

  efi_physical_addr paddr = 0;
  efi_status status = gEfiSystemTable->BootServices->AllocatePages(
      AllocateAnyPages, EfiLoaderData, EfiPageCount(alloc_size), &paddr);
  ac.arm(size, status == EFI_SUCCESS);
  if (status == EFI_SUCCESS) {
    const uintptr_t addr = static_cast<uintptr_t>(paddr);
    ZX_ASSERT(addr == paddr);
    uintptr_t aligned_addr = fbl::round_up(addr, alignment);
    alloc.data_ = {reinterpret_cast<ktl::byte*>(aligned_addr), size};

    // Trim excess pages before the aligned block.
    const size_t pages_before = EfiPageCount(aligned_addr - addr);
    if (pages_before > 0) {
      status = gEfiSystemTable->BootServices->FreePages(addr, pages_before);
      ZX_ASSERT_MSG(status == EFI_SUCCESS, "FreePages(%#" PRIx64 ", %#zx) -> %#zx", addr,
                    pages_before, status);
    }

    // Trim excess pages after the aligned block.
    uintptr_t after = aligned_addr + fbl::round_up(size, kEfiPageSize);
    const size_t pages_after = EfiPageCount(addr + alloc_size - after);
    if (pages_after > 0) {
      status = gEfiSystemTable->BootServices->FreePages(after, pages_after);
      ZX_ASSERT_MSG(status == EFI_SUCCESS, "FreePages(%#" PRIxPTR ", %#zx) -> %#zx", after,
                    pages_after, status);
    }
  }

  return alloc;
}

void Allocation::Resize(fbl::AllocChecker& ac, size_t new_size) {
  ZX_ASSERT(!data_.empty());
  ZX_ASSERT(new_size > 0);

  const size_t old_pages = EfiPageCount(size_bytes());
  const size_t new_pages = EfiPageCount(new_size);

  if (new_pages <= old_pages) {
    if (new_pages < old_pages) {
      const efi_physical_addr addr =
          reinterpret_cast<uintptr_t>(get()) + (old_pages * kEfiPageSize);
      const size_t free_pages = old_pages - new_pages;
      efi_status status = gEfiSystemTable->BootServices->FreePages(addr, free_pages);
      ZX_ASSERT_MSG(status == EFI_SUCCESS, "FreePages(%#" PRIx64 ", %#zx) -> %#zx", addr,
                    free_pages, status);
    }

    // Even if no pages were freed, adjust our tracked sub-page size.
    ac.arm(new_size, true);
    data_ = {data_.data(), new_size};
    return;
  }

  // Try first to allocate the immediately following pages.
  efi_physical_addr addr = reinterpret_cast<uintptr_t>(get()) + old_pages;
  efi_status status = gEfiSystemTable->BootServices->AllocatePages(AllocateAddress, EfiLoaderData,
                                                                   new_pages - old_pages, &addr);
  if (status == EFI_SUCCESS) {
    ZX_DEBUG_ASSERT(addr == reinterpret_cast<uintptr_t>(get()) + old_pages);
    ac.arm(new_size, true);
    data_ = {data_.data(), new_size};
    return;
  }

  // No dice.  Allocate a fresh block and copy the data in.
  if (Allocation new_block = New(ac, memalloc::Type::kPhysScratch, new_size, alignment_)) {
    memcpy(new_block.get(), get(), size_bytes());
    *this = ktl::move(new_block);
  }
}

void Allocation::Extend(fbl::AllocChecker& ac, size_t new_size) {
  ZX_ASSERT(!data_.empty());
  ZX_ASSERT(new_size > size_bytes());
  ZX_DEBUG_ASSERT(type_ != memalloc::Type::kMaxAllocated);
  size_t base_addr = reinterpret_cast<size_t>(data_.data());
  size_t page_addr = base_addr & ~(kEfiPageSize - 1);
  size_t extra_bytes = base_addr - page_addr;

  size_t new_pages = EfiPageCount(new_size + extra_bytes);
  size_t old_pages = EfiPageCount(size_bytes() + extra_bytes);

  // EFI allocation is page aligned, so `new_size` might be contained in the current set of pages.
  if (new_pages == old_pages) {
    ac.arm(new_size, true);
    return;
  }

  efi_physical_addr addr = page_addr + old_pages;
  size_t extra_pages = new_pages - old_pages;

  if (efi_status result = gEfiSystemTable->BootServices->AllocatePages(
          AllocateAddress, EfiLoaderData, extra_pages, &addr);
      result == EFI_SUCCESS) {
    ZX_DEBUG_ASSERT(addr == page_addr + old_pages);
    ac.arm(new_size, true);
    data_ = {data_.data(), new_size};
    return;
  }

  // Allocation failed.
  ac.arm(size_bytes(), false);
}

size_t Allocation::PageSize() { return kEfiPageSize; }

// This is where actual deallocation happens.  The destructor just calls this.
void Allocation::reset() {
  if (!data_.empty()) {
    size_t page_addr = reinterpret_cast<size_t>(data_.data());
    efi_physical_addr aligned_addr = page_addr & ~(kEfiPageSize - 1);
    size_t bytes = size_bytes() + page_addr - aligned_addr;
    efi_status status = gEfiSystemTable->BootServices->FreePages(aligned_addr, EfiPageCount(bytes));
    ZX_ASSERT_MSG(status == EFI_SUCCESS, "FreePages(%#" PRIx64 ", %#zx) -> %#zx", aligned_addr,
                  EfiPageCount(bytes), status);
    data_ = {};
  }
}

// Plain new/delete is supported in EFI via AllocatePool/FreePool.
// Aligned variants are not supported.
void* operator new(size_t size, const std::nothrow_t&) noexcept {
  void* ptr;
  efi_status status = gEfiSystemTable->BootServices->AllocatePool(EfiLoaderData, size, &ptr);
  return status == 0 ? ptr : nullptr;
}

void operator delete(void* ptr, const std::nothrow_t&) noexcept {
  gEfiSystemTable->BootServices->FreePool(ptr);
}

void* operator new[](size_t size, const std::nothrow_t&) noexcept {
  return operator new(size, std::nothrow);
}

void operator delete[](void* ptr, const std::nothrow_t&) noexcept {
  operator delete(ptr, std::nothrow);
}

void* operator new(size_t size) { return operator new(size, std::nothrow); }

void operator delete(void* ptr) { operator delete(ptr, std::nothrow); }

void* operator new[](size_t size) { return operator new(size, std::nothrow); }

void operator delete[](void* ptr) { operator delete(ptr, std::nothrow); }

void operator delete(void* ptr, size_t size) { operator delete(ptr, std::nothrow); }

void operator delete[](void* ptr, size_t size) { operator delete(ptr, std::nothrow); }
