// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_TEST_HELPER_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_TEST_HELPER_H_

#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <stdint.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <unistd.h>

#include <optional>
#include <set>
#include <string_view>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/genetlink.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <linux/taskstats.h>

#define SAFE_SYSCALL(X)                                                             \
  ({                                                                                \
    auto retval = (X);                                                              \
    if (retval < 0) {                                                               \
      ADD_FAILURE() << #X << " failed: " << strerror(errno) << "(" << errno << ")"; \
      retval = {};                                                                  \
    }                                                                               \
    retval;                                                                         \
  })

// TODO(https://fxbug.dev/317285180) don't skip on baseline
#define SAFE_SYSCALL_SKIP_ON_EPERM(X)                                          \
  ({                                                                           \
    auto retval = (X);                                                         \
    if (retval < 0) {                                                          \
      if (errno == EPERM) {                                                    \
        GTEST_SKIP() << "Permission denied for " << #X << ", skipping tests."; \
      } else {                                                                 \
        FAIL() << #X << " failed: " << strerror(errno) << "(" << errno << ")"; \
      }                                                                        \
    }                                                                          \
    retval;                                                                    \
  })

#define ASSERT_RESULT_SUCCESS_AND_RETURN(S)          \
  ({                                                 \
    auto retval = (S);                               \
    ASSERT_TRUE(retval.is_ok()) << #S << " failed."; \
    std::move(retval.value());                       \
  })

namespace test_helper {

struct ForkResult {
  pid_t subprocess_id;
  int subprocess_exit_status;
  testing::AssertionResult determined_result;
};

// Helper class to handle test that needs to fork and do assertion on the child
// process.
class ForkHelper {
 public:
  ForkHelper();
  ~ForkHelper();

  // Wait for all children of the current process, and return true if all exited
  // with a 0 status or with the expected signal.
  testing::AssertionResult WaitForChildren();

  // Wait for a specific child of the current process, and return results on it.
  ForkResult WaitForChild(pid_t child_pid);

  // Wait for all children of the current process, and return results on each.
  std::vector<ForkResult> WaitForChildrenWithResults();

  // For the current process and execute the given |action| inside the child,
  // then exit with a status equals to the number of failed expectation and
  // assertion. Return immediately with the pid of the child.
  pid_t RunInForkedProcess(fit::function<void()> action);

  // If called, checks for process termination by the given signal, instead of expecting
  // the forked process to terminate normally.
  void ExpectSignal(int signum);

  // If called, checks for process termination with the given exit value,
  // instead of expecting the forked process to terminate normally.
  void ExpectExitValue(int value);

  // If called, only waits for the children explicitly forked by the ForkHelper, instead
  // of waiting for all children.
  void OnlyWaitForForkedChildren();

 private:
  std::vector<pid_t> child_pids_;
  bool wait_for_all_children_;
  int death_signum_;
  int exit_value_;

  std::vector<ForkResult> WaitForChildrenInternal(int exit_value, int death_signum);
  ForkResult GenerateForkResult(pid_t pid, int wstatus, int exit_value, int death_signum);
};

// Helper class to handle tests that needs to clone processes.
class CloneHelper {
 public:
  CloneHelper();
  ~CloneHelper();

  // Call clone with the specified childFunction and cloneFlags.
  // Perform the necessary asserts to ensure the clone was performed with
  // no errors and return the new process ID.
  int runInClonedChild(unsigned int cloneFlags, int (*childFunction)(void *));

  // Call clone and execute the given |action| inside the child,
  // then exit with a status equal to the number of failed expectations and
  // assertions. Return immediately with the pid of the child.
  int runInClonedChild(unsigned int cloneFlags, std::function<void()> action);

  // Handy trivial function for passing clone when we want the child to
  // sleep for 1 second and return 0.
  static int sleep_1sec(void *);

  // Handy trivial function for passing clone when we want the child to
  // do nothing and return 0.
  static int doNothing(void *);

 private:
  uint8_t *_childStack;
  uint8_t *_childStackBegin;
  static constexpr size_t _childStackSize = 0x5000;
};

// Helper class to modify signal masks of processes
class SignalMaskHelper {
 public:
  // Blocks the specified signal and saves the original signal mask
  // to _sigmaskCopy.
  void blockSignal(int signal);

  // Blocks the execution until the specified signal is received.
  void waitForSignal(int signal);

