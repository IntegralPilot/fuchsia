// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/testing/mock-loader-service.h>

namespace {

using ld::testing::MockLoaderService;

void CreateVmo(zx::vmo& vmo, zx_info_handle_basic_t& info) {
  ASSERT_EQ(zx::vmo::create(0, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr), ZX_OK);
}

zx::result<zx::vmo> MakeLoadObjectRequest(fidl::ClientEnd<fuchsia_ldsvc::Loader>& client,
                                          std::string_view name) {
  auto result = fidl::WireCall(client)->LoadObject(fidl::StringView::FromExternal(name));
  EXPECT_EQ(result.status(), ZX_OK);
  auto load_result = result.Unwrap();
  if (load_result->rv != ZX_OK) {
    return zx::error(load_result->rv);
  }
  return zx::ok(std::move(load_result->object));
}

TEST(LdTests, MockLoaderServiceLoadObject) {
  MockLoaderService mock_loader_service;
  ASSERT_NO_FATAL_FAILURE(mock_loader_service.Init());

  constexpr std::string_view kFoo = "foo.so";
  constexpr std::string_view kBaz = "baz.so";

  zx::vmo vmo;
  zx_info_handle_basic_t info;
  CreateVmo(vmo, info);

  mock_loader_service.ExpectLoadObject(kFoo, zx::ok(std::move(vmo)));
  mock_loader_service.ExpectLoadObject(kBaz, zx::error(ZX_ERR_NOT_FOUND));

  zx_info_handle_basic_t result_info;
  auto foo_result = MakeLoadObjectRequest(mock_loader_service.client(), kFoo);
  EXPECT_EQ(foo_result.status_value(), ZX_OK);
  ASSERT_EQ(foo_result->get_info(ZX_INFO_HANDLE_BASIC, &result_info, sizeof(result_info), nullptr,
                                 nullptr),
            ZX_OK);
  EXPECT_EQ(info.koid, result_info.koid);

  auto baz_result = MakeLoadObjectRequest(mock_loader_service.client(), kBaz);
  EXPECT_EQ(baz_result.status_value(), ZX_ERR_NOT_FOUND);
}

TEST(LdTests, MockLoaderServiceMultipleRequestsSameObject) {
  MockLoaderService mock_loader_service;
  ASSERT_NO_FATAL_FAILURE(mock_loader_service.Init());

  constexpr std::string_view kFoo = "foo.so";

  zx::vmo vmo;
  zx_info_handle_basic_t info;
  CreateVmo(vmo, info);

  mock_loader_service.ExpectLoadObject(kFoo, zx::ok(std::move(vmo)));
  mock_loader_service.ExpectLoadObject(kFoo, zx::error(ZX_ERR_NOT_FOUND));

  // Test that the first request for "foo.so" succeeds per first expectation.
  zx_info_handle_basic_t result_info;
  auto foo1_result = MakeLoadObjectRequest(mock_loader_service.client(), kFoo);
  EXPECT_EQ(foo1_result.status_value(), ZX_OK);
  ASSERT_EQ(foo1_result->get_info(ZX_INFO_HANDLE_BASIC, &result_info, sizeof(result_info), nullptr,
                                  nullptr),
            ZX_OK);
  EXPECT_EQ(info.koid, result_info.koid);

  // Test that the second request for "foo.so" fails fails per second expectation.
  auto foo2_result = MakeLoadObjectRequest(mock_loader_service.client(), kFoo);
  EXPECT_EQ(foo2_result.status_value(), ZX_ERR_NOT_FOUND);
}

TEST(LdTests, MockLoaderServiceConfig) {
  MockLoaderService mock_loader_service;
  ASSERT_NO_FATAL_FAILURE(mock_loader_service.Init());

  constexpr std::string_view kFoo = "foo.so";
  constexpr std::string_view kBaz = "baz.so";

  mock_loader_service.ExpectConfig(kFoo, zx::ok());
  mock_loader_service.ExpectConfig(kBaz, zx::error(ZX_ERR_NOT_FOUND));

  auto foo_result =
      fidl::WireCall(mock_loader_service.client())->Config(fidl::StringView::FromExternal(kFoo));
  // expect FIDL call success.
  EXPECT_EQ(foo_result.status(), ZX_OK);
  // expect Config result success.
  EXPECT_EQ(foo_result.Unwrap()->rv, ZX_OK);

  auto baz_result =
      fidl::WireCall(mock_loader_service.client())->Config(fidl::StringView::FromExternal(kBaz));
  // expect FIDL call success.
  EXPECT_EQ(baz_result.status(), ZX_OK);
  // expect Config result failure.
  EXPECT_EQ(baz_result.Unwrap()->rv, ZX_ERR_NOT_FOUND);
}

}  // namespace
