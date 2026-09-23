// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/zx/clock.h>
#include <lib/zx/guest.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <lib/zx/vcpu.h>
#include <lib/zx/vmar.h>
#include <zircon/errors.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/object.h>

#include <cstddef>
#include <thread>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

#include "fixture.h"

namespace {

constexpr zx::time_boot kSignaledTimeStamp1(12345);
constexpr zx::time_boot kSignaledTimeStamp2(67890);
constexpr uint32_t kKey = 789;

// Use an alias so we use a different test case name.
using InterruptTest = ResourceFixture;

// This is not really a function, but an entry point for a thread that has
// a tiny stack and no other setup. It's not really entered with the C ABI
// as such.  Rather, it's entered with the first argument register set to
// zx_handle_t and with the SP at the very top of the allocated stack.
// It's defined in pure assembly so that there are no issues with
// compiler-generated code's assumptions about the proper ABI setup,
// instrumentation, etc.
extern "C" void ThreadEntry(zx_handle_t handle);
// if (zx_interrupt_wait(handle, nullptr) == ZX_OK) {
//   zx_thread_exit();
// }
// ASSERT(false);
__asm__(
    ".pushsection .text.ThreadEntry,\"ax\",%progbits\n"
    ".balign 4\n"
    ".type ThreadEntry,%function\n"
    "ThreadEntry:\n"
#ifdef __aarch64__
    "  mov x1, xzr\n"           // Load nullptr into argument register.
    "  bl zx_interrupt_wait\n"  // Call.
    "  cbz w0, exit\n"          // Exit if returned ZX_OK.
    "  brk #0\n"                // Else crash.
    "exit:\n"
    "  bl zx_thread_exit\n"
    "  brk #0\n"  // Crash if we didn't exit.
#elif defined(__riscv)
    "  mv a1, zero\n"             // nullptr in second argument register.
    "  call zx_interrupt_wait\n"  // Call.
    "  beqz a0, 0f\n"             // Exit if returned ZX_OK.
    "  unimp\n"                   // Else crash.
    "0:\n"
    "  call zx_thread_exit\n"
    "  unimp\n"  // Crash if we didn't exit.
#elif defined(__x86_64__)
    "  xor %edx, %edx\n"          // Load nullptr into argument register.
    "  call zx_interrupt_wait\n"  // Call.
    "  testl %eax, %eax\n"        // If returned ZX_OK...
    "  jz exit\n"                 // ...exit.
    "  ud2\n"                     // Else crash.
    "exit:\n"
    "  call zx_thread_exit\n"
    "  ud2\n"  // Crash if we didn't exit.
#else
#error "what machine?"
#endif
    ".size ThreadEntry, . - ThreadEntry\n"
    ".popsection");

// Test the rules for creating a virtual interrupt.
//
// 1) Creating a basic virtual interrupt succeeds regardless of what resource
//    object is passed.
// 2) Creating a wake-vector virtual interrupt succeeds _only_ if the IRQ
//    resource is provided, and fails otherwise.
//
TEST_F(InterruptTest, VirtualNotWakeable) {
  const zx::resource invalid_handle{};
  zx::interrupt interrupt;

  constexpr uint32_t kBasicOpts = ZX_INTERRUPT_VIRTUAL;
  ASSERT_OK(zx::interrupt::create(invalid_handle, 0, kBasicOpts, &interrupt));
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, kBasicOpts, &interrupt));

  constexpr uint32_t kWakeVectorOpts = ZX_INTERRUPT_VIRTUAL | ZX_INTERRUPT_WAKE_VECTOR;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE,
            zx::interrupt::create(invalid_handle, 0, kWakeVectorOpts, &interrupt));
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, kWakeVectorOpts, &interrupt));
}

// Tests to bind interrupt to a non-bindable port
TEST_F(InterruptTest, NonBindablePort) {
  zx::interrupt interrupt;
  zx::port port;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  // Incorrectly pass 0 for options instead of ZX_PORT_BIND_TO_INTERRUPT
  ASSERT_OK(zx::port::create(0, &port));

  ASSERT_EQ(interrupt.bind(port, kKey, 0), ZX_ERR_WRONG_TYPE);
}

