// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/bpf.h"

#include <fcntl.h>
#include <netinet/in.h>
#include <sys/epoll.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <syscall.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <climits>
#include <fstream>
#include <tuple>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#ifndef CAP_BPF
#define CAP_BPF 39
#endif

#define BPF_LOAD_MAP(reg, value)                               \
  bpf_insn{                                                    \
      .code = BPF_LD | BPF_DW,                                 \
      .dst_reg = reg,                                          \
      .src_reg = 1,                                            \
      .off = 0,                                                \
      .imm = static_cast<int32_t>(value),                      \
  },                                                           \
      bpf_insn {                                               \
    .code = 0, .dst_reg = 0, .src_reg = 0, .off = 0, .imm = 0, \
  }

#define BPF_LOAD_OFFSET(dst, src, offset)                                                       \
  bpf_insn {                                                                                    \
    .code = BPF_LDX | BPF_MEM | BPF_W, .dst_reg = dst, .src_reg = src, .off = offset, .imm = 0, \
  }

#define BPF_MOV_IMM(reg, value)                                                    \
  bpf_insn {                                                                       \
    .code = BPF_ALU64 | BPF_MOV | BPF_IMM, .dst_reg = reg, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(value),                                            \
  }

#define BPF_MOV_REG(dst, src)                                                                \
  bpf_insn {                                                                                 \
    .code = BPF_ALU64 | BPF_MOV | BPF_X, .dst_reg = dst, .src_reg = src, .off = 0, .imm = 0, \
  }

#define BPF_ADD_IMM(reg, value)                                                  \
  bpf_insn {                                                                     \
    .code = BPF_ALU64 | BPF_ADD | BPF_K, .dst_reg = reg, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(value),                                          \
  }

#define BPF_CALL_EXTERNAL(function)                                   \
  bpf_insn {                                                          \
    .code = BPF_JMP | BPF_CALL, .dst_reg = 0, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(function),                            \
  }

#define BPF_STORE_B(ptr, value)                                               \
  bpf_insn {                                                                  \
    .code = BPF_ST | BPF_B | BPF_MEM, .dst_reg = ptr, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(value),                                       \
  }

#define BPF_STORE_W(ptr, value)                                               \
  bpf_insn {                                                                  \
    .code = BPF_ST | BPF_W | BPF_MEM, .dst_reg = ptr, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(value),                                       \
  }

#define BPF_RETURN() \
  bpf_insn { .code = BPF_JMP | BPF_EXIT, .dst_reg = 0, .src_reg = 0, .off = 0, .imm = 0, }

#define BPF_JNE_IMM(dst, value, offset)                                             \
  bpf_insn {                                                                        \
    .code = BPF_JMP | BPF_JNE | BPF_K, .dst_reg = dst, .src_reg = 0, .off = offset, \
    .imm = static_cast<int32_t>(value),                                             \
  }

#define BPF_JLT_REG(dst, src, offset)                                                           \
  bpf_insn {                                                                                    \
    .code = BPF_JMP | BPF_JLT | BPF_X, .dst_reg = dst, .src_reg = src, .off = offset, .imm = 0, \
  }

namespace {

int bpf(int cmd, union bpf_attr* attr) { return (int)syscall(__NR_bpf, cmd, attr, sizeof(*attr)); }

void TestMapCreationFail(uint32_t type, uint32_t key_size, uint32_t value_size,
                         uint32_t max_entries, int expected_errno) {
  bpf_attr attr = {
      .map_type = type,
      .key_size = key_size,
      .value_size = value_size,
      .max_entries = max_entries,
  };
  int result = bpf(BPF_MAP_CREATE, &attr);
  EXPECT_EQ(result, -1);
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (errno == EPERM) {
    GTEST_SKIP() << "Permission denied.";
  }
  EXPECT_EQ(errno, expected_errno);
}

TEST(BpfTest, ArraySizeOverflow) {
  TestMapCreationFail(BPF_MAP_TYPE_ARRAY, sizeof(int), 1024, INT_MAX / 8, ENOMEM);
}

TEST(BpfTest, ArraySizeZero) {
  TestMapCreationFail(BPF_MAP_TYPE_ARRAY, sizeof(int), 1024, 0, EINVAL);
}

TEST(BpfTest, HashMapSizeZero) { TestMapCreationFail(BPF_MAP_TYPE_HASH, 1, 1024, 0, EINVAL); }

TEST(BpfTest, HashMapZeroKeySize) { TestMapCreationFail(BPF_MAP_TYPE_HASH, 0, 1024, 10, EINVAL); }

class BpfTestBase : public testing::Test {
 protected:
  void SetUp() override {
    testing::Test::SetUp();

    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "bpf() system call requires CAP_SYS_ADMIN";
    }
  }

  fbl::unique_fd LoadProgram(const bpf_insn* program, size_t len, uint32_t prog_type,
                             uint32_t expected_attach_type) {
    char buffer[4096];
    union bpf_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.prog_type = prog_type;
    attr.expected_attach_type = expected_attach_type;
    attr.insns = reinterpret_cast<uint64_t>(program);
    attr.insn_cnt = static_cast<uint32_t>(len);
    attr.license = reinterpret_cast<uint64_t>("N/A");
    attr.log_buf = reinterpret_cast<uint64_t>(buffer);
    attr.log_size = 4096;
    attr.log_level = 1;

    return fbl::unique_fd(SAFE_SYSCALL(bpf(BPF_PROG_LOAD, &attr)));
  }

  fbl::unique_fd CreateMap(uint32_t type, uint32_t key_size, uint32_t value_size,
                           uint32_t max_entries, uint32_t flags = 0) {
    auto fd = TryCreateMap(type, key_size, value_size, max_entries, flags);
    if (!fd.is_valid()) {
      ADD_FAILURE() << " failed to create eBPF map: " << strerror(errno) << "(" << errno << ")";
    }
    return fd;
  }

  fbl::unique_fd TryCreateMap(uint32_t type, uint32_t key_size, uint32_t value_size,
                              uint32_t max_entries, uint32_t flags = 0) {
    union bpf_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.map_type = type;
    attr.key_size = key_size;
    attr.value_size = value_size;
    attr.max_entries = max_entries;
    attr.map_flags = flags;

    return fbl::unique_fd(bpf(BPF_MAP_CREATE, &attr));
  }

  int BpfGetMapId(const int fd) {
    struct bpf_map_info info = {};
    union bpf_attr attr = {.info = {
                               .bpf_fd = static_cast<uint32_t>(fd),
                               .info_len = sizeof(info),
                               .info = reinterpret_cast<uint64_t>(&info),
                           }};
    int rv = bpf(BPF_OBJ_GET_INFO_BY_FD, &attr);
    if (rv)
      return rv;
    if (attr.info.info_len < offsetof(bpf_map_info, id) + sizeof(info.id)) {
      errno = EOPNOTSUPP;
      return -1;
    };
    return info.id;
  }

  int BpfGetProgId(const int fd) {
    struct bpf_prog_info info = {};
    union bpf_attr attr = {.info = {
                               .bpf_fd = static_cast<uint32_t>(fd),
                               .info_len = sizeof(info),
                               .info = reinterpret_cast<uint64_t>(&info),
                           }};
    int rv = bpf(BPF_OBJ_GET_INFO_BY_FD, &attr);
    if (rv)
      return rv;
    if (attr.info.info_len < offsetof(bpf_prog_info, id) + sizeof(info.id)) {
      errno = EOPNOTSUPP;
      return -1;
    };
    return info.id;
  }
};

// Call fcntl syscall directly. It's used instead of `fcntl()` provided by libc
// because `fcntl()` is not consistent between different versions of libc:
//   - glibc expects `struct flock` in `fcntl(F_OFD_SETLK)`. Internally it
//     converts the argument to `flock64` before calling the syscall.
//   - bionic passes the argument to the syscall directly.
// The syscall itself expects always expect `struct flock64`.
int sys_fcntl(int fd, int op, void* arg) {
  const int NR =
#if defined(__LP64__)
      __NR_fcntl;
#else
      __NR_fcntl64;
#endif
  return static_cast<int>(syscall(NR, fd, op, arg));
}

class BpfMapTest : public BpfTestBase {
 protected:
  static constexpr uint32_t kArrayMapValueSize = 1024;

  void SetUp() override {
    BpfTestBase::SetUp();

    if (IsSkipped())
      return;

    array_fd_ = CreateMap(BPF_MAP_TYPE_ARRAY, sizeof(int), kArrayMapValueSize, 10);
    map_fd_ = CreateMap(BPF_MAP_TYPE_HASH, sizeof(int), sizeof(int), 10);
    ringbuf_fd_ = CreateMap(BPF_MAP_TYPE_RINGBUF, 0, 0, static_cast<uint32_t>(getpagesize()));

    CheckMapInfo();
  }

