// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "common.h"

#include <fidl/fuchsia.sysinfo/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <zxtest/zxtest.h>

zx_koid_t get_koid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  ZX_ASSERT(ZX_OK == zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                        nullptr));
  return info.koid;
}

zx_koid_t get_related_koid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  ZX_ASSERT(ZX_OK == zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                        nullptr));
  return info.related_koid;
}

const std::string& GetBoardName() {
  static std::string s_board_name;
  if (s_board_name.empty()) {
    zx::result client_end = component::Connect<fuchsia_sysinfo::SysInfo>();
    EXPECT_OK(client_end.status_value());

    fidl::WireSyncClient sysinfo{std::move(client_end.value())};
    auto result = sysinfo->GetBoardName();
    EXPECT_OK(result.status());
    EXPECT_OK(result.value().status);

    s_board_name = result.value().name.get();
    printf("\nFound board %s\n", s_board_name.c_str());
  }
  return s_board_name;
}

bool is_board_astro() { return GetBoardName() == "astro"; }

bool is_board_sherlock() { return GetBoardName() == "sherlock"; }

bool is_board_luis() { return GetBoardName() == "luis"; }

bool is_board_nelson() { return GetBoardName() == "nelson"; }

bool is_board_with_amlogic_secure() {
  if (is_board_astro()) {
    return true;
  }
  if (is_board_sherlock()) {
    return true;
  }
  if (is_board_luis()) {
    return true;
  }
  if (is_board_nelson()) {
    return true;
  }
  return false;
}

bool is_board_with_amlogic_secure_vdec() { return is_board_with_amlogic_secure(); }

void nanosleep_duration(zx::duration duration) {
  ZX_ASSERT(ZX_OK == zx::nanosleep(zx::deadline_after(duration)));
}
