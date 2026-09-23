// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/memalloc/pool.h>

#include <fbl/no_destructor.h>
#include <ktl/array.h>
#include <phys/allocation.h>
#include <phys/arch/arch-allocation.h>

#include <ktl/enforce.h>

void Allocation::Init(ktl::span<memalloc::Range> mem_ranges,
                      ktl::span<memalloc::Range> special_ranges,
                      memalloc::Pool::AccessCallback access_callback) {
  // We page-align the special ranges (rounding down start addresses and
  // rounding up sizes) since these ranges in practice are handed off to the
  // kernel as page-aligned VMOs. Aligning them here prevents avoiding
  // unintended inclusions in tail and arena selection complications (which
  // also deals in whole pages).
  const uint64_t page_size = PageSize();
  for (auto& range : special_ranges) {
    range = range.AlignTo(page_size);
  }

  // Use fbl::NoDestructor to avoid generation of static destructors,
  // which fails in the phys environment.
  static fbl::NoDestructor<memalloc::Pool> pool;

  constexpr uint64_t kMin = kArchAllocationMinAddr.value_or(memalloc::Pool::kDefaultMinAddr);
  constexpr uint64_t kMax = kArchAllocationMaxAddr.value_or(memalloc::Pool::kDefaultMaxAddr);

  if (access_callback) {
    pool->set_access_callback(ktl::move(access_callback));
  }

  ktl::array ranges{mem_ranges, special_ranges};
  ZX_ASSERT(pool->Init(ranges, kMin, kMax).is_ok());

  if (kArchAllocationMinAddr) {
    // While the pool will now prevent allocation in
    // [0, *kArchAllocationMinAddr), it can be strictly enforced by allocating
    // all such RAM now. Plus, it is convenient to distinguish this memory at
    // hand-off time.
    ZX_ASSERT(pool->UpdateRamSubranges(memalloc::Type::kReservedLow, 0, kMin).is_ok());
  }

  // Install the pool for GetPool.
  InitWithPool(*pool);
}
