// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/intrin.h>
#include <lib/fit/defer.h>
#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/system/utest/core/needs-next.h>
#include <zircon/testonly-syscalls.h>
#include <zircon/threads.h>

#include <mutex>
#include <optional>
#include <thread>

#include <bringup/lib/restricted-machine/environment.h>
#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/machine.h>
#include <bringup/lib/restricted-machine/testing/needs-next.h>
#include <zxtest/zxtest.h>

#include "arch-helper.h"

#include <bringup/lib/restricted-machine/testing/fixture.zxtest.h>

static const uint32_t kRestrictedThreadCount = 32;

namespace {

static constexpr size_t kEnvironmentMemorySize = 8 * 1024 * 1024;  // 8M

class RestrictedMode : public restricted_machine::testing::SupportedMachinesTest {
 public:
  static constexpr uint64_t kBoundaryTargetMapping = 0x00000000fffff000;

  static void SetUpTestSuite() {
    RM_NEEDS_NEXT_SKIP;
    static const std::vector<std::string_view> kSymbols{
        "syscall_bounce",    "syscall_bounce_post_syscall",
        "exception_bounce",  "exception_bounce_exception_address",
        "wait_then_syscall", "store_one",
#ifdef __aarch64__
        "bad_increment",
#endif
    };
    SetUpTestSuiteHelper("restricted-blob", &kSymbols, kEnvironmentMemorySize);
    auto arm_env = environment(restricted_machine::MachineType::kArm);
    if (arm_env.is_ok()) {
      ASSERT_OK(arm_env.value()->AddLoadableBlob("boundary", kBoundaryTargetMapping));
      ASSERT_OK(arm_env.value()->AddLoadableBlob("thumb"));
    }
  }

  void SetUp() override {
    restricted_machine::testing::SupportedMachinesTest::SetUp();
    RM_NEEDS_NEXT_SKIP;
  }
  void TearDown() override {}

  std::unique_ptr<ArchHelper> GetArchHelper() { return ArchHelperFactory::Create(machine()); }
};

}  // namespace

// Test all valid restricted mode targets.
INSTANTIATE_TEST_SUITE_P(, RestrictedMode, zxtest::ValuesIn(restricted_machine::kSupportedMachines),
                         ::restricted_machine::testing::SupportedMachinesTest::ParamToText);

// Verify that restricted_enter handles invalid args.
TEST_P(RestrictedMode, EnterInvalidArgs) {
  NEEDS_NEXT_SKIP(zx_restricted_enter);

  // Invalid options.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_restricted_enter(0xffffffff, 0, 0));

  // Enter restricted mode with invalid args.
  // Vector table must be valid user pointer.
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_restricted_enter(0, -1, 0));
}

TEST_P(RestrictedMode, BindState) {
  NEEDS_NEXT_SKIP(zx_restricted_bind_state);

  // Bad options.
  zx::vmo v_invalid;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_restricted_bind_state(1, v_invalid.reset_and_get_address(), nullptr));
  ASSERT_FALSE(v_invalid.is_valid());
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_restricted_bind_state(2, v_invalid.reset_and_get_address(), nullptr));
  ASSERT_FALSE(v_invalid.is_valid());
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_restricted_bind_state(0xffffffff, v_invalid.reset_and_get_address(), nullptr));
  ASSERT_FALSE(v_invalid.is_valid());

  // Happy case.
  zx::vmo vmo;
  ASSERT_OK(zx_restricted_bind_state(0, vmo.reset_and_get_address(), nullptr));
  auto cleanup = fit::defer([]() { EXPECT_OK(zx_restricted_unbind_state(0)); });

  // Binding again is fine and replaces any previously bound VMO.
  ASSERT_OK(zx_restricted_bind_state(0, vmo.reset_and_get_address(), nullptr));
  ASSERT_TRUE(vmo.is_valid());

  // Map the vmo and verify the state follows.
  zx_vaddr_t ptr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &ptr));
  zx_restricted_state_t* state2 = reinterpret_cast<zx_restricted_state_t*>(ptr);

  // Read the state out of the vmo and compare with memory map.
  zx_restricted_state_t state = {};
  ASSERT_OK(vmo.read(&state, 0, sizeof(state)));
  EXPECT_EQ(0, memcmp(state2, &state, sizeof(state)));

  // Fill the state with garbage and make sure it follows.
  memset(state2, 0x99, sizeof(state));
  ASSERT_OK(vmo.read(&state, 0, sizeof(state)));
  EXPECT_EQ(0, memcmp(state2, &state, sizeof(state)));

  // Write garbage via the write syscall and read it back out of the mapping.
  memset(&state, 0x55, sizeof(state));
  ASSERT_OK(vmo.write(&state, 0, sizeof(state)));
  EXPECT_EQ(0, memcmp(state2, &state, sizeof(state)));

  // Teardown the mapping.
  zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
}

