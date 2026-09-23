// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-startup-spawn-process-tests-posix.h"

#include <fcntl.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <signal.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstring>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

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

// The nicer thing to use is posix_spawn, and that's a closer parallel with
// fdio_spawn that is used on Fuchsia.  However, the POSIX standard API doesn't
// have the features like addfchdir_np and it's nicer to only change that state
// in the child rather than also in the parent.  Newer glibc (Linux) versions
// support extensions that are sufficient for the needs:
//  * 2.29 added posix_spawn_file_actions_addfchdir_np.
//  * 2.34 added posix_spawn_file_actions_addclosefrom_np
#if defined(__GLIBC_PREREQ) && __GLIBC_PREREQ(2, 34)

// The SpawnPlan object collects actions to be applied by posix_spawn.
// All the fd's passed to it must remain live until posix_spawn is called.
class SpawnPlan {
 public:
  SpawnPlan() {
    int error = posix_spawn_file_actions_init(&actions_);
    EXPECT_EQ(error, 0) << strerror(error);
  }

  void Fchdir(int fd) {
    int error = posix_spawn_file_actions_addfchdir_np(&actions_, fd);
    EXPECT_EQ(error, 0) << strerror(error);
  }

  void Dup2(int from, int to) {
    int error = posix_spawn_file_actions_adddup2(&actions_, from, to);
    EXPECT_EQ(error, 0) << strerror(error);
  }

  void Closefrom(int from) {
    int error = posix_spawn_file_actions_addclosefrom_np(&actions_, from);
    EXPECT_EQ(error, 0) << strerror(error);
  }

  ~SpawnPlan() {
    int error = posix_spawn_file_actions_destroy(&actions_);
    EXPECT_EQ(error, 0) << strerror(error);
  }

  pid_t Launch(const std::string& executable, const std::vector<std::string>& argv) {
    pid_t pid = -1;
    int error =
        posix_spawn(&pid, executable.c_str(), &actions_, nullptr, ArgvPtrs(argv).data(), nullptr);
    EXPECT_EQ(error, 0) << strerror(error);
    return pid;
  }

 private:
  posix_spawn_file_actions_t actions_;
};

#else

// Support the same abstracted version of the posix_spawn_file_actions_t API
// but implemented directly using fork and execve.
class SpawnPlan {
 public:
  void Fchdir(int fd) { fchdir_ = fd; }

  void Dup2(int from, int to) { dup2_.emplace_back(from, to); }

  void Closefrom(int from) {
    // TODO(https://fxbug.dev/479524197): Newer glibc versions have the
    // closefrom() call, but not the build's current sysroot version. Rather
    // than do the potentially costly technique of looping up to
    // getdtablesize(), just allow for some descriptors to leak, and only close
    // the ones we know about.  This is really just extra surety anyway, since
    // ideally all the extra fds that were opened had FD_CLOEXEC set.
    for (auto [dup_from, dup_to] : dup2_) {
      EXPECT_GE(dup_from, from);
      auto is_dup_from = [dup_from = dup_from](const auto& dup) -> bool {
        return dup.first == dup_from;
      };
      if (std::none_of(dup2_.begin(), dup2_.end(), is_dup_from)) {
        EXPECT_EQ(close(dup_from), 0) << "close(" << dup_from << "): " << strerror(errno);
      }
    }
  }

  pid_t Launch(const std::string& executable, const std::vector<std::string>& argv,
               const std::vector<std::string>& envp) {
    fflush(nullptr);  // Flush buffers before fork duplicates them.
    pid_t pid = fork();
    if (pid == 0) {
      Child(executable, argv, envp);
    }
    EXPECT_GT(pid, 0) << "fork: " << strerror(errno);
    return pid;
  }

 private:
  [[noreturn]] void Child(const std::string& executable, const std::vector<std::string>& argv,
                          const std::vector<std::string>& envp) {
    // Child side.  Install state before exec.
    if (fchdir_ >= 0) {
      EXPECT_EQ(fchdir(fchdir_), 0) << "fchdir: " << strerror(errno);
    }

    for (auto [from, to] : dup2_) {
      if (from != to) {
        EXPECT_EQ(dup2(from, to), to) << "dup2(" << from << ", " << to << "): " << strerror(errno);
      }
    };

    if (!::testing::Test::HasFailure()) {
      EXPECT_EQ(execve(executable.c_str(), ArgvPtrs(argv).data(), ArgvPtrs(envp).data()), 0)
          << "execve: " << executable << ": " << strerror(errno);
    }
    fflush(nullptr);
    _exit(127);
  }

  int fchdir_ = -1;
  std::vector<std::pair<int, int>> dup2_;
};

#endif

}  // namespace

void LdStartupSpawnProcessTests::Init(std::initializer_list<std::string_view> args,
                                      std::initializer_list<std::string_view> env) {
  argv_ = std::vector<std::string>{args.begin(), args.end()};
  envp_ = std::vector<std::string>{env.begin(), env.end()};
}

void LdStartupSpawnProcessTests::Load(std::string_view executable_name) {
  ASSERT_NO_FATAL_FAILURE(LoadTestDir(executable_name));
  executable_ = executable_name;
  ASSERT_NO_FATAL_FAILURE(CheckNeededLibs());
}

int64_t LdStartupSpawnProcessTests::Run() {
  SpawnPlan spawn;

  // Change into the directory where all the test ELF files can be found.
  spawn.Fchdir(test_dir());

  // Put /dev/null on stdin and stdout.  They should not be used.
  fbl::unique_fd null_fd{open("/dev/null", O_RDWR, O_CLOEXEC)};
  EXPECT_TRUE(null_fd) << "/dev/null: " << strerror(errno);
  fbl::unique_fd log_fd;
  spawn.Dup2(null_fd.get(), STDIN_FILENO);
  spawn.Dup2(null_fd.get(), STDOUT_FILENO);

  // Put the log pipe on stderr to collect any diagnostics.
  InitLog(log_fd);
  spawn.Dup2(log_fd.get(), STDERR_FILENO);

  // Close all other fd's in case any were opened without CLOEXEC.
  spawn.Closefrom(STDERR_FILENO + 1);

  if (HasFailure()) {
    return -1;
  }

  // Launch the child.
  pid_ = spawn.Launch(executable_, argv_, envp_);
  if (HasFailure()) {
    return -1;
  }

  // Wait for it to die.
  int status;
  pid_t waited = waitpid(pid_, &status, 0);
  EXPECT_EQ(waited, pid_) << strerror(errno);
  if (waited != pid_) {
    return -1;
  }

  // Return an exit code or termination signal (+ 128, in the style of sh).
  EXPECT_FALSE(WIFSTOPPED(status)) << strsignal(WSTOPSIG(status));
  if (WIFSIGNALED(status)) {
    return 128 + WTERMSIG(status);
  }
  EXPECT_TRUE(WIFEXITED(status));
  return WEXITSTATUS(status);
}

LdStartupSpawnProcessTests::~LdStartupSpawnProcessTests() {
  if (pid_ > 0) {
    if (kill(pid_, SIGKILL) < 0) {
      EXPECT_EQ(errno, ESRCH) << strerror(errno);
    }
  }
}

}  // namespace ld::testing
