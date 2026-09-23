// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_PROFILE_H_
#define SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_PROFILE_H_

#include <fidl/fuchsia.scheduler.deprecated/cpp/wire.h>
#include <lib/zx/resource.h>

#include "config.h"

class ProfileProvider : public fidl::WireServer<fuchsia_scheduler_deprecated::ProfileProvider> {
 public:
  static zx::result<std::unique_ptr<ProfileProvider>> Create();

 private:
  ProfileProvider(zx::resource profile_rsrc, zircon_profile::ConfiguredProfiles profiles)
      : profile_rsrc_(std::move(profile_rsrc)), profiles_(std::move(profiles)) {}

  void GetProfile(GetProfileRequestView request, GetProfileCompleter::Sync& completer) override;

  void GetDeadlineProfile(GetDeadlineProfileRequestView request,
                          GetDeadlineProfileCompleter::Sync& completer) override;

  void GetCpuAffinityProfile(GetCpuAffinityProfileRequestView request,
                             GetCpuAffinityProfileCompleter::Sync& completer) override;

  void SetProfileByRole(SetProfileByRoleRequestView request,
                        SetProfileByRoleCompleter::Sync& completer) override;

  fidl::ServerBindingGroup<fuchsia_scheduler_deprecated::ProfileProvider> bindings_;
  zx::resource profile_rsrc_;
  zircon_profile::ConfiguredProfiles profiles_;
};

#endif  // SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_PROFILE_H_
