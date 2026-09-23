// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#include <dirent.h>
#include <elf.h>
#include <fcntl.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/string_view.h>
#include <limits.h>
#include <link.h>
#include <poll.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <optional>
#include <string_view>

#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/fs.h>
#include <linux/loop.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace test_helper {

namespace {

int lambda_wrapper(void *func_ptr) {
  auto func = static_cast<std::function<void()> *>(func_ptr);
  (*func)();
  _exit(testing::Test::HasFailure());
  return 0;
}

}  // namespace

ForkResult ForkHelper::GenerateForkResult(pid_t pid, int wstatus, int exit_value,
                                          int death_signum) {
  // If we had a normal death signal wired up, only fail if the exit status wasn't expected.
  if (death_signum == 0 && (!WIFEXITED(wstatus) || WEXITSTATUS(wstatus) != exit_value)) {
    auto unexpected_exit = ::testing::AssertionFailure()
                           << "wait_status: WIFEXITED(wstatus) = " << WIFEXITED(wstatus)
                           << ", WEXITSTATUS(wstatus) = " << WEXITSTATUS(wstatus)
                           << ", WTERMSIG(wstatus) = " << WTERMSIG(wstatus);
    return {.subprocess_id = pid,
            .subprocess_exit_status = WEXITSTATUS(wstatus),
            .determined_result = unexpected_exit};
  }
  // If the death signal was set differently, fail if the termination signal didn't match.
  else if (death_signum != 0 && (!WIFSIGNALED(wstatus) || WTERMSIG(wstatus) != death_signum)) {
    auto unexpected_death = ::testing::AssertionFailure()
                            << "wait_status: WIFSIGNALED(wstatus) = " << WIFSIGNALED(wstatus)
                            << ", WEXITSTATUS(wstatus) = " << WEXITSTATUS(wstatus)
                            << ", WTERMSIG(wstatus) = " << WTERMSIG(wstatus);
    return {.subprocess_id = pid,
            .subprocess_exit_status = WEXITSTATUS(wstatus),
            .determined_result = unexpected_death};
  }
  // Otherwise, we can mark this forked process as successful.
  else {
    return {.subprocess_id = pid,
            .subprocess_exit_status = WEXITSTATUS(wstatus),
            .determined_result = testing::AssertionSuccess()};
  }
}

std::vector<ForkResult> ForkHelper::WaitForChildrenInternal(int exit_value, int death_signum) {
  std::vector<ForkResult> results;
  while (wait_for_all_children_ || !child_pids_.empty()) {
    int wstatus;
    pid_t target_pid = wait_for_all_children_ ? -1 : child_pids_.back();
    pid_t pid = waitpid(target_pid, &wstatus, 0);
    if (pid == -1) {
      if (errno == EINTR) {
        continue;
      }
      if (errno == ECHILD) {
        if (wait_for_all_children_) {
          // No more children, reaping is done.
          return results;
        }
        // If we are waiting for specific children, ECHILD means the target_pid is not waitable.
        // This typically happens if the child has already been reaped manually. Pop it and continue
        // to check others.
        child_pids_.pop_back();
        continue;
      }

      // Another error is unexpected, so we'll push that to the results stack.
      auto unexpected_failure = testing::AssertionFailure()
                                << "wait error: " << strerror(errno) << "(" << errno << ")";
      results.push_back({.subprocess_id = pid,
                         .subprocess_exit_status = errno,
                         .determined_result = unexpected_failure});
      if (!wait_for_all_children_) {
        child_pids_.pop_back();
      }
      continue;
    }

    bool check_result = wait_for_all_children_;
    if (!check_result) {
      auto it = std::ranges::find(child_pids_, pid);
      if (it != child_pids_.end()) {
        child_pids_.erase(it);
        check_result = true;
      }
    }

    if (check_result) {
      results.push_back(GenerateForkResult(pid, wstatus, exit_value, death_signum));
    }
  }

  return results;
}

ForkHelper::ForkHelper() : wait_for_all_children_(true), death_signum_(0), exit_value_(0) {
  // Ensure that all children will ends up being parented to the process that
  // created the helper.
  prctl(PR_SET_CHILD_SUBREAPER, 1);
}

