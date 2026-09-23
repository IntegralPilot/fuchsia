// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <ctype.h>
#include <inttypes.h>
#include <lib/arch/intrin.h>
#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/fzl/memory-probe.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/pager.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/object.h>

#include <algorithm>
#include <atomic>
#include <iterator>
#include <thread>
#include <vector>

#include <arch/kernel_aspace.h>
#include <explicit-memory/bytes.h>
#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

#include "helpers.h"
#include "test_thread.h"
#include "userpager.h"

namespace {

const size_t kPageSize = zx_system_get_page_size();

TEST(VmoTestCase, Create) {
  zx_status_t status;
  zx_handle_t vmo[16];

  // allocate a bunch of vmos then free them
  for (size_t i = 0; i < std::size(vmo); i++) {
    status = zx_vmo_create(i * zx_system_get_page_size(), 0, &vmo[i]);
    EXPECT_OK(status, "vm_object_create");
  }

  for (zx_handle_t vmo_handle : vmo) {
    EXPECT_OK(zx_handle_close(vmo_handle));
  }
}

TEST(VmoTestCase, ReadWriteBadLen) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object and attempt read/write from it, with bad length
  const size_t len = zx_system_get_page_size() * 4;
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  std::vector<char> buf(len);
  for (int i = 1; i <= 2; i++) {
    status = zx_vmo_read(vmo, buf.data(), 0,
                         std::numeric_limits<size_t>::max() - (zx_system_get_page_size() / i));
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status);
    status = zx_vmo_write(vmo, buf.data(), 0,
                          std::numeric_limits<size_t>::max() - (zx_system_get_page_size() / i));
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status);
  }
  status = zx_vmo_read(vmo, buf.data(), 0, len);
  EXPECT_OK(status, "vmo_read");
  status = zx_vmo_write(vmo, buf.data(), 0, len);
  EXPECT_OK(status, "vmo_write");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, ReadWrite) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object and read/write from it
  const size_t len = zx_system_get_page_size() * 4;
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  std::vector<char> buf(len);
  status = zx_vmo_read(vmo, buf.data(), 0, buf.size());
  EXPECT_OK(status, "vm_object_read");

  // make sure it's full of zeros
  size_t count = 0;
  for (auto c : buf) {
    EXPECT_EQ(c, 0, "zero test");
    if (c != 0) {
      printf("char at offset %#zx is bad\n", count);
    }
    count++;
  }

  memset(buf.data(), 0x99, buf.size());
  status = zx_vmo_write(vmo, buf.data(), 0, buf.size());
  EXPECT_OK(status, "vm_object_write");

  // map it
  uintptr_t ptr;
  status =
      zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, len, &ptr);
  EXPECT_OK(status, "vm_map");
  EXPECT_NE(0u, ptr, "vm_map");

  // check that it matches what we last wrote into it
  EXPECT_BYTES_EQ(buf.data(), (reinterpret_cast<const char *>(ptr)), buf.size(), "mapped buffer");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "vm_unmap");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, ReadWriteRange) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object
  const size_t len = zx_system_get_page_size() * 4;
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // fail to read past end
  std::vector<char> buf(len * 2);
  status = zx_vmo_read(vmo, buf.data(), 0, buf.size());
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_read past end");

  // Successfully read 0 bytes at end
  status = zx_vmo_read(vmo, buf.data(), len, 0);
  EXPECT_OK(status, "vm_object_read zero at end");

  // Fail to read 0 bytes past end
  status = zx_vmo_read(vmo, buf.data(), len + 1, 0);
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_read zero past end");

  // fail to write past end
  status = zx_vmo_write(vmo, buf.data(), 0, buf.size());
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_write past end");

  // Successfully write 0 bytes at end
  status = zx_vmo_write(vmo, buf.data(), len, 0);
  EXPECT_OK(status, "vm_object_write zero at end");

  // Fail to read 0 bytes past end
  status = zx_vmo_write(vmo, buf.data(), len + 1, 0);
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_write zero past end");

  // Test for unsigned wraparound
  status = zx_vmo_read(vmo, buf.data(), UINT64_MAX - (len / 2), len);
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_read offset + len wraparound");
  status = zx_vmo_write(vmo, buf.data(), UINT64_MAX - (len / 2), len);
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE, "vm_object_write offset + len wraparound");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, Map) {
  zx_status_t status;
  zx_handle_t vmo;
  uintptr_t ptr[3] = {};

  // allocate a vmo
  status = zx_vmo_create(4 * zx_system_get_page_size(), 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // do a regular map
  ptr[0] = 0;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(),
                       &ptr[0]);
  EXPECT_OK(status, "map");
  EXPECT_NE(0u, ptr[0], "map address");
  // printf("mapped %#" PRIxPTR "\n", ptr[0]);

  // try to map something completely out of range without any fixed mapping, should succeed
  ptr[2] = UINTPTR_MAX;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, zx_system_get_page_size(),
                       &ptr[2]);
  EXPECT_OK(status, "map");
  EXPECT_NE(0u, ptr[2], "map address");

  // try to map something completely out of range fixed, should fail
  uintptr_t map_addr;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_SPECIFIC, UINTPTR_MAX, vmo, 0,
                       zx_system_get_page_size(), &map_addr);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status, "map");

  // cleanup
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");

  for (auto p : ptr) {
    if (p) {
      status = zx_vmar_unmap(zx_vmar_root_self(), p, zx_system_get_page_size());
      EXPECT_OK(status, "unmap");
    }
  }
}

// Ensure that zx_vmo_read and zx_vmo_write  returns ZX_ERR_NOT_FOUND with a null buffer. Ensure
// that it does not return ERR_INVALID_ARGS. See: https://fxbug.dev/42054407
TEST(VmoTestCase, ReadWriteInvalidNullBuffer) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), "vmo create");
  EXPECT_STATUS(vmo.read(nullptr, 0, 1), ZX_ERR_NOT_FOUND, "vmo read nullptr");
  EXPECT_STATUS(vmo.write(nullptr, 0, 1), ZX_ERR_NOT_FOUND, "vmo write nullptr");
}

// Ensure that zx_vmo_read and zx_vmo_write returns ZX_ERR_NOT_FOUND with a kernel buffer. Ensure
// that it does not return ERR_INVALID_ARGS. See: https://fxbug.dev/42054407
TEST(VmoTestCase, ReadWriteInvalidKernelBuffer) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), "vmo create");

  // buffer address starting in the kernel
  void *const kernel_start = reinterpret_cast<void *>(KERNEL_ASPACE_BASE);
  EXPECT_STATUS(vmo.read(kernel_start, 0, 10), ZX_ERR_NOT_FOUND, "vmo read kernel");
  EXPECT_STATUS(vmo.write(kernel_start, 0, 10), ZX_ERR_NOT_FOUND, "vmo write kernel");

  // buffer spanning between between userspace and kernel space
  uintptr_t before_kernel_start_addr;
  ASSERT_FALSE(sub_overflow(KERNEL_ASPACE_BASE, 5, &before_kernel_start_addr));
  void *const before_kernel_start = reinterpret_cast<void *>(before_kernel_start_addr);
  EXPECT_STATUS(vmo.read(before_kernel_start, 0, 10), ZX_ERR_NOT_FOUND, "vmo read before kernel");
  EXPECT_STATUS(vmo.write(before_kernel_start, 0, 10), ZX_ERR_NOT_FOUND, "vmo write before kernel");
}

// Ensure that zx_vmo_read and zx_vmo_write return ZX_ERR_NOT_FOUND with an unmapped buffer. Ensure
// that it does not return ERR_INVALID_ARGS. See: https://fxbug.dev/42054407
TEST(VmoTestCase, ReadWriteInvalidUserBuffer) {
  const size_t mapped_size = zx_system_get_page_size();
  const size_t vmo_size = 2 * mapped_size;
  const auto root_vmar = zx::vmar::root_self();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo), "vmo create");

  uintptr_t vmar_addr;
  zx::vmar vmar;
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, mapped_size, &vmar,
                                &vmar_addr));
  const auto destroy_vmar = fit::defer([&]() { EXPECT_OK(vmar.destroy()); });

  void *const vmar_ptr = reinterpret_cast<void *>(vmar_addr);

  EXPECT_STATUS(vmo.read(vmar_ptr, 0, 1), ZX_ERR_NOT_FOUND, "vmo read unmapped");
  EXPECT_STATUS(vmo.write(vmar_ptr, 0, 1), ZX_ERR_NOT_FOUND, "vmo write unmapped");

  // let's assert that read and write would have worked if we back `vmar`.

  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &buffer_vmo), "buffer vmo create");
  uintptr_t vmar_addr2;
  ASSERT_OK(
      vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, buffer_vmo, 0, mapped_size, &vmar_addr2));
  EXPECT_EQ(vmar_addr, vmar_addr2);

  // read and write with same arguments as above is now okay.
  EXPECT_OK(vmo.read(vmar_ptr, 0, 1), "vmo read mapped");
  EXPECT_OK(vmo.write(vmar_ptr, 0, 1), "vmo write mapped");

  // but still fails if we reach the end of the region of the vmar that is vmo backed.
  EXPECT_STATUS(vmo.read(vmar_ptr, 0, mapped_size + 1), ZX_ERR_NOT_FOUND, "vmo read mapped oob");
  EXPECT_STATUS(vmo.write(vmar_ptr, 0, mapped_size + 1), ZX_ERR_NOT_FOUND, "vmo write mapped oob");

  // fails instead with out of range when we reach the end of the vmo backing our buffer.
  EXPECT_STATUS(vmo.read(vmar_ptr, 0, vmo_size + 1), ZX_ERR_OUT_OF_RANGE,
                "vmo read mapped out of range");
  EXPECT_STATUS(vmo.write(vmar_ptr, 0, vmo_size + 1), ZX_ERR_OUT_OF_RANGE,
                "vmo write mapped out of range");
}

TEST(VmoTestCase, MapRead) {
  zx::vmo vmo;

  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));

  uintptr_t vaddr;
  // Map in the first page
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &vaddr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(vaddr, zx_system_get_page_size());
  });

  // Read from the second page of the vmo to mapping.
  // This should succeed and not deadlock in the kernel.
  EXPECT_OK(vmo.read(reinterpret_cast<void *>(vaddr), zx_system_get_page_size(),
                     zx_system_get_page_size()));
}

// This test attempts to write to a memory location from multiple threads
// while other threads are decommitting that same memory.
//
// The expected behavior is that the writes succeed without crashing
// the kernel.
//
// See https://fxbug.dev/42145844 for more details.
TEST(VmoTestCase, ParallelWriteAndDecommit) {
  const size_t kVmoSize = zx_system_get_page_size();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  uintptr_t base;
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, kVmoSize, &base));
  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(base, kVmoSize);
  });

  ASSERT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, kVmoSize, nullptr, 0));

  constexpr size_t kNumWriters = 2;
  constexpr size_t kNumDecommitters = 2;

  void *dst = reinterpret_cast<void *>(base);

  std::atomic<size_t> running = 0;
  std::atomic<bool> start = false;

  auto writer = [dst, &running, &start] {
    running++;
    while (!start) {
      sched_yield();
    }
    for (size_t i = 0; i < 100000; i++) {
      mandatory_memset(dst, 0x1, 128);
    }
  };

  auto decommitter = [&vmo, &running, &start, kVmoSize] {
    running++;
    while (!start) {
      sched_yield();
    }
    for (size_t i = 0; i < 100000; i++) {
      EXPECT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, kVmoSize, nullptr, 0));
    }
  };

  std::vector<std::thread> threads;

  for (size_t i = 0; i < kNumDecommitters; i++) {
    threads.push_back(std::thread(decommitter));
  }
  for (size_t i = 0; i < kNumWriters; i++) {
    threads.push_back(std::thread(writer));
  }

  while (running < kNumDecommitters + kNumWriters) {
    sched_yield();
  }
  start = true;

  for (auto &thread : threads) {
    thread.join();
  }
}

TEST(VmoTestCase, ParallelRead) {
  constexpr size_t kNumPages = 1024;
  zx::vmo vmo1, vmo2;

  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * kNumPages, 0, &vmo1));
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * kNumPages, 0, &vmo2));

  uintptr_t vaddr1, vaddr2;
  // Map the bottom half of both in.
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo1, 0,
                                       zx_system_get_page_size() * (kNumPages / 2), &vaddr1));
  auto unmap1 = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(vaddr1, zx_system_get_page_size() * (kNumPages / 2));
  });

  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo2, 0,
                                       zx_system_get_page_size() * (kNumPages / 2), &vaddr2));
  auto unmap2 = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(vaddr2, zx_system_get_page_size() * (kNumPages / 2));
  });

  // Spin up a thread to read from one of the vmos, whilst we read from the other
  auto vmo_read_closure = [&vmo1, &vaddr2] {
    vmo1.read(reinterpret_cast<void *>(vaddr2), zx_system_get_page_size() * (kNumPages / 2),
              zx_system_get_page_size() * (kNumPages / 2));
  };
  std::thread thread(vmo_read_closure);
  // As these two threads read from one vmo into the mapping from another vmo if there are any
  // scenarios where the kernel would try and hold both vmo locks at the same time (without
  // attempting to resolve lock ordering) then this should trigger a deadlock to occur.
  EXPECT_OK(vmo2.read(reinterpret_cast<void *>(vaddr1), zx_system_get_page_size() * (kNumPages / 2),
                      zx_system_get_page_size() * (kNumPages / 2)));
  thread.join();
}

TEST(VmoTestCase, ReadOnlyMap) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object and read/write from it
  const size_t len = zx_system_get_page_size();
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // map it
  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, len, &ptr);
  EXPECT_OK(status, "vm_map");
  EXPECT_NE(0u, ptr, "vm_map");

  EXPECT_STATUS(probe_for_write(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "write");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "vm_unmap");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, NoPermMap) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object and read/write from it
  const size_t len = zx_system_get_page_size();
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // map it with read permissions
  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, len, &ptr);
  EXPECT_OK(status, "vm_map");
  EXPECT_NE(0u, ptr, "vm_map");

  // protect it to no permissions
  status = zx_vmar_protect(zx_vmar_root_self(), 0, ptr, len);
  EXPECT_OK(status, "vm_protect");

  // test reading writing to the mapping
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "read");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "write");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "vm_unmap");

  // close the handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");
}

