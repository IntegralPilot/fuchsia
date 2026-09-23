// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/e2e_tests/ffx_debug_agent_bridge.h"

#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#include <csignal>
#include <cstdlib>
#include <filesystem>
#include <system_error>
#include <vector>

#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/common/inet_util.h"

namespace zxdb {

namespace {

constexpr std::string_view kFuchsiaDeviceSshAddr = "FUCHSIA_DEVICE_ADDR";
constexpr std::string_view kFuchsiaDeviceSshPort = "FUCHSIA_SSH_PORT";
constexpr std::string_view kFuchsiaSshKey = "FUCHSIA_SSH_KEY";
constexpr std::string_view kTestOutDir = "FUCHSIA_TEST_OUTDIR";
constexpr std::string_view kFfxIsolateDir = "zxdb_e2e_tests_ffx_isolate_dir";
constexpr std::string_view kFfxCommonConfig =
    "log.level=debug,ffx.isolated=true,fastboot.usb.disabled=true,discovery.mdns.enabled=false";

// This is atomic so it can be used in the signal handler below.
std::atomic<FfxDebugAgentBridge*> global_instance = nullptr;

// When running tests locally, we need to cleanup the child process and the ffx isolate carefully if
// the user ctrl+c's the tests. The child process can be cleaned up with async-signal-safe
// functions, and then we exit the parent process immediately. Note: we cannot post to the message
// loop here, since PostTask will end up taking a lock and may create a deadlock.
void OnSigInt(int /*signum*/, siginfo_t* /*info*/, void* /*ptr*/) {
  auto bridge = global_instance.load();
  if (bridge)
    bridge->Cleanup();

  exit(EXIT_FAILURE);
}

Err KillProcessWithSignal(pid_t pid, int signal) {
  if (kill(pid, signal) != 0) {
    const std::string s(strerror(errno));
    return Err("Failed to send signal " + std::to_string(signal) + " to child process: " + s);
  }

  int status;
  const int wait_pid = wait(&status);

  if (wait_pid == -1) {
    const std::string s(strerror(errno));
    return Err("Failed while waiting for child to terminate: " + s);
  }

  // This should be the normal case.
  if (WIFEXITED(status)) {
    return Err();
  } else if (WIFSIGNALED(status)) {
    int sig = WTERMSIG(status);
    FX_LOGS(WARNING) << "Child forced to terminate (with signal " << sig << ": "
                     << std::string{strsignal(sig)} << ").";
    return Err();
  }

  return Err("Child exited due to an unexpected signal (" + std::string{strsignal(status)} +
             "), this is likely a bug.");
}

std::vector<char*> GetFfxArgV(const std::filesystem::path& ffx_test_data_path,
                              const std::string& isolate_dir) {
  std::vector<char*> ffx_args = {const_cast<char*>("ffx")};

  // In infra, this environment variable is populated with the device that's been assigned to the
  // infra bot. Locally, a user can also set this to point to a specific device if they choose, but
  // `fx set-device` will also work just as well.
  std::string device_addr = std::getenv(kFuchsiaDeviceSshAddr.data());
  if (!device_addr.empty()) {
    ffx_args.push_back(const_cast<char*>("--target"));
    if (auto port = std::getenv(kFuchsiaDeviceSshPort.data()); port != nullptr) {
      device_addr.push_back(':');
      device_addr.append(port);
    }
    ffx_args.push_back(strdup(device_addr.c_str()));
  }
  ffx_args.push_back(const_cast<char*>("--config"));
  std::string ffx_config_arg(kFfxCommonConfig);
  ffx_config_arg.append(",ffx.subtool-search-paths=");
  ffx_config_arg.append(ffx_test_data_path);
  char* test_outdir = std::getenv(kTestOutDir.data());
  if (test_outdir) {
    ffx_config_arg.append(const_cast<char*>(",log.dir="));
    ffx_config_arg.append(test_outdir);
  }
  ffx_args.push_back(strdup(ffx_config_arg.c_str()));

  ffx_args.push_back(const_cast<char*>("--isolate-dir"));
  ffx_args.push_back(strdup(isolate_dir.c_str()));
  ffx_args.push_back(const_cast<char*>("debug"));
  ffx_args.push_back(const_cast<char*>("connect"));
  ffx_args.push_back(const_cast<char*>("--agent-only"));
  ffx_args.push_back(const_cast<char*>("--new-agent"));
  ffx_args.push_back(nullptr);  // argv must be null-terminated

  return ffx_args;
}

// The environment variable |kFuchsiaSshKey| needs to be a full path for FFX to properly resolve
// the file, but in infra, it's set to a relative path. This function expands the environment
// variable to the full path to the ssh key file, if it exists. Other environment variables are
// copied. The returned strings must be freed properly.
std::vector<char*> GetFfxEnv() {
  char** env = GetEnviron();
  std::vector<char*> new_env = {};

  // Duplicate the strings in the parent environment for us to manage in the child process. The
  // ownership of these strings is kept by the class and they are deallocated when this object goes
  // out of scope.
  for (size_t i = 0; env[i] != nullptr; i++) {
    // Do not duplicate |kFuchsiaSshKey| because we're going to modify it before putting it
    // back into place later.
    if (strstr(env[i], kFuchsiaSshKey.data()) == nullptr) {
      new_env.push_back(strdup(env[i]));
    }
  }

  if (char* ssh_key_path_str = std::getenv(kFuchsiaSshKey.data())) {
    std::string ssh_key_env_var{kFuchsiaSshKey};
    ssh_key_env_var.append("=");
    ssh_key_env_var.append(std::filesystem::absolute(ssh_key_path_str).string());

    new_env.push_back(strdup(ssh_key_env_var.data()));
  }

  new_env.push_back(nullptr);
  return new_env;
}

}  // namespace

Err FfxDebugAgentBridge::Init() {
  struct sigaction sa;
  sa.sa_sigaction = OnSigInt;
  sa.sa_flags = SA_SIGINFO;

  // Handle ctrl+c.
  sigaction(SIGINT, &sa, nullptr);

  ffx_isolate_dir_ = std::filesystem::temp_directory_path() / kFfxIsolateDir;
  std::error_code ec;

  // If the isolate directory already exists, we either failed to cleanup from a prior run (could
  // have been ctrl+c'd if running locally) or had been forcibly killed which would leave a zombie
  // process anyway. Cleaning up the directory will stop the isolate running.
  if (std::filesystem::exists(ffx_isolate_dir_)) {
    if (std::filesystem::remove_all(ffx_isolate_dir_, ec); ec) {
      FX_LOGS(WARNING) << ffx_isolate_dir_ << " exists, but could not be deleted: " << ec.message();
    }
  }

  if (!std::filesystem::create_directory(ffx_isolate_dir_, ec)) {
    return Err("could not create FFX isolate directory: %s", ec.message().c_str());
  }

  Err e = SetupPipeAndFork();
  if (e.has_error()) {
    return e;
  }

  return ReadDebugAgentSocketPath();
}

FfxDebugAgentBridge::FfxDebugAgentBridge() {
  FX_CHECK(!global_instance.load());
  global_instance.store(this);
}

FfxDebugAgentBridge::~FfxDebugAgentBridge() { Cleanup(); }

FfxDebugAgentBridge* FfxDebugAgentBridge::Get() { return global_instance.load(); }

Err FfxDebugAgentBridge::SetupPipeAndFork() {
  int p[2];

  if (pipe(p) < 0) {
    const std::string s(strerror(errno));
    return Err("Could not create pipe: " + s);
  }

  pipe_read_end_ = p[0];
  pipe_write_end_ = p[1];

  const pid_t child_pid = fork();

  if (child_pid == 0) {
    close(pipe_read_end_);

    const std::filesystem::path me(GetSelfPath());

    // In variant builds that put the test executable in a different directory (potentially
    // something like out/default/host_x64-asan/...), ffx could be in a different directory than the
    // test executable.
    const std::filesystem::path ffx_test_data =
        me.parent_path().parent_path() / "host_x64" / ZXDB_E2E_TESTS_FFX_TEST_DATA;
    const std::filesystem::path ffx_path = ffx_test_data / "ffx";

    if (!std::filesystem::exists(ffx_path)) {
      FX_LOGS(ERROR) << "Could not locate ffx binary at " << std::filesystem::absolute(ffx_path);
      FX_LOGS(ERROR) << "Note: zxdb_e2e_tests binary is at " << std::filesystem::absolute(me);
      exit(EXIT_FAILURE);
    }

    // |pipe_write_end_| will be closed along with stdout when the child program
    // terminates.
    if (dup2(pipe_write_end_, STDOUT_FILENO) < 0) {
      FX_LOGS(ERROR) << "Failed to dup child stdout to pipe write end: " << strerror(errno);
      exit(EXIT_FAILURE);
    }

    execve(std::filesystem::absolute(ffx_path).c_str(),
           GetFfxArgV(ffx_test_data, ffx_isolate_dir_).data(), GetFfxEnv().data());

    FX_NOTREACHED();
  } else {
    close(pipe_write_end_);
    child_pid_ = child_pid;
  }

  return Err();
}

void FfxDebugAgentBridge::Cleanup() {
  if (global_instance.load()) {
    FX_CHECK(global_instance.load() == this);

    socket_path_.clear();

    if (pipe_read_end_ != 0) {
      close(pipe_read_end_);
    }

    if (child_pid_ != 0) {
      Err e = CleanupChild();
      if (e.has_error()) {
        FX_LOGS(ERROR) << "Error encountered while cleaning up child: " << e.msg();
      }
    }

    // Remove the isolate directory, which implicitly stops the isolate's daemon to complete
    // cleanup. This should be signal safe, since there is no state handled by
    // `std::filesystem::remove_all`.
    std::error_code ec;
    std::filesystem::remove_all(ffx_isolate_dir_, ec);
    if (ec) {
      FX_LOGS(ERROR) << "Failed to remove FFX isolate directory: " << ec.message();
    }

    FX_CHECK(global_instance.load() == this);
    global_instance.store(nullptr);
  }
}

Err FfxDebugAgentBridge::ReadDebugAgentSocketPath() {
  FILE* child_stdout = fdopen(pipe_read_end_, "r");
  if (child_stdout == nullptr) {
    const std::string s(strerror(errno));
    return Err("Failed to open pipe_read_end_ fd as FILE*: " + s);
  }

  char c;
  size_t bytes_read = fread(&c, 1, 1, child_stdout);
  while (c != '\n') {
    if (bytes_read == 0) {
      Err e;
      if (int err = feof(child_stdout); err != 0) {
        e = Err("Unexpected EOF while reading stdout from child process " + std::to_string(err));
      } else if (int err = ferror(child_stdout); err != 0) {
        e = Err("Unexpected error while reading stdout from child process " + std::to_string(err));
      } else {
        e = Err("Unknown error occurred while reading from child process, got 0 bytes from fread");
      }
      return e;
    }
    socket_path_.push_back(c);
    bytes_read = fread(&c, 1, 1, child_stdout);
  }

  fclose(child_stdout);

  // Now check to make sure this is actually a path.
  std::filesystem::path ffx_path(socket_path_);
  if (!std::filesystem::exists(ffx_path)) {
    return Err("Output of \"ffx debug connect --agent-only\" is not a valid path: " + socket_path_);
  }

  return Err();
}

Err FfxDebugAgentBridge::CleanupChild() const {
  Err e = KillProcessWithSignal(child_pid_, SIGTERM);
  if (e.has_error()) {
    FX_LOGS(WARNING) << "Failed to kill child [" << child_pid_ << "] with SIGTERM, trying SIGKILL.";
    e = KillProcessWithSignal(child_pid_, SIGKILL);
    if (e.has_error()) {
      FX_LOGS(ERROR) << "Failed to kill child with SIGKILL. There is a zombie process with pid "
                     << child_pid_;
    }
  }

  return e;
}

}  // namespace zxdb
