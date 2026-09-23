// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-load-zircon-process-tests-base.h"

#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/vmo.h>
#include <lib/ld/abi.h>
#include <lib/zx/job.h>
#include <zircon/processargs.h>

#include <gtest/gtest.h>

namespace ld::testing {

const char* LdLoadZirconProcessTestsBase::process_name() const {
  return ::testing::UnitTest::GetInstance()->current_test_info()->name();
}

void LdLoadZirconProcessTestsBase::set_process(zx::process process) {
  ASSERT_FALSE(process_);
  process_ = std::move(process);
}

void LdLoadZirconProcessTestsBase::CreateProcess() {
  ASSERT_FALSE(process_);

  std::string_view name = process_name();
  ASSERT_EQ(
      zx::process::create(*zx::job::default_job(), name.data(), static_cast<uint32_t>(name.size()),
                          create_options_, &process_, &root_vmar_),
      ZX_OK);

  ASSERT_EQ(zx::thread::create(this->process(), name.data(), static_cast<uint32_t>(name.size()), 0,
                               &thread_),
            ZX_OK);

  // Set up the log pipe and stash the send side to be passed to process().
  ASSERT_NO_FATAL_FAILURE(InitLog(process_log_fd_));
}

int64_t LdLoadZirconProcessTestsBase::Wait() {
  int64_t result = -1;
  auto wait_for_termination = [process = std::exchange(process_, {}), &result]() {
    ASSERT_TRUE(process) << "Wait() called before Init()?";
    zx_info_handle_basic_t basic_info;
    ASSERT_EQ(
        process.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr),
        ZX_OK);
    zx_info_process_t info;
    ASSERT_EQ(process.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr), ZX_OK);
    ASSERT_TRUE(info.flags & ZX_INFO_PROCESS_FLAG_STARTED)
        << "process " << basic_info.koid << " not started";
    zx_signals_t signals;
    zx_status_t status = process.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), &signals);
    ASSERT_EQ(status, ZX_OK) << "process " << basic_info.koid
                             << " wait: " << zx_status_get_string(status);
    ASSERT_TRUE(signals & ZX_PROCESS_TERMINATED) << "process " << basic_info.koid;
    ASSERT_EQ(process.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr), ZX_OK);
    ASSERT_TRUE(info.flags & ZX_INFO_PROCESS_FLAG_STARTED);
    ASSERT_TRUE(info.flags & ZX_INFO_PROCESS_FLAG_EXITED);
    result = info.return_code;
  };
  wait_for_termination();

  return result;
}

zx::channel LdLoadZirconProcessTestsBase::Start(bool custom_bootstrap) {
  // Allocate the stack.  This is delayed until here in case the test uses
  // bootstrap() methods after Init() that affect bootstrap().GetStackSize().
  zx::vmo stack_vmo;
  uintptr_t sp;
  std::optional<size_t> bootstrap_stack_size = stack_size_;
  if (!bootstrap_stack_size) {
    // TODO(https://fxbug.dev/479521328): stack use too big for procargs piddly
    // default bootstrap_stack_size = bootstrap.GetStackSize();
    bootstrap_stack_size = 64 << 10;
  }

  const size_t page_size = zx_system_get_page_size();
  const size_t stack_vmo_size = (*bootstrap_stack_size + page_size - 1) & -page_size;
  const size_t stack_vmar_size = stack_vmo_size + page_size;

  EXPECT_EQ(zx::vmo::create(stack_vmo_size, 0, &stack_vmo), ZX_OK);

  zx::vmar stack_vmar;
  uintptr_t stack_vmar_base;
  EXPECT_EQ(root_vmar().allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                 0, stack_vmar_size, &stack_vmar, &stack_vmar_base),
            ZX_OK);

  zx_vaddr_t stack_base;
  EXPECT_EQ(stack_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_ALLOW_FAULTS,
                           page_size, stack_vmo, 0, stack_vmo_size, &stack_base),
            ZX_OK);

  sp = elfldltl::AbiTraits<>::InitialStackPointer(stack_base, stack_vmo_size);

  // Now that all the allocations are done, clear the address space
  // reservation so there's no such VMAR when the process starts.
  ClearVmarReservation();

  // Pack up the bootstrap message(s) and start the process running.
  zx::channel bootstrap_receiver = procargs_.MakeBootstrap();
  EXPECT_TRUE(bootstrap_receiver);
  if (!procargs_.empty()) {
    // Complete the pending message for the startup dynamic linker.  This
    // resets the procargs_ object so it can be used for the second message.
    procargs_.PackBootstrap();
  }
  if (custom_bootstrap) {
    EXPECT_THAT(procargs_deferred_, ::testing::IsEmpty());
  } else {
    // The log fd must be consumed here so as not to keep the TestPipeReader
    // alive from the other end after the process dies or otherwise drops it.
    EXPECT_TRUE(process_log_fd_);
    procargs_  //
        .AddProcess(process().borrow())
        .AddThread(thread().borrow())
        .AddAllocationVmar(root_vmar().borrow())
        .AddStackVmo(std::move(stack_vmo))
        .AddFd(STDERR_FILENO, std::move(process_log_fd_))
        .SetArgs(argv())
        .SetEnv(envp());
    for (auto& f : std::exchange(procargs_deferred_, {})) {
      std::move(f)(procargs_);
    }
    procargs_.PackBootstrap();
  }

  EXPECT_EQ(this->process().start(thread(), entry_, sp, std::move(bootstrap_receiver), vdso_base_),
            ZX_OK);

  return std::move(procargs_.bootstrap_sender());
}

