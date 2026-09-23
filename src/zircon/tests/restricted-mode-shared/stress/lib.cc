// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib.h"

#include <lib/fit/defer.h>
#include <lib/zx/clock.h>
#include <lib/zx/event.h>
#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <thread>

#include <fbl/string_printf.h>
#include <zxtest/zxtest.h>

#include "helpers.h"
#include "reader_blob.h"

namespace {

enum class TestState : uint8_t {
  Mapped,
  Unmapped,
};

void Reader(std::atomic<uint32_t>& num_readers_ready, std::atomic<TestState>& mapped,
            zx_instant_mono_ticks_t* shared_value, zx_instant_mono_ticks_t* read_value) {
  // Attempt to read from the shared value. This should succeed because the value has been mapped
  // at this point.
  *read_value = *shared_value;

  // Signal to the orchestrator that we've completed our read and that it should unmap the
  // shared_value.
  num_readers_ready.fetch_add(1);

  // Wait for the orchestrator to unmap.
  while (mapped.load() == TestState::Mapped) {
  }

  // Now, this read should fault. The orchestrator should ensure that it does so.
  *read_value = *shared_value;
}

zx_status_t RunRestricted(const zx::vmo& vmo, zx_vaddr_t cs_addr, zx_vaddr_t stack_addr,
                          const zx_instant_mono_ticks_t* shared_value,
                          zx_instant_mono_ticks_t* read_value,
                          const zx_excp_type_t expected_exception) {
  zx_restricted_state_t state{};
#if defined(__x86_64__)
  state.ip = static_cast<uint64_t>(cs_addr);
  state.rsp = static_cast<uint64_t>(stack_addr);
  state.rdi = reinterpret_cast<uint64_t>(shared_value);
  state.rsi = reinterpret_cast<uint64_t>(read_value);
#elif defined(__aarch64__)  // defined(__x86_64__)
  state.pc = static_cast<uint64_t>(cs_addr);
  state.sp = static_cast<uint64_t>(stack_addr);
  state.x[0] = reinterpret_cast<uint64_t>(shared_value);
  state.x[1] = reinterpret_cast<uint64_t>(read_value);
#elif defined(__riscv)      // defined(__aarch64__)
  state.pc = static_cast<uint64_t>(cs_addr);
  state.sp = static_cast<uint64_t>(stack_addr);
  state.a0 = reinterpret_cast<uint64_t>(shared_value);
  state.a1 = reinterpret_cast<uint64_t>(read_value);
#else                       // defined(__riscv)
#error "what machine?"
#endif
  zx_status_t status = vmo.write(&state, 0, sizeof(state));
  if (status != ZX_OK) {
    return status;
  }

  zx_restricted_reason_t exit_reason = 99;
  zx_restricted_exception_t exception_state{};
  status = restricted_enter_wrapper(0, &exit_reason);
  if (status != ZX_OK) {
    return status;
  }
  if (exit_reason != ZX_RESTRICTED_REASON_EXCEPTION) {
    return ZX_ERR_INTERNAL;
  }
  status = vmo.read(&exception_state, 0, sizeof(exception_state));
  if (status != ZX_OK) {
    return status;
  }
  if (exception_state.exception.header.type != expected_exception) {
    printf("unexpected exception type; got 0x%x, want 0x%x\n",
           exception_state.exception.header.type, expected_exception);
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

void RestrictedReader(zx_vaddr_t cs_addr, zx_vaddr_t stack_addr, zx_status_t* status,
                      std::atomic<uint32_t>& num_readers_ready, std::atomic<TestState>& mapped,
                      zx_instant_mono_ticks_t* shared_value, zx_instant_mono_ticks_t* read_value,
                      zx_instant_mono_ticks_t expected_value) {
  // Wait for the RestrictedWriter to map and write the shared value.
  while (mapped.load() == TestState::Unmapped) {
  }

  zx::vmo restricted;
  *status = zx_restricted_bind_state(0, restricted.reset_and_get_address(), nullptr);
  if (*status != ZX_OK) {
    return;
  }
  // Enter restricted mode and expect it to exit with a software breakpoint.
  *status = RunRestricted(restricted, cs_addr, stack_addr, shared_value, read_value,
                          ZX_EXCP_SW_BREAKPOINT);
  if (*status != ZX_OK) {
    return;
  }

  // Restricted mode should have successfully read the value.
  if (*read_value != expected_value) {
    *status = ZX_ERR_INTERNAL;
    return;
  }

  // Signal to the orchestrator that we've successfully read the value and that it should unmap the
  // shared value.
  num_readers_ready.fetch_add(1);

  // Wait for the writer to have unmapped the shared value.
  while (mapped.load() == TestState::Mapped) {
  }

  // Attempt to read the shared value from the unified aspace. This should page fault until the
  // orchestrator remaps the value and resumes us.
  *read_value = *shared_value;
}

void RestrictedWriter(std::atomic<TestState>& mapped, zx_vaddr_t shared_addr,
                      zx_instant_mono_ticks_t value) {
  *reinterpret_cast<volatile zx_instant_mono_ticks_t*>(shared_addr) = value;
  mapped.store(TestState::Mapped);
}

void RunRestrictedTestIteration() {
  const uint32_t kNumReaders = 16;
  const uint32_t page_size = zx_system_get_page_size();

  // Create a shared process that will hold all of the threads that need to access a restricted
  // address space. This includes all of the reader threads and a writer thread.
  zx::process restricted_proc;
  zx::vmar restricted_vmar;
  auto proc_name = fbl::StringPrintf("unified-restricted-stress-process");
  ASSERT_OK(zx_process_create_shared(zx_process_self(), 0, proc_name.data(), proc_name.size(),
                                     restricted_proc.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  // Create a sub_vmar to make it easy to guarantee that the shared address does not change on
  // future remaps.
  zx::vmar sub_vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(restricted_vmar.allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, page_size,
                                     &sub_vmar, &vmar_addr));

  // Create an exception channel attached to the shared process.
  zx::channel exception_channel;
  ASSERT_OK(restricted_proc.create_exception_channel(0, &exception_channel));

  // Set up a code segment in the restricted region for all of the reader threads to use.
  zx::result<zx_vaddr_t> result = SetupCodeSegment(restricted_vmar.get(), reader_blob());
  ASSERT_OK(result.status_value());
  zx_vaddr_t cs_addr = result.value();

  // Set up a VMO that will store the values the readers observed.
  zx::vmo observed_values;
  zx_vaddr_t observed_values_addr;
  ASSERT_OK(zx::vmo::create(page_size, 0, &observed_values));
  ASSERT_OK(restricted_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, observed_values, 0,
                                page_size, &observed_values_addr));
  zx_instant_mono_ticks_t* read_values =
      reinterpret_cast<zx_instant_mono_ticks_t*>(observed_values_addr);

  // Map shared address.
  zx::vmo vmo;
  zx_vaddr_t shared_addr;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));
  ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &shared_addr));

  // Spin up all of the readers.
  zx::vmo stack_vmos[kNumReaders];
  zx_status_t reader_statuses[kNumReaders]{};
  std::atomic<uint32_t> num_readers_ready{};
  std::atomic<TestState> mapped = TestState::Unmapped;
  zx_instant_mono_ticks_t expected_value = zx::ticks::now().get();
  std::vector<std::thread> threads;
  threads.reserve(kNumReaders);

  zx_handle_t orig_proc = thrd_set_zx_process(restricted_proc.get());
  for (uint32_t i = 0; i < kNumReaders; i++) {
    result = SetupStack(restricted_vmar.get(), page_size, &stack_vmos[i]);
    ASSERT_OK(result.status_value());

    // Create the reader thread.
    threads.emplace_back(RestrictedReader, cs_addr, result.value(), &reader_statuses[i],
                         std::ref(num_readers_ready), std::ref(mapped),
                         reinterpret_cast<zx_instant_mono_ticks_t*>(shared_addr), &read_values[i],
                         expected_value);
  }
  // Spin up a writer thread to write expected_value to the shared_addr.
  // We do this after spinning up the reader threads to ensure that we have other running threads
  // present in the restricted_proc when the writer exits. Spinning this thread up before spawning
  // the readers creates a race condition in which the writer can terminate before the reader
  // threads are created, which then leads to the restricted_proc being torn down prematurely.
  std::thread writer(RestrictedWriter, std::ref(mapped), shared_addr, expected_value);
  writer.join();
  thrd_set_zx_process(orig_proc);

  // Wait for all of the readers to successfully read the shared value.
  while (num_readers_ready.load() != kNumReaders) {
  }

  // Unmap the shared address.
  ASSERT_OK(sub_vmar.unmap(shared_addr, page_size));

  // Signal to the readers that the shared address has been unmapped and that they should fault.
  mapped.store(TestState::Unmapped);

  // Wait for all of the readers to fault.
  zx::exception exceptions[kNumReaders];
  for (uint32_t i = 0; i < kNumReaders; i++) {
    zx_exception_info_t info;
    zx::exception exception;
    ASSERT_OK(exception_channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(exception_channel.read(0, &info, exceptions[i].reset_and_get_address(), sizeof(info),
                                     1, nullptr, nullptr));
    EXPECT_EQ(info.type, ZX_EXCP_FATAL_PAGE_FAULT);
  }

  // Remap the shared address so that the readers can resume.
  ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &shared_addr));

  // Resume the readers.
  for (uint32_t i = 0; i < kNumReaders; i++) {
    uint32_t handled = ZX_EXCEPTION_STATE_HANDLED;
    ASSERT_OK(exceptions[i].set_property(ZX_PROP_EXCEPTION_STATE, &handled, sizeof(handled)));
    exceptions[i].reset();
  }

  // Wait for all the readers to complete.
  for (size_t i = 0; i < threads.size(); i++) {
    threads[i].join();
  }

  // Verify that all of the readers succeeded.
  for (uint32_t i = 0; i < kNumReaders; i++) {
    EXPECT_OK(reader_statuses[i]);
  }
}

