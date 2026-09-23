// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SCHEDULER_ROLE_H_
#define LIB_SCHEDULER_ROLE_H_

#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>

#include <string_view>
#include <variant>
#include <vector>

//
// # Fuchsia Scheduler C++ API
//
// Utility functions for calling the fuchsia.scheduler.ProfileProvider role API.
//
// These functions automatically handle service connection and handle duplication to minimize
// client boilerplate.
//

namespace fuchsia_scheduler {

using RoleParameterValue = std::variant<double, int64_t, std::string>;

// Represents a single out parameter for a scheduler role.
struct RoleParameter {
  std::string name;
  RoleParameterValue value;

  // For testing.
  const RoleParameter& operator<=>(const RoleParameter&) const = default;
};

zx_status_t SetRoleForVmar(zx::unowned_vmar vmar, std::string_view role);
zx_status_t SetRoleForRootVmar(std::string_view role);

zx_status_t SetRoleForThread(zx::unowned_thread thread, std::string_view role);
zx::result<std::vector<RoleParameter>> SetRoleForThread(
    zx::unowned_thread borrowed_thread, std::string_view role,
    std::vector<RoleParameter>& input_parameters);

zx_status_t SetRoleForThisThread(std::string_view role);
zx::result<std::vector<RoleParameter>> SetRoleForThisThread(
    std::string_view role, std::vector<RoleParameter>& input_parameters);

}  // namespace fuchsia_scheduler

#endif  // LIB_SCHEDULER_ROLE_H_