  void CheckMapInfo() { CheckMapInfo(map_fd_.get()); }

  void CheckMapInfo(int map_fd) {
    struct bpf_map_info map_info;
    bpf_attr attr = {
        .info =
            {
                .bpf_fd = (unsigned)map_fd_.get(),
                .info_len = sizeof(map_info),
                .info = (uintptr_t)&map_info,
            },
    };
    EXPECT_EQ(bpf(BPF_OBJ_GET_INFO_BY_FD, &attr), 0) << strerror(errno);
    EXPECT_EQ(map_info.type, BPF_MAP_TYPE_HASH);
    EXPECT_EQ(map_info.key_size, sizeof(int));
    EXPECT_EQ(map_info.value_size, sizeof(int));
    EXPECT_EQ(map_info.max_entries, 10u);
    EXPECT_EQ(map_info.map_flags, 0u);
  }

  void Pin(int fd, const char* pin_path) {
    unlink(pin_path);
    bpf_attr attr = {
        .pathname = reinterpret_cast<uintptr_t>(pin_path),
        .bpf_fd = static_cast<unsigned>(fd),
    };
    ASSERT_EQ(bpf(BPF_OBJ_PIN, &attr), 0) << strerror(errno);
  }

  fbl::unique_fd BpfLock(fbl::unique_fd fd, short type) {
    if (!fd.is_valid())
      return fd;  // pass any errors straight through
    int map_id = BpfGetMapId(fd.get());
    if (map_id <= 0) {
      EXPECT_GT(map_id, 0);
      return fbl::unique_fd();
    }

    struct flock64 fl = {
        .l_type = type,        // short: F_{RD,WR,UN}LCK
        .l_whence = SEEK_SET,  // short: SEEK_{SET,CUR,END}
        .l_start = map_id,     // off_t: start offset
        .l_len = 1,            // off_t: number of bytes
    };

    int ret = sys_fcntl(fd.get(), F_OFD_SETLK, &fl);
    if (ret != 0) {
      fd.reset();
    }
    return fd;
  }

  fbl::unique_fd BpfFdGet(const char* pathname, uint32_t flag) {
    bpf_attr attr = {
        .pathname = reinterpret_cast<uint64_t>(pathname),
        .file_flags = flag,
    };
    return fbl::unique_fd(bpf(BPF_OBJ_GET, &attr));
  }

  fbl::unique_fd MapRetrieveLocklessRW(const char* pathname) { return BpfFdGet(pathname, 0); }

  fbl::unique_fd MapRetrieveExclusiveRW(const char* pathname) {
    return BpfLock(MapRetrieveLocklessRW(pathname), F_WRLCK);
  }

  fbl::unique_fd MapRetrieveRW(const char* pathname) {
    return BpfLock(MapRetrieveLocklessRW(pathname), F_RDLCK);
  }

  fbl::unique_fd MapRetrieveRO(const char* pathname) { return BpfFdGet(pathname, BPF_F_RDONLY); }

  // WARNING: it's impossible to grab a shared (ie. read) lock on a write-only fd,
  // so we instead choose to grab an exclusive (ie. write) lock.
  fbl::unique_fd MapRetrieveWO(const char* pathname) {
    return BpfLock(BpfFdGet(pathname, BPF_F_WRONLY), F_WRLCK);
  }

  // Run the given bpf program.
  void Run(const bpf_insn* program, size_t len) {
    fbl::unique_fd prog_fd = LoadProgram(program, len, BPF_PROG_TYPE_SOCKET_FILTER, 0);
    int sk[2];
    SAFE_SYSCALL(socketpair(AF_UNIX, SOCK_DGRAM, 0, sk));
    fbl::unique_fd sk0(sk[0]);
    fbl::unique_fd sk1(sk[1]);
    int fd = prog_fd.get();
    SAFE_SYSCALL(setsockopt(sk1.get(), SOL_SOCKET, SO_ATTACH_BPF, &fd, sizeof(int)));
    SAFE_SYSCALL(write(sk0.get(), "", 1));
  }

  void WriteToRingBuffer(char v, uint32_t submit_flag = 0) {
    // A bpf program that write a record in the ringbuffer.
    bpf_insn program[] = {
        // r1 <- ringbuf_fd_
        BPF_LOAD_MAP(1, ringbuf_fd()),
        // r2 <- 1
        BPF_MOV_IMM(2, 1),
        // r3 <- 0
        BPF_MOV_IMM(3, 0),
        // Call bpf_ringbuf_reserve
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_reserve),
        // r0 != 0 -> JMP 1
        BPF_JNE_IMM(0, 0, 1),
        // exit
        BPF_RETURN(),
        // *r0 = `v`
        BPF_STORE_B(0, v),
        // r1 <- r0,
        BPF_MOV_REG(1, 0),
        // r2 <- `submit_flag`,
        BPF_MOV_IMM(2, submit_flag),
        // Call bpf_ringbuf_submit
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_submit),
        // r0 <- 0,
        BPF_MOV_IMM(0, 0),
        // exit
        BPF_RETURN(),
    };
    Run(program, sizeof(program) / sizeof(program[0]));
  }

  // Writes a new message with the specified `size`, which must be a multiple
  // of 4. `v` is written throughout the message.
  void WriteToRingBufferLarge(uint32_t v, size_t size, uint32_t submit_flag = 0) {
    ASSERT_TRUE(size % 4 == 0);

    // A bpf program that write a record in the ringbuffer.
    bpf_insn program[] = {
        // r1 <- ringbuf_fd_
        BPF_LOAD_MAP(1, ringbuf_fd()),
        // r2 <- `size`
        BPF_MOV_IMM(2, size),
        // r3 <- 0
        BPF_MOV_IMM(3, 0),
        // Call bpf_ringbuf_reserve
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_reserve),
        // r0 != 0 -> JMP 1
        BPF_JNE_IMM(0, 0, 1),
        // exit
        BPF_RETURN(),
        // r1 <- r0,
        BPF_MOV_REG(1, 0),
        // r2 <- r0,
        BPF_MOV_REG(2, 0),
        // r2 <- r2 + `size`
        BPF_ADD_IMM(2, size),
        // *r0 = `v`
        BPF_STORE_W(0, v),
        // r0 <- r0 + 4
        BPF_ADD_IMM(0, 4),
        // r0 < r2 -> JMP -3
        BPF_JLT_REG(0, 2, -3),
        // r2 <- `submit_flag`,
        BPF_MOV_IMM(2, submit_flag),
        // Call bpf_ringbuf_submit
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_submit),
        // r0 <- 0,
        BPF_MOV_IMM(0, 0),
        // exit
        BPF_RETURN(),
    };
    Run(program, sizeof(program) / sizeof(program[0]));
  }

  void DiscardWriteToRingBuffer() {
    // A bpf program that cancel a write the ringbuffer.
    bpf_insn program[] = {
        // r1 <- ringbuf_fd_
        BPF_LOAD_MAP(1, ringbuf_fd()),
        // r2 <- 1
        BPF_MOV_IMM(2, 1),
        // r3 <- 0
        BPF_MOV_IMM(3, 0),
        // Call bpf_ringbuf_reserve
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_reserve),
        // r0 != 0 -> JMP 1
        BPF_JNE_IMM(0, 0, 1),
        // exit
        BPF_RETURN(),
        // r1 <- r0,
        BPF_MOV_REG(1, 0),
        // r2 <- 0,
        BPF_MOV_IMM(2, 0),
        // Call bpf_ringbuf_discard
        BPF_CALL_EXTERNAL(BPF_FUNC_ringbuf_discard),
        // r0 <- 0,
        BPF_MOV_IMM(0, 0),
        // exit
        BPF_RETURN(),
    };
    Run(program, sizeof(program) / sizeof(program[0]));
  }

  int array_fd() const { return array_fd_.get(); }
  int map_fd() const { return map_fd_.get(); }
  int ringbuf_fd() const { return ringbuf_fd_.get(); }

 private:
  fbl::unique_fd array_fd_;
  fbl::unique_fd map_fd_;
  fbl::unique_fd ringbuf_fd_;
};

