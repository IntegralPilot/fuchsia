// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-cpu.h"

#include <fuchsia/hardware/thermal/cpp/banjo.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/inspect/cpp/inspector.h>
#include <zircon/errors.h>

#include <map>
#include <memory>
#include <optional>

#include <ddktl/fidl.h>
#include <soc/aml-common/aml-cpu-metadata.h>

#include "fidl/fuchsia.hardware.thermal/cpp/wire.h"

namespace {
using fuchsia_hardware_thermal::wire::kMaxDvfsDomains;
using fuchsia_hardware_thermal::wire::PowerDomain;

constexpr zx_off_t kCpuVersionOffset = 0x220;

fidl::WireSyncClient<amlogic_cpu::fuchsia_thermal::Device> CreateFidlClient(
    const ddk::ThermalProtocolClient& protocol_client, zx_status_t* status) {
  // This channel pair will be used to talk to the Thermal Device's FIDL
  // interface.
  zx::result endpoints = fidl::CreateEndpoints<amlogic_cpu::fuchsia_thermal::Device>();
  *status = endpoints.status_value();
  if (*status != ZX_OK) {
    zxlogf(ERROR, "aml-cpu: Failed to create channel pair, st = %d\n", *status);
    return {};
  }
  auto& [channel_local, channel_remote] = endpoints.value();

  // Pass one end of the channel to the Thermal driver. The thermal driver will
  // serve its FIDL interface over this channel.
  *status = protocol_client.Connect(channel_remote.TakeChannel());
  if (*status != ZX_OK) {
    zxlogf(ERROR, "aml-cpu: failed to connect to thermal driver, st = %d\n", *status);
    return {};
  }

  return fidl::WireSyncClient{std::move(channel_local)};
}

zx_status_t GetDeviceName(bool big_little, PowerDomain power_domain, char const** name) {
  if (!big_little) {
    *name = "domain-0";
  } else {
    switch (power_domain) {
      case PowerDomain::kBigClusterPowerDomain:
        *name = "big-cluster";
        break;
      case PowerDomain::kLittleClusterPowerDomain:
        *name = "little-cluster";
        break;
      default:
        zxlogf(ERROR, "aml-cpu: Got invalid power domain %u", static_cast<uint32_t>(power_domain));
        *name = "invalid";
        return ZX_ERR_INVALID_ARGS;
    }
  }
  return ZX_OK;
}

}  // namespace
namespace amlogic_cpu {

zx_status_t AmlCpu::Create(void* context, zx_device_t* parent) {
  zx_status_t status;

  // Determine the cluster size of each cluster.
  auto cluster_info_metadata =
      ddk::GetMetadataArray<legacy_cluster_info_t>(parent, DEVICE_METADATA_CLUSTER_SIZE_LEGACY);
  if (!cluster_info_metadata.is_ok()) {
    return cluster_info_metadata.error_value();
  }

  std::map<PerfDomainId, legacy_cluster_info_t> cluster_info_map;
  for (auto cluster_info : cluster_info_metadata.value()) {
    cluster_info_map[cluster_info.pd_id] = cluster_info;
  }

  // The Thermal Driver is our parent and it exports an interface with one
  // method (Connect) which allows us to connect to its FIDL interface.
  ddk::ThermalProtocolClient thermal_protocol_client;
  status =
      ddk::ThermalProtocolClient::CreateFromDevice(parent, "thermal", &thermal_protocol_client);
  if (status != ZX_OK) {
    zxlogf(ERROR, "aml-cpu: Failed to get thermal protocol client, st = %d", status);
    return status;
  }

  auto thermal_fidl_client = CreateFidlClient(thermal_protocol_client, &status);
  if (!thermal_fidl_client) {
    return status;
  }

  auto device_info = thermal_fidl_client->GetDeviceInfo();
  if (device_info.status() != ZX_OK) {
    zxlogf(ERROR, "aml-cpu: failed to get device info, st = %d", device_info.status());
    return device_info.status();
  }

  const fuchsia_thermal::wire::ThermalDeviceInfo* info = device_info.value().info.get();

  // Ensure there is at least one non-empty power domain. We expect one to exist if this function
  // has been called.
  {
    bool found_nonempty_domain = false;
    for (size_t i = 0; i < kMaxDvfsDomains; i++) {
      if (info->opps[i].count > 0) {
        found_nonempty_domain = true;
        break;
      }
    }
    if (!found_nonempty_domain) {
      zxlogf(ERROR, "aml-cpu: No cpu devices were created; all power domains are empty\n");
      return ZX_ERR_INTERNAL;
    }
  }

  // Look up the CPU version.
  uint32_t cpu_version_packed = 0;
  {
    zx::result pdev_client_end =
        DdkConnectFragmentFidlProtocol<fuchsia_hardware_platform_device::Service::Device>(parent,
                                                                                          "pdev");
    if (pdev_client_end.is_error()) {
      zxlogf(ERROR, "Failed to connect to platform device: %s", pdev_client_end.status_string());
      return pdev_client_end.status_value();
    }
    fdf::PDev pdev{std::move(pdev_client_end.value())};

    // Map AOBUS registers
    zx::result mmio_buffer = pdev.MapMmio(0);
    if (mmio_buffer.is_error()) {
      zxlogf(ERROR, "Failed to map mmio: %s", mmio_buffer.status_string());
      return mmio_buffer.status_value();
    }

    cpu_version_packed = mmio_buffer->Read32(kCpuVersionOffset);
  }

  // Create an AmlCpu for each power domain with nonempty operating points.
  for (size_t i = 0; i < kMaxDvfsDomains; i++) {
    const fuchsia_thermal::wire::OperatingPoint& opps = info->opps[i];

    // If this domain is empty, don't create a driver.
    if (opps.count == 0) {
      continue;
    }

    const auto& cluster_core_info_it = cluster_info_map.find(i);
    if (cluster_core_info_it == cluster_info_map.end()) {
      zxlogf(ERROR, "aml-cpu: Could not find cluster core count for cluster %lu", i);
      return ZX_ERR_NOT_FOUND;
    }
    const auto& cluster_core_info = cluster_core_info_it->second;

    // If the FIDL client has been previously consumed, create a new one. Then build the CPU device
    // and consume the FIDL client.
    if (!thermal_fidl_client) {
      thermal_fidl_client = CreateFidlClient(thermal_protocol_client, &status);
      if (!thermal_fidl_client) {
        return status;
      }
    }
    auto cpu_device = std::make_unique<AmlCpu>(parent, std::move(thermal_fidl_client), i,
                                               cluster_core_info.core_count,
                                               cluster_core_info.relative_performance);
    thermal_fidl_client = {};

    cpu_device->SetCpuInfo(cpu_version_packed);

    char const* name;
    status = GetDeviceName(info->big_little, static_cast<PowerDomain>(i), &name);
    if (status != ZX_OK) {
      return status;
    }

    auto directory_client = cpu_device->AddService();
    if (directory_client.is_error()) {
      zxlogf(ERROR, "aml-cpu: Failed to add cpu control service to outgoing directory (%d): %s",
             directory_client.status_value(), directory_client.status_string());
      return directory_client.status_value();
    }

    std::array offers = {
        fuchsia_cpuctrl::Service::Name,
    };

    status = cpu_device->DdkAdd(ddk::DeviceAddArgs(name)
                                    .set_flags(DEVICE_ADD_NON_BINDABLE)
                                    .set_proto_id(ZX_PROTOCOL_CPU_CTRL)
                                    .set_fidl_service_offers(offers)
                                    .set_outgoing_dir(directory_client->TakeChannel())
                                    .set_inspect_vmo(cpu_device->inspector_.DuplicateVmo()));

    if (status != ZX_OK) {
      zxlogf(ERROR, "aml-cpu: Failed to add cpu device for domain %zu, st = %d\n", i, status);
      return status;
    }

    // Intentionally leak this device because it's owned by the driver framework.
    [[maybe_unused]] auto unused = cpu_device.release();
  }

  return ZX_OK;
}

void AmlCpu::DdkRelease() { delete this; }

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> AmlCpu::AddService() {
  auto result =
      outgoing_.AddService<fuchsia_cpuctrl::Service>(fuchsia_cpuctrl::Service::InstanceHandler({
          .device =
              [this](fidl::ServerEnd<fuchsia_cpuctrl::Device> server_end) {
                bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                     std::move(server_end), this, fidl::kIgnoreBindingClosure);
              },
      }));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to add CpuCtrl protocol: %s", result.status_string());
    return zx::error_result(result.take_error());
  }

  auto [directory_client, directory_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  result = outgoing_.Serve(std::move(directory_server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to service the outgoing directory");
    return zx::error_result(result.take_error());
  }

  return zx::ok(std::move(directory_client));
}

zx_status_t AmlCpu::SetCurrentOperatingPointInternal(uint32_t requested_opp, uint32_t* out_opp) {
  zx_status_t status;
  fuchsia_thermal::wire::OperatingPoint opps;
  std::scoped_lock lock(lock_);

  status = GetThermalOperatingPoints(&opps);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: Failed to get Thermal operating points, st = %d", __func__, status);
    return status;
  }

  // Opps in range [0, opps.count) are supported.
  if (requested_opp >= opps.count) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  const auto result =
      thermal_client_->SetDvfsOperatingPoint(static_cast<uint16_t>(opps.count - requested_opp - 1),
                                             static_cast<PowerDomain>(power_domain_index_));

  if (!result.ok() || result.value().status != ZX_OK) {
    zxlogf(ERROR, "%s: failed to set dvfs operating point.", __func__);
    return ZX_ERR_INTERNAL;
  }

  *out_opp = requested_opp;
  current_operating_point_ = requested_opp;

  return ZX_OK;
}

