// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_INTERNAL_INTERNALS_H_
#define LIB_DRIVER_TESTING_CPP_INTERNAL_INTERNALS_H_

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/internal/driver_lifecycle.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/driver/testing/cpp/test_node.h>

#include <type_traits>

namespace fdf_testing {

// Forward declare.
class Environment;

namespace internal {

template <typename EnvironmentType>
class EnvWrapper {
 public:
  fuchsia_driver_framework::DriverStartArgs Init() {
    zx::result start_args = node_server_.CreateStartArgsAndServe();
    ZX_ASSERT_MSG(start_args.is_ok(), "Failed to CreateStartArgsAndServe: %s.",
                  start_args.status_string());

    zx::result result =
        test_environment_.Initialize(std::move(start_args->incoming_directory_server));
    ZX_ASSERT_MSG(result.is_ok(), "Failed to Initialize the test_environment: %s.",
                  result.status_string());
    outgoing_client_ = std::move(start_args->outgoing_directory_client);

    if (!user_env_served_) {
      result = user_env_.Serve(test_environment_.incoming_directory());
      ZX_ASSERT_MSG(result.is_ok(), "Failed to Serve the user's Environment: %s",
                    result.status_string());
      user_env_served_ = true;
    }

    start_args->start_args.node_offers(std::vector<fuchsia_driver_framework::Offer>{});
    for (auto& [service, instances] : test_environment_.incoming_directory().GetDriverServices()) {
      for (auto& instance : instances) {
        start_args->start_args.node_offers()->push_back(
            fuchsia_driver_framework::Offer::WithDriverTransport(
                fuchsia_component_decl::Offer::WithService(fuchsia_component_decl::OfferService{{
                    .source_name = service,
                    .target_name = service,
                    .renamed_instances = std::vector<fuchsia_component_decl::NameMapping>(
                        {fuchsia_component_decl::NameMapping(instance, instance)}),
                }})));
      }
    }

    for (auto& [service, instances] : test_environment_.incoming_directory().GetZirconServices()) {
      for (auto& instance : instances) {
        start_args->start_args.node_offers()->push_back(
            fuchsia_driver_framework::Offer::WithZirconTransport(
                fuchsia_component_decl::Offer::WithService(fuchsia_component_decl::OfferService{{
                    .source_name = service,
                    .target_name = service,
                    .renamed_instances = std::vector<fuchsia_component_decl::NameMapping>(
                        {fuchsia_component_decl::NameMapping(instance, instance)}),
                }})));
      }
    }

    return std::move(start_args->start_args);
  }

  fidl::ClientEnd<fuchsia_io::Directory> TakeOutgoingClient() {
    ZX_ASSERT_MSG(outgoing_client_.is_valid(), "Cannot call TakeOutgoingClient more than once.");
    return std::move(outgoing_client_);
  }

  fdf_testing::TestNode& node_server() { return node_server_; }

  EnvironmentType& user_env() { return user_env_; }

 private:
  fdf_testing::TestNode node_server_{"root"};
  TestEnvironment test_environment_;
  fidl::ClientEnd<fuchsia_io::Directory> outgoing_client_;

  // User env should be the last field as it could contain references to the test_environment_.
  EnvironmentType user_env_;
  bool user_env_served_ = false;
};

#if __cplusplus >= 202002l
template <typename T>
concept HasDriverType = requires { typename T::DriverType; };

template <typename T>
concept HasEnvironmentType =
    requires { typename T::EnvironmentType; } &&
    std::is_base_of_v<fdf_testing::Environment, typename T::EnvironmentType> &&
    !std::is_abstract_v<typename T::EnvironmentType> &&
    std::is_default_constructible_v<typename T::EnvironmentType>;

template <typename T>
concept HasGetDriverRegistration = requires {
  { T::GetDriverRegistration };
};
#endif

}  // namespace internal

}  // namespace fdf_testing

#endif  // LIB_DRIVER_TESTING_CPP_INTERNAL_INTERNALS_H_