TEST(VmoTestCase, NoPermProtect) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object and read/write from it
  const size_t len = zx_system_get_page_size();
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // map it with no permissions
  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(), 0, 0, vmo, 0, len, &ptr);
  EXPECT_OK(status, "vm_map");
  EXPECT_NE(0u, ptr, "vm_map");

  // test writing to the mapping
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "write");
  // test reading to the mapping
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "read");

  // protect it to read permissions and make sure it works as expected
  status = zx_vmar_protect(zx_vmar_root_self(), ZX_VM_PERM_READ, ptr, len);
  EXPECT_OK(status, "vm_protect");

  // test writing to the mapping
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void *>(ptr)), ZX_ERR_ACCESS_DENIED, "write");

  // test reading from the mapping
  EXPECT_OK(probe_for_read(reinterpret_cast<void *>(ptr)), "read");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "vm_unmap");

  // close the handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");
}

TEST(VmoTestCase, Resize) {
  zx_status_t status;
  zx_handle_t vmo;

  // allocate an object
  size_t len = zx_system_get_page_size() * 4;
  status = zx_vmo_create(len, ZX_VMO_RESIZABLE, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // get the size that we set it to
  uint64_t size = 0x99999999;
  status = zx_vmo_get_size(vmo, &size);
  EXPECT_OK(status, "vm_object_get_size");
  EXPECT_EQ(len, size, "vm_object_get_size");

  // try to resize it
  len += zx_system_get_page_size();
  status = zx_vmo_set_size(vmo, len);
  EXPECT_OK(status, "vm_object_set_size");

  // get the size again
  size = 0x99999999;
  status = zx_vmo_get_size(vmo, &size);
  EXPECT_OK(status, "vm_object_get_size");
  EXPECT_EQ(len, size, "vm_object_get_size");

  // try to resize it to a ludicrous size
  status = zx_vmo_set_size(vmo, UINT64_MAX);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "vm_object_set_size too big");

  // resize it to a non aligned size
  status = zx_vmo_set_size(vmo, len + 1);
  EXPECT_OK(status, "vm_object_set_size");

  // size should be rounded up to the next page boundary
  size = 0x99999999;
  status = zx_vmo_get_size(vmo, &size);
  EXPECT_OK(status, "vm_object_get_size");
  EXPECT_EQ(fbl::round_up(len + 1u, static_cast<size_t>(zx_system_get_page_size())), size,
            "vm_object_get_size");
  len = fbl::round_up(len + 1u, static_cast<size_t>(zx_system_get_page_size()));

  // map it
  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, len, &ptr);
  EXPECT_OK(status, "vm_map");
  EXPECT_NE(ptr, 0, "vm_map");

  // attempt to map expecting an non resizable vmo.
  uintptr_t ptr2;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_REQUIRE_NON_RESIZABLE, 0, vmo,
                       0, len, &ptr2);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, status, "vm_map");

  // resize it with it mapped
  status = zx_vmo_set_size(vmo, size);
  EXPECT_OK(status, "vm_object_set_size");

  // unmap it
  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "unmap");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, GrowMappedVmo) {
  zx_status_t status;
  zx::vmo vmo;

  // allocate a VMO with an initial size of 2 pages
  size_t vmo_size = zx_system_get_page_size() * 2;
  status = zx::vmo::create(vmo_size, ZX_VMO_RESIZABLE, &vmo);
  EXPECT_OK(status, "vmo_create");

  size_t mapping_size = zx_system_get_page_size() * 8;
  uintptr_t mapping_addr;

  // create an 8 page mapping backed by this VMO
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, mapping_size,
                                      &mapping_addr);
  ASSERT_OK(status, "vmar_map");
  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(mapping_addr, mapping_size);
  });

  // grow the vmo to cover part of the mapping and a partial page
  vmo_size = zx_system_get_page_size() * 6 + zx_system_get_page_size() / 2;

  status = vmo.set_size(vmo_size);
  EXPECT_OK(status, "vmo_set_size");

  // stores to the mapped area past the initial end of the VMO should be reflected in the object
  size_t store_offset_within_vmo = zx_system_get_page_size() * 5;
  *reinterpret_cast<volatile std::byte *>(mapping_addr + store_offset_within_vmo) = std::byte{1};

  std::byte vmo_value;
  status = vmo.read(&vmo_value, store_offset_within_vmo, 1);
  EXPECT_OK(status, "vmo_read");

  // stores to memory past the size set on the VMO but within that page should also be reflected in
  // the object
  size_t store_offset_past_size = vmo_size + 16;
  *reinterpret_cast<volatile std::byte *>(mapping_addr + store_offset_past_size) = std::byte{2};
  status = vmo.read(&vmo_value, store_offset_past_size, 1);
  EXPECT_OK(status, "vmo_read");

  EXPECT_EQ(vmo_value, std::byte{2});

  // writes to the VMO past the initial end of the VMO should be reflected in the mapped area.
  size_t load_offset = zx_system_get_page_size() * 5 + 16;
  std::byte value{3};
  status = vmo.write(&value, load_offset, 1);
  EXPECT_OK(status, "vmo_read");

  EXPECT_EQ(value, *reinterpret_cast<volatile std::byte *>(mapping_addr + load_offset));
}

TEST(VmoTestCase, MappedVmoSmallerThanMapping) {
  zx_status_t status;
  zx_vaddr_t addr;
  zx::vmo vmo;

  // Allocate a VMO with a size of 1 page but create a 2-page mapping backed by this VMO.
  size_t allocated_size = zx_system_get_page_size();
  size_t reserved_size = zx_system_get_page_size();
  size_t mapped_size = allocated_size + reserved_size;
  status = zx::vmo::create(allocated_size, ZX_VMO_RESIZABLE, &vmo);
  ASSERT_OK(status, "vmo_create");
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo.get(), 0,
                       mapped_size, &addr);
  ASSERT_OK(status, "vmar_map");
  auto cleanup =
      fit::defer([&]() { EXPECT_OK(zx_vmar_unmap(zx_vmar_root_self(), addr, mapped_size)); });

  // Verify that only the first page can be accessed.
  EXPECT_OK(probe_for_read(reinterpret_cast<void *>(addr)), "read before end");
  EXPECT_OK(probe_for_write(reinterpret_cast<void *>(addr)), "write before end");
  EXPECT_STATUS(probe_for_read(reinterpret_cast<void *>(addr + allocated_size)),
                ZX_ERR_OUT_OF_RANGE, "read past end");
  EXPECT_STATUS(probe_for_write(reinterpret_cast<void *>(addr + allocated_size)),
                ZX_ERR_OUT_OF_RANGE, "write past end");

  // Verify that zx_process_{read,write}_memory on the whole range complete partially.
  std::vector<uint8_t> buffer(mapped_size);
  size_t actual;
  EXPECT_OK(zx_process_read_memory(zx_process_self(), addr, buffer.data(), mapped_size, &actual),
            "read whole range");
  EXPECT_EQ(allocated_size, actual, "should have only read the allocated bytes");
  EXPECT_OK(zx_process_write_memory(zx_process_self(), addr, buffer.data(), mapped_size, &actual),
            "write whole range");
  EXPECT_EQ(allocated_size, actual, "should have only written the allocated bytes");

  // Verify that zx_process_{read,write}_memory on the reserved part fail.
  status =
      zx_process_read_memory(zx_process_self(), addr + allocated_size, buffer.data(), 1, &actual);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "read reserved range");
  status =
      zx_process_write_memory(zx_process_self(), addr + allocated_size, buffer.data(), 1, &actual);
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status, "write reserved range");
}

// Check that non-resizable VMOs cannot get resized.
TEST(VmoTestCase, NoResize) {
  const size_t len = zx_system_get_page_size() * 4;
  zx_handle_t vmo = ZX_HANDLE_INVALID;

  zx_vmo_create(len, 0, &vmo);

  EXPECT_NE(vmo, ZX_HANDLE_INVALID);

  zx_status_t status;
  status = zx_vmo_set_size(vmo, len + zx_system_get_page_size());
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "vm_object_set_size");

  status = zx_vmo_set_size(vmo, len - zx_system_get_page_size());
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, status, "vm_object_set_size");

  size_t size;
  status = zx_vmo_get_size(vmo, &size);
  EXPECT_OK(status, "vm_object_get_size");
  EXPECT_EQ(len, size, "vm_object_get_size");

  uintptr_t ptr;
  status = zx_vmar_map(zx_vmar_root_self(),
                       ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE, 0, vmo, 0,
                       len, &ptr);
  ASSERT_OK(status, "vm_map");
  ASSERT_NE(ptr, 0, "vm_map");

  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
  EXPECT_OK(status, "unmap");

  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

// Check that the RESIZE right is set on resizable VMO creation, and required for a resize.
TEST(VmoTestCase, ResizeRight) {
  zx::vmo vmo;
  const size_t len = zx_system_get_page_size() * 4;
  ASSERT_OK(zx::vmo::create(len, 0, &vmo));

  // A non-resizable VMO does not get the RESIZE right.
  zx_info_handle_basic_t info;
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);

  // Should not be possible to resize the VMO.
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, vmo.set_size(len + zx_system_get_page_size()));
  vmo.reset();

  // A resizable VMO gets the RESIZE right.
  ASSERT_OK(zx::vmo::create(len, ZX_VMO_RESIZABLE, &vmo));
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, ZX_RIGHT_RESIZE);

  // Should be possible to resize the VMO.
  EXPECT_OK(vmo.set_size(len + zx_system_get_page_size()));

  // Remove the RESIZE right. Resizing should fail.
  zx::vmo vmo_dup;
  ASSERT_OK(vmo.duplicate(info.rights & ~ZX_RIGHT_RESIZE, &vmo_dup));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmo_dup.set_size(zx_system_get_page_size()));

  vmo.reset();
  vmo_dup.reset();

  // Try the same operations with a pager-backed VMO.
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(pager.create_vmo(0, port, 0, len, &vmo));

  // A non-resizable VMO does not get the RESIZE right.
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);

  // Should not be possible to resize the VMO.
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, vmo.set_size(len + zx_system_get_page_size()));
  vmo.reset();

  // A resizable VMO gets the RESIZE right.
  ASSERT_OK(pager.create_vmo(ZX_VMO_RESIZABLE, port, 0, len, &vmo));
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, ZX_RIGHT_RESIZE);

  // Should be possible to resize the VMO.
  EXPECT_OK(vmo.set_size(len + zx_system_get_page_size()));

  // Remove the RESIZE right. Resizing should fail.
  ASSERT_OK(vmo.duplicate(info.rights & ~ZX_RIGHT_RESIZE, &vmo_dup));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmo_dup.set_size(zx_system_get_page_size()));
}

// Check that the RESIZE right is set on resizable child creation, and required for a resize.
TEST(VmoTestCase, ChildResizeRight) {
  zx::vmo parent;
  const size_t len = zx_system_get_page_size() * 4;
  ASSERT_OK(zx::vmo::create(len, 0, &parent));

  uint32_t child_types[] = {ZX_VMO_CHILD_SNAPSHOT, ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE,
                            ZX_VMO_CHILD_SLICE, ZX_VMO_CHILD_SNAPSHOT_MODIFIED};

  for (auto child_type : child_types) {
    zx::vmo vmo;
    ASSERT_OK(parent.create_child(child_type, 0, len, &vmo));

    // A non-resizable VMO does not get the RESIZE right.
    zx_info_handle_basic_t info;
    ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);

    // Should not be possible to resize the VMO.
    EXPECT_EQ(ZX_ERR_UNAVAILABLE, vmo.set_size(len + zx_system_get_page_size()));
    vmo.reset();

    // Cannot create resizable slices. Skip the rest of the loop.
    if (child_type == ZX_VMO_CHILD_SLICE) {
      continue;
    }

    // A resizable VMO gets the RESIZE right.
    ASSERT_OK(parent.create_child(child_type | ZX_VMO_CHILD_RESIZABLE, 0, len, &vmo));
    ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, ZX_RIGHT_RESIZE);

    // Should be possible to resize the VMO.
    EXPECT_OK(vmo.set_size(len + zx_system_get_page_size()));

    // Remove the RESIZE right. Resizing should fail.
    zx::vmo vmo_dup;
    ASSERT_OK(vmo.duplicate(info.rights & ~ZX_RIGHT_RESIZE, &vmo_dup));
    EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmo_dup.set_size(zx_system_get_page_size()));
  }
}

TEST(VmoTestCase, Info) {
  size_t len = zx_system_get_page_size() * 4;
  zx::vmo vmo;
  zx_info_vmo_t info;
  zx_status_t status;

  // Create a non-resizeable VMO, query the INFO on it
  // and dump it.
  status = zx::vmo::create(len, 0, &vmo);
  EXPECT_OK(status, "vm_info_test: vmo_create");

  status = vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr);
  EXPECT_OK(status, "vm_info_test: info_vmo");

  vmo.reset();

  EXPECT_EQ(info.size_bytes, len, "vm_info_test: info_vmo.size_bytes");
  EXPECT_EQ(info.flags, ZX_INFO_VMO_TYPE_PAGED | ZX_INFO_VMO_VIA_HANDLE,
            "vm_info_test: info_vmo.flags");
  EXPECT_EQ(info.cache_policy, ZX_CACHE_POLICY_CACHED, "vm_info_test: info_vmo.cache_policy");

  // Create a resizeable uncached VMO, query the INFO on it and dump it.
  len = zx_system_get_page_size() * 8;
  zx::vmo::create(len, ZX_VMO_RESIZABLE, &vmo);
  EXPECT_OK(status, "vm_info_test: vmo_create");
  vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED);

  size_t actual, avail;
  status = vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), &actual, &avail);
  EXPECT_OK(status, "vm_info_test: info_vmo");
  EXPECT_EQ(actual, 1);
  EXPECT_EQ(avail, 1);

  zx_info_handle_basic_t basic_info;
  status = vmo.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), &actual, &avail);
  EXPECT_OK(status, "vm_info_test: info_handle_basic");
  EXPECT_EQ(actual, 1);
  EXPECT_EQ(avail, 1);

  vmo.reset();

  EXPECT_EQ(info.size_bytes, len, "vm_info_test: info_vmo.size_bytes");
  EXPECT_EQ(info.flags, ZX_INFO_VMO_TYPE_PAGED | ZX_INFO_VMO_VIA_HANDLE | ZX_INFO_VMO_RESIZABLE,
            "vm_info_test: info_vmo.flags");
  EXPECT_EQ(info.cache_policy, ZX_CACHE_POLICY_UNCACHED, "vm_info_test: info_vmo.cache_policy");
  EXPECT_EQ(info.handle_rights, basic_info.rights, "vm_info_test: info_vmo.handle_rights");

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx_iommu_desc_stub_t desc;
  EXPECT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::Info");

  len = zx_system_get_page_size() * 12;
  EXPECT_OK(zx::vmo::create_contiguous(bti, len, 0, &vmo));

  status = vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr);
  EXPECT_OK(status, "vm_info_test: info_vmo");

  EXPECT_EQ(info.size_bytes, len, "vm_info_test: info_vmo.size_bytes");
  EXPECT_EQ(info.flags, ZX_INFO_VMO_TYPE_PAGED | ZX_INFO_VMO_VIA_HANDLE | ZX_INFO_VMO_CONTIGUOUS,
            "vm_info_test: info_vmo.flags");
  EXPECT_EQ(info.cache_policy, ZX_CACHE_POLICY_CACHED, "vm_info_test: info_vmo.cache_policy");
}

