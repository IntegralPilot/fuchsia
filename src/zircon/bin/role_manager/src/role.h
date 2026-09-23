// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_ROLE_H_
#define SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_ROLE_H_

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/resource.h>

#include "config.h"

class RoleManager : public fidl::WireServer<fuchsia_scheduler::RoleManager> {
 public:
  static zx::result<std::unique_ptr<RoleManager>> Create(inspect::Node& parent);
  void SetRole(SetRoleRequestView request, SetRoleCompleter::Sync& completer) override;
  void GetProfileForRole(GetProfileForRoleRequestView request,
                         GetProfileForRoleCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_scheduler::RoleManager> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  // Makes a best effort attempt to log the request to the syslog if a sufficient log level has
  // been specified.
  void LogRequest(SetRoleRequestView request);

  RoleManager(zx::resource profile_resource, zircon_profile::ConfiguredProfiles profiles)
      : profile_resource_(std::move(profile_resource)), profiles_(std::move(profiles)) {}
  zx::resource profile_resource_;
  zircon_profile::ConfiguredProfiles profiles_;

  void PublishInspect(inspect::Node& parent);
  fpromise::promise<inspect::Inspector> Inspect();
};

#endif  // SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_ROLE_H_
