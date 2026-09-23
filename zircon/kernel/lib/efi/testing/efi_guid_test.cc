// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <efi/types.h>
#include <gmock/gmock.h>

namespace efi {

namespace {

using ::testing::_;
using ::testing::Return;

const std::vector<efi_guid> test_guids = {
    {0x00000000, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
    {0x00000000, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01}},
    {0x00000000, 0x0000, 0x0001, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
    {0x00000000, 0x0001, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
    {0x00000001, 0x0000, 0x0000, {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}},
    {0x01020304, 0x0506, 0x0708, {0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10}},
    {0xffffffff, 0xffff, 0xffff, {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff}},
};

// Make sure struct is packed
TEST(EfiGuid, Size) {
  const auto struct_size = sizeof(efi_guid);
  const auto members_size = sizeof(efi_guid::data1) + sizeof(efi_guid::data2) +
                            sizeof(efi_guid::data3) + sizeof(efi_guid::data4);
  EXPECT_EQ(struct_size, members_size);
}

TEST(EfiGuid, Equal) {
  for (auto& it : test_guids) {
    EXPECT_EQ(it, it);
  }
}

TEST(EfiGuid, NotEqual) {
  for (size_t i = 0; i < test_guids.size(); i++) {
    for (size_t j = i + 1; j < test_guids.size(); j++) {
      EXPECT_NE(test_guids[i], test_guids[j]);
    }
  }
}

TEST(EfiGuid, LessThan) {
  for (size_t i = 0; i < test_guids.size(); i++) {
    for (size_t j = i + 1; j < test_guids.size(); j++) {
      EXPECT_LT(test_guids[i], test_guids[j]);
    }
  }
}

}  // namespace

}  // namespace efi