// Test that zx_vmo_create_physical rejects invalid initial sizes.
TEST(VmoTestCase, PhysicalVmoInvalidInitialSize) {
  const zx::unowned_resource mmio = maybe_standalone::GetMmioResource();
  if (!mmio->is_valid()) {
    printf("Mmio resource not available, skipping\n");
    return;
  }

  zx_paddr_t mmio_base;
  size_t mmio_size;
  {
    const BootOptions *boot_options = maybe_standalone::GetBootOptions();
    if (!boot_options || !boot_options->test_ram_reserve) {
      printf("Ram reservation not available, skipping\n");
      return;
    }
    const RamReservation ram = boot_options->test_ram_reserve.to_std().value();
    // The kernel should have filled in the value by now.
    ASSERT_TRUE(ram.paddr);
    mmio_base = *ram.paddr.to_std();
    mmio_size = ram.size;
  }

  // Test that zx_vmo_create_physical rejects zero initial sizes.
  {
    zx::vmo vmo;
    EXPECT_EQ(zx::vmo::create_physical(*mmio, mmio_base, /*size*/ 0, &vmo), ZX_ERR_INVALID_ARGS);
  }

  // Test that zx_vmo_create_physical rejects unaligned initial sizes.
  {
    const size_t unaligned_len = 1 + zx_system_get_page_size();
    // The test only makes sense if mmio is large enough to support unaligned_len.
    ASSERT_GE(mmio_size, unaligned_len);
    zx::vmo vmo;
    EXPECT_EQ(zx::vmo::create_physical(*mmio, mmio_base, unaligned_len, &vmo), ZX_ERR_INVALID_ARGS);
  }
}

TEST(VmoTestCase, PhysicalVmoOobMapping) {
  const zx::unowned_resource mmio = maybe_standalone::GetMmioResource();
  if (!mmio->is_valid()) {
    printf("Mmio resource not available, skipping\n");
    return;
  }

  zx_paddr_t mmio_base;
  size_t mmio_size;
  {
    const BootOptions *boot_options = maybe_standalone::GetBootOptions();
    if (!boot_options || !boot_options->test_ram_reserve) {
      printf("Ram reservation not available, skipping\n");
      return;
    }
    const RamReservation ram = boot_options->test_ram_reserve.to_std().value();
    // The kernel should have filled in the value by now.
    ASSERT_TRUE(ram.paddr);
    mmio_base = *ram.paddr.to_std();
    mmio_size = ram.size;
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create_physical(*mmio, mmio_base, zx_system_get_page_size(), &vmo));

  // Map should reject a mapping larger than the VMO size.
  zx_vaddr_t vaddr;
  EXPECT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0, vmo, 0,
                                       zx_system_get_page_size() * 2, &vaddr),
            ZX_ERR_OUT_OF_RANGE);
}

TEST(VmoTestCase, SizeAlign) {
  for (uint64_t s = 0; s < zx_system_get_page_size() * 4; s++) {
    zx_handle_t vmo;

    // create a new object with nonstandard size
    zx_status_t status = zx_vmo_create(s, 0, &vmo);
    EXPECT_OK(status, "vm_object_create");

    // should be the size rounded up to the nearest page boundary
    uint64_t size = 0x99999999;
    status = zx_vmo_get_size(vmo, &size);
    EXPECT_OK(status, "vm_object_get_size");
    EXPECT_EQ(fbl::round_up(s, static_cast<size_t>(zx_system_get_page_size())), size,
              "vm_object_get_size");

    // close the handle
    EXPECT_OK(zx_handle_close(vmo), "handle_close");
  }
}

TEST(VmoTestCase, ResizeAlign) {
  // resize a vmo with a particular size and test that the resulting size is aligned on a page
  // boundary.
  zx_handle_t vmo;
  zx_status_t status = zx_vmo_create(0, ZX_VMO_RESIZABLE, &vmo);
  EXPECT_OK(status, "vm_object_create");

  for (uint64_t s = 0; s < zx_system_get_page_size() * 4; s++) {
    // set the size of the object
    zx_status_t status = zx_vmo_set_size(vmo, s);
    EXPECT_OK(status, "vm_object_create");

    // should be the size rounded up to the nearest page boundary
    uint64_t size = 0x99999999;
    status = zx_vmo_get_size(vmo, &size);
    EXPECT_OK(status, "vm_object_get_size");
    EXPECT_EQ(fbl::round_up(s, static_cast<size_t>(zx_system_get_page_size())), size,
              "vm_object_get_size");
  }

  // close the handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");
}

// Test that zx_vmo_create_contiguous rejects unaligned initial sizes.
TEST(VmoTestCase, ContiguousRequireAlignedSize) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::bti bti;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx_iommu_desc_stub_t desc;
  zx::iommu iommu;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::ContiguousRequireAlignedSize");

  const size_t unaligned_len = 1 + zx_system_get_page_size();
  zx::vmo vmo;
  EXPECT_EQ(zx::vmo::create_contiguous(bti, unaligned_len, 0, &vmo), ZX_ERR_INVALID_ARGS);
}

// Test that zx_vmo_create_contiguous blocks the creation of zero-sized contiguous VMOs.
TEST(VmoTestCase, ContiguousRequireNonzeroSize) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::bti bti;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx_iommu_desc_stub_t desc;
  zx::iommu iommu;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::ContiguousRequireAlignedSize");

  zx::vmo vmo;
  EXPECT_EQ(zx::vmo::create_contiguous(bti, /*size*/ 0, 0, &vmo), ZX_ERR_INVALID_ARGS);
}

TEST(VmoTestCase, ContentSize) {
  zx_status_t status;
  zx::vmo vmo;

  size_t len = zx_system_get_page_size() * 4;
  status = zx::vmo::create(len, ZX_VMO_RESIZABLE, &vmo);
  EXPECT_OK(status, "zx::vmo::create");

  uint64_t content_size = 42;
  status = vmo.get_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size));
  EXPECT_OK(status, "get_property");
  EXPECT_EQ(len, content_size);

  uint64_t target_size = len / 3;
  status = vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &target_size, sizeof(target_size));
  EXPECT_OK(status, "set_property");

  content_size = 42;
  status = vmo.get_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size));
  EXPECT_OK(status, "get_property");
  EXPECT_EQ(target_size, content_size);

  target_size = 5461;
  status = vmo.set_size(target_size);
  EXPECT_OK(status, "set_size");

  content_size = 42;
  status = vmo.get_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size));
  EXPECT_OK(status, "get_property");
  EXPECT_EQ(target_size, content_size);

  zx::vmo read_write_vmo;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_SET_PROPERTY, &read_write_vmo));
  EXPECT_OK(
      read_write_vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::vmo read_only_vmo;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ | ZX_RIGHT_SET_PROPERTY, &read_only_vmo));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, read_only_vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE,
                                                             &content_size, sizeof(content_size)));
}

// Test that zx_vmo_get_stream_size and zx_vmo_set_stream_size require the correct rights.
TEST(VmoTestCase, StreamSizeRights) {
  zx::vmo vmo_default_rights;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_default_rights));

  // If we have no rights, we should be able to read the stream size.
  {
    zx::vmo no_rights;
    ASSERT_OK(vmo_default_rights.duplicate(ZX_RIGHT_NONE, &no_rights));
    uint64_t size;
    EXPECT_OK(no_rights.get_stream_size(&size));
  }

  // If we have no rights, we should not be able to set the stream size.
  {
    zx::vmo no_rights;
    ASSERT_OK(vmo_default_rights.duplicate(ZX_RIGHT_NONE, &no_rights));
    EXPECT_STATUS(ZX_ERR_ACCESS_DENIED, no_rights.set_stream_size(zx_system_get_page_size()));
  }

  // If we have ZX_RIGHT_WRITE, we should be able to set the stream size.
  {
    zx::vmo write_only;
    ASSERT_OK(vmo_default_rights.duplicate(ZX_RIGHT_WRITE, &write_only));
    EXPECT_OK(write_only.set_stream_size(zx_system_get_page_size()));
  }
}

TEST(VmoTestCase, StreamSize) {
  zx::vmo vmo;
  const size_t create_len = zx_system_get_page_size() * 4 + 42;

  // VMO with stream size of 4 pages & 42 bytes.
  EXPECT_OK(zx::vmo::create(create_len, ZX_VMO_RESIZABLE, &vmo));

  // Stream size should be create_len & VMO size should be rounded up be a multiple of page size.
  uint64_t stream_size = 0;
  uint64_t vmo_size = 0;

  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, create_len);
  EXPECT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 5);

  // Reducing stream size doesn't affect the VMO size.
  EXPECT_OK(vmo.set_stream_size(zx_system_get_page_size() - 1));

  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() - 1);
  EXPECT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 5);

  // Can resize stream up to the limit of the VMO size.
  EXPECT_OK(vmo.set_stream_size(zx_system_get_page_size() * 5));
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 5);

  // Out of range error if stream is resized to be larger than the VMO.
  EXPECT_EQ(vmo.set_stream_size(zx_system_get_page_size() * 5 + 1), ZX_ERR_OUT_OF_RANGE);

  // zx_vmo_set_size should trim the stream size to ensure it is never larger than the VMO size.
  EXPECT_OK(vmo.set_size(zx_system_get_page_size() * 2));

  // VMO size should be rounded up a page & stream size
  EXPECT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 2);
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 2);

  // TODO(341218975@): add test that zx_vmo_set_size will not modify stream size, unless to
  // maintain invariant that stream size does not exceed VMO size.

  // zx_vmo_set_size should trim the stream size to ensure it is never larger than the VMO size.
  EXPECT_OK(vmo.set_size(zx_system_get_page_size() * 10));

  // VMO size should be rounded up a page & stream size
  EXPECT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 10);
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 10);

  // Can change stream size on non-resizable VMO (up to the VMO size).
  zx::vmo vmo_unbounded;
  EXPECT_OK(zx::vmo::create(create_len, ZX_VMO_UNBOUNDED, &vmo_unbounded));

  EXPECT_OK(vmo_unbounded.set_stream_size(create_len * 2 + 1));
  EXPECT_OK(vmo_unbounded.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, create_len * 2 + 1);
}

// Tests that if the stream size is increased, the range in between the old stream size & the new
// stream size is zeroed.
TEST(VmoTestCase, ZeroRangeOnSetStreamSize) {
  const std::span fill_data = vmo_test::TestFillPages<4, 9>();

  // 2 Page stream, unbounded VMO.
  const size_t create_len = zx_system_get_page_size() * 2;
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(create_len, ZX_VMO_UNBOUNDED, &vmo));

  // Write data to 4 pages of VMO.
  EXPECT_OK(vmo.write(fill_data.data(), 0, fill_data.size_bytes()));

  // Increase stream length.
  EXPECT_OK(vmo.set_stream_size(fill_data.size_bytes()));

  // Range between the old stream size & the new stream size should have been zeroed.
  const std::span two_pages_zero = vmo_test::TestFillPages<2, 0>();
  ASSERT_EQ(fill_data.size_bytes() - create_len, two_pages_zero.size_bytes());

  std::vector<uint8_t> vmo_buff(two_pages_zero.size_bytes());
  EXPECT_OK(vmo.read(vmo_buff.data(), create_len, vmo_buff.size()));
  EXPECT_BYTES_EQ(vmo_buff.data(), two_pages_zero.data(), vmo_buff.size());

  // Write data to all pages of VMO.
  EXPECT_OK(vmo.write(fill_data.data(), 0, fill_data.size_bytes()));

  // Decrease stream length back to create_len.
  vmo.set_stream_size(create_len);

  // Range in between the new (smaller) stream len & old (larger) stream len should have been
  // zeroed.
  EXPECT_OK(vmo.read(vmo_buff.data(), create_len, two_pages_zero.size_bytes()));
  EXPECT_BYTES_EQ(vmo_buff.data(), two_pages_zero.data(), two_pages_zero.size_bytes());

  // Nn page-aligned range.

  // 2 pages - 42 bytes stream size.
  const size_t create_len_non_aligned = zx_system_get_page_size() * 2 - 42;

  zx::vmo vmo2;
  EXPECT_OK(zx::vmo::create(create_len_non_aligned, ZX_VMO_UNBOUNDED, &vmo2));

  // Write data to 4 pages.
  EXPECT_OK(vmo2.write(fill_data.data(), 0, fill_data.size_bytes()));

  // Increase stream len to 4 pages
  EXPECT_OK(vmo2.set_stream_size(fill_data.size_bytes()));

  // Range from old create len to the end of the VMO should be zeroed.
  const std::span non_aligned_zero =
      vmo_test::TestFillPages<4, 0>().subspan(create_len_non_aligned);
  std::vector<uint8_t> vmo2_buff(non_aligned_zero.size_bytes());
  EXPECT_OK(vmo2.read(vmo2_buff.data(), create_len_non_aligned, non_aligned_zero.size_bytes()));
  EXPECT_BYTES_EQ(vmo2_buff.data(), non_aligned_zero.data(), vmo2_buff.size());

  // Range from start of VMO to old create len should retain data.
  const std::span vmo2_fill = fill_data.subspan(0, create_len_non_aligned);
  std::vector<uint8_t> vmo2_buff2(create_len_non_aligned);
  EXPECT_OK(vmo2.read(vmo2_buff2.data(), 0, create_len_non_aligned));
  EXPECT_BYTES_EQ(vmo2_buff2.data(), vmo2_fill.data(), vmo2_buff2.size());
}

