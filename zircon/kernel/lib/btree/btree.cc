// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/btree.h>
#include <lib/lazy_init/lazy_init.h>

#include <lk/init.h>
#include <vm/page_slab_allocator.h>

namespace {

DECLARE_SINGLETON_CRITICAL_MUTEX(SlabLock32);
lazy_init::LazyInit<PageSlabAllocator<32>, lazy_init::CheckType::None,
                    lazy_init::Destructor::Disabled>
    node_slab32_ TA_GUARDED(SlabLock32::Get());
DECLARE_SINGLETON_CRITICAL_MUTEX(SlabLock64);
lazy_init::LazyInit<PageSlabAllocator<64>, lazy_init::CheckType::None,
                    lazy_init::Destructor::Disabled>
    node_slab64_ TA_GUARDED(SlabLock64::Get());
DECLARE_SINGLETON_CRITICAL_MUTEX(SlabLock128);
lazy_init::LazyInit<PageSlabAllocator<128>, lazy_init::CheckType::None,
                    lazy_init::Destructor::Disabled>
    node_slab128_ TA_GUARDED(SlabLock128::Get());
DECLARE_SINGLETON_CRITICAL_MUTEX(SlabLock256);
lazy_init::LazyInit<PageSlabAllocator<256>, lazy_init::CheckType::None,
                    lazy_init::Destructor::Disabled>
    node_slab256_ TA_GUARDED(SlabLock256::Get());
DECLARE_SINGLETON_CRITICAL_MUTEX(SlabLock512);
lazy_init::LazyInit<PageSlabAllocator<512>, lazy_init::CheckType::None,
                    lazy_init::Destructor::Disabled>
    node_slab512_ TA_GUARDED(SlabLock512::Get());

// This runs in early init before mutex support might be available and where we are still single
// threaded, so disable analysis for this method.
void init_slab_allocators(uint32_t /*level*/) TA_NO_THREAD_SAFETY_ANALYSIS {
  node_slab32_.Initialize();
  node_slab64_.Initialize();
  node_slab128_.Initialize();
  node_slab256_.Initialize();
  node_slab512_.Initialize();
}

}  // namespace

namespace btree {

void* GlobalSlabAllocator::allocate(size_t size_align) {
  if (size_align == 32) {
    Guard<CriticalMutex> guard{SlabLock32::Get()};
    return node_slab32_->allocate_bytes();
  }
  if (size_align == 64) {
    Guard<CriticalMutex> guard{SlabLock64::Get()};
    return node_slab64_->allocate_bytes();
  }
  if (size_align == 128) {
    Guard<CriticalMutex> guard{SlabLock128::Get()};
    return node_slab128_->allocate_bytes();
  }
  if (size_align == 256) {
    Guard<CriticalMutex> guard{SlabLock256::Get()};
    return node_slab256_->allocate_bytes();
  }
  if (size_align == 512) {
    Guard<CriticalMutex> guard{SlabLock512::Get()};
    return node_slab512_->allocate_bytes();
  }
  ZX_ASSERT(false);
  return nullptr;
}

void GlobalSlabAllocator::deallocate(size_t size_align, void* ptr) {
  if (size_align == 32) {
    Guard<CriticalMutex> guard{SlabLock32::Get()};
    node_slab32_->deallocate_bytes(ptr);
  } else if (size_align == 64) {
    Guard<CriticalMutex> guard{SlabLock64::Get()};
    node_slab64_->deallocate_bytes(ptr);
  } else if (size_align == 128) {
    Guard<CriticalMutex> guard{SlabLock128::Get()};
    node_slab128_->deallocate_bytes(ptr);
  } else if (size_align == 256) {
    Guard<CriticalMutex> guard{SlabLock256::Get()};
    node_slab256_->deallocate_bytes(ptr);
  } else if (size_align == 512) {
    Guard<CriticalMutex> guard{SlabLock512::Get()};
    node_slab512_->deallocate_bytes(ptr);
  } else {
    ZX_ASSERT(false);
  }
}

LK_INIT_HOOK(global_page_slab_init, init_slab_allocators, LK_INIT_LEVEL_VM_PREHEAP - 1)

}  // namespace btree
