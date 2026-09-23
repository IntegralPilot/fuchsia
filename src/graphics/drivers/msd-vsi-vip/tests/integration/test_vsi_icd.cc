// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_client/test_util/test_device_helper.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "magma_vendor_queries.h"

TEST(TestIcd, IcdList) {
  magma::TestDeviceBase test_device(MAGMA_VENDOR_ID_VSI);

  fidl::UnownedClientEnd<fuchsia_gpu_magma::TestDevice> channel{test_device.magma_channel()};
  const fidl::WireResult result = fidl::WireCall(channel)->GetIcdList();
  EXPECT_TRUE(result.ok()) << result.FormatDescription();
  const fidl::WireResponse response = result.value();
  EXPECT_EQ(response.icd_list.size(), 2u);

  const auto& icd_item = response.icd_list[0];

  EXPECT_TRUE(icd_item.has_flags());
  const auto& flags = icd_item.flags();
  EXPECT_TRUE(flags & fuchsia_gpu_magma::wire::IcdFlags::kSupportsOpencl);

  std::string res_string(icd_item.component_url().get());
  EXPECT_EQ(res_string.length(), icd_item.component_url().size());
  EXPECT_EQ(0u, res_string.find("fuchsia-pkg://fuchsia.com/libopencl_vsi_vip_"));
  EXPECT_THAT(res_string, testing::EndsWith("_test#meta/opencl.cm"));
}
