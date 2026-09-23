// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/tests/lib/enclosed_guest.h"

#include <dirent.h>
#include <fcntl.h>
#include <fuchsia/element/cpp/fidl.h>
#include <fuchsia/kernel/cpp/fidl.h>
#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/net/virtualization/cpp/fidl.h>
#include <fuchsia/scheduler/cpp/fidl.h>
#include <fuchsia/sysinfo/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/ui/app/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <fuchsia/ui/input3/cpp/fidl.h>
#include <fuchsia/ui/observation/geometry/cpp/fidl.h>
#include <fuchsia/virtualization/cpp/fidl.h>
#include <fuchsia/vulkan/loader/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fit/result.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <sys/mount.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <algorithm>
#include <cmath>
#include <memory>
#include <optional>
#include <string>

#include <fbl/unique_fd.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/ui/testing/ui_test_realm/ui_test_realm.h"
#include "src/virtualization/tests/lib/guest_constants.h"
#include "src/virtualization/tests/lib/logger.h"
#include "src/virtualization/tests/lib/periodic_logger.h"

namespace {

using fuchsia::ui::observation::geometry::ViewDescriptor;

constexpr char kZirconGuestUrl[] = "zircon_guest_manager#meta/zircon_guest_manager.cm";
constexpr char kDebianGuestUrl[] = "debian_guest_manager#meta/debian_guest_manager.cm";

constexpr auto kGuestManagerName = "guest_manager";

// TODO(https://fxbug.dev/42076670): Use consistent naming for the test utils here.
constexpr char kDebianTestUtilDir[] = "/test_utils";
constexpr zx::duration kRetryStep = zx::msec(200);

std::string JoinArgVector(const std::vector<std::string>& argv) {
  std::string result;
  for (const auto& arg : argv) {
    result += arg;
    result += " ";
  }
  return result;
}

void InstallTestGraphicalPresenter(component_testing::Realm& realm) {
  using component_testing::ChildRef;
  using component_testing::ParentRef;
  using component_testing::Protocol;
  using component_testing::Route;

  // UITestRealm does not currently provide a fuchsia.element.GraphicalPresenter, but the
  // test_graphical_presenter exposes a ViewProvider and a GraphicalPresenter. We will connect this
  // to the UITestRealm such that our view under test will become a child of the
  // test_graphical_presetner.
  constexpr auto kGraphicalPresenterName = "test_graphical_presenter";
  constexpr auto kGraphicalPresenterUrl = "#meta/test_graphical_presenter.cm";
  realm.AddChild(kGraphicalPresenterName, kGraphicalPresenterUrl);
  realm
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::logger::LogSink::Name_},
                              Protocol{fuchsia::scheduler::RoleManager::Name_},
                              Protocol{fuchsia::sysmem::Allocator::Name_},
                              Protocol{fuchsia::sysmem2::Allocator::Name_},
                              Protocol{fuchsia::tracing::provider::Registry::Name_},
                              Protocol{fuchsia::vulkan::loader::Loader::Name_},
                              Protocol{fuchsia::ui::composition::Flatland::Name_},
                              Protocol{fuchsia::ui::composition::Allocator::Name_},
                              Protocol{fuchsia::ui::input3::Keyboard::Name_},
                          },
                      .source = {ParentRef()},
                      .targets = {ChildRef{kGraphicalPresenterName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::element::GraphicalPresenter::Name_},
                          },
                      .source = {ChildRef{kGraphicalPresenterName}},
                      .targets = {ChildRef{kGuestManagerName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::ui::app::ViewProvider::Name_},
                          },
                      .source = {ChildRef{kGraphicalPresenterName}},
                      .targets = {ParentRef()}});
}

std::optional<ViewDescriptor> FindDisplayView(ui_testing::UITestManager& ui_test_manager) {
  auto presenter_koid = ui_test_manager.ClientViewRefKoid();
  if (!presenter_koid) {
    return {};
  }
  auto presenter = ui_test_manager.FindViewFromSnapshotByKoid(*presenter_koid);
  if (!presenter || !presenter->has_children() || presenter->children().empty()) {
    return {};
  }
  return ui_test_manager.FindViewFromSnapshotByKoid(presenter->children()[0]);
}

}  // namespace