TEST_P(RestrictedMode, BindStateWithExceptionReport) {
  NEEDS_NEXT_SKIP(zx_restricted_bind_state);

  zx::vmo vmo;
  zx_exception_report_t report;

  ASSERT_OK(zx_restricted_bind_state(0, vmo.reset_and_get_address(), &report));
  auto cleanup = fit::defer([]() { EXPECT_OK(zx_restricted_unbind_state(0)); });
}

TEST_P(RestrictedMode, UnbindState) {
  NEEDS_NEXT_SKIP(zx_restricted_unbind_state);

  // Repeated unbind is OK.
  ASSERT_OK(zx_restricted_unbind_state(0));
  ASSERT_OK(zx_restricted_unbind_state(0));
  ASSERT_OK(zx_restricted_unbind_state(0));

  // Options must be 0.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_restricted_unbind_state(1));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_restricted_unbind_state(1));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_restricted_unbind_state(0xffffffff));
}

// This is the happy case.
TEST_P(RestrictedMode, Basic) {
  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  helper->SetInitialState(machine.registers());

  auto sb_addr = environment()->SymbolAddress("syscall_bounce");
  ASSERT_OK(sb_addr);
  machine.registers()->set_pc(sb_addr.value());
  EXPECT_OK(machine.CommitState());
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_SYSCALL, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_SYSCALL);
  ASSERT_EQ(ZX_RESTRICTED_REASON_SYSCALL, r.value());

  // Validate that the instruction pointer is right after the syscall instruction.
  auto lookup = environment()->SymbolAddress("syscall_bounce_post_syscall");
  ASSERT_OK(lookup);
  EXPECT_EQ(lookup.value(), machine.registers()->pc());

  helper->VerifyState(machine.registers());
  helper->VerifyStateMutation(machine.registers(), RegisterMutation::kFromSyscall);
}