TEST_F(BpfMapTest, Map) {
  const int NUM_VALUES = 2;

  for (int i = 0; i < NUM_VALUES; ++i) {
    int v = i + 1;
    bpf_attr attr = {
        .map_fd = (unsigned)map_fd(),
        .key = (uintptr_t)(&i),
        .value = (uintptr_t)(&v),
    };
    EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), 0) << strerror(errno);
  }

  std::vector<int> keys;
  int next_key;
  int* last_key = nullptr;
  for (;;) {
    bpf_attr attr = {
        .map_fd = (unsigned)map_fd(),
        .key = (uintptr_t)last_key,
        .next_key = (uintptr_t)&next_key,
    };
    int err = bpf(BPF_MAP_GET_NEXT_KEY, &attr);
    if (err < 0 && errno == ENOENT)
      break;
    ASSERT_GE(err, 0) << strerror(errno);
    keys.push_back(next_key);
    last_key = &next_key;
  }
  std::sort(keys.begin(), keys.end());
  EXPECT_EQ(keys.size(), static_cast<size_t>(NUM_VALUES));
  for (int i = 0; i < NUM_VALUES; ++i) {
    EXPECT_EQ(keys[i], i);
  }

  for (int i = 0; i < NUM_VALUES; ++i) {
    int v;
    bpf_attr attr = {
        .map_fd = (unsigned)map_fd(),
        .key = (uintptr_t)(&i),
        .value = (uintptr_t)(&v),
    };
    EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), 0) << strerror(errno);
    EXPECT_EQ(v, i + 1);
  }

  for (int i = 0; i < NUM_VALUES; ++i) {
    bpf_attr attr = {
        .map_fd = (unsigned)map_fd(),
        .key = (uintptr_t)(&i),
    };
    EXPECT_EQ(bpf(BPF_MAP_DELETE_ELEM, &attr), 0) << strerror(errno);
  }

  for (int i = 0; i < NUM_VALUES; ++i) {
    int v;
    bpf_attr attr = {
        .map_fd = (unsigned)map_fd(),
        .key = (uintptr_t)(&i),
        .value = (uintptr_t)(&v),
    };
    EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), -1);
    EXPECT_EQ(errno, ENOENT);
  }

  CheckMapInfo();
}

TEST_F(BpfMapTest, MapWriteOnly) {
  auto map = CreateMap(BPF_MAP_TYPE_ARRAY, sizeof(int), sizeof(int), 10, BPF_F_WRONLY);
  uint32_t x = 0;
  bpf_attr attr = {
      .map_fd = (unsigned)map.get(),
      .key = (uintptr_t)(&x),
      .value = (uintptr_t)(&x),
  };
  EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), 0) << strerror(errno);

  // Lookup and GetNextKey should fail.
  EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), -1);
  EXPECT_EQ(errno, EPERM);

  uint32_t next_key = 0;
  attr = {
      .map_fd = (unsigned)map.get(),
      .key = 0,
      .next_key = (uintptr_t)&next_key,
  };
  EXPECT_EQ(bpf(BPF_MAP_GET_NEXT_KEY, &attr), -1);
  EXPECT_EQ(errno, EPERM);
}

TEST_F(BpfMapTest, MapReadOnly) {
  auto map = CreateMap(BPF_MAP_TYPE_ARRAY, sizeof(int), sizeof(int), 10, BPF_F_RDONLY);
  uint32_t x = 0;
  bpf_attr attr = {
      .map_fd = (unsigned)map.get(),
      .key = (uintptr_t)(&x),
      .value = (uintptr_t)(&x),
  };

  // Update and Delete should fail.
  EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), -1);
  EXPECT_EQ(errno, EPERM);

  EXPECT_EQ(bpf(BPF_MAP_DELETE_ELEM, &attr), -1);
  EXPECT_EQ(errno, EPERM);

  // Lookups should still succeed.
  EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), 0);
}

TEST_F(BpfMapTest, PinMap) {
  const char* pin_path = "/sys/fs/bpf/foo";

  Pin(map_fd(), pin_path);
  EXPECT_EQ(access(pin_path, F_OK), 0) << strerror(errno);

  struct statx statx1;
  ASSERT_EQ(statx(AT_FDCWD, pin_path, 0, STATX_ATIME, &statx1), 0) << strerror(errno);

  // Close the map fd.
  EXPECT_EQ(close(map_fd()), 0);

  // Sleep a bit to allow the atime to change.
  usleep(10000);

  union bpf_attr attr = {.pathname = (uintptr_t)pin_path};
  int map_fd = bpf(BPF_OBJ_GET, &attr);
  ASSERT_GE(map_fd, 0) << strerror(errno);
  CheckMapInfo(map_fd);

  // The atime should have changed.
  struct statx statx2;
  ASSERT_EQ(statx(AT_FDCWD, pin_path, 0, STATX_ATIME, &statx2), 0) << strerror(errno);
  EXPECT_TRUE(statx1.stx_atime.tv_sec < statx2.stx_atime.tv_sec ||
              (statx1.stx_atime.tv_sec == statx2.stx_atime.tv_sec &&
               statx1.stx_atime.tv_nsec < statx2.stx_atime.tv_nsec));
}

TEST_F(BpfMapTest, FreezeMap) {
  // 1. Write an initial value to the map.
  int key = 0;
  std::vector<char> value(kArrayMapValueSize, 'A');
  bpf_attr attr = {
      .map_fd = static_cast<unsigned>(array_fd()),
      .key = reinterpret_cast<uintptr_t>(&key),
      .value = reinterpret_cast<uintptr_t>(value.data()),
  };
  EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), 0) << strerror(errno);

  // 2. Freeze the map.
  attr = {.map_fd = static_cast<unsigned>(array_fd())};
  EXPECT_EQ(bpf(BPF_MAP_FREEZE, &attr), 0) << strerror(errno);

  // 3. Attempt to write to the map again (this should fail).
  std::vector<char> new_value(kArrayMapValueSize, 'B');
  attr = {
      .map_fd = static_cast<unsigned>(array_fd()),
      .key = reinterpret_cast<uintptr_t>(&key),
      .value = reinterpret_cast<uintptr_t>(new_value.data()),
  };
  EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), -1);
  EXPECT_EQ(errno, EPERM);

  // 4. Read the value back to confirm it wasn't changed.
  std::vector<char> read_value(kArrayMapValueSize, 0);
  attr = {
      .map_fd = static_cast<unsigned>(array_fd()),
      .key = reinterpret_cast<uintptr_t>(&key),
      .value = reinterpret_cast<uintptr_t>(read_value.data()),
  };
  EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), 0) << strerror(errno);
  EXPECT_EQ(value, read_value);
}

TEST_F(BpfMapTest, FreezeMmapInteraction) {
  // Create a mappable array map.
  bpf_attr attr = {
      .map_type = BPF_MAP_TYPE_ARRAY,
      .key_size = sizeof(int),
      .value_size = static_cast<uint32_t>(getpagesize()),
      .max_entries = 1,
      .map_flags = BPF_F_MMAPABLE,
  };
  fbl::unique_fd mappable_map_fd(SAFE_SYSCALL_SKIP_ON_EPERM(bpf(BPF_MAP_CREATE, &attr)));
  ASSERT_TRUE(mappable_map_fd.is_valid());

  // 1. Mmap the map, then try to freeze it. Should fail with EBUSY.
  {
    auto mapping = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, mappable_map_fd.get(), 0));

    attr = {.map_fd = static_cast<unsigned>(mappable_map_fd.get())};
    EXPECT_EQ(bpf(BPF_MAP_FREEZE, &attr), -1);
    EXPECT_EQ(errno, EBUSY);
  }

  // 2. Now that it is unmapped, freezing should succeed.
  EXPECT_EQ(bpf(BPF_MAP_FREEZE, &attr), 0) << strerror(errno);

  // 3. Try to mmap the frozen map. Should fail with EPERM.
  auto mmap_rw_result = test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, mappable_map_fd.get(), 0);
  ASSERT_TRUE(mmap_rw_result.is_error());
  EXPECT_EQ(mmap_rw_result.error_value(), EPERM);

  // Also check read-only mmap
  auto mmap_ro_result = test_helper::ScopedMMap::MMap(nullptr, getpagesize(), PROT_READ, MAP_SHARED,
                                                      mappable_map_fd.get(), 0);
  ASSERT_TRUE(mmap_ro_result.is_error());
  EXPECT_EQ(mmap_ro_result.error_value(), EPERM);
}

