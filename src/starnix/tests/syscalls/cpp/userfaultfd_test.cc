// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/xattr.h>
#include <syscall.h>
#include <unistd.h>

#include <cerrno>
#include <csignal>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <functional>
#include <optional>
#include <ostream>

#include <linux/capability.h>
#include <linux/userfaultfd.h>

#include "gtest/gtest.h"
#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP 4
#endif

#ifndef UFFD_USER_MODE_ONLY
#define UFFD_USER_MODE_ONLY 1
#endif

class UffdWrapper {
 public:
  UffdWrapper() {
    int flags = O_CLOEXEC | O_NONBLOCK;
    // UFFD_USER_MODE_ONLY was introduced in 5.11 and restricts page fault handling to faults
    // originated from user space only. Without this flag, additional privileges might be needed to
    // run these tests.
    if (test_helper::IsKernelVersionAtLeast(5, 11) || test_helper::IsStarnix()) {
      flags |= UFFD_USER_MODE_ONLY;
    }
    uffd_ = SAFE_SYSCALL(static_cast<int>(syscall(__NR_userfaultfd, flags)));
  }

  ~UffdWrapper() { SAFE_SYSCALL(close(uffd_)); }

  void api() const { api(0); }

  void api(uint64_t features) const {
    uffdio_api api = {.api = UFFD_API, .features = features, .ioctls = 0};
    SAFE_SYSCALL(ioctl(uffd_, UFFDIO_API, &api));
  }

  void register_range(uintptr_t start, size_t len, size_t mode) const {
    uffdio_register uffdio_register;
    uffdio_register.range.start = start;
    uffdio_register.range.len = len;
    uffdio_register.mode = mode;

    ASSERT_EQ(ioctl(uffd_, UFFDIO_REGISTER, &uffdio_register), 0);
  }

  int unregister_range(uintptr_t start, size_t len) const {
    uffdio_range uffdio_range;
    uffdio_range.start = start;
    uffdio_range.len = len;

    return ioctl(uffd_, UFFDIO_UNREGISTER, &uffdio_range);
  }

  void copy_one_page(uintptr_t src, uintptr_t dst) const {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    uffdio_copy uffdio_copy{
        .dst = dst & ~(page_size - 1),  // Align to page size
        .src = src,
        .len = page_size,
        .mode = 0,
        .copy = 0,
    };
    ASSERT_EQ(ioctl(uffd_, UFFDIO_COPY, &uffdio_copy), 0);
  }

  int zero_one_page(uintptr_t dst) const {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    uffdio_range uffdio_range;
    uffdio_range.start = dst;
    uffdio_range.len = page_size;
    uffdio_zeropage uffdio_zeropage;
    uffdio_zeropage.range = uffdio_range;
    uffdio_zeropage.mode = 0;

    int res = ioctl(uffd_, UFFDIO_ZEROPAGE, &uffdio_zeropage);
    return res;
  }

  int fd() const { return uffd_; }

 private:
  int uffd_;
};

TEST(UffdTest, InitReturnsSupportedFeatures) {
  UffdWrapper uffd;
  uffdio_api api = {.api = UFFD_API, .features = 0, .ioctls = 0};
  ASSERT_EQ(ioctl(uffd.fd(), UFFDIO_API, &api), 0);
  // Lists of supported features and supported ioctls are not empty
  ASSERT_NE(api.features, 0ul);
  ASSERT_NE(api.ioctls, 0ul);
}

