// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <random>
#include <vector>

#include <fbl/string_printf.h>
#include <perftest/perftest.h>

#include "assert.h"

// This file contains various test cases that measure the cost of copying to/from a VMO, effectively
// measuring the cost of a memcpy() for different cases. These include:
//  * Vmo/Read, Vmo/Write: cost of copying from/to a VMO with zx_vmo_read()/zx_vmo_write(). The
//    operated-on VMO is already mapped, with page table entries populated.
//    * Subcase: Vmo/Write/ZeroPage: cost of zx_vmo_write() when the memory being read from is the
//      shared zero page (as implemented by the kernel).
//  * VmoMap/Read, VmoMap/Write: cost of mapping a VMO and then copying it. The operated-on VMO
//    already has its pages committed.
//    * Subcase: "/Kernel" variants use zx_vmo_read()/zx_vmo_write() to copy to/from the VMO; other
//      variants use memcpy() in userland.
//    * Subcase: VmoMapRange: uses ZX_VM_MAP_RANGE so that the map operation pre-populates the page
//      table entries for the mappings.
//  * Vmo/Memcpy: cost of creating a VMO, mapping it, then copying it using memcpy().
//    * Subcase: "/WithPrecommit" variants use ZX_VMAR_OP_COMMIT to map and commit the VMO prior to
//      issuing a memcpy.
//    * Subcase: "/WithoutPrecommit" variants perform a memcpy into the VMO without committing it.
//  * Vmo/TransferData: cost of zx_vmo_transfer_data().

namespace {

// Measure the time taken to write or read a chunk of data to/from a VMO using the zx_vmo_write() or
// zx_vmo_read() syscalls respectively. If |do_write| and |zero_page| are true, this measures the
// time to do a zx_vmo_write() that copies from a buffer that maps to the kernel's shared zero page
// into the VMO. One reason for testing this case is that this uses a different code path in the
// kernel than if non-zero pages were used. For multi-page buffers, it will also read fewer pages of
// physical memory.
bool VmoReadOrWriteTest(perftest::RepeatState* state, uint32_t copy_size, bool do_write,
                        bool zero_page) {
  if (zero_page) {
    // This is the only meaningful combination. See comments where the test is registered below.
    ZX_ASSERT(do_write);
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));

  // Use a vmo as the buffer to read from / write to, so we can exactly control whether we're using
  // distinct physical pages or the singleton zero page.
  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &buffer_vmo));
  zx_vaddr_t buffer_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, buffer_vmo, 0,
                                       copy_size, &buffer_addr));

  // If |zero_page| is not specified, memset to non-zero to make sure buffer_vmo's pages are
  // populated and not eligible for zero page deduping, otherwise let the kernel fault in the zero
  // page as required.
  //
  // This can alter the runtime of the vmo write below. If |zero_page| is true, for vmo
  // write, the buffer is being read from, so we will just use the singleton zero page.
  //
  // Also when performing page lookups in the vmo to retrieve backing pages, the logic in the kernel
  // for handling distinct physical pages differs from the zero page.
  if (!zero_page) {
    memset(reinterpret_cast<void*>(buffer_addr), 0xa, copy_size);
  }

  // Write the VMO so that the pages are pre-committed. This matters
  // more for the read case.
  ASSERT_OK(vmo.write(reinterpret_cast<void*>(buffer_addr), 0, copy_size));

  if (do_write) {
    while (state->KeepRunning()) {
      ASSERT_OK(vmo.write(reinterpret_cast<void*>(buffer_addr), 0, copy_size));
    }
  } else {
    while (state->KeepRunning()) {
      ASSERT_OK(vmo.read(reinterpret_cast<void*>(buffer_addr), 0, copy_size));
    }
  }

  ASSERT_OK(zx::vmar::root_self()->unmap(buffer_addr, copy_size));
  return true;
}

