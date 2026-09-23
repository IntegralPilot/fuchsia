// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/zx/counter.h>
#include <lib/zx/event.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <zxtest/zxtest.h>

namespace {

////////////////////////////////////////////////////////////////////////////////
// Create
////////////////////////////////////////////////////////////////////////////////

// Create a counter and verify its initial conditions.
TEST(CounterTest, CreateInitial) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  // Verify the koid, type, and rights.
  zx_info_handle_basic_t info;
  ASSERT_OK(counter.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  ASSERT_NE(0, info.koid);
  ASSERT_EQ(ZX_OBJ_TYPE_COUNTER, info.type);
  ASSERT_EQ(ZX_DEFAULT_COUNTER_RIGHTS, info.rights);

  // Verify that only ZX_COUNTER_NON_POSITIVE is asserted.
  zx_signals_t observed;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &observed));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, observed);

  // Verify the value is zero.
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);
}

// Verify that attempting to create with invalid options results in an error.
TEST(CounterTest, CreateInvalidOptions) {
  zx::counter counter;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::counter::create(1, &counter));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::counter::create(2, &counter));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::counter::create(3, &counter));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::counter::create(UINT32_MAX, &counter));
}

////////////////////////////////////////////////////////////////////////////////
// Add
////////////////////////////////////////////////////////////////////////////////

// Verify that add with an invalid handle fails.
TEST(CounterTest, AddInvalidHandle) {
  zx::counter counter;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, counter.add(0));
}

// Verify that add with a handle of the wrong type fails.
TEST(CounterTest, AddWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::unowned_counter counter(event.get());
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, counter->add(0));
}

// Verify that add with a handle lacking either ZX_RIGHT_READ or ZX_RIGHT_WRITE fails.
TEST(CounterTest, AddMissingRights) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  zx::counter c;

  // No ZX_RIGHT_READ.
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_READ, &c));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, c.add(0));

  // No ZX_RIGHT_WRITE.
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_WRITE, &c));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, c.add(0));

  // Neither.
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_READ & ~ZX_RIGHT_WRITE, &c));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, c.add(0));
}

// Verify that attempting to overflow fails.
TEST(CounterTest, AddOverflow) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  ASSERT_OK(counter.add(INT64_MAX));
  zx_signals_t current_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_POSITIVE, current_signals);

  // Signals are unchanged.
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, counter.add(1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_POSITIVE, current_signals);

  // Value is unchanged.
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(INT64_MAX, value);
}

// Verify that attempting to underflow fails.
TEST(CounterTest, AddUnderflow) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  ASSERT_OK(counter.add(INT64_MIN));
  zx_signals_t current_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, current_signals);

  // Signals are unchanged.
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, counter.add(-1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, current_signals);

  // Value is unchanged.
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(INT64_MIN, value);
}

// Verify that the happy case works as expected.
TEST(CounterTest, AddHappyCase) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));
  zx_signals_t initial_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &initial_signals));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, initial_signals);
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // Add 0 to 0, see that the signals and value are unchanged.
  ASSERT_OK(counter.add(0));
  zx_signals_t current_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(initial_signals, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // Add -1 to 0, see that the signals are unchanged and the value is updated.
  ASSERT_OK(counter.add(-1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(initial_signals, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(-1, value);

  // Add 1 to -1, see that the signals are unchanged and the value is updated.
  ASSERT_OK(counter.add(1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(initial_signals, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // We're now back to 0.  Add 1 and see that we deassert ZX_COUNTER_NON_POSITIVE and assert
  // ZX_COUNTER_POSITIVE.
  ASSERT_OK(counter.add(1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_POSITIVE, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(1, value);
}

////////////////////////////////////////////////////////////////////////////////
// Read
////////////////////////////////////////////////////////////////////////////////

// Verify that read with an invalid handle fails.
TEST(CounterTest, ReadInvalidHandle) {
  zx::counter counter;
  int64_t value;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, counter.read(&value));
}

// Verify that read with a handle of the wrong type fails.
TEST(CounterTest, ReadWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::unowned_counter counter(event.get());
  int64_t value;
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, counter->read(&value));
}

// Verify that read with a handle lacking ZX_RIGHT_READ fails.
TEST(CounterTest, ReadMissingRights) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_READ, &counter));

  int64_t value;
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, counter.read(&value));
}

// Verify that read with an invalid pointer fails.
TEST(CounterTest, ReadInvalidPointer) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  // Create an empty VMAR so we can verify what happens when the kernel tries to copy-in and there's
  // no mapping.
  const size_t kSize = zx_system_get_page_size();
  zx::vmar vmar;
  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, kSize,
                                            &vmar, &addr));
  auto* const value = reinterpret_cast<int64_t*>(addr);

  // No mapping.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, counter.read(value));

  // Mapped, but not readable.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0, &vmo));
  ASSERT_OK(vmar.map(0, 0, vmo, 0, kSize, &addr));
  auto unmap = fit::defer([&]() { ASSERT_OK(zx::vmar::root_self()->unmap(addr, kSize)); });

  // Simple null pointer.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, counter.read(nullptr));
}