TEST_F(BpfMapTest, LockTest) {
  const char* m1 = "/sys/fs/bpf/array";
  const char* m2 = "/sys/fs/bpf/map";

  Pin(array_fd(), m1);
  Pin(map_fd(), m2);

  fbl::unique_fd fd0 = MapRetrieveExclusiveRW(m1);
  ASSERT_TRUE(fd0.is_valid());
  fbl::unique_fd fd1 = MapRetrieveExclusiveRW(m2);
  ASSERT_TRUE(fd1.is_valid());  // no conflict with fd0
  fbl::unique_fd fd2 = MapRetrieveExclusiveRW(m2);
  ASSERT_FALSE(fd2.is_valid());  // busy due to fd1
  fbl::unique_fd fd3 = MapRetrieveRO(m2);
  ASSERT_TRUE(fd3.is_valid());  // no lock taken
  fbl::unique_fd fd4 = MapRetrieveRW(m2);
  ASSERT_FALSE(fd4.is_valid());  // busy due to fd1
  fd1.reset();                   // releases exclusive lock
  fbl::unique_fd fd5 = MapRetrieveRO(m2);
  ASSERT_TRUE(fd5.is_valid());  // no lock taken
  fbl::unique_fd fd6 = MapRetrieveRW(m2);
  ASSERT_TRUE(fd6.is_valid());  // now ok
  fbl::unique_fd fd7 = MapRetrieveRO(m2);
  ASSERT_TRUE(fd7.is_valid());  // no lock taken
  fbl::unique_fd fd8 = MapRetrieveExclusiveRW(m2);
  ASSERT_FALSE(fd8.is_valid());  // busy due to fd6
}

// Verify BPF_GET_OBJ behavior on a regular file.
TEST_F(BpfMapTest, GetObjFile) {
  test_helper::ScopedTempDir temp_dir;
  std::string file_name = temp_dir.path() + "/file";
  {
    std::ofstream file(file_name);
    file << "test";
    ASSERT_TRUE(file.is_open());
  }

  fbl::unique_fd file_fd(BpfFdGet(file_name.c_str(), BPF_F_RDONLY));
  ASSERT_FALSE(file_fd.is_valid());
  EXPECT_EQ(errno, EPERM);
}

// Verify that BPF_OBJ_GET checks file permissions.
TEST_F(BpfMapTest, PinAccessCheck) {
  const char* array_path = "/sys/fs/bpf/array_access";
  Pin(array_fd(), array_path);

  const uid_t TEST_UID = 32;

  EXPECT_EQ(chmod(array_path, 0770), 0) << strerror(errno);
  EXPECT_EQ(chown(array_path, TEST_UID, TEST_UID), 0) << strerror(errno);

  EXPECT_EQ(setegid(TEST_UID), 0) << strerror(errno);
  EXPECT_EQ(seteuid(TEST_UID), 0) << strerror(errno);

  fbl::unique_fd map_fd(BpfFdGet(array_path, BPF_F_RDONLY));
  EXPECT_TRUE(map_fd.is_valid()) << strerror(errno);

  // Reset permissions and try opening the same file again. It should fail.
  EXPECT_EQ(chmod(array_path, 0), 0) << strerror(errno);
  map_fd = fbl::unique_fd(BpfFdGet(array_path, BPF_F_RDONLY));
  EXPECT_FALSE(map_fd.is_valid());

  // Restore original uid.
  EXPECT_EQ(setresuid(0, 0, 0), 0) << strerror(errno);
  EXPECT_EQ(setresgid(0, 0, 0), 0) << strerror(errno);
}

TEST_F(BpfMapTest, ArrayEpoll) {
  fbl::unique_fd epollfd(SAFE_SYSCALL(epoll_create(1)));

  struct epoll_event ev;
  ev.events = EPOLLIN | EPOLLET;

  SAFE_SYSCALL(epoll_ctl(epollfd.get(), EPOLL_CTL_ADD, array_fd(), &ev));
  ASSERT_EQ(epoll_wait(epollfd.get(), &ev, 1, 0), 1);
  ASSERT_EQ(ev.events, EPOLLERR);
}

TEST_F(BpfMapTest, ArraySelect) {
  {
    fd_set readfds = {};
    fd_set writefds = {};
    FD_SET(array_fd(), &readfds);
    ASSERT_EQ(select(FD_SETSIZE, &readfds, &writefds, nullptr, nullptr), 1);
    ASSERT_TRUE(FD_ISSET(array_fd(), &readfds));
    ASSERT_FALSE(FD_ISSET(array_fd(), &writefds));
  }

  {
    fd_set readfds = {};
    fd_set writefds = {};
    FD_SET(array_fd(), &readfds);
    FD_SET(array_fd(), &writefds);
    ASSERT_EQ(select(FD_SETSIZE, &readfds, &writefds, nullptr, nullptr), 2);
    ASSERT_TRUE(FD_ISSET(array_fd(), &readfds));
    ASSERT_TRUE(FD_ISSET(array_fd(), &writefds));
  }
}

TEST_F(BpfMapTest, HashMapEpoll) {
  fbl::unique_fd epollfd(SAFE_SYSCALL(epoll_create(1)));

  struct epoll_event ev;
  ev.events = EPOLLIN | EPOLLET;

  SAFE_SYSCALL(epoll_ctl(epollfd.get(), EPOLL_CTL_ADD, map_fd(), &ev));
  ASSERT_EQ(epoll_wait(epollfd.get(), &ev, 1, 0), 1);
  ASSERT_EQ(ev.events, EPOLLERR);
}

TEST_F(BpfMapTest, MMapRingBufTest) {
  // Can map the first page of the ringbuffer R/W
  ASSERT_THAT(test_helper::ScopedMMap::MMap(nullptr, getpagesize(), PROT_READ | PROT_WRITE,
                                            MAP_SHARED, ringbuf_fd(), 0),
              SyscallResultIsOk());
  // Cannot mmap the second page of the ringbuffer R/W
  auto mmap_2nd_page_rw_result = test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), getpagesize());
  ASSERT_THAT(mmap_2nd_page_rw_result, SyscallResultIsErrno(EPERM));
  // Cannot mmap the 3rd or 4th page of the ringbuffer R/W
  auto mmap_3rd_page_rw_result = test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 2 * getpagesize());
  ASSERT_THAT(mmap_3rd_page_rw_result, SyscallResultIsErrno(EPERM));
  // Cannot mmap multiple pages starting from 0 R/W
  auto mmap_2_pages_rw_result = test_helper::ScopedMMap::MMap(
      nullptr, 2 * getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 0);
  ASSERT_THAT(mmap_2_pages_rw_result, SyscallResultIsErrno(EPERM));

  // Can mmap the second page, 3rd and 4th page RO
  for (int i = 0; i < 3; ++i) {
    auto mmap_ro = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), (i + 1) * getpagesize()));
    // Cannot elevate permissions of read-only pages to R/W with mprotect
    EXPECT_EQ(mprotect(mmap_ro.mapping(), getpagesize(), PROT_READ | PROT_WRITE), -1);
    EXPECT_EQ(errno, EACCES);
  }

  // The full ringbuffer (consumer + producer + double-mapped data pages) can be
  // mapped read-only. mprotect(PROT_WRITE) is rejected  with EACCES for the
  // entire mapping as well as for individual pages, including page 0.
  auto mmap_4_pages_ro = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 4 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), 0));
  EXPECT_EQ(mprotect(mmap_4_pages_ro.mapping(), getpagesize(), PROT_READ | PROT_WRITE), -1);
  EXPECT_EQ(errno, EACCES);
  EXPECT_EQ(mprotect(mmap_4_pages_ro.mapping(), 4 * getpagesize(), PROT_READ | PROT_WRITE), -1);
  EXPECT_EQ(errno, EACCES);

  // If page 0 is mapped as RO from the start, it cannot be elevated to RW via mprotect.
  auto mmap_page0_ro = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), 0));
  EXPECT_EQ(mprotect(mmap_page0_ro.mapping(), getpagesize(), PROT_READ | PROT_WRITE), -1);
  EXPECT_EQ(errno, EACCES);
  // Cannot mmap 5 pages.
  auto mmap_5_pages_ro_result = test_helper::ScopedMMap::MMap(nullptr, 5 * getpagesize(), PROT_READ,
                                                              MAP_SHARED, ringbuf_fd(), 0);
  ASSERT_THAT(mmap_5_pages_ro_result, SyscallResultIsErrno(EINVAL));
}