void AmlCpu::GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                                   GetOperatingPointInfoCompleter::Sync& completer) {
  // Get all operating points.
  zx_status_t status;
  fuchsia_thermal::wire::OperatingPoint opps;

  status = GetThermalOperatingPoints(&opps);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: Failed to get Thermal operating points, st = %d", __func__, status);
    completer.ReplyError(status);
  }

  // Make sure that the opp is in bounds?
  if (request->opp >= opps.count) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  const uint16_t index = static_cast<uint16_t>(opps.count - request->opp - 1);

  fuchsia_cpuctrl::wire::CpuOperatingPointInfo result;
  result.frequency_hz = opps.opp[index].freq_hz;
  result.voltage_uv = opps.opp[index].volt_uv;
  completer.ReplySuccess(result);
}

void AmlCpu::SetCurrentOperatingPoint(SetCurrentOperatingPointRequestView request,
                                      SetCurrentOperatingPointCompleter::Sync& completer) {
  uint32_t out_opp = 0;
  zx_status_t status = SetCurrentOperatingPointInternal(request->requested_opp, &out_opp);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  }
  completer.ReplySuccess(out_opp);
}

void AmlCpu::GetCurrentOperatingPoint(GetCurrentOperatingPointCompleter::Sync& completer) {
  uint32_t operating_point = 0;
  {
    std::scoped_lock lock(lock_);
    operating_point = current_operating_point_;
  }
  completer.Reply(operating_point);
}