void RunSharedTestIteration() {
  const uint32_t kNumReaders = 16;
  zx::process procs[kNumReaders];
  zx::vmar vmars[kNumReaders];
  zx::channel exception_channels[kNumReaders];
  zx_instant_mono_ticks_t read_values[kNumReaders]{};
  std::vector<std::thread> threads;
  threads.reserve(kNumReaders);

  // Create a sub-vmar that will contain the address we are mapping. We do this to make sure no
  // other code can "steal" this address out from under us.
  auto page_size = zx_system_get_page_size();
  zx::vmar sub_vmar;
  uintptr_t vmar_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, page_size,
                                            &sub_vmar, &vmar_addr));
  auto cleanup = fit::defer([&sub_vmar]() { ASSERT_OK(sub_vmar.destroy()); });

  // Map a shared address.
  zx::vmo vmo;
  zx_vaddr_t shared_addr;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));
  ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &shared_addr));
  ASSERT_EQ(shared_addr, (zx_vaddr_t)vmar_addr);

  // Write a random value (in this case the current ticks) to addr.
  zx_instant_mono_ticks_t expected_value = zx::ticks::now().get();
  *reinterpret_cast<volatile zx_instant_mono_ticks_t*>(shared_addr) = expected_value;

  std::atomic<TestState> mapped = TestState::Mapped;
  std::atomic<uint32_t> num_readers_ready = 0;
  for (uint32_t i = 0; i < kNumReaders; i++) {
    // Create a shared process to house the reader thread.
    auto proc_name = fbl::StringPrintf("%u-unified-shared-stress-reader", i);
    ASSERT_OK(zx_process_create_shared(zx_process_self(), 0, proc_name.data(), proc_name.size(),
                                       procs[i].reset_and_get_address(),
                                       vmars[i].reset_and_get_address()));

    // Create an exception channel attached to the shared process.
    ASSERT_OK(procs[i].create_exception_channel(0, &exception_channels[i]));

    // Create the reader thread.
    zx_handle_t orig_proc = thrd_set_zx_process(procs[i].get());
    threads.emplace_back(Reader, std::ref(num_readers_ready), std::ref(mapped),
                         reinterpret_cast<zx_instant_mono_ticks_t*>(shared_addr), &read_values[i]);
    thrd_set_zx_process(orig_proc);
  }

  // Wait for all of the readers to signal to us that they have read the value and are ready.
  while (num_readers_ready.load() != kNumReaders) {
  }

  // Verify that all of the readers read the expected value out of the given address.
  for (uint32_t i = 0; i < kNumReaders; i++) {
    EXPECT_EQ(read_values[i], expected_value);
  }

  // Unmap the shared address and signal to the readers that we have done so.
  ASSERT_OK(sub_vmar.unmap(shared_addr, page_size));
  mapped.store(TestState::Unmapped);

  // Wait for all the readers to fault.
  zx::exception exceptions[kNumReaders];
  for (uint32_t i = 0; i < kNumReaders; i++) {
    zx_exception_info_t info;
    ASSERT_OK(exception_channels[i].wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(exception_channels[i].read(0, &info, exceptions[i].reset_and_get_address(),
                                         sizeof(info), 1, nullptr, nullptr));
    EXPECT_EQ(info.type, ZX_EXCP_FATAL_PAGE_FAULT);
  }

  // Remap the VMO.
  ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &shared_addr));
  ASSERT_EQ(shared_addr, (zx_vaddr_t)vmar_addr);
  expected_value = zx::ticks::now().get();
  *reinterpret_cast<volatile zx_instant_mono_ticks_t*>(shared_addr) = expected_value;

  // Resume all of the readers.
  for (uint32_t i = 0; i < kNumReaders; i++) {
    uint32_t handled = ZX_EXCEPTION_STATE_HANDLED;
    ASSERT_OK(exceptions[i].set_property(ZX_PROP_EXCEPTION_STATE, &handled, sizeof(handled)));
    exceptions[i].reset();
  }

  // Wait for all of the readers to complete successfully.
  for (size_t i = 0; i < threads.size(); i++) {
    threads[i].join();
  }

  // Once again, verify that all of the readers read the expected value out of the given address.
  for (uint32_t i = 0; i < kNumReaders; i++) {
    EXPECT_EQ(read_values[i], expected_value);
  }
}

}  // namespace

void Orchestrator(zx::duration runtime, zx::duration poll_interval, TestMode test_mode) {
  const zx::time deadline = zx::deadline_after(runtime);
  zx::time poll_deadline = zx::deadline_after(poll_interval);
  uint64_t num_iterations = 0;
  printf("running stress test for %ld seconds\n", runtime.to_secs());
  while (zx::clock::get_monotonic() < deadline) {
    if (zx::clock::get_monotonic() > poll_deadline) {
      printf("test still running, %lu iterations completed\n", num_iterations);
      poll_deadline = zx::deadline_after(poll_interval);
    }
    if (test_mode == TestMode::Restricted) {
      RunRestrictedTestIteration();
    } else if (test_mode == TestMode::Shared) {
      RunSharedTestIteration();
    }
    num_iterations++;
  }
}