TEST_F(BpfMapTest, MMapRingBufMremapNoWriteEscalation) {
  // 1. Map the consumer page (offset 0, 1 page) with PROT_READ | PROT_WRITE.
  auto consumer_mmap = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 0));

  // 2. Also map the full ring buffer read-only to observe the producer position.
  auto ro_mmap = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 4 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), 0));

  // 3. Attempt to mremap the consumer page to grow across the producer page.
  // In Linux and Starnix, expanding a DONT_EXPAND mapping fails with EFAULT.
  void* remapped =
      mremap(consumer_mmap.mapping(), getpagesize(), 2 * getpagesize(), MREMAP_MAYMOVE);
  EXPECT_EQ(remapped, MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Ensure producer position was not corrupted.
  std::atomic<unsigned long>* producer_pos = reinterpret_cast<std::atomic<unsigned long>*>(
      reinterpret_cast<uintptr_t>(ro_mmap.mapping()) + getpagesize());
  EXPECT_EQ(producer_pos->load(std::memory_order_acquire), 0u);
}

TEST_F(BpfMapTest, MMapArrayMremapFails) {
  bpf_attr attr = {
      .map_type = BPF_MAP_TYPE_ARRAY,
      .key_size = sizeof(int),
      .value_size = static_cast<uint32_t>(getpagesize()),
      .max_entries = 1,
      .map_flags = BPF_F_MMAPABLE,
  };
  fbl::unique_fd mappable_map_fd(SAFE_SYSCALL_SKIP_ON_EPERM(bpf(BPF_MAP_CREATE, &attr)));
  ASSERT_TRUE(mappable_map_fd.is_valid());

  auto mapping = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, mappable_map_fd.get(), 0));

  // Attempt to mremap the array map mapping to grow.
  // In Linux and Starnix, expanding a DONT_EXPAND mapping fails with EFAULT.
  void* remapped = mremap(mapping.mapping(), getpagesize(), 2 * getpagesize(), MREMAP_MAYMOVE);
  EXPECT_EQ(remapped, MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);
}

TEST_F(BpfMapTest, WriteRingBufTest) {
  auto pagewr = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 0));
  auto pagero = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 3 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), getpagesize()));
  std::atomic<unsigned long>* consumer_pos =
      static_cast<std::atomic<unsigned long>*>(pagewr.mapping());
  std::atomic<unsigned long>* producer_pos =
      static_cast<std::atomic<unsigned long>*>(pagero.mapping());
  uint8_t* data = static_cast<uint8_t*>(pagero.mapping()) + getpagesize();
  ASSERT_EQ(0u, consumer_pos->load(std::memory_order_acquire));
  ASSERT_EQ(0u, producer_pos->load(std::memory_order_acquire));

  WriteToRingBuffer(42);

  ASSERT_EQ(0u, consumer_pos->load(std::memory_order_acquire));
  ASSERT_EQ(16u, producer_pos->load(std::memory_order_acquire));

  uint32_t record_length = *reinterpret_cast<uint32_t*>(data);
  ASSERT_EQ(0u, record_length & BPF_RINGBUF_BUSY_BIT);
  ASSERT_EQ(0u, record_length & BPF_RINGBUF_DISCARD_BIT);
  ASSERT_EQ(1u, record_length);

  uint32_t page_offset = *reinterpret_cast<uint32_t*>(data + 4);
  ASSERT_EQ(3u, page_offset);

  uint8_t record_value = *(data + 8);
  ASSERT_EQ(42u, record_value);

  DiscardWriteToRingBuffer();

  ASSERT_EQ(0u, consumer_pos->load(std::memory_order_acquire));
  ASSERT_EQ(32u, producer_pos->load(std::memory_order_acquire));

  record_length = *reinterpret_cast<uint32_t*>(data + 16);
  ASSERT_EQ(0u, record_length & BPF_RINGBUF_BUSY_BIT);
  ASSERT_EQ(BPF_RINGBUF_DISCARD_BIT, record_length & BPF_RINGBUF_DISCARD_BIT);
}

TEST_F(BpfMapTest, IdenticalPagesRingBufTest) {
  // Map the last 2 pages, and check that they are the same after some operations on the buffer.
  auto pages = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 2 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), 2 * getpagesize()));

  uint8_t* page1 = static_cast<uint8_t*>(pages.mapping());
  uint8_t* page2 = page1 + getpagesize();

  // Check that the pages are equal after creating the buffer.
  ASSERT_EQ(memcmp(page1, page2, getpagesize()), 0);

  for (size_t i = 0; i < 256; ++i) {
    WriteToRingBuffer(static_cast<uint8_t>(i));
  }

  // Check that they are still equals after some operations.
  ASSERT_EQ(memcmp(page1, page2, getpagesize()), 0);
}

TEST_F(BpfMapTest, RingBufferWrapAround) {
  auto pagewr = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 0));
  auto pagero = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 3 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), getpagesize()));
  std::atomic<uint32_t>* consumer_pos = static_cast<std::atomic<uint32_t>*>(pagewr.mapping());
  std::atomic<uint32_t>* producer_pos = static_cast<std::atomic<uint32_t>*>(pagero.mapping());
  uint8_t* data = static_cast<uint8_t*>(pagero.mapping()) + getpagesize();
  ASSERT_EQ(0u, consumer_pos->load(std::memory_order_acquire));
  ASSERT_EQ(0u, producer_pos->load(std::memory_order_acquire));

  struct TestMessage {
    size_t size;
    uint32_t value;
  } messages[] = {
      {
          .size = 3016,
          .value = 0x6143abcd,
      },
      // Wrap-around in the middle of the message blob.
      {
          .size = 2072,
          .value = 0xc23414f2,
      },
      {
          .size = 3072,
          .value = 0x12345678,
      },
      // Wrap-around at the edge between message header and the message itself
      // (8 + 3016 + 8 + 2072 + 8 + 3072 + 8 = 8192).
      {
          .size = 128,
          .value = 0xdeadbeef,
      },
  };

  for (const auto& message : messages) {
    // Queue a message.
    WriteToRingBufferLarge(message.value, message.size);
    uint32_t* msg_ptr =
        reinterpret_cast<uint32_t*>(data + (consumer_pos->load() + 8) % getpagesize());
    for (size_t i = 0; i < message.size / 4; ++i) {
      ASSERT_EQ(msg_ptr[i], message.value);
    }
    // Consume the message.
    uint32_t pos = producer_pos->load(std::memory_order_acquire);
    consumer_pos->store(pos, std::memory_order_release);
  }
}

TEST_F(BpfMapTest, NotificationsRingBufTest) {
  auto pagewr = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, getpagesize(), PROT_READ | PROT_WRITE, MAP_SHARED, ringbuf_fd(), 0));
  auto pagero = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 3 * getpagesize(), PROT_READ, MAP_SHARED, ringbuf_fd(), getpagesize()));
  std::atomic<unsigned long>* consumer_pos =
      static_cast<std::atomic<unsigned long>*>(pagewr.mapping());
  std::atomic<unsigned long>* producer_pos =
      static_cast<std::atomic<unsigned long>*>(pagero.mapping());

  fbl::unique_fd epollfd(SAFE_SYSCALL(epoll_create(1)));

  struct epoll_event ev;
  ev.events = EPOLLIN | EPOLLET;

  SAFE_SYSCALL(epoll_ctl(epollfd.get(), EPOLL_CTL_ADD, ringbuf_fd(), &ev));

  // First wait should return no result.
  ASSERT_EQ(0, epoll_wait(epollfd.get(), &ev, 1, 0));

  // After a normal write, epoll should return an event.
  WriteToRingBuffer(42);
  ASSERT_EQ(1, epoll_wait(epollfd.get(), &ev, 1, 0));

  // But only once, as this is edge triggered.
  ASSERT_EQ(0, epoll_wait(epollfd.get(), &ev, 1, 0));

  // A new write will not trigger an event, as the reader is late.
  WriteToRingBuffer(42);
  ASSERT_EQ(0, epoll_wait(epollfd.get(), &ev, 1, 0));

  // Unless the event is forced through flags
  WriteToRingBuffer(42, BPF_RB_FORCE_WAKEUP);
  ASSERT_EQ(1, epoll_wait(epollfd.get(), &ev, 1, 0));

  // Let's catch up.
  consumer_pos->store(producer_pos->load(std::memory_order_acquire), std::memory_order_release);

  // Execute a write, preventing an event to run.
  WriteToRingBuffer(42, BPF_RB_NO_WAKEUP);
  ASSERT_EQ(0, epoll_wait(epollfd.get(), &ev, 1, 0));

  // A normal write will now not send an event because the client has not caught
  // up.
  WriteToRingBuffer(42);
  ASSERT_EQ(0, epoll_wait(epollfd.get(), &ev, 1, 0));

  // Let's catch up again.
  consumer_pos->store(producer_pos->load(std::memory_order_acquire), std::memory_order_release);

  // A normal write will now send an event.
  WriteToRingBuffer(42);
  EXPECT_EQ(1, epoll_wait(epollfd.get(), &ev, 1, 0));
}