ForkHelper::~ForkHelper() {
  // Wait for all remaining children, and ensure none failed.
  auto results = WaitForChildrenInternal(exit_value_, death_signum_);
  for (const auto &result : results) {
    // EXPECT_TRUE on the result directly will implicitly print failure details.!
    EXPECT_TRUE(result.determined_result);
  }
}

void ForkHelper::OnlyWaitForForkedChildren() { wait_for_all_children_ = false; }

void ForkHelper::ExpectSignal(int signum) { death_signum_ = signum; }

void ForkHelper::ExpectExitValue(int value) { exit_value_ = value; }

testing::AssertionResult ForkHelper::WaitForChildren() {
  auto results = WaitForChildrenInternal(exit_value_, death_signum_);
  for (const auto &result : results) {
    if (result.determined_result != testing::AssertionSuccess()) {
      return result.determined_result;
    }
  }

  return testing::AssertionSuccess();
}

ForkResult ForkHelper::WaitForChild(pid_t child_pid) {
  int wstatus;
  int pid;

  // Continually loop waitpid to handle EINTR signals.
  while ((pid = waitpid(child_pid, &wstatus, 0)) == -1 && errno == EINTR) {
  }

  if (pid == -1) {
    auto unexpected_failure = testing::AssertionFailure()
                              << "wait error: " << strerror(errno) << "(" << errno << ")";
    return {.subprocess_id = pid,
            .subprocess_exit_status = errno,
            .determined_result = unexpected_failure};
  }

  auto it = std::ranges::find(child_pids_, pid);
  EXPECT_TRUE(it != child_pids_.end());
  child_pids_.erase(it);

  return GenerateForkResult(pid, wstatus, exit_value_, death_signum_);
}

std::vector<ForkResult> ForkHelper::WaitForChildrenWithResults() {
  return WaitForChildrenInternal(exit_value_, death_signum_);
}

pid_t ForkHelper::RunInForkedProcess(fit::function<void()> action) {
  // Flush parent buffers before forking to prevent the child from double-flushing them.
  fflush(nullptr);
  pid_t pid = SAFE_SYSCALL(fork());
  if (pid != 0) {
    child_pids_.push_back(pid);
    return pid;
  }
  action();
  // Flush buffers before exiting to ensure failures are logged. Unlike exit(), _exit() bypasses
  // flushing.
  fflush(nullptr);
  _exit(testing::Test::HasFailure());
}