void LdLoadZirconProcessTestsBase::NeverStart() {
  process_log_fd_.reset();  // Never used when no bootstrap message (Start).
  thread_.reset();          // Never used when the process is never started.

  // The root_vmar_ and process_ are still used just to populate and examine
  // the unstarted process with no threads in it.
  EXPECT_TRUE(process_);
  ASSERT_TRUE(root_vmar_);
  CheckVmar();

  // Remove this before examination as before start.
  ClearVmarReservation();
}

void LdLoadZirconProcessTestsBase::ClearVmarReservation() { reserve_vmar_ = {}; }

zx_info_vmar_t LdLoadZirconProcessTestsBase::RootVmarInfo() const {
  EXPECT_TRUE(root_vmar_);
  zx_info_vmar_t info;
  zx_status_t status = root_vmar_.get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
  if (status != ZX_OK) {
    info = {};
  }
  return info;
}

void LdLoadZirconProcessTestsBase::InitVmarReservation(zx_info_vmar_t bounds) {
  ASSERT_FALSE(reserve_vmar_) << "called twice??";

  zx_info_vmar_t info = RootVmarInfo();
  auto res = reserve_vmar_.Init(root_vmar_.borrow(), info, bounds);

  ASSERT_TRUE(res.is_ok()) << "zx_vmar_allocate " << std::hex << std::showbase << bounds.len
                           << " at 0 " << zx_status_get_string(res.status_value())
                           << " vs root base=" << info.base << " len=" << info.len;
  ASSERT_TRUE(reserve_vmar_);
}

int64_t LdLoadZirconProcessTestsBase::Run() {
  Start(false);
  return ::testing::Test::HasFatalFailure() ? -1 : Wait();
}

std::pair<int64_t, zx::channel> LdLoadZirconProcessTestsBase::RunWithCustomBootstrap() {
  // Don't keep the TestPipeReader alive from the other end.
  process_log_fd().reset();

  zx::channel bootstrap_sender = Start(true);
  int64_t exit_code = ::testing::Test::HasFatalFailure() ? -1 : Wait();
  return {exit_code, std::move(bootstrap_sender)};
}

LdLoadZirconProcessTestsBase::~LdLoadZirconProcessTestsBase() {
  if (process_) {
    EXPECT_EQ(process_.kill(), ZX_OK);
  }
}

void LdLoadZirconProcessTestsBase::CheckProcess() {
  ASSERT_TRUE(process_);
  zx_signals_t pending = 0;
  zx_status_t status =
      process_.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite_past(), &pending);
  if (status == ZX_OK) {
    if (pending & ZX_PROCESS_TERMINATED) {
      ADD_FAILURE() << "process died with exit_code " << Wait();
    }
  } else {
    ASSERT_EQ(status, ZX_ERR_TIMED_OUT) << zx_status_get_string(status);
  }
}

void LdLoadZirconProcessTestsBase::CheckVmar() {
  zx_status_t status = root_vmar_.get_info(ZX_INFO_VMAR_MAPS, nullptr, 0, nullptr, nullptr);
  if (status != ZX_ERR_BUFFER_TOO_SMALL) {
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }
}

void LdLoadZirconProcessTestsBase::RedirectFd(int target_number, fbl::unique_fd transfer_fd) {
  // This is for the second message, not the first.  So defer doing the work
  // until that message is being packed.
  procargs_deferred_.emplace_back(
      [target_number, fd = std::move(transfer_fd)](TestProcessArgs& procargs) mutable {
        procargs.AddFd(target_number, std::move(fd));
      });
}

}  // namespace ld::testing
