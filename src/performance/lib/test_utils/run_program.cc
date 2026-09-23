// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/lib/test_utils/run_program.h"

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include "src/lib/files/file.h"
#include "src/lib/fsl/types/type_converters.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace tracing {
namespace test {

void AppendLoggingArgs(std::vector<std::string>* argv, const char* prefix,
                       const fxl::LogSettings& log_settings) {
  // Transfer our log settings to the subprogram.
  std::string log_file_arg;
  std::string verbose_or_quiet_arg;
  if (log_settings.min_log_level != 0) {
    if (log_settings.min_log_level < 0) {
      verbose_or_quiet_arg =
          fxl::StringPrintf("%s--verbose=%d", prefix, -log_settings.min_log_level);
    } else {
      verbose_or_quiet_arg = fxl::StringPrintf("%s--quiet=%d", prefix, log_settings.min_log_level);
    }
    argv->push_back(verbose_or_quiet_arg);
  }
}

static void StringArgvToCArgv(const std::vector<std::string>& argv,
                              std::vector<const char*>* c_argv) {
  for (const auto& arg : argv) {
    c_argv->push_back(arg.c_str());
  }
  c_argv->push_back(nullptr);
}

zx_status_t SpawnProgram(const zx::job& job, const std::vector<std::string>& argv,
                         zx_handle_t arg_handle, zx::process* out_process) {
  size_t num_actions = 0;
  fdio_spawn_action_t spawn_actions[1];

  if (arg_handle != ZX_HANDLE_INVALID) {
    spawn_actions[num_actions].action = FDIO_SPAWN_ACTION_ADD_HANDLE;
    spawn_actions[num_actions].h.id = PA_HND(PA_USER0, 0);
    spawn_actions[num_actions].h.handle = arg_handle;
    ++num_actions;
  }

  return RunProgram(job, argv, num_actions, spawn_actions, out_process);
}

zx_status_t RunProgram(const zx::job& job, const std::vector<std::string>& argv, size_t num_actions,
                       const fdio_spawn_action_t* actions, zx::process* out_process) {
  std::vector<const char*> c_argv;
  StringArgvToCArgv(argv, &c_argv);

  FX_LOGS(INFO) << "Running " << fxl::JoinStrings(argv, " ");

  char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
  zx_status_t status =
      fdio_spawn_etc(job.get(), FDIO_SPAWN_CLONE_ALL, c_argv[0], c_argv.data(), nullptr,
                     num_actions, actions, out_process->reset_and_get_address(), err_msg);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Spawning " << c_argv[0] << " failed: " << err_msg;
    return status;
  }

  return ZX_OK;
}

bool WaitAndGetReturnCode(const std::string& program_name, const zx::process& process,
                          int64_t* out_return_code) {
  // Leave it to the test harness to provide a timeout. If it doesn't that's
  // its bug.
  auto status = process.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed waiting for program " << program_name << " to exit";
    return false;
  }

  zx_info_process_t proc_info;
  status = zx_object_get_info(process.get(), ZX_INFO_PROCESS, &proc_info, sizeof(proc_info),
                              nullptr, nullptr);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Error getting return code for program " << program_name;
    return false;
  }

  if (proc_info.return_code != 0) {
    FX_LOGS(INFO) << program_name << " exited with return code " << proc_info.return_code;
  }
  *out_return_code = proc_info.return_code;
  return true;
}

bool RunProgramAndWait(const zx::job& job, const std::vector<std::string>& argv, size_t num_actions,
                       const fdio_spawn_action_t* actions) {
  zx::process subprocess;

  auto status = RunProgram(job, argv, num_actions, actions, &subprocess);
  if (status != ZX_OK) {
    return false;
  }

  int64_t return_code;
  if (!WaitAndGetReturnCode(argv[0], subprocess, &return_code)) {
    return false;
  }
  if (return_code != 0) {
    FX_LOGS(ERROR) << argv[0] << " exited with return code " << return_code;
    return false;
  }

  return true;
}

}  // namespace test
}  // namespace tracing
