// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/auxv.h>

#include <array>
#include <fstream>
#include <sstream>
#include <string>

#include "gtest/gtest.h"

class CpuinfoTest : public ::testing::Test {};

TEST_F(CpuinfoTest, Cpuinfo) {
#if !defined(__arm__)
  GTEST_SKIP() << "we only test arm32";
#endif
  std::stringstream ss;
  static constexpr std::array<std::string_view, 28> hwcap_str = {
      "swp",      "half", "thumb",   "26bit",   "fastmult", "fpa",       "vfp",
      "edsp",     "java", "iwmmxt",  "crunch",  "thumbee",  "neon",      "vfpv3",
      "vfpv3d16", "tls",  "vfpv4",   "idiva",   "idivt",    "vfpd32",    "lpae",
      "evtstrm",  "fphp", "asimdhp", "asimddp", "asimdfhm", "asimdbf16", "i8mm",
  };

  static constexpr std::array<std::string_view, 7> hwcap2_str = {
      "aes", "pmull", "sha1", "sha2", "crc32", "sb", "ssbs",
  };
  ss << "Features\t:";
  for (size_t i = 0; i < hwcap_str.size(); i++) {
    if (getauxval(AT_HWCAP) & (1 << i)) {
      ss << " " << hwcap_str[i];
    }
  }
  for (size_t i = 0; i < hwcap2_str.size(); i++) {
    if (getauxval(AT_HWCAP2) & (1 << i)) {
      ss << " " << hwcap2_str[i];
    }
  }
  const auto expected_features_line = ss.str();

  std::string line;
  std::fstream cpuinfo("/proc/cpuinfo");

  while (std::getline(cpuinfo, line)) {
    if (line.starts_with("Features\t:")) {
      EXPECT_EQ(line, expected_features_line);
      break;
    }
  }
}
