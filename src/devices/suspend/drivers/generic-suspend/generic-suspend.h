// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SUSPEND_DRIVERS_GENERIC_SUSPEND_GENERIC_SUSPEND_H_
#define SRC_DEVICES_SUSPEND_DRIVERS_GENERIC_SUSPEND_GENERIC_SUSPEND_H_

#include <fidl/fuchsia.hardware.power.suspend/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <zircon/syscalls-next.h>

#include "lib/fdf/cpp/dispatcher.h"

namespace suspend {

/// Maximum supported wake source report entries.
constexpr uint32_t kMaxWakeSourceEntriesCount = 100;

/// Reports the information about wake sources as reported by the kernel upon
/// call to `zx_system_suspend_enter`.
struct WakeSourceReport {
  /// The kernel's wake source report header information, always filled out.
  zx_wake_source_report_header_t header;
  /// The number of wake sources available in the kernel to report.  This number
  /// may be larger than `kMaxWakeSourceEntriesCount`, in which case the overflow
  /// is simply omitted.
  uint32_t actual_entry_count;
  /// The wake source details as reported by the kernel. We forward a maximum
  /// of `kMaxWakeSourceEntriesCount`, if there are more those are not reported.
  ///
  /// However, the specified maximum number of entries should be enough for
  /// everyone.
  zx_wake_source_report_entry_t entries[kMaxWakeSourceEntriesCount];
};

class GenericSuspend : public fdf::DriverBase2,
                       public fidl::WireServer<fuchsia_hardware_power_suspend::Suspender> {
 public:
  GenericSuspend();

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power_suspend::Suspender> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unexpected suspend FIDL call: 0x{:x}", metadata.method_ordinal);
  }

  void GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) override;
  void Suspend(SuspendRequestView request, SuspendCompleter::Sync& completer) override;
  void ForceLowestPowerMode(ForceLowestPowerModeRequestView request,
                            ForceLowestPowerModeCompleter::Sync& completer) override;

 protected:
  virtual zx::result<zx::resource> GetCpuResource(fdf::Namespace& incoming);
  virtual zx::result<WakeSourceReport> SystemSuspendEnter();

  // Called just at Start(). Used in testing, otherwise a no-op.
  virtual void AtStart() {}

 private:
  void Serve(fidl::ServerEnd<fuchsia_hardware_power_suspend::Suspender> request);
  zx::result<> CreateDevfsNode();

  std::optional<inspect::BoundedListNode> inspect_events_;
  std::optional<inspect::ComponentInspector> exposed_inspector_;

  fidl::ServerBindingGroup<fuchsia_hardware_power_suspend::Suspender> suspend_bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_hardware_power_suspend::Suspender> devfs_connector_;

  zx::resource cpu_resource_;
};

}  // namespace suspend

#endif  // SRC_DEVICES_SUSPEND_DRIVERS_GENERIC_SUSPEND_GENERIC_SUSPEND_H_
