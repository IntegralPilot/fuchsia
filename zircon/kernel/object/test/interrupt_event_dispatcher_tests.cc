// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>

#include <platform/timer.h>
#if ARCH_X86
#include <arch/x86/apic.h>
#endif
#include <lib/unittest/unittest.h>
#include <platform.h>
#include <zircon/syscalls-next.h>

#include <kernel/idle_power_thread.h>
#include <ktl/atomic.h>
#include <object/interrupt_dispatcher.h>
#include <object/interrupt_event_dispatcher.h>
#include <object/port_dispatcher.h>

#include <ktl/enforce.h>

namespace {

// Tests that if an irq handler fires at the same time as an interrupt dispatcher is destroyed
// the system does not deadlock.
static bool TestConcurrentIntEventDispatcherTeardown() {
  BEGIN_TEST;

  // Generating the interrupt events for this test is necessarily arch specific and is only
  // implemented for x86 here.
#if ARCH_X86
  KernelHandle<InterruptDispatcher> interrupt;
  zx_rights_t rights;

  uint32_t gsi;
  constexpr uint32_t gsi_search_max = 24;
  for (gsi = 0; gsi < gsi_search_max; gsi++) {
    zx_status_t status =
        InterruptEventDispatcher::Create(&interrupt, &rights, gsi, ZX_INTERRUPT_MODE_DEFAULT);
    if (status == ZX_OK) {
      break;
    }
  }
  ASSERT_NE(gsi, gsi_search_max, "Failed to find free global interrupt");

  // Find the local vector, put it in the low byte of our shared state.
  ktl::atomic<uint16_t> state = apic_io_fetch_irq_vector(gsi);

  // Spin up a thread to generate the interrupt. As IPIs cannot be masked this causes the
  // associated InterruptDispatcher handler to constantly get invoked, which is what we want.
  Thread* int_thread = Thread::Create(
      "int",
      [](void* arg) -> int {
        ktl::atomic<uint16_t>* state = static_cast<ktl::atomic<uint16_t>*>(arg);
        uint8_t vec = state->load(ktl::memory_order_seq_cst) & 0xffu;
        // Loop until anything is set in the high byte of the state.
        while ((state->load(ktl::memory_order_seq_cst) & 0xff00) == 0) {
          apic_send_self_ipi(vec, DELIVERY_MODE_FIXED);
          Thread::Current::Yield();
        }
        return -1;
      },
      &state, DEFAULT_PRIORITY);
  int_thread->Resume();

  // Remove the interrupt and if we don't deadlock and keep executing then all is well.
  interrupt.reset();

  // Signal the thread by setting bits in the high byte.
  state.fetch_or(0xff00, ktl::memory_order_seq_cst);
  // Shutdown the test.
  zx_status_t status = int_thread->Join(nullptr, current_mono_time() + ZX_SEC(5));
  EXPECT_EQ(status, ZX_OK);
#endif

  END_TEST;
}

bool TestPendingWakeEventBlocksSuspend() {
  BEGIN_TEST;

  // Generating the interrupt events for this test is necessarily arch specific and is only
  // implemented for x86 here.
#if ARCH_X86
  KernelHandle<InterruptDispatcher> interrupt;
  zx_rights_t rights;

  uint32_t gsi;
  constexpr uint32_t gsi_search_max = 24;
  for (gsi = 0; gsi < gsi_search_max; gsi++) {
    zx_status_t status =
        InterruptEventDispatcher::Create(&interrupt, &rights, gsi, ZX_INTERRUPT_WAKE_VECTOR,
                                         /*allow_ack_without_port_for_test=*/true);
    if (status == ZX_OK) {
      break;
    }
  }
  ASSERT_NE(gsi, gsi_search_max, "Failed to find free global interrupt");

  // Move to the boot CPU to guarantee there are no races between handling the interrupt and calling
  // suspend.
  auto restore_affinity = fit::defer(
      [previous_affinity = Thread::Current::Get()->SetCpuAffinity(cpu_num_to_mask(BOOT_CPU_ID))] {
        Thread::Current::Get()->SetCpuAffinity(previous_affinity);
      });
  ASSERT_EQ(arch_curr_cpu_num(), static_cast<cpu_num_t>(BOOT_CPU_ID));

  ASSERT_EQ(0u, IdlePowerThread::pending_wake_events());

  // Make sure suspend entry works before testing suspend abort due to pending wake events.
  zx_instant_boot_t resume_at = current_boot_time() + ZX_SEC(5);
  IdlePowerThread::TransitionAllActiveToSuspend(resume_at);

  // Pend a wake event by issuing the IPI.
  const uint8_t vector = apic_io_fetch_irq_vector(gsi);
  apic_send_self_ipi(vector, DELIVERY_MODE_FIXED);
  ASSERT_EQ(1u, IdlePowerThread::pending_wake_events());

  // Suspend entry should immediately bounce out with pending wake events.
  resume_at = current_boot_time() + ZX_SEC(5);
  IdlePowerThread::TransitionAllActiveToSuspend(resume_at);
  ASSERT_EQ(1u, IdlePowerThread::pending_wake_events());

  // Wait for the interrupt to advance the state machine from TRIGGERED to NEEDACK.
  zx_instant_boot_t timestamp = 0;
  zx_status_t status = interrupt.dispatcher()->WaitForInterrupt(&timestamp);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_EQ(1u, IdlePowerThread::pending_wake_events());

  // Acknowledge the wake event to allow suspend again.
  status = interrupt.dispatcher()->Ack();
  ASSERT_EQ(ZX_OK, status);
  ASSERT_EQ(0u, IdlePowerThread::pending_wake_events());

  // Suspend entry should succeed again.
  resume_at = current_boot_time() + ZX_SEC(5);
  IdlePowerThread::TransitionAllActiveToSuspend(resume_at);

  // Check that a pending wake event is acknowledged if the interrupt event dispatcher is destroyed
  // without an explicit acknowledgment.
  apic_send_self_ipi(vector, DELIVERY_MODE_FIXED);
  ASSERT_EQ(1u, IdlePowerThread::pending_wake_events());

  // Suspend entry should immediately bounce out with pending wake events.
  resume_at = current_boot_time() + ZX_SEC(5);
  IdlePowerThread::TransitionAllActiveToSuspend(resume_at);

  interrupt.reset();
  ASSERT_EQ(0u, IdlePowerThread::pending_wake_events());

  // Suspend entry should succeed.
  resume_at = current_boot_time() + ZX_SEC(5);
  IdlePowerThread::TransitionAllActiveToSuspend(resume_at);
#endif

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(interrupt_event_dispatcher_tests)
UNITTEST("ConcurrentIntEventDispatcherTeardown", TestConcurrentIntEventDispatcherTeardown)
UNITTEST("PendingWakeEventBlocksSuspend", TestPendingWakeEventBlocksSuspend)
UNITTEST_END_TESTCASE(interrupt_event_dispatcher_tests, "interrupt_event_dispatcher_tests",
                      "InterruptEventDispatcher tests")