  // Blocks the execution until the specified signal is received or timed out.
  int timedWaitForSignal(int signal, time_t msec);

  // Sets the signal mask of the process with _sigmaskCopy.
  void restoreSigmask();

 private:
  sigset_t _sigset;
  sigset_t _sigmaskCopy;
};

class ScopedTempFD {
 public:
  ScopedTempFD();
  ~ScopedTempFD() { unlink(name_.c_str()); }

  ScopedTempFD(ScopedTempFD &&other) noexcept;
  ScopedTempFD &operator=(ScopedTempFD &&other) noexcept;

  bool is_valid() const { return fd_.is_valid(); }
  explicit operator bool() const { return is_valid(); }

  const std::string &name() const { return name_; }
  int fd() const { return fd_.get(); }

  std::string name_;
  fbl::unique_fd fd_;
};

class ScopedTempDir {
 public:
  ScopedTempDir();
  explicit ScopedTempDir(const std::string &parent_path);
  ~ScopedTempDir();

  ScopedTempDir(const ScopedTempDir &) = delete;
  ScopedTempDir &operator=(const ScopedTempDir &) = delete;

  ScopedTempDir(ScopedTempDir &&) = default;
  ScopedTempDir &operator=(ScopedTempDir &&) = default;

  const std::string &path() const { return path_; }

 private:
  std::string path_;
};

class ScopedTempSymlink {
 public:
  explicit ScopedTempSymlink(const char *target_path);
  ~ScopedTempSymlink();

  bool is_valid() const { return !path_.empty(); }
  explicit operator bool() const { return is_valid(); }

  const std::string &path() const { return path_; }

 private:
  std::string path_;
};

#define HANDLE_EINTR(x)                                     \
  ({                                                        \
    decltype(x) eintr_wrapper_result;                       \
    do {                                                    \
      eintr_wrapper_result = (x);                           \
    } while (eintr_wrapper_result == -1 && errno == EINTR); \
    eintr_wrapper_result;                                   \
  })

void waitForChildSucceeds(unsigned int waitFlag, int cloneFlags, int (*childRunFunction)(void *),
                          int (*parentRunFunction)(void *));

void waitForChildFails(unsigned int waitFlag, int cloneFlags, int (*childRunFunction)(void *),
                       int (*parentRunFunction)(void *));

std::string get_tmp_path();

// Returns the path to the system dynamic linker requested by /proc/self/exe.
std::string GetSystemDynamicLinkerPath();

// Returns the path to a test resource named `resource`, which should be in
// "data" relative to the current working directory.
std::string GetTestResourcePath(const std::string &resource);

struct MemoryMapping {
  uintptr_t start;
  uintptr_t end;
  std::string perms;
  size_t offset;
  std::string device;
  size_t inode;
  std::string pathname;
};

struct MemoryMappingExt : public MemoryMapping {
  size_t rss;
  std::vector<std::string> vm_flags;

 public:
  explicit MemoryMappingExt(const MemoryMapping &mapping) : MemoryMapping(mapping) {}
  bool ContainsFlag(std::string_view flag) const {
    return std::ranges::find(vm_flags, flag) != vm_flags.end();
  }
  friend std::ostream &operator<<(std::ostream &os, const MemoryMappingExt &mapping);
};

#define MY_NLMSG_OK(nlh, len) NLMSG_OK((nlh), static_cast<decltype((nlh)->nlmsg_len)>(len))

// Encoder for serializing netlink messages
class NetlinkEncoder {
 public:
  // Writes a value to the buffer at the current offset
  template <typename T>
  void Write(const T &value) {
    Write(&value, sizeof(T));
  }

  void WriteSpan(const std::span<const uint8_t> &data) { Write(data.data(), data.size_bytes()); }

  void WriteString(const std::string_view &data) { Write(data.data(), data.size()); }

  // Writes a value to the buffer at a specified offset
  template <typename T>
  void Write(T &value, size_t offset) {
    Write(&value, offset, sizeof(T));
  }

  // Reads a value from the buffer at a specified offset
  template <typename T>
  void Read(T &out, size_t offset) {
    Read(&out, offset, sizeof(T));
  }

  NetlinkEncoder() = default;
  NetlinkEncoder(__u16 type, __u16 flags) { StartMessage(type, flags); }