// Execute |command| on the guest serial and wait for the |result|.
zx_status_t EnclosedGuest::Execute(const std::vector<std::string>& argv,
                                   const std::unordered_map<std::string, std::string>& env,
                                   zx::time deadline, std::string* result, int32_t* return_code) {
  if (!env.empty()) {
    FX_LOGS(ERROR) << "EnclosedGuest::Execute does not accept environment variables.";
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto command = JoinArgVector(argv);
  return console_->ExecuteBlocking(command, ShellPrompt(), deadline, result);
}

std::unique_ptr<sys::ServiceDirectory> EnclosedGuest::StartWithRealmBuilder(
    zx::time deadline, GuestLaunchInfo& guest_launch_info) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  InstallInRealm(realm_builder.root(), guest_launch_info);
  realm_root_ = realm_builder.Build(dispatcher_);
  return std::make_unique<sys::ServiceDirectory>(realm_root_->component().CloneExposedDir());
}

std::unique_ptr<sys::ServiceDirectory> EnclosedGuest::StartWithUITestManager(
    zx::time deadline, GuestLaunchInfo& guest_launch_info) {
  using component_testing::Directory;
  using component_testing::Protocol;
  using component_testing::Storage;

  // UITestManager allows us to run these tests against a hermetic UI stack (ex: to test
  // interactions with Flatland, GraphicalPresenter, and Input).
  //
  // As structured, the virtualization components will be run in a sub-realm created by the
  // UITestRealm. Some of the below config fields will allow us to route capabilities through that
  // realm.
  ui_testing::UITestRealm::Config ui_config;
  ui_config.use_scene_owner = true;
  ui_config.accessibility_owner = ui_testing::UITestRealm::AccessibilityOwnerType::FAKE;

  // These are services that we need to expose from the UITestRealm.
  ui_config.exposed_client_services = {guest_launch_info.interface_name,
                                       fuchsia::virtualization::LinuxManager::Name_,
                                       fuchsia::ui::app::ViewProvider::Name_};

  // These are the services we need to consume from the UITestRealm.
  ui_config.ui_to_client_services = {
      fuchsia::ui::composition::Flatland::Name_,
      fuchsia::ui::composition::Allocator::Name_,
      fuchsia::ui::input3::Keyboard::Name_,
  };

  // These are the parent services (from our cml) that we need the UITestRealm to forward to use so
  // that they can be routed to the guest manager.
  ui_config.passthrough_capabilities = {
      Protocol{fuchsia::kernel::HypervisorResource::Name_},
      Protocol{fuchsia::kernel::VmexResource::Name_},
      Protocol{fuchsia::sysinfo::SysInfo::Name_},
      Storage{.name = "data", .path = "/data"},
  };

  // Now create and install the virtualization components into a new sub-realm.
  ui_test_manager_.emplace(std::move(ui_config));
  auto guest_realm = ui_test_manager_->AddSubrealm();
  InstallInRealm(guest_realm, guest_launch_info);
  InstallTestGraphicalPresenter(guest_realm);
  ui_test_manager_->BuildRealm();
  ui_test_manager_->InitializeScene();
  return ui_test_manager_->CloneExposedServicesDirectory();
}

EnclosedGuest::~EnclosedGuest() {
  bool ui_test_manager_teardown_complete = false;
  if (ui_test_manager_.has_value()) {
    ui_test_manager_->TeardownRealm(
        [&](fit::result<fuchsia::component::Error>) { ui_test_manager_teardown_complete = true; });
  } else {
    ui_test_manager_teardown_complete = true;
  }
  bool realm_root_teardown_complete = false;
  if (realm_root_.has_value()) {
    realm_root_->Teardown(
        [&](fit::result<fuchsia::component::Error>) { realm_root_teardown_complete = true; });
  } else {
    realm_root_teardown_complete = true;
  }
  RunLoopUntil([&]() { return ui_test_manager_teardown_complete && realm_root_teardown_complete; },
               zx::time::infinite());
}