// Tests that an interrupt that is in the TRIGGERED state will send the IRQ out on a port
// if it is bound to that port.
TEST_F(InterruptTest, BindTriggeredIrqToPort) {
  zx::interrupt interrupt;
  zx::port port;
  zx_port_packet_t out;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  // Trigger the IRQ.
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));

  // Bind to a Port.
  ASSERT_OK(interrupt.bind(port, kKey, 0));

  // See if the packet is delivered.
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());
}

// Tests Interrupts bound to a port
TEST_F(InterruptTest, BindPort) {
  zx::interrupt interrupt;
  zx::port port;
  zx_port_packet_t out;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  // Test port binding
  ASSERT_OK(interrupt.bind(port, kKey, 0));
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());

  // Triggering 2nd time, ACKing it causes port packet to be delivered
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_OK(interrupt.ack());
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());
  ASSERT_EQ(out.key, kKey);
  ASSERT_EQ(out.type, ZX_PKT_TYPE_INTERRUPT);
  ASSERT_OK(out.status);
  ASSERT_OK(interrupt.ack());

  // Triggering it twice
  // the 2nd timestamp is recorded and upon ACK another packet is queued
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp2));
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());
  ASSERT_OK(interrupt.ack());
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp2.get());

  // Try to destroy now, expecting to return error telling packet
  // has been read but the interrupt has not been re-armed
  ASSERT_EQ(interrupt.destroy(), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(interrupt.ack(), ZX_ERR_CANCELED);
  ASSERT_EQ(interrupt.trigger(0, kSignaledTimeStamp1), ZX_ERR_CANCELED);
}

TEST_F(InterruptTest, PortUnbindPurgesQueuedPacket) {
  zx::interrupt interrupt;
  zx::port port;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  ASSERT_OK(interrupt.bind(port, kKey, 0));

  // Trigger the interrupt once to queue a packet into the port.
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));

  // Triggering a second time before reading should be suppressed since a packet is already queued.
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp2));

  // Destroying the interrupt while its packet is still queued in the port must purge the packet.
  ASSERT_OK(interrupt.destroy());

  // Verify that the port packet was removed and cannot be dequeued.
  zx_port_packet_t out = {};
  EXPECT_EQ(port.wait(zx::deadline_after(zx::msec(10)), &out), ZX_ERR_TIMED_OUT);
}

// Tests that triggering an interrupt multiple times while bound to a port does not result in
// duplicate packet delivery or unexpected queue states.
TEST_F(InterruptTest, DuplicateTriggerIgnoredWhileQueued) {
  zx::interrupt interrupt;
  zx::port port;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  ASSERT_OK(interrupt.bind(port, kKey, 0));

  // Trigger the interrupt once: queues the packet with the first timestamp.
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));

  // Trigger again before reading the port packet.
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp2));

  // Attempting to acknowledge the interrupt while its packet is still pending on the port
  // fails with ZX_ERR_BAD_STATE.
  ASSERT_STATUS(interrupt.ack(), ZX_ERR_BAD_STATE);

  // Verify that exactly one packet is dequeued and it carries the first timestamp.
  zx_port_packet_t out = {};
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  EXPECT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());

  // Verify no duplicate packet remains in the port queue.
  EXPECT_EQ(port.wait(zx::deadline_after(zx::msec(10)), &out), ZX_ERR_TIMED_OUT);

  // Once the port packet has been dequeued, acknowledging succeeds and re-arms the interrupt.
  ASSERT_OK(interrupt.ack());
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp2));
  ASSERT_OK(port.wait(zx::time::infinite_past(), &out));
  EXPECT_EQ(out.interrupt.timestamp, kSignaledTimeStamp2.get());
}