// Tests that calling set_stream_size won't discard pinned pages.
TEST(VmoTestCase, SetStreamSizePinnedPages) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::UncachedContiguous");

  // 4 page VMO (& stream).
  zx::vmo vmo;
  const size_t create_len = zx_system_get_page_size() * 4;
  const size_t reduced_len = zx_system_get_page_size() * 2;
  EXPECT_OK(zx::vmo::create(create_len, ZX_VMO_RESIZABLE, &vmo));

  // Pin the the entire VMO.
  zx_paddr_t addrs[4];
  zx::pmt pmt;
  EXPECT_OK(bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo, 0, create_len, addrs, 4, &pmt));

  // Reduce stream size to 2 pages (shouldn't discard pinned pages).
  ASSERT_OK(vmo.set_stream_size(reduced_len));

  // Unpin.
  pmt.unpin();

  // Re-pin entire VMO (which includes pages past the steam size).
  EXPECT_OK(bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo, 0, create_len, addrs, 4, &pmt));

  // Increase stream size back to 4 pages (should not discard pinned pages).
  ASSERT_OK(vmo.set_stream_size(create_len));

  pmt.unpin();
}

TEST(VmoTestCase, StreamSizeSlicesAndReferences) {
  uint64_t stream_size = 0;
  uint64_t vmo_size = 0;

  // 4 page stream, unbounded VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4 * zx_system_get_page_size(), ZX_VMO_UNBOUNDED, &vmo));

  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 4);

  // 3 page slice (smaller than parent stream).
  zx::vmo smaller_slice;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SLICE, 0, 3 * zx_system_get_page_size(), &smaller_slice));

  // VMO size and stream size are 3 pages.
  EXPECT_OK(smaller_slice.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 3);
  EXPECT_OK(smaller_slice.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 3);

  // Reduce smaller-slice stream size to 2 pages.
  smaller_slice.set_stream_size(zx_system_get_page_size() * 2);

  EXPECT_OK(smaller_slice.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 3);
  EXPECT_OK(smaller_slice.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 2);

  // 5 page slice.
  zx::vmo larger_slice;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SLICE, 0, 5 * zx_system_get_page_size(), &larger_slice));

  // VMO size and stream size are 5 pages.
  EXPECT_OK(larger_slice.get_size(&vmo_size));
  EXPECT_EQ(vmo_size, zx_system_get_page_size() * 5);
  EXPECT_OK(larger_slice.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 5);

  // Parent stream size is unchanged.
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  EXPECT_EQ(stream_size, zx_system_get_page_size() * 4);

  // Reference.
  zx::vmo ref;
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Will have same stream size as parent.
  EXPECT_OK(ref.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 4);

  // Call set_stream_size on reference, increase to 8 pages.
  EXPECT_OK(ref.set_stream_size(_zx_system_get_page_size() * 8));

  // Reference & VMO have 8 page stream.
  EXPECT_OK(ref.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 8);
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 8);

  // Call set_stream_size on vmo, increase to 10 pages.
  EXPECT_OK(vmo.set_stream_size(_zx_system_get_page_size() * 10));

  // Reference & VMO have 10 page stream.
  EXPECT_OK(ref.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 10);
  EXPECT_OK(vmo.get_stream_size(&stream_size));
  ASSERT_EQ(stream_size, zx_system_get_page_size() * 10);
}

TEST(VmoTestCase, FaultBeyondStreamSizeNoPhysOrContig) {
  size_t vmo_size = _zx_system_get_page_size() * 2;
  zx_info_vmo_t info;
  uint64_t slice_size;
  uint64_t stream_size;

  // 6 page VMAR.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      zx_system_get_page_size() * 6, &vmar, &vmar_addr));

  // Can't create fault-beyond-stream-size mapping of physical VMO.
  vmo_test::PhysVmo phys_vmo;
  if (auto res = vmo_test::GetTestPhysVmo(vmo_size); !res.is_ok()) {
    if (res.error_value() == ZX_ERR_NOT_SUPPORTED) {
      printf("Root resource not available, skipping\n");
    }
    return;
  } else {
    phys_vmo = std::move(res.value());
  }

  zx_vaddr_t addr;

  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                                               ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                                           0, phys_vmo.vmo, 0, vmo_size, &addr));

  // Can't create fault-beyond-stream-size mapping of a slice child of a physical VMO.
  zx::vmo phys_slice;
  ASSERT_OK(phys_vmo.vmo.create_child(ZX_VMO_CHILD_SLICE, 0, 1, &phys_slice));

  // Type should be contiguous & slice size should be rounded up to kPageSize. Stream size should be
  // 0.
  ASSERT_OK(phys_slice.get_size(&slice_size));
  EXPECT_OK(phys_slice.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_TRUE(info.flags & ZX_INFO_VMO_CONTIGUOUS);
  ASSERT_EQ(slice_size, zx_system_get_page_size());
  phys_slice.get_stream_size(&stream_size);
  EXPECT_EQ(stream_size, 0);

  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                                               ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                                           0, phys_slice, 0, zx_system_get_page_size(), &addr));

  // Can't create fault-beyond-stream-size mapping of contiguous VMO.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::UncachedContiguous");

  zx::vmo contig_vmo;
  EXPECT_OK(zx::vmo::create_contiguous(bti, vmo_size, 0, &contig_vmo));

  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                                               ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                                           0, contig_vmo, 0, vmo_size, &addr));

  // Can't create fault-beyond-stream-size mapping of a child slice of a contiguous VMO.
  zx::vmo contig_slice;
  ASSERT_OK(contig_vmo.create_child(ZX_VMO_CHILD_SLICE, 0, 1, &contig_slice));

  // Type should be contiguous & slice size should be rounded up to kPageSize.
  ASSERT_OK(contig_slice.get_size(&slice_size));
  EXPECT_OK(contig_slice.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_TRUE(info.flags & ZX_INFO_VMO_CONTIGUOUS);
  ASSERT_EQ(slice_size, zx_system_get_page_size());
  contig_slice.get_stream_size(&stream_size);
  EXPECT_EQ(stream_size, 0);

  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC |
                                               ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS,
                                           0, contig_slice, 0, zx_system_get_page_size(), &addr));
}

void RightsTestMapHelper(zx_handle_t vmo, size_t len, uint32_t flags, bool expect_success,
                         zx_status_t fail_err_code) {
  uintptr_t ptr;

  zx_status_t r = zx_vmar_map(zx_vmar_root_self(), flags, 0, vmo, 0, len, &ptr);
  if (expect_success) {
    EXPECT_OK(r);

    r = zx_vmar_unmap(zx_vmar_root_self(), ptr, len);
    EXPECT_OK(r, "unmap");
  } else {
    EXPECT_EQ(fail_err_code, r);
  }
}

zx_rights_t GetHandleRights(zx_handle_t h) {
  zx_info_handle_basic_t info;
  zx_status_t s =
      zx_object_get_info(h, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (s != ZX_OK) {
    EXPECT_OK(s);  // Poison the test
    return 0;
  }
  return info.rights;
}

void ChildPermsTestHelper(const zx_handle_t vmo) {
  // Read out the current rights;
  const zx_rights_t parent_rights = GetHandleRights(vmo);

  // Make different kinds of children and ensure we get the correct rights.
  zx_handle_t child;
  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));
  EXPECT_EQ(GetHandleRights(child),
            (parent_rights | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY | ZX_RIGHT_WRITE) &
                ~ZX_RIGHT_EXECUTE);
  EXPECT_OK(zx_handle_close(child));

  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE, 0,
                                zx_system_get_page_size(), &child));
  EXPECT_EQ(GetHandleRights(child),
            (parent_rights | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY) & ~ZX_RIGHT_WRITE);
  EXPECT_OK(zx_handle_close(child));

  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SLICE, 0, zx_system_get_page_size(), &child));
  EXPECT_EQ(GetHandleRights(child), parent_rights | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY);
  EXPECT_OK(zx_handle_close(child));

  EXPECT_OK(zx_vmo_create_child(vmo, ZX_VMO_CHILD_SLICE | ZX_VMO_CHILD_NO_WRITE, 0,
                                zx_system_get_page_size(), &child));
  EXPECT_EQ(GetHandleRights(child),
            (parent_rights | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY) & ~ZX_RIGHT_WRITE);
  EXPECT_OK(zx_handle_close(child));
}

TEST(VmoTestCase, Rights) {
  char buf[4096];
  size_t len = zx_system_get_page_size() * 4;
  zx_status_t status;
  zx_handle_t vmo, vmo2;

  // allocate an object
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // Check that the handle has at least the expected rights.
  // This list should match the list in docs/syscalls/vmo_create.md.
  static const zx_rights_t kExpectedRights =
      ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_WAIT | ZX_RIGHT_READ | ZX_RIGHT_WRITE |
      ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY;
  EXPECT_EQ(kExpectedRights, kExpectedRights & GetHandleRights(vmo));

  // test that we can read/write it
  status = zx_vmo_read(vmo, buf, 0, 0);
  EXPECT_EQ(0, status, "vmo_read");
  status = zx_vmo_write(vmo, buf, 0, 0);
  EXPECT_EQ(0, status, "vmo_write");

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ, &vmo2);
  status = zx_vmo_read(vmo2, buf, 0, 0);
  EXPECT_EQ(0, status, "vmo_read");
  status = zx_vmo_write(vmo2, buf, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, status, "vmo_write");
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_WRITE, &vmo2);
  status = zx_vmo_read(vmo2, buf, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, status, "vmo_read");
  status = zx_vmo_write(vmo2, buf, 0, 0);
  EXPECT_EQ(0, status, "vmo_write");
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, 0, &vmo2);
  status = zx_vmo_read(vmo2, buf, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, status, "vmo_read");
  status = zx_vmo_write(vmo2, buf, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, status, "vmo_write");
  zx_handle_close(vmo2);

  // full perm test
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, true, 0));

  // try most of the permutations of mapping and clone a vmo with various rights dropped
  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_DUPLICATE, &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE, &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE, &vmo2);
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE,
                      &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, true, 0));
  zx_handle_close(vmo2);

  // test that we can get/set a property on it
  const char *set_name = "test vmo";
  status = zx_object_set_property(vmo, ZX_PROP_NAME, set_name, sizeof(set_name));
  EXPECT_OK(status, "set_property");
  char get_name[ZX_MAX_NAME_LEN];
  status = zx_object_get_property(vmo, ZX_PROP_NAME, get_name, sizeof(get_name));
  EXPECT_OK(status, "get_property");
  EXPECT_STREQ(set_name, get_name, "vmo name");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");

  // Use wrong handle with wrong permission, and expect wrong type not
  // ZX_ERR_ACCESS_DENIED
  vmo = ZX_HANDLE_INVALID;
  vmo2 = ZX_HANDLE_INVALID;
  status = zx_port_create(0, &vmo);
  EXPECT_OK(status, "zx_port_create");
  status = zx_handle_duplicate(vmo, 0, &vmo2);
  EXPECT_OK(status, "zx_handle_duplicate");
  status = zx_vmo_read(vmo2, buf, 0, 0);
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, status, "vmo_read wrong type");

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
  status = zx_handle_close(vmo2);
  EXPECT_OK(status, "handle_close");
}

