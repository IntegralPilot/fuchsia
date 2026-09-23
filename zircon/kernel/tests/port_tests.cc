// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>

#include <ktl/byte.h>
#include <object/handle.h>
#include <object/port_dispatcher.h>

#include "tests.h"

#include <ktl/enforce.h>

namespace {

// Observes whether the port honours the PortAllocator::Free() completion protocol.
class SinglePortAllocator final : public PortAllocator {
 public:
  PortPacket* Alloc() override {
    DEBUG_ASSERT(allocated_ == 0);
    ++allocated_;
    return new (&storage_) PortPacket(nullptr, this);
  }
  void Free(PortPacket* port_packet) override {
    DEBUG_ASSERT(port_packet == reinterpret_cast<PortPacket*>(&storage_));
    ++freed_;
    port_packet->~PortPacket();
  }

  int allocated() const { return allocated_; }
  int freed() const { return freed_; }

 private:
  alignas(PortPacket) ktl::byte storage_[sizeof(PortPacket)];
  int allocated_ = 0;
  int freed_ = 0;
};

bool port_cancel_key_frees_ephemeral_packets() {
  BEGIN_TEST;

  KernelHandle<PortDispatcher> handle;
  zx_rights_t rights;
  ASSERT_EQ(ZX_OK, PortDispatcher::Create(0, &handle, &rights));
  fbl::RefPtr<PortDispatcher> port = handle.dispatcher();

  SinglePortAllocator allocator;
  constexpr uint64_t kKey = 0xF1;

  PortPacket* packet = allocator.Alloc();
  ASSERT_NONNULL(packet);
  packet->packet.key = kKey;
  packet->packet.type = ZX_PKT_TYPE_USER;
  ASSERT_EQ(ZX_OK, port->Queue(packet));

  // Cancelling by key must unlink the packet AND return it to its allocator.
  ASSERT_EQ(ZX_OK, port->CancelKey(kKey));

  EXPECT_EQ(1, allocator.allocated());
  EXPECT_EQ(1, allocator.freed());

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(port_tests)
UNITTEST("cancel_key frees ephemeral packets", port_cancel_key_frees_ephemeral_packets)
UNITTEST_END_TESTCASE(port_tests, "port", "PortDispatcher tests")