// Measure the time taken to write or read a chunk of data to/from a mapped VMO. The writing/reading
// is either done from userland using memcpy() (when user_memcpy=true) or by the kernel using
// zx_vmo_read()/zx_vmo_write() (when user_memcpy=false).
bool VmoReadOrWriteMapTestImpl(perftest::RepeatState* state, uint32_t copy_size, bool do_write,
                               int flags, bool user_memcpy) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));
  std::vector<char> buffer(copy_size);
  zx_vaddr_t mapped_addr;

  zx::vmo vmo_buf;
  if (!user_memcpy) {
    // Create a temporary VMO that we can use to get the kernel to read/write our mapped memory.
    ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo_buf));
  }

  // Write the VMO so that the pages are pre-committed. This matters
  // more for the read case.
  ASSERT_OK(vmo.write(buffer.data(), 0, copy_size));

  if (do_write) {
    while (state->KeepRunning()) {
      ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | flags, 0, vmo, 0,
                                           copy_size, &mapped_addr));
      if (user_memcpy) {
        std::memcpy(reinterpret_cast<void*>(mapped_addr), buffer.data(), copy_size);
      } else {
        // To write to the mapped in portion we *read* from the temporary VMO.
        ASSERT_OK(vmo_buf.read(reinterpret_cast<void*>(mapped_addr), 0, copy_size));
      }
      ASSERT_OK(zx::vmar::root_self()->unmap(mapped_addr, copy_size));
    }
  } else {  // read
    while (state->KeepRunning()) {
      ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | flags, 0, vmo, 0,
                                           copy_size, &mapped_addr));
      if (user_memcpy) {
        std::memcpy(buffer.data(), reinterpret_cast<void*>(mapped_addr), copy_size);
      } else {
        // To read from the mapped in portion we *write* it to the temporary VMO.
        ASSERT_OK(vmo_buf.write(reinterpret_cast<void*>(mapped_addr), 0, copy_size));
      }
      ASSERT_OK(zx::vmar::root_self()->unmap(mapped_addr, copy_size));
    }
  }
  return true;
}

bool VmoReadOrWriteMapTest(perftest::RepeatState* state, uint32_t copy_size, bool do_write,
                           bool user_memcpy) {
  return VmoReadOrWriteMapTestImpl(state, copy_size, do_write, 0, user_memcpy);
}

bool VmoReadOrWriteMapRangeTest(perftest::RepeatState* state, uint32_t copy_size, bool do_write,
                                bool user_memcpy) {
  return VmoReadOrWriteMapTestImpl(state, copy_size, do_write, ZX_VM_MAP_RANGE, user_memcpy);
}

// Measure the time taken to create a VMO, map it into the root VMAR, optionally commit and
// map the pages, memcpy data into the VMO, then unmap and destroy the VMO. This is used as
// an indirect way to measure the overhead induced by page faulting during a memcpy.
bool VmoMemcpyPrecommitTest(perftest::RepeatState* state, uint32_t size, bool precommit) {
  state->DeclareStep("create_and_map_vmo");
  if (precommit) {
    state->DeclareStep("precommit");
  }
  state->DeclareStep("memcpy");
  state->DeclareStep("unmap_and_destroy_vmo");

  // Set up a source buffer and initialize it.
  std::unique_ptr<char[]> src(new char[size]);
  memset(src.get(), 0xff, size);

  while (state->KeepRunning()) {
    // Create and map the destination VMO.
    zx::vmo dst_vmo;
    ASSERT_OK(zx::vmo::create(size, 0, &dst_vmo));
    zx_vaddr_t dst;
    ASSERT_OK(
        zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, dst_vmo, 0, size, &dst));
    state->NextStep();

    // Commit the destination VMO if we're running the precommit case.
    if (precommit) {
      ASSERT_OK(zx::vmar::root_self()->op_range(ZX_VMAR_OP_COMMIT, dst, size, 0, 0));
      state->NextStep();
    }

    // Memcpy from source into dst.
    memcpy(reinterpret_cast<void*>(dst), src.get(), size);
    state->NextStep();

    // Unmap the destination VMO.
    ASSERT_OK(zx::vmar::root_self()->unmap(dst, size));
  }
  return true;
}

