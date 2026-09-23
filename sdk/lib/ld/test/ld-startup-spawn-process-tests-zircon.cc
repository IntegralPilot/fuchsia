// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-startup-spawn-process-tests-zircon.h"

#include <lib/elfldltl/load.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/vmo.h>
#include <lib/fdio/spawn.h>
#include <lib/ld/testing/get-test-vmo.h>
#include <lib/zx/job.h>
#include <unistd.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <cstring>
#include <filesystem>
#include <vector>

#include <gtest/gtest.h>

#include "load-tests.h"

namespace ld::testing {
namespace {

// Pack up a nullptr-terminated array of the argument pointers.
std::vector<char*> ArgvPtrs(const std::vector<std::string>& argv) {
  std::vector<char*> argv_ptrs;
  argv_ptrs.reserve(argv.size() + 1);
  for (const std::string& arg : argv) {
    argv_ptrs.push_back(const_cast<char*>(arg.c_str()));
  }
  argv_ptrs.push_back(nullptr);
  return argv_ptrs;
}

// The SpawnPlan object collects actions to be applied by fdio_spawn.
class SpawnPlan {
 public:
  void Name(const char* name) {
    actions_.push_back({.action = FDIO_SPAWN_ACTION_SET_NAME, .name = {name}});
  }

  void Dup2(fbl::unique_fd from, int to) {
    actions_.push_back({.action = FDIO_SPAWN_ACTION_TRANSFER_FD,
                        .fd = {.local_fd = from.release(), .target_fd = to}});
  }

  void AddLdsvc(zx::channel client_end) {
    AddHandle(PA_LDSVC_LOADER, zx::handle{client_end.release()});
  }

  zx::process Launch(zx::vmo executable_vmo, const std::vector<std::string>& argv,
                     const std::vector<std::string>& envp) {
    EXPECT_TRUE(executable_vmo);
    zx::process process;
    char error[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
    zx_status_t status =
        fdio_spawn_vmo(zx::job::default_job()->get(), FDIO_SPAWN_CLONE_UTC_CLOCK,
                       executable_vmo.release(), ArgvPtrs(argv).data(), ArgvPtrs(envp).data(),
                       actions_.size(), actions_.data(), process.reset_and_get_address(), error);
    actions_.clear();  // That consumed the fds and handles even if it failed.
    EXPECT_EQ(status, ZX_OK) << error << ": " << zx_status_get_string(status);
    return process;
  }

  ~SpawnPlan() {
    for (const fdio_spawn_action_t& action : actions_) {
      switch (action.action) {
        case FDIO_SPAWN_ACTION_TRANSFER_FD:
          EXPECT_EQ(close(action.fd.local_fd), 0)
              << "close(" << action.fd.local_fd << "): " << strerror(errno);
          break;
        case FDIO_SPAWN_ACTION_ADD_HANDLE: {
          zx_status_t status = zx_handle_close(action.h.handle);
          EXPECT_EQ(status, ZX_OK)
              << "Bad handle for " << action.h.id << ": " << zx_status_get_string(status);
          break;
        }
        default:
          ADD_FAILURE() << "who added action type " << action.action << "???";
          break;
      }
    }
  }

 private:
  void AddHandle(uint32_t id, zx::handle handle) {
    ASSERT_TRUE(handle);
    actions_.push_back({
        .action = FDIO_SPAWN_ACTION_ADD_HANDLE,
        .h = {.id = id, .handle = handle.release()},
    });
  }

  std::vector<fdio_spawn_action_t> actions_;
};

}  // namespace

LdStartupSpawnProcessTests::~LdStartupSpawnProcessTests() = default;

void LdStartupSpawnProcessTests::Load(std::string_view executable_name,
                                      std::optional<std::string_view> expected_config) {
  // This points GetLibVmo() to the right place.
  LdsvcPathPrefix(executable_name);

  ASSERT_NO_FATAL_FAILURE(executable_ = GetExecutableVmo(executable_name));

  // The program launcher service first uses the loader service channel to look
  // up the PT_INTERP and then transfers it to the new process to be used by
  // the dynamic linker.  So inject an expectation before any from the test
  // itself calling Needed.
  std::string interp;
  ASSERT_NO_FATAL_FAILURE(interp = FindInterp(executable_.borrow()));
  if (!interp.empty() && !expected_config) {
    ASSERT_NO_FATAL_FAILURE(LdsvcExpectDependency(interp));
  }
  LdsvcExpectConfig(ConfigFromInterp(interp, expected_config));

  // Prime the mock loader service from the Needed() calls.
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());
}

int64_t LdStartupSpawnProcessTests::Run() {
  SpawnPlan spawn;

  spawn.Name(process_name());

  // Pass in the mock loader service channel.
  spawn.AddLdsvc(TakeLdsvc());

  // Put the log pipe on stderr to collect any diagnostics.
  fbl::unique_fd log_fd;
  InitLog(log_fd);
  spawn.Dup2(std::move(log_fd), STDERR_FILENO);

  if (HasFailure()) {
    return -1;
  }

  // Launch the child and save the process handle.
  set_process(spawn.Launch(std::move(executable_), argv(), envp()));
  if (HasFailure()) {
    return -1;
  }

  return Wait();
}

}  // namespace ld::testing
