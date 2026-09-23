// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <filesystem>
#include <format>
#include <thread>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/if_tun.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

uint32_t GetLoopbackIndex() { return 1; }

// Waits for the address on the loopback device to be added or removed.
bool HasLoopbackAddress(int family, const char *address_str) {
  fbl::unique_fd nl_sock(socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE));
  EXPECT_TRUE(nl_sock.is_valid());

  test_helper::NetlinkEncoder encoder(RTM_GETADDR, NLM_F_REQUEST | NLM_F_DUMP);
  ifaddrmsg ifa_msg = {
      .ifa_family = AF_UNSPEC,
      .ifa_index = GetLoopbackIndex(),
  };
  encoder.Write(ifa_msg);

  iovec iov = {};
  encoder.Finalize(iov);
  struct msghdr msg = {
      .msg_iov = &iov,
      .msg_iovlen = 1,
  };

  EXPECT_GE(sendmsg(nl_sock.get(), &msg, 0), 0) << strerror(errno);

  uint8_t addr[16];
  EXPECT_EQ(inet_pton(family, address_str, &addr), 1) << strerror(errno);

  char buf[8192];
  while (true) {
    ssize_t len = recv(nl_sock.get(), buf, sizeof(buf), 0);
    if (errno == EINTR) {
      continue;
    }
    EXPECT_GE(len, 0);
    for (nlmsghdr *nh = reinterpret_cast<nlmsghdr *>(buf); MY_NLMSG_OK(nh, len);
         nh = NLMSG_NEXT(nh, len)) {
      if (nh->nlmsg_type == NLMSG_DONE) {
        return false;
      }
      if (nh->nlmsg_type != RTM_NEWADDR) {
        continue;
      }

      ifaddrmsg *ifa = reinterpret_cast<ifaddrmsg *>(NLMSG_DATA(nh));
      if (ifa->ifa_family != family) {
        continue;
      }

      rtattr *rta = IFA_RTA(ifa);
      int rta_len = IFA_PAYLOAD(nh);
      for (; RTA_OK(rta, rta_len); rta = RTA_NEXT(rta, rta_len)) {
        if (rta->rta_type != IFA_ADDRESS) {
          continue;
        }
        if (memcmp(addr, RTA_DATA(rta), RTA_PAYLOAD(rta)) != 0) {
          continue;
        }
        // TODO(https://issues.fuchsia.dev/472336920): Netstack currently
        // only marks the address as unavailable which gets later translated
        // to tentative by netlink. We should report RTM_DELADDR if we find
        // it to be load bearing.
        if (test_helper::IsStarnix() && (ifa->ifa_flags & IFA_F_TENTATIVE)) {
          continue;
        }
        return true;
      }
    }
  }
}

// Creates a new TUN device with the given name.
fbl::unique_fd NewTunDevice(const char *name) {
  int tun = open("/dev/tun", O_RDWR);
  if (tun == -1 && errno == ENOENT) {
    tun = open("/dev/net/tun", O_RDWR);
  }
  EXPECT_GT(tun, 0) << strerror(errno);

  ifreq ifr{};
  ifr.ifr_flags = IFF_NO_PI | IFF_TUN;

  strncpy(ifr.ifr_name, name, IFNAMSIZ);
  EXPECT_EQ(ioctl(tun, TUNSETIFF, &ifr), 0) << strerror(errno);

  return fbl::unique_fd(tun);
}

class SysctlTest : public ::testing::Test {};

class SysctlTestWithParam
    : public SysctlTest,
      public ::testing::WithParamInterface<std::tuple<std::string, std::string>> {};

TEST_P(SysctlTestWithParam, DirectoryContainsInterfaces) {
  auto const &[version, conf_or_neigh] = GetParam();
  std::vector<std::string> files;
  EXPECT_TRUE(
      files::ReadDirContents(std::format("/proc/sys/net/{}/{}", version, conf_or_neigh), &files));
  EXPECT_THAT(files, testing::IsSupersetOf({"default", "lo"}));
}

INSTANTIATE_TEST_SUITE_P(SysctlTest, SysctlTestWithParam,
                         ::testing::Combine(::testing::Values("ipv4", "ipv6"),
                                            ::testing::Values("conf", "neigh")),
                         [](const ::testing::TestParamInfo<SysctlTestWithParam::ParamType> &info) {
                           return std::format("{}_{}", std::get<0>(info.param),
                                              std::get<1>(info.param));
                         });