TEST(UffdTest, NoOperationsBeforeInit) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uffdio_range uffdio_range{.start = reinterpret_cast<uintptr_t>(full_range.mapping()),
                            .len = page_size};

  UffdWrapper uffd;

  // No other ioctl is permitted on this file descriptor until the UFFDIO_API is called for the
  // first time
  uffdio_register uffdio_register{
      .range = uffdio_range, .mode = UFFDIO_REGISTER_MODE_MISSING, .ioctls = 0};
  EXPECT_NE(ioctl(uffd.fd(), UFFDIO_REGISTER, &uffdio_register), 0);
  EXPECT_EQ(errno, EINVAL);
  EXPECT_NE(ioctl(uffd.fd(), UFFDIO_UNREGISTER, &uffdio_range), 0);
  EXPECT_EQ(errno, EINVAL);

  uffdio_api api = {.api = UFFD_API, .features = UFFD_FEATURE_SIGBUS, .ioctls = 0};
  ASSERT_EQ(ioctl(uffd.fd(), UFFDIO_API, &api), 0);

  // The same ioctls succeed now
  EXPECT_EQ(ioctl(uffd.fd(), UFFDIO_REGISTER, &uffdio_register), 0);
  EXPECT_EQ(ioctl(uffd.fd(), UFFDIO_UNREGISTER, &uffdio_range), 0);

  // Reinitialization is not allowed
  ASSERT_NE(ioctl(uffd.fd(), UFFDIO_API, &api), 0);
  EXPECT_EQ(errno, EPERM);
}

class UffdProcTest : public ProcTestBase {
 public:
  std::optional<test_helper::MemoryMappingExt> FindMemoryMappingInSmaps(uintptr_t addr) const {
    std::string smaps;
    if (!files::ReadFileToString(proc_path() + "/self/smaps", &smaps)) {
      return std::nullopt;
    }
    return test_helper::find_memory_mapping_ext(addr, smaps);
  }
};

TEST_F(UffdProcTest, Register) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = page_size;

  UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());

  uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

  auto registered_mapping = FindMemoryMappingInSmaps(start);
  ASSERT_TRUE(registered_mapping->ContainsFlag("um"));
}

TEST_F(UffdProcTest, RegisterPart) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = 3 * page_size;

  UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());

  uffd.register_range(start + page_size, page_size, UFFDIO_REGISTER_MODE_MISSING);

  auto page1 = FindMemoryMappingInSmaps(start);
  ASSERT_FALSE(page1->ContainsFlag("um"));
  auto page2 = FindMemoryMappingInSmaps(start + page_size);
  ASSERT_TRUE(page2.has_value());
  ASSERT_TRUE(page2->ContainsFlag("um"));
  ASSERT_EQ(page2->start, start + page_size);
  ASSERT_EQ(page2->end, start + (2 * page_size));
  auto page3 = FindMemoryMappingInSmaps(start + (2 * page_size));
  ASSERT_FALSE(page3->ContainsFlag("um"));
}

TEST(UffdTest, RegisterEmpty) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);

  // Identify an empty page by mapping and then unmapping
  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());
  full_range.Unmap();

  uffdio_register uffdio_register;
  uffdio_register.range.start = start;
  uffdio_register.range.len = page_size;
  uffdio_register.mode = UFFDIO_REGISTER_MODE_MISSING;

  ASSERT_NE(ioctl(uffd.fd(), UFFDIO_REGISTER, &uffdio_register), 0);
  EXPECT_EQ(errno, EINVAL);
}

TEST(UffdTest, TwoUffdCantOverlap) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = page_size;

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());
  uffdio_register uffdio_register;

  // Register the mapping with uffd_a
  UffdWrapper uffd_a;
  uffd_a.api(UFFD_FEATURE_SIGBUS);
  uffdio_register.range.start = start;
  uffdio_register.range.len = len;
  uffdio_register.mode = UFFDIO_REGISTER_MODE_MISSING;
  ASSERT_EQ(ioctl(uffd_a.fd(), UFFDIO_REGISTER, &uffdio_register), 0);

  // Can't register the same mapping with a different uffd
  UffdWrapper uffd_b;
  uffd_b.api(UFFD_FEATURE_SIGBUS);
  uffdio_register.range.start = start;
  uffdio_register.range.len = page_size;
  uffdio_register.mode = UFFDIO_REGISTER_MODE_MISSING;
  ASSERT_NE(ioctl(uffd_b.fd(), UFFDIO_REGISTER, &uffdio_register), 0);
  EXPECT_EQ(errno, EBUSY);
}