// Verify that floating point state is saved correctly on context switch.
TEST_P(RestrictedMode, FloatingPointState) {
  auto helper = GetArchHelper();
  constexpr uint32_t kNumRestrictedThreads = kRestrictedThreadCount;
  // Ensure that the number of threads will be high enough to write to all FPU
  // registers.
  static const uint32_t kNumFloatingPointThreads = zx_system_get_num_cpus() * 2;
  std::atomic_int num_threads_ready = 0;
  auto num_threads_in_rmode = environment()->MakeArgument<std::atomic_int>(0);
  std::atomic_int start_restricted_threads = 0;
  auto exit_restricted_mode = environment()->MakeArgument<std::atomic_int>(0);
  zx_status_t statuses[kNumRestrictedThreads]{};
  memset(statuses, ZX_OK, sizeof(statuses));
  std::vector<std::thread> threads;

  auto thread_body = [this, &helper, &exit_restricted_mode, &num_threads_ready,
                      &num_threads_in_rmode, &start_restricted_threads,
                      &statuses](uint32_t thread_num) {
    // Configure the initial register state.
    restricted_machine::Machine machine(environment());
    ASSERT_TRUE(machine.Initialize(0));
    helper->SetInitialState(machine.registers());
    machine.enable_fpu_registers(true);

    auto wait_then_syscall_addr = environment()->SymbolAddress("wait_then_syscall");
    ASSERT_OK(wait_then_syscall_addr);
    machine.registers()->set_pc(wait_then_syscall_addr.value());
    machine.registers()->set_arg_regs(reinterpret_cast<uint64_t>(num_threads_in_rmode.get()),
                                      reinterpret_cast<uint64_t>(exit_restricted_mode.get()));
    auto commit_result = machine.CommitState();
    if (commit_result.is_error()) {
      statuses[thread_num] = commit_result.error_value();
      return;
    }

    // Wait for the main thread to tell us that we can write the FPU state and enter
    // restricted mode. We synchronize this to make sure that all of the restricted threads
    // modify their FPU state at the same time - context switching between the FPU write and
    // the entry into restricted mode can cause the threads to save the FPU state in normal
    // mode, which we do not want.
    num_threads_ready.fetch_add(1);
    while (start_restricted_threads.load() == 0) {
      std::ignore = zx_thread_legacy_yield(0);
    }

    // Construct the desired FPU state and save a copy to validate after.
    memset(machine.FpuRegisters()->data(), 0x10 + thread_num, machine.FpuRegisters()->size());
    std::vector<uint8_t> expected_fpu_registers(machine.FpuRegisters()->size());
    memcpy(expected_fpu_registers.data(), machine.FpuRegisters()->data(),
           expected_fpu_registers.size());
    auto restricted_result = machine.Enter();
    if (restricted_result.is_error()) {
      statuses[thread_num] = restricted_result.error_value();
      return;
    }
    zx_restricted_reason_t reason_code = restricted_result.value();
    if (reason_code != ZX_RESTRICTED_REASON_SYSCALL) {
      ADD_FAILURE() << "thread " << thread_num << ": received reason code " << reason_code
                    << "instead of ZX_RESTRICTED_REASON_SYSCALL";
      // This is not a desired outcome. However, if it happens, logging
      // actionable information is helpful.
      machine.LogState(ZX_RESTRICTED_REASON_SYSCALL);
      statuses[thread_num] = ZX_ERR_BAD_STATE;
      return;
    }

    // Validate that the FPU contains the expected contents.
    if (this->machine() == restricted_machine::MachineType::kAarch64) {
      for (size_t i = 0; i < 32 * 16; i++) {
        expected_fpu_registers[i] = static_cast<uint8_t>(expected_fpu_registers[i] << 1);
      }
    } else if (this->machine() == restricted_machine::MachineType::kArm) {
      for (size_t i = 0; i < 8 * 16; i++) {
        expected_fpu_registers[i] = static_cast<uint8_t>(expected_fpu_registers[i] << 1);
      }
    }

    if (memcmp(expected_fpu_registers.data(), machine.FpuRegisters()->data(),
               expected_fpu_registers.size()) != 0) {
      statuses[thread_num] = ZX_ERR_BAD_STATE;
      // Print out the diff for easy debugging.
      for (uint16_t i = 0; i < machine.FpuRegisters()->size(); i++) {
        if (machine.FpuRegisters()->at(i) != expected_fpu_registers.at(i)) {
          // Mark this test as a failure.
          ADD_FAILURE() << "thread " << thread_num << ": byte " << i << " differs; got 0x"
                        << std::hex << static_cast<unsigned int>(machine.FpuRegisters()->at(i))
                        << ", want 0x" << static_cast<unsigned int>(expected_fpu_registers.at(i));
        }
      }
    } else {
      statuses[thread_num] = ZX_OK;
    }
  };

  for (uint32_t i = 0; i < kNumRestrictedThreads; i++) {
    threads.emplace_back(thread_body, i);
  }

  // Wait for all the threads that will run restricted mode to spawn.
  while (num_threads_ready.load() != kNumRestrictedThreads) {
    // Check that each thread has successfully made it to the fetch_add that
    // increments num_threads_ready. This will ensure the test fails out
    // instead of hanging in those cases.
    for (uint32_t i = 0; i < kNumRestrictedThreads; i++) {
      ASSERT_OK(statuses[i]) << "thread " << i << " failed to bind state or write state VMO\n";
    }
    std::ignore = zx_thread_legacy_yield(0);
  }
  // Tell all of the restricted threads to start.
  start_restricted_threads.store(1);

  // Wait for all of the restricted threads to enter restricted mode.
  while (num_threads_in_rmode->load() != kNumRestrictedThreads) {
    // Check that each thread has successfully made it to the fetch_add that
    // increments num_threads_in_rmode. This will ensure the test fails out
    // instead of hanging in those cases.
    for (uint32_t i = 0; i < kNumRestrictedThreads; i++) {
      ASSERT_OK(statuses[i]) << "thread " << i << " failed to make it into restricted mode\n";
    }
    std::ignore = zx_thread_legacy_yield(0);
  }

  // Spawn a bunch of threads that overwrite the contents of the floating point registers.
  // We spawn enough threads to make sure that all CPU's floating point registers are overwritten.
  for (uint32_t i = 0; i < kNumFloatingPointThreads; i++) {
    threads.emplace_back(
        [](uint32_t thread_num) {
          char fpu_buffer[restricted_machine::RegisterState::kFpuBufferSize];
          memset(&fpu_buffer[0], 0x90 + thread_num,
                 restricted_machine::RegisterState::kFpuBufferSize);
          restricted_machine::internal::load_fpu_registers(fpu_buffer);
        },
        i);
  }

  // Signal all of the restricted mode threads to exit, then wait for them to do so.
  exit_restricted_mode->store(1);
  for (auto& thread : threads) {
    thread.join();
  }
  for (uint32_t i = 0; i < kNumRestrictedThreads; i++) {
    ASSERT_OK(statuses[i]);
  }
}