TEST_F(BpfMapTest, LpmTrieNoFlag) {
  // LPM trie creation without `BPF_F_NO_PREALLOC` should fail.
  auto fd = TryCreateMap(BPF_MAP_TYPE_LPM_TRIE, 20, 4, 10, 0);
  EXPECT_FALSE(fd.is_valid());
  EXPECT_EQ(errno, EINVAL);
}

TEST_F(BpfMapTest, LpmTrie) {
  struct LpmKey {
    uint32_t prefix_len;
    uint32_t data;
  };
  fbl::unique_fd trie_fd =
      CreateMap(BPF_MAP_TYPE_LPM_TRIE, sizeof(LpmKey), sizeof(uint32_t), 10, BPF_F_NO_PREALLOC);

  struct KeyValue {
    LpmKey key;
    uint32_t value;
  };

  KeyValue entries[3] = {
      {
          .key =
              {
                  .prefix_len = 8,
                  .data = 0x0000000a,
              },
          .value = 1,
      },
      {
          .key =
              {
                  .prefix_len = 24,
                  .data = 0x000a000a,
              },
          .value = 2,
      },

      {
          .key =
              {
                  .prefix_len = 32,
                  .data = 0x7b0a000a,
              },
          .value = 3,
      },
  };

  for (const auto& entry : entries) {
    bpf_attr attr = {
        .map_fd = (unsigned)trie_fd.get(),
        .key = (uintptr_t)(&entry.key),
        .value = (uintptr_t)(&entry.value),
    };
    EXPECT_EQ(bpf(BPF_MAP_UPDATE_ELEM, &attr), 0) << strerror(errno);
  }

  KeyValue tests[3] = {
      {
          .key =
              {
                  .prefix_len = 32,
                  .data = 0x7b0a000a,
              },
          .value = 3,
      },
      {
          .key =
              {
                  .prefix_len = 32,
                  .data = 0xc80a000a,
              },
          .value = 2,
      },
      {
          .key =
              {
                  .prefix_len = 32,
                  .data = 0x01000c0a,
              },
          .value = 1,
      },
  };

  for (const auto& test : tests) {
    uint32_t value;
    bpf_attr attr = {
        .map_fd = (unsigned)trie_fd.get(),
        .key = (uintptr_t)(&test.key),
        .value = (uintptr_t)(&value),
    };
    EXPECT_EQ(bpf(BPF_MAP_LOOKUP_ELEM, &attr), 0) << strerror(errno);
    EXPECT_EQ(value, test.value);
  }
}

TEST_F(BpfMapTest, CannotUpdatePinnedMapWithReadOnlyFd) {
  const char* pin_path = "/sys/fs/bpf/my_map_perms";

  // Pin the array map that was created during test setup.
  Pin(array_fd(), pin_path);

  // Get a read-only file descriptor for the pinned map via BPF_OBJ_GET using the BPF_F_RDONLY flag.
  fbl::unique_fd ro_map_fd = MapRetrieveRO(pin_path);
  ASSERT_TRUE(ro_map_fd.is_valid());

  // Try to update an element in the map using the read-only file descriptor. This should fail with
  // EPERM.
  uint32_t key = 0;
  uint32_t value = 42;
  union bpf_attr update_attr = {};
  update_attr.map_fd = static_cast<uint32_t>(ro_map_fd.get());
  update_attr.key = reinterpret_cast<uint64_t>(&key);
  update_attr.value = reinterpret_cast<uint64_t>(&value);
  update_attr.flags = BPF_ANY;

  EXPECT_THAT(bpf(BPF_MAP_UPDATE_ELEM, &update_attr), SyscallFailsWithErrno(EPERM));

  // Clean up.
  EXPECT_THAT(unlink(pin_path), SyscallSucceeds());
}

TEST_F(BpfMapTest, CannotLookUpPinnedMapWithWriteOnlyFd) {
  const char* pin_path = "/sys/fs/bpf/my_map_perms";

  // Pin the array map that was created during test setup.
  Pin(array_fd(), pin_path);

  // Get a write-only file descriptor for the pinned map via BPF_OBJ_GET using the BPF_F_WRONLY
  // flag.
  fbl::unique_fd wo_map_fd = BpfFdGet(pin_path, BPF_F_WRONLY);
  ASSERT_TRUE(wo_map_fd.is_valid());

  // Try to look up an element in the map using the write-only file descriptor. This should fail
  // with EPERM.
  uint32_t key = 0;
  std::vector<uint8_t> value(kArrayMapValueSize);
  union bpf_attr lookup_attr = {};
  lookup_attr.map_fd = static_cast<uint32_t>(wo_map_fd.get());
  lookup_attr.key = reinterpret_cast<uint64_t>(&key);
  lookup_attr.value = reinterpret_cast<uint64_t>(value.data());

  EXPECT_THAT(bpf(BPF_MAP_LOOKUP_ELEM, &lookup_attr), SyscallFailsWithErrno(EPERM));

  // Clean up.
  EXPECT_THAT(unlink(pin_path), SyscallSucceeds());
}

TEST_F(BpfMapTest, ObjGetInfoByFdOnlyWritesRequestedSize) {
  // Allocate two pages, and make the second page inaccessible to catch out-of-bounds writes.
  size_t page_size = getpagesize();
  void* mem = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mem, MAP_FAILED);
  ASSERT_EQ(mprotect((char*)mem + page_size, page_size, PROT_NONE), 0);

  // Create a user struct with byte size 8, smaller than the full info struct.
  // Place it at the very end of the first page.
  struct my_bpf_attr {
    uint32_t bpf_fd;
    uint32_t info_len;
  } __attribute__((aligned(8)));

  if (sizeof(struct my_bpf_attr) >= sizeof(((union bpf_attr*)0)->info)) {
    GTEST_SKIP() << "my_bpf_attr must be smaller than the full info struct to test overflow";
  }

  struct my_bpf_attr* info_attr =
      (struct my_bpf_attr*)((char*)mem + page_size - sizeof(struct my_bpf_attr));
  info_attr->bpf_fd = static_cast<uint32_t>(array_fd());
  info_attr->info_len = 0;  // Don't need to provide an info buffer

  // Call BPF_OBJ_GET_INFO_BY_FD with size 8. Since the struct is placed at the end of the
  // first page and the second page is inaccessible, any access beyond the requested 8 bytes will
  // cause a fault.
  int ret = static_cast<int>(
      syscall(SYS_bpf, BPF_OBJ_GET_INFO_BY_FD, info_attr, sizeof(struct my_bpf_attr)));

  EXPECT_THAT(ret, SyscallSucceeds())
      << "BPF_OBJ_GET_INFO_BY_FD accessed memory beyond the requested size";

  munmap(mem, page_size * 2);
}

class BpfMapCapabilityTest
    : public BpfTestBase,
      public ::testing::WithParamInterface<std::tuple<uint32_t, bool, bool, bool, bool>> {
 protected:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "bpf() system call requires CAP_SYS_ADMIN";
    }
  }

  void TearDown() override {
    if (restore_unpriv_bpf_disabled) {
      SetUnprivilegedBpfDisabled(restore_unpriv_bpf_disabled.value());
    }
  }

  void GetUnprivilegedBpfDisabled(int* out_result) {
    std::ifstream file("/proc/sys/kernel/unprivileged_bpf_disabled");
    ASSERT_TRUE(file.is_open());
    ASSERT_TRUE(file >> *out_result);
  }

  void SetUnprivilegedBpfDisabled(int value) {
    std::ofstream file("/proc/sys/kernel/unprivileged_bpf_disabled");
    ASSERT_TRUE(file.is_open());
    ASSERT_TRUE(file << value);
  }

  std::optional<int> restore_unpriv_bpf_disabled;
};

