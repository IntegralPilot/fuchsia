// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/hw/inout.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <atomic>
#include <thread>

#include <hwreg/bitfields.h>
#include <zxtest/zxtest.h>

namespace {

// Barrier synchronization helper for tests.
//
// Since we don't want to introduce thread scheduling and blocking on tests which
// test regression of racy interrupt bugs, this version is implemented with busy waiting.
class SpinBarrier {
 public:
  explicit SpinBarrier(size_t expected) : expected_(expected) {}

  void ArriveAndWait() {
    size_t generation = generation_.load(std::memory_order_acquire);
    size_t previous = arrived_.fetch_add(1, std::memory_order_acq_rel);
    if (previous + 1 == expected_) {
      arrived_.store(0, std::memory_order_release);
      generation_.fetch_add(1, std::memory_order_release);
    } else {
      SpinUntil([&] { return generation_.load(std::memory_order_acquire) != generation; });
    }
  }

 private:
  const size_t expected_;
  std::atomic<size_t> arrived_{0};
  std::atomic<size_t> generation_{0};

  template <typename Predicate>
  inline void SpinUntil(Predicate pred) {
    while (!pred()) {
      __builtin_ia32_pause();
    }
  }
};

// I/O abstraction for the CMOS chip
//
// The CMOS chip is accessible via two 8-bit I/O ports, at ports 0x70 and 0x71.
// CMOS has its own 7-bit address space and memory map. When interfacing with
// the CMOS chip, the port at 0x70 holds the address into the CMOS address space,
// and the port at 0x71 holds the data we want to write to or read from that
// address.
//
// The RTC hardware is mapped to the CMOS address space at addresses 0xA-0xC.
//
// Per https://wiki.osdev.org/CMOS#Non-Maskable_Interrupts,
//
// "Whenever you send a byte to IO port 0x70, the high order bit tells the
// hardware whether to disable NMIs from reaching the CPU. If the bit is on,
// NMI is disabled (until the next time you send a byte to Port 0x70)."
//
// So we always set the top bit for all bytes written to port 0x70.
class CmosIo {
 public:
  template <typename IntType>
  void Write(IntType value, uint32_t cmos_reg) {
    static_assert(std::is_same_v<uint8_t, IntType>, "CmosIo::Write only supports uint8_t");
    outp(kAddressPort, static_cast<uint8_t>(cmos_reg) | kDisableNmiBit);
    outp(kDataPort, static_cast<uint8_t>(value));
  }

  template <typename IntType>
  IntType Read(uint32_t cmos_reg) {
    static_assert(std::is_same_v<uint8_t, IntType>, "CmosIo::Read only supports uint8_t");
    outp(kAddressPort, static_cast<uint8_t>(cmos_reg) | kDisableNmiBit);
    return static_cast<IntType>(inp(kDataPort));
  }

  static zx::result<CmosIo> Create(const zx::resource& io_res) {
    zx_status_t status = zx_ioports_request(io_res.get(), kAddressPort, 2);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(CmosIo());
  }

 private:
  static constexpr uint32_t kDisableNmiBit = 0x80u;
  static constexpr uint16_t kAddressPort = 0x70u;
  static constexpr uint16_t kDataPort = 0x71u;
};

class RegisterA : public hwreg::RegisterBase<RegisterA, uint8_t> {
 public:
  DEF_FIELD(3, 0, rate_select);
  DEF_FIELD(6, 4, division_chain_select);
  DEF_BIT(7, update_in_progress);

  static auto Get() { return hwreg::RegisterAddr<RegisterA>(0x0A); }
};

class RegisterB : public hwreg::RegisterBase<RegisterB, uint8_t> {
 public:
  DEF_BIT(0, daylight_savings_enable);
  DEF_BIT(1, hour_format);
  DEF_BIT(2, data_mode);
  DEF_BIT(3, square_wave_enable);
  DEF_BIT(4, update_ended_interrupt_enable);
  DEF_BIT(5, alarm_interrupt_enable);
  DEF_BIT(6, periodic_interrupt_enable);
  DEF_BIT(7, update_cycle_inhibit);

  static auto Get() { return hwreg::RegisterAddr<RegisterB>(0x0B); }
};

class RegisterC : public hwreg::RegisterBase<RegisterC, uint8_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<RegisterC>(0x0C); }
};