TEST_F(SysctlTest, AcceptRaRtTable) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Need CAP_NET_ADMIN to run SysctlTest";
  }
  std::string accept_ra_rt_table_str;

  constexpr const char *kAcceptRaRtTable = "/proc/sys/net/ipv6/conf/{}/accept_ra_rt_table";
  const std::string kDefault = std::format(kAcceptRaRtTable, "default");
  const std::string kLo = std::format(kAcceptRaRtTable, "lo");

  if (!test_helper::IsStarnix() && access(kDefault.c_str(), F_OK) == -1) {
    GTEST_SKIP() << "The kernel is not compiled with this sysctl";
  }

  const char *kVal1 = "-100\n";
  const char *kVal2 = "-200\n";

  for (auto const &path : {kDefault, kLo}) {
    EXPECT_TRUE(files::ReadFileToString(path, &accept_ra_rt_table_str));
    EXPECT_STREQ(accept_ra_rt_table_str.c_str(), "0\n");
  }

  // Write then read back value for interface lo.
  EXPECT_TRUE(files::WriteFile(kLo, kVal1));
  EXPECT_TRUE(files::ReadFileToString(kLo, &accept_ra_rt_table_str));
  EXPECT_STREQ(accept_ra_rt_table_str.c_str(), kVal1);

  // Write then read back value for special file `default`.
  EXPECT_TRUE(files::WriteFile(kDefault, kVal2));
  EXPECT_TRUE(files::ReadFileToString(kDefault, &accept_ra_rt_table_str));
  EXPECT_STREQ(accept_ra_rt_table_str.c_str(), kVal2);

  const char *kTunName = "tun0";
  auto tun = NewTunDevice(kTunName);

  const std::string kTunPath = std::format(kAcceptRaRtTable, kTunName);
  int trial = 0;
  while (!files::ReadFileToString(kTunPath, &accept_ra_rt_table_str)) {
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    ASSERT_LE(++trial, 100);
  }

  // The new device will have the `default` value.
  EXPECT_STREQ(accept_ra_rt_table_str.c_str(), kVal2);
}

TEST_F(SysctlTest, DisableIpv6) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Need CAP_NET_ADMIN to run SysctlTest";
  }

  ASSERT_TRUE(HasLoopbackAddress(AF_INET6, "::1"));

  const char kDisableIpv6[] = "/proc/sys/net/ipv6/conf/lo/disable_ipv6";
  ASSERT_TRUE(files::WriteFile(kDisableIpv6, "1"));

  // IP configurations are applied synchronously at netstack, but netlink
  // watches for changes and updates addresses asynchronously. So we add some
  // retry to avoid flakiness. The maximum delay is 100 * 100ms = 10s.
  constexpr int kMaxAttempts = 100;
  constexpr std::chrono::milliseconds kRetryTimeout(100);

  // Verify ::1 is gone.
  bool removed = false;
  for (int trial = 0; trial < kMaxAttempts; trial++) {
    if (!HasLoopbackAddress(AF_INET6, "::1")) {
      removed = true;
      break;
    }
    std::this_thread::sleep_for(kRetryTimeout);
  }
  ASSERT_TRUE(removed) << "::1 is not gone after "
                       << kMaxAttempts * kRetryTimeout / std::chrono::seconds(1) << "s";

  // Re-enable it and assert ::1 is back.
  ASSERT_TRUE(files::WriteFile(kDisableIpv6, "0"));
  bool added = false;
  for (int trial = 0; trial < kMaxAttempts; trial++) {
    if (HasLoopbackAddress(AF_INET6, "::1")) {
      added = true;
      break;
    }
    std::this_thread::sleep_for(kRetryTimeout);
  }
  ASSERT_TRUE(added) << "::1 is not back after "
                     << kMaxAttempts * kRetryTimeout / std::chrono::seconds(1) << "s";
}