// This is a simple benchmark test that prints some rough performance numbers.
TEST_P(RestrictedMode, Bench) {
  auto helper = GetArchHelper();
  // Run the test 5 times to help filter out noise.
  for (auto i = 0; i < 5; i++) {
    zx::vmo vmo;

    // Set the state.
    restricted_machine::Machine machine(environment());
    ASSERT_TRUE(machine.Initialize());
    helper->SetInitialState(machine.registers());

    auto sb_addr = environment()->SymbolAddress("syscall_bounce");
    ASSERT_OK(sb_addr);
    machine.registers()->set_pc(sb_addr.value());
    EXPECT_OK(machine.CommitState());

    // Go through a full restricted syscall entry/exit cycle iter times and show the time.
    {
      auto t = zx::ticks::now();
      auto deadline = t + zx::ticks::per_second();
      int iter = 0;
      while (zx::ticks::now() <= deadline) {
        ASSERT_OK(machine.Continue());
        iter++;
      }
      t = zx::ticks::now() - t;

      printf("restricted call %ld ns per round trip (%ld raw ticks), %d iters\n",
             t / iter * ZX_SEC(1) / zx::ticks::per_second(), t.get(), iter);
    }

    zx::nanosleep(zx::deadline_after(zx::msec(10)));

    {
      // For way of comparison, time a null syscall.
      auto t = zx::ticks::now();
      auto deadline = t + zx::ticks::per_second();
      int iter = 0;
      while (zx::ticks::now() <= deadline) {
        ASSERT_OK(zx_syscall_test_0());
        iter++;
      }
      t = zx::ticks::now() - t;

      printf("test syscall %ld ns per call (%ld raw ticks), %d iters\n",
             t / iter * ZX_SEC(1) / zx::ticks::per_second(), t.get(), iter);
    }

    zx::nanosleep(zx::deadline_after(zx::msec(10)));

    // In-thread exception handling
    auto t = zx::ticks::now();
    auto deadline = t + zx::ticks::per_second();
    int iter = 0;
    auto exc_addr = environment()->SymbolAddress("exception_bounce_exception_address");
    ASSERT_OK(exc_addr);
    machine.registers()->set_pc(exc_addr.value());
    EXPECT_OK(machine.CommitState());

    while (zx::ticks::now() <= deadline) {
      ASSERT_OK(machine.Continue());
      iter++;
    }
    t = zx::ticks::now() - t;

    printf("in-thread exceptions %ld ns per round trip (%ld raw ticks) %d iters\n",
           t / iter * ZX_SEC(1) / zx::ticks::per_second(), t.get(), iter);

    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
}

enum class ExceptionReportLocation {
  UserPtr,
  ModeStateVmo,
};
void TestInThreadException(RestrictedMode& fixture, ExceptionReportLocation report_location) {
  auto helper = fixture.GetArchHelper();
  restricted_machine::Machine machine(fixture.environment());
  zx_exception_report_t report_storage{};
  zx_exception_report_t* const report =
      report_location == ExceptionReportLocation::UserPtr ? &report_storage : nullptr;
  ASSERT_TRUE(machine.Initialize(restricted_machine::Machine::kDefaultStackBytes, report));
  helper->SetInitialState(machine.registers());

  auto exc_addr = fixture.environment()->SymbolAddress("exception_bounce");
  ASSERT_OK(exc_addr);
  machine.registers()->set_pc(exc_addr.value());
  EXPECT_OK(machine.CommitState());
  auto result = machine.Enter();
  EXPECT_OK(result);
  ASSERT_EQ(ZX_RESTRICTED_REASON_EXCEPTION, result.value());

  zx_exception_report_t* exception_report = &machine.registers()->exception_report();
  EXPECT_EQ(ZX_EXCP_UNDEFINED_INSTRUCTION, exception_report->header.type);
  EXPECT_EQ(0u, exception_report->context.synth_code);
  EXPECT_EQ(0u, exception_report->context.synth_data);
#if defined(__x86_64__)
  EXPECT_EQ(0u, exception_report->context.arch.u.x86_64.err_code);
  EXPECT_EQ(0u, exception_report->context.arch.u.x86_64.cr2);
  // 0x6 corresponds to the invalid opcode vector.
  EXPECT_EQ(0x6u, exception_report->context.arch.u.x86_64.vector);
#elif defined(__aarch64__)
  constexpr uint32_t kEsrIlBit = 1ull << 25;
  EXPECT_EQ(kEsrIlBit, exception_report->context.arch.u.arm_64.esr);
  EXPECT_EQ(0u, exception_report->context.arch.u.arm_64.far);
#elif defined(__riscv)
  EXPECT_EQ(0x2, exception_report->context.arch.u.riscv_64.cause);
  EXPECT_EQ(0u, exception_report->context.arch.u.riscv_64.tval);
#endif
}

// Verify we can receive exceptions using an in-thread exception handler and bound user pointer.
TEST_P(RestrictedMode, InThreadExceptionUserPtr) {
  TestInThreadException(*this, ExceptionReportLocation::UserPtr);
}

// Verify we can receive exceptions using an in-thread exception handler and the mode state VMO.
TEST_P(RestrictedMode, InThreadExceptionModeState) {
  TestInThreadException(*this, ExceptionReportLocation::ModeStateVmo);
}

// Verify we properly report failure when we fail to copy-out the exception report.
TEST_P(RestrictedMode, ExceptionReportPointerReadOnly) {
  // Create a VMO to hold the exception report and map it.  We map it as read-only to ensure the
  // copy-out fails.
  zx::vmo report_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &report_vmo));
  zx_vaddr_t report_mapped_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, report_vmo, 0, zx_system_get_page_size(),
                                       &report_mapped_addr));
  auto unmap_report = fit::defer(
      [&]() { zx::vmar::root_self()->unmap(report_mapped_addr, zx_system_get_page_size()); });
  auto* report_storage = reinterpret_cast<zx_exception_report_t*>(report_mapped_addr);

  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize(restricted_machine::Machine::kDefaultStackBytes, report_storage));
  helper->SetInitialState(machine.registers());

  auto exc_addr = environment()->SymbolAddress("exception_bounce");
  ASSERT_OK(exc_addr);
  machine.registers()->set_pc(exc_addr.value());
  EXPECT_OK(machine.CommitState());
  auto result = machine.Enter();
  EXPECT_OK(result);
  ASSERT_EQ(ZX_RESTRICTED_REASON_EXCEPTION_LOST, result.value());
}