// Tests Interrupt Unbind
TEST_F(InterruptTest, UnBindPort) {
  zx::interrupt interrupt;
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  // Test port binding
  ASSERT_OK(interrupt.bind(port, kKey, ZX_INTERRUPT_BIND));
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  zx_port_packet_t out;
  ASSERT_OK(port.wait(zx::time::infinite(), &out));
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());

  // Ubind port, and test the unbind-trigger-port_wait sequence. The interrupt packet
  // should not be delivered from port_wait, since the trigger happened after the Unbind.
  // not receive the interrupt packet. But test some invalid use cases of unbind first.
  ASSERT_STATUS(interrupt.bind(port, 0, 2), ZX_ERR_INVALID_ARGS);
  zx::port port2;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port2));
  ASSERT_STATUS(interrupt.bind(port2, 0, ZX_INTERRUPT_UNBIND), ZX_ERR_NOT_FOUND);
  ASSERT_OK(interrupt.bind(port, 0, ZX_INTERRUPT_UNBIND));
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_STATUS(port.wait(zx::deadline_after(zx::msec(10)), &out), ZX_ERR_TIMED_OUT);

  // Bind again, and test the trigger-unbind-port_wait sequence. Interrupt packet should
  // be removed from the port at unbind, so there should be no interrupt packets to read here.
  ASSERT_OK(interrupt.bind(port, kKey, ZX_INTERRUPT_BIND));
  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_OK(interrupt.bind(port, 0, ZX_INTERRUPT_UNBIND));
  ASSERT_STATUS(port.wait(zx::deadline_after(zx::msec(10)), &out), ZX_ERR_TIMED_OUT);
  ASSERT_EQ(out.interrupt.timestamp, kSignaledTimeStamp1.get());

  // Finally test the case of an UNBIND after the interrupt dispatcher object has been
  // destroyed.
  ASSERT_OK(interrupt.bind(port, kKey, ZX_INTERRUPT_BIND));
  // destroy the interrupt and try unbind. For the destroy, we expect ZX_ERR_CANCELED,
  // since the packet has been read but the interrupt hasn't been re-armed.
  ASSERT_STATUS(interrupt.destroy(), ZX_ERR_NOT_FOUND);
  ASSERT_STATUS(interrupt.bind(port, 0, ZX_INTERRUPT_UNBIND), ZX_ERR_CANCELED);
}

// Tests support for virtual interrupts
TEST_F(InterruptTest, VirtualInterrupts) {
  zx::interrupt interrupt;
  zx::interrupt interrupt_cancelled;
  zx::time_boot timestamp;

  ASSERT_EQ(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_SLOT_USER, &interrupt),
            ZX_ERR_INVALID_ARGS);
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt_cancelled));

  ASSERT_OK(interrupt_cancelled.destroy());
  ASSERT_EQ(interrupt_cancelled.trigger(0, kSignaledTimeStamp1), ZX_ERR_CANCELED);

  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));

  ASSERT_EQ(interrupt_cancelled.wait(&timestamp), ZX_ERR_CANCELED);
  ASSERT_OK(interrupt.wait(&timestamp));
  ASSERT_EQ(timestamp.get(), kSignaledTimeStamp1.get());

  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));
  ASSERT_OK(interrupt.wait(&timestamp));
}

bool WaitThread(const zx::thread& thread, uint32_t reason) {
  while (true) {
    zx_info_thread_t info;
    EXPECT_OK(thread.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));
    if (info.state == reason) {
      return true;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  return true;
}

// Tests interrupt thread after suspend/resume
TEST_F(InterruptTest, WaitThreadFunctionsAfterSuspendResume) {
  zx::interrupt interrupt;
  zx::thread thread;
  constexpr char name[] = "interrupt_test_thread";

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));

  // Create and start a thread which waits for an IRQ
  ASSERT_OK(zx::thread::create(*zx::process::self(), name, sizeof(name), 0, &thread));

  const uint64_t pc = reinterpret_cast<uintptr_t>(&ThreadEntry);
  alignas(elfldltl::AbiTraits<>::kStackAlignment<>) static std::byte thread_stack[1024];
  const uint64_t sp = reinterpret_cast<uintptr_t>(&thread_stack[sizeof(thread_stack)]);
  ASSERT_OK(thread.start(pc, sp, interrupt.get()));

  // Wait till the thread is in blocked state
  ASSERT_TRUE(WaitThread(thread, ZX_THREAD_STATE_BLOCKED_INTERRUPT));

  // Suspend the thread, wait till it is suspended
  zx::suspend_token suspend_token;
  ASSERT_OK(thread.suspend(&suspend_token));
  ASSERT_TRUE(WaitThread(thread, ZX_THREAD_STATE_SUSPENDED));

  // Resume the thread, wait till it is back to being in blocked state
  suspend_token.reset();
  ASSERT_TRUE(WaitThread(thread, ZX_THREAD_STATE_BLOCKED_INTERRUPT));

  // Signal the interrupt and wait for the thread to exit.
  interrupt.trigger(0, zx::time_boot());
  zx_signals_t observed;
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), &observed));
}

