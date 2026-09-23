// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/bpf.h"

#include <fcntl.h>
#include <stdlib.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <syscall.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

constexpr char kContextLackingLockPermission[] = "test_u:test_r:bpf_map_read_write_t:s0";

int bpf(int cmd, union bpf_attr* attr) { return (int)syscall(__NR_bpf, cmd, attr, sizeof(*attr)); }

std::string MakeLabel(const std::string& short_domain) {
  return "test_u:test_r:bpf_map_" + short_domain + "_t:s0";
}

std::string ProtToString(int prot) {
  switch (prot) {
    case PROT_NONE:
      return "NONE";
    case PROT_READ:
      return "RO";
    case PROT_WRITE:
      return "WO";
    case PROT_READ | PROT_WRITE:
      return "RW";
    default:
      abort();
  }
}

std::string MakePinLabel(const std::string& short_domain) {
  return "test_u:test_r:bpf_pin_" + short_domain + "_t:s0";
}

std::string BpfFlagsToString(uint32_t flags) {
  switch (flags) {
    case BPF_F_RDONLY:
      return "RO";
    case BPF_F_WRONLY:
      return "WO";
    case 0:
      return "RW";
    default:
      abort();
  }
}

int BpfGetMapId(const int fd) {
  struct bpf_map_info info = {};
  union bpf_attr attr = {.info = {
                             .bpf_fd = static_cast<uint32_t>(fd),
                             .info_len = sizeof(info),
                             .info = reinterpret_cast<uint64_t>(&info),
                         }};
  int rv = bpf(BPF_OBJ_GET_INFO_BY_FD, &attr);
  if (rv) {
    return rv;
  }
  if (attr.info.info_len < offsetof(bpf_map_info, id) + sizeof(info.id)) {
    errno = EOPNOTSUPP;
    return -1;
  }
  return info.id;
}

fbl::unique_fd CreateArrayMap() {
  bpf_attr attr = {
      .map_type = BPF_MAP_TYPE_ARRAY,
      .key_size = sizeof(int),
      .value_size = static_cast<uint32_t>(getpagesize()),
      .max_entries = 1,
      .map_flags = BPF_F_MMAPABLE,
  };

  return fbl::unique_fd(bpf(BPF_MAP_CREATE, &attr));
}

#define BPF_MOV_IMM(reg, value)                                                    \
  bpf_insn {                                                                       \
    .code = BPF_ALU64 | BPF_MOV | BPF_IMM, .dst_reg = reg, .src_reg = 0, .off = 0, \
    .imm = static_cast<int32_t>(value),                                            \
  }
#define BPF_RETURN() \
  bpf_insn { .code = BPF_JMP | BPF_EXIT, .dst_reg = 0, .src_reg = 0, .off = 0, .imm = 0, }

fbl::unique_fd LoadProgram() {
  bpf_insn program[] = {
      // r0 <- 1
      BPF_MOV_IMM(0, 1),
      // exit
      BPF_RETURN(),
  };

  char buffer[4096] = {};
  union bpf_attr attr;
  memset(&attr, 0, sizeof(attr));

  attr.prog_type = BPF_PROG_TYPE_SOCKET_FILTER;
  attr.expected_attach_type = 0;
  attr.insns = reinterpret_cast<uint64_t>(program);
  attr.insn_cnt = static_cast<uint32_t>(sizeof(program) / sizeof(program[0]));
  attr.license = reinterpret_cast<uint64_t>("N/A");
  attr.log_buf = reinterpret_cast<uint64_t>(buffer);
  attr.log_size = sizeof(buffer);
  attr.log_level = 1;

  return fbl::unique_fd(bpf(BPF_PROG_LOAD, &attr));
}

struct BpfMapTestParam {
  const char* short_domain;
  int map_flags;
  bool should_succeed;

  BpfMapTestParam(const char* short_domain, int map_flags, bool should_succeed)
      : short_domain(short_domain), map_flags(map_flags), should_succeed(should_succeed) {}
};

struct BpfMapCreateTestParam {
  const char* short_domain;
  bool should_succeed;

  BpfMapCreateTestParam(const char* short_domain, bool should_succeed)
      : short_domain(short_domain), should_succeed(should_succeed) {}
};

class BpfMapCreateTest : public ::testing::TestWithParam<BpfMapCreateTestParam> {};