// This test covers VMOs with the execute bit set using the vmo_replace_as_executable syscall.
// This call is available in the unified core-tests binary and in the bootfs environment, but not
// when running as a component in a full Fuchsia system.
TEST(VmoTestCase, RightsExec) {
  if (getenv("NO_AMBIENT_MARK_VMO_EXEC")) {
    ZXTEST_SKIP("Running without the ZX_POL_AMBIENT_MARK_VMO_EXEC policy, skipping test case.");
  }

  size_t len = zx_system_get_page_size() * 4;
  zx_status_t status;
  zx_handle_t vmo, vmo2;

  // allocate an object
  status = zx_vmo_create(len, 0, &vmo);
  EXPECT_OK(status, "vm_object_create");

  // Check that the handle has at least the expected rights.
  // This list should match the list in docs/syscalls/vmo_create.md.
  static const zx_rights_t kExpectedRights =
      ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_WAIT | ZX_RIGHT_READ | ZX_RIGHT_WRITE |
      ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY;
  EXPECT_EQ(kExpectedRights, kExpectedRights & GetHandleRights(vmo));

  status = zx_vmo_replace_as_executable(vmo, ZX_HANDLE_INVALID, &vmo);
  EXPECT_OK(status, "vmo_replace_as_executable");
  EXPECT_EQ(kExpectedRights | ZX_RIGHT_EXECUTE,
            (kExpectedRights | ZX_RIGHT_EXECUTE) & GetHandleRights(vmo));

  // full perm test
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(
      vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, true, 0));

  // try most of the permutations of mapping and clone a vmo with various rights dropped
  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_EXECUTE | ZX_RIGHT_DUPLICATE,
                      &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, false,
                          ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE,
                                              false, ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE, &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, false,
                          ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE,
                                              false, ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE, &vmo2);
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, false, ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, false,
                          ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE,
                                              false, ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE,
                      &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, false,
                          ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE,
                                              false, ZX_ERR_ACCESS_DENIED));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(vmo, ZX_RIGHT_READ | ZX_RIGHT_EXECUTE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE,
                      &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, false,
                                              ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, false,
                          ZX_ERR_ACCESS_DENIED));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, true, 0));
  zx_handle_close(vmo2);

  vmo2 = ZX_HANDLE_INVALID;
  zx_handle_duplicate(
      vmo, ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_EXECUTE | ZX_RIGHT_MAP | ZX_RIGHT_DUPLICATE,
      &vmo2);
  ASSERT_NO_FATAL_FAILURE(ChildPermsTestHelper(vmo2));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, 0, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_WRITE, false, ZX_ERR_INVALID_ARGS));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, true, 0));
  ASSERT_NO_FATAL_FAILURE(RightsTestMapHelper(
      vmo2, len, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE, true, 0));
  ASSERT_NO_FATAL_FAILURE(
      RightsTestMapHelper(vmo, len, ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, true, 0));

  // close the handles
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
  status = zx_handle_close(vmo2);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, Commit) {
  zx_handle_t vmo;
  zx_status_t status;
  uintptr_t ptr, ptr2, ptr3;

  // create a vmo
  const size_t size = 16384;

  status = zx_vmo_create(size, 0, &vmo);
  EXPECT_EQ(0, status, "vm_object_create");

  // commit a range of it
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0, size, nullptr, 0);
  EXPECT_EQ(0, status, "vm commit");

  // decommit that range
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_DECOMMIT, 0, size, nullptr, 0);
  EXPECT_EQ(0, status, "vm decommit");

  // commit a range of it
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0, size, nullptr, 0);
  EXPECT_EQ(0, status, "vm commit");

  // map it
  ptr = 0;
  status =
      zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &ptr);
  EXPECT_OK(status, "map");
  EXPECT_NE(ptr, 0, "map address");

  // second mapping with an offset
  ptr2 = 0;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo,
                       zx_system_get_page_size(), size, &ptr2);
  EXPECT_OK(status, "map2");
  EXPECT_NE(ptr2, 0, "map address2");

  // third mapping with a totally non-overlapping offset
  ptr3 = 0;
  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, size * 2,
                       size, &ptr3);
  EXPECT_OK(status, "map3");
  EXPECT_NE(ptr3, 0, "map address3");

  // write into it at offset zx_system_get_page_size(), read it back
  volatile uint32_t *u32 = (volatile uint32_t *)(ptr + zx_system_get_page_size());
  *u32 = 99;
  EXPECT_EQ(99u, (*u32), "written memory");

  // check the alias
  volatile uint32_t *u32a = (volatile uint32_t *)(ptr2);
  EXPECT_EQ(99u, (*u32a), "written memory");

  // decommit page 0
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0);
  EXPECT_EQ(0, status, "vm decommit");

  // verify that it didn't get unmapped
  EXPECT_EQ(99u, (*u32), "written memory");
  // verify that it didn't get unmapped
  EXPECT_EQ(99u, (*u32a), "written memory2");

  // decommit page 1
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_DECOMMIT, zx_system_get_page_size(),
                           zx_system_get_page_size(), nullptr, 0);
  EXPECT_EQ(0, status, "vm decommit");

  // verify that it did get unmapped
  EXPECT_EQ(0u, (*u32), "written memory");
  // verify that it did get unmapped
  EXPECT_EQ(0u, (*u32a), "written memory2");

  // unmap our vmos
  status = zx_vmar_unmap(zx_vmar_root_self(), ptr, size);
  EXPECT_OK(status, "vm_unmap");
  status = zx_vmar_unmap(zx_vmar_root_self(), ptr2, size);
  EXPECT_OK(status, "vm_unmap");
  status = zx_vmar_unmap(zx_vmar_root_self(), ptr3, size);
  EXPECT_OK(status, "vm_unmap");

  // commit invalid ranges that overflow.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, zx_system_get_page_size(),
                            UINT64_MAX - zx_system_get_page_size() + 1, nullptr, 0));

  // close the handle
  status = zx_handle_close(vmo);
  EXPECT_OK(status, "handle_close");
}

TEST(VmoTestCase, ZeroPage) {
  zx_handle_t vmo;
  zx_status_t status;
  uintptr_t ptr[3];

  // create a vmo
  const size_t size = zx_system_get_page_size() * 4;

  EXPECT_OK(zx_vmo_create(size, 0, &vmo), "vm_object_create");

  // make a few mappings of the vmo
  for (auto &p : ptr) {
    EXPECT_EQ(
        ZX_OK,
        zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &p),
        "map");
    EXPECT_NE(0, p, "map address");
  }

  volatile uint32_t *val = (volatile uint32_t *)ptr[0];
  volatile uint32_t *val2 = (volatile uint32_t *)ptr[1];
  volatile uint32_t *val3 = (volatile uint32_t *)ptr[2];

  // read fault in the first mapping
  EXPECT_EQ(0, *val, "read zero");

  // write fault the second mapping
  *val2 = 99;
  EXPECT_EQ(99, *val2, "read back 99");

  // expect the third mapping to read fault in the new page
  EXPECT_EQ(99, *val3, "read 99");

  // expect the first mapping to have gotten updated with the new mapping
  // and no longer be mapping the zero page
  EXPECT_EQ(99, *val, "read 99 from former zero page");

  // read fault in zeros on the second page
  val = (volatile uint32_t *)(ptr[0] + zx_system_get_page_size());
  EXPECT_EQ(0, *val, "read zero");

  // write to the page via a vmo_write call
  uint32_t v = 100;
  status = zx_vmo_write(vmo, &v, zx_system_get_page_size(), sizeof(v));
  EXPECT_OK(status, "writing to vmo");

  // expect it to read back the new value
  EXPECT_EQ(100, *val, "read 100 from former zero page");

  // read fault in zeros on the third page
  val = (volatile uint32_t *)(ptr[0] + zx_system_get_page_size() * 2);
  EXPECT_EQ(0, *val, "read zero");

  // commit this range of the vmo via a commit call
  status = zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, zx_system_get_page_size() * 2,
                           zx_system_get_page_size(), nullptr, 0);
  EXPECT_OK(status, "committing memory");

  // write to the third page
  status = zx_vmo_write(vmo, &v, zx_system_get_page_size() * 2, sizeof(v));
  EXPECT_OK(status, "writing to vmo");

  // expect it to read back the new value
  EXPECT_EQ(100, *val, "read 100 from former zero page");

  // unmap
  for (auto p : ptr)
    EXPECT_OK(zx_vmar_unmap(zx_vmar_root_self(), p, size), "unmap");

  // close the handle
  EXPECT_OK(zx_handle_close(vmo), "handle_close");
}

TEST(VmoTestCase, Cache) {
  zx::vmo vmo;
  const size_t size = zx_system_get_page_size();

  EXPECT_OK(zx::vmo::create(size, 0, &vmo), "creation for cache_policy");

  // clean vmo can have all valid cache policies set
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED_DEVICE));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_WRITE_COMBINING));

  // bad cache policy
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo.set_cache_policy(ZX_CACHE_POLICY_MASK + 1));

  // map the vmo, make sure policy doesn't set
  uintptr_t ptr;
  EXPECT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, size, &ptr));
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  EXPECT_OK(zx::vmar::root_self()->unmap(ptr, size));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));

  // clone the vmo, make sure policy doesn't set
  zx::vmo clone;
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone));
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  clone.reset();
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));

  // clone the vmo, try to set policy on the clone
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone));
  EXPECT_EQ(ZX_ERR_BAD_STATE, clone.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  clone.reset();

  // set the policy, make sure future clones do not go through
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone));
  clone.reset();

  // set the policy, make sure vmo read/write do not work
  char c;
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.read(&c, 0, sizeof(c)));
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.write(&c, 0, sizeof(c)));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
  EXPECT_OK(vmo.read(&c, 0, sizeof(c)));
  EXPECT_OK(vmo.write(&c, 0, sizeof(c)));

  vmo.reset();

  // Create a large sparsely populated VMO and check setting cache policy works.
  EXPECT_OK(zx::vmo::create(size << 40, 0, &vmo));
  c = 42;
  EXPECT_OK(vmo.write(&c, 1ull << 50, sizeof(c)));
  EXPECT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));
  vmo.reset();
}

TEST(VmoTestCase, PhysicalSlice) {
  vmo_test::PhysVmo phys;
  if (auto res = vmo_test::GetTestPhysVmo(); !res.is_ok()) {
    if (res.error_value() == ZX_ERR_NOT_SUPPORTED) {
      printf("Root resource not available, skipping\n");
    }
    return;
  } else {
    phys = std::move(res.value());
  }

  const size_t size = zx_system_get_page_size() * 2;
  ASSERT_GE(phys.size, size);

  // Switch to a cached policy as we are operating on real memory and do not need to be uncached.
  EXPECT_OK(phys.vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));

  zx_info_vmo_t phys_vmo_info;
  ASSERT_OK(
      phys.vmo.get_info(ZX_INFO_VMO, &phys_vmo_info, sizeof(phys_vmo_info), nullptr, nullptr));
  ASSERT_NE(phys_vmo_info.koid, 0ul);
  ASSERT_EQ(phys_vmo_info.parent_koid, 0ul);

  // Create a slice of the second page.
  zx::vmo slice_vmo;
  EXPECT_OK(phys.vmo.create_child(ZX_VMO_CHILD_SLICE, size / 2, size / 2, &slice_vmo));

  // Sliced VMO should have correct parent_koid in its VMO info struct.
  zx_info_vmo_t slice_vmo_info;
  ASSERT_OK(
      slice_vmo.get_info(ZX_INFO_VMO, &slice_vmo_info, sizeof(slice_vmo_info), nullptr, nullptr));
  ASSERT_EQ(slice_vmo_info.parent_koid, phys_vmo_info.koid);

  // Map both VMOs in so we can access them.
  uintptr_t parent_vaddr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, phys.vmo, 0, size,
                                       &parent_vaddr));
  auto unmap_parent = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(parent_vaddr, size);
  });

  uintptr_t slice_vaddr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, slice_vmo, 0,
                                       size / 2, &slice_vaddr));
  auto unmap_slice = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(slice_vaddr, size / 2);
  });

  // Just do some tests using the first byte of each page
  char *parent_private_test = (char *)parent_vaddr;
  char *parent_shared_test = (char *)(parent_vaddr + size / 2);
  char *slice_test = (char *)slice_vaddr;

  // We expect parent_shared_test and slice_test to be accessing the same physical pages, but we
  // should have gotten different mappings for them.
  EXPECT_NE(parent_shared_test, slice_test);

  *parent_private_test = 0;
  *parent_shared_test = 1;

  // This should have set the child.
  EXPECT_EQ(*slice_test, 1);

  // Write to the child now and validate parent changed correctly.
  *slice_test = 42;
  EXPECT_EQ(*parent_shared_test, 42);
  EXPECT_EQ(*parent_private_test, 0);
}

TEST(VmoTestCase, CacheOp) {
  constexpr size_t kSize = 0x8000;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0, &vmo));

  auto test_op = [&vmo](uint32_t op) {
    EXPECT_OK(vmo.op_range(op, 0, 1, nullptr, 0), "0 1");
    EXPECT_OK(vmo.op_range(op, 0, 1, nullptr, 0), "0 1");
    EXPECT_OK(vmo.op_range(op, 1, 1, nullptr, 0), "1 1");
    EXPECT_OK(vmo.op_range(op, 0, kSize, nullptr, 0), "0 size");
    EXPECT_OK(vmo.op_range(op, 1, kSize - 1, nullptr, 0), "1 size-1");
    EXPECT_OK(vmo.op_range(op, 0x5200, 1, nullptr, 0), "0x5200 1");
    EXPECT_OK(vmo.op_range(op, 0x5200, 0x800, nullptr, 0), "0x5200 0x800");
    EXPECT_OK(vmo.op_range(op, 0x5200, 0x1000, nullptr, 0), "0x5200 0x1000");
    EXPECT_OK(vmo.op_range(op, 0x5200, 0x1200, nullptr, 0), "0x5200 0x1200");

    EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo.op_range(op, 0, 0, nullptr, 0), "0 0");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, 1, kSize, nullptr, 0), "0 size");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, kSize, 1, nullptr, 0), "size 1");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, kSize + 1, 1, nullptr, 0), "size+1 1");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, UINT64_MAX - 1, 1, nullptr, 0),
              "UINT64_MAX-1 1");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, UINT64_MAX, 1, nullptr, 0), "UINT64_MAX 1");
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, UINT64_MAX, UINT64_MAX, nullptr, 0),
              "UINT64_MAX UINT64_MAX");
  };

  test_op(ZX_VMO_OP_CACHE_SYNC);
  test_op(ZX_VMO_OP_CACHE_CLEAN);
  test_op(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE);
  test_op(ZX_VMO_OP_CACHE_INVALIDATE);
}

// Physical VMOs can only be created from non RAM memory and the kernel may not retain mappings to
// such memory and so cannot perform cache operations for us.
TEST(VmoTestCase, CacheOpPhys) {
  vmo_test::PhysVmo phys;
  if (auto res = vmo_test::GetTestPhysVmo(); res.is_ok()) {
    phys = std::move(res.value());
    // Go ahead and set the cache policy; we don't want the op_range calls
    // below to potentially skip running any code.
    EXPECT_OK(phys.vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED), "zx_vmo_set_cache_policy");
  } else {
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, res.status_value());
    return;
  }
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, phys.vmo.op_range(ZX_VMO_OP_CACHE_SYNC, 0, 32, nullptr, 0));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, phys.vmo.op_range(ZX_VMO_OP_CACHE_CLEAN, 0, 32, nullptr, 0));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED,
            phys.vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, 0, 32, nullptr, 0));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, phys.vmo.op_range(ZX_VMO_OP_CACHE_INVALIDATE, 0, 32, nullptr, 0));
}

TEST(VmoTestCase, CacheOpLargeVmo) {
  zx::vmo vmo;

  const uint64_t kMaxSize = UINT64_MAX - static_cast<uint64_t>(zx_system_get_page_size()) * 64 + 1;

  ASSERT_OK(zx::vmo::create(kMaxSize, 0, &vmo));

  // Cache ops should be quick on this VMO has it has no committed pages.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_CACHE_CLEAN, 0, kMaxSize, nullptr, 0));

  // Commit some pages around.
  constexpr uint64_t kCommitPages = 64;
  for (uint64_t i = 0; i < kCommitPages; i++) {
    uint64_t val = 42;
    const uint64_t offset = kMaxSize / kCommitPages * i;
    EXPECT_OK(vmo.write(&val, offset, sizeof(val)));
  }

  // With these committed pages, ops should still be fast.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_CACHE_CLEAN, 0, kMaxSize, nullptr, 0));
}