  // Starts a new netlink message
  void StartMessage(__u16 type, __u16 flags) {
    Clear();
    nlmsghdr header = {};
    header.nlmsg_type = type;
    // Length is encoded when the message is finalized
    header.nlmsg_len = sizeof(nlmsghdr);
    header.nlmsg_flags = flags;
    header.nlmsg_pid = getpid();
    header.nlmsg_seq = sequence_;
    netlink_header = offset_;
    Write(header);
  }

  // Begins a genetlink message
  void BeginGenetlinkHeader(__u8 cmd) {
    genlmsghdr hdr = {};
    hdr.cmd = cmd;
    hdr.version = 1;
    genetlink_header = offset_;
    Write(hdr);
  }

  // Starts encoding an NLA
  void BeginNla(__u16 type) {
    nlattr attr = {};
    attr.nla_type = type;
    // Updated when the NLA is finalized
    attr.nla_len = 0;
    nla_start = offset_;
    Write(attr);
  }

  // Finishes encoding an NLA
  void EndNla() {
    nlattr attr;
    Read(attr, nla_start);
    attr.nla_len += static_cast<__u16>(offset_ - nla_start);
    Write(attr, nla_start);
  }

  template <typename T>
  void AddRtAttr(uint16_t type, T &attr) {
    Write(rtattr{
        .rta_len = static_cast<uint16_t>(RTA_LENGTH(sizeof(T))),
        .rta_type = type,
    });
    Write(attr);
  }

  // Finalizes the message, allowing it to be sent using sendmsg.
  void Finalize(iovec &out) {
    nlmsghdr hdr;
    Read(hdr, netlink_header);
    out.iov_base = data_.data() + netlink_header;
    hdr.nlmsg_len = static_cast<uint32_t>(offset_);
    out.iov_len = hdr.nlmsg_len;
    sequence_++;
    Write(hdr, netlink_header);
  }

  // Clears the buffer, invalidating any iovecs that were
  // obtained from this encoder.
  void Clear() { offset_ = 0; }

  uint32_t sequence() const { return sequence_; }

 private:
  void Write(const void *data, size_t len) {
    size_t min_size = offset_ + len;
    if (min_size > data_.size()) {
      data_.resize(min_size);
    }
    memcpy(data_.data() + offset_, data, len);
    offset_ += len;
  }
  void Read(void *data, size_t offset, size_t len) { memcpy(data, data_.data() + offset, len); }
  void Write(const void *data, size_t offset, size_t len) {
    memcpy(data_.data() + offset, data, len);
  }
  __u32 sequence_ = 0;
  size_t offset_ = 0;
  size_t nla_start;
  size_t netlink_header;
  size_t genetlink_header;
  std::vector<uint8_t> data_;
};

// An RAII class that handles a memory mapping. The container will ensure the
// mapping is destroyed when the object is deleted.
class ScopedMMap {
 public:
  static fit::result<int, ScopedMMap> MMap(void *addr, size_t length, int prot, int flags, int fd,
                                           off_t offset) {
    void *mapping = mmap(addr, length, prot, flags, fd, offset);
    if (mapping == MAP_FAILED) {
      int error = errno;
      return fit::error(error);
    }
    return fit::ok(ScopedMMap(mapping, length));
  }

  ScopedMMap(const ScopedMMap &) = delete;
  ScopedMMap &operator=(const ScopedMMap &) = delete;

  ScopedMMap(ScopedMMap &&other) noexcept : mapping_(MAP_FAILED), length_(0) {
    *this = std::move(other);
  }

  ~ScopedMMap() { Unmap(); }

  ScopedMMap &operator=(ScopedMMap &&other) noexcept {
    Unmap();
    mapping_ = other.mapping_;
    length_ = other.length_;
    other.mapping_ = MAP_FAILED;
    return *this;
  }

  void Unmap() {
    if (is_valid()) {
      munmap(mapping_, length_);
      mapping_ = MAP_FAILED;
    }
  }

  bool is_valid() const { return mapping_ != MAP_FAILED; }

  explicit operator bool() const { return is_valid(); }

  void *mapping() const { return mapping_; }

 private:
  explicit ScopedMMap(void *mapping, size_t length) : mapping_(mapping), length_(length) {}