template <typename Protocol>
zx::result<zx::resource> GetResource() {
  auto client = component::Connect<Protocol>();
  if (client.is_error()) {
    return client.take_error();
  }
  auto result = fidl::WireCall(*client)->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetIoportResource() {
  return GetResource<fuchsia_kernel::IoportResource>();
}

zx::result<zx::resource> GetIrqResource() { return GetResource<fuchsia_kernel::IrqResource>(); }

void EnableRtcPeriodicInterrupt(CmosIo& cmos_io, uint8_t rate_select) {
  RegisterA::Get().ReadFrom(&cmos_io).set_rate_select(rate_select).WriteTo(&cmos_io);

  RegisterB::Get().ReadFrom(&cmos_io).set_periodic_interrupt_enable(1).WriteTo(&cmos_io);

  // Clear pending
  RegisterC::Get().ReadFrom(&cmos_io);
}

void DisableRtcPeriodicInterrupt(CmosIo& cmos_io) {
  RegisterB::Get().ReadFrom(&cmos_io).set_periodic_interrupt_enable(0).WriteTo(&cmos_io);
  RegisterC::Get().ReadFrom(&cmos_io);
}

class RtcPeriodicInterruptTest : public zxtest::Test {
 public:
  void SetUp() override {
    auto irq_res_status = GetIrqResource();
    auto io_res_status = GetIoportResource();
    if (irq_res_status.is_error() || io_res_status.is_error()) {
      ZXTEST_SKIP("IRQ or IOPORT resource not available");
    }
    irq_res = std::move(irq_res_status.value());
    io_res = std::move(io_res_status.value());

    auto cmos_io_status = CmosIo::Create(io_res);
    ASSERT_OK(cmos_io_status.status_value());
    cmos_io = cmos_io_status.value();
  }

  zx::resource irq_res;
  zx::resource io_res;
  CmosIo cmos_io;
};

// This test verifies basic functionality of the RTC hardware and how the
// interrupt dispatcher interfaces with it
TEST_F(RtcPeriodicInterruptTest, Wait) {
  zx::interrupt h;
  ASSERT_OK(zx::interrupt::create(irq_res, 8u, ZX_INTERRUPT_MODE_EDGE_HIGH, &h));

  EnableRtcPeriodicInterrupt(cmos_io, 0x06u);  // 1024 Hz

  // Wait once for the interrupt to fire
  zx::time timestamp;
  EXPECT_OK(h.wait(&timestamp));

  // Clean up
  h.destroy();

  DisableRtcPeriodicInterrupt(cmos_io);
}

// This is a regression test for https://fxbug.dev/511565489
TEST_F(RtcPeriodicInterruptTest, InterruptDispatcherWaitTeardownRace) {
  // High frequency is required so that the initial waits complete quickly,
  // allowing thousands of iterations without timeout.
  EnableRtcPeriodicInterrupt(cmos_io, 0x03u);  // 8192 Hz

  std::atomic<zx_handle_t> int_handle{ZX_HANDLE_INVALID};
  std::atomic<bool> shutdown{false};

  SpinBarrier barrier_ready(3);
  SpinBarrier barrier_go(3);
  SpinBarrier barrier_done(3);

  auto run_worker = [&](auto&& body) {
    while (!shutdown.load()) {
      barrier_ready.ArriveAndWait();

      if (shutdown.load()) {
        break;
      }

      barrier_go.ArriveAndWait();

      zx_handle_t h = int_handle.load();
      body(h);

      barrier_done.ArriveAndWait();
    }
  };

  std::thread wait_t([&] {
    run_worker([](zx_handle_t h) {
      zx_time_t timestamp;
      zx_interrupt_wait(h, &timestamp);
    });
  });

  std::thread destroy_t([&] { run_worker([](zx_handle_t h) { zx_interrupt_destroy(h); }); });

  constexpr size_t kNumIters = 5000;
  for (size_t i = 0; i < kNumIters; i++) {
    zx::interrupt h;
    ASSERT_OK(zx::interrupt::create(irq_res, 8u, ZX_INTERRUPT_MODE_LEVEL_HIGH, &h));

    // Transition dispatcher to NEEDACK state so subsequent waits trigger the UnmaskInterrupt path.
    // The high-frequency periodic interrupt ensures this initial wait completes immediately.
    zx_time_t timestamp;
    ASSERT_OK(zx_interrupt_wait(h.get(), &timestamp));

    int_handle.store(h.get());

    barrier_ready.ArriveAndWait();
    barrier_go.ArriveAndWait();
    barrier_done.ArriveAndWait();

    // Clean up handle
    int_handle.store(ZX_HANDLE_INVALID);
    zx_handle_close(h.release());
    RegisterC::Get().ReadFrom(&cmos_io);

    // Reset IO-APIC back to edge-triggered mode to clear level-trigger masking for the next
    // iteration.
    // TODO(https://fxbug.dev/525560700): Fix this bug and delete the following logic which is used
    // as a quick hack
    zx::interrupt recovery_handle;
    if (zx::interrupt::create(irq_res, 8u, ZX_INTERRUPT_MODE_EDGE_HIGH, &recovery_handle) ==
        ZX_OK) {
      recovery_handle.destroy();
    }
  }

  shutdown.store(true);
  barrier_ready.ArriveAndWait();
  wait_t.join();
  destroy_t.join();
  DisableRtcPeriodicInterrupt(cmos_io);
}

// This is a regression test for https://fxbug.dev/525560700
TEST_F(RtcPeriodicInterruptTest, PCHandleDestroyInterruptRace) {
  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  constexpr size_t kNumIters = 5000;
  for (size_t i = 0; i < kNumIters; i++) {
    zx::interrupt h;
    ASSERT_OK(zx::interrupt::create(irq_res, 8u, ZX_INTERRUPT_MODE_LEVEL_HIGH, &h));
    EnableRtcPeriodicInterrupt(cmos_io, 0x02u);  // 16384 Hz

    // Bind to port. This will immediately trigger the interrupt and queue a packet,
    // transitioning the dispatcher to NEEDACK state.
    ASSERT_OK(h.bind(port, 0, 0));

    // Wait for the packet
    zx_port_packet_t packet;
    ASSERT_OK(port.wait(zx::time::infinite(), &packet));

    // Now ack and destroy
    h.ack();
    h.destroy();

    RegisterC::Get().ReadFrom(&cmos_io);
    DisableRtcPeriodicInterrupt(cmos_io);
  }
}

}  // namespace