TEST_F(UffdProcTest, SmapsOnFork) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = page_size;

  UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

    // The registration isn't reflected in child's smaps
    auto registered_mapping = FindMemoryMappingInSmaps(start);
    ASSERT_FALSE(registered_mapping->ContainsFlag("um"));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  // Instead, it is reflected in the parent's smaps
  auto registered_mapping = FindMemoryMappingInSmaps(start);
  ASSERT_TRUE(registered_mapping->ContainsFlag("um"));
}

TEST_F(UffdProcTest, Unregister) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = 2 * page_size;
  UffdWrapper uffd_a;
  uffd_a.api(UFFD_FEATURE_SIGBUS);
  UffdWrapper uffd_b;
  uffd_b.api(UFFD_FEATURE_SIGBUS);

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());
  reinterpret_cast<char *>(full_range.mapping())[0] = 'a';

  // Unregistering without prior registration is a noop
  EXPECT_EQ(uffd_a.unregister_range(start, len), 0);
  uffd_a.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);
  EXPECT_EQ(uffd_a.unregister_range(start, len), 0);
  uffd_b.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);
  uffd_a.register_range(start + page_size, page_size, UFFDIO_REGISTER_MODE_MISSING);
  // Unregistration affects all uffd registrations in the range, regardless of which uffd they
  // belong to.
  EXPECT_EQ(uffd_a.unregister_range(start, len), 0);

  auto registered_mapping_a = FindMemoryMappingInSmaps(start);
  ASSERT_FALSE(registered_mapping_a->ContainsFlag("um"));
  auto registered_mapping_b = FindMemoryMappingInSmaps(start + page_size);
  ASSERT_FALSE(registered_mapping_b->ContainsFlag("um"));
}

TEST_F(UffdProcTest, Close) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = 2 * page_size;

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());
  memset(full_range.mapping(), 'a', len);
  UffdWrapper uffd_a;
  uffd_a.api(UFFD_FEATURE_SIGBUS);
  {
    UffdWrapper uffd_b;
    uffd_b.api(UFFD_FEATURE_SIGBUS);

    uffd_a.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);
    uffd_b.register_range(start + page_size, page_size, UFFDIO_REGISTER_MODE_MISSING);
  }

  auto registered_mapping_a = FindMemoryMappingInSmaps(start);
  auto registered_mapping_b = FindMemoryMappingInSmaps(start + page_size);
  ASSERT_NE(registered_mapping_a->start, registered_mapping_b->start);
  ASSERT_TRUE(registered_mapping_a->ContainsFlag("um"));
  ASSERT_FALSE(registered_mapping_b->ContainsFlag("um"));
}

// This test intentionally triggers a deadly signal (SIGBUS) which can't pass on sanitizer runs.
#if (!__has_feature(address_sanitizer))
TEST(UffdTest, Sigbus) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGBUS);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    // Register the mapping with uffd_a
    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

    reinterpret_cast<char *>(full_range.mapping())[0] = 'a';
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
#endif

TEST(UffdTest, NoSigbusAfterUnregister) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    // Register the mapping with uffd_a
    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
    uffd.unregister_range(start, len);

    reinterpret_cast<char *>(full_range.mapping())[0] = 'a';
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(UffdTest, NoSigbusAfterClose) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    {
      UffdWrapper uffd;
      uffd.api(UFFD_FEATURE_SIGBUS);

      uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
    }

    reinterpret_cast<char *>(full_range.mapping())[0] = 'a';
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(UffdTest, NoSigbusFromSyscall) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  uintptr_t start = reinterpret_cast<uintptr_t>(full_range.mapping());

  UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);

  // Register 2nd page only.
  uffd.register_range(start + page_size, page_size, UFFDIO_REGISTER_MODE_MISSING);

  test_helper::ScopedTempFD temp_file;
  ASSERT_TRUE(temp_file);

  // Complete bad write.
  errno = 0;
  ssize_t sresult = write(temp_file.fd(), reinterpret_cast<void *>(start + page_size), page_size);
  EXPECT_EQ(-1, sresult);
  EXPECT_EQ(EFAULT, errno);
}