zx_status_t EnclosedGuest::Start(zx::time deadline) {
  using component_testing::RealmBuilder;
  using component_testing::RealmRoot;

  GuestLaunchInfo guest_launch_info;
  if (auto status = BuildLaunchInfo(&guest_launch_info); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failure building GuestLaunchInfo";
    return status;
  }

  // Tests must be explicit about GPU support in the tests.
  //
  // If we need GPU support we will launch with UITestManager to provide a hermetic instance of UI
  // and input services. Otherwise we will launch directly using RealmBuilder. We make this
  // distinction because UITestManager depends on the availability of vulkan and we can avoid that
  // dependency for tests that don't need to test any interactions with the UI stack.
  FX_CHECK(guest_launch_info.config.has_virtio_gpu())
      << "virtio-gpu support must be explicitly declared.";
  std::unique_ptr<sys::ServiceDirectory> realm_services;
  if (guest_launch_info.config.virtio_gpu()) {
    realm_services = StartWithUITestManager(deadline, guest_launch_info);
  } else {
    realm_services = StartWithRealmBuilder(deadline, guest_launch_info);
  }

  return LaunchInRealm(std::move(realm_services), guest_launch_info, deadline);
}

void EnclosedGuest::InstallInRealm(component_testing::Realm& realm,
                                   GuestLaunchInfo& guest_launch_info) {
  using component_testing::ChildRef;
  using component_testing::Directory;
  using component_testing::ParentRef;
  using component_testing::Protocol;
  using component_testing::Route;
  using component_testing::Storage;

  constexpr auto kFakeNetstackComponentName = "fake_netstack";
  constexpr auto kFakeMemoryPressureProvider = "fake_memory_pressure_provider";

  realm.AddChild(kGuestManagerName, guest_launch_info.url);
  realm.AddLocalChild(kFakeNetstackComponentName, [&]() { return fake_netstack_.NewComponent(); });
  realm.AddLocalChild(kFakeMemoryPressureProvider,
                      [&]() { return fake_memory_pressure_provider_.NewComponent(); });

  realm
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::logger::LogSink::Name_},
                              Protocol{fuchsia::scheduler::RoleManager::Name_},
                              Protocol{fuchsia::sysmem::Allocator::Name_},
                              Protocol{fuchsia::sysmem2::Allocator::Name_},
                              Protocol{fuchsia::tracing::provider::Registry::Name_},
                              Protocol{fuchsia::vulkan::loader::Loader::Name_},
                              Protocol{fuchsia::ui::composition::Flatland::Name_},
                              Protocol{fuchsia::ui::composition::Allocator::Name_},
                              Protocol{fuchsia::ui::input3::Keyboard::Name_},
                          },
                      .source = {ParentRef()},
                      .targets = {ChildRef{kGuestManagerName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::kernel::HypervisorResource::Name_},
                              Protocol{fuchsia::kernel::VmexResource::Name_},
                              Protocol{fuchsia::sysinfo::SysInfo::Name_},
                              Storage{.name = "data", .path = "/data"},
                          },
                      .source = {ParentRef()},
                      .targets = {ChildRef{kGuestManagerName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::net::virtualization::Control::Name_},
                          },
                      .source = {ChildRef{kFakeNetstackComponentName}},
                      .targets = {ChildRef{kGuestManagerName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::memorypressure::Provider::Name_},
                          },
                      .source = {ChildRef{kFakeMemoryPressureProvider}},
                      .targets = {ChildRef{kGuestManagerName}}})
      .AddRoute(Route{.capabilities =
                          {
                              Protocol{fuchsia::virtualization::LinuxManager::Name_},
                              Protocol{guest_launch_info.interface_name},
                          },
                      .source = ChildRef{kGuestManagerName},
                      .targets = {ParentRef()}});
}