void AmlCpu::GetOperatingPointCount(GetOperatingPointCountCompleter::Sync& completer) {
  zx_status_t status = ZX_ERR_INTERNAL;
  fuchsia_thermal::wire::OperatingPoint opps;
  {
    std::scoped_lock lock(lock_);
    status = GetThermalOperatingPoints(&opps);
  }

  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: Failed to get Thermal operating points, st = %d", __func__, status);
    completer.ReplyError(status);
  }

  completer.ReplySuccess(opps.count);
}

zx_status_t AmlCpu::GetThermalOperatingPoints(fuchsia_thermal::wire::OperatingPoint* out) {
  auto result = thermal_client_->GetDeviceInfo();
  if (!result.ok() || result.value().status != ZX_OK) {
    zxlogf(ERROR, "%s: Failed to get thermal device info", __func__);
    return ZX_ERR_INTERNAL;
  }

  fuchsia_thermal::wire::ThermalDeviceInfo* info = result.value().info.get();

  memcpy(out, &info->opps[power_domain_index_], sizeof(*out));
  return ZX_OK;
}

void AmlCpu::GetNumLogicalCores(GetNumLogicalCoresCompleter::Sync& completer) {
  completer.Reply(ClusterCoreCount());
}

void AmlCpu::GetLogicalCoreId(GetLogicalCoreIdRequestView request,
                              GetLogicalCoreIdCompleter::Sync& completer) {
  // Placeholder.
  completer.Reply(0);
}

void AmlCpu::GetDomainId(GetDomainIdCompleter::Sync& completer) {
  completer.Reply(PowerDomainIndex());
}

void AmlCpu::GetRelativePerformance(GetRelativePerformanceCompleter::Sync& completer) {
  completer.ReplySuccess(relative_performance_);
}

void AmlCpu::GetRelativePerformance2(GetRelativePerformance2Completer::Sync& completer) {
  completer.ReplySuccess(static_cast<uint64_t>(relative_performance_));
}

void AmlCpu::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_cpu_ctrl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown FIDL method ordinal 0x%016lx", metadata.method_ordinal);
}

void AmlCpu::SetCpuInfo(uint32_t cpu_version_packed) {
  const uint8_t major_revision = (cpu_version_packed >> 24) & 0xff;
  const uint8_t minor_revision = (cpu_version_packed >> 8) & 0xff;
  const uint8_t cpu_package_id = (cpu_version_packed >> 20) & 0x0f;
  zxlogf(INFO, "major revision number: 0x%x", major_revision);
  zxlogf(INFO, "minor revision number: 0x%x", minor_revision);
  zxlogf(INFO, "cpu package id number: 0x%x", cpu_package_id);

  cpu_info_.CreateUint("cpu_major_revision", major_revision, &inspector_);
  cpu_info_.CreateUint("cpu_minor_revision", minor_revision, &inspector_);
  cpu_info_.CreateUint("cpu_package_id", cpu_package_id, &inspector_);
}

}  // namespace amlogic_cpu

static constexpr zx_driver_ops_t aml_cpu_driver_ops = []() {
  zx_driver_ops_t result = {};
  result.version = DRIVER_OPS_VERSION;
  result.bind = amlogic_cpu::AmlCpu::Create;
  return result;
}();

// clang-format off
ZIRCON_DRIVER(aml_cpu, aml_cpu_driver_ops, "zircon", "0.1");