TEST_P(BpfMapCapabilityTest, MapCreation) {
  auto [map_type, unpriv_bpf_disabled, has_bpf, has_net_admin, has_sys_admin] = GetParam();

  bool is_starnix = test_helper::IsStarnix();
  if (!is_starnix) {
    if (map_type == BPF_MAP_TYPE_SK_STORAGE) {
      GTEST_SKIP() << "Linux requires BTF for SK_STORAGE maps";
    }
  }

  // `unprivileged_bpf_disabled` can be set to 0, 1, or 2. Unprivileged BPF
  // access is disabled when the value is either 1 or 2, but 1 also prohibits
  // changing it.
  int current_unpriv_bpf_disabled = {};
  ASSERT_NO_FATAL_FAILURE(GetUnprivilegedBpfDisabled(&current_unpriv_bpf_disabled));

  // True if we need to update `unprivileged_bpf_disabled` for this test.
  bool update_unpriv_bpf_disabled = unpriv_bpf_disabled != (current_unpriv_bpf_disabled != 0);

  // If `unprivileged_bpf_disabled` is 1 then we cannot update it, which means
  // we may need to skip the test.
  if (current_unpriv_bpf_disabled == 1 && update_unpriv_bpf_disabled) {
    GTEST_SKIP() << "/proc/sys/kernel/unprivileged_bpf_disabled is set to 1";
  }

  if (update_unpriv_bpf_disabled) {
    ASSERT_NO_FATAL_FAILURE(SetUnprivilegedBpfDisabled(unpriv_bpf_disabled ? 2 : 0));
    restore_unpriv_bpf_disabled = current_unpriv_bpf_disabled;
  }

  // Determine expected success.
  bool cap_bpf_always = (map_type == BPF_MAP_TYPE_LPM_TRIE || map_type == BPF_MAP_TYPE_LRU_HASH ||
                         map_type == BPF_MAP_TYPE_SK_STORAGE);
  bool requires_bpf = cap_bpf_always || unpriv_bpf_disabled;
  bool has_bpf_req = !requires_bpf || has_bpf || has_sys_admin;

  bool requires_net_admin =
      (map_type == BPF_MAP_TYPE_DEVMAP || map_type == BPF_MAP_TYPE_DEVMAP_HASH);
  bool has_net_admin_req = !requires_net_admin || has_net_admin || has_sys_admin;

  bool expect_success = has_bpf_req && has_net_admin_req;

  // Some maps need specific flags or sizes.
  uint32_t key_size = 4;
  uint32_t value_size = 8;
  uint32_t max_entries = 32;
  uint32_t map_flags = 0;

  if (map_type == BPF_MAP_TYPE_LPM_TRIE) {
    key_size = 8;  // Needs to be at least 8 for LPM_TRIE (prefixlen + key)
    map_flags = BPF_F_NO_PREALLOC;
  } else if (map_type == BPF_MAP_TYPE_RINGBUF) {
    key_size = 0;
    value_size = 0;
    max_entries = 4096;
  } else if (map_type == BPF_MAP_TYPE_SK_STORAGE) {
    key_size = 4;
    max_entries = 0;
    map_flags = BPF_F_NO_PREALLOC;
  }

  // On some versions of Linux SK_STORAGE maps may need CAP_NET_ADMIN even with
  // CAP_SYS_ADMIN.
  bool may_fail = !test_helper::IsStarnix() && !has_net_admin && has_sys_admin &&
                  map_type == BPF_MAP_TYPE_DEVMAP_HASH;

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    if (!has_bpf) {
      test_helper::UnsetCapabilityEffective(CAP_BPF);
    }
    if (!has_net_admin) {
      test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    }
    if (!has_sys_admin) {
      test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);
    }

    auto fd = TryCreateMap(map_type, key_size, value_size, max_entries, map_flags);
    if (may_fail) {
      EXPECT_THAT(fd.get(), AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EPERM)));
    } else if (expect_success) {
      EXPECT_THAT(fd.get(), SyscallSucceeds());
    } else {
      EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EPERM));
    }
  });
}

INSTANTIATE_TEST_SUITE_P(
    BpfMapCapabilityTests, BpfMapCapabilityTest,
    ::testing::Combine(::testing::Values(BPF_MAP_TYPE_HASH, BPF_MAP_TYPE_RINGBUF,
                                         BPF_MAP_TYPE_SK_STORAGE, BPF_MAP_TYPE_LPM_TRIE,
                                         BPF_MAP_TYPE_LRU_HASH, BPF_MAP_TYPE_DEVMAP_HASH),
                       ::testing::Bool(), ::testing::Bool(), ::testing::Bool(), ::testing::Bool()));

class BpfCgroupTest : public BpfTestBase {
 protected:
  const uint16_t BLOCKED_PORT = 1236;

  void SetUp() override {
    ASSERT_FALSE(temp_dir_.path().empty());
    int mount_result = mount(nullptr, temp_dir_.path().c_str(), "cgroup2", 0, nullptr);
    if (mount_result == -1 && errno == EPERM) {
      GTEST_SKIP() << "Can't mount cgroup2.";
    }
    ASSERT_EQ(mount_result, 0);

    root_cgroup_.reset(open(temp_dir_.path().c_str(), O_RDONLY));
    assert(root_cgroup_);
  }

  fbl::unique_fd LoadBlockPortProgram(uint32_t expected_attach_type) {
    // A bpf program that blocks bind on 42.
    bpf_insn program[] = {
        // r0 <- [r1+24] (bpf_sock_addr.user_port)
        BPF_LOAD_OFFSET(0, 1, offsetof(bpf_sock_addr, user_port)),
        // r0 != BLOCKED_PORT -> JMP 2
        BPF_JNE_IMM(0, htons(BLOCKED_PORT), 2),

        // r0 <- 0
        BPF_MOV_IMM(0, 0),
        // exit
        BPF_RETURN(),

        // r0 <- 1
        BPF_MOV_IMM(0, 1),
        // exit
        BPF_RETURN(),
    };

    return LoadProgram(program, sizeof(program) / sizeof(program[0]),
                       BPF_PROG_TYPE_CGROUP_SOCK_ADDR, expected_attach_type);
  }

  fbl::unique_fd LoadCgroupSkbProgram(uint32_t expected_attach_type) {
    bpf_insn program[] = {
        BPF_MOV_IMM(0, 1),
        BPF_RETURN(),
    };

    return LoadProgram(program, sizeof(program) / sizeof(program[0]), BPF_PROG_TYPE_CGROUP_SKB,
                       expected_attach_type);
  }

  int TryAttachToRootCgroup(uint32_t attach_type, int prog_fd) {
    bpf_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.target_fd = root_cgroup_.get();
    attr.attach_bpf_fd = prog_fd;
    attr.attach_type = attach_type;
    return bpf(BPF_PROG_ATTACH, &attr);
  }

  void AttachToRootCgroup(uint32_t attach_type, int prog_fd) {
    ASSERT_THAT(TryAttachToRootCgroup(attach_type, prog_fd), SyscallSucceeds());
  }

  int TryDetachFromRootCgroup(uint32_t attach_type) {
    bpf_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.target_fd = root_cgroup_.get();
    attr.attach_type = attach_type;
    return bpf(BPF_PROG_DETACH, &attr);
  }

  void DetachFromRootCgroup(uint32_t attach_type) {
    ASSERT_THAT(TryDetachFromRootCgroup(attach_type), SyscallSucceeds());
  }

  testing::AssertionResult TryBind(uint16_t port, int expected_errno) {
    fbl::unique_fd sock(socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP));
    if (!sock) {
      return testing::AssertionFailure() << "socket failed: " << strerror(errno);
    }
    sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons(port),
        .sin_addr =
            {
                .s_addr = htonl(INADDR_LOOPBACK),
            },
    };
    int r = bind(sock.get(), reinterpret_cast<sockaddr*>(&addr), sizeof(addr));
    if (expected_errno) {
      if (r != -1) {
        return testing::AssertionFailure() << "bind succeeded when it expected to fail";
      }
      if (errno != expected_errno) {
        return testing::AssertionFailure() << "bind failed with an invalid errno=" << errno
                                           << ", expected errno=" << expected_errno;
      }
    } else if (r != 0) {
      return testing::AssertionFailure() << "bind failed: " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult TryConnect(uint16_t port, int expected_errno) {
    fbl::unique_fd sock(socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP));
    if (!sock) {
      return testing::AssertionFailure() << "socket failed: " << strerror(errno);
    }

    sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons(port),
        .sin_addr =
            {
                .s_addr = htonl(INADDR_LOOPBACK),
            },
    };
    int r = connect(sock.get(), reinterpret_cast<sockaddr*>(&addr), sizeof(addr));
    if (expected_errno) {
      if (r != -1) {
        return testing::AssertionFailure() << "connect succeeded when it expected to fail";
      }
      if (errno != expected_errno) {
        return testing::AssertionFailure() << "connect failed with an invalid errno=" << errno
                                           << ", expected errno=" << expected_errno;
      }
    } else if (r != 0) {
      return testing::AssertionFailure() << "connect failed: " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

 protected:
  test_helper::ScopedTempDir temp_dir_;
  fbl::unique_fd root_cgroup_;
};

