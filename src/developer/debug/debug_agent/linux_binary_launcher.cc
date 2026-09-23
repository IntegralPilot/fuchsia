// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_binary_launcher.h"

#include <fcntl.h>
#include <sys/ptrace.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
#include <zircon/errors.h>

#include "src/developer/debug/debug_agent/linux_process_handle.h"
#include "src/developer/debug/debug_agent/linux_task.h"
#include "src/developer/debug/debug_agent/linux_utils.h"
#include "src/developer/debug/debug_agent/posix/dir_reader_linux.h"
#include "src/developer/debug/debug_agent/posix/eintr_wrapper.h"
#include "src/developer/debug/debug_agent/posix/file_descriptor_shuffle.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace debug_agent {

namespace {

static const rlim_t kSystemDefaultMaxFds = 8192;
static const char kFDDir[] = "/proc/self/fd";

size_t GetMaxFds() {
  rlim_t max_fds;
  struct rlimit nofile;
  if (getrlimit(RLIMIT_NOFILE, &nofile)) {
    // getrlimit failed. Take a best guess.
    max_fds = kSystemDefaultMaxFds;
  } else {
    max_fds = nofile.rlim_cur;
  }

  if (max_fds > INT_MAX)
    max_fds = INT_MAX;

  return static_cast<size_t>(max_fds);
}

void CloseSuperfluousFds(const posix::InjectiveMultimap& saved_mapping) {
  // DANGER: no calls to malloc or locks are allowed from now on:
  // http://crbug.com/36678

  // Get the maximum number of FDs possible.
  size_t max_fds = GetMaxFds();

  ::linux::DirReaderLinux fd_dir(kFDDir);
  if (!fd_dir.IsValid()) {
    // Fallback case: Try every possible fd.
    for (size_t i = 0; i < max_fds; ++i) {
      const int fd = static_cast<int>(i);
      if (fd == STDIN_FILENO || fd == STDOUT_FILENO || fd == STDERR_FILENO)
        continue;
      // Cannot use STL iterators here, since debug iterators use locks.
      size_t j;
      for (j = 0; j < saved_mapping.size(); j++) {
        if (fd == saved_mapping[j].dest)
          break;
      }
      if (j < saved_mapping.size())
        continue;

      // Since we're just trying to close anything we can find, ignore any error return values of
      // close().
      close(fd);
    }
    return;
  }

  const int dir_fd = fd_dir.fd();

  for (; fd_dir.Next();) {
    // Skip . and .. entries.
    if (fd_dir.name()[0] == '.')
      continue;

    char* endptr;
    errno = 0;
    const long int fd = strtol(fd_dir.name(), &endptr, 10);
    if (fd_dir.name()[0] == 0 || *endptr || fd < 0 || errno ||
        fd > static_cast<decltype(fd)>(std::numeric_limits<int>::max())) {
      continue;
    }
    if (fd == STDIN_FILENO || fd == STDOUT_FILENO || fd == STDERR_FILENO)
      continue;
    // Cannot use STL iterators here, since debug iterators use locks.
    size_t i;
    for (i = 0; i < saved_mapping.size(); i++) {
      if (fd == saved_mapping[i].dest)
        break;
    }
    if (i < saved_mapping.size())
      continue;
    if (fd == dir_fd)
      continue;

    int ret = IGNORE_EINTR(close(static_cast<int>(fd)));
    FX_DCHECK(ret == 0);
  }
}

}  // namespace