zx_status_t EnclosedGuest::LaunchInRealm(std::unique_ptr<sys::ServiceDirectory> services,
                                         GuestLaunchInfo& guest_launch_info, zx::time deadline) {
  realm_services_ = std::move(services);

  guest_manager_ =
      realm_services_
          ->Connect<fuchsia::virtualization::GuestManager>(guest_launch_info.interface_name)
          .Unbind()
          .BindSync();

  return LaunchInternal(guest_launch_info, deadline);
}

zx_status_t EnclosedGuest::LaunchInternal(GuestLaunchInfo& guest_launch_info, zx::time deadline) {
  Logger::Get().Reset();
  PeriodicLogger logger;
  guest_error_ = std::nullopt;

  // Get whether the vsock device will be installed for this guest. This is used later to validate
  // whether we expect GetHostVsockEndpoint to succeed.
  const bool vsock_enabled =
      !guest_launch_info.config.has_virtio_vsock() || guest_launch_info.config.virtio_vsock();

  fuchsia::virtualization::GuestManager_Launch_Result res;
  auto status =
      guest_manager_->Launch(std::move(guest_launch_info.config), guest_.NewRequest(), &res);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failure launching guest " << guest_launch_info.url;
    return status;
  }
  if (res.is_err()) {
    FX_LOGS(ERROR) << "Launch failed with error " << static_cast<uint32_t>(res.err())
                   << " for guest " << guest_launch_info.url;
    return ZX_ERR_INTERNAL;
  }

  guest_cid_ = fuchsia::virtualization::DEFAULT_GUEST_CID;

  if (vsock_enabled && GetHostVsockEndpoint(vsock_.NewRequest()).is_error()) {
    FX_LOGS(ERROR) << "Failed to get host vsock endpoint";
    return ZX_ERR_INTERNAL;
  }

  // Launch the guest.
  logger.Start("Launching guest", zx::sec(5));
  guest_.set_error_handler([this](zx_status_t status) { this->guest_error_ = status; });

  // Connect to guest serial, and log it to the logger.
  logger.Start("Connecting to guest serial", zx::sec(10));
  std::optional<zx::socket> get_serial_result;

  guest_->GetSerial(
      [&get_serial_result](zx::socket socket) { get_serial_result = std::move(socket); });

  bool success = RunLoopUntil(
      [this, &get_serial_result] {
        return this->guest_error_.has_value() || get_serial_result.has_value();
      },
      deadline);
  if (!success) {
    FX_LOGS(ERROR) << "Timed out waiting to connect to guest's serial";
    return ZX_ERR_TIMED_OUT;
  }
  if (guest_error_.has_value()) {
    FX_LOGS(ERROR) << "Error connecting to guest's serial: "
                   << zx_status_get_string(guest_error_.value());
    return guest_error_.value();
  }
  serial_logger_.emplace(&Logger::Get(), std::move(get_serial_result.value()));

  // Connect to guest console.
  logger.Start("Connecting to guest console", zx::sec(10));
  std::optional<fuchsia::virtualization::Guest_GetConsole_Result> get_console_result;
  guest_->GetConsole(
      [&get_console_result](fuchsia::virtualization::Guest_GetConsole_Result result) {
        get_console_result = std::move(result);
      });
  success = RunLoopUntil(
      [this, &get_console_result] {
        return guest_error_.has_value() || get_console_result.has_value();
      },
      deadline);
  if (!success) {
    FX_LOGS(ERROR) << "Timed out waiting to connect to guest's console";
    return ZX_ERR_TIMED_OUT;
  }
  if (guest_error_.has_value()) {
    FX_LOGS(ERROR) << "Error connecting to guest's console: "
                   << zx_status_get_string(guest_error_.value());
    return guest_error_.value();
  }
  if (get_console_result->is_err()) {
    FX_LOGS(ERROR) << "Failed to open guest console"
                   << static_cast<int32_t>(get_console_result->err());
    return ZX_ERR_INTERNAL;
  }
  console_.emplace(std::make_unique<ZxSocket>(std::move(get_console_result->response().socket)));

  // Wait for output to appear on the console.
  logger.Start("Waiting for output to appear on guest console", zx::sec(10));
  status = console_->Start(deadline);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Error waiting for output on guest console: " << zx_status_get_string(status);
    return status;
  }

  // Poll the system for all services to come up.
  logger.Start("Waiting for system to become ready", zx::sec(10));
  status = WaitForSystemReady(deadline);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failure while waiting for guest system to become ready: "
                   << zx_status_get_string(status);
    return status;
  }

  return ZX_OK;
}