// Tests support for null output timestamp
// NOTE: Absent the changes to interrupt.h also submitted in this CL, this test invokes undefined
//       behavior not detectable at runtime.
TEST_F(InterruptTest, NullOutputTimestamp) {
  zx::interrupt interrupt;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));

  ASSERT_OK(interrupt.trigger(0, kSignaledTimeStamp1));

  ASSERT_OK(interrupt.wait(nullptr));
}

// Tests that user signals work on interrupt objects.
TEST_F(InterruptTest, UserSignals) {
  zx::interrupt interrupt;

  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));

  ASSERT_OK(interrupt.signal(0, ZX_USER_SIGNAL_0));

  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(ZX_USER_SIGNAL_0, pending & ZX_USER_SIGNAL_ALL);

  ASSERT_OK(interrupt.signal(ZX_USER_SIGNAL_0, 0));

  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(0, pending & ZX_USER_SIGNAL_ALL);
}

// Make sure that the ZX_VIRTUAL_INTERRUPT_UNTRIGGERED signal behaves as expected for a virtual
// interrupt when used with zx_interrupt_wait.
TEST_F(InterruptTest, UntriggeredSignalInterruptWait) {
  zx::interrupt interrupt;

  // Create a new interrupt object.
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));

  // Fetch the initial signal state.  We expect the
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED to be asserted.
  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Now trigger the interrupt, the signal should become de-asserted.
  zx::time_boot expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Waiting on the interrupt should immediately unblock, but it will not
  // ack/re-arm the interrupt. This will not happen until the next time we wait
  // on the interrupt after having been previously woken.
  zx::time_boot actual_trigger_time;
  EXPECT_OK(interrupt.wait(&actual_trigger_time));
  EXPECT_EQ(expected_trigger_time, actual_trigger_time);
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Finally, spin a thread to wait a second time and ack the interrupt.
  // Unfortunately, there is no mechanism (aside from either triggering or
  // destroying the interrupt) to time-out a thread in the middle of a
  // zx_interrupt_wait operations, so we have to just break down and use a
  // second thread.
  std::thread waiter_thread([&]() { interrupt.wait(&actual_trigger_time); });

  // Go ahead and actually wait until the UNTRIGGERED signal is asserted.  We
  // cannot simply check to see if it has been set yet as our worker thread may
  // not have managed to block yet.
  EXPECT_EQ(interrupt.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite(), &pending),
            ZX_OK);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Trigger the interrupt, then join the thread so we can verify the trigger
  // time.
  expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  waiter_thread.join();
  EXPECT_EQ(expected_trigger_time, actual_trigger_time);

  // One last check to make sure that UNTRIGGERED is now de-asserted, and we are
  // finished.
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);
}

