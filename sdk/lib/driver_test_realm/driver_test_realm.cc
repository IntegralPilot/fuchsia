// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver_test_realm/src/boot_items.h>
#include <lib/driver_test_realm/src/internal_server.h>
#include <lib/driver_test_realm/src/root_job.h>
#include <lib/driver_test_realm/src/system_state.h>
#include <lib/fdio/directory.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <sdk/lib/driver_test_realm/driver_test_realm_config.h>

namespace {

namespace fio = fuchsia_io;
namespace fdt = fuchsia_driver_test;
namespace fres = fuchsia_component_resolution;

using namespace component_testing;

class DriverTestRealm final : public fidl::Server<fuchsia_driver_test::Realm> {
 public:
  DriverTestRealm(component::OutgoingDirectory* outgoing, async_dispatcher_t* dispatcher,
                  driver_test_realm_config::Config config)
      : outgoing_(outgoing), dispatcher_(dispatcher), config_(config) {}

  zx::result<> Init() {
    // Set up realm_builder_exposed_dir now so that we can queue up requests before `Start` has been
    // called.  When `Start` has been called, we'll connect the directory to the exposed directory
    // of the started realm.
    {
      constexpr std::string_view kRealmBuilderExposedDir = "realm_builder_exposed_dir";

      zx::result client_end = fidl::CreateEndpoints(&realm_builder_exposed_dir_);
      if (client_end.is_error()) {
        return client_end.take_error();
      }

      zx::result result =
          outgoing_->AddDirectory(std::move(client_end.value()), kRealmBuilderExposedDir);
      if (result.is_error()) {
        FX_LOG_KV(ERROR, "Failed to add directory to outgoing directory",
                  FX_KV("directory", kRealmBuilderExposedDir));
        return result.take_error();
      }
    }

    // Hook up fuchsia.driver.test/Realm so we can proceed with the rest of initialization once
    // |Start| is invoked
    zx::result result = outgoing_->AddUnmanagedProtocol<fuchsia_driver_test::Realm>(
        bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    if (result.is_error()) {
      FX_LOG_KV(ERROR, "Failed to add protocol to outgoing directory",
                FX_KV("protocol", "fuchsia.driver.test/Realm"));
      return result.take_error();
    }

    return zx::ok();
  }

  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    // Non-hermetic users will end up calling start several times as the component test framework
    // invokes the binary multiple times, resulting in main running several times. We may be
    // ignoring real issues by ignoring the subsequent calls in the case that multiple parties
    // are invoking start unknowingly. Comparing the args may be a way to avoid that issue.
    // TODO(https://fxbug.dev/42073125): Remedy this situation
    if (is_started_) {
      completer.Reply(zx::ok());
      return;
    }
    is_started_ = true;

    if (request.args().board_name().has_value()) {
      boot_items_.SetBoardName(*request.args().board_name());
    }

    if (request.args().devicetree().has_value()) {
      boot_items_.SetDeviceTree(std::move(*request.args().devicetree()));
    }

    if (request.args().platform_vid().has_value()) {
      boot_items_.SetVid(std::move(*request.args().platform_vid()));
    }

    if (request.args().platform_pid().has_value()) {
      boot_items_.SetPid(std::move(*request.args().platform_pid()));
    }

    zx::result result = outgoing_->AddUnmanagedProtocol<fuchsia_boot::Items>(
        [this, tunnel_boot_items =
                   config_.tunnel_boot_items()](fidl::ServerEnd<fuchsia_boot::Items> server_end) {
          auto result = boot_items_.Serve(dispatcher_, std::move(server_end), tunnel_boot_items);
          if (result.is_error()) {
            FX_LOGS(ERROR) << "Failed to tunnel fuchsia_boot::Items" << result.status_string();
          }
        });

    result = outgoing_->AddUnmanagedProtocol<fuchsia_system_state::SystemStateTransition>(
        [this](fidl::ServerEnd<fuchsia_system_state::SystemStateTransition> server_end) {
          system_state_.Serve(dispatcher_, std::move(server_end));
        });

    if (result.is_error()) {
      completer.Reply(result.take_error());
      return;
    }

    result = outgoing_->AddUnmanagedProtocol<fuchsia_kernel::RootJob>(
        [this](fidl::ServerEnd<fuchsia_kernel::RootJob> server_end) {
          root_job_.Serve(dispatcher_, std::move(server_end));
        });
    if (result.is_error()) {
      completer.Reply(result.take_error());
      return;
    }

    fidl::ClientEnd<fuchsia_io::Directory> boot_dir;
    if (request.args().boot().has_value()) {
      boot_dir = fidl::ClientEnd<fuchsia_io::Directory>(std::move(*request.args().boot()));
    }

    fidl::ClientEnd<fuchsia_io::Directory> pkg_dir;
    if (request.args().pkg().has_value()) {
      pkg_dir = fidl::ClientEnd<fuchsia_io::Directory>(std::move(*request.args().pkg()));
    }

    // Look at the test's component package and subpackages.
    fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir;
    std::optional<fres::Context> test_resolution_context;
    if (request.args().test_component().has_value()) {
      test_resolution_context = request.args().test_component()->resolution_context();
      test_pkg_dir = std::move(*request.args().test_component()->package()->directory());
    }

    internal_server_.emplace(std::move(boot_dir), std::move(pkg_dir), std::move(test_pkg_dir),
                             test_resolution_context, request.args().boot_driver_components());

    result = outgoing_->AddUnmanagedProtocol<fuchsia_driver_test::Internal>(
        [this](fidl::ServerEnd<fuchsia_driver_test::Internal> server_end) {
          internal_server_->Serve(dispatcher_, std::move(server_end));
        });
    if (result.is_error()) {
      completer.Reply(result.take_error());
      return;
    }

    // Add additional routes if specified.
    std::unordered_map<fdt::Collection, std::vector<Ref>> kMap = {
        {
            fdt::Collection::kBootDrivers,
            {
                CollectionRef{"boot-drivers"},
            },
        },
        {
            fdt::Collection::kPackageDrivers,
            {
                CollectionRef{"base-drivers"},
                CollectionRef{"full-drivers"},
            },
        },
    };
    if (request.args().offers().has_value()) {
      for (const auto& offer : *request.args().offers()) {
        realm_builder_.AddRoute(Route{.capabilities = {Protocol{offer.protocol_name()}},
                                      .source = {ParentRef()},
                                      .targets = kMap[offer.collection()]});
      }
    }

    if (request.args().dtr_offers()) {
      for (const auto& offer_cap : *request.args().dtr_offers()) {
        std::optional<Capability> converted;
        switch (offer_cap.Which()) {
          case fuchsia_component_test::Capability::Tag::kProtocol: {
            const auto& offer_cap_proto = offer_cap.protocol().value();
            converted.emplace(Protocol{
                .name = offer_cap_proto.name().value(),
                .as = offer_cap_proto.as(),
                .type = offer_cap_proto.type().has_value()
                            ? std::make_optional(
                                  static_cast<DependencyType>(offer_cap_proto.type().value()))
                            : std::nullopt,
                .path = offer_cap_proto.path(),
            });
            break;
          }
          case fuchsia_component_test::Capability::Tag::kConfig: {
            const auto& offer_cap_config = offer_cap.config().value();
            converted.emplace(Config{
                .name = offer_cap_config.name().value(),
                .as = offer_cap_config.as(),
            });
            break;
          }
          case fuchsia_component_test::Capability::Tag::kDirectory:
          case fuchsia_component_test::Capability::Tag::kStorage:
          case fuchsia_component_test::Capability::Tag::kService:
          case fuchsia_component_test::Capability::Tag::kEventStream:
          case fuchsia_component_test::Capability::Tag::kDictionary:
          default:
            FX_LOG_KV(WARNING, "Skipping unsupported offer capability.",
                      FX_KV("type", static_cast<uint64_t>(offer_cap.Which())));
            break;
        }

        if (converted.has_value()) {
          realm_builder_.AddRoute(Route{
              .capabilities = {converted.value()},
              .source = {ParentRef()},
              .targets =
                  {
                      CollectionRef{"boot-drivers"},
                      CollectionRef{"base-drivers"},
                      CollectionRef{"full-drivers"},
                  },
          });
        }
      }
    }

    if (request.args().exposes().has_value()) {
      for (const auto& expose : *request.args().exposes()) {
        for (const auto& ref : kMap[expose.collection()]) {
          realm_builder_.AddRoute(Route{.capabilities = {Service{expose.service_name()}},
                                        .source = ref,
                                        .targets = {ParentRef()}});
        }
      }
    }

    if (request.args().dtr_exposes()) {
      for (const auto& expose_cap : *request.args().dtr_exposes()) {
        std::optional<Capability> converted;
        switch (expose_cap.Which()) {
          case fuchsia_component_test::Capability::Tag::kService: {
            const auto& expose_cap_service = expose_cap.service().value();
            converted.emplace(Service{
                .name = expose_cap_service.name().value(),
                .as = expose_cap_service.as(),
                .path = expose_cap_service.path(),
            });
            break;
          }
          case fuchsia_component_test::Capability::Tag::kProtocol:
          case fuchsia_component_test::Capability::Tag::kDirectory:
          case fuchsia_component_test::Capability::Tag::kStorage:
          case fuchsia_component_test::Capability::Tag::kEventStream:
          case fuchsia_component_test::Capability::Tag::kConfig:
          case fuchsia_component_test::Capability::Tag::kDictionary:
          default:
            FX_LOG_KV(WARNING, "Skipping unsupported expose capability.",
                      FX_KV("type", static_cast<uint64_t>(expose_cap.Which())));
            break;
        }

        if (converted.has_value()) {
          realm_builder_.AddRoute(Route{
              .capabilities = {converted.value()},
              .source =
                  {
                      CollectionRef{"boot-drivers"},
                  },
              .targets = {ParentRef()},
          });
          realm_builder_.AddRoute(Route{
              .capabilities = {converted.value()},
              .source =
                  {
                      CollectionRef{"base-drivers"},
                  },
              .targets = {ParentRef()},
          });
          realm_builder_.AddRoute(Route{
              .capabilities = {converted.value()},
              .source =
                  {
                      CollectionRef{"full-drivers"},
                  },
              .targets = {ParentRef()},
          });
        }
      }
    }

    // Set driver-index config based on request.
    const std::vector<std::string> kEmptyVec;
    std::vector<component_testing::ConfigCapability> configurations;
    configurations.push_back({
        .name = "fuchsia.driver.BootDrivers",
        .value = kEmptyVec,
    });
    configurations.push_back({
        .name = "fuchsia.driver.BaseDrivers",
        .value = kEmptyVec,
    });
    configurations.push_back({
        .name = "fuchsia.driver.BindEager",
        .value = request.args().driver_bind_eager().value_or(kEmptyVec),
    });
    configurations.push_back({
        .name = "fuchsia.driver.DisabledDrivers",
        .value = request.args().driver_disable().value_or(kEmptyVec),
    });
    configurations.push_back({
        .name = "fuchsia.driver.index.StopOnIdleTimeoutMillis",
        .value = ConfigValue::Int64(request.args().driver_index_stop_timeout_millis().value_or(-1)),
    });
    realm_builder_.AddConfiguration(std::move(configurations));
    realm_builder_.AddRoute({
        .capabilities =
            {
                component_testing::Config{.name = "fuchsia.driver.BootDrivers"},
                component_testing::Config{.name = "fuchsia.driver.BaseDrivers"},
                component_testing::Config{.name = "fuchsia.driver.BindEager"},
                component_testing::Config{.name = "fuchsia.driver.DisabledDrivers"},
                component_testing::Config{.name = "fuchsia.driver.index.StopOnIdleTimeoutMillis"},
            },
        .source = component_testing::SelfRef{},
        .targets = {component_testing::ChildRef{"driver-index"}},
    });

    // Set driver_manager config based on request.
    configurations = std::vector<component_testing::ConfigCapability>();
    const std::string default_root = "fuchsia-boot:///dtr#meta/test-parent-sys.cm";
    configurations.push_back({
        .name = "fuchsia.driver.manager.RootDriver",
        .value = request.args().root_driver().value_or(default_root),
    });
    realm_builder_.AddConfiguration(std::move(configurations));
    realm_builder_.AddRoute({
        .capabilities =
            {
                component_testing::Config{.name = "fuchsia.driver.manager.RootDriver"},
            },
        .source = component_testing::SelfRef{},
        .targets = {component_testing::ChildRef{"driver_manager"}},
    });

    configurations = std::vector<component_testing::ConfigCapability>();
    configurations.push_back({
        .name = "fuchsia.power.WaitForSuspendingToken",
        .value = ConfigValue::Bool(false),
    });
    realm_builder_.AddConfiguration(std::move(configurations));
    realm_builder_.AddRoute(
        {.capabilities =
             {
                 component_testing::Config{.name = "fuchsia.power.WaitForSuspendingToken"},
             },
         .source = component_testing::SelfRef{},
         .targets = {component_testing::ChildRef{.name = "driver_manager"}}});

    // Set platform bus config based on request.
    configurations = std::vector<component_testing::ConfigCapability>();
    component_testing::Ref source;
    if (request.args().software_devices()) {
      source = component_testing::SelfRef();
      std::vector<std::string> device_names;
      std::vector<uint32_t> device_ids;
      for (const auto& device : *request.args().software_devices()) {
        device_names.push_back(device.device_name());
        device_ids.push_back(device.device_id());
      }
      configurations.push_back({
          .name = "fuchsia.platform.bus.SoftwareDeviceNames",
          .value = ConfigValue(device_names),
      });
      configurations.push_back({
          .name = "fuchsia.platform.bus.SoftwareDeviceIds",
          .value = ConfigValue(device_ids),
      });
    } else {
      source = component_testing::VoidRef();
    }
    realm_builder_.AddConfiguration(std::move(configurations));
    realm_builder_.AddRoute({
        .capabilities =
            {
                component_testing::Config{.name = "fuchsia.platform.bus.SoftwareDeviceNames"},
                component_testing::Config{.name = "fuchsia.platform.bus.SoftwareDeviceIds"},
            },
        .source = source,
        .targets = {component_testing::CollectionRef{"boot-drivers"}},
    });

    realm_ = realm_builder_.SetRealmName("0").Build(dispatcher_);

    // Forward exposes.
    auto exposed_dir = realm_->component().exposed().unowned_channel()->get();
    if (request.args().exposes().has_value()) {
      for (const auto& expose : *request.args().exposes()) {
        auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
        if (endpoints.is_error()) {
          completer.Reply(endpoints.take_error());
          return;
        }
        auto flags =
            static_cast<uint64_t>(fio::kPermReadable | fio::wire::Flags::kProtocolDirectory);
        zx_status_t status = fdio_open3_at(exposed_dir, expose.service_name().c_str(), flags,
                                           endpoints->server.TakeChannel().release());
        if (status != ZX_OK) {
          completer.Reply(zx::error(status));
          return;
        }
        auto result =
            outgoing_->AddDirectoryAt(std::move(endpoints->client), "svc", expose.service_name());
        if (result.is_error()) {
          completer.Reply(result.take_error());
          return;
        }
      }
    }

    // Connect realm_builder_exposed_dir.
    if (zx_status_t status =
            fdio_open3_at(exposed_dir, ".",
                          static_cast<uint64_t>(fio::kPermReadable | fio::Flags::kPermInheritWrite |
                                                fio::Flags::kPermInheritExecute),
                          realm_builder_exposed_dir_.TakeChannel().release());
        status != ZX_OK) {
      completer.Reply(zx::error(status));
      return;
    }

    // Connect to the driver manager and wait for bootup to complete before returning.
    auto manager = component::ConnectAt<fuchsia_driver_development::Manager>(
        fidl::UnownedClientEnd<fuchsia_io::Directory>(
            realm_->component().exposed().unowned_channel()));
    if (manager.is_error()) {
      completer.Reply(manager.take_error());
      return;
    }

    development_manager_client_.Bind(*std::move(manager), dispatcher_);

    development_manager_client_->WaitForBootup().Then(
        [completer = completer.ToAsync()](
            fidl::Result<fuchsia_driver_development::Manager::WaitForBootup>& wait_result) mutable {
          if (wait_result.is_error()) {
            completer.Reply(zx::error(wait_result.error_value().status()));
            return;
          }

          completer.Reply(zx::ok());
        });
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_test::Realm> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    std::string method_type;
    switch (metadata.unknown_method_type) {
      case fidl::UnknownMethodType::kOneWay:
        method_type = "one-way";
        break;
      case fidl::UnknownMethodType::kTwoWay:
        method_type = "two-way";
        break;
    };

    FX_LOG_KV(WARNING, "DriverDevelopmentService received unknown method.",
              FX_KV("Direction", method_type.c_str()), FX_KV("Ordinal", metadata.method_ordinal));
  }

