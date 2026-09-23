// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/testing/integration/hermetic_audio_realm.h"

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.inspect/cpp/fidl.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/synchronous_interface_ptr.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/remote_dir.h>
#include <lib/vfs/cpp/service.h>
#include <zircon/status.h>

#include <utility>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

namespace media::audio::test {

namespace {

class InspectSinkMock : public component_testing::LocalComponentImpl,
                        public fidl::WireServer<fuchsia_inspect::InspectSink> {
 public:
  static const inline std::string kName = "inspect_sink_mock";

  InspectSinkMock(async_dispatcher_t* dispatcher,
                  std::shared_ptr<std::optional<inspect::testing::TreeClient>> tree)
      : dispatcher_{dispatcher}, tree_{std::move(tree)} {}

  void OnStart() override {
    auto provider_svc = std::make_unique<vfs::Service>([this](zx::channel request,
                                                              async_dispatcher_t* dispatcher) {
      fidl::ServerEnd<fuchsia_inspect::InspectSink> server_end(std::move(request));
      bindings_.AddBinding(dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    });

    FX_CHECK(outgoing()->AddPublicService(
                 std::move(provider_svc),
                 fidl::DiscoverableProtocolName<fuchsia_inspect::InspectSink>) == ZX_OK);
  }

  void Publish(PublishRequestView request, PublishCompleter::Sync& completer) override {
    ZX_ASSERT(request->has_tree());
    *tree_ = {inspect::testing::TreeClient(std::move(request->tree()), dispatcher_)};
  }

  void FetchEscrow(FetchEscrowRequestView request, FetchEscrowCompleter::Sync& completer) override {
    FX_CHECK(false) << "Unexpected call to InspectSink/Escrow";
  }

  void Escrow(EscrowRequestView request, EscrowCompleter::Sync& completer) override {
    FX_CHECK(false) << "Unexpected call to InspectSink/FetchEscrow";
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_inspect::InspectSink> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_CHECK(false) << "Unexpected unknown method call on InspectSink";
  }

 private:
  async_dispatcher_t* dispatcher_ = nullptr;
  std::shared_ptr<std::optional<inspect::testing::TreeClient>> tree_;
  fidl::ServerBindingGroup<fuchsia_inspect::InspectSink> bindings_;
};

void ConnectToVirtualAudio(component_testing::RealmRoot& root,
                           fidl::SynchronousInterfacePtr<fuchsia::virtualaudio::Control>& out) {
  // Connect to dev.
  fidl::InterfaceHandle<fuchsia::io::Directory> dev;
  ASSERT_EQ(root.component().exposed()->Open("dev-topological", fuchsia::io::PERM_READABLE, {},
                                             dev.NewRequest().TakeChannel()),
            ZX_OK);
  fbl::unique_fd dev_fd;
  ASSERT_EQ(fdio_fd_create(dev.TakeChannel().release(), dev_fd.reset_and_get_address()), ZX_OK);

  // This file hosts a fuchsia.virtualaudio.Control channel.
  //
  // Wait for the driver to load.
  zx::result channel = device_watcher::RecursiveWaitForFile(
      dev_fd.get(), fuchsia::virtualaudio::LEGACY_CONTROL_NODE_NAME);
  ASSERT_EQ(channel.status_value(), ZX_OK);

  // Turn the connection into FIDL.
  out.Bind(std::move(channel.value()));
}

// Implements a simple component that serves fuchsia.audio.effects.ProcessorCreator
// using a TestEffectsV2.
class LocalProcessorCreator : public component_testing::LocalComponentImpl {
 public:
  explicit LocalProcessorCreator(std::vector<TestEffectsV2::Effect> effects)
      : effects_(std::move(effects)) {}

  void OnStart() override {
    ASSERT_EQ(ZX_OK,
              outgoing()->AddPublicService(
                  std::make_unique<vfs::Service>([this](zx::channel channel,
                                                        async_dispatcher_t* dispatcher) {
                    if (!server_) {
                      server_ = std::make_unique<TestEffectsV2>(dispatcher);
                      for (auto& effect : effects_) {
                        server_->AddEffect(std::move(effect));
                      }
                    }
                    server_->HandleRequest(fidl::ServerEnd<fuchsia_audio_effects::ProcessorCreator>(
                        std::move(channel)));
                  }),
                  "fuchsia.audio.effects.ProcessorCreator"));
  }

 private:
  std::vector<TestEffectsV2::Effect> effects_;
  std::unique_ptr<TestEffectsV2> server_;
};

// Implements a simple component that exports the given local directory as a capability named "dir".
class LocalDirectoryExporter : public component_testing::LocalComponentImpl {
 public:
  static inline const char kCapability[] = "exported-dir";
  static inline const char kPath[] = "/exported-dir";

  explicit LocalDirectoryExporter(const std::string& local_dir_name) {
    // Open a handle to the directory.
    zx::channel local, remote;
    auto status = zx::channel::create(0, &local, &remote);
    FX_CHECK(status == ZX_OK) << status;
    status = fdio_open3(local_dir_name.c_str(),
                        static_cast<uint64_t>(fuchsia_io::wire::kPermReadable |
                                              fuchsia_io::wire::Flags::kProtocolDirectory),
                        remote.release());
    FX_CHECK(status == ZX_OK) << status;
    local_dir_ = std::move(local);
  }

  void OnStart() override {
    ASSERT_EQ(ZX_OK, outgoing()->root_dir()->AddEntry(
                         kCapability, std::make_unique<vfs::RemoteDir>(std::move(local_dir_))));
  }

 private:
  zx::channel local_dir_;
};

}  // namespace

// Cannot define these until LocalProcessorCreator is defined.
HermeticAudioRealm::HermeticAudioRealm(CtorArgs&& args)
    : root_(std::move(args.root)),
      local_components_(std::move(args.local_components)),
      inspect_tree_(std::move(args.inspect_tree)) {}
HermeticAudioRealm::~HermeticAudioRealm() = default;

// This returns `void` so we can ASSERT from within Create.
void HermeticAudioRealm::Create(Options options, async_dispatcher* dispatcher,
                                std::unique_ptr<HermeticAudioRealm>& realm_out) {
  // Build the realm.
  realm_out = std::unique_ptr<HermeticAudioRealm>(
      new HermeticAudioRealm(BuildRealm(std::move(options), dispatcher)));
  auto& realm = realm_out->root_;

  // Start DriverTestRealm.
  zx::result<> boot_result = driver_test_realm::WaitForBootup(realm);
  ASSERT_EQ(ZX_OK, boot_result.status_value());

  // Hold a reference to fuchsia.virtualaudio.Control.
  ASSERT_NO_FATAL_FAILURE(ConnectToVirtualAudio(realm, realm_out->virtual_audio_control_));
}

void HermeticAudioRealm::Teardown(
    component_testing::ScopedChild::TeardownCallback on_teardown_complete) {
  root_.Teardown(std::move(on_teardown_complete));
}

HermeticAudioRealm::CtorArgs HermeticAudioRealm::BuildRealm(Options options,
                                                            async_dispatcher* dispatcher) {
  auto builder = component_testing::RealmBuilder::Create();

  using component_testing::ChildRef;
  using component_testing::Dictionary;
  using component_testing::DictionaryRef;
  using component_testing::Directory;
  using component_testing::DirectoryContents;
  using component_testing::ParentRef;
  using component_testing::Protocol;
  using component_testing::SelfRef;
  using component_testing::Service;

  builder.AddChild(kAudioCore, "#meta/audio_core.cm");

  auto inspect_tree = std::make_shared<std::optional<inspect::testing::TreeClient>>(std::nullopt);
  builder.AddLocalChild(InspectSinkMock::kName, [=] {
    return std::make_unique<InspectSinkMock>(dispatcher, inspect_tree);
  });

  // Route AudioCore -> test component.
  builder.AddRoute({
      .capabilities =
          {
              Protocol{.name = "fuchsia.media.ActivityReporter"},
              Protocol{.name = "fuchsia.media.Audio"},
              Protocol{.name = "fuchsia.media.AudioCore"},
              Protocol{.name = "fuchsia.media.AudioDeviceEnumerator"},
              Protocol{.name = "fuchsia.media.audio.EffectsController"},
              Protocol{.name = "fuchsia.media.tuning.AudioTuner"},
              Protocol{.name = "fuchsia.media.UsageGainReporter"},
              Protocol{.name = "fuchsia.media.UsageReporter"},
              Protocol{.name = "fuchsia.ultrasound.Factory"},
          },
      .source = ChildRef{kAudioCore},
      .targets = {ParentRef()},
  });

  builder.AddCapability(fuchsia::component::decl::Capability::WithDictionary(
      std::move(fuchsia::component::decl::Dictionary().set_name("test-diagnostics"))));

  builder.AddRoute({
      .capabilities =
          {
              Dictionary{.name = "test-diagnostics", .as = "diagnostics"},
          },
      .source = SelfRef{},
      .targets = {ChildRef{kAudioCore}},
  });

  // Route test component -> AudioCore.
  builder.AddRoute({
      .capabilities =
          {
              Protocol{.name = "fuchsia.scheduler.RoleManager"},
              // Not necessary for tests but can be useful when debugging tests.
              Protocol{.name = "fuchsia.tracing.provider.Registry"},
          },
      .source = ParentRef(),
      .targets = {ChildRef{kAudioCore}},
  });

  builder.AddRoute({
      .capabilities =
          {
              Protocol{.name = "fuchsia.inspect.InspectSink"},
          },
      .source = ChildRef{InspectSinkMock::kName},
      .targets = {DictionaryRef{.base_ref = SelfRef(), .path = "test-diagnostics"}},
  });

  builder.AddRoute({
      .capabilities =
          {
              Protocol{.name = "fuchsia.logger.LogSink"},
          },
      .source = ParentRef(),
      .targets = {DictionaryRef{.base_ref = SelfRef(), .path = "test-diagnostics"}},
  });

  switch (options.audio_core_config_data.index()) {
    case 0:  // empty
      builder.RouteReadOnlyDirectory("config-data", {ChildRef{kAudioCore}}, DirectoryContents());
      break;
    case 1: {  // route from parent
      // Export the given local directory as AudioCore's config-data. To export a directory,
      // we need to publish it in a component's outgoing directory. The simplest way to do that
      // is to export the directory from a local component.
      auto dir = std::get<1>(options.audio_core_config_data).directory_name;
      builder.AddLocalChild("local_config_data_exporter",
                            [dir] { return std::make_unique<LocalDirectoryExporter>(dir); });
      builder.AddRoute({
          .capabilities = {Directory{
              .name = LocalDirectoryExporter::kCapability,
              .as = "config-data",
              .rights = fuchsia::io::R_STAR_DIR,
              .path = LocalDirectoryExporter::kPath,
          }},
          .source = ChildRef{"local_config_data_exporter"},
          .targets = {ChildRef{kAudioCore}},
      });
      break;
    }
    case 2:  // use specified files
      builder.RouteReadOnlyDirectory("config-data", {ChildRef{kAudioCore}},
                                     std::move(std::get<2>(options.audio_core_config_data)));
      break;
    default:
      FX_CHECK(false) << "unexpected index " << options.audio_core_config_data.index();
  }

  // If needed, add a local component to host effects-over-FIDL.
  if (!options.test_effects_v2.empty()) {
    auto test_effects = std::move(options.test_effects_v2);
    builder.AddLocalChild("local_processor_creator", [test_effects = std::move(test_effects)] {
      return std::make_unique<LocalProcessorCreator>(test_effects);
    });
    builder.AddRoute({
        .capabilities = {Protocol{.name = "fuchsia.audio.effects.ProcessorCreator"}},
        .source = ChildRef{"local_processor_creator"},
        .targets = {ChildRef{kAudioCore}},
    });
  }

  fuchsia_driver_test::RealmArgs realm_args;
  realm_args.root_driver("fuchsia-boot:///platform-bus#meta/platform-bus.cm");
  realm_args.software_devices(std::vector{
      fuchsia_driver_test::SoftwareDevice{{
          .device_name = "virtual-audio-legacy",
          .device_id = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_VIRTUAL_AUDIO_LEGACY,
      }},
  });

  // Add a hermetic driver realm and route audio services to audio_core.
  driver_test_realm::OptionsBuilder options_builder;
  options_builder.driver_exposes({
      fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service{{
          .name = "fuchsia.hardware.audio.StreamConfigConnectorInputService",
      }}),
      fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service{{
          .name = "fuchsia.hardware.audio.StreamConfigConnectorOutputService",
      }}),
  });