debug::Status LinuxBinaryLauncher::Setup(const std::vector<std::string>& argv) {
  if (argv.empty())
    return debug::Status("No program specified");

  // Create the pipes to redirect stdout and stderr. Note: [0] = read, [1] = write
  int stdout_pipe[2] = {0, 0};
  if (pipe2(stdout_pipe, 0) != 0) {
    return debug::Status("Could not create pipe for stdout.");
  }
  int stderr_pipe[2] = {0, 0};
  if (pipe2(stderr_pipe, 0) != 0) {
    return debug::Status("Could not create pipe for stdout.");
  }

  // For redirecting stdin, stdout, and stderr. This needs to be allocated before forking to avoid
  // allocating in the child. We need two copies because one will be destructively modified.
  posix::InjectiveMultimap fd_shuffle1;
  posix::InjectiveMultimap fd_shuffle2;
  fd_shuffle1.reserve(3);
  fd_shuffle2.reserve(3);

  // Construct the parameter array.
  std::vector<char*> ptrs;
  for (const auto& arg : argv)
    ptrs.push_back(const_cast<char*>(arg.c_str()));
  ptrs.push_back(nullptr);

  pid_ = fork();
  if (pid_ == 0) {
    // DANGER: fork() rule: in the child, if you don't end up doing exec*(), you call _exit()
    // instead of exit(). This is because _exit() does not call any previously-registered (in the
    // parent) exit handlers, which might do things like block waiting for threads that don't even
    // exist in the child.

    // Map /dev/null to stdin so programs waiting for input (as for readline) don't hang.
    {
      int null_fd = HANDLE_EINTR(open("/dev/null", O_RDONLY));
      if (null_fd == -1) {
        _exit(127);
      }

      int new_null_fd = HANDLE_EINTR(dup2(null_fd, STDIN_FILENO));
      if (new_null_fd != STDIN_FILENO) {
        _exit(127);
      }
      IGNORE_EINTR(close(null_fd));
    }

    // Redirect output to our pipes (for both copies of the shuffle structure).
    fd_shuffle1.push_back(posix::InjectionArc(stdout_pipe[1], STDOUT_FILENO, false));
    fd_shuffle2.push_back(posix::InjectionArc(stdout_pipe[1], STDOUT_FILENO, false));
    fd_shuffle1.push_back(posix::InjectionArc(stderr_pipe[1], STDERR_FILENO, false));
    fd_shuffle2.push_back(posix::InjectionArc(stderr_pipe[1], STDERR_FILENO, false));

    // NOTE: fd_shuffle1 is destructively mutated by this call: it can not be used after this.
    if (!posix::ShuffleFileDescriptors(&fd_shuffle1)) {
      _exit(127);
    }

    CloseSuperfluousFds(fd_shuffle2);

    ptrace(PTRACE_TRACEME, 0, nullptr, nullptr);

    // Child process. Execve returns only on failure.
    execv(argv[0].c_str(), &ptrs[0]);
    _exit(1);
  } else {
    // Close our copy of the write end of the stdio handles.
    IGNORE_EINTR(close(stdout_pipe[1]));
    IGNORE_EINTR(close(stderr_pipe[1]));

    // Make our read end of the pipes non-blocking (expected by the handle watching system).
    int flags = fcntl(stdout_pipe[0], F_GETFL);
    if (flags != -1 && (flags & O_NONBLOCK) == 0) {
      fcntl(stdout_pipe[0], F_SETFL, flags | O_NONBLOCK);
    }
    flags = fcntl(stderr_pipe[0], F_GETFL);
    if (flags != -1 && (flags & O_NONBLOCK) == 0) {
      fcntl(stderr_pipe[0], F_SETFL, flags | O_NONBLOCK);
    }

    stdio_handles_.out = OwnedStdioHandle(stdout_pipe[0]);
    stdio_handles_.err = OwnedStdioHandle(stderr_pipe[0]);
  }

  // The execv will issue a trap just after setting up the new process. To ensure the new process is
  // set up by the time we return, block on that signal.
  int status = 0;
  if (waitpid(pid_, &status, 0) < 0)
    return debug::ErrnoStatus(errno, "Can't create process.");

  if (WIFEXITED(status)) {
    // If it exits before we got a stop signal, that means exec failed and the process exited.
    return debug::Status(fxl::StringPrintf("Can't launch '%s'.", argv[0].c_str()));
  }

  if (!WIFSTOPPED(status) || WSTOPSIG(status) != SIGTRAP) {
    DEBUG_LOG(Process) << "Unexpected first signal from the child.";
    return debug::Status("Unexpected signal " + std::to_string(status));
  }

  task_ = fxl::MakeRefCounted<LinuxTask>(pid_, true);

  // The process is now stopped. We need to track this suspension such that later parts of the
  // initialization can use SuspendHandles without resuming the process when those go out-of-scope.
  //
  // As a result, we manually take a suspend reference count to account for the current stop.
  task_->suspend_count_++;
  holding_initial_suspend_ = true;

  return debug::Status();
}

StdioHandles LinuxBinaryLauncher::ReleaseStdioHandles() { return std::move(stdio_handles_); }

std::unique_ptr<ProcessHandle> LinuxBinaryLauncher::GetProcess() const {
  return std::make_unique<LinuxProcessHandle>(task_);
}

debug::Status LinuxBinaryLauncher::Start() {
  // Continue from the first trap caught in Setup(). Decrement the suspend count using the official
  // Task::DecrementSuspendCount() function because we want this to actually resume the task
  // (assuming that the suspend count is going to 0).
  FX_DCHECK(holding_initial_suspend_);
  holding_initial_suspend_ = false;
  task_->DecrementSuspendCount();
  return debug::Status();
}

}  // namespace debug_agent