TEST_F(BpfCgroupTest, BlockBind) {
  ASSERT_TRUE(TryBind(BLOCKED_PORT, 0));

  auto prog = LoadBlockPortProgram(BPF_CGROUP_INET4_BIND);

  AttachToRootCgroup(BPF_CGROUP_INET4_BIND, prog.get());

  // The port should be blocked now.
  ASSERT_TRUE(TryBind(BLOCKED_PORT, EPERM));

  // Other ports are not blocked.
  ASSERT_TRUE(TryBind(BLOCKED_PORT + 1, 0));

  DetachFromRootCgroup(BPF_CGROUP_INET4_BIND);

  // Repeated attempt to detach the program should fail.
  EXPECT_EQ(TryDetachFromRootCgroup(BPF_CGROUP_INET4_BIND), -1);
  EXPECT_EQ(errno, ENOENT);

  // Should be unblocked now.
  ASSERT_TRUE(TryBind(BLOCKED_PORT, 0));
}

TEST_F(BpfCgroupTest, BlockConnect) {
  ASSERT_TRUE(TryConnect(BLOCKED_PORT, 0));

  auto prog = LoadBlockPortProgram(BPF_CGROUP_INET4_CONNECT);

  AttachToRootCgroup(BPF_CGROUP_INET4_CONNECT, prog.get());

  // The port should be blocked now.
  ASSERT_TRUE(TryConnect(BLOCKED_PORT, EPERM));

  // Other ports are not blocked.
  ASSERT_TRUE(TryConnect(BLOCKED_PORT + 1, 0));

  DetachFromRootCgroup(BPF_CGROUP_INET4_CONNECT);

  // Should be unblocked now.
  ASSERT_TRUE(TryConnect(BLOCKED_PORT, 0));
}

// Checks that epoll is handled properly for program FDs.
TEST_F(BpfCgroupTest, ProgFdEpoll) {
  auto prog = LoadBlockPortProgram(BPF_CGROUP_INET4_CONNECT);

  fbl::unique_fd epollfd(SAFE_SYSCALL(epoll_create(1)));

  struct epoll_event ev;
  ev.events = EPOLLIN;

  ASSERT_EQ(epoll_ctl(epollfd.get(), EPOLL_CTL_ADD, prog.get(), &ev), -1);
  ASSERT_EQ(errno, EPERM);
}

TEST_F(BpfCgroupTest, CgroupSkbAttachRequiresNetAdmin) {
  auto prog = LoadCgroupSkbProgram(BPF_CGROUP_INET_INGRESS);

  AttachToRootCgroup(BPF_CGROUP_INET_INGRESS, prog.get());
  DetachFromRootCgroup(BPF_CGROUP_INET_INGRESS);

  // Older versions of Linux behave inconsisntently with current versions:
  //  - CAP_NET_ADMIN is needed even with CAP_SYS_ADMIN.
  //  - EINVAL returned instead of EPERM.
  bool version_at_least_6_9 = test_helper::IsKernelVersionAtLeast(6, 9);

  test_helper::ForkHelper fork_helper;
  if (version_at_least_6_9) {
    SCOPED_TRACE("Without CAP_NET_ADMIN");
    fork_helper.RunInForkedProcess([&]() {
      test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
      AttachToRootCgroup(BPF_CGROUP_INET_INGRESS, prog.get());
      DetachFromRootCgroup(BPF_CGROUP_INET_INGRESS);
    });
    ASSERT_TRUE(fork_helper.WaitForChildren());
  }

  {
    SCOPED_TRACE("Without CAP_SYS_ADMIN");
    fork_helper.RunInForkedProcess([&]() {
      test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);
      AttachToRootCgroup(BPF_CGROUP_INET_INGRESS, prog.get());
      DetachFromRootCgroup(BPF_CGROUP_INET_INGRESS);
    });
    ASSERT_TRUE(fork_helper.WaitForChildren());
  }

  SCOPED_TRACE("Without CAP_SYS_ADMIN and CAP_NET_ADMIN");
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    EXPECT_THAT(TryAttachToRootCgroup(BPF_CGROUP_INET_INGRESS, prog.get()), SyscallFails());

    // On Linux this may fail with `EINVAL`,
    if (version_at_least_6_9) {
      EXPECT_EQ(errno, EPERM);
    }
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

class BpfIdTest : public BpfTestBase {
 protected:
  std::optional<int> GetNext(int cmd, int start_id) {
    bpf_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.start_id = start_id;
    int result = bpf(cmd, &attr);
    if (result < 0) {
      return std::nullopt;
    }
    return attr.next_id;
  }

  std::vector<int> GetObjectList(int get_next_cmd) {
    int last = 0;
    std::vector<int> result;
    while (true) {
      auto next = GetNext(get_next_cmd, last);
      if (!next) {
        EXPECT_EQ(errno, ENOENT);
        break;
      }
      last = *next;
      result.push_back(*next);
    }
    return result;
  }

  std::vector<int> GetProgIds() { return GetObjectList(BPF_PROG_GET_NEXT_ID); }
  std::vector<int> GetMapIds() { return GetObjectList(BPF_MAP_GET_NEXT_ID); }
};

TEST_F(BpfIdTest, ProgIds) {
  auto progs_before = GetProgIds();

  // A bpf program that blocks bind on 42.
  bpf_insn program[] = {
      // r0 <- 1
      BPF_MOV_IMM(0, 1),
      // exit
      BPF_RETURN(),
  };

  auto prog =
      LoadProgram(program, sizeof(program) / sizeof(program[0]), BPF_PROG_TYPE_SOCKET_FILTER, 0);
  int prog_id = BpfGetProgId(prog.get());

  auto progs_after = GetProgIds();

  ASSERT_TRUE(std::find(progs_before.begin(), progs_before.end(), prog_id) == progs_before.end());
  ASSERT_TRUE(std::find(progs_after.begin(), progs_after.end(), prog_id) != progs_after.end());

  prog.reset();

  progs_after = GetProgIds();
  ASSERT_TRUE(std::find(progs_after.begin(), progs_after.end(), prog_id) == progs_after.end());
}

TEST_F(BpfIdTest, MapIds) {
  auto maps_before = GetMapIds();

  auto map = CreateMap(BPF_MAP_TYPE_ARRAY, sizeof(int), 1024, 10);
  int map_id = BpfGetMapId(map.get());

  auto maps_after = GetMapIds();

  ASSERT_TRUE(std::find(maps_before.begin(), maps_before.end(), map_id) == maps_before.end());
  ASSERT_TRUE(std::find(maps_after.begin(), maps_after.end(), map_id) != maps_after.end());

  map.reset();

  maps_after = GetMapIds();
  ASSERT_TRUE(std::find(maps_after.begin(), maps_after.end(), map_id) == maps_after.end());
}

TEST_F(BpfIdTest, RequiresSysAdmin) {
  if (!test_helper::HasCapability(CAP_SYS_ADMIN)) {
    GTEST_SKIP() << "Test expects CAP_SYS_ADMIN";
  }

  // Create a BPF program and a map while capabilities are still present.
  bpf_insn program_insns[] = {
      BPF_MOV_IMM(0, 1),
      BPF_RETURN(),
  };
  auto prog = LoadProgram(program_insns, sizeof(program_insns) / sizeof(program_insns[0]),
                          BPF_PROG_TYPE_SOCKET_FILTER, 0);
  int prog_id = BpfGetProgId(prog.get());

  auto map = CreateMap(BPF_MAP_TYPE_ARRAY, sizeof(int), 1024, 10);
  int map_id = BpfGetMapId(map.get());

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([prog_id, map_id] {
    test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);

    bpf_attr attr;
    memset(&attr, 0, sizeof(attr));

    // BPF_PROG_GET_NEXT_ID
    attr.start_id = 0;
    EXPECT_EQ(bpf(BPF_PROG_GET_NEXT_ID, &attr), -1);
    EXPECT_EQ(errno, EPERM);

    // BPF_MAP_GET_NEXT_ID
    memset(&attr, 0, sizeof(attr));
    attr.start_id = 0;
    EXPECT_EQ(bpf(BPF_MAP_GET_NEXT_ID, &attr), -1);
    EXPECT_EQ(errno, EPERM);

    // BPF_PROG_GET_FD_BY_ID
    memset(&attr, 0, sizeof(attr));
    attr.prog_id = prog_id;
    EXPECT_EQ(bpf(BPF_PROG_GET_FD_BY_ID, &attr), -1);
    EXPECT_EQ(errno, EPERM);

    // BPF_MAP_GET_FD_BY_ID
    memset(&attr, 0, sizeof(attr));
    attr.map_id = map_id;
    EXPECT_EQ(bpf(BPF_MAP_GET_FD_BY_ID, &attr), -1);
    EXPECT_EQ(errno, EPERM);
  });
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

}  // namespace
