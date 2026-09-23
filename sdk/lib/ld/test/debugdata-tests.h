// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_DEBUGDATA_TESTS_H_
#define LIB_LD_TEST_DEBUGDATA_TESTS_H_

#include <lib/zx/eventpair.h>
#include <lib/zx/vmo.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

namespace ld::testing {

constexpr std::string_view kSinkName = "test-sink";
constexpr std::string_view kVmoName = "test-vmo";
constexpr std::string_view kVmoContents = "test-vmo contents";
constexpr std::string_view kNotIt = "not it";

template <zx_koid_t zx_info_handle_basic_t::* Member = &zx_info_handle_basic_t::koid>
inline void GetKoid(const zx::object_base& obj, zx_koid_t& koid) {
  zx_info_handle_basic_t info;
  zx_status_t status = obj.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  koid = info.*Member;
}

inline void MakeEventPair(zx::eventpair& eventpair0, zx::eventpair& eventpair1) {
  zx_status_t status = zx::eventpair::create(0, &eventpair0, &eventpair1);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

inline void MakeTestVmo(zx::vmo& vmo) {
  zx_status_t status = zx::vmo::create(kVmoContents.size(), 0, &vmo);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  status = vmo.set_property(ZX_PROP_NAME, kVmoName.data(), kVmoName.size());
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  status = vmo.write(kVmoContents.data(), 0, kVmoContents.size());
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
}

}  // namespace ld::testing

#endif  // LIB_LD_TEST_DEBUGDATA_TESTS_H_