// Make sure that the ZX_VIRTUAL_INTERRUPT_UNTRIGGERED signal behaves as expected for a virtual
// interrupt when used with ports and zx_interrupt_ack.
TEST_F(InterruptTest, UntriggeredSignalPortsAndAck) {
  zx::interrupt interrupt;
  zx::port port;

  // Create a new interrupt object and a new port, then bind the two together.
  constexpr uint64_t kInterruptPortKey = 0xDeadBeefBaadF00d;
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, 0));

  // Fetch the initial signal state.  We expect the
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED to be asserted.
  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Trigger the interrupt, then verify that the UNTRIGGERED signal is de-asserted.
  const zx::time_boot expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Reading the port packet which was queued also does not re-assert the signal.
  auto VerifyInterruptPacket = [kInterruptPortKey](const zx_port_packet& packet,
                                                   const zx::time_boot& ett) {
    EXPECT_EQ(kInterruptPortKey, packet.key);
    ASSERT_EQ(ZX_PKT_TYPE_INTERRUPT, packet.type);
    EXPECT_EQ(ett.get(), packet.interrupt.timestamp);
  };
  zx_port_packet_t out;
  ASSERT_OK(port.wait(zx::time::infinite_past(), &out));
  VerifyInterruptPacket(out, expected_trigger_time);
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Ack'ing the interrupt is what should finally re-assert the UNTRIGGERED signal
  EXPECT_OK(interrupt.ack());
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Now, double trigger the interrupt.  A packet will be delivered and the UNTRIGGERED signal
  // cleared immediately after the first trigger, then a second packet will end up pending (but not
  // delivered) after the second trigger.
  const zx::time_boot ett1 = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, ett1));
  const zx::time_boot ett2 = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, ett2));

  ASSERT_OK(port.wait(zx::time::infinite_past(), &out));
  VerifyInterruptPacket(out, ett1);
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Attempting to read a second port packet should fail.  No new packet is
  // going to be queued until we ack.
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &out), ZX_ERR_TIMED_OUT);

  // Here is where things get a bit tricky.  Ack'ing the first packet should
  // result in a second packet being delivered immediately along with the second
  // timestamp.  The final UNTRIGGERED signal status should be de-asserted; but
  // what happens if there was a user already waiting on the signal when the ack
  // happened?
  //
  // In theory, the signal should "strobe" and the thread should be released.
  // To make it easier to verify this, we will use a port and make certain that
  // we have an async wait posted before we perform the ack.
  constexpr uint64_t kUntriggeredPortKey = 0xBadDecafC0ffee;
  zx::port untriggered_signal_port;
  ASSERT_OK(zx::port::create(0, &untriggered_signal_port));
  EXPECT_OK(interrupt.wait_async(untriggered_signal_port, kUntriggeredPortKey,
                                 ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, 0));
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &out), ZX_ERR_TIMED_OUT);

  // Now ack the signal.  We expect our async wait to have been satisfied, even
  // though the interrupt object does not have UNTRIGGERED asserted on it.
  EXPECT_OK(interrupt.ack());
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);  // No signals are asserted on our interrupt object.
  ASSERT_OK(untriggered_signal_port.wait(zx::time::infinite_past(), &out));
  EXPECT_EQ(kUntriggeredPortKey, out.key);
  ASSERT_EQ(ZX_PKT_TYPE_SIGNAL_ONE, out.type);
  EXPECT_EQ(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, out.signal.trigger);  // but, the UNTRIGGERED signal
                                                                    // satisfied our wait.

  // The packet for our second trigger should now be readable.
  ASSERT_OK(port.wait(zx::time::infinite_past(), &out));
  VerifyInterruptPacket(out, ett2);

  // Finally, perform our final ack.  This should reassert the UNTRIGGERED signal, but there should
  // not be any more interrupt packets waiting.
  EXPECT_OK(interrupt.ack());
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  EXPECT_EQ(port.wait(zx::time::infinite_past(), &out), ZX_ERR_TIMED_OUT);
}

TEST_F(InterruptTest, PacketDeliveredAfterBind) {
  zx::interrupt interrupt;
  zx::port port;

  // Create a new interrupt object and a new port, but *don't bind them* yet.
  constexpr uint64_t kInterruptPortKey = 0xDeadBeefBaadF00d;
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  // Fetch the initial signal state.  We expect the
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED to be asserted.
  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Trigger the interrupt, then verify that the UNTRIGGERED signal is de-asserted.
  const zx::time_boot expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Now bind the interrupt.  We expect a port packet to be delivered to our port immediately.
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, 0));

  // Reading and validate the port packet which we are expecting.
  auto VerifyInterruptPacket = [kInterruptPortKey](const zx_port_packet& packet,
                                                   const zx::time_boot& ett) {
    EXPECT_EQ(kInterruptPortKey, packet.key);
    ASSERT_EQ(ZX_PKT_TYPE_INTERRUPT, packet.type);
    EXPECT_EQ(ett.get(), packet.interrupt.timestamp);
  };

  zx_port_packet_t out;
  ASSERT_OK(port.wait(zx::time::infinite_past(), &out));
  VerifyInterruptPacket(out, expected_trigger_time);
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Even though we have received our packet, the interrupt does not re-enter the UNTRIGGERED state
  // until after we have ack'ed it.
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Now ack the interrupt and confirm that UNTRIGGERED is now asserted.
  EXPECT_OK(interrupt.ack());
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
}