// Measure the time taken to clone a vmo and destroy it. If map_size is non zero, then this function
// tests the case where the original vmo is mapped in chunks of map_size; otherwise it tests the
// case where the original vmo is not mapped.
bool VmoCloneTest(perftest::RepeatState* state, uint32_t copy_size, uint32_t map_size) {
  if (map_size > 0) {
    state->DeclareStep("map");
  }
  state->DeclareStep("clone");
  state->DeclareStep("close");
  if (map_size > 0) {
    state->DeclareStep("unmap");
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, copy_size, nullptr, 0));

  zx::vmar vmar;
  zx_vaddr_t addr = 0;
  // Allocate a single vmar so we have a single reserved block if mapping in using multiple chunks.
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, copy_size, &vmar,
      &addr));

  while (state->KeepRunning()) {
    if (map_size > 0) {
      zx_vaddr_t chunk_addr;
      for (uint32_t off = 0; off < copy_size; off += map_size) {
        ASSERT_OK(vmar.map(ZX_VM_MAP_RANGE | ZX_VM_PERM_READ | ZX_VM_SPECIFIC, off, vmo, off,
                           map_size, &chunk_addr));
      }
      state->NextStep();
    }

    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, copy_size, &clone));
    state->NextStep();

    clone.reset();

    if (map_size > 0) {
      state->NextStep();
      ASSERT_OK(vmar.unmap(addr, copy_size));
    }
  }

  return true;
}

// Measure the time taken to create a clone, map, unmap and then destroy it.
bool VmoMapCloneTest(perftest::RepeatState* state, uint32_t copy_size) {
  state->DeclareStep("clone");
  state->DeclareStep("map");
  state->DeclareStep("unmap");
  state->DeclareStep("close");

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));
  // Fully commit the parent vmo's pages, so that the clone mapping has backing pages to map in.
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, copy_size, nullptr, 0));

  while (state->KeepRunning()) {
    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, copy_size, &clone));
    state->NextStep();

    zx_vaddr_t addr = 0;
    // ZX_VM_MAP_RANGE will fully populate the mapping.
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_MAP_RANGE | ZX_VM_PERM_READ, 0, clone, 0, copy_size,
                                         &addr));
    state->NextStep();

    ASSERT_OK(zx::vmar::root_self()->unmap(addr, copy_size));
    state->NextStep();

    clone.reset();
  }

  return true;
}

// Measure the time it takes to clone a vmo. Specifically, this measures:
//   - Clone a vmo.
//   - Read or write either the original vmo (do_target_clone=false) or the
//     clone (do_target_clone=true).
//     - For bidirectional clones, we don't expect varying do_target_clone to
//       significantly affect this performance.
//     - do_full_op controls whether we read or write the whole vmo or just
//       a subset of the pages, as the performance characteristics of a
//       partially populated clone and a fully populated clone can differ.
//   - Destroy the clone.
bool VmoCloneReadOrWriteTest(perftest::RepeatState* state, uint32_t copy_size, bool do_write,
                             bool do_target_clone, bool do_full_op) {
  const size_t kPageSize = zx_system_get_page_size();
  state->DeclareStep("clone");
  state->DeclareStep(do_write ? "write" : "read");
  state->DeclareStep("close");

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_COMMIT, 0, copy_size, nullptr, 0));

  std::vector<char> buffer(copy_size);

  while (state->KeepRunning()) {
    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, copy_size, &clone));
    state->NextStep();

    const zx::vmo& target = do_target_clone ? clone : vmo;
    if (do_full_op) {
      if (do_write) {
        ASSERT_OK(target.write(buffer.data(), 0, copy_size));
      } else {
        ASSERT_OK(target.read(buffer.data(), 0, copy_size));
      }
    } else {
      // There's no special meaning behind the particular value of this
      // constant. It just needs to result in a couple of writes into
      // the vmo without populating it too densely.
      const uint64_t kWriteInterval = 8 * kPageSize;
      for (uint64_t offset = 0; offset < copy_size; offset += kWriteInterval) {
        if (do_write) {
          ASSERT_OK(target.write(buffer.data(), offset, kPageSize));
        } else {
          ASSERT_OK(target.read(buffer.data(), offset, kPageSize));
        }
      }
    }

    state->NextStep();
    // The clone goes out of scope and is implicitly closed.
  }

  return true;
}