// Verify that restricted_enter fails on invalid zx_restricted_state_t values.
TEST_P(RestrictedMode, EnterBadStateStruct) {
  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  helper->SetInitialState(machine.registers());
  auto state = machine.registers();

  [[maybe_unused]] auto set_state_and_enter = [&]() {
    // Set the state.
    ASSERT_OK(machine.CommitState());

    // This should fail with bad state.
    auto result = machine.Enter();
    ASSERT_TRUE(result.is_error());
    ASSERT_EQ(ZX_ERR_BAD_STATE, result.error_value());
  };

  state->set_pc(-1);  // pc is outside of user space
  set_state_and_enter();

  helper->SetInitialState(state);
  auto sb_addr = environment()->SymbolAddress("syscall_bounce");
  ASSERT_OK(sb_addr);
  state->set_pc(sb_addr.value());

#ifdef __x86_64__
  state->restricted_state().flags = (1UL << 31);  // set an invalid flag
  set_state_and_enter();

  helper->SetInitialState(state);
  state->set_pc(sb_addr.value());
  state->restricted_state().fs_base = (1UL << 63);  // invalid fs (non canonical)
  set_state_and_enter();

  helper->SetInitialState(state);
  state->set_pc(sb_addr.value());
  state->restricted_state().gs_base = (1UL << 63);  // invalid gs (non canonical)
  set_state_and_enter();
#endif

#ifdef __aarch64__
  state->restricted_state().cpsr = 0x1;  // CPSR contains non-user settable flags.
  set_state_and_enter();
#endif
}

TEST_P(RestrictedMode, KickBeforeEnter) {
  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  helper->SetInitialState(machine.registers());

  auto sb_addr = environment()->SymbolAddress("syscall_bounce");
  ASSERT_OK(sb_addr);
  machine.registers()->set_pc(sb_addr.value());
  EXPECT_OK(machine.CommitState());

  // Issue a kick on ourselves which should apply to the next attempt to enter restricted mode.
  ASSERT_OK(machine.Kick());

  // Enter restricted mode with reasonable args, expect it to return due to kick and not run any
  // restricted mode code.
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_KICK, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_KICK);

  // Verify the state
  helper->VerifyState(machine.registers());

  // Validate that the instruction pointer is still pointing at the entry point.
  EXPECT_EQ(sb_addr.value(), machine.registers()->pc());

#if defined(__x86_64__)
  // Validate that the state is unchanged
  EXPECT_EQ(0x0101010101010101, machine.registers()->restricted_state().rax);
#elif defined(__aarch64__)  // defined(__x86_64__)
  // Even aarch32 will show the initialized 64-bit value here since
  // it was never re-saved by zircon.
  EXPECT_EQ(0x0202020202020202, machine.registers()->restricted_state().x[1]);
#elif defined(__riscv)      // defined(__aarch64__)
  EXPECT_EQ(0x0b0b0b0b0b0b0b0b, machine.registers()->restricted_state().a1);