TEST_F(SysctlTest, DisableIpv6Default) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Need CAP_NET_ADMIN to run SysctlTest";
  }

  files::WriteFile("/proc/sys/net/ipv6/conf/default/disable_ipv6", "1");
  auto tun = NewTunDevice("tun1");
  std::string disable_ipv6_str;
  int trial = 0;
  while (!files::ReadFileToString("/proc/sys/net/ipv6/conf/tun1/disable_ipv6", &disable_ipv6_str)) {
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    ASSERT_LE(++trial, 100);
  }
  ASSERT_STREQ(disable_ipv6_str.c_str(), "1\n");
}

TEST_F(SysctlTest, DropCaches) {
  if (!test_helper::HasCapability(CAP_SYS_ADMIN)) {
    GTEST_SKIP() << "Need CAP_SYS_ADMIN to run DropCaches";
  }
  const char kDropCaches[] = "/proc/sys/vm/drop_caches";
  ASSERT_TRUE(files::WriteFile(kDropCaches, "1"));
  ASSERT_TRUE(files::WriteFile(kDropCaches, "2"));
  ASSERT_TRUE(files::WriteFile(kDropCaches, "3"));
  ASSERT_TRUE(files::WriteFile(kDropCaches, "4"));
  EXPECT_FALSE(files::WriteFile(kDropCaches, "0"));
  EXPECT_EQ(errno, EINVAL);
  EXPECT_FALSE(files::WriteFile(kDropCaches, "5"));
  EXPECT_EQ(errno, EINVAL);
  EXPECT_FALSE(files::WriteFile(kDropCaches, "invalid"));
  EXPECT_EQ(errno, EINVAL);
}

struct SysctlTestReadBackParam {
  std::string path;
  const char *value;
};

class SysctlTestReadBack : public SysctlTest,
                           public ::testing::WithParamInterface<SysctlTestReadBackParam> {};

TEST_P(SysctlTestReadBack, ReadBack) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Need CAP_NET_ADMIN to run SysctlTestReadBack";
  }
  const auto &[path, value] = GetParam();
  std::string to_write = std::format("{}\n", value);
  ASSERT_TRUE(files::WriteFile(path, to_write)) << strerror(errno);
  std::string to_read;
  ASSERT_TRUE(files::ReadFileToString(path, &to_read)) << strerror(errno);
  ASSERT_EQ(to_read, to_write);
}

INSTANTIATE_TEST_SUITE_P(
    SysctlTest, SysctlTestReadBack,
    ::testing::Values(
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/neigh/default/ucast_solicit", "3"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv4/neigh/default/ucast_solicit", "3"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/neigh/default/mcast_resolicit", "3"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv4/neigh/default/mcast_resolicit", "3"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/conf/default/dad_transmits", "1"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/neigh/default/base_reachable_time_ms", "2000"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv4/neigh/default/base_reachable_time_ms", "2000"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/neigh/default/retrans_time_ms", "2000"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv4/neigh/default/retrans_time_ms", "2000"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/conf/default/use_tempaddr", "0"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/conf/default/use_tempaddr", "2"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/conf/default/accept_ra_defrtr", "0"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv6/conf/default/accept_ra_defrtr", "1"},
        SysctlTestReadBackParam{"/proc/sys/net/core/rmem_max", "6291456"},
        SysctlTestReadBackParam{"/proc/sys/net/core/wmem_max", "6291456"},
        SysctlTestReadBackParam{"/proc/sys/net/ipv4/tcp_rmem", "4096\t87380\t6291456"}),
    [](const testing::TestParamInfo<SysctlTestReadBackParam> &info) {
      auto path = std::filesystem::path(info.param.path);
      auto name = path.filename().string();
      auto version = std::next(path.begin(), 4)->string();
      std::string value = info.param.value;
      std::ranges::replace(value, '\t', '_');
      return std::format("{}_{}_{}", version, name, value);
    });

struct SysctlNodeParam {
  const char *path;
  bool readable = true;
  bool writable_by_root = true;
  bool writable = true;
};