zx_status_t EnclosedGuest::ForceRestart(GuestLaunchInfo& guest_launch_info, zx::time deadline) {
  guest_manager_->ForceShutdown();

  // Instead of waiting on the guest closed signal to determine whether a VM has shutdown, this
  // polls the guest status. This is done to avoid a race where the VM object has stopped but the
  // guest manager isn't yet aware, and is thus not in a state where a VM can be restarted.
  const bool shutdown_complete = RunLoopUntil(
      [this] {
        ::fuchsia::virtualization::GuestInfo info;
        guest_manager_->GetInfo(&info);
        return info.guest_status() == ::fuchsia::virtualization::GuestStatus::STOPPED;
      },
      zx::deadline_after(zx::sec(20)));
  if (!shutdown_complete) {
    FX_LOGS(ERROR) << "Timed out waiting for the guest to report shutdown";
    return ZX_ERR_TIMED_OUT;
  }

  return LaunchInternal(guest_launch_info, deadline);
}

fit::result<::fuchsia::virtualization::GuestError> EnclosedGuest::ConnectToBalloon(
    ::fidl::InterfaceRequest<::fuchsia::virtualization::BalloonController> controller) {
  zx_status_t status = ZX_ERR_TIMED_OUT;
  fuchsia::virtualization::GuestError error;
  guest_->GetBalloonController(
      std::move(controller),
      [&status, &error](fuchsia::virtualization::Guest_GetBalloonController_Result result) {
        if (result.is_response()) {
          status = ZX_OK;
        } else {
          status = ZX_ERR_INTERNAL;
          error = result.err();
        }
      });

  const bool success = RunLoopUntil([&status] { return status != ZX_ERR_TIMED_OUT; },
                                    zx::deadline_after(zx::sec(20)));
  if (!success) {
    FX_LOGS(ERROR) << "Timed out waiting to get balloon controller";
    return fit::error(fuchsia::virtualization::GuestError::DEVICE_NOT_PRESENT);
  }

  if (status != ZX_OK) {
    return fit::error(error);
  }
  return fit::ok();
}

fit::result<::fuchsia::virtualization::GuestError> EnclosedGuest::ConnectToMem(
    ::fidl::InterfaceRequest<::fuchsia::virtualization::MemController> controller) {
  zx_status_t status = ZX_ERR_TIMED_OUT;
  fuchsia::virtualization::GuestError error;
  guest_->GetMemController(
      std::move(controller),
      [&status, &error](fuchsia::virtualization::Guest_GetMemController_Result result) {
        if (result.is_response()) {
          status = ZX_OK;
        } else {
          status = ZX_ERR_INTERNAL;
          error = result.err();
        }
      });

  const bool success = RunLoopUntil([&status] { return status != ZX_ERR_TIMED_OUT; },
                                    zx::deadline_after(zx::sec(20)));
  if (!success) {
    FX_LOGS(ERROR) << "Timed out waiting to get mem controller";
    return fit::error(fuchsia::virtualization::GuestError::DEVICE_NOT_PRESENT);
  }

  if (status != ZX_OK) {
    return fit::error(error);
  }
  return fit::ok();
}

