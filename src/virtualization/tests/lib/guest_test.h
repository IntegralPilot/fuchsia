// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_TESTS_LIB_GUEST_TEST_H_
#define SRC_VIRTUALIZATION_TESTS_LIB_GUEST_TEST_H_

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <memory>

#include <fbl/type_info.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/virtualization/tests/lib/enclosed_guest.h"

template <class T>
class GuestTest : public gtest::RealLoopFixture {
 public:
  GuestTest()
      : enclosed_guest_(dispatcher(),
                        [this](fit::function<bool()> condition, zx::duration timeout) {
                          return RunLoopWithTimeoutOrUntil(std::move(condition), timeout);
                        }) {}

 protected:
  void SetUp() override {
    FX_LOGS(INFO) << "Guest: " << fbl::TypeInfo<T>::Name();
    zx_status_t status = GetEnclosedGuest().Start(zx::time::infinite());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  void TearDown() override {
    FX_LOGS(INFO) << "Teardown Guest: " << fbl::TypeInfo<T>::Name();
    zx_status_t status = GetEnclosedGuest().Stop(zx::time::infinite());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  zx_status_t Execute(const std::vector<std::string>& argv, std::string* result = nullptr,
                      int32_t* return_code = nullptr) {
    return GetEnclosedGuest().Execute(argv, {}, zx::time::infinite(), result, return_code);
  }

  zx_status_t Execute(const std::vector<std::string>& argv,
                      const std::unordered_map<std::string, std::string>& env,
                      std::string* result = nullptr, int32_t* return_code = nullptr) {
    return GetEnclosedGuest().Execute(argv, env, zx::time::infinite(), result, return_code);
  }

  zx_status_t RunUtil(const std::string& util, const std::vector<std::string>& argv,
                      std::string* result = nullptr) {
    return GetEnclosedGuest().RunUtil(util, argv, zx::time::infinite(), result);
  }

  bool RunLoopUntil(fit::function<bool()> condition, zx::time deadline) {
    return GetEnclosedGuest().RunLoopUntil(std::move(condition), deadline);
  }

  GuestKernel GetGuestKernel() { return GetEnclosedGuest().GetGuestKernel(); }

  uint32_t GetGuestCid() { return GetEnclosedGuest().GetGuestCid(); }

  bool GetHostVsockEndpoint(
      fidl::InterfaceRequest<fuchsia::virtualization::HostVsockEndpoint> endpoint) {
    return GetEnclosedGuest().GetHostVsockEndpoint(std::move(endpoint)).is_ok();
  }

  bool ConnectToBalloon(
      fidl::InterfaceRequest<fuchsia::virtualization::BalloonController> balloon_controller) {
    return GetEnclosedGuest().ConnectToBalloon(std::move(balloon_controller)).is_ok();
  }

  bool ConnectToMem(fidl::InterfaceRequest<fuchsia::virtualization::MemController> mem_controller) {
    return GetEnclosedGuest().ConnectToMem(std::move(mem_controller)).is_ok();
  }

  T& GetEnclosedGuest() { return enclosed_guest_; }

 private:
  T enclosed_guest_;
};

#endif  // SRC_VIRTUALIZATION_TESTS_LIB_GUEST_TEST_H_