TEST(VmoTestCase, CacheOpOutOfRange) {
  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));
  // Commit the second page
  EXPECT_OK(vmo.write("A", zx_system_get_page_size(), 1));
  // Generate a bad cache op that exceeds the range of the vmo.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE,
                                              zx_system_get_page_size(), UINT64_MAX, nullptr, 0));
  // Generate a cache op close to integer wrap around
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE,
                         UINT64_MAX - zx_system_get_page_size() + 1, 1, nullptr, 0));
}

TEST(VmoTestCase, CacheFlush) {
  zx::vmo vmo;
  const size_t size = 0x8000;

  EXPECT_OK(zx::vmo::create(size, 0, &vmo), "creation for cache op");

  uintptr_t ptr_ro;
  EXPECT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, size, &ptr_ro), "map");
  EXPECT_NE(ptr_ro, 0, "map address");
  void *pro = (void *)ptr_ro;

  uintptr_t ptr_rw;
  EXPECT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &ptr_rw),
      "map");
  EXPECT_NE(ptr_rw, 0, "map address");
  void *prw = (void *)ptr_rw;

  EXPECT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, size, nullptr, 0));

  // Check some invalid combinations.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_cache_flush(pro, size, 0), "no args");
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_cache_flush(pro, size, ZX_CACHE_FLUSH_INVALIDATE),
            "invalidate requires data");
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            zx_cache_flush(pro, size, ZX_CACHE_FLUSH_INSN | ZX_CACHE_FLUSH_INVALIDATE),
            "invalidate requires data");
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_cache_flush(pro, size, 1u << 3), "out of range a");
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_cache_flush(pro, size, ~0u), "out of range b");

  // Check all the valid the combinations.
  //
  // TODO(https://fxbug.dev/42075218): zx_cache_flush is not yet fully implemented on riscv64 and
  // does not yet support ZX_CACHE_FLUSH_INSN
#if defined(__riscv)
  constexpr zx_status_t expected = ZX_ERR_NOT_SUPPORTED;
#else
  constexpr zx_status_t expected = ZX_OK;
#endif
  EXPECT_EQ(expected, zx_cache_flush(prw, size, ZX_CACHE_FLUSH_INSN), "rw flush insn");
  EXPECT_EQ(ZX_OK, zx_cache_flush(prw, size, ZX_CACHE_FLUSH_DATA), "rw clean");
  EXPECT_EQ(expected, zx_cache_flush(prw, size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INSN),
            "rw clean w/ insn");
  EXPECT_EQ(ZX_OK, zx_cache_flush(prw, size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE),
            "rw clean/invalidate");
  EXPECT_EQ(expected,
            zx_cache_flush(prw, size,
                           ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE | ZX_CACHE_FLUSH_INSN),
            "rw all");

  // riscv64 does not permit clean operations against RO mappings
#if !defined(__riscv)
  EXPECT_EQ(expected, zx_cache_flush(pro, size, ZX_CACHE_FLUSH_INSN), "ro flush insn");
  EXPECT_EQ(ZX_OK, zx_cache_flush(pro, size, ZX_CACHE_FLUSH_DATA), "ro clean");
  EXPECT_EQ(expected, zx_cache_flush(pro, size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INSN),
            "ro clean w/ insn");
  EXPECT_EQ(ZX_OK, zx_cache_flush(pro, size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE),
            "ro clean/invalidate");
  EXPECT_EQ(expected,
            zx_cache_flush(pro, size,
                           ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE | ZX_CACHE_FLUSH_INSN),
            "ro all");
#endif

  EXPECT_OK(zx::vmar::root_self()->unmap(ptr_rw, size));
  EXPECT_OK(zx::vmar::root_self()->unmap(ptr_ro, size));
}

TEST(VmoTestCase, DecommitMisaligned) {
  zx_handle_t vmo;
  EXPECT_OK(zx_vmo_create(zx_system_get_page_size() * 2, 0, &vmo), "creation for decommit test");

  // Forbid unaligned decommit, even if there's nothing committed.
  zx_status_t status = zx_vmo_op_range(vmo, ZX_VMO_OP_DECOMMIT, 0x10, 0x100, NULL, 0);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status, "decommitting uncommitted memory");

  status = zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0x10, 0x100, NULL, 0);
  EXPECT_OK(status, "committing memory");

  status = zx_vmo_op_range(vmo, ZX_VMO_OP_DECOMMIT, 0x10, 0x100, NULL, 0);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status, "decommitting memory");

  EXPECT_OK(zx_handle_close(vmo), "close handle");
}

// Resizing a regular mapped VMO causes a fault.
TEST(VmoTestCase, ResizeHazard) {
  const size_t size = zx_system_get_page_size() * 2;
  zx_handle_t vmo;
  ASSERT_OK(zx_vmo_create(size, ZX_VMO_RESIZABLE, &vmo));

  uintptr_t ptr_rw;
  EXPECT_OK(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                        &ptr_rw),
            "map");

  auto int_arr = reinterpret_cast<int *>(ptr_rw);
  EXPECT_EQ(int_arr[1], 0);

  EXPECT_OK(zx_vmo_set_size(vmo, 0u));

  EXPECT_STATUS(probe_for_read(&int_arr[1]), ZX_ERR_OUT_OF_RANGE, "read probe");
  EXPECT_STATUS(probe_for_write(&int_arr[1]), ZX_ERR_OUT_OF_RANGE, "write probe");

  EXPECT_OK(zx_handle_close(vmo));
  EXPECT_OK(zx_vmar_unmap(zx_vmar_root_self(), ptr_rw, size), "unmap");
}

TEST(VmoTestCase, CompressedContiguous) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::CompressedContiguous");

  zx_info_bti_t bti_info;
  EXPECT_OK(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr));

  constexpr uint32_t kMaxAddrs = 2;
  // If the minimum contiguity is too high this won't be an effective test, but
  // the code should still work.
  uint64_t size = std::min(128ul * 1024 * 1024, bti_info.minimum_contiguity * kMaxAddrs);

  zx::vmo contig_vmo;
  EXPECT_OK(zx::vmo::create_contiguous(bti, size, 0, &contig_vmo));

  zx_paddr_t paddrs[kMaxAddrs];

  uint64_t num_addrs =
      fbl::round_up(size, bti_info.minimum_contiguity) / bti_info.minimum_contiguity;

  zx::pmt pmt;
  EXPECT_OK(
      bti.pin(ZX_BTI_COMPRESS | ZX_BTI_PERM_READ, contig_vmo, 0, size, paddrs, num_addrs, &pmt));
  pmt.unpin();

  if (num_addrs > 1) {
    zx::pmt pmt2;
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              bti.pin(ZX_BTI_COMPRESS | ZX_BTI_PERM_READ, contig_vmo, 0, size, paddrs, 1, &pmt2));
  }
}

TEST(VmoTestCase, UncachedContiguous) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::UncachedContiguous");

  const uint64_t kSize = zx_system_get_page_size() * 4;

  zx::vmo contig_vmo;
  EXPECT_OK(zx::vmo::create_contiguous(bti, kSize, 0, &contig_vmo));

  // Attempt to make the vmo uncached.
  EXPECT_OK(contig_vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));

  // Validate that it really is uncached by making sure operations that should fail, do.
  uint64_t data = 42;
  EXPECT_EQ(contig_vmo.write(&data, 0, sizeof(data)), ZX_ERR_BAD_STATE);

  // Pin part of the vmo and validate we cannot change the cache policy whilst pinned.
  zx_paddr_t paddr;

  zx::pmt pmt;
  EXPECT_OK(bti.pin(ZX_BTI_COMPRESS | ZX_BTI_PERM_READ, contig_vmo, 0, zx_system_get_page_size(),
                    &paddr, 1, &pmt));

  EXPECT_EQ(contig_vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED), ZX_ERR_BAD_STATE);

  // Unpin and then validate that we can now change the cache policy.
  pmt.unpin();
  EXPECT_OK(contig_vmo.set_cache_policy(ZX_CACHE_POLICY_CACHED));
}

// Test various pinning operations.  In particular, we would like to test
//
// ++ Pinning of normal VMOs, contiguous VMOs, and RAM backed physical VMOs.
// ++ Pinning of child-slices of VMOs.
// ++ Attempting to overpin regions section of VMOs, particularly overpin
//    operations which do not fit in a target child-slice, but _would_ fit within
//    the main parent VMO.  See bug 53547 for details.
TEST(VmoTestCase, PinTests) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  constexpr size_t kTestPages = 6;

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::PinTests");

  enum class VmoFlavor { Normal, Contig, Physical };
  constexpr std::array FLAVORS = {VmoFlavor::Normal, VmoFlavor::Contig, VmoFlavor::Physical};

  for (auto flavor : FLAVORS) {
    struct Level {
      Level(size_t offset_pages, size_t size_pages)
          : offset(offset_pages * zx_system_get_page_size()),
            size(size_pages * zx_system_get_page_size()) {}

      zx::vmo vmo;
      const size_t offset;
      const size_t size;
      size_t size_past_end = 0;
    };

    std::array levels = {
        Level{0, kTestPages},
        Level{1, kTestPages - 2},
        Level{1, kTestPages - 4},
    };

    // Create the root level of the child-slice hierarchy based on the flavor we
    // are currently testing.
    switch (flavor) {
      case VmoFlavor::Normal:
        ASSERT_OK(zx::vmo::create(levels[0].size, 0, &levels[0].vmo));
        break;

      case VmoFlavor::Contig:
        ASSERT_OK(zx::vmo::create_contiguous(bti, levels[0].size, 0, &levels[0].vmo));
        break;

      case VmoFlavor::Physical:
        if (auto res = vmo_test::GetTestPhysVmo(levels[0].size); !res.is_ok()) {
          ASSERT_OK(res.error_value());
        } else {
          levels[0].vmo = std::move(res->vmo);
          ASSERT_EQ(levels[0].size, res->size);
        }
        break;
    }

    // Now that we have our root, create each of our child slice levels.  Each
    // level will be offset by level[i].offset bytes into level[i - 1].vmo.
    for (size_t i = 1; i < levels.size(); ++i) {
      auto &child = levels[i];
      auto &parent = levels[i - 1];
      ASSERT_OK(parent.vmo.create_child(ZX_VMO_CHILD_SLICE, child.offset, child.size, &child.vmo));

      // Compute the amount of space past this end of this child slice which
      // still exists in the root VMO.
      ASSERT_LE(child.size + child.offset, parent.size);
      child.size_past_end = parent.size_past_end + (parent.size - (child.size + child.offset));
    }

    // OK, now we should be ready to test each of the levels.
    for (const auto &level : levels) {
      // Make sure that we test ranges which have starting and ending points
      // entirely inside of the VMO, in the region after the VMO but inside the
      // root VMO, and entirely outside of even the root VMO.
      size_t root_end = level.size + level.size_past_end;
      for (size_t start = 0; start <= root_end; start += zx_system_get_page_size()) {
        for (size_t end = start + zx_system_get_page_size();
             end <= (root_end + zx_system_get_page_size()); end += zx_system_get_page_size()) {
          zx::pmt pmt;
          uint64_t paddrs[kTestPages];
          size_t size = end - start;
          size_t expected_addrs = std::min(size / zx_system_get_page_size(), std::size(paddrs));

          zx_status_t expected_status =
              ((start >= level.size) || (end > level.size)) ? ZX_ERR_OUT_OF_RANGE : ZX_OK;
          EXPECT_STATUS(
              bti.pin(ZX_BTI_PERM_READ, level.vmo, start, size, paddrs, expected_addrs, &pmt),
              expected_status,
              "Op was pin offset 0x%zx size 0x%zx in VMO (offset 0x%zx size 0x%zx spe 0x%zx)",
              start, size, level.offset, level.size, level.size_past_end);

          if (pmt.is_valid()) {
            pmt.unpin();
            pmt.reset();
          }
        }
      }
    }
  }
}

TEST(VmoTestCase, DecommitChildSliceTests) {
  constexpr size_t kTestPages = 6;

  struct Level {
    Level(size_t offset_pages, size_t size_pages)
        : offset(offset_pages * zx_system_get_page_size()),
          size(size_pages * zx_system_get_page_size()) {}

    zx::vmo vmo;
    const size_t offset;
    const size_t size;
    size_t size_past_end = 0;
  };

  std::array levels = {
      Level{0, kTestPages},
      Level{1, kTestPages - 2},
      Level{1, kTestPages - 4},
  };

  // Create the root level of the child-slice hierarchy.
  ASSERT_OK(zx::vmo::create(levels[0].size, 0, &levels[0].vmo));

  // Now that we have our root, create each of our child slice levels.  Each
  // level will be offset by level[i].offset bytes into level[i - 1].vmo.
  for (size_t i = 1; i < levels.size(); ++i) {
    auto &child = levels[i];
    auto &parent = levels[i - 1];
    ASSERT_OK(parent.vmo.create_child(ZX_VMO_CHILD_SLICE, child.offset, child.size, &child.vmo));

    // Compute the amount of space past this end of this child slice which
    // still exists in the root VMO.
    ASSERT_LE(child.size + child.offset, parent.size);
    child.size_past_end = parent.size_past_end + (parent.size - (child.size + child.offset));
  }

  // OK, now we should be ready to test each of the levels.
  for (const auto &level : levels) {
    // Make sure that we test ranges which have starting and ending points
    // entirely inside of the VMO, in the region after the VMO but inside the
    // root VMO, and entirely outside of even the root VMO.
    const size_t root_end = level.size + level.size_past_end;
    bool exercised_out_of_range = false;
    bool exercised_ok = false;
    for (size_t start = 0; start <= (root_end + zx_system_get_page_size());
         start += zx_system_get_page_size()) {
      for (size_t end = start + zx_system_get_page_size();
           end <= (root_end + (2 * zx_system_get_page_size())); end += zx_system_get_page_size()) {
        ASSERT_LT(start, end);
        const size_t size = end - start;

        // Attempt to completely commit the root before the decommit operation.
        EXPECT_OK(levels[0].vmo.op_range(ZX_VMO_OP_COMMIT, 0, levels[0].size, nullptr, 0));

        // Now attempt to decommit our test range and check to make sure that we
        // get the expected result.  We expect failure if our [start, start + size) is out
        // of range for our child VMO, not the parent.
        zx_status_t expected_status;

        if (start + size > level.size) {
          expected_status = ZX_ERR_OUT_OF_RANGE;
          exercised_out_of_range = true;
        } else {
          expected_status = ZX_OK;
          exercised_ok = true;
        }

        EXPECT_STATUS(
            level.vmo.op_range(ZX_VMO_OP_DECOMMIT, start, size, nullptr, 0), expected_status,
            "\nDecommit op was offset 0x%zx size 0x%zx in VMO (offset 0x%zx size 0x%zx spe 0x%zx)",
            start, size, level.offset, level.size, level.size_past_end);
      }
    }

    // For every level that we test, make sure we have at least one test vector
    // which expects ZX_ERR_OUT_OF_RANGE and at least one which expects ZX_OK.
    EXPECT_TRUE(exercised_out_of_range);
    EXPECT_TRUE(exercised_ok);
  }
}

