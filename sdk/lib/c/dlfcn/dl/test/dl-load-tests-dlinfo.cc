// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>

#include "dl-iterate-phdr-tests.h"
#include "dl-load-tests.h"

// Fuchsia-Musl doesn't define or support RTLD_DI_TLS_* options. Define them
// here so tests that use them can compile, even though these tests will not be
// run against Musl.
#ifndef RTLD_DI_TLS_MODID
#define RTLD_DI_TLS_MODID 9
#endif

#ifndef RTLD_DI_TLS_DATA
#define RTLD_DI_TLS_DATA 10
#endif

#ifndef RTLD_DI_PHDR
#define RTLD_DI_PHDR 11
#endif

namespace {

using dl::testing::GetPhdrInfoForModule;
using dl::testing::TestModule;

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

// Test dlinfo with an unknown RTLD_DI_* flag.
TYPED_TEST(DlTests, DlInfoBadFlag) {
  const std::string kRet17File = TestModule("ret17");
  struct link_map *link_map = nullptr;

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), -1, &link_map);
  ASSERT_TRUE(res.is_error());

  if constexpr (TestFixture::kEmitsDlInfoUnsupportedValue) {
    EXPECT_EQ(res.error_value().take_str(), "Unsupported request -1");
  } else {
    EXPECT_EQ(res.error_value().take_str(), "unsupported dlinfo request");
  }

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_LINKMAP.
TYPED_TEST(DlTests, DlInfoRtldDiLinkMap) {
  const std::string kRet17File = TestModule("ret17");
  struct link_map *link_map = nullptr;

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_LINKMAP, &link_map);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  EXPECT_EQ(res.value(), 0);

  // Expect the link_map to point to the same link map owned by the module
  // returned from dlopen. This is a redundant check for testing the system
  // dlinfo since DlSystemTests::ModuleLinkMap also calls dlinfo to return the
  // link_map value. But this will verify that libdl's dlinfo will set the right
  // pointer value, since DlImplTests::ModuleLinkMap returns a pointer to the
  // struct link_map embedded in the module's ABI.
  EXPECT_EQ(link_map, this->ModuleLinkMap(open.value()));

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_TLS_MODID.
TYPED_TEST(DlTests, DlInfoRtldDiTlsModid) {
  const std::string kTlsVarFile = "libstatic-tls-var.so";
  size_t tls_modid = 0;

  if (!TestFixture::kSupportsDlInfoExtensionFlags) {
    GTEST_SKIP() << "test requires fixture to support RTLD_DI_TLS_MODID";
  }

  // There is no "Needed" call here to prime the mock loader since the shared
  // object is already loaded as a startup module.

  auto open = this->DlOpen(kTlsVarFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_TLS_MODID, &tls_modid);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  EXPECT_EQ(res.value(), 0);

  EXPECT_NE(tls_modid, 0u);

  auto phdr_info = GetPhdrInfoForModule(*this, kTlsVarFile);
  EXPECT_EQ(tls_modid, phdr_info.tls_modid());

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_TLS_MODID on a module that does not have TLS.
TYPED_TEST(DlTests, DlInfoRtldDiTlsModidNoTls) {
  const std::string kRet17File = TestModule("ret17");
  // Initialize to non-zero value so that dlinfo() correctly sets to zero later.
  size_t tls_modid = 1;

  if (!TestFixture::kSupportsDlInfoExtensionFlags) {
    GTEST_SKIP() << "test requires fixture to support RTLD_DI_TLS_MODID";
  }

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_TLS_MODID, &tls_modid);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  EXPECT_EQ(res.value(), 0);

  // dlinfo() will set tls_modid to 0 if there is no TLS segment.
  EXPECT_EQ(tls_modid, 0u);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_TLS_DATA.
TYPED_TEST(DlTests, DlInfoRtldDiTlsData) {
  const std::string kTlsVarFile = "libstatic-tls-var.so";
  void *tls_data = nullptr;

  if (!TestFixture::kSupportsDlInfoExtensionFlags) {
    GTEST_SKIP() << "test requires fixture to support RTLD_DI_TLS_DATA";
  }

  // There is no "Needed" call here to prime the mock loader since the shared
  // object is already loaded as a startup module.

  auto open = this->DlOpen(kTlsVarFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_TLS_DATA, &tls_data);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  EXPECT_EQ(res.value(), 0);

  EXPECT_TRUE(tls_data);

  auto phdr_info = GetPhdrInfoForModule(*this, kTlsVarFile);
  // Expect dlinfo() will return the same address that is found by
  // dl_iterate_phdr.
  EXPECT_EQ(tls_data, phdr_info.tls_data());

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_TLS_DATA on a module that does not have TLS.
TYPED_TEST(DlTests, DlInfoRtldDiTlsDataNoTls) {
  const std::string kRet17File = TestModule("ret17");
  // Initialize to a non-null value so that dlinfo() correctly sets to null
  // later.
  void *tls_data = this;

  if (!TestFixture::kSupportsDlInfoExtensionFlags) {
    GTEST_SKIP() << "test requires fixture to support RTLD_DI_TLS_DATA";
  }

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_TLS_DATA, &tls_data);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  EXPECT_EQ(res.value(), 0);

  // dlinfo() will set tls_data to NULL if there is no TLS segment.
  EXPECT_EQ(tls_data, nullptr);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test dlinfo with RTLD_DI_PHDRS.
TYPED_TEST(DlTests, DlInfoRtldDiPhdrs) {
  const std::string kRet17File = TestModule("ret17");
  const ElfW(Phdr) *phdrs = nullptr;

  if (!TestFixture::kSupportsDlInfoExtensionFlags) {
    GTEST_SKIP() << "test requires fixture to support RTLD_DI_PHDRS";
  }

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), RTLD_DI_PHDR, &phdrs);
  ASSERT_TRUE(res.is_ok()) << res.error_value();
  // The dlinfo() return value (res.value()) is the number of phdrs in the
  // module, checked below.

  auto phdr_info = GetPhdrInfoForModule(*this, kRet17File);
  EXPECT_EQ(res.value(), phdr_info.phnum());

  for (int i = 0; i < res.value(); ++i) {
    EXPECT_EQ(&phdrs[i], reinterpret_cast<const ElfW(Phdr *)>(&phdr_info.phdrs()[i]));
  }

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

}  // namespace
