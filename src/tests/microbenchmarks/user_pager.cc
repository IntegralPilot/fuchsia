// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/pager.h>
#include <lib/zx/port.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>

#include <algorithm>
#include <atomic>
#include <cstdlib>
#include <cstring>
#include <latch>
#include <random>
#include <thread>
#include <vector>

#include <perftest/perftest.h>

#include "assert.h"

namespace {

// This microbenchmark measures the latency of page faults on VMOs backed by a user pager.
// It uses a `SimplePager` to supply trivial data (a pre-allocated VMO filled with '1's)
// to satisfy page requests as quickly as possible. This allows us to measure the pure
// overhead of the kernel/user context switch, message passing on the pager port, and
// the MMU mapping updates, without being bottlenecked by actual disk or network I/O.
class SimplePager {
 public:
  explicit SimplePager(size_t size) {
    ASSERT_OK(zx::pager::create(0, &pager_));
    ASSERT_OK(zx::port::create(0, &port_));
    ASSERT_OK(zx::vmo::create(size, 0, &supply_vmo_));

    uintptr_t addr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, supply_vmo_, 0,
                                         size, &addr));
    std::memset(reinterpret_cast<void*>(addr), 1, size);
    ASSERT_OK(zx::vmar::root_self()->unmap(addr, size));

    ASSERT_OK(pager_.create_vmo(0, port_, 0, size, &vmo_));

    std::latch started(1);

    thread_ = std::thread([this, &started]() {
      started.count_down();
      PagerLoop();
    });

    // We wait here for the thread_ to actually start before returning, ensuring that we don't
    // measure thread startup time in the user pager benchmark.
    started.wait();
  }

  ~SimplePager() {
    zx_port_packet_t packet = {};
    packet.type = ZX_PKT_TYPE_USER;
    packet.key = kQuitKey;
    ASSERT_OK(port_.queue(&packet));
    thread_.join();
  }

  SimplePager(const SimplePager&) = delete;
  SimplePager(SimplePager&&) = delete;
  SimplePager& operator=(const SimplePager&) = delete;
  SimplePager& operator=(SimplePager&&) = delete;

  zx::unowned_vmo vmo() const { return zx::unowned_vmo(vmo_); }

 private:
  // The port wait blocks indefinitely. We need a way to cleanly unblock and terminate
  // the pager thread when the SimplePager is destroyed at the end of each benchmark run.
  // We queue a user packet with this specific key to signal the thread to exit.
  static constexpr uint64_t kQuitKey = 1;

  void PagerLoop() {
    while (true) {
      zx_port_packet_t packet;
      ASSERT_OK(port_.wait(zx::time::infinite(), &packet));
      if (packet.type == ZX_PKT_TYPE_USER) {
        if (packet.key == kQuitKey) {
          break;
        }
        continue;
      }

      if (packet.type == ZX_PKT_TYPE_PAGE_REQUEST &&
          packet.page_request.command == ZX_PAGER_VMO_READ) {
        uint64_t req_offset = packet.page_request.offset;
        uint64_t req_length = packet.page_request.length;
        ASSERT_OK(pager_.supply_pages(vmo_, req_offset, req_length, supply_vmo_, req_offset));
      }
    }
  }

  zx::pager pager_;
  zx::port port_;
  zx::vmo supply_vmo_;
  zx::vmo vmo_;
  std::thread thread_;
};

// This test measures the time it takes to fault pages sequentially in a user-pager-backed VMO.
bool UserPagerSequentialFaultsTest(perftest::RepeatState* state) {
  const size_t kPageSize = zx_system_get_page_size();
  const size_t kNumPages = 1024;  // 4MB with 4K pages
  const size_t kTotalSize = kNumPages * kPageSize;

  state->DeclareStep("setup_ignore");
  state->DeclareStep("page_fault");
  state->DeclareStep("teardown_ignore");

  std::optional<SimplePager> pager;
  size_t vmo_offset = 0;
  uintptr_t mapping_addr = 0;

  auto cleanup = [&]() {
    if (mapping_addr != 0) {
      zx::vmar::root_self()->unmap(mapping_addr, kTotalSize);
      pager.reset();
      mapping_addr = 0;
    }
  };

  while (state->KeepRunning()) {
    if (mapping_addr == 0) {
      pager.emplace(kTotalSize);
      zx::unowned_vmo vmo = pager->vmo();

      ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, *vmo, 0, kTotalSize, &mapping_addr));
    }

    state->NextStep();
    perftest::DoNotOptimize(reinterpret_cast<uint8_t*>(mapping_addr)[vmo_offset]);
    state->NextStep();

    vmo_offset += kPageSize;
    if (vmo_offset >= kTotalSize) {
      cleanup();
      vmo_offset = 0;
    }
  }

  cleanup();
  return true;
}

void RegisterTests() {
  perftest::RegisterTest("UserPager/PageFault/Sequential", UserPagerSequentialFaultsTest);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