CloneHelper::CloneHelper() {
  // Stack setup
  this->_childStack =
      static_cast<uint8_t *>(mmap(nullptr, CloneHelper::_childStackSize, PROT_WRITE | PROT_READ,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  if (this->_childStack == MAP_FAILED) {
    std::cerr << "CloneHelper mmap failed, errno was set to '" << strerror(errno) << "' (" << errno
              << ").\n";
    assert(false);
  }
  this->_childStackBegin = this->_childStack + CloneHelper::_childStackSize;
}

CloneHelper::~CloneHelper() { munmap(this->_childStack, CloneHelper::_childStackSize); }

int CloneHelper::runInClonedChild(unsigned int cloneFlags, int (*childFunction)(void *)) {
  int childPid = clone(childFunction, this->_childStackBegin, cloneFlags, nullptr);
  assert(childPid != -1);
  return childPid;
}

int CloneHelper::runInClonedChild(unsigned int cloneFlags, std::function<void()> action) {
  return SAFE_SYSCALL(clone(lambda_wrapper, this->_childStackBegin, cloneFlags, &action));
}

int CloneHelper::sleep_1sec(void *) {
  struct timespec res;
  res.tv_sec = 1;
  res.tv_nsec = 0;
  clock_nanosleep(CLOCK_MONOTONIC, 0, &res, &res);
  return 0;
}

int CloneHelper::doNothing(void *) { return 0; }

void SignalMaskHelper::blockSignal(int signal) {
  sigemptyset(&this->_sigset);
  sigaddset(&this->_sigset, signal);
  sigprocmask(SIG_BLOCK, &this->_sigset, &this->_sigmaskCopy);
}

void SignalMaskHelper::waitForSignal(int signal) {
  int sig;
  int result = static_cast<int>(TEMP_FAILURE_RETRY(sigwait(&this->_sigset, &sig)));
  ASSERT_EQ(result, 0);
  ASSERT_EQ(sig, signal);
}

int SignalMaskHelper::timedWaitForSignal(int signal, time_t msec) {
  siginfo_t siginfo;
  struct timespec ts;
  ts.tv_sec = 0;
  ts.tv_nsec = msec * 1000000;
  return static_cast<int>(TEMP_FAILURE_RETRY(sigtimedwait(&this->_sigset, &siginfo, &ts)));
}

void SignalMaskHelper::restoreSigmask() { sigprocmask(SIG_SETMASK, &this->_sigmaskCopy, nullptr); }

ScopedTempFD::ScopedTempFD() : name_("/tmp/proc_test_file_XXXXXX") {
  char *mut_name = const_cast<char *>(name_.c_str());
  fd_ = fbl::unique_fd(mkstemp(mut_name));
}

ScopedTempFD::ScopedTempFD(ScopedTempFD &&other) noexcept
    : name_(std::move(other.name_)), fd_(std::move(other.fd_)) {}

ScopedTempFD &ScopedTempFD::operator=(ScopedTempFD &&other) noexcept {
  name_ = std::move(other.name_);
  fd_ = std::move(other.fd_);
  return *this;
}

ScopedTempDir::ScopedTempDir() : ScopedTempDir(get_tmp_path()) {}

ScopedTempDir::ScopedTempDir(const std::string &parent_path) {
  path_ = parent_path + "/testdirXXXXXX";
  if (!mkdtemp(path_.data())) {
    path_.clear();
  }
}

ScopedTempDir::~ScopedTempDir() {
  if (!path_.empty()) {
    RecursiveUnmountAndRemove(path_);
  }
}

ScopedTempSymlink::ScopedTempSymlink(const char *target_path) {
  std::string prefix = "/tmp/syscall_test_symlink_";
  size_t retries = 100;
  while (retries--) {
    std::string path = prefix + RandomHexString(6);
    if (symlink(target_path, path.c_str()) == 0) {
      path_ = path;
      return;
    }
  }
  path_.clear();
}

ScopedTempSymlink::~ScopedTempSymlink() {
  if (!path_.empty()) {
    unlink(path_.c_str());
  }
}

void waitForChildSucceeds(unsigned int waitFlag, int cloneFlags, int (*childRunFunction)(void *),
                          int (*parentRunFunction)(void *)) {
  CloneHelper cloneHelper;
  int expectedWaitPid = cloneHelper.runInClonedChild(cloneFlags, childRunFunction);

  parentRunFunction(nullptr);

  int expectedWaitStatus = 0;
  int expectedErrno = 0;
  int actualWaitStatus;
  int actualWaitPid = waitpid(expectedWaitPid, &actualWaitStatus, waitFlag);
  EXPECT_EQ(actualWaitPid, expectedWaitPid);
  EXPECT_EQ(actualWaitStatus, expectedWaitStatus);
  EXPECT_EQ(errno, expectedErrno);
}

void waitForChildFails(unsigned int waitFlag, int cloneFlags, int (*childRunFunction)(void *),
                       int (*parentRunFunction)(void *)) {
  CloneHelper cloneHelper;
  int pid = cloneHelper.runInClonedChild(cloneFlags, childRunFunction);

  parentRunFunction(nullptr);

  int expectedWaitPid = -1;
  int actualWaitPid = waitpid(pid, nullptr, waitFlag);
  EXPECT_EQ(actualWaitPid, expectedWaitPid);
  EXPECT_EQ(errno, ECHILD);
  errno = 0;
}

std::string get_tmp_path() {
  static std::string tmp_path = [] {
    const char *tmp = getenv("TEST_TMPDIR");
    if (tmp == nullptr)
      tmp = "/tmp";
    return tmp;
  }();
  return tmp_path;
}

std::string GetSystemDynamicLinkerPath() {
  fbl::unique_fd fd(open("/proc/self/exe", O_RDONLY));
  if (!fd) {
    return "";
  }
  ElfW(Ehdr) ehdr;
  if (read(fd.get(), &ehdr, sizeof(ehdr)) != sizeof(ehdr)) {
    return "";
  }
  if (memcmp(ehdr.e_ident, ELFMAG, SELFMAG) != 0) {
    return "";
  }
  std::vector<ElfW(Phdr)> phdrs(ehdr.e_phnum);
  if (pread(fd.get(), phdrs.data(), ehdr.e_phnum * sizeof(ElfW(Phdr)), ehdr.e_phoff) !=
      static_cast<ssize_t>(ehdr.e_phnum * sizeof(ElfW(Phdr)))) {
    return "";
  }
  for (const auto &phdr : phdrs) {
    if (phdr.p_type == PT_INTERP) {
      std::string interp(phdr.p_filesz, '\0');
      if (pread(fd.get(), interp.data(), phdr.p_filesz, phdr.p_offset) ==
          static_cast<ssize_t>(phdr.p_filesz)) {
        if (!interp.empty() && interp.back() == '\0') {
          interp.pop_back();
        }
        return interp;
      }
    }
  }
  return "";
}

std::string GetTestResourcePath(const std::string &resource) {
  std::filesystem::path test_file = std::filesystem::path("data") / resource;
  return test_file.string();
}

namespace {
std::optional<MemoryMapping> parse_mapping_entry(std::string_view line) {
  // format:
  // start-end perms offset device inode path
  std::vector<std::string_view> parts =
      fxl::SplitString(line, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (parts.size() < 5) {
    return std::nullopt;
  }
  std::vector<std::string_view> addrs =
      fxl::SplitString(parts[0], "-", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  if (addrs.size() != 2) {
    return std::nullopt;
  }

  uintptr_t start;
  uintptr_t end;

  if (!fxl::StringToNumberWithError(addrs[0], &start, fxl::Base::k16) ||
      !fxl::StringToNumberWithError(addrs[1], &end, fxl::Base::k16)) {
    return std::nullopt;
  }

  size_t offset;
  size_t inode;
  if (!fxl::StringToNumberWithError(parts[2], &offset, fxl::Base::k16) ||
      !fxl::StringToNumberWithError(parts[4], &inode, fxl::Base::k10)) {
    return std::nullopt;
  }

  std::string pathname;
  if (parts.size() > 5) {
    // The pathname always starts at pos 73.
    pathname = line.substr(73);
  }

  return MemoryMapping{
      .start = start,
      .end = end,
      .perms = std::string(parts[1]),
      .offset = offset,
      .device = std::string(parts[3]),
      .inode = inode,
      .pathname = pathname,
  };
}
}  // namespace

std::optional<size_t> parse_field_in_kb(std::string_view value) {
  const std::string_view suffix = " kB";
  if (!value.ends_with(suffix)) {
    return std::nullopt;
  }

  value.remove_suffix(suffix.length());
  size_t result;
  if (!fxl::StringToNumberWithError(value, &result, fxl::Base::k10)) {
    return std::nullopt;
  }
  return result;
}

std::optional<MemoryMapping> find_memory_mapping(fit::function<bool(const MemoryMapping &)> match,
                                                 std::string_view maps) {
  std::vector<std::string_view> lines =
      fxl::SplitString(maps, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  for (auto line : lines) {
    std::optional<MemoryMapping> mapping = parse_mapping_entry(line);
    if (!mapping) {
      return std::nullopt;
    }

    if (match(*mapping)) {
      return mapping;
    }
  }
  return std::nullopt;
}

std::optional<MemoryMapping> find_memory_mapping(uintptr_t addr, std::string_view maps) {
  return find_memory_mapping(
      [addr](const MemoryMapping &mapping) { return mapping.start <= addr && addr < mapping.end; },
      maps);
}

std::optional<MemoryMappingExt> find_memory_mapping_ext(
    fit::function<bool(const MemoryMappingExt &)> match, std::string_view maps) {
  std::vector<std::string_view> lines =
      fxl::SplitString(maps, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  std::optional<MemoryMappingExt> current_mapping;
  for (auto line : lines) {
    std::optional<MemoryMapping> maybe_new_mapping = parse_mapping_entry(line);
    if (maybe_new_mapping) {
      if (current_mapping && match(*current_mapping)) {
        return current_mapping;
      }

      current_mapping = MemoryMappingExt(*maybe_new_mapping);
      continue;
    }
    std::vector<std::string_view> fields =
        fxl::SplitString(line, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
    if (fields.size() != 2) {
      return std::nullopt;
    }
    if (fields[0] == "Rss") {
      if (std::optional<size_t> rss = parse_field_in_kb(fields[1])) {
        current_mapping->rss = *rss;
      } else {
        return std::nullopt;
      }
    }
    if (fields[0] == "VmFlags") {
      current_mapping->vm_flags =
          fxl::SplitStringCopy(fields[1], " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
    }
  }
  if (current_mapping && match(*current_mapping)) {
    return current_mapping;
  }
  return std::nullopt;
}

std::optional<MemoryMappingExt> find_memory_mapping_ext(uintptr_t addr, std::string_view maps) {
  return find_memory_mapping_ext(
      [addr](const MemoryMappingExt &mapping) {
        return mapping.start <= addr && addr < mapping.end;
      },
      maps);
}

std::ostream &operator<<(std::ostream &os, const MemoryMappingExt &mapping) {
  os << "\tstart:\t0x" << std::hex << mapping.start << "\n";
  os << "\tend:\t0x" << std::hex << mapping.end << "\n";
  os << "\tperms:\t" << mapping.perms << "\n";
  os << "\toffset:\t0x" << mapping.offset << "\n";
  os << "\tdevice:\t" << mapping.device << "\n";
  os << "\tinode:\t" << mapping.inode << "\n";
  os << "\tpath:\t" << mapping.pathname << "\n";
  os << "\trss:\t" << mapping.rss << "\n";
  os << "\tflags:\t";
  for (auto &vm_flag : mapping.vm_flags) {
    os << vm_flag << " ";
  }
  return os;
}

std::string RandomHexString(size_t length) {
  constexpr char kHexCharacters[] = "0123456789ABCDEF";
  constexpr size_t kRadix = sizeof(kHexCharacters) - 1;

  std::string value(length, '\0');
  for (size_t i = 0; i < length; ++i) {
    value[i] = kHexCharacters[random() % kRadix];
  }

  return value;
}

bool HasSysAdmin() { return HasCapability(CAP_SYS_ADMIN); }

bool IsStarnix() {
  struct utsname buf;
  return uname(&buf) == 0 && strstr(buf.release, "starnix") != nullptr;
}

bool IsKernelVersionAtLeast(int min_major, int min_minor) {
  struct utsname buf;
  int major, minor;
  if (uname(&buf) != 0) {
    return false;
  }
  if (sscanf(buf.release, "%d.%d:", &major, &minor) != 2) {
    return false;
  }
  return major > min_major || (major == min_major && minor >= min_minor);
}

void RecursiveUnmountAndRemove(const std::string &path) {
  if (HasSysAdmin()) {
    // Repeatedly call umount to handle shadowed mounts properly.
    do {
      errno = 0;
      ASSERT_THAT(umount2(path.c_str(), MNT_DETACH),
                  AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EINVAL)))
          << path;
    } while (errno != EINVAL);
  }

  int dir_fd = open(path.c_str(), O_DIRECTORY | O_NOFOLLOW | O_RDONLY);
  if (dir_fd >= 0) {
    std::vector<std::pair<std::string, unsigned char>> entries;

    // Use getdents64 directly to avoid EOVERFLOW on 32-bit architectures
    // when the filesystem returns 64-bit inode numbers or offsets.
    struct linux_dirent64 {
      uint64_t d_ino;
      int64_t d_off;
      unsigned short d_reclen;
      unsigned char d_type;
      char d_name[];
    };

    std::vector<uint8_t> buffer(32768);
    while (true) {
      long nread = syscall(SYS_getdents64, dir_fd, buffer.data(), buffer.size());
      if (nread == -1) {
        ADD_FAILURE() << "getdents64 failed: " << std::strerror(errno);
        break;
      }
      if (nread == 0) {
        break;
      }

      long bpos = 0;
      while (bpos < nread) {
        ASSERT_EQ(bpos % 8, 0);
        auto d = reinterpret_cast<linux_dirent64 *>(buffer.data() + bpos);
        std::string name(d->d_name);
        if (name != "." && name != "..") {
          entries.push_back({name, d->d_type});
        }
        bpos += d->d_reclen;
      }
    }

    for (const auto &entry : entries) {
      std::string subpath = std::string(path) + "/" + entry.first;
      if (entry.second == DT_DIR) {
        RecursiveUnmountAndRemove(subpath);
      } else {
        EXPECT_THAT(unlink(subpath.c_str()), SyscallSucceeds()) << subpath;
      }
    }
    close(dir_fd);
    EXPECT_THAT(rmdir(path.c_str()), SyscallSucceeds()) << path;
  }
}

int MemFdCreate(const char *name, unsigned int flags) {
  return static_cast<int>(syscall(SYS_memfd_create, name, flags));
}

int PidFdOpen(pid_t pid, unsigned int flags) {
  return static_cast<int>(syscall(SYS_pidfd_open, pid, flags));
}

// Attempts to read a byte from the given memory address.
// Returns whether the read succeeded or not.
bool TryRead(uintptr_t addr) {
  fbl::unique_fd mem_fd(MemFdCreate("try_read", O_WRONLY));
  EXPECT_TRUE(mem_fd.is_valid());

  return write(mem_fd.get(), reinterpret_cast<void *>(addr), 1) == 1;
}

// Attempts to write a zero byte to the given memory address.
// Returns whether the write succeeded or not.
bool TryWrite(uintptr_t addr) {
  fbl::unique_fd zero_fd(open("/dev/zero", O_RDONLY));
  EXPECT_TRUE(zero_fd.is_valid());

  return read(zero_fd.get(), reinterpret_cast<void *>(addr), 1) == 1;
}

testing::AssertionResult WaitUntilBlocked(pid_t target, bool ignore_tracer) {
  return WaitForTaskState(target, target, [ignore_tracer](std::string_view state) {
    return cpp23::contains(state, 'S') || (!ignore_tracer && cpp23::contains(state, 't'));
  });
}

testing::AssertionResult WaitUntilZombie(pid_t target) {
  return WaitForTaskState(target, target,
                          [](std::string_view state) { return cpp23::contains(state, 'Z'); });
}

testing::AssertionResult WaitForTaskState(pid_t pid, pid_t tid,
                                          fit::function<bool(std::string_view)> predicate) {
  std::string stat_path = fxl::StringPrintf("/proc/%d/task/%d/stat", pid, tid);
  for (int i = 0; i < 100000; i++) {
    std::string stat_buf;
    if (!files::ReadFileToString(stat_path, &stat_buf)) {
      return testing::AssertionFailure() << "Failed to read " << stat_path;
    }
    if (predicate(stat_buf)) {
      return testing::AssertionSuccess();
    }
    if (i == 99999) {
      return testing::AssertionFailure()
             << "Failed to wait for task with PID=" << pid << " TID=" << tid
             << " to reach state. Final state: " << stat_buf;
    }
    usleep(1000);  // Sleep for 1ms to avoid busy-looping.
  }
  return testing::AssertionFailure() << "Unreachable";
}

// This variable is accessed from within a signal handler and thus must be declared volatile.
static volatile void *expected_fault_address;

testing::AssertionResult TestThatAccessSegfaults(void *test_address, AccessType type) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([test_address, type] {
    struct sigaction action;
    action.sa_sigaction = [](int signo, siginfo_t *info, void *ucontext) {
      if (signo == SIGSEGV && info->si_addr == expected_fault_address) {
        _exit(EXIT_SUCCESS);
      } else {
        _exit(EXIT_FAILURE);
      }
    };
    action.sa_flags = SA_SIGINFO;
    SAFE_SYSCALL(sigaction(SIGSEGV, &action, nullptr));
    expected_fault_address = test_address;
    if (type == AccessType::Read) {
      *static_cast<volatile std::byte *>(test_address);
    } else {
      *static_cast<volatile std::byte *>(test_address) = std::byte{};
    }
    FAIL() << "Must have observed segfault after access.";
  });
  return helper.WaitForChildren();
}

ScopedLoopDevice::ScopedLoopDevice(fbl::unique_fd dev_fd, std::string device_path)
    : dev_fd_(std::move(dev_fd)), device_path_(std::move(device_path)) {}

fit::result<int, ScopedLoopDevice> ScopedLoopDevice::Create(int image_fd) {
  fbl::unique_fd loop_control(open("/dev/loop-control", O_RDWR | O_CLOEXEC));
  if (!loop_control.is_valid()) {
    return fit::error(errno);
  }
  int num = ioctl(loop_control.get(), LOOP_CTL_GET_FREE, nullptr);
  if (num < 0) {
    return fit::error(errno);
  }
  std::string device_path = "/dev/loop" + std::to_string(num);
  int mode = fcntl(image_fd, F_GETFL) & O_ACCMODE;
  int dev_flags = (mode == O_RDONLY) ? O_RDONLY : O_RDWR;
  fbl::unique_fd dev_fd(open(device_path.c_str(), dev_flags | O_CLOEXEC));
  if (!dev_fd.is_valid()) {
    return fit::error(errno);
  }
  if (ioctl(dev_fd.get(), LOOP_SET_FD, image_fd) < 0) {
    return fit::error(errno);
  }
  return fit::ok(ScopedLoopDevice(std::move(dev_fd), std::move(device_path)));
}

ScopedLoopDevice &ScopedLoopDevice::operator=(ScopedLoopDevice &&other) noexcept {
  if (this == &other) {
    return *this;
  }
  Detach();
  dev_fd_ = std::move(other.dev_fd_);
  device_path_ = std::move(other.device_path_);
  return *this;
}

ScopedLoopDevice::~ScopedLoopDevice() { Detach(); }

void ScopedLoopDevice::Detach() {
  if (dev_fd_.is_valid()) {
    ioctl(dev_fd_.get(), LOOP_CLR_FD, 0);
    dev_fd_.reset();
  }
}

ScopedMount::ScopedMount(std::string target_path)
    : target_path_(std::move(target_path)), is_mounted_(true) {}

fit::result<int, ScopedMount> ScopedMount::Mount(const std::string &source,
                                                 const std::string &target,
                                                 const std::string &filesystemtype,
                                                 unsigned long mountflags, const void *data) {
  if (mount(source.c_str(), target.c_str(), filesystemtype.c_str(), mountflags, data) != 0) {
    int error = errno;
    return fit::error(error);
  }
  return fit::ok(ScopedMount(target));
}

fit::result<int, ScopedMount> ScopedMount::CreateDirAndMount(const std::string &source,
                                                             const std::string &target,
                                                             const std::string &filesystemtype,
                                                             unsigned long mountflags,
                                                             const void *data) {
  if (mkdir(target.c_str(), 0777) != 0 && errno != EEXIST) {
    return fit::error(errno);
  }
  return Mount(source, target, filesystemtype, mountflags, data);
}

ScopedMount &ScopedMount::operator=(ScopedMount &&other) noexcept {
  Unmount();
  is_mounted_ = other.is_mounted_;
  target_path_ = other.target_path_;
  other.is_mounted_ = false;
  return *this;
}

ScopedMount::ScopedMount(ScopedMount &&other) noexcept : ScopedMount("") {
  *this = std::move(other);
}

ScopedMount::~ScopedMount() { Unmount(); }

void ScopedMount::Unmount() {
  if (is_mounted_) {
    umount(target_path_.c_str());
    is_mounted_ = false;
  }
}

fit::result<int> SetCasefold(const std::string &path, bool enable) {
  fbl::unique_fd dir_fd(open(path.c_str(), O_RDONLY | O_DIRECTORY));
  if (!dir_fd.is_valid()) {
    return fit::error(errno);
  }
  int flags = 0;
  if (ioctl(dir_fd.get(), FS_IOC_GETFLAGS, &flags) < 0) {
    return fit::error(errno);
  }
  if (enable) {
    flags |= FS_CASEFOLD_FL;
  } else {
    flags &= ~FS_CASEFOLD_FL;
  }
  if (ioctl(dir_fd.get(), FS_IOC_SETFLAGS, &flags) < 0) {
    return fit::error(errno);
  }
  return fit::ok();
}

ScopedPipe::ScopedPipe() {
  int pipe_fds[2];
  SAFE_SYSCALL(pipe(pipe_fds));
  read_side_ = fbl::unique_fd(pipe_fds[0]);
  write_side_ = fbl::unique_fd(pipe_fds[1]);
}

ScopedPipe::ScopedPipe(int flags) {
  int pipe_fds[2];
  SAFE_SYSCALL(pipe2(pipe_fds, flags));
  read_side_ = fbl::unique_fd(pipe_fds[0]);
  write_side_ = fbl::unique_fd(pipe_fds[1]);
}

ScopedPipe::ScopedPipe(ScopedPipe &&o)
    : read_side_(std::move(o.read_side_)), write_side_(std::move(o.write_side_)) {}

ScopedPipe &ScopedPipe::operator=(ScopedPipe &&o) {
  read_side_ = std::move(o.read_side_);
  write_side_ = std::move(o.write_side_);
  return *this;
}

Poker::Poker(fbl::unique_fd pipe_write_side) : pipe_write_side_(std::move(pipe_write_side)) {}

Poker::Poker(Poker &&o) : pipe_write_side_(std::move(o.pipe_write_side_)) {}

Poker &Poker::operator=(Poker &&o) {
  pipe_write_side_ = std::move(o.pipe_write_side_);
  return *this;
}

void Poker::poke() {
  char clog[] = "clog";  // This data will enter but never leave the pipe.
  SAFE_SYSCALL(write(pipe_write_side_.get(), clog, sizeof(clog)));
  SAFE_SYSCALL(pipe_write_side_.reset());
}

Holder::Holder(fbl::unique_fd pipe_read_side) : pipe_read_side_(std::move(pipe_read_side)) {}

Holder::Holder(Holder &&o) : pipe_read_side_(std::move(o.pipe_read_side_)) {}

Holder &Holder::operator=(Holder &&o) {
  pipe_read_side_ = std::move(o.pipe_read_side_);
  return *this;
}

void Holder::hold() {
  struct pollfd pfd = {
      .fd = pipe_read_side_.get(),
      .events = POLLIN | POLLHUP,
      .revents = 0,
  };
  SAFE_SYSCALL(HANDLE_EINTR(poll(&pfd, 1, -1)));
}

Rendezvous MakeRendezvous() { return MakeRendezvous(ScopedPipe()); }

Rendezvous MakeRendezvous(ScopedPipe pipe) {
  return {
      .poker = Poker(std::move(pipe.WriteSide())),
      .holder = Holder(std::move(pipe.ReadSide())),
  };
}

fit::result<int, std::vector<MountInfo>> ReadMountInfo() {
  std::string content;
  if (!files::ReadFileToString("/proc/self/mountinfo", &content)) {
    return fit::error(errno);
  }

  std::vector<MountInfo> result;
  std::vector<std::string_view> lines =
      fxl::SplitString(content, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);

  for (auto line : lines) {
    std::vector<std::string_view> parts =
        fxl::SplitString(line, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
    if (parts.size() < 10) {
      continue;
    }

    MountInfo info;
    info.mount_id = std::string(parts[0]);
    info.parent_id = std::string(parts[1]);
    info.major_minor = std::string(parts[2]);
    info.root = std::string(parts[3]);
    info.mount_point = std::string(parts[4]);
    info.mount_options = std::string(parts[5]);

    size_t i = 6;
    while (i < parts.size() && parts[i] != "-") {
      info.optional_fields.push_back(std::string(parts[i]));
      i++;
    }

    if (i < parts.size() && parts[i] == "-") {
      i++;  // Skip separator
    }

    if (i + 2 < parts.size()) {
      info.fs_type = std::string(parts[i]);
      info.mount_source = std::string(parts[i + 1]);
      info.super_options = std::string(parts[i + 2]);
    }
    result.push_back(std::move(info));
  }

  return fit::ok(result);
}

std::optional<MountInfo> ReadMountInfoLine(const std::string &path) {
  auto result = ReadMountInfo();
  if (result.is_error()) {
    return std::nullopt;
  }

  for (const auto &info : result.value()) {
    if (info.mount_point == path) {
      return std::make_optional(info);
    }
  }

  return std::nullopt;
}

fit::result<int, std::set<std::string>> ListDirectory(const char *dir) {
  DIR *d = opendir(dir);
  if (d == nullptr) {
    return fit::error(errno);
  }
  std::set<std::string> name_set;
  dirent *e;
  while ((e = readdir(d)) != nullptr) {
    if (strcmp(e->d_name, ".") != 0 && strcmp(e->d_name, "..") != 0) {
      name_set.insert(e->d_name);
    }
  }
  closedir(d);
  return fit::ok(std::move(name_set));
}

constexpr char kDevInputDir[] = "/dev/input";

std::vector<std::string> WaitForDevice(const std::set<std::string> &ls_before) {
  std::vector<std::string> diff;
  for (int i = 0; i < 100; ++i) {
    diff.clear();
    auto ls_after = ListDirectory(kDevInputDir);
    if (ls_after.is_error()) {
      if (ls_after.error_value() != ENOENT) {
        return {};
      }
    } else {
      std::set_difference(ls_after->begin(), ls_after->end(), ls_before.begin(), ls_before.end(),
                          std::back_inserter(diff));
      if (!diff.empty()) {
        break;
      }
    }
    usleep(50000);  // 50ms (total 5s)
  }
  return diff;
}

}  // namespace test_helper