#endif                      // defined(__riscv)

  // Check that the kicked state is cleared
  r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_SYSCALL, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_SYSCALL);

  // Read the state out of the thread.
  helper->VerifyState(machine.registers());

  // Validate that the instruction pointer is right after the syscall instruction.
  auto post_syscall = environment()->SymbolAddress("syscall_bounce_post_syscall");
  ASSERT_OK(post_syscall);
  EXPECT_EQ(post_syscall.value(), machine.registers()->pc());

  // Validate that the value in first general purpose register is incremented.
  helper->VerifyStateMutation(machine.registers(), RegisterMutation::kFromSyscall);
}

TEST_P(RestrictedMode, KickWhileStartingAndExiting) {
  struct ExceptionChannelRegistered {
    std::condition_variable cv;
    std::mutex m;
    bool registered = false;
  };
  ExceptionChannelRegistered ec;

  struct ChildThreadStarted {
    std::condition_variable cv;
    std::mutex m;
    zx_koid_t koid = 0;
  };
  ChildThreadStarted ct;

  // Register a debugger exception channel so we can intercept thread lifecycle events
  // and issue restricted kicks. This runs on a child thread so that we can process events
  // while the main thread is blocked on the main thread starting and joining.
  std::thread exception_thread([&ec, &ct]() {
    zx::channel exception_channel;
    ASSERT_OK(zx::process::self()->create_exception_channel(ZX_EXCEPTION_CHANNEL_DEBUGGER,
                                                            &exception_channel));
    // Notify the main thread that the exception channel is registered so it knows when to
    // start the test thread.
    {
      std::lock_guard lock(ec.m);
      ec.registered = true;
      ec.cv.notify_one();
    }
    zx_koid_t child_koid;
    // Wait for the child thread to start and tell us its KOID.
    {
      std::unique_lock lock(ct.m);
      ct.cv.wait(lock, [&ct]() { return ct.koid != 0; });
      child_koid = ct.koid;
    }

    // Read exceptions out of the exception channel until we get the first one triggered by
    // the child thread. We do this to avoid a rare race condition in which a
    // ZX_EXCP_THREAD_EXITING triggered by a thread in another test case is delivered to the
    // process exception channel after this test case starts. See https://fxbug.dev/42078955
    // for more info.
    zx_exception_info_t info;
    zx::exception exception;
    zx_info_handle_basic_t handle_info = {};
    zx::thread thread;
    while (handle_info.koid != child_koid) {
      ASSERT_OK(exception_channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
      ASSERT_OK(exception_channel.read(0, &info, exception.reset_and_get_address(), sizeof(info), 1,
                                       nullptr, nullptr));
      ASSERT_OK(exception.get_thread(&thread));
      size_t actual;
      size_t avail;
      ASSERT_OK(zx_object_get_info(thread.get(), ZX_INFO_HANDLE_BASIC, &handle_info,
                                   sizeof(handle_info), &actual, &avail));
      ASSERT_EQ(actual, 1);
      ASSERT_EQ(avail, 1);
    }

    // Starting child_thread should generate a ZX_EXCP_THREAD_STARTING message on our exception
    // channel.
    ASSERT_EQ(info.type, ZX_EXCP_THREAD_STARTING);

    uint32_t kick_options = 0;
    ASSERT_OK(restricted_machine::Machine::Kick(kick_options, thread.get()));
    // Release the exception to let the thread start the rest of the way.
    exception.reset();

    // When the thread joins, we expect to receive a ZX_EXCP_THREAD_EXITING message on our exception
    // channel.
    ASSERT_OK(exception_channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(exception_channel.read(0, &info, exception.reset_and_get_address(), sizeof(info), 1,
                                     nullptr, nullptr));
    ASSERT_EQ(info.type, ZX_EXCP_THREAD_EXITING);

    ASSERT_OK(exception.get_thread(&thread));
    // Since this thread is now in the DYING state, sending a restricted kick is expected to return
    // ZX_ERR_BAD_STATE.
    EXPECT_EQ(ZX_ERR_BAD_STATE, restricted_machine::Machine::Kick(kick_options, thread.get()));
  });

  {
    std::unique_lock lock(ec.m);
    ec.cv.wait(lock, [&ec]() { return ec.registered; });
  }

  std::thread child_thread([this]() {
    // Setup a machine for use below
    restricted_machine::Machine machine(environment());
    ASSERT_TRUE(machine.Initialize());
    machine.registers()->set_pc(0u);
    ASSERT_OK(machine.CommitState());
    // Attempting to enter restricted mode should return immediately with a kick.
    auto result = machine.Enter();
    ASSERT_OK(result);
    EXPECT_EQ(result.value(), ZX_RESTRICTED_REASON_KICK);
    machine.LogState(ZX_RESTRICTED_REASON_KICK);
  });

  // Get the KOID of the child thread and communicate it to the exception handler.
  auto child_handle = native_thread_get_zx_handle(child_thread.native_handle());
  size_t actual;
  size_t avail;
  zx_info_handle_basic_t info = {};
  ASSERT_OK(
      zx_object_get_info(child_handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &actual, &avail));
  ASSERT_EQ(actual, 1);
  ASSERT_EQ(avail, 1);

  {
    std::lock_guard lock(ct.m);
    ct.koid = info.koid;
    ct.cv.notify_one();
  }

  child_thread.join();
  exception_thread.join();
}

TEST_P(RestrictedMode, KickWhileRunning) {
  // Configure the initial register state.
  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  helper->SetInitialState(machine.registers());

  auto so_addr = environment()->SymbolAddress("store_one");
  ASSERT_OK(so_addr);
  machine.registers()->set_pc(so_addr.value());

  auto flag = environment()->MakeArgument<std::atomic_int>(0);
  machine.registers()->set_arg_regs(reinterpret_cast<uint64_t>(flag.get()), 42);

  EXPECT_OK(machine.CommitState());

  zx::unowned<zx::thread> current_thread(thrd_get_zx_handle(thrd_current()));

  // Start up a thread that will enter kick this thread once it detects that 'flag' has been
  // written to, indicating that r-mode code is running.
  std::thread kicker([&flag, &current_thread, &machine] {
    // Wait for the first thread to write to 'flag' so we know it's in restricted mode.
    while (flag->load() == 0) {
    }
    // Kick it
    uint32_t options = 0;
    ASSERT_OK(machine.Kick(options, current_thread->get()));
  });

  // Enter restricted mode and expect to tell us that it was kicked out.
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  machine.LogState(ZX_RESTRICTED_REASON_KICK);
  ASSERT_EQ(r.value(), ZX_RESTRICTED_REASON_KICK);

  kicker.join();
  EXPECT_EQ(flag->load(), 1);

  // Read the state out of the thread.
  helper->VerifyState(machine.registers());

  // Expect to see second general purpose register incremented in the observed restricted state.
  auto* state = machine.registers();
#if defined(__x86_64__)
  EXPECT_EQ(state->restricted_state().rsi, 43);
#elif defined(__aarch64__)  // defined(__x86_64__)
  EXPECT_EQ(state->restricted_state().x[1], 43);
#elif defined(__riscv)      // defined(__aarch64__)
  EXPECT_EQ(state->restricted_state().a1, 43);
#endif                      // defined(__riscv)
}

TEST_P(RestrictedMode, KickJustBeforeSyscall) {
  // Configure the initial register state.
  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  machine.enable_fpu_registers(true);
  helper->SetInitialState(machine.registers());

  auto wait_then_syscall_addr = environment()->SymbolAddress("wait_then_syscall");
  ASSERT_OK(wait_then_syscall_addr);
  machine.registers()->set_pc(wait_then_syscall_addr.value());
  // Create atomic int 'signal' and 'wait_on'
  auto wait_on = environment()->MakeArgument<std::atomic_int>(0);
  auto flag = environment()->MakeArgument<std::atomic_int>(0);
  auto signal = environment()->MakeArgument<std::atomic_int>(0);
  machine.registers()->set_arg_regs(reinterpret_cast<uint64_t>(wait_on.get()),
                                    reinterpret_cast<uint64_t>(signal.get()));
  ASSERT_OK(machine.CommitState());

  zx::unowned<zx::thread> current_thread(thrd_get_zx_handle(thrd_current()));

  std::thread kicker([&wait_on, &signal, &current_thread, &machine] {
    // Wait until the restricted mode thread is just about to issue a syscall.
    while (wait_on->load() == 0) {
    }
    // Suspend the restricted mode thread before we kick it so we can ensure that it doesn't
    // proceed before the kick is processed.
    zx::suspend_token token;
    ASSERT_OK(current_thread->suspend(&token));
    ASSERT_OK(current_thread->wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));
    // Issue a kick.
    uint32_t kick_options = 0;
    ASSERT_OK(machine.Kick(kick_options, current_thread->get()));
    // Unsuspend the thread.
    token.reset();
    // Store a signal to release the restricted mode thread so it could issue a syscall
    // if it continues executing. We expect it to come out of thread suspend and process
    // the kick instead of continuing.
    signal->store(1);
  });
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_KICK, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_KICK);
  ASSERT_EQ(ZX_RESTRICTED_REASON_KICK, r.value());
  kicker.join();
}