// Verify that the happy case works as expected.
TEST(CounterTest, ReadHappyCase) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  // See that the initial value is zero.
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // Write a different value and see that we can read it back.
  ASSERT_OK(counter.write(42));
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(42, value);
}

////////////////////////////////////////////////////////////////////////////////
// Write
////////////////////////////////////////////////////////////////////////////////

// Verify that write with an invalid handle fails.
TEST(CounterTest, WriteInvalidHandle) {
  zx::counter counter;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, counter.write(0));
}

// Verify that write with a handle of the wrong type fails.
TEST(CounterTest, WriteWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx::unowned_counter counter(event.get());
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, counter->write(0));
}

// Verify that write with a handle lacking ZX_RIGHT_WRITE fails.
TEST(CounterTest, WriteMissingRights) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  zx::counter c;

  // No ZX_RIGHT_WRITE.
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_WRITE, &c));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, c.write(0));

  // However, no ZX_RIGHT_READ is OK.
  ASSERT_OK(counter.duplicate(ZX_DEFAULT_COUNTER_RIGHTS & ~ZX_RIGHT_READ, &c));
  ASSERT_OK(c.write(0));
}

// Verify that the happy case works as expected.
TEST(CounterTest, WriteHappyCase) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));
  zx_signals_t initial_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &initial_signals));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, initial_signals);
  int64_t value;
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // Write 0, see that the signals and value are unchanged.
  ASSERT_OK(counter.write(0));
  zx_signals_t current_signals;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(initial_signals, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(0, value);

  // Write -1, see that the signals are unchanged and the value is updated.
  ASSERT_OK(counter.write(-1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(initial_signals, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(-1, value);

  // Write 1, see that the signals and value are updated.
  ASSERT_OK(counter.write(1));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &current_signals));
  ASSERT_EQ(ZX_COUNTER_POSITIVE, current_signals);
  ASSERT_OK(counter.read(&value));
  ASSERT_EQ(1, value);
}

// Verify that zx_object_signal works on a counter.
TEST(CounterTest, Signal) {
  zx::counter counter;
  ASSERT_OK(zx::counter::create(0, &counter));

  // Signal ZX_USER_SIGNAL_0 and ZX_COUNTER_SIGNALED.
  ASSERT_OK(counter.signal(0, ZX_USER_SIGNAL_0 | ZX_COUNTER_SIGNALED));

  // Test invalid signals (e.g. ZX_SIGNAL_HANDLE_CLOSED).
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, counter.signal(0, ZX_SIGNAL_HANDLE_CLOSED));

  zx_signals_t observed;
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &observed));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE | ZX_USER_SIGNAL_0 | ZX_COUNTER_SIGNALED, observed);

  // Clear them.
  ASSERT_OK(counter.signal(ZX_USER_SIGNAL_0 | ZX_COUNTER_SIGNALED, 0));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, counter.wait_one(0u, zx::time::infinite_past(), &observed));
  ASSERT_EQ(ZX_COUNTER_NON_POSITIVE, observed);
}

}  // namespace