// Measure the times taken to create, write and then read some data from a VMO on a single thread.
// This is used to measure the performance of a brand new VMO's entire lifecycle up to data read
// completion time. This test is useful because this is essentially what users of `fuchsia.mem.Data`
// or `fuchsia.mem.Buffer` must do on top of their default zx.channel write/read operations. It's
// worth measuring these operations together (when they are also tested separately) because we
// expect them to have different performance behavior together.
//
// The zx_vmo_write() call will cause pages to be allocated in the VMO, and closing the VMO handle
// will free those pages.
bool VmoCreateWriteReadCloseTest(perftest::RepeatState* state, uint32_t copy_size) {
  state->DeclareStep("create");
  state->DeclareStep("write");
  state->DeclareStep("read");
  state->DeclareStep("close");

  // Use a vmo as the buffer to read from / write to.
  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(copy_size, 0, &buffer_vmo));
  zx_vaddr_t buffer_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, buffer_vmo, 0,
                                       copy_size, &buffer_addr));
  memset(reinterpret_cast<void*>(buffer_addr), 0xa, copy_size);

  while (state->KeepRunning()) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(copy_size, 0, &vmo));
    state->NextStep();
    ASSERT_OK(vmo.write(reinterpret_cast<void*>(buffer_addr), 0, copy_size));
    state->NextStep();
    ASSERT_OK(vmo.read(reinterpret_cast<void*>(buffer_addr), 0, copy_size));
    state->NextStep();
  }

  ASSERT_OK(zx::vmar::root_self()->unmap(buffer_addr, copy_size));
  return true;
}

// Measure the time taken to transfer data between VMOs using zx_vmo_transfer_data().
bool VmoTransferDataTest(perftest::RepeatState* state, uint64_t transfer_size) {
  const uint64_t vmo_size = transfer_size * 2;
  const uint64_t src_offset = transfer_size;
  const uint64_t dst_offset = 0;
  zx::vmo src_vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &src_vmo));
  zx_vaddr_t buffer_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, src_vmo, 0, vmo_size,
                                       &buffer_addr));
  zx::vmo dst_vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &dst_vmo));

  state->DeclareStep("fill_source");
  state->DeclareStep("transfer");
  while (state->KeepRunning()) {
    memset((void*)buffer_addr, 'f', vmo_size);
    state->NextStep();
    ASSERT_OK(dst_vmo.transfer_data(0, dst_offset, transfer_size, &src_vmo, src_offset));
  }
  ASSERT_OK(zx::vmar::root_self()->unmap(buffer_addr, vmo_size));
  return true;
}

// Measure the time taken to manipulate a small portion of a VMO that has many mappings associated
// with it.
bool VmoManyMappingsTest(perftest::RepeatState* state, uint64_t num_mappings) {
  // Create a VMO large enough that it can have |num_mappings| of non overlapping mappings, i.e. has
  // at least that many pages.
  const uint64_t vmo_size = num_mappings * zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));
  zx_vaddr_t base;
  // Populate a vmar with subvmars that then contain our mappings to ensure that the VMO is
  // completely mapped in an easy to access (i.e. contiguous) fashion, but is in unique mappings
  // that cannot be internally merged by the kernel.
  zx::vmar vmar;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, vmo_size, &vmar,
      &base));
  for (uint64_t i = 0; i < num_mappings; i++) {
    const uint64_t offset = i * zx_system_get_page_size();
    zx::vmar sub_vmar;
    zx_vaddr_t addr;
    ASSERT_OK(vmar.allocate(ZX_VM_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, offset,
                            zx_system_get_page_size(), &sub_vmar, &addr));
    ZX_ASSERT(addr == base + offset);
    ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, offset,
                           zx_system_get_page_size(), &addr));
    ZX_ASSERT(addr == base + offset);
  }

  // Prepare a random source for picking a random location in the VMO to read/write from.
  std::random_device rand_dev;
  std::uniform_int_distribution rand_page_gen(uint64_t{0}, num_mappings - 1);

  state->DeclareStep("read");
  state->DeclareStep("write");
  state->DeclareStep("decommit");

  while (state->KeepRunning()) {
    // Roll a random page to use.
    const uint64_t offset = rand_page_gen(rand_dev) * zx_system_get_page_size();
    uint64_t* target_addr = reinterpret_cast<uint64_t*>(base + offset);
    // Perform a read from the location, requiring a page table entry to be populated.
    uint64_t data;
    memcpy(&data, target_addr, sizeof(data));
    state->NextStep();
    // Perform a write to the location, invalidating any zero page mappings.
    memcpy(target_addr, &data, sizeof(data));
    state->NextStep();
    // Decommit the page allocated in the write step, requiring page table entries to be updated.
    ASSERT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, offset, zx_system_get_page_size(), nullptr, 0));
  }
  // Cleanup all the mappings, vmars and subvmars.
  ASSERT_OK(zx::vmar::root_self()->unmap(base, vmo_size));
  return true;
}