// This test intentionally triggers a deadly signal (SIGBUS) which can't pass on sanitizer runs.
#if (!__has_feature(address_sanitizer))
TEST(UffdTest, RegisterThenFork) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGBUS);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);
    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

    test_helper::ForkHelper helper2;
    helper2.OnlyWaitForForkedChildren();
    helper2.RunInForkedProcess([&] {
      char c_from_fork = reinterpret_cast<char *>(full_range.mapping())[0];
      // Accessing the registered range from child doesn't trigger uffd
      EXPECT_EQ(c_from_fork, 0);
      exit(EXIT_SUCCESS);
    });
    ASSERT_TRUE(helper2.WaitForChildren());
    // Accessing the registered range from parent triggers uffd
    char c = reinterpret_cast<char *>(full_range.mapping())[0];
    EXPECT_EQ(c, 0);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
#endif

// This test intentionally triggers a deadly signal (SIGBUS) which can't pass on sanitizer runs.
#if (!__has_feature(address_sanitizer))
TEST(UffdTest, ForkThenRegister) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGBUS);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    test_helper::ForkHelper helper2;
    helper2.OnlyWaitForForkedChildren();
    helper2.RunInForkedProcess([&] {
      uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
      // Accessing the registered range from child doesn't trigger uffd
      char c_from_fork = reinterpret_cast<char *>(full_range.mapping())[0];
      EXPECT_EQ(c_from_fork, 0);
      exit(EXIT_SUCCESS);
    });
    ASSERT_TRUE(helper2.WaitForChildren());
    // Accessing the registered range from parent triggers uffd
    char c = reinterpret_cast<char *>(full_range.mapping())[0];
    EXPECT_EQ(c, 0);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
#endif

// This test intentionally triggers a deadly signal (SIGBUS) which can't pass on sanitizer runs.
#if (!__has_feature(address_sanitizer))
TEST(UffdTest, ForkThenInit) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGBUS);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;
    UffdWrapper uffd;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    test_helper::ForkHelper helper2;
    helper2.OnlyWaitForForkedChildren();
    helper2.RunInForkedProcess([&] {
      uffd.api(UFFD_FEATURE_SIGBUS);
      uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
      // Accessing the registered range from child doesn't trigger uffd
      char c_from_fork = reinterpret_cast<char *>(full_range.mapping())[0];
      EXPECT_EQ(c_from_fork, 0);
      exit(EXIT_SUCCESS);
    });
    ASSERT_TRUE(helper2.WaitForChildren());
    // Accessing the registered range from parent triggers uffd
    char c = reinterpret_cast<char *>(full_range.mapping())[0];
    EXPECT_EQ(c, 0);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
#endif