  void *mapping_;
  size_t length_;
};

// An RAII class that handles a mount. The container will ensure the
// unmount when the object is deleted.
class ScopedMount {
 public:
  static fit::result<int, ScopedMount> Mount(const std::string &source, const std::string &target,
                                             const std::string &filesystemtype,
                                             unsigned long mountflags = 0,
                                             const void *data = nullptr);
  static fit::result<int, ScopedMount> CreateDirAndMount(const std::string &source,
                                                         const std::string &target,
                                                         const std::string &filesystemtype,
                                                         unsigned long mountflags = 0,
                                                         const void *data = nullptr);
  ScopedMount() = default;
  ScopedMount(const ScopedMount &) = delete;
  ScopedMount &operator=(const ScopedMount &) = delete;
  ScopedMount &operator=(ScopedMount &&other) noexcept;
  ScopedMount(ScopedMount &&other) noexcept;
  ~ScopedMount();

  void Unmount();

 private:
  explicit ScopedMount(std::string target_path);
  std::string target_path_;
  bool is_mounted_ = false;
};

std::optional<size_t> parse_field_in_kb(std::string_view value);

fit::result<int> SetCasefold(const std::string &path, bool enable = true);

// A semaphore implemented with EventFd. Works across Threads and Forks.
class EventFdSem {
 public:
  explicit EventFdSem(int initial_value) {
    fd_ = fbl::unique_fd(eventfd(initial_value, EFD_SEMAPHORE));
  }

  ~EventFdSem() = default;
  EventFdSem(EventFdSem &&other) noexcept = default;
  EventFdSem &operator=(EventFdSem &&other) noexcept = default;

  EventFdSem(const EventFdSem &) = delete;
  EventFdSem &operator=(const EventFdSem &) = delete;

  int Wait() {
    eventfd_t val;
    return static_cast<int>(TEMP_FAILURE_RETRY(eventfd_read(fd_.get(), &val)));
  }

  int Notify(int value) { return eventfd_write(fd_.get(), value); }

 private:
  fbl::unique_fd fd_;
};

// Returns the first memory mapping that matches the given predicate.
std::optional<MemoryMapping> find_memory_mapping(fit::function<bool(const MemoryMapping &)> match,
                                                 std::string_view maps);

std::optional<MemoryMapping> find_memory_mapping(uintptr_t addr, std::string_view maps);

// Same as above, but with info from smaps
std::optional<MemoryMappingExt> find_memory_mapping_ext(
    fit::function<bool(const MemoryMappingExt &)> match, std::string_view maps);

std::optional<MemoryMappingExt> find_memory_mapping_ext(uintptr_t addr, std::string_view maps);

// Returns a random hex string of the given length.
std::string RandomHexString(size_t length);

// Returns true if running with sysadmin capabilities.
bool HasSysAdmin();

// Returns true if running with the given capability.
bool HasCapability(uint32_t cap);

// Returns true if running on Starnix.  This is likely only necessary when there are known bugs.
bool IsStarnix();

// Returns true if reported kernel version is equal or greater than a given one.
bool IsKernelVersionAtLeast(int min_major, int min_minor);

/// Unmount anything mounted at or under `path` and remove it.
void RecursiveUnmountAndRemove(const std::string &path);

// Attempts to read a byte from the given memory address.
// Returns whether the read succeeded or not.
bool TryRead(uintptr_t addr);

// Attempts to write a zero byte to the given memory address.
// Returns whether the write succeeded or not.
bool TryWrite(uintptr_t addr);

// Wrapper for the memfd_create system call.
int MemFdCreate(const char *name, unsigned int flags);

// Wrapper for the pidfd_open system call.
int PidFdOpen(pid_t pid, unsigned int flags);

// Wait until the target process indicates a sleeping (S) or traced sleeping (t) state.
testing::AssertionResult WaitUntilBlocked(pid_t target, bool ignore_tracer);

// Wait until the target process indicates a zombie (Z) state.
testing::AssertionResult WaitUntilZombie(pid_t target);

// Loop until the target task's process state in /proc/<pid>/task/<tid>/stat satisfies the given
// predicate function. The string passed to the predicate is the task state (R, S, T, Z, etc.).
testing::AssertionResult WaitForTaskState(pid_t pid, pid_t tid,
                                          fit::function<bool(std::string_view)> predicate);

enum AccessType { Read, Write };

// Checks whether the provided access segfaults.
testing::AssertionResult TestThatAccessSegfaults(void *test_address, AccessType type);

// An RAII class that handles loop device allocation and attachment.
// Automatically detaches the loop device (LOOP_CLR_FD) on destruction.
class ScopedLoopDevice {
 public:
  static fit::result<int, ScopedLoopDevice> Create(int image_fd);