template <typename Func, typename... Args>
void RegisterVmoTest(const char* name, Func fn, Args... args) {
  for (unsigned size_in_kbytes : {4, 32, 128, 512, 2048}) {
    auto full_name = fbl::StringPrintf("%s/%ukbytes", name, size_in_kbytes);
    perftest::RegisterTest(full_name.c_str(), fn, size_in_kbytes * 1024, args...);
  }
}

void RegisterTests() {
  for (bool do_write : {false, true}) {
    for (bool zero : {false, true}) {
      // The zero case for vmo read is not meaningful since it will only operate on the zero page in
      // the first iteration; the remaining iterations will use forked pages which is equivalent to
      // the non-zero case. Skip this combo.
      if (zero && !do_write) {
        continue;
      }
      const char* rw = do_write ? "Write" : "Read";
      const char* z = zero ? "/ZeroPage" : "";
      auto rw_name = fbl::StringPrintf("Vmo/%s%s", rw, z);
      RegisterVmoTest(rw_name.c_str(), VmoReadOrWriteTest, do_write, zero);
    }
  }

  for (bool do_write : {false, true}) {
    for (bool user_memcpy : {false, true}) {
      const char* rw = do_write ? "Write" : "Read";
      const char* user_kernel = user_memcpy ? "" : "/Kernel";

      auto rw_name = fbl::StringPrintf("VmoMap/%s%s", rw, user_kernel);
      RegisterVmoTest(rw_name.c_str(), VmoReadOrWriteMapTest, do_write, user_memcpy);

      rw_name = fbl::StringPrintf("VmoMapRange/%s%s", rw, user_kernel);
      RegisterVmoTest(rw_name.c_str(), VmoReadOrWriteMapRangeTest, do_write, user_memcpy);
    }
  }

  for (bool precommit : {false, true}) {
    const char* pc = precommit ? "WithPrecommit" : "WithoutPrecommit";
    auto precommit_name = fbl::StringPrintf("Vmo/Memcpy/%s", pc);
    RegisterVmoTest(precommit_name.c_str(), VmoMemcpyPrecommitTest, precommit);
  }

  for (bool map : {false, true}) {
    auto clone_name = fbl::StringPrintf("Vmo/Clone%s", map ? "/MapParent" : "");
    RegisterVmoTest(clone_name.c_str(), [map](perftest::RepeatState* state, uint32_t size) {
      return VmoCloneTest(state, size, map ? size : 0);
    });
  }

  for (unsigned map_chunk_kb : {4, 64, 2048, 32768}) {
    constexpr uint32_t vmo_size_kb = 32768;
    auto name = fbl::StringPrintf("Vmo/Clone/MapParent%usegments/%ukbytes",
                                  vmo_size_kb / map_chunk_kb, vmo_size_kb);
    perftest::RegisterTest(name.c_str(), VmoCloneTest, vmo_size_kb * 1024, map_chunk_kb * 1024);
  }

  auto name = fbl::StringPrintf("Vmo/MapClone");
  RegisterVmoTest(name.c_str(), VmoMapCloneTest);

  for (bool do_write : {false, true}) {
    for (bool do_target_clone : {false, true}) {
      for (bool do_full_op : {false, true}) {
        const char* rw = do_write ? "Write" : "Read";
        const char* target = do_target_clone ? "Clone" : "Orig";
        const char* density = do_full_op ? "All" : "Some";
        auto clone_rw_name = fbl::StringPrintf("Vmo/Clone/%s%s%s", rw, target, density);
        RegisterVmoTest(clone_rw_name.c_str(), VmoCloneReadOrWriteTest, do_write, do_target_clone,
                        do_full_op);
      }
    }
  }

  name = fbl::StringPrintf("Vmo/CreateWriteReadClose");
  RegisterVmoTest(name.c_str(), VmoCreateWriteReadCloseTest);

  name = fbl::StringPrintf("Vmo/TransferData");
  RegisterVmoTest(name.c_str(), VmoTransferDataTest);

  perftest::RegisterTest("Vmo/ManyMappingsReadWriteDecommit/8192", VmoManyMappingsTest, 8192);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