TEST(UffdTest, SigbusHandling) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    static UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    // SIGBUS handler
    struct sigaction act;
    std::memset(&act, '\0', sizeof(act));
    act.sa_flags = SA_SIGINFO | SA_RESTART;
    act.sa_sigaction = [](int sig, siginfo_t *info, void *context) {
      const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
      size_t len = page_size;
      auto src = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
          nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
      auto dst = reinterpret_cast<uintptr_t>(info->si_addr) & ~(page_size - 1);
      uffd.copy_one_page(reinterpret_cast<uintptr_t>(src.mapping()), dst);
    };
    SAFE_SYSCALL(sigaction(SIGBUS, &act, nullptr));

    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

    reinterpret_cast<char *>(full_range.mapping())[5] = 'a';
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(UffdTest, PopulateFromAnotherUffd) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    static UffdWrapper uffd_a;
    uffd_a.api(UFFD_FEATURE_SIGBUS);
    static UffdWrapper uffd_b;
    uffd_b.api(UFFD_FEATURE_SIGBUS);

    uffd_a.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);

    struct sigaction act;
    std::memset(&act, '\0', sizeof(act));
    act.sa_flags = SA_SIGINFO | SA_RESTART;
    act.sa_sigaction = [](int sig, siginfo_t *info, void *context) {
      const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
      auto dst = reinterpret_cast<uintptr_t>(info->si_addr) & ~(page_size - 1);
      auto buffer = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
          nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
      memset(buffer.mapping(), 'a', page_size);

      // Populate from uffd_b a range that is registered with uffd_a
      uffd_b.copy_one_page(reinterpret_cast<uintptr_t>(buffer.mapping()), dst);
    };
    SAFE_SYSCALL(sigaction(SIGBUS, &act, nullptr));

    EXPECT_EQ(reinterpret_cast<char *>(start)[5], 'a');
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(UffdTest, IoctlZeroIncludingUnregistered) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = 2 * page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    static UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);
    // Register only the first page
    uffd.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);

    struct sigaction act;
    std::memset(&act, '\0', sizeof(act));
    act.sa_flags = SA_SIGINFO | SA_RESTART;
    act.sa_sigaction = [](int sig, siginfo_t *info, void *context) {
      const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
      auto dst = reinterpret_cast<uintptr_t>(info->si_addr) & ~(page_size - 1);
      uffdio_range uffdio_range;
      uffdio_range.start = dst;
      uffdio_range.len = 2 * page_size;
      uffdio_zeropage uffdio_zeropage;
      uffdio_zeropage.range = uffdio_range;
      uffdio_zeropage.mode = 0;

      // Try to fill both pages with zeroes. This should fail and the first page should remain
      // unmodified.
      EXPECT_NE(ioctl(uffd.fd(), UFFDIO_ZEROPAGE, &uffdio_zeropage), 0);
      EXPECT_EQ(errno, ENOENT);
      EXPECT_EQ(uffdio_zeropage.zeropage, -1 * ENOENT);
      // Try filling just first page.
      uffdio_range.len = page_size;
      uffdio_zeropage.range = uffdio_range;
      EXPECT_EQ(ioctl(uffd.fd(), UFFDIO_ZEROPAGE, &uffdio_zeropage), 0);
      EXPECT_EQ(uffdio_zeropage.zeropage, static_cast<int64_t>(page_size));
    };
    SAFE_SYSCALL(sigaction(SIGBUS, &act, nullptr));

    EXPECT_EQ(reinterpret_cast<char *>(start)[5], '\0');
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(UffdTest, CannotPopulateTwice) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = 2 * page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

    static intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    static UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);

    struct sigaction act;
    std::memset(&act, '\0', sizeof(act));
    act.sa_flags = SA_SIGINFO | SA_RESTART;
    act.sa_sigaction = [](int sig, siginfo_t *info, void *context) {
      const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
      auto buffer = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
          nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
      memset(buffer.mapping(), 'a', 2 * page_size);
      uintptr_t src = reinterpret_cast<uintptr_t>(buffer.mapping());
      uintptr_t first_page = start;
      uintptr_t second_page = start + page_size;

      // Populate the second page (this should work)
      uffd.copy_one_page(src, second_page);

      // Filling the same page should fail as it is not empty.
      uffdio_copy uffdio_copy_second{
          .dst = second_page,
          .src = src,
          .len = page_size,
          .mode = 0,
          .copy = 0,
      };

      EXPECT_NE(ioctl(uffd.fd(), UFFDIO_COPY, &uffdio_copy_second), 0);
      EXPECT_EQ(errno, EEXIST);
      // The error is reported
      EXPECT_EQ(uffdio_copy_second.copy, -1 * EEXIST);

      // Filling both pages should fail as well, but first page should get filled. The error is
      // EAGAIN in that case.
      uffdio_copy uffdio_copy_both{
          .dst = first_page,
          .src = src,
          .len = 2 * page_size,
          .mode = 0,
          .copy = 0,
      };

      EXPECT_NE(ioctl(uffd.fd(), UFFDIO_COPY, &uffdio_copy_both), 0);
      EXPECT_EQ(errno, EAGAIN);

      // The number of bytes copied is reported
      ASSERT_EQ(uffdio_copy_both.copy, static_cast<int64_t>(page_size));
    };
    SAFE_SYSCALL(sigaction(SIGBUS, &act, nullptr));

    // Accessing any page in the registered range will trigger the signal handler.
    EXPECT_EQ(reinterpret_cast<char *>(start)[5], 'a');
    // Second page is still populated
    EXPECT_EQ(reinterpret_cast<char *>(start)[page_size + 5], 'a');
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