TEST_P(BpfMapCreateTest, Create) {
  auto [short_domain, should_succeed] = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  std::string label = MakeLabel(short_domain);

  ASSERT_TRUE(RunSubprocessAs(label.c_str(), [&] {
    fbl::unique_fd fd = CreateArrayMap();
    if (should_succeed) {
      EXPECT_THAT(fd.get(), SyscallSucceeds());
    } else {
      EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(BpfMapCreateTestSuite, BpfMapCreateTest,
                         ::testing::Values(BpfMapCreateTestParam("create", true),
                                           BpfMapCreateTestParam("none", false)),
                         [](const ::testing::TestParamInfo<BpfMapCreateTestParam>& info) {
                           return std::string(info.param.short_domain);
                         });

struct BpfProgLoadTestParam {
  const char* test_name;
  const char* label;
  bool should_succeed;

  BpfProgLoadTestParam(const char* test_name, const char* label, bool should_succeed)
      : test_name(test_name), label(label), should_succeed(should_succeed) {}
};

class BpfProgLoadTest : public ::testing::TestWithParam<BpfProgLoadTestParam> {};

TEST_P(BpfProgLoadTest, Load) {
  auto [test_name, label, should_succeed] = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(label, [&] {
    fbl::unique_fd fd = LoadProgram();
    if (should_succeed) {
      EXPECT_THAT(fd.get(), SyscallSucceeds());
    } else {
      EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(
    BpfProgLoadTestSuite, BpfProgLoadTest,
    ::testing::Values(BpfProgLoadTestParam("load", "test_u:test_r:bpf_prog_load_t:s0", true),
                      BpfProgLoadTestParam("no_prog_load", "test_u:test_r:bpf_no_prog_load_t:s0",
                                           false)),
    [](const ::testing::TestParamInfo<BpfProgLoadTestParam>& info) {
      return std::string(info.param.test_name);
    });

class BpfMapTest : public ::testing::TestWithParam<BpfMapTestParam> {};

TEST_P(BpfMapTest, Map) {
  auto [short_domain, map_flags, should_succeed] = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();
  // Create a mappable array map.
  fbl::unique_fd mappable_map_fd = CreateArrayMap();
  ASSERT_TRUE(mappable_map_fd.is_valid()) << strerror(errno);

  std::string label = MakeLabel(short_domain);

  ASSERT_TRUE(RunSubprocessAs(label.c_str(), [&] {
    intptr_t result = reinterpret_cast<intptr_t>(
        mmap(nullptr, getpagesize(), map_flags, MAP_SHARED, mappable_map_fd.get(), 0));
    if (should_succeed) {
      EXPECT_THAT(result, SyscallSucceeds());
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(EACCES));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(BpfMapTestSuite, BpfMapTest,
                         ::testing::Values(BpfMapTestParam("none", PROT_NONE, false),
                                           BpfMapTestParam("none", PROT_READ, false),
                                           BpfMapTestParam("none", PROT_WRITE, false),
                                           BpfMapTestParam("none", PROT_READ | PROT_WRITE, false),
                                           BpfMapTestParam("read", PROT_NONE, false),
                                           BpfMapTestParam("read", PROT_READ, false),
                                           BpfMapTestParam("read", PROT_WRITE, false),
                                           BpfMapTestParam("read", PROT_READ | PROT_WRITE, false),
                                           BpfMapTestParam("write", PROT_NONE, false),
                                           BpfMapTestParam("write", PROT_READ, false),
                                           BpfMapTestParam("write", PROT_WRITE, false),
                                           BpfMapTestParam("write", PROT_READ | PROT_WRITE, false),
                                           BpfMapTestParam("read_write", PROT_NONE, true),
                                           BpfMapTestParam("read_write", PROT_READ, true),
                                           BpfMapTestParam("read_write", PROT_WRITE, true),
                                           BpfMapTestParam("read_write", PROT_READ | PROT_WRITE,
                                                           true)),
                         [](const ::testing::TestParamInfo<BpfMapTestParam>& info) {
                           return std::string(info.param.short_domain) + "_" +
                                  ProtToString(info.param.map_flags);
                         });

struct BpfPinTestParam {
  const char* short_domain;
  uint32_t flags;
  bool map_should_succeed;
  bool prog_should_succeed;

  BpfPinTestParam(const char* short_domain, uint32_t flags, bool map_should_succeed,
                  bool prog_should_succeed)
      : short_domain(short_domain),
        flags(flags),
        map_should_succeed(map_should_succeed),
        prog_should_succeed(prog_should_succeed) {}
};

const char PIN_PATH[] = "/sys/fs/bpf/foo";
class BpfPinTest : public ::testing::TestWithParam<BpfPinTestParam> {};

TEST_P(BpfPinTest, PinnedMap) {
  auto [short_domain, flags, map_should_succeed, prog_should_succeed] = GetParam();
  std::string label = MakePinLabel(short_domain);

  {
    auto enforce = ScopedEnforcement::SetEnforcing();

    // Create a mappable array map.
    fbl::unique_fd mappable_map_fd = CreateArrayMap();
    ASSERT_TRUE(mappable_map_fd.is_valid()) << strerror(errno);

    bpf_attr attr = {
        .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
        .bpf_fd = static_cast<unsigned>(mappable_map_fd.get()),
    };
    EXPECT_THAT(bpf(BPF_OBJ_PIN, &attr), SyscallSucceeds());

    EXPECT_TRUE(RunSubprocessAs(label.c_str(), [&] {
      bpf_attr attr = {
          .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
          .file_flags = flags,
      };
      int result = bpf(BPF_OBJ_GET, &attr);
      if (map_should_succeed) {
        EXPECT_THAT(result, SyscallSucceeds());
      } else {
        EXPECT_THAT(result, SyscallFailsWithErrno(EACCES));
      }
      if (result >= 0) {
        close(result);
      }
    }));
  }

  EXPECT_THAT(unlink(PIN_PATH), SyscallSucceeds());
}

TEST_P(BpfPinTest, PinnedProg) {
  auto [short_domain, flags, map_should_succeed, prog_should_succeed] = GetParam();
  std::string label = MakePinLabel(short_domain);

  {
    auto enforce = ScopedEnforcement::SetEnforcing();

    // Create a mappable array map.
    fbl::unique_fd prog_fd = LoadProgram();
    ASSERT_TRUE(prog_fd.is_valid()) << strerror(errno);

    bpf_attr attr = {
        .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
        .bpf_fd = static_cast<unsigned>(prog_fd.get()),
    };
    EXPECT_THAT(bpf(BPF_OBJ_PIN, &attr), SyscallSucceeds());

    EXPECT_TRUE(RunSubprocessAs(label.c_str(), [&] {
      bpf_attr attr = {
          .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
          .file_flags = flags,
      };
      int result = bpf(BPF_OBJ_GET, &attr);
      if (prog_should_succeed) {
        EXPECT_THAT(result, SyscallSucceeds());
      } else {
        EXPECT_THAT(result, SyscallFailsWithErrno(EACCES));
      }
      if (result >= 0) {
        close(result);
      }
    }));
  }

  EXPECT_THAT(unlink(PIN_PATH), SyscallSucceeds());
}

// Pinned maps need map_read and read for readable access, map_write and
// write for writable access. prog_run is not needed.
// Opening a pinned program always requires prog_run. File read and file write are
// needed unless the access is readonly or writeonly.
INSTANTIATE_TEST_SUITE_P(
    BpfPinTestSuite, BpfPinTest,
    ::testing::Values(BpfPinTestParam("no_read", BPF_F_RDONLY, false, false),
                      BpfPinTestParam("no_read", BPF_F_WRONLY, true, true),
                      BpfPinTestParam("no_read", 0, false, false),
                      BpfPinTestParam("no_write", BPF_F_RDONLY, true, true),
                      BpfPinTestParam("no_write", BPF_F_WRONLY, false, false),
                      BpfPinTestParam("no_write", 0, false, false),
                      BpfPinTestParam("no_map_read", BPF_F_RDONLY, false, true),
                      BpfPinTestParam("no_map_read", BPF_F_WRONLY, true, true),
                      BpfPinTestParam("no_map_read", 0, false, true),
                      BpfPinTestParam("no_map_write", BPF_F_RDONLY, true, true),
                      BpfPinTestParam("no_map_write", BPF_F_WRONLY, false, true),
                      BpfPinTestParam("no_map_write", 0, false, true),
                      BpfPinTestParam("no_prog_run", BPF_F_RDONLY, true, false),
                      BpfPinTestParam("no_prog_run", BPF_F_WRONLY, true, false),
                      BpfPinTestParam("no_prog_run", 0, true, false),
                      BpfPinTestParam("all_rights", BPF_F_RDONLY, true, true),
                      BpfPinTestParam("all_rights", BPF_F_WRONLY, true, true),
                      BpfPinTestParam("all_rights", 0, true, true)),
    [](const ::testing::TestParamInfo<BpfPinTestParam>& info) {
      return std::string(info.param.short_domain) + "_" + BpfFlagsToString(info.param.flags);
    });

struct BpfFcntlLockTestParam {
  int cmd;
  short lock_type;  // F_RDLCK or F_WRLCK
};

class BpfFcntlLockTest : public ::testing::TestWithParam<BpfFcntlLockTestParam> {};

TEST_P(BpfFcntlLockTest, DoesNotRequireFileLock) {
  auto [cmd, lock_type] = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd mappable_map_fd = CreateArrayMap();
  ASSERT_TRUE(mappable_map_fd.is_valid()) << strerror(errno);

  bpf_attr attr = {
      .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
      .bpf_fd = static_cast<unsigned>(mappable_map_fd.get()),
  };
  EXPECT_THAT(bpf(BPF_OBJ_PIN, &attr), SyscallSucceeds());

  EXPECT_TRUE(RunSubprocessAs(kContextLackingLockPermission, [&] {
    bpf_attr attr = {
        .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
        .file_flags = 0,
    };
    fbl::unique_fd fd(bpf(BPF_OBJ_GET, &attr));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    int lock_ret = -1;
    if (cmd == F_SETLEASE) {
      lock_ret = fcntl(fd.get(), cmd, lock_type);
    } else {
      off_t start_offset = BpfGetMapId(fd.get());
      ASSERT_GT(start_offset, 0);
      struct flock64 fl = {
          .l_type = lock_type,
          .l_whence = SEEK_SET,
          .l_start = start_offset,
          .l_len = 1,
      };
      lock_ret = fcntl(fd.get(), cmd, &fl);
    }

    if (cmd == F_SETLEASE) {
      EXPECT_THAT(lock_ret, SyscallFailsWithErrno(EINVAL));
    } else {
      EXPECT_THAT(lock_ret, SyscallSucceeds());
    }
  }));

  EXPECT_THAT(unlink(PIN_PATH), SyscallSucceeds());
}

std::string FcntlCmdToString(int cmd) {
  switch (cmd) {
    case F_OFD_SETLK:
      return "OFD_SETLK";
    case F_SETLK:
      return "SETLK";
    case F_SETLEASE:
      return "SETLEASE";
    default:
      return std::to_string(cmd);
  }
}

INSTANTIATE_TEST_SUITE_P(BpfFcntlLockTestSuite, BpfFcntlLockTest,
                         ::testing::Values(BpfFcntlLockTestParam{F_OFD_SETLK, F_WRLCK},
                                           BpfFcntlLockTestParam{F_SETLK, F_WRLCK},
                                           BpfFcntlLockTestParam{F_SETLEASE, F_WRLCK}),
                         [](const testing::TestParamInfo<BpfFcntlLockTestParam>& info) {
                           return FcntlCmdToString(info.param.cmd);
                         });

TEST(BpfFlockTest, DoesNotRequireFileLock) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd mappable_map_fd = CreateArrayMap();
  ASSERT_TRUE(mappable_map_fd.is_valid()) << strerror(errno);

  bpf_attr attr = {
      .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
      .bpf_fd = static_cast<unsigned>(mappable_map_fd.get()),
  };
  EXPECT_THAT(bpf(BPF_OBJ_PIN, &attr), SyscallSucceeds());

  EXPECT_TRUE(RunSubprocessAs(kContextLackingLockPermission, [&] {
    bpf_attr attr = {
        .pathname = reinterpret_cast<uintptr_t>(PIN_PATH),
        .file_flags = 0,
    };
    fbl::unique_fd fd(bpf(BPF_OBJ_GET, &attr));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    int lock_ret = flock(fd.get(), LOCK_EX);
    EXPECT_THAT(lock_ret, SyscallSucceeds());
  }));

  EXPECT_THAT(unlink(PIN_PATH), SyscallSucceeds());
}

}  // namespace

extern std::string DoPrePolicyLoadWork() { return "bpf_policy"; }