fit::result<::fuchsia::virtualization::GuestError> EnclosedGuest::GetHostVsockEndpoint(
    ::fidl::InterfaceRequest<::fuchsia::virtualization::HostVsockEndpoint> endpoint) {
  zx_status_t status = ZX_ERR_TIMED_OUT;
  fuchsia::virtualization::GuestError error;
  guest_->GetHostVsockEndpoint(
      std::move(endpoint),
      [&status, &error](fuchsia::virtualization::Guest_GetHostVsockEndpoint_Result result) {
        if (result.is_response()) {
          status = ZX_OK;
        } else {
          status = ZX_ERR_INTERNAL;
          error = result.err();
        }
      });

  const bool success = RunLoopUntil([&status] { return status != ZX_ERR_TIMED_OUT; },
                                    zx::deadline_after(zx::sec(20)));
  if (!success) {
    FX_LOGS(ERROR) << "Timed out waiting to get host vsock endpoint";
    return fit::error(fuchsia::virtualization::GuestError::DEVICE_NOT_PRESENT);
  }

  if (status != ZX_OK) {
    return fit::error(error);
  }
  return fit::ok();
}

zx_status_t EnclosedGuest::Stop(zx::time deadline) {
  zx_status_t status = ShutdownAndWait(deadline);
  if (status != ZX_OK) {
    return status;
  }
  return ZX_OK;
}

zx_status_t EnclosedGuest::RunUtil(const std::string& util, const std::vector<std::string>& argv,
                                   zx::time deadline, std::string* result) {
  return Execute(GetTestUtilCommand(util, argv), {}, deadline, result);
}

bool EnclosedGuest::RunLoopUntil(fit::function<bool()> condition, zx::time deadline) {
  zx::duration timeout = deadline - zx::clock::get_monotonic();
  return run_loop_until_(std::move(condition), timeout);
}

zx_status_t ZirconEnclosedGuest::BuildLaunchInfo(GuestLaunchInfo* launch_info) {
  launch_info->url = kZirconGuestUrl;
  launch_info->interface_name = fuchsia::virtualization::ZirconGuestManager::Name_;
  // Disable netsvc to avoid spamming the net device with logs.
  launch_info->config.mutable_cmdline_add()->emplace_back("netsvc.disable=true");
  launch_info->config.set_virtio_gpu(enable_gpu_);
  return ZX_OK;
}

EnclosedGuest::DisplayInfo EnclosedGuest::WaitForDisplay() {
  // Wait for the display view to render.
  std::optional<ViewDescriptor> view_descriptor;
  RunLoopUntil(
      [this, &view_descriptor] {
        view_descriptor = FindDisplayView(*ui_test_manager_);
        return view_descriptor.has_value();
      },
      zx::deadline_after(zx::sec(20)));

  // Now wait for the view to get focus.
  auto koid = view_descriptor->view_ref_koid();
  RunLoopUntil([this, koid] { return ui_test_manager_->ViewIsFocused(koid); },
               zx::time::infinite());

  const auto& extent = view_descriptor->layout().extent;
  return DisplayInfo{
      .width = static_cast<uint32_t>(std::round(extent.max.x - extent.min.x)),
      .height = static_cast<uint32_t>(std::round(extent.max.y - extent.min.y)),
  };
}

namespace {
fit::result<std::string> EnsureValidZirconPsOutput(std::string_view ps_output) {
  if (ps_output.find("virtual-console") == std::string::npos) {
    return fit::error("'virtual-console' cannot be found in 'ps' output");
  }
  if (ps_output.find("pkg-cache") == std::string::npos) {
    return fit::error("'pkg-cache' cannot be found in 'ps' output");
  }
  return fit::ok();
}
}  // namespace