TEST(VmoTestCase, MetadataBytes) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Until we use the VMO we expect metadata to be zero
  zx_info_vmo_t info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));

  EXPECT_EQ(info.metadata_bytes, 0);

  // This is a paged VMO so once we do something to commit a page we expect non-zero metadata.
  EXPECT_OK(vmo.write(&info, 0, sizeof(info)));
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));

  EXPECT_NE(info.metadata_bytes, 0);
}

TEST(VmoTestCase, V1Info) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Check that the old info can be queried and makes sense
  zx_info_vmo_v1_t v1info;
  zx_info_vmo_t info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO_V1, &v1info, sizeof(v1info), nullptr, nullptr));
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));

  // Check a subset of the fields that we expect to be stable and non-racy between the two different
  // get_info invocations.
  EXPECT_EQ(v1info.koid, info.koid);
  EXPECT_EQ(v1info.size_bytes, info.size_bytes);
  EXPECT_EQ(v1info.parent_koid, info.parent_koid);
  EXPECT_EQ(v1info.num_children, info.num_children);
  EXPECT_EQ(v1info.num_mappings, info.num_mappings);
  EXPECT_EQ(v1info.share_count, info.share_count);
  EXPECT_EQ(v1info.flags, info.flags);
  EXPECT_EQ(v1info.handle_rights, info.handle_rights);
  EXPECT_EQ(v1info.cache_policy, info.cache_policy);
}

TEST(VmoTestCase, V2Info) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Check that the old info can be queried and makes sense
  zx_info_vmo_v2_t v2info;
  zx_info_vmo_t info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO_V2, &v2info, sizeof(v2info), nullptr, nullptr));
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));

  // Check a subset of the fields that we expect to be stable and non-racy between the two different
  // get_info invocations.
  EXPECT_EQ(v2info.koid, info.koid);
  EXPECT_EQ(v2info.size_bytes, info.size_bytes);
  EXPECT_EQ(v2info.parent_koid, info.parent_koid);
  EXPECT_EQ(v2info.num_children, info.num_children);
  EXPECT_EQ(v2info.num_mappings, info.num_mappings);
  EXPECT_EQ(v2info.share_count, info.share_count);
  EXPECT_EQ(v2info.flags, info.flags);
  EXPECT_EQ(v2info.handle_rights, info.handle_rights);
  EXPECT_EQ(v2info.cache_policy, info.cache_policy);
  EXPECT_EQ(v2info.metadata_bytes, info.metadata_bytes);
  EXPECT_EQ(v2info.committed_change_events, info.committed_change_events);
}

TEST(VmoTestCase, V3Info) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Check that the old info can be queried and makes sense
  zx_info_vmo_v3_t v3info;
  zx_info_vmo_t info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO_V3, &v3info, sizeof(v3info), nullptr, nullptr));
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));

  // Check a subset of the fields that we expect to be stable and non-racy between the two different
  // get_info invocations.
  EXPECT_EQ(v3info.koid, info.koid);
  EXPECT_EQ(v3info.size_bytes, info.size_bytes);
  EXPECT_EQ(v3info.parent_koid, info.parent_koid);
  EXPECT_EQ(v3info.num_children, info.num_children);
  EXPECT_EQ(v3info.num_mappings, info.num_mappings);
  EXPECT_EQ(v3info.share_count, info.share_count);
  EXPECT_EQ(v3info.flags, info.flags);
  EXPECT_EQ(v3info.handle_rights, info.handle_rights);
  EXPECT_EQ(v3info.cache_policy, info.cache_policy);
  EXPECT_EQ(v3info.metadata_bytes, info.metadata_bytes);
  EXPECT_EQ(v3info.committed_change_events, info.committed_change_events);
  EXPECT_EQ(v3info.populated_bytes, info.populated_bytes);
}

TEST(VmoTestCase, Discardable) {
  // Create a discardable VMO.
  zx::vmo vmo;
  const size_t kSize = 3 * zx_system_get_page_size();
  ASSERT_OK(zx::vmo::create(kSize, ZX_VMO_DISCARDABLE, &vmo));

  // Verify that the discardable bit is set.
  zx_info_vmo_t info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_NE(0, info.flags & ZX_INFO_VMO_DISCARDABLE);

  // Lock the VMO.
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, sizeof(lock_state)));

  // Make sure we read zeros.
  std::vector<uint8_t> buf(kSize, 0xff);
  EXPECT_OK(vmo.read(buf.data(), 0, kSize));
  const std::span zero_fill = vmo_test::TestFillPages<3, 0>();
  EXPECT_BYTES_EQ(buf.data(), zero_fill.data(), kSize);

  // Write something to verify later.
  const std::span bb_fill = vmo_test::TestFillPages<3, 0xbb>();
  EXPECT_OK(vmo.write(bb_fill.data(), 0, bb_fill.size_bytes()));

  // Try mapping for READ and WRITE. Mapping without ALLOW_FAULTS should fail.
  uintptr_t ptr;
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED,
            zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo.get(), 0,
                        kSize, &ptr));

  // Try again with ALLOW_FAULTS. This should succeed.
  EXPECT_OK(zx_vmar_map(zx_vmar_root_self(),
                        ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS, 0, vmo.get(), 0,
                        kSize, &ptr));
  EXPECT_NE(0u, ptr);

  const std::span ptr_data{reinterpret_cast<uint8_t *>(ptr), kSize};
  // Verify the contents written last. Overwrite it via the mapped address.
  EXPECT_BYTES_EQ(bb_fill.data(), ptr_data.data(), bb_fill.size_bytes());
  memset(ptr_data.data(), 0xcc, ptr_data.size_bytes());

  // Verify contents again.
  const std::span cc_fill = vmo_test::TestFillPages<3, 0xcc>();
  EXPECT_OK(vmo.read(buf.data(), 0, buf.size()));
  EXPECT_BYTES_EQ(buf.data(), cc_fill.data(), cc_fill.size_bytes());

  // Unlock and unmap.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));
  EXPECT_OK(zx_vmar_unmap(zx_vmar_root_self(), ptr, kSize));

  // Cannot create clones of any type for discardable VMOs.
  zx::vmo child;
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, vmo.create_child(ZX_VMO_CHILD_SLICE, 0, kSize, &child));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kSize, &child));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, kSize, &child));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_MODIFIED, 0, kSize, &child));

  // Cannot create a discardable pager backed VMO.
  zx::pager pager;
  EXPECT_OK(zx::pager::create(0, &pager));
  zx::port port;
  EXPECT_OK(zx::port::create(0, &port));
  zx::vmo pager_vmo;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, pager.create_vmo(ZX_VMO_DISCARDABLE, port, 0, kSize, &pager_vmo));
}

TEST(VmoTestCase, LockUnlock) {
  // Lock/unlock only works with discardable VMOs.
  zx::vmo vmo;
  const size_t kSize = 3 * zx_system_get_page_size();
  ASSERT_OK(zx::vmo::create(kSize, 0, &vmo));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, kSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));

  vmo.reset();
  ASSERT_OK(zx::vmo::create(kSize, ZX_VMO_DISCARDABLE, &vmo));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, kSize, nullptr, 0));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, sizeof(lock_state)));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));

  // Lock/unlock requires read/write rights.
  zx_info_handle_basic_t info;
  size_t a1, a2;
  EXPECT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &a1, &a2));
  EXPECT_NE(0u, info.rights & ZX_RIGHT_READ);
  EXPECT_NE(0u, info.rights & ZX_RIGHT_WRITE);

  EXPECT_OK(vmo.replace(info.rights & ~ZX_RIGHT_WRITE, &vmo));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, kSize, nullptr, 0));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, sizeof(lock_state)));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));

  EXPECT_OK(vmo.replace(info.rights & ~(ZX_RIGHT_READ | ZX_RIGHT_WRITE), &vmo));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, kSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, sizeof(lock_state)));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));

  vmo.reset();
  ASSERT_OK(zx::vmo::create(kSize, ZX_VMO_DISCARDABLE, &vmo));

  // Lock/unlock work only on the entire VMO.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_LOCK, 0, zx_system_get_page_size(),
                                              &lock_state, sizeof(lock_state)));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_UNLOCK, 0, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_TRY_LOCK, zx_system_get_page_size(),
                                              kSize - zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_LOCK, zx_system_get_page_size(),
                         kSize - zx_system_get_page_size(), &lock_state, sizeof(lock_state)));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_UNLOCK, zx_system_get_page_size(),
                                              kSize - zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_TRY_LOCK, zx_system_get_page_size(),
                                              zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_LOCK, zx_system_get_page_size(), zx_system_get_page_size(),
                         &lock_state, sizeof(lock_state)));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_UNLOCK, zx_system_get_page_size(),
                                              zx_system_get_page_size(), nullptr, 0));

  // Lock requires a zx_vmo_lock_state_t arg.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, nullptr, sizeof(lock_state)));
  uint64_t tmp;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &tmp, sizeof(tmp)));

  // Verify the lock state returned on a successful lock.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_LOCK, 0, kSize, &lock_state, sizeof(lock_state)));
  EXPECT_EQ(0u, lock_state.offset);
  EXPECT_EQ(kSize, lock_state.size);
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));

  // Unlock on an unlocked VMO should fail.
  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kSize, nullptr, 0));
}

TEST(VmoTestCase, NoWriteResizable) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Show that any way of creating a child that is no write and resizable fails.
  zx::vmo child;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE,
                             0, zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_NO_WRITE |
                                 ZX_VMO_CHILD_RESIZABLE,
                             0, zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(
                ZX_VMO_CHILD_SNAPSHOT_MODIFIED | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE, 0,
                zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SLICE | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE, 0,
                             zx_system_get_page_size(), &child));
  // Prove that creating a non-resizable one works.
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE, 0,
                             zx_system_get_page_size(), &child));

  // Do it again with a resziable parent to show the resizability of the parent is irrelevant.
  child.reset();
  vmo.reset();
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE,
                             0, zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_NO_WRITE |
                                 ZX_VMO_CHILD_RESIZABLE,
                             0, zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(
                ZX_VMO_CHILD_SNAPSHOT_MODIFIED | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE, 0,
                zx_system_get_page_size(), &child));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SLICE | ZX_VMO_CHILD_NO_WRITE | ZX_VMO_CHILD_RESIZABLE, 0,
                             zx_system_get_page_size(), &child));
  EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE, 0,
                             zx_system_get_page_size(), &child));
}

TEST(VmoTestCase, OpOutOfBounds) {
  zx::vmo vmo;
  const size_t kVmoSize = zx_system_get_page_size();
  const size_t kOpSize = 2 * kVmoSize;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  uint32_t ops[] = {ZX_VMO_OP_COMMIT,
                    ZX_VMO_OP_DECOMMIT,
                    ZX_VMO_OP_ZERO,
                    ZX_VMO_OP_CACHE_SYNC,
                    ZX_VMO_OP_CACHE_INVALIDATE,
                    ZX_VMO_OP_CACHE_CLEAN,
                    ZX_VMO_OP_CACHE_CLEAN_INVALIDATE};
  for (auto op : ops) {
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(op, 0, kOpSize, nullptr, 0),
              "zx_vmo_op_range (op=%u) succeeded\n", op);
  }

  vmo.reset();

  // Use a pager-backed VMO for hinting ops.
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(pager.create_vmo(0, port, 0, kVmoSize, &vmo));

  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_DONT_NEED, 0, kOpSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_ALWAYS_NEED, 0, kOpSize, nullptr, 0));

  vmo.reset();

  // Use a discardable VMO for lock/unlock ops.
  ASSERT_OK(zx::vmo::create(kVmoSize, ZX_VMO_DISCARDABLE, &vmo));
  zx_vmo_lock_state_t lock_state = {};
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo.op_range(ZX_VMO_OP_LOCK, 0, kOpSize, &lock_state, sizeof(lock_state)));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_TRY_LOCK, 0, kOpSize, nullptr, 0));
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, vmo.op_range(ZX_VMO_OP_UNLOCK, 0, kOpSize, nullptr, 0));
}