TEST_P(RestrictedMode, BadInstructionAbort) {
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  zx::result<uint64_t> r = machine.Thunk(0xffffffffffffffe);
  EXPECT_TRUE(r.is_ok());
  EXPECT_EQ(ZX_RESTRICTED_REASON_EXCEPTION, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_EXCEPTION);
}

// The following testing are currently only targeting ARM/ARM64.
#if defined(__aarch64__)
TEST_P(RestrictedMode, BadIncrement) {
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());

  auto addr = environment()->SymbolAddress("bad_increment");
  ASSERT_OK(addr);
  machine.registers()->set_pc(addr.value());
  ASSERT_OK(machine.CommitState());
  zx::result<uint64_t> r = machine.Enter();
  EXPECT_TRUE(r.is_ok());
  machine.LogState(ZX_RESTRICTED_REASON_EXCEPTION);
}

TEST_P(RestrictedMode, PrefetchInstructionAbort) {
  if (machine() != restricted_machine::MachineType::kArm) {
    ZXTEST_SKIP() << "kArm machine type only test";
    return;
  }
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());

  // Jump to an executable allocation at the boundary.
  zx::result<uint64_t> r = machine.Thunk(0xfffffffc, 1, 2, 3, 4);

  EXPECT_TRUE(r.is_ok());
  EXPECT_EQ(ZX_RESTRICTED_REASON_EXCEPTION, r.value());
  machine.LogState(ZX_RESTRICTED_REASON_EXCEPTION);
}