// This doesn't work on Starnix as we don't have an easy way to check if a page is present in
// physical memory or not.
TEST(UffdTest, NoSigBusOnAccessedPage) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  size_t len = page_size;

  auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());
  EXPECT_EQ(reinterpret_cast<char *>(start)[5], '\0');

  static UffdWrapper uffd;
  uffd.api(UFFD_FEATURE_SIGBUS);
  uffd.register_range(start, page_size, UFFDIO_REGISTER_MODE_MISSING);

  EXPECT_EQ(reinterpret_cast<char *>(start)[5], '\0');
}

// A weaker version of the previous test: sigbus is not triggered if the page was already populated
// from uffd.
TEST(UffdTest, NoSigbusAfterPopulating) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    auto full_range = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    intptr_t start = reinterpret_cast<intptr_t>(full_range.mapping());

    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);

    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
    uffd.zero_one_page(start);

    ASSERT_EQ(reinterpret_cast<char *>(start)[5], '\0');
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

// This test intentionally triggers a deadly signal (SIGBUS) which can't pass on sanitizer runs.
#if (!__has_feature(address_sanitizer))
TEST_F(UffdProcTest, RegisterThenMove) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGBUS);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([&] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    size_t len = page_size;

    void *source =
        mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    // Add an extra page to prevent the source being merged with the remapped region
    void *filler =
        mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(source, MAP_FAILED);
    ASSERT_NE(filler, MAP_FAILED);
    intptr_t start = reinterpret_cast<intptr_t>(source);

    UffdWrapper uffd;
    uffd.api(UFFD_FEATURE_SIGBUS);
    uffd.register_range(start, len, UFFDIO_REGISTER_MODE_MISSING);
    auto smaps_init = FindMemoryMappingInSmaps(start);
    ASSERT_TRUE(smaps_init->ContainsFlag("um"));

    void *remapped = mremap(source, len, len, MREMAP_MAYMOVE | MREMAP_DONTUNMAP, 0);
    ASSERT_NE(remapped, MAP_FAILED);
    ASSERT_NE(remapped, source);

    auto smaps_source = FindMemoryMappingInSmaps(start);
    ASSERT_TRUE(smaps_source->ContainsFlag("um"));
    auto smaps_remapped = FindMemoryMappingInSmaps(reinterpret_cast<intptr_t>(remapped));
    ASSERT_FALSE(smaps_remapped->ContainsFlag("um"));

    // No sigbus on remapped
    ASSERT_EQ(reinterpret_cast<char *>(remapped)[5], '\0');
    // SIGBUS on original
    ASSERT_EQ(reinterpret_cast<char *>(start)[5], '\0');
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
#endif

TEST(UffdTest, NonUserModeOnlyRequiresCapSysPtrace) {
  test_helper::ForkHelper helper;

  // Subprocess 1: Without capability
  helper.RunInForkedProcess([]() {
    test_helper::UnsetCapabilityEffective(CAP_SYS_PTRACE);

    int flags = O_CLOEXEC | O_NONBLOCK;  // Omitted UFFD_USER_MODE_ONLY
    long ret = syscall(SYS_userfaultfd, flags);

    EXPECT_THAT(ret, SyscallFailsWithErrno(EPERM));
  });

  // Subprocess 2: With capability
  helper.RunInForkedProcess([]() {
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_SYS_PTRACE))
        << "Test must be run with CAP_SYS_PTRACE (e.g., as root)";

    int flags = O_CLOEXEC | O_NONBLOCK;
    long ret = syscall(SYS_userfaultfd, flags);

    EXPECT_THAT(ret, SyscallSucceeds());
    if (ret >= 0) {
      close(static_cast<int>(ret));
    }
  });

  EXPECT_TRUE(helper.WaitForChildren());
}
