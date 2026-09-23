// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/driver_test_realm/src/internal_server.h>
#include <lib/fdio/directory.h>
#include <lib/sync/cpp/completion.h>

namespace driver_test_realm {

OptionsBuilder& OptionsBuilder::using_subpackage(bool using_subpackage) {
  options_.using_subpackage = using_subpackage;
  return *this;
}

OptionsBuilder& OptionsBuilder::driver_offers(
    component_testing::Ref provider,
    const std::vector<fuchsia_component_test::Capability>& offers) {
  options_.driver_offers = std::make_tuple(provider, offers);
  return *this;
}

OptionsBuilder& OptionsBuilder::driver_exposes(
    const std::vector<fuchsia_component_test::Capability>& exposes) {
  options_.driver_exposes = exposes;
  return *this;
}

OptionsBuilder& OptionsBuilder::add_extra_realm_capability(
    fuchsia_component_test::Capability capability, component_testing::Ref provider) {
  options_.extra_realm_capabilities.emplace_back(capability, provider);
  return *this;
}

using component_testing::Capability;
using component_testing::ChildRef;
using component_testing::CollectionRef;
using component_testing::Config;
using component_testing::ConfigCapability;
using component_testing::ConfigValue;
using component_testing::Dictionary;
using component_testing::Directory;
using component_testing::LocalComponentImpl;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::RealmBuilder;
using component_testing::RealmRoot;
using component_testing::Ref;
using component_testing::Resolver;
using component_testing::Route;
using component_testing::Runner;
using component_testing::SelfRef;
using component_testing::Service;
using component_testing::Storage;
using component_testing::VoidRef;

namespace {

Capability ConvertCapability(const fuchsia_component_test::Capability& capability) {
  std::optional<Capability> converted;
  switch (capability.Which()) {
    case fuchsia_component_test::Capability::Tag::kProtocol: {
      std::optional<fuchsia::component::decl::DependencyType> type;
      if (capability.protocol()->type().has_value()) {
        type = static_cast<fuchsia::component::decl::DependencyType>(
            capability.protocol()->type().value());
      }
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.protocol()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.protocol()->availability().value());
      }
      converted = Protocol{
          .name = capability.protocol()->name().value(),
          .as = capability.protocol()->as(),
          .type = type,
          .path = capability.protocol()->path(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kDirectory: {
      std::optional<fuchsia::component::decl::DependencyType> type;
      if (capability.directory()->type().has_value()) {
        type = static_cast<fuchsia::component::decl::DependencyType>(
            capability.directory()->type().value());
      }
      std::optional<fuchsia::io::Operations> rights;
      if (capability.directory()->rights().has_value()) {
        rights = static_cast<fuchsia::io::Operations>(
            static_cast<uint64_t>(capability.directory()->rights().value()));
      }
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.directory()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.directory()->availability().value());
      }

      converted = Directory{
          .name = capability.directory()->name().value(),
          .as = capability.directory()->as(),
          .type = type,
          .subdir = capability.directory()->subdir(),
          .rights = rights,
          .path = capability.directory()->path(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kStorage: {
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.storage()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.storage()->availability().value());
      }
      converted = Storage{
          .name = capability.storage()->name().value(),
          .as = capability.storage()->as(),
          .path = capability.storage()->path(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kService: {
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.service()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.service()->availability().value());
      }
      converted = Service{
          .name = capability.service()->name().value(),
          .as = capability.service()->as(),
          .path = capability.service()->path(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kEventStream: {
      ZX_ASSERT_MSG(false, "EventStream capability not supported here.");
      break;
    }
    case fuchsia_component_test::Capability::Tag::kConfig: {
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.config()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.config()->availability().value());
      }
      converted = Config{
          .name = capability.config()->name().value(),
          .as = capability.config()->as(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kDictionary: {
      std::optional<fuchsia::component::decl::Availability> availability;
      if (capability.dictionary()->availability().has_value()) {
        availability = static_cast<fuchsia::component::decl::Availability>(
            capability.dictionary()->availability().value());
      }
      converted = Dictionary{
          .name = capability.dictionary()->name().value(),
          .as = capability.dictionary()->as(),
          .availability = availability,
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kResolver: {
      converted = Resolver{
          .name = capability.resolver()->name().value(),
          .as = capability.resolver()->as(),
          .path = capability.resolver()->path(),
      };
      break;
    }
    case fuchsia_component_test::Capability::Tag::kRunner: {
      converted = Runner{
          .name = capability.runner()->name().value(),
          .as = capability.runner()->as(),
          .path = capability.runner()->path(),
      };
      break;
    }
    default:
      break;
  }

  return converted.value();
}

bool CapabilitiesEqualName(const Capability& lhs, const Capability& rhs) {
  if (lhs.index() != rhs.index()) {
    return false;
  }
  const Protocol* lhs_protocol = std::get_if<Protocol>(&lhs);
  const Protocol* rhs_protocol = std::get_if<Protocol>(&rhs);
  const Directory* lhs_directory = std::get_if<Directory>(&lhs);
  const Directory* rhs_directory = std::get_if<Directory>(&rhs);
  const Storage* lhs_storage = std::get_if<Storage>(&lhs);
  const Storage* rhs_storage = std::get_if<Storage>(&rhs);
  const Service* lhs_service = std::get_if<Service>(&lhs);
  const Service* rhs_service = std::get_if<Service>(&rhs);
  const Config* lhs_config = std::get_if<Config>(&lhs);
  const Config* rhs_config = std::get_if<Config>(&rhs);
  const Dictionary* lhs_dictionary = std::get_if<Dictionary>(&lhs);
  const Dictionary* rhs_dictionary = std::get_if<Dictionary>(&rhs);
  const Resolver* lhs_resolver = std::get_if<Resolver>(&lhs);
  const Resolver* rhs_resolver = std::get_if<Resolver>(&rhs);
  const Runner* lhs_runner = std::get_if<Runner>(&lhs);
  const Runner* rhs_runner = std::get_if<Runner>(&rhs);

  if (lhs_protocol && rhs_protocol) {
    return lhs_protocol->name == rhs_protocol->name;
  }
  if (lhs_directory && rhs_directory) {
    return lhs_directory->name == rhs_directory->name;
  }
  if (lhs_storage && rhs_storage) {
    return lhs_storage->name == rhs_storage->name;
  }
  if (lhs_service && rhs_service) {
    return lhs_service->name == rhs_service->name;
  }
  if (lhs_config && rhs_config) {
    return lhs_config->name == rhs_config->name;
  }
  if (lhs_dictionary && rhs_dictionary) {
    return lhs_dictionary->name == rhs_dictionary->name;
  }
  if (lhs_resolver && rhs_resolver) {
    return lhs_resolver->name == rhs_resolver->name;
  }
  if (lhs_runner && rhs_runner) {
    return lhs_runner->name == rhs_runner->name;
  }

  return false;
}

class InternalServerComponent final
    : public LocalComponentImpl,
      public fidl::WireServer<fuchsia_driver_test::ResourceProvider> {
 public:
  InternalServerComponent(async_dispatcher_t* dispatcher,
                          std::shared_ptr<driver_test_realm::InternalServer> server,
                          std::shared_ptr<std::optional<zx::vmo>> devicetree)
      : dispatcher_(dispatcher), server_(std::move(server)), devicetree_(std::move(devicetree)) {}
  void OnStart() override {
    outgoing()->AddProtocol<fuchsia_driver_test::Internal>(
        [this](fidl::ServerEnd<fuchsia_driver_test::Internal> server_end) {
          server_->Serve(dispatcher_, std::move(server_end));
        });

    outgoing()->AddProtocol<fuchsia_driver_test::ResourceProvider>(
        resource_provider_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  }

  void GetDeviceTree(GetDeviceTreeCompleter::Sync& completer) override {
    if ((*devicetree_).has_value()) {
      zx::vmo out;
      zx_status_t status = (*devicetree_)->duplicate(ZX_RIGHT_SAME_RIGHTS, &out);
      if (status == ZX_OK) {
        completer.ReplySuccess(std::move(out));
        return;
      }
    }
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }

 private:
  async_dispatcher_t* dispatcher_;
  std::shared_ptr<driver_test_realm::InternalServer> server_;
  std::shared_ptr<std::optional<zx::vmo>> devicetree_;
  fidl::ServerBindingGroup<fuchsia_driver_test::ResourceProvider> resource_provider_bindings_;
};

}  // namespace

void Setup(RealmBuilder& realm_builder, async_dispatcher_t* dispatcher, Options options,
           fuchsia_driver_test::RealmArgs args) {
  auto manifest_provider = component::Connect<fuchsia_driver_test::ManifestProvider>();
  ZX_ASSERT(manifest_provider.is_ok());

  auto manifest_result = fidl::Call(*manifest_provider)
                             ->GetManifest({{
                                 .using_subpackage = options.using_subpackage,
                             }});
  ZX_ASSERT(manifest_result.is_ok());

  std::vector<uint8_t> manifest;
  uint8_t buffer[fuchsia_io::kMaxBuf];
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = fuchsia_io::kMaxBuf,
  };
  size_t read = 1;
  while (read != 0) {
    zx_status_t read_result = manifest_result->manifest().readv(0, &vec, 1, &read);
    ZX_ASSERT_MSG(read_result == ZX_OK, "Failed to read manifest stream.");
    manifest.insert(manifest.end(), buffer, buffer + read);
  }

  auto component = fidl::Unpersist<fuchsia_component_decl::Component>(manifest);
  ZX_ASSERT(component.is_ok());
  fuchsia::component::decl::Component hlcpp_component =
      fidl::NaturalToHLCPP(std::move(component.value()));

  // Keep the rust and c++ realm_builder setups in sync.
  // LINT.IfChange
  auto realm = realm_builder.AddChildRealmFromDecl("driver_test_realm", hlcpp_component);
  Ref dtr_realm_ref = ChildRef{"driver_test_realm"};

  // From the test root into the dtr.
  realm_builder.AddRoute(Route{
      .capabilities =
          std::vector<Capability>{
              Protocol{.name = "fuchsia.diagnostics.ArchiveAccessor"},
          },
      .source = {ParentRef{}},
      .targets = {dtr_realm_ref},
  });

  Ref driver_manager = ChildRef{"driver_manager"};
  Ref fake_resolver = ChildRef{"fake_resolver"};
  Ref driver_index = ChildRef{"driver_index"};
  Ref dtr_support = ChildRef{"dtr_support"};

  Ref boot_drivers = CollectionRef{"boot-drivers"};
  Ref base_drivers = CollectionRef{"base-drivers"};
  Ref full_drivers = CollectionRef{"full-drivers"};

  // Get the test component information.
  fuchsia_component_resolution::Component test_component;
  if (args.test_component().has_value()) {
    test_component = std::move(args.test_component().value());
  } else {
    auto component_realm = component::Connect<fuchsia_component::Realm>();
    fidl::SyncClient<fuchsia_component::Realm> realm_client(std::move(component_realm.value()));
    auto resolved_info = realm_client->GetResolvedInfo();
    ZX_ASSERT(resolved_info.is_ok());

    test_component = std::move(resolved_info.value().resolved_info());
  }

  // Route the resolvers from the test root.
  realm_builder.AddRoute(Route{
      .capabilities =
          std::vector<Capability>{
              Protocol{
                  .name = "fuchsia.component.resolution.Resolver-hermetic",
                  .availability = fuchsia::component::decl::Availability::OPTIONAL,
              },
              Protocol{
                  .name = "fuchsia.pkg.PackageResolver-hermetic",
                  .availability = fuchsia::component::decl::Availability::OPTIONAL,
              },
          },
      .source = {ParentRef{}},
      .targets = {dtr_realm_ref},
  });

  // Setup the local Internal protocol server child.
  std::optional<std::vector<std::string>> boot_driver_components =
      std::move(args.boot_driver_components());

  std::shared_ptr<driver_test_realm::InternalServer> internal_server =
      std::make_shared<driver_test_realm::InternalServer>(
          std::move(args.boot()).value_or(fidl::ClientEnd<fuchsia_io::Directory>{}),
          fidl::ClientEnd<fuchsia_io::Directory>{},
          std::move(test_component.package()->directory()).value(),
          std::move(test_component.resolution_context()).value(),
          std::move(boot_driver_components));

  std::shared_ptr<std::optional<zx::vmo>> devicetree =
      std::make_shared<std::optional<zx::vmo>>(std::move(args.devicetree()));
  realm.AddLocalChild("driver_test_internal",
                      [dispatcher, internal_server = std::move(internal_server), devicetree]() {
                        auto impl = std::make_unique<InternalServerComponent>(
                            dispatcher, internal_server, devicetree);
                        return impl;
                      });

  Ref driver_test_internal = ChildRef{"driver_test_internal"};

  // These are capabilities that are routed from void by default but can be provided manually
  // from the user through extra_realm_capabilities.
  bool tunnel_boot_items = false;
  std::vector<Capability> voided_offers = {
      Protocol{
          .name = "fuchsia.tracing.provider.Registry",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.boot.WriteOnlyLog",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.scheduler.RoleManager",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.boot.Items",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.kernel.IommuResource",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.diagnostics.LogFlusher",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.kernel.MexecResource",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
      Protocol{
          .name = "fuchsia.kernel.PowerResource",
          .availability = fuchsia::component::decl::Availability::OPTIONAL,
      },
  };

  for (const auto& [capability, from] : options.extra_realm_capabilities) {
    Capability converted = ConvertCapability(capability);

    // Remove the default voiding for any user provided capabilities.
    for (auto it = voided_offers.begin(); it != voided_offers.end(); ++it) {
      if (CapabilitiesEqualName(converted, *it)) {
        voided_offers.erase(it);
        break;
      }
    }

    if (CapabilitiesEqualName(converted, Protocol{.name = "fuchsia.boot.Items"})) {
      tunnel_boot_items = true;
    }

    realm_builder.AddRoute(Route{
        .capabilities = {converted},
        .source = {from},
        .targets = {dtr_realm_ref},
    });
  }

  // Set the default void route for remaining voided offers.
  for (const auto& voided_offer : voided_offers) {
    realm_builder.AddRoute(Route{
        .capabilities = {voided_offer},
        .source = {VoidRef{}},
        .targets = {dtr_realm_ref},
    });
  }

  // Provide offers from the driver_offers, if the test provides one, to the driver
  // collections.
  if (args.dtr_offers()) {
    ZX_ASSERT_MSG(false, "Please use |Options::driver_offers| instead of dtr_offers.");
  }
  if (options.driver_offers) {
    auto [provider, offers] = options.driver_offers.value();
    for (const auto& offer : offers) {
      realm_builder.AddRoute(Route{
          .capabilities = {ConvertCapability(offer)},
          .source = {provider},
          .targets = {dtr_realm_ref},
      });
      realm.AddRoute(Route{
          .capabilities = {ConvertCapability(offer)},
          .source = {ParentRef{}},
          .targets =
              {
                  boot_drivers,
                  base_drivers,
                  full_drivers,
              },
      });
    }
  }

  // Provide exposes from the driver collections to the test.
  if (args.dtr_exposes()) {
    ZX_ASSERT_MSG(false, "Please use |Options::driver_exposes| instead of dtr_exposes.");
  }
  if (options.driver_exposes) {
    for (const auto& expose : options.driver_exposes.value()) {
      realm.AddRoute(Route{
          .capabilities = {ConvertCapability(expose)},
          .source = {boot_drivers},
          .targets = {ParentRef{}},
      });
      realm.AddRoute(Route{
          .capabilities = {ConvertCapability(expose)},
          .source = {base_drivers},
          .targets = {ParentRef{}},
      });
      realm.AddRoute(Route{
          .capabilities = {ConvertCapability(expose)},
          .source = {full_drivers},
          .targets = {ParentRef{}},
      });

      realm_builder.AddRoute(Route{
          .capabilities = {ConvertCapability(expose)},
          .source = {dtr_realm_ref},
          .targets = {ParentRef{}},
      });
    }
  }

  // Setup the driver test resource provider.
  realm.AddRoute(Route{
      .capabilities = {Protocol{.name = "fuchsia.driver.test.ResourceProvider"}},
      .source = {driver_test_internal},
      .targets = {dtr_support},
  });

  // Setup various basic config capabilities.
  {
    std::string vid;
    if (args.platform_vid()) {
      vid = std::format("{}", args.platform_vid().value());
    }

    std::string pid;
    if (args.platform_pid()) {
      pid = std::format("{}", args.platform_pid().value());
    }

    std::vector<ConfigCapability> configs;
    configs.push_back({
        .name = "fuchsia.driver.testrealm.TunnelBootItems",
        .value = ConfigValue::Bool(tunnel_boot_items),
    });

    configs.push_back({
        .name = "fuchsia.driver.testrealm.BoardName",
        .value = ConfigValue(args.board_name().value_or("")),
    });
    configs.push_back({
        .name = "fuchsia.driver.testrealm.PlatformVid",
        .value = ConfigValue(vid),
    });
    configs.push_back({
        .name = "fuchsia.driver.testrealm.PlatformPid",
        .value = ConfigValue(pid),
    });
    configs.push_back({
        .name = "fuchsia.driver.BindEager",
        .value = ConfigValue(args.driver_bind_eager().value_or(std::vector<std::string>{})),
    });
    configs.push_back({
        .name = "fuchsia.driver.DisabledDrivers",
        .value = ConfigValue(args.driver_disable().value_or(std::vector<std::string>{})),
    });
    configs.push_back({
        .name = "fuchsia.driver.index.StopOnIdleTimeoutMillis",
        .value = ConfigValue::Int64(args.driver_index_stop_timeout_millis().value_or(-1)),
    });
    configs.push_back({
        .name = "fuchsia.driver.manager.RootDriver",
        .value =
            ConfigValue(args.root_driver().value_or("fuchsia-boot:///dtr#meta/test-parent-sys.cm")),
    });
    configs.push_back({
        .name = "fuchsia.power.WaitForSuspendingToken",
        .value = ConfigValue::Bool(false),
    });
    realm.AddConfiguration(std::move(configs));
  }

  // Setup software device config capabilities.
  Ref software_dev_source = VoidRef{};
  {
    if (args.software_devices().has_value()) {
      std::vector<ConfigCapability> configs;
      software_dev_source = SelfRef{};
      std::vector<std::string> device_names;
      std::vector<uint32_t> device_ids;
      for (const auto& device : args.software_devices().value()) {
        device_names.push_back(device.device_name());
        device_ids.push_back(device.device_id());
      }
      configs.push_back({
          .name = "fuchsia.platform.bus.SoftwareDeviceNames",
          .value = ConfigValue(device_names),
      });
      configs.push_back({
          .name = "fuchsia.platform.bus.SoftwareDeviceIds",
          .value = ConfigValue(device_ids),
      });
      realm.AddConfiguration(std::move(configs));
    }
  }

  // Config routes.
  {
    realm.AddRoute(Route{
        .capabilities =
            {
                Config{.name = "fuchsia.driver.BindEager"},
                Config{.name = "fuchsia.driver.DisabledDrivers"},
                Config{.name = "fuchsia.driver.index.StopOnIdleTimeoutMillis"},
            },
        .source = SelfRef{},
        .targets = {driver_index},
    });

    realm.AddRoute(Route{
        .capabilities = {Config{.name = "fuchsia.driver.manager.RootDriver"}},
        .source = SelfRef{},
        .targets = {driver_manager},
    });

    realm.AddRoute(Route{
        .capabilities =
            {
                Config{
                    .name = "fuchsia.driver.manager.SetRootDriverHostCritical",
                    .availability = fuchsia::component::decl::Availability::OPTIONAL,
                },
                Config{
                    .name = "fuchsia.driver.manager.SuspendTimeoutFallback",
                    .availability = fuchsia::component::decl::Availability::OPTIONAL,
                },
            },
        .source = VoidRef{},
        .targets = {driver_manager},
    });

    realm.AddRoute(Route{
        .capabilities =
            {
                Config{.name = "fuchsia.driver.testrealm.TunnelBootItems"},
                Config{.name = "fuchsia.driver.testrealm.BoardName"},
                Config{.name = "fuchsia.driver.testrealm.PlatformVid"},
                Config{.name = "fuchsia.driver.testrealm.PlatformPid"},
            },
        .source = SelfRef{},
        .targets = {dtr_support},
    });

    realm.AddRoute(Route{
        .capabilities =
            {
                Config{
                    .name = "fuchsia.platform.bus.SoftwareDeviceNames",
                    .availability = fuchsia::component::decl::Availability::OPTIONAL,
                },
                Config{
                    .name = "fuchsia.platform.bus.SoftwareDeviceIds",
                    .availability = fuchsia::component::decl::Availability::OPTIONAL,
                },
            },
        .source = software_dev_source,
        .targets = {boot_drivers},
    });

    realm.AddRoute(Route{.capabilities = {Config{.name = "fuchsia.power.WaitForSuspendingToken"}},
                         .source = {SelfRef{}},
                         .targets = {driver_manager}});
  }

  // Dynamic routes to the driver framework children.
  {
    realm.AddRoute(Route{
        .capabilities = {Protocol{.name = "fuchsia.driver.test.Internal"}},
        .source = {driver_test_internal},
        .targets = {fake_resolver},
    });
  }

  // Routes from the driver framework children out to the test.
  {
    // TODO(https://fxbug.dev/377735979): Remove dev-topological when no longer using topological
    // in driver test realm tests.
    realm_builder.AddRoute(Route{
        .capabilities =
            {
                Directory{.name = "dev-class"},
                Directory{.name = "dev-topological"},
                Protocol{.name = "fuchsia.driver.registrar.DriverRegistrar"},
                Protocol{.name = "fuchsia.driver.development.Manager"},
                Protocol{.name = "fuchsia.driver.framework.CompositeNodeManager"},
                Protocol{.name = "fuchsia.driver.framework.NodeManager"},
                Protocol{.name = "fuchsia.system.state.Administrator"},
            },
        .source = {dtr_realm_ref},
        .targets = {ParentRef{}},
    });
  }
  // LINT.ThenChange(/sdk/lib/driver_test_realm/realm_builder/rust/src/builder.rs)
}

zx::result<> WaitForBootup(RealmRoot& realm_root) {
  // Connect to the driver manager and wait for boot up to complete before returning.
  auto manager = component::ConnectAt<fuchsia_driver_development::Manager>(
      fidl::UnownedClientEnd<fuchsia_io::Directory>(
          realm_root.component().exposed().unowned_channel()));

  if (manager.is_error()) {
    return manager.take_error();
  }

  fidl::SyncClient<fuchsia_driver_development::Manager> development_manager_client(
      *std::move(manager));

  fidl::Result<fuchsia_driver_development::Manager::WaitForBootup> result =
      development_manager_client->WaitForBootup();
  if (result.is_error()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

zx::result<fuchsia_driver_development::NodeInfo> WaitForNode(
    component_testing::RealmRoot& realm_root, std::string_view moniker) {
  // Connect to the driver manager and wait for the node with the given moniker to show.
  auto manager = component::ConnectAt<fuchsia_driver_development::Manager>(
      fidl::UnownedClientEnd<fuchsia_io::Directory>(
          realm_root.component().exposed().unowned_channel()));

  if (manager.is_error()) {
    return manager.take_error();
  }

  fidl::SyncClient<fuchsia_driver_development::Manager> development_manager_client(
      *std::move(manager));

  while (true) {
    auto [info_client, info_server] =
        fidl::Endpoints<fuchsia_driver_development::NodeInfoIterator>::Create();
    auto result = development_manager_client->GetNodeInfo(
        fidl::Request<fuchsia_driver_development::Manager::GetNodeInfo>{{
            .node_filter = {std::string(moniker)},
            .iterator = std::move(info_server),
            .exact_match = true,
        }});
    if (result.is_error()) {
      return zx::error(result.error_value().status());
    }
    auto info_next = fidl::Call(info_client)->GetNext();
    if (info_next.is_error()) {
      return zx::error(info_next.error_value().status());
    }

    if (info_next->nodes().empty()) {
      continue;
    }

    for (auto& node : info_next->nodes()) {
      if (node.moniker() == moniker) {
        return zx::ok(std::move(node));
      }
    }
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

void ShutdownRealm(component_testing::RealmRoot& realm_root) {
  libsync::Completion completion;
  realm_root.Teardown(
      [&completion](fit::result<fuchsia::component::Error>) { completion.Signal(); });
  completion.Wait();
}

}  // namespace driver_test_realm
