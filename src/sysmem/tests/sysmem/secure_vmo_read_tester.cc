// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "secure_vmo_read_tester.h"

SecureVmoReadTester::SecureVmoReadTester(zx::vmo secure_vmo_to_delete)
    : secure_vmo_to_delete_(std::move(secure_vmo_to_delete)),
      unowned_secure_vmo_(secure_vmo_to_delete_) {
  Init();
}

SecureVmoReadTester::SecureVmoReadTester(zx::unowned_vmo unowned_secure_vmo)
    : unowned_secure_vmo_(std::move(unowned_secure_vmo)) {
  Init();
}

void SecureVmoReadTester::Init() {
  // We need a child VMAR so we can clean up robustly without relying on a fault
  // to occur at location where a VMO was recently mapped but which
  // theoretically something else could be mapped unless we're specific with a
  // VMAR that isn't letting something else get mapped there yet.
  zx_vaddr_t child_vaddr;
  EXPECT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size(), &child_vmar_, &child_vaddr));

  uint64_t vmo_size;
  EXPECT_OK(unowned_secure_vmo_->get_size(&vmo_size));
  EXPECT_EQ(vmo_size % zx_system_get_page_size(), 0);
  uint64_t vmo_offset =
      (distribution_(prng_) % (vmo_size / zx_system_get_page_size())) * zx_system_get_page_size();

  uintptr_t map_addr_raw;
  EXPECT_OK(child_vmar_.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_MAP_RANGE,
                            0, *unowned_secure_vmo_, vmo_offset, zx_system_get_page_size(),
                            &map_addr_raw));
  map_addr_ = reinterpret_cast<uint8_t*>(map_addr_raw);
  EXPECT_EQ(reinterpret_cast<uint8_t*>(child_vaddr), map_addr_);

  // No data should be in CPU cache for a secure VMO; no fault should happen here.
  EXPECT_OK(zx_cache_flush(reinterpret_cast<void*>(map_addr_raw), zx_system_get_page_size(),
                           ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

  // But currently the read doesn't visibly fault while the vaddr is mapped to
  // a secure page.  Instead the read gets stuck and doesn't complete (perhaps
  // internally faulting from kernel's point of view).  While that's not ideal,
  // we can check that the thread doing the reading doesn't get anything from
  // the read while mapped to a secure page, and then let the thread fault
  // normally by unmapping the secure VMO.
  let_die_thread_ = std::thread([this] {
    is_let_die_started_ = true;
    // Ensure is_read_from_secure_attempted_ becomes true before we start
    // waiting.  This just increases the likelihood that we wait long enough
    // for the read itself to potentially execute (expected to fault instead).
    while (!is_read_from_secure_attempted_) {
      nanosleep_duration(zx::msec(10));
    }
    // Wait 10ms for the read attempt to succeed; the read attempt should not
    // succeed.  The read attempt may fail immediately or may get stuck.  It's
    // possible we might very occasionally not wait long enough for the read
    // to have actually started - if that occurs the test will "pass" without
    // having actually attempted the read.
    nanosleep_duration(zx::msec(10));
    // Let thread running fn die if it hasn't already (if it got stuck, let it
    // no longer be stuck).
    //
    // By removing ZX_VM_PERM_READ, if the read is stuck, the read will cause a
    // process-visible fault instead.  We don't zx_vmar_unmap() here because the
    // syscall docs aren't completely clear on whether zx_vmar_unmap() might
    // make the vaddr page available for other uses.
    EXPECT_OK(
        child_vmar_.protect(0, reinterpret_cast<uintptr_t>(map_addr_), zx_system_get_page_size()));
  });

  while (!is_let_die_started_) {
    nanosleep_duration(zx::msec(10));
  }
}

SecureVmoReadTester::~SecureVmoReadTester() {
  if (let_die_thread_.joinable()) {
    let_die_thread_.join();
  }

  child_vmar_.destroy();
}

bool SecureVmoReadTester::IsReadFromSecureAThing() {
  EXPECT_TRUE(is_let_die_started_);
  EXPECT_TRUE(is_read_from_secure_attempted_);
  return is_read_from_secure_a_thing_;
}

void SecureVmoReadTester::AttemptReadFromSecure(bool expect_read_success) {
  EXPECT_TRUE(is_let_die_started_);
  EXPECT_TRUE(!is_read_from_secure_attempted_);
  is_read_from_secure_attempted_ = true;
  // This attempt to read from a vaddr that's mapped to a secure paddr won't succeed.  For now the
  // read gets stuck while mapped to secure memory, and then faults when we've unmapped the VMO.
  // This address is in a child VMAR so we know nothing else will be getting mapped to the vaddr.
  //
  // The loop is mainly for the benefit of debugging/fixing the test should the very first write,
  // flush, read not force and fence a fault.
  for (uint32_t i = 0; i < zx_system_get_page_size(); ++i) {
    map_addr_[i] = 0xF0;
    EXPECT_OK(zx_cache_flush((const void*)&map_addr_[i], 1,
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
    uint8_t value = map_addr_[i];
    // Despite the flush above often causing the fault to be sync, sometimes the fault doesn't
    // happen but we read zero.  For now, only complain if we read back something other than zero.
    if (value != 0) {
      is_read_from_secure_a_thing_ = true;
    }
    constexpr bool kDumpPageContents = false;
    if (!expect_read_success && kDumpPageContents) {
      if (i % 64 == 0) {
        printf("%08x: ", i);
      }
      printf("%02x ", value);
      if ((i + 1) % 64 == 0) {
        printf("\n");
      }
    }
  }
  if (!expect_read_success) {
    printf("\n");
    // If we made it through the whole page without faulting, yet only read zero, consider that
    // success in the sense that we weren't able to read anything in secure memory.  Cause the thead
    // to "die" here on purpose so the test can pass.  This is not the typical case, but can happen
    // at least on sherlock.  Typically we fault during the write, flush, read of byte 0 above.
    ZX_PANIC("didn't fault, but also didn't read non-zero, so pretend to fault");
  }
}
