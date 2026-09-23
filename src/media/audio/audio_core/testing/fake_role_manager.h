// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_TESTING_FAKE_ROLE_MANAGER_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_TESTING_FAKE_ROLE_MANAGER_H_

#include <fuchsia/scheduler/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>

#include <cstdint>
#include <unordered_set>

namespace media::audio {

class FakeRoleManager : public fuchsia::scheduler::RoleManager {
 public:
  fidl::InterfaceRequestHandler<fuchsia::scheduler::RoleManager> GetHandler() {
    return bindings_.GetHandler(this);
  }

 private:
  // |fuchsia::scheduler::RoleManager|
  void SetRole(fuchsia::scheduler::RoleManagerSetRoleRequest req,
               SetRoleCallback callback) override {
    callback(fuchsia::scheduler::RoleManager_SetRole_Result::WithResponse(
        fuchsia::scheduler::RoleManager_SetRole_Response{}));
  }

  void GetProfileForRole(fuchsia::scheduler::RoleManagerGetProfileForRoleRequest req,
                         GetProfileForRoleCallback callback) override {
    callback(fuchsia::scheduler::RoleManager_GetProfileForRole_Result::WithResponse(
        fuchsia::scheduler::RoleManager_GetProfileForRole_Response{}));
  }

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

  fidl::BindingSet<fuchsia::scheduler::RoleManager> bindings_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_TESTING_FAKE_ROLE_MANAGER_H_