// Test to be sure that an un-read interrupt packet will be preserved across a
// rebind operation.
//
// If:
// 1) An interrupt is bound to a port.
// 2) The interrupt has been triggered and a packet has been queued to the bound
//    port, not yet read.
// 3) The interrupt is unbound from the port, and re-bound to a different port.
// 4) The packet which had been queued to the port first port will have been
//    moved to the new port and be available for read.
//
TEST_F(InterruptTest, PacketFollowsRebindToNewPort) {
  zx::interrupt interrupt;
  zx::port port;
  zx::port port2;

  // Create a new interrupt object and a two ports.  Bind the interrupt to the first port.
  constexpr uint64_t kInterruptPortKey = 0xDeadBeefBaadF00d;
  constexpr uint64_t kInterruptPortKey2 = 0xBadDecafC0ffee;
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port2));
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, 0));

  // Fetch the initial signal state.  We expect the
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED to be asserted.
  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Trigger the interrupt, then verify that the UNTRIGGERED signal is de-asserted.
  const zx::time_boot expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // There should be a packet waiting for us to read, but don't read it yet.
  // Instead, unbind our interrupt from the first port and re-bind it to the
  // second port.
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, ZX_INTERRUPT_UNBIND));
  ASSERT_OK(interrupt.bind(port2, kInterruptPortKey2, 0));

  // Now try to read the packet from the first port.  This should fail.  Then
  // try to read the packet and verify from the second port.  Our packet should
  // be waiting for us there.
  zx_port_packet_t out;
  EXPECT_EQ(ZX_ERR_TIMED_OUT, port.wait(zx::time::infinite_past(), &out));
  EXPECT_OK(port2.wait(zx::time::infinite_past(), &out));
  EXPECT_EQ(kInterruptPortKey2, out.key);
  ASSERT_EQ(ZX_PKT_TYPE_INTERRUPT, out.type);
  EXPECT_EQ(expected_trigger_time.get(), out.interrupt.timestamp);
}

// Test to be sure that an un-read interrupt packet will be preserved across a
// unbind operation followed by a zx_interrupt_wait.
//
// If:
// 1) An interrupt is bound to a port.
// 2) The interrupt has been triggered and a packet has been queued to the bound
//    port, not yet read.
// 3) The interrupt is unbound from the port, but not rebound.
// 4) Someone then waits on the interrupt object using zx_interrupt_wait.
// 5) The original interrupt will be delivered to the waiter with the original
//    timestamp.
//
TEST_F(InterruptTest, PacketFollowsUnbindToWait) {
  zx::interrupt interrupt;
  zx::port port;

  // Create a new interrupt object and a port.  Bind the interrupt to the port.
  constexpr uint64_t kInterruptPortKey = 0xDeadBeefBaadF00d;
  ASSERT_OK(zx::interrupt::create(*irq_resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt));
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, 0));

  // Fetch the initial signal state.  We expect the
  // ZX_VIRTUAL_INTERRUPT_UNTRIGGERED to be asserted.
  zx_signals_t pending = 0;
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // Trigger the interrupt, then verify that the UNTRIGGERED signal is de-asserted.
  zx::time_boot expected_trigger_time = zx::clock::get_boot();
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // There should be a packet waiting for us to read, but we are not going to
  // read it.  Instead, unbind the interrupt from its port.
  ASSERT_OK(interrupt.bind(port, kInterruptPortKey, ZX_INTERRUPT_UNBIND));

  // Now wait on the interrupt object.  We should immediately succeed the wait
  // operation and be given the timestamp which was used to trigger the
  // interrupt.
  zx::time_boot actual_trigger_time;
  ASSERT_OK(interrupt.wait(&actual_trigger_time));
  EXPECT_EQ(expected_trigger_time.get(), actual_trigger_time.get());

  // The interrupt object's UNTRIGGERED signal should still not be asserted
  // since it has not been ack'ed yet.
  EXPECT_EQ(interrupt.wait_one(0, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0);

  // Create a thread to block on the interrupt object once again.  This will
  // acknowledge the interrupt and should re-assert the UNTRIGGERED signal.
  std::thread waiter_thread([&]() { interrupt.wait(&actual_trigger_time); });
  EXPECT_OK(interrupt.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite(), &pending));
  EXPECT_EQ(pending, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  // trigger our interrupt once again to release our thread, and then exit.
  expected_trigger_time += zx::msec(1500);
  EXPECT_OK(interrupt.trigger(0, expected_trigger_time));
  waiter_thread.join();
  EXPECT_EQ(expected_trigger_time.get(), actual_trigger_time.get());
}

}  // namespace