  driver_test_realm::Setup(builder, dispatcher, options_builder.Build(), std::move(realm_args));
  builder.AddRoute({
      .capabilities =
          {
              Service{.name = "fuchsia.hardware.audio.StreamConfigConnectorInputService"},
              Service{.name = "fuchsia.hardware.audio.StreamConfigConnectorOutputService"},
          },
      .source = ChildRef{"driver_test_realm"},
      .targets = {ChildRef{kAudioCore}},
  });

  // Some tests need to control the thermal state.
  // For simplicity, always add this test thermal control server.
  builder.AddChild(kThermalTestControl, "#meta/thermal_test_control.cm");
  builder.AddRoute({
      .capabilities = {Protocol{.name = "fuchsia.thermal.ClientStateConnector"}},
      .source = ChildRef{kThermalTestControl},
      .targets = {ChildRef{kAudioCore}},
  });
  builder.AddRoute({
      .capabilities = {Protocol{.name = "test.thermal.ClientStateControl"}},
      .source = ChildRef{kThermalTestControl},
      .targets = {ParentRef()},
  });
  builder.AddRoute({
      .capabilities =
          {
              Dictionary{.name = "test-diagnostics", .as = "diagnostics"},
          },
      .source = SelfRef(),
      .targets = {ChildRef{kThermalTestControl}},
  });

  // Include a fake cobalt to silence warnings that we can't connect to cobalt.
  builder.AddChild(kFakeCobalt, "#meta/fake_cobalt.cm");
  builder.AddRoute({
      .capabilities = {Protocol{.name = "fuchsia.metrics.MetricEventLoggerFactory"}},
      .source = ChildRef{kFakeCobalt},
      .targets = {ChildRef{kAudioCore}},
  });
  builder.AddRoute({
      .capabilities =
          {
              Dictionary{.name = "test-diagnostics", .as = "diagnostics"},
          },
      .source = SelfRef(),
      .targets = {ChildRef{kFakeCobalt}},
  });

  // Lastly, allow further customization.
  if (options.customize_realm) {
    auto status = options.customize_realm(builder);
    FX_CHECK(status == ZX_OK) << "customize_realm failed with status=" << status;
  }

  // The lifecycle of local components created here is managed by the realm builder,
  // hence we return empty `.local_components`.
  return {.root = builder.Build(dispatcher), .local_components = {}, .inspect_tree = inspect_tree};
}

}  // namespace media::audio::test