class SysctlNodeTest : public testing::TestWithParam<SysctlNodeParam> {
 protected:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Need CAP_SYS_ADMIN to run this test";
    }
    const std::string sysctl_path = GetSysctlPath();
    if (access(sysctl_path.c_str(), F_OK) != 0) {
      if (!test_helper::IsStarnix()) {
        GTEST_SKIP() << sysctl_path << " does not exist on Linux host environment";
      } else {
        FAIL() << sysctl_path << " should exist";
      }
    }
  }

  static void DropToNonRootWithKeepcaps() {
    ASSERT_THAT(prctl(PR_SET_KEEPCAPS, 1), SyscallSucceeds());
    ASSERT_THAT(setresuid(99, 99, 99), SyscallSucceeds());
    ASSERT_EQ(getuid(), 99u);
  }

  std::string GetSysctlPath() const { return std::format("/proc/sys/{}", GetParam().path); }

  std::string GetNodeValueForWriteback(const std::string &path) const {
    std::string current_val;
    if (GetParam().readable) {
      EXPECT_TRUE(files::ReadFileToString(path, &current_val));
    } else {
      EXPECT_FALSE(files::ReadFileToString(path, &current_val));
      current_val = "1\n";
    }
    return current_val;
  }

  void TryWriteNode(const std::string &path, const std::string &val,
                    int expected_open_errno = EACCES, int expected_write_errno = EINVAL) const {
    if (!GetParam().writable_by_root) {
      EXPECT_THAT(open(path.c_str(), O_WRONLY), SyscallFailsWithErrno(expected_open_errno));
      return;
    }

    fbl::unique_fd fd(open(path.c_str(), O_WRONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    if (!GetParam().writable) {
      EXPECT_THAT(write(fd.get(), val.data(), val.size()),
                  SyscallFailsWithErrno(expected_write_errno));
    } else {
      EXPECT_THAT(write(fd.get(), val.data(), val.size()), SyscallSucceeds());
    }
  }
};

TEST_P(SysctlNodeTest, NonRootWithCapNetAdminCanWriteProcSysNet) {
  const std::string sysctl_path = GetSysctlPath();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    std::string current_val = GetNodeValueForWriteback(sysctl_path);

    DropToNonRootWithKeepcaps();

    // Give the user CAP_NET_ADMIN. For /proc/sys/net, this overrides DAC checks.
    test_helper::SetCapabilityEffective(CAP_NET_ADMIN);
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_NET_ADMIN));
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));

    bool is_net = sysctl_path.starts_with("/proc/sys/net/");
    if (is_net) {
      int fd_num = open(sysctl_path.c_str(), O_WRONLY);
      if (fd_num < 0) {
        EXPECT_THAT(fd_num, SyscallSucceeds());
        return;
      }
      fbl::unique_fd fd(fd_num);
      // Since we have CAP_NET_ADMIN, write should succeed regardless of whether the handler
      // requires it.
      EXPECT_THAT(write(fd.get(), current_val.data(), current_val.size()), SyscallSucceeds());
    } else {
      EXPECT_THAT(open(sysctl_path.c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
    }
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST_P(SysctlNodeTest, NonRootWithCapDacOverrideCannotWrite) {
  const std::string sysctl_path = GetSysctlPath();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    DropToNonRootWithKeepcaps();

    // Enable CAP_DAC_OVERRIDE but ensure CAP_NET_ADMIN is disabled.
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_NET_ADMIN));

    // Open for writing should fail, because CAP_DAC_OVERRIDE is ignored for /proc/sys.
    EXPECT_THAT(open(sysctl_path.c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST_P(SysctlNodeTest, CapDacOverrideHasNoEffectOnRootWrite) {
  const std::string sysctl_path = GetSysctlPath();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_NET_ADMIN));

    std::string current_val = GetNodeValueForWriteback(sysctl_path);
    TryWriteNode(sysctl_path, current_val);
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST_P(SysctlNodeTest, RootCanWriteWithoutCapDacOverrideAndNetAdmin) {
  const std::string sysctl_path = GetSysctlPath();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_NET_ADMIN));

    std::string current_val = GetNodeValueForWriteback(sysctl_path);
    TryWriteNode(sysctl_path, current_val);
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

const SysctlNodeParam kSysctlNodePaths[] = {
    {"debug/exception-trace"},
    {"fs/inotify/max_queued_events"},
    {"fs/inotify/max_user_instances"},
    {"fs/inotify/max_user_watches"},
    {"fs/pipe-max-size"},
    {"fs/protected_hardlinks"},
    {"fs/protected_symlinks"},
    {"fs/suid_dumpable"},
    {"kernel/core_pattern"},
    {"kernel/core_pipe_limit"},
    {"kernel/dmesg_restrict"},
    {"kernel/domainname"},
    {"kernel/hostname"},
    {"kernel/hung_task_check_count"},
    {"kernel/hung_task_panic"},
    {"kernel/hung_task_timeout_secs"},
    {"kernel/hung_task_warnings"},
    {"kernel/io_uring_disabled"},
    {"kernel/io_uring_group"},
    {"kernel/kptr_restrict"},
    {"kernel/modprobe"},

    // `writable` is false for modules_disabled because modules_disabled natively rejects any writes
    // with EINVAL if it is 0 (since it only accepts a transition to 1) and also rejects writes with
    // EINVAL if it is already 1 (since it is a strict one-way security toggle that locks the
    // kernel).
    {.path = "kernel/modules_disabled", .writable = false},
    {"kernel/overflowgid"},
    {"kernel/overflowuid"},
    {"kernel/panic_on_oops"},
    {"kernel/perf_cpu_time_max_percent"},
    {"kernel/perf_event_max_sample_rate"},
    {"kernel/perf_event_mlock_kb"},
    {"kernel/perf_event_paranoid"},
    {"kernel/pid_max"},
    {"kernel/printk"},

    // boot_id has 0444 permissions, so Root (owner) doesn't have write permission since DAC
    // override is ignored for /proc/sys.
    {.path = "kernel/random/boot_id", .writable_by_root = false},
    {"kernel/randomize_va_space"},
    {"kernel/sched_child_runs_first"},
    {"kernel/sched_latency_ns"},
    {"kernel/sched_lib_mask"},
    {"kernel/sched_lib_name"},
    {"kernel/sched_rt_period_us"},
    {"kernel/sched_rt_runtime_us"},
    {"kernel/sched_schedstats"},
    {"kernel/sched_tunable_scaling"},
    {"kernel/sched_wakeup_granularity_ns"},
    {"kernel/seccomp/actions_logged"},
    {"kernel/sysrq"},
    {"kernel/tainted"},
    {"kernel/unprivileged_bpf_disabled"},
    {"kernel/yama/ptrace_scope"},
    {"lsm/image_init"},
    {"net/core/rmem_max"},
    {"net/core/wmem_max"},
    {"net/ipv4/conf/all/accept_redirects"},
    {"net/ipv4/neigh/default/base_reachable_time_ms"},
    {"net/ipv4/neigh/default/mcast_resolicit"},
    {"net/ipv4/neigh/default/retrans_time_ms"},
    {"net/ipv4/neigh/default/ucast_solicit"},
    {"net/ipv4/ping_group_range"},
    {"net/ipv4/tcp_rmem"},
    {"net/ipv6/conf/all/accept_ra_rt_table"},
    {"net/ipv6/conf/all/disable_ipv6"},
    {"net/ipv6/conf/default/accept_ra_defrtr"},
    {"net/ipv6/conf/default/accept_ra_info_min_plen"},
    {"net/ipv6/conf/default/accept_ra_rt_table"},
    {"net/ipv6/conf/default/dad_transmits"},
    {"net/ipv6/conf/default/disable_ipv6"},
    {"net/ipv6/conf/default/use_tempaddr"},
    {"net/ipv6/neigh/default/base_reachable_time_ms"},
    {"net/ipv6/neigh/default/mcast_resolicit"},
    {"net/ipv6/neigh/default/retrans_time_ms"},
    {"net/ipv6/neigh/default/ucast_solicit"},
    {"user/max_user_namespaces"},
    {"vm/dirty_background_ratio"},
    {"vm/dirty_expire_centisecs"},
    {.path = "vm/drop_caches", .readable = false},
    {"vm/extra_free_kbytes"},
    {"vm/max_map_count"},
    {"vm/mmap_min_addr"},
    {"vm/mmap_rnd_bits"},
    {"vm/mmap_rnd_compat_bits"},
    {"vm/overcommit_memory"},
    {"vm/page-cluster"},
    {"vm/watermark_scale_factor"},
};

INSTANTIATE_TEST_SUITE_P(SysctlTest, SysctlNodeTest, testing::ValuesIn(kSysctlNodePaths),
                         [](const testing::TestParamInfo<SysctlNodeParam> &info) {
                           std::string name = info.param.path;
                           std::ranges::replace_if(
                               name, [](char c) { return c == '/' || c == '-'; }, '_');
                           return name;
                         });

}  // namespace
