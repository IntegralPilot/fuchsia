// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_utils.h"

#include <gtest/gtest.h>

namespace debug_agent {
namespace linux {

TEST(LinuxUtils, ParseMaps) {
  // Empty input.
  std::vector<MapEntry> result = ParseMaps("");
  EXPECT_TRUE(result.empty());

  result = ParseMaps("\n");
  EXPECT_TRUE(result.empty());

  const char kTestInput[] =
      R"(562288f50000-562288f67000 r--p 00000000 fe:01 11534420                   /usr/bin/zsh
562288f67000-562288ffa000 r-xp 00017000 fe:01 11534420                   /usr/bin/zsh
562288ffa000-56228901c000 r--p 000aa000 fe:01 11534420                   /usr/bin/zsh
562289024000-562289038000 rw-p 00000000 00:00 0 
56228966d000-5622897d2000 rw-p 00000000 00:00 0                          [heap]
7fdff47df000-7fdff4a5f000 r--s 00000000 fe:01 14559256                   /usr/share/zsh/functions/Completion/Unix.zwc
7fdff4a6a000-7fdff4a6d000 r--p 00000000 fe:01 12720175                   /usr/lib/x86_64-linux-gnu/zsh/5.8/zsh/computil
.so
)";
  result = ParseMaps(kTestInput);
  ASSERT_EQ(7u, result.size());

  // Check all of the details of one of the lines.
  EXPECT_EQ(0x562288f67000ull, result[1].range.begin());
  EXPECT_EQ(0x562288ffa000ull, result[1].range.end());
  EXPECT_TRUE(result[1].read);
  EXPECT_FALSE(result[1].write);
  EXPECT_TRUE(result[1].execute);
  EXPECT_FALSE(result[1].shared);
  EXPECT_EQ(0x17000ull, result[1].offset);
  EXPECT_EQ("fe:01", result[1].device);
  EXPECT_EQ("/usr/bin/zsh", result[1].path);
}

TEST(LinuxUtils, ParseStatus) {
  // Empty input.
  std::map<std::string, std::string> result = ParseStatus("");
  EXPECT_TRUE(result.empty());

  result = ParseStatus("\n");
  EXPECT_TRUE(result.empty());

  const char kTestInput[] = R"(Name:   zsh
Umask:  0022
State:  S (sleeping)
Tgid:   1738
Ngid:   0
Pid:    1738
PPid:   1736
TracerPid:      0
Uid:    1000    1000    1000    1000
Gid:    1000    1000    1000    1000
FDSize: 64
Groups: 1000 1001 665357 20 24 25 27 29 44 46 100 109 
NStgid: 1738
NSpid:  1738
NSpgid: 1738
NSsid:  1738
VmPeak:     8912 kB
VmSize:     8896 kB
VmLck:         0 kB
VmPin:         0 kB
VmHWM:      4772 kB
VmRSS:      4772 kB
RssAnon:             720 kB
RssFile:            4052 kB
RssShmem:              0 kB
VmData:      660 kB
VmStk:       132 kB
VmExe:       600 kB
VmLib:      2600 kB
VmPTE:        52 kB
VmSwap:        0 kB
HugetlbPages:          0 kB
CoreDumping:    0
THP_enabled:    1
Threads:        1
SigQ:   2/120935
SigPnd: 0000000000000000
ShdPnd: 0000000000000000
SigBlk: 0000000000000002
SigIgn: 0000000000384004
SigCgt: 0000000008013003
CapInh: 0000000000000000
CapPrm: 0000000000000000
CapEff: 0000000000000000
CapBnd: 000001ffffffffff
CapAmb: 0000000000000000
NoNewPrivs:     0
Seccomp:        2
Seccomp_filters:        1
Speculation_Store_Bypass:       thread force mitigated
SpeculationIndirectBranch:      conditional force disabled
Cpus_allowed:   fff
Cpus_allowed_list:      0-11
Mems_allowed:   1
Mems_allowed_list:      0
voluntary_ctxt_switches:        1207
nonvoluntary_ctxt_switches:     152
)";
  result = ParseStatus(kTestInput);

  // Spot check some values.
  EXPECT_EQ("zsh", result["Name"]);
  EXPECT_EQ("1738", result["Pid"]);
}

}  // namespace linux
}  // namespace debug_agent
