// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "asid_allocator.h"

#include <debug.h>
#include <trace.h>
#include <zircon/types.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

AsidAllocator::AsidAllocator(enum arm64_asid_width width_override) {
  bitmap_.Reset(MMU_ARM64_MAX_USER_ASID_16 + 1);

  // save whether or not the cpu only supports 8 bits which is fairly exceptional.
  // most support the full 16 bit ASID space.
  asid_width_ = (width_override != arm64_asid_width::UNKNOWN) ? width_override : arm64_asid_width();
  DEBUG_ASSERT(asid_width_ == arm64_asid_width::ASID_8 || asid_width_ == arm64_asid_width::ASID_16);
}

AsidAllocator::~AsidAllocator() {}

zx::result<uint16_t> AsidAllocator::Alloc() {
  uint16_t new_asid;

  // use the bitmap allocator to allocate ids in the range of
  // [MMU_ARM64_FIRST_USER_ASID, MMU_ARM64_MAX_USER_ASID]
  // start the search from the last found id + 1 and wrap when hitting the end of the range
  {
    Guard<Mutex> al{&lock_};

    size_t val;
    bool notfound = bitmap_.Get(last_ + 1, max_user_asid() + 1, &val);
    if (unlikely(notfound)) {
      // search again from the start
      notfound = bitmap_.Get(MMU_ARM64_FIRST_USER_ASID, max_user_asid() + 1, &val);
      if (unlikely(notfound)) {
        return zx::error(ZX_ERR_NO_MEMORY);
      }
    }
    bitmap_.SetOne(val);

    DEBUG_ASSERT(val <= max_user_asid());

    new_asid = (uint16_t)val;
    last_ = new_asid;
  }

  LTRACEF("new asid %#x\n", new_asid);

  return zx::ok(new_asid);
}

zx::result<> AsidAllocator::Free(uint16_t asid) {
  LTRACEF("free asid %#x\n", asid);

  Guard<Mutex> al{&lock_};

  bitmap_.ClearOne(asid);

  return zx::ok();
}
