// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_LOAD_ZIRCON_PROCESS_TESTS_BASE_H_
#define LIB_LD_TEST_LD_LOAD_ZIRCON_PROCESS_TESTS_BASE_H_

#include <lib/elfldltl/soname.h>
#include <lib/fit/function.h>
#include <lib/ld/testing/test-processargs.h>
#include <lib/ld/vmar.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>

#include <vector>

#include "ld-load-zircon-ldsvc-tests-base.h"

namespace ld::testing {

// This is the common base class for test fixtures to launch a Zircon process.
class LdLoadZirconProcessTestsBase : public LdLoadZirconLdsvcTestsBase {
 public:
  // The Fuchsia test executables (via modules/zircon-test-start.cc) link
  // directly to the vDSO, so it appears before other modules.
  static constexpr std::optional<elfldltl::Soname<>> kTestExecutableNeedsVdso{
      "libzircon.so",
  };

  static constexpr int64_t kRunFailureForTrap = ZX_TASK_RETCODE_EXCEPTION_KILL;
  static constexpr int64_t kRunFailureForBadPointer = ZX_TASK_RETCODE_EXCEPTION_KILL;

  ~LdLoadZirconProcessTestsBase();

  const char* process_name() const;

  // This just folds together Start() and Wait(), below.
  int64_t Run();

  // Like Run(), but tells Start() not to send the standard "main" bootstrap
  // message (only, possibly, the startup dynamic linker message).  Instead the
  // sender side of the bootstrap channel is returned.
  std::pair<int64_t, zx::channel> RunWithCustomBootstrap();

  // This drops references that won't be used if neither Start() nor Run() is
  // ever called.
  void NeverStart();

  // Assert that root_vmar() is valid and not destroyed.
  void CheckVmar();

  // Assert that the process is valid and not terminated.
  void CheckProcess();

  // Use transfer_fd as the child's target_number fd in the main (libc)
  // bootstrap message.  STDERR_FILENO is already implicitly redirected to
  // process_log_fd() before this is applied.
  void RedirectFd(int target_number, fbl::unique_fd transfer_fd);

  [[gnu::const]] zx_info_vmar_t RootVmarInfo() const;

 protected:
  const zx::process& process() const { return process_; }

  // A subclass calls this when not using CreateProcess().
  void set_process(zx::process process);

  // A subclass can optionally call this before calling CreateProcess().
  void set_create_options(uint32_t options) { create_options_ = options; }

  // A subclass calls CreateProcess() to set process(), root_vmar(), and thread().
  void CreateProcess();

  // This does the actual initialization of the `reserve_vmar_`.
  void InitVmarReservation(zx_info_vmar_t bounds);

  // These are set by CreateProcess() and used by Start() and Run().
  const zx::vmar& root_vmar() { return root_vmar_; }
  const zx::thread& thread() { return thread_; }

  // This is set by CreateProcess() and ultimately consumed by Start(false),
  // but left intact by Start(true).
  fbl::unique_fd& process_log_fd() { return process_log_fd_; }

  // This is used by Start() and Run().  If it's not empty() when they're
  // called, its pending startup dynamic linker message gets packed and sent.
  TestProcessArgs& bootstrap() { return procargs_; }

  // These are used by Start() and Run().  The subclass must set them first.
  uintptr_t entry() const { return entry_; }
  uintptr_t vdso_base() const { return vdso_base_; }
  std::optional<size_t> stack_size() const { return stack_size_; }
  void set_entry(uintptr_t entry) { entry_ = entry; }
  void set_vdso_base(uintptr_t vdso_base) { vdso_base_ = vdso_base; }
  void set_stack_size(std::optional<size_t> stack_size) { stack_size_ = stack_size; }

  // Start the process() using all those parameters.  If bootstrap() has a
  // pending message being built, that's completed and sent as the startup
  // dynamic linker's message.  Then, if custom_bootstrap is false, a standard
  // "main" (libc) message is sent.  Finally, the sender side of the bootstrap
  // channel is returned in case the test wants to either send its own messages
  // or read replies from the test module.
  zx::channel Start(bool custom_bootstrap);

  // Wait for the process to die and collect its exit code.
  // This clears the process() so a new one can be installed.
  int64_t Wait();

 private:
  void ClearVmarReservation();

  uint32_t create_options_ = 0;
  zx::process process_;

  // Not all subclasses use these.
  fbl::unique_fd process_log_fd_;
  zx::vmar root_vmar_;
  ld::VmarReservation reserve_vmar_;
  zx::thread thread_;
  TestProcessArgs procargs_;
  std::vector<fit::function<void(TestProcessArgs&)>> procargs_deferred_;
  uintptr_t entry_ = 0;
  uintptr_t vdso_base_ = 0;
  std::optional<size_t> stack_size_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_LOAD_ZIRCON_PROCESS_TESTS_BASE_H_