// Regression test for a race allowing multiple mappings to a VMO with different cache policies. As
// this is a race this test does not reliably trigger the race, and even if it does it needs to be
// running on a platform where mixing uncached and cached mappings can produce a data access
// discrepancy to the user.
TEST(VmoTestCase, CachePolicyRace) {
  for (int iterations = 0; iterations < 10; iterations++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

    std::atomic<bool> running = false;

    auto policy = [&] {
      running = true;
      uint32_t cache_policy = ZX_CACHE_POLICY_UNCACHED;
      // Rapidly alternate the cache policy of the VMO with the goal to change the policy mid way
      // through the vmar_map below, causing the mapping and vmo cache policy to disagree.
      while (vmo.set_cache_policy(cache_policy) == ZX_OK) {
        cache_policy = cache_policy == ZX_CACHE_POLICY_UNCACHED ? ZX_CACHE_POLICY_CACHED
                                                                : ZX_CACHE_POLICY_UNCACHED;
      }
    };

    // Spin up the policy thread and wait until we know it's running.
    std::thread policy_thread = std::thread(policy);
    while (!running) {
      zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));
    }

    // Create the first mapping. This will get some random cache policy depending on where the other
    // thread got to.
    zx_vaddr_t ptr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         zx_system_get_page_size(), &ptr));
    // As there is a mapping, setting the policy will error so we can wait for that thread to
    // finish.
    policy_thread.join();

    // Create a second mapping. This should be the same cache policy as the first mapping, unless
    // we managed to trigger the race condition.
    zx_vaddr_t ptr2;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         zx_system_get_page_size(), &ptr2));

    // Check the final VMO cache policy. Our next test will involve reading and writing to our two
    // mappings in a way that, depending on the architecture and uncached mapping type, would not be
    // well defined if our mappings were uncached.
    // If the vmo claims it is of the cached policy then, unless we have hit the bug, both our
    // mappings should be cached, and our attempts to read and write should have well defined
    // semantics.
    zx_info_vmo_t vmo_info;
    EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &vmo_info, sizeof(vmo_info), nullptr, nullptr));
    if (vmo_info.cache_policy == ZX_CACHE_POLICY_CACHED) {
      // There's no way to query the caching policy of the mapping, since it's suppose to be the
      // same as the VMO. However if we read and write to the two pointers and get a disagreement,
      // then at least one of them us uncached, while the VMO is claiming to be cached.
      volatile uint64_t *p1 = reinterpret_cast<volatile uint64_t *>(ptr);
      volatile uint64_t *p2 = reinterpret_cast<volatile uint64_t *>(ptr2);
      for (uint64_t i = 0; i < 10; i++) {
        *p1 = i;
        ASSERT_EQ(i, *p2);
        i++;
        *p2 = i;
        ASSERT_EQ(i, *p1);
      }
    }
    ASSERT_OK(zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size()));
    ASSERT_OK(zx::vmar::root_self()->unmap(ptr2, zx_system_get_page_size()));
  }
}

TEST(VmoTestCase, VmoUnbounded) {
  zx::vmo vmo;
  zx_info_handle_basic_t info;

  // Can't create a VMO that's both unbounded and resizable.
  EXPECT_EQ(zx::vmo::create(0, ZX_VMO_UNBOUNDED | ZX_VMO_RESIZABLE, &vmo), ZX_ERR_INVALID_ARGS);

  // Make a a vmo with ZX_VMO_UNBOUNDED option.
  ASSERT_OK(zx::vmo::create(42, ZX_VMO_UNBOUNDED, &vmo));

  // Stream size should be set to size argument.
  uint64_t content_size = 0;
  EXPECT_OK(vmo.get_stream_size(&content_size));
  EXPECT_EQ(content_size, 42);

  uint64_t size = 0;
  vmo.get_size(&size);

  // Can't call set_size
  ASSERT_EQ(vmo.set_size(zx_system_get_page_size()), ZX_ERR_UNAVAILABLE);

  // Buffer to test read/writes
  const std::span zero_fill = vmo_test::TestFillPages<4, 0>();
  std::vector<char> buf(zero_fill.size_bytes());

  // Can read up to end of VMO.
  EXPECT_OK(vmo.read(buf.data(), size - buf.size(), buf.size()));

  // Can't read off end of VMO (unbounded is a lie).
  EXPECT_EQ(vmo.read(buf.data(), size, buf.size()), ZX_ERR_OUT_OF_RANGE);

  // Can write up to end of VMO
  EXPECT_OK(vmo.write(zero_fill.data(), size - zero_fill.size_bytes(), zero_fill.size_bytes()));

  // Can't write off end of VMO
  EXPECT_EQ(vmo.write(zero_fill.data(), size, zero_fill.size_bytes()), ZX_ERR_OUT_OF_RANGE);

  // Check contents
  std::vector<char> check(buf.size());
  EXPECT_OK(vmo.read(check.data(), size - check.size(), check.size()));
  EXPECT_EQ(std::strcmp(buf.data(), check.data()), 0);

  // Unbounded clones are not supported, but large clones can be made.
  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, size, &clone));

  // Clone should not have resize right.
  ASSERT_OK(clone.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);

  // Clone can read pages form parent
  EXPECT_OK(clone.read(check.data(), size - buf.size(), buf.size()));
  EXPECT_EQ(std::strcmp(buf.data(), check.data()), 0);

  ASSERT_OK(clone.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, clone.set_size(zx_system_get_page_size()));

  // Test the flag on a pager-backed VMO
  zx::vmo pvmo;
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  // Can't create a VMO that's both unbounded and resizable.
  EXPECT_EQ(pager.create_vmo(ZX_VMO_UNBOUNDED | ZX_VMO_RESIZABLE, port, 0, 0, &pvmo),
            ZX_ERR_INVALID_ARGS);

  ASSERT_OK(pager.create_vmo(ZX_VMO_UNBOUNDED, port, 0, 43, &pvmo));

  // Stream size should be set to size argument.
  EXPECT_OK(pvmo.get_stream_size(&content_size));
  EXPECT_EQ(content_size, 43);

  // Unbounded VMO does not get the RESIZE right, or be able to be resized.
  ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, vmo.set_size(buf.size() + zx_system_get_page_size()));

  // Create a clone, which should not be resizable.
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, buf.size(), &clone));

  ASSERT_OK(clone.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights & ZX_RIGHT_RESIZE, 0);
  EXPECT_EQ(ZX_ERR_UNAVAILABLE, clone.set_size(zx_system_get_page_size()));

  vmo.reset();
  clone.reset();
}

// Regression test for https://fxbug.dev/506441830
TEST(VmoTestCase, CreateVmoUnboundedInvalidStreamSize) {
  zx::vmo vmo;
  // The initial stream size of an unbounded vmo must be within the vmo size, and attempting to set
  // an overly large one shouldn't trigger a kernel panic.
  EXPECT_EQ(zx::vmo::create(UINT64_MAX, ZX_VMO_UNBOUNDED, &vmo), ZX_ERR_OUT_OF_RANGE);
}

// Helper function which will map the passed VMO & cause a read to write permission fault on the
// first two pages. Passed VMO must be at least two pages in length.
void ProtectToWriteTestHelper(zx::vmo *vmo, const size_t len) {
  ASSERT_GE(len, 2 * zx_system_get_page_size());

  // Map VMO as read/write.
  uintptr_t vaddr;
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, *vmo, 0, len, &vaddr));

  // Write so (at least) first page is faulted in.
  *reinterpret_cast<uint64_t *>(vaddr) = 0xdead1eaf;

  // Protect to remove write permissions.
  ASSERT_OK(zx::vmar::root_self()->protect(ZX_VM_PERM_READ, vaddr, len));

  EXPECT_EQ(*reinterpret_cast<uint64_t *>(vaddr), 0xdead1eaf);

  // Add back write permissions
  ASSERT_OK(zx::vmar::root_self()->protect(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, vaddr, len));

  *reinterpret_cast<uint64_t *>(vaddr) = 0xc0ffee;
  *reinterpret_cast<uint64_t *>(vaddr + zx_system_get_page_size()) = 0xc0ffee;

  EXPECT_EQ(*reinterpret_cast<uint64_t *>(vaddr), 0xc0ffee);
  EXPECT_EQ(*reinterpret_cast<uint64_t *>(vaddr + zx_system_get_page_size()), 0xc0ffee);
}

// Tests that page faults work as expected when a write fault follows a read fault of the same page.
// This will result in the case where a permission fault occurs on a page that is already mapped.
TEST(VmoTestCase, ProtectToWrite) {
  const size_t len = zx_system_get_page_size() * 2;

  // Physical VMO.
  vmo_test::PhysVmo phys;
  zx::result<vmo_test::PhysVmo> result = vmo_test::GetTestPhysVmo(len);
  if (result.is_error()) {
    printf("Root resource not available, skipping\n");
    return;
  }

  phys = std::move(result.value());

  ASSERT_NO_FATAL_FAILURE(ProtectToWriteTestHelper(&phys.vmo, len));

  // Paged VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(len, 0, &vmo));
  ASSERT_NO_FATAL_FAILURE(ProtectToWriteTestHelper(&vmo, len));
}

// Prefetch on an anonymous VMO only has an effect of decompressing pages, which we cannot trigger
// and check here in userspace, so this test just validates general syscall errors.
TEST(VmoTestCase, Prefetch) {
  auto check_vmo = [](zx::vmo &vmo, uint64_t size) {
    // simple smoke test.
    EXPECT_OK(vmo.op_range(ZX_VMO_OP_PREFETCH, 0, zx_system_get_page_size(), nullptr, 0));
    // zero lengths should work.
    EXPECT_OK(vmo.op_range(ZX_VMO_OP_PREFETCH, 0, 0, nullptr, 0));
    // unaligned test.
    EXPECT_OK(vmo.op_range(ZX_VMO_OP_PREFETCH, 4, 2, nullptr, 0));
    // unaligned spanning pages test.
    EXPECT_OK(vmo.op_range(ZX_VMO_OP_PREFETCH, zx_system_get_page_size() - 1, 2, nullptr, 0));
    // out of range should fail.
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
              vmo.op_range(ZX_VMO_OP_PREFETCH, 0, size + kPageSize, nullptr, 0));
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
              vmo.op_range(ZX_VMO_OP_PREFETCH, size + kPageSize, 0, nullptr, 0));

    // duplicate a handle without read permissions.
    zx::vmo vmo_dup;
    zx_info_handle_basic_t info;
    EXPECT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    ASSERT_OK(vmo.duplicate(info.rights & ~ZX_RIGHT_READ, &vmo_dup));
    // prefetch should fail with insufficient rights.
    EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
              vmo_dup.op_range(ZX_VMO_OP_PREFETCH, 0, zx_system_get_page_size(), nullptr, 0));
  };

  {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));
    check_vmo(vmo, zx_system_get_page_size() * 2);
  }

  // Check if physical VMOs work.
  if (auto res = vmo_test::GetTestPhysVmo(); res.is_ok()) {
    check_vmo((*res).vmo, (*res).size);
  }
}

TEST(VmoTestCase, UnmapLoanedcontiguousWhileFaulting) {
  using pager_tests::TestThread;
  using pager_tests::UserPager;
  using pager_tests::Vmo;

  if constexpr (LIMIT_CONTIGUOUS_ALLOCATIONS) {
    ZXTEST_SKIP("avoiding large contiguous allocations");
  }

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef,
                                 "VmoTestCase::UnmapLoanedcontiguousWhileFaulting");

  constexpr uint64_t kNumPages = 4096;

  // Try a bunch of times.
  for (size_t i = 0; i < 100; i++) {
    // Create the contiguous VMO for this iteration, decommitting its pages to make them loaned.
    zx::vmo contig_vmo;
    ASSERT_OK(
        zx::vmo::create_contiguous(bti, kNumPages * zx_system_get_page_size(), 0, &contig_vmo));
    zx_status_t status = contig_vmo.op_range(ZX_VMO_OP_DECOMMIT, 0,
                                             kNumPages * zx_system_get_page_size(), nullptr, 0);
    if (status == ZX_ERR_NOT_SUPPORTED) {
      // Contiguous decommit may not be supported by the current kernel cmdline, so cannot consider
      // this a hard error.
      printf("Contiguous decommit not supported, skipping\n");
      return;
    }
    ASSERT_OK(status);

    // Create a pager backed VMO and supply it full of pages to attempt to soak the newly loaned
    // pages.
    UserPager pager;
    ASSERT_TRUE(pager.Init());
    Vmo *vmo;
    ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));
    ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

    // Map in the contiguous VMO.
    zx_vaddr_t vaddr = 0;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, contig_vmo, 0,
                                         kNumPages * zx_system_get_page_size(), &vaddr));
    // Drop the contig_vmo reference, just leaving the mapping.
    contig_vmo.reset();

    // Spawn a test thread to touch the mapping. Using an atomic to ensure that the test thread has
    // started taking its faults before we unmap to increase the chance of a successful race.
    std::atomic<bool> running = false;
    TestThread t([vaddr, &running]() -> bool {
      for (size_t i = 0; i < kNumPages; i++) {
        *(volatile char *)(vaddr + i * zx_system_get_page_size()) = 42;
        running = true;
      }
      // Ensure we always crash for consistency.
      __builtin_abort();
      return false;
    });

    // Start the thread and then remove the mapping.
    ASSERT_TRUE(t.Start());
    while (!running) {
      arch::Yield();
    }
    zx::vmar::root_self()->unmap(vaddr, kNumPages * zx_system_get_page_size());

    // Wait for the test thread to complete. In correct execution it will either crash due to the
    // mapping going away, or because it calls __builtin_abort, so we just wait for any crash.
    ASSERT_TRUE(t.WaitForAnyCrash());
    // Successfully getting here indicates that the kernel didn't crash when closing the internal
    // physical page provider.
  }
}

// Test that we cannot circumvent the resize right by creating a resizable reference or slice VMO.
// Regression test for https://fxbug.dev/520618980.
TEST(VmoTestCase, CreateChildMintResize) {
  zx::vmo vmo_with_resize;
  ASSERT_OK(zx::vmo::create(2 * zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo_with_resize));

  // If the parent has ZX_RIGHT_RESIZE, we can create a resizable reference child.
  zx::vmo child_with_resize;
  EXPECT_OK(vmo_with_resize.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_RESIZABLE, 0, 0,
                                         &child_with_resize));
  EXPECT_OK(child_with_resize.set_size(zx_system_get_page_size()));

  // Without ZX_RIGHT_RESIZE on the parent handle, we cannot create the child.
  zx::vmo vmo_no_resize;
  zx_info_handle_basic_t info;
  ASSERT_OK(vmo_with_resize.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  ASSERT_OK(vmo_with_resize.duplicate(info.rights & ~ZX_RIGHT_RESIZE, &vmo_no_resize));

  EXPECT_STATUS(ZX_ERR_ACCESS_DENIED,
                vmo_no_resize.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_RESIZABLE, 0, 0,
                                           &child_with_resize));

  // The ZX_VMO_CHILD_SLICE option is always incompatible with ZX_VMO_CHILD_RESIZABLE, even when the
  // parent handle is resizable. Check here anyway so that this restriction is interpreted in the
  // same security context as the restriction on reference VMOs.
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                vmo_no_resize.create_child(ZX_VMO_CHILD_SLICE | ZX_VMO_CHILD_RESIZABLE, 0,
                                           zx_system_get_page_size(), &child_with_resize));
}

}  // namespace