 private:
  bool is_started_ = false;
  component::OutgoingDirectory* outgoing_;
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_driver_test::Realm> bindings_;
  fidl::Client<fuchsia_driver_development::Manager> development_manager_client_;

  struct Directory {
    const char* name;
    uint32_t flags;
    fidl::ServerEnd<fuchsia_io::Directory> server_end;
  };

  component_testing::RealmBuilder realm_builder_ =
      component_testing::RealmBuilder::CreateFromRelativeUrl("#meta/test_realm.cm");
  std::optional<component_testing::RealmRoot> realm_;
  driver_test_realm_config::Config config_;
  fidl::ServerEnd<fuchsia_io::Directory> realm_builder_exposed_dir_;
  driver_test_realm::BootItems boot_items_;
  driver_test_realm::SystemStateTransition system_state_;
  driver_test_realm::RootJob root_job_;
  std::optional<driver_test_realm::InternalServer> internal_server_;
};

}  // namespace

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithDispatcher(loop.dispatcher()).BuildAndInitialize();
  component::OutgoingDirectory outgoing(loop.dispatcher());

  auto config = driver_test_realm_config::Config::TakeFromStartupHandle();

  DriverTestRealm dtr(&outgoing, loop.dispatcher(), config);
  {
    zx::result result = dtr.Init();
    ZX_ASSERT(result.is_ok());
  }

  {
    zx::result result = outgoing.ServeFromStartupInfo();
    ZX_ASSERT(result.is_ok());
  }

  loop.Run();
  return 0;
}