zx_status_t ZirconEnclosedGuest::WaitForSystemReady(zx::time deadline) {
  std::string output;

  // Keep running the ready test until we get a reasonable result or run out of time. Ideally we
  // want to wait for the driver framework to have finished enumerating and binding devices, as
  // shutting down before this is a known bug that can lead to system hangs.
  // Checking for this is difficult and for x86 we can wait for the ACPI driver to finish (which is
  // the last thing to come up), but otherwise we just wait for some general system processes to
  // come up, by inspecting ps, and hope that is good enough.
#if __x86_64__
  auto acpi_check = [&]() {
    do {
      zx_status_t status =
          Execute({"waitfor", "verbose", "class=acpi", "topo=/dev/sys/platform/pt/acpi/_SB_/pt",
                   "&&", "echo", "ACPI_READY"},
                  {}, deadline, &output);
      if (status != ZX_OK) {
        return status;
      }
      if (output.find("ACPI_READY") != std::string::npos) {
        return ZX_OK;
      }

      // Keep trying until we run out of time.
      zx::nanosleep(std::min(zx::deadline_after(kRetryStep), deadline));
    } while (zx::clock::get_monotonic() < deadline);

    FX_LOGS(ERROR) << "Failed to wait for ACPI_READY";
    return ZX_ERR_TIMED_OUT;
  };
  if (zx_status_t status = acpi_check(); status != ZX_OK) {
    return status;
  }
#endif
  do {
    // Execute `ps`.
    zx_status_t status = Execute({"ps"}, {}, deadline, &output);
    if (status != ZX_OK) {
      return status;
    }
    if (EnsureValidZirconPsOutput(output).is_ok()) {
      return ZX_OK;
    }
    // Keep trying until we run out of time.
    zx::nanosleep(std::min(zx::deadline_after(kRetryStep), deadline));
  } while (zx::clock::get_monotonic() < deadline);

  FX_LOGS(ERROR) << "Failed to wait for processes: "
                 << EnsureValidZirconPsOutput(output).error_value();
  return ZX_ERR_TIMED_OUT;
}

zx_status_t ZirconEnclosedGuest::ShutdownAndWait(zx::time deadline) {
  std::optional<GuestConsole>& console_opt = GetConsole();
  if (console_opt.has_value()) {
    GuestConsole& console = console_opt.value();
    zx_status_t status = console.SendBlocking("power shutdown\n", deadline);
    if (status != ZX_OK) {
      return status;
    }
    return console.WaitForSocketClosed(deadline);
  }
  return ZX_OK;
}

std::vector<std::string> ZirconEnclosedGuest::GetTestUtilCommand(
    const std::string& util, const std::vector<std::string>& argv) {
  std::vector<std::string> exec_argv = {util};
  exec_argv.insert(exec_argv.end(), argv.begin(), argv.end());
  return exec_argv;
}

zx_status_t DebianEnclosedGuest::BuildLaunchInfo(GuestLaunchInfo* launch_info) {
  launch_info->url = kDebianGuestUrl;
  launch_info->interface_name = fuchsia::virtualization::DebianGuestManager::Name_;
  // Enable kernel debugging serial output.
  for (std::string_view cmd : kLinuxKernelSerialDebugCmdline) {
    launch_info->config.mutable_cmdline_add()->emplace_back(cmd);
  }
  launch_info->config.set_virtio_gpu(enable_gpu_);
  return ZX_OK;
}

zx_status_t DebianEnclosedGuest::WaitForSystemReady(zx::time deadline) {
  std::optional<GuestConsole>& console_opt = GetConsole();
  if (console_opt.has_value()) {
    GuestConsole& console = console_opt.value();
    constexpr zx::duration kEchoWaitTime = zx::sec(1);
    return console.RepeatCommandTillSuccess("echo guest ready", ShellPrompt(), "guest ready",
                                            deadline, kEchoWaitTime);
  }
  return ZX_ERR_BAD_STATE;
}

zx_status_t DebianEnclosedGuest::ShutdownAndWait(zx::time deadline) {
  PeriodicLogger logger("Attempting to shut down guest", zx::sec(10));
  std::optional<GuestConsole>& console_opt = GetConsole();
  if (console_opt.has_value()) {
    GuestConsole& console = console_opt.value();
    zx_status_t status = console.SendBlocking("shutdown now\n", deadline);
    if (status != ZX_OK) {
      return status;
    }
    return console.WaitForSocketClosed(deadline);
  }
  return ZX_OK;
}

std::vector<std::string> DebianEnclosedGuest::GetTestUtilCommand(
    const std::string& util, const std::vector<std::string>& argv) {
  std::string bin_path = fxl::StringPrintf("%s/%s", kDebianTestUtilDir, util.c_str());

  std::vector<std::string> exec_argv = {bin_path};
  exec_argv.insert(exec_argv.end(), argv.begin(), argv.end());
  return exec_argv;
}