  ScopedLoopDevice() = default;
  ScopedLoopDevice(ScopedLoopDevice &&other) = default;
  ScopedLoopDevice &operator=(ScopedLoopDevice &&other) noexcept;
  ~ScopedLoopDevice();

  const std::string &path() const { return device_path_; }

 private:
  ScopedLoopDevice(fbl::unique_fd dev_fd, std::string device_path);
  void Detach();

  fbl::unique_fd dev_fd_;
  std::string device_path_;
};

// A RAII container for a pair of file descriptors representing a pipe.
class ScopedPipe {
 public:
  ScopedPipe();
  explicit ScopedPipe(int flags);
  ScopedPipe(ScopedPipe &&o);
  ScopedPipe &operator=(ScopedPipe &&o);
  ScopedPipe(const ScopedPipe &) = delete;
  ScopedPipe &operator=(const ScopedPipe &) = delete;

  const fbl::unique_fd &ReadSide() const { return read_side_; }
  fbl::unique_fd &ReadSide() { return read_side_; }

  const fbl::unique_fd &WriteSide() const { return write_side_; }
  fbl::unique_fd &WriteSide() { return write_side_; }

 private:
  fbl::unique_fd read_side_;
  fbl::unique_fd write_side_;
};

// A means of unblocking some other thread or process that is waiting (or may in the
// future wait) on a corresponding `Holder`.
class Poker {
 public:
  Poker() = default;
  explicit Poker(fbl::unique_fd pipe_write_side);
  Poker(Poker &&o);
  Poker &operator=(Poker &&o);
  Poker(const Poker &) = delete;
  Poker &operator=(const Poker &) = delete;

  bool is_valid() const { return pipe_write_side_.is_valid(); }
  explicit operator bool() const { return is_valid(); }

  void poke();

 private:
  fbl::unique_fd pipe_write_side_;
};

// A means of blocking until some other thread or process uses a corresponding `Poker`
// to indicate that some desired state has been reached and further blocking is no longer
// necessary.
class Holder {
 public:
  Holder() = default;
  explicit Holder(fbl::unique_fd pipe_read_side);
  Holder(Holder &&o);
  Holder &operator=(Holder &&o);
  Holder(const Holder &) = delete;
  Holder &operator=(const Holder &) = delete;

  bool is_valid() const { return pipe_read_side_.is_valid(); }
  explicit operator bool() const { return is_valid(); }

  void hold();

 private:
  fbl::unique_fd pipe_read_side_;
};

// A simple IPC synchronization mechanism for use in multithread and multiprocess
// tests.
struct Rendezvous {
  Poker poker;
  Holder holder;
};

// Constructs a new pipe-backed `Poker`-`Holder` pair for synchronization of two
// threads or processes.
Rendezvous MakeRendezvous();

// Constructs a `Poker`-`Holder` pair from an existing pipe.
Rendezvous MakeRendezvous(ScopedPipe pipe);

// Information about a mount from /proc/[pid]/mountinfo.
// See proc(5) for more details.
struct MountInfo {
  std::string mount_id;
  std::string parent_id;
  std::string major_minor;
  std::string root;
  std::string mount_point;
  std::string mount_options;
  std::vector<std::string> optional_fields;
  std::string fs_type;
  std::string mount_source;
  std::string super_options;
};

// Reads and parses "/proc/self/mountinfo".
fit::result<int, std::vector<MountInfo>> ReadMountInfo();

// Reads "/proc/self/mountinfo" and returns the info for the mount at `path`.
// If no such mount is found, or the file cannot be read, the result will be an empty optional.
std::optional<MountInfo> ReadMountInfoLine(const std::string &path);

// Returns the set of file names in `dir`. `.` and `..` are not included.
// If `dir` does not exist or cannot be opened, returns an error (e.g. ENOENT).
fit::result<int, std::set<std::string>> ListDirectory(const char *dir);

// Polls `/dev/input` for up to 5 seconds until new device entries appear that were
// not present in `ls_before`.
//
// Returns a vector of newly appeared device names. If the poll times out and no
// new devices appeared, the returned vector will be empty. Note that more than one
// device name may be returned if multiple devices appeared.
std::vector<std::string> WaitForDevice(const std::set<std::string> &ls_before);

}  // namespace test_helper

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_TEST_HELPER_H_
