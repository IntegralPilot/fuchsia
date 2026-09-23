// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_INCOMING_CPP_SERVICE_VALIDATOR_H
#define LIB_DRIVER_INCOMING_CPP_SERVICE_VALIDATOR_H

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>

#include <unordered_set>
#include <vector>

namespace fdf {

class ServiceValidator {
 public:
  explicit ServiceValidator(const std::vector<fuchsia_driver_framework::Offer>& offers);

  bool IsValidZirconServiceInstance(const std::string& service_name,
                                    const std::string& instance) const;
  bool IsValidDriverServiceInstance(const std::string& service_name,
                                    const std::string& instance) const;

 private:
  std::unordered_map<std::string, std::unordered_set<std::string>>
      instance_to_zircon_service_mapping_;
  std::unordered_map<std::string, std::unordered_set<std::string>>
      instance_to_driver_service_mapping_;
};

}  // namespace fdf

#endif  // LIB_DRIVER_INCOMING_CPP_SERVICE_VALIDATOR_H