TEST_P(RestrictedMode, ExceptionRedirectRestoresTpidrroEl0) {
  if (machine() != restricted_machine::MachineType::kArm) {
    ZXTEST_SKIP() << "kArm machine type only test";
    return;
  }

  // Save the normal-mode TPIDRRO_EL0 register value before entering restricted mode.
  const uint64_t normal_tpidrro_before = __arm_rsr64("tpidrro_el0");

  auto helper = GetArchHelper();
  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());
  helper->SetInitialState(machine.registers());

  // Set restricted mode's tpidr_el0 to a distinct canary value.
  // In aarch32 restricted mode, ArchEnterRestricted mirrors state.tpidr_el0 into
  // hardware TPIDRRO_EL0 (TPIDRURO).
  constexpr uint64_t kCanaryTpidr = 0xdeadbeef12345678ULL;
  machine.registers()->restricted_state().tpidr_el0 = kCanaryTpidr;

  // Set the instruction pointer to trigger a CPU exception in restricted mode.
  auto exc_addr = environment()->SymbolAddress("exception_bounce");
  ASSERT_OK(exc_addr);
  machine.registers()->set_pc(exc_addr.value());
  EXPECT_OK(machine.CommitState());

  // Enter restricted mode and expect an exception.
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_EXCEPTION, r.value());

  // Read normal-mode TPIDRRO_EL0 register after returning to normal mode from exception redirect.
  const uint64_t normal_tpidrro_after = __arm_rsr64("tpidrro_el0");

  // Verify that the normal-mode TPIDRRO_EL0 register was restored and did not retain
  // the restricted mode value.
  EXPECT_EQ(normal_tpidrro_before, normal_tpidrro_after);
}

TEST_P(RestrictedMode, ThumbModeSyscall) {
  if (machine() != restricted_machine::MachineType::kArm) {
    ZXTEST_SKIP() << "kArm machine type only test";
    return;
  }

  auto arm_env = environment(restricted_machine::MachineType::kArm);
  ASSERT_TRUE(arm_env.is_ok());

  auto thumb_addr = arm_env.value()->SymbolAddress("thumb_syscall_bounce");
  ASSERT_OK(thumb_addr);

  // In Thumb mode, the function symbol address will have bit 0 set (interworking convention).
  EXPECT_EQ(thumb_addr.value() & 1, 1);

  restricted_machine::Machine machine(environment());
  ASSERT_TRUE(machine.Initialize());

  // Set CPSR to AArch32 Thumb mode (0x30: ARM32_BIT_MODE | ARM32_BIT_THUMB_MODE).
  constexpr uint32_t kArm32ThumbCpsr = 0x30;
  machine.registers()->restricted_state().cpsr = kArm32ThumbCpsr;
  machine.registers()->set_pc(thumb_addr.value());
  EXPECT_OK(machine.CommitState());

  // Enter restricted mode in Thumb mode and verify successful entry & syscall exit.
  zx::result<uint64_t> r = machine.Enter();
  ASSERT_OK(r);
  EXPECT_EQ(ZX_RESTRICTED_REASON_SYSCALL, r.value());

  // Verify that the syscall was issued (r7 == 64).
  EXPECT_EQ(64, machine.registers()->restricted_state().x[7]);
}
#endif  // defined(__aarch64__)
