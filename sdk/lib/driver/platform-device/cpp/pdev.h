// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_PLATFORM_DEVICE_CPP_PDEV_H_
#define LIB_DRIVER_PLATFORM_DEVICE_CPP_PDEV_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/power/cpp/power-support.h>
#include <lib/driver/power/cpp/types.h>
#include <lib/fidl/cpp/natural_types.h>
#include <lib/zx/bti.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <string>
#include <type_traits>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf {

// A helper class that wraps the `fuchsia.hardware.platform.device/Device` FIDL calls.
// This class exists to make it simpler for clients to move onto the platform device FIDL.
class PDev {
 public:
  static constexpr char kFragmentName[] = "pdev";

  PDev() = default;
  explicit PDev(fidl::ClientEnd<fuchsia_hardware_platform_device::Device> client);

  // Connect to the platform device in the given namespace.
  //
  // If the fragment_name is not provided, it will be set to the default value of `pdev`.
  static zx::result<PDev> Connect(const std::shared_ptr<Namespace>& incoming,
                                  std::string_view instance = kFragmentName);

  fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> borrow();

  zx::result<fdf::MmioBuffer> MapMmio(
      uint32_t index, uint32_t cache_policy = ZX_CACHE_POLICY_UNCACHED_DEVICE) const;
  zx::result<fdf::MmioBuffer> MapMmio(
      std::string_view name, uint32_t cache_policy = ZX_CACHE_POLICY_UNCACHED_DEVICE) const;

  // Retrieves metadata from the platform device associated with the id |metadata_id| and unpersists
  // it into |FidlType|. |FidlType| must be persistable. Assumes that the metadata from the platform
  // device is a persisted |FidlType|.
  template <typename FidlType>
#if __cplusplus >= 202002l
    requires(fidl::IsFidlTypeV<FidlType> && !fidl::IsResourceV<FidlType>)
#endif
  zx::result<FidlType> GetFidlMetadata(
      std::string_view metadata_id = FidlType::kSerializableName) const {
    fidl::WireResult<fuchsia_hardware_platform_device::Device::GetMetadata> persisted_metadata =
        pdev_->GetMetadata(fidl::StringView::FromExternal(metadata_id));
    if (!persisted_metadata.ok()) {
      return zx::error(persisted_metadata.status());
    }
    if (persisted_metadata->is_error()) {
      return persisted_metadata->take_error();
    }

    auto metadata = fidl::Unpersist<FidlType>(persisted_metadata.value()->metadata.get());
    if (metadata.is_error()) {
      return zx::error(metadata.error_value().status());
    }

    return zx::ok(metadata.value());
  }

  struct MmioInfo {
    zx_off_t offset;
    size_t size;
    zx::vmo vmo;
  };
  zx::result<MmioInfo> GetMmio(uint32_t index) const;
  zx::result<MmioInfo> GetMmio(std::string_view name) const;

  zx::result<zx::interrupt> GetInterrupt(uint32_t index, uint32_t flags = 0) const;
  zx::result<zx::interrupt> GetInterrupt(std::string_view name, uint32_t flags = 0) const;
  zx::result<zx::bti> GetBti(uint32_t index) const;
  zx::result<zx::resource> GetSmc(uint32_t index) const;

  struct DeviceInfo {
    uint32_t vid;
    uint32_t pid;
    uint32_t did;
    uint32_t mmio_count;
    uint32_t irq_count;
    uint32_t bti_count;
    uint32_t smc_count;
    uint32_t metadata_count;
    std::string name;
  };
  zx::result<DeviceInfo> GetDeviceInfo() const;

  struct BoardInfo {
    uint32_t vid;
    uint32_t pid;
    std::string board_name;
    uint32_t board_revision;
  };
  zx::result<BoardInfo> GetBoardInfo() const;

  zx::result<std::vector<fdf_power::PowerElementConfiguration>> GetPowerConfiguration();

  /// Uses the provided namespace and platform device instance to get a power
  /// configuration, add corresponding power elements to the power topology, and
  /// return `fdf_power::ElementDesc` objects equivalent to the power configuration.
  ///
  /// This function retrieves the config via |dev| and then calls
  /// `fdf_power::ApplyPowerConfiguration`, see its documentation for additional
  /// information.
  fit::result<fdf_power::Error, std::vector<fdf_power::ElementDesc>> GetAndApplyPowerConfiguration(
      const fdf::Namespace& ns, bool use_element_runner = true);

  bool is_valid() const { return pdev_.is_valid(); }

 private:
  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev_;
};

namespace internal {

// This helper is marked weak because it is sometimes necessary to provide a
// test-only version that allows for MMIO fakes or mocks to reach the driver under test.
// For example say you have a fake Protocol device that needs to smuggle a fake MMIO:
//
//  class FakePDev {
//    .....
//    zx::result<MmioInfo> PDevGetMmio(uint32_t index) {
//      MmioInfo out_mmio = {};
//      out_mmio.offset = reinterpret_cast<size_t>(this);
//      return zx::ok(out_mmio);
//    }
//    .....
//  };
//
// The actual implementation expects a real {size, offset, VMO} and therefore it will
// fail. But works if a replacement PDevMakeMmioBufferWeak in the test is provided:
//
//  zx::result<fdf::MmioBuffer> PDevMakeMmioBufferWeak(const MmioInfo& pdev_mmio) {
//    auto* test_harness = reinterpret_cast<FakePDev*>(pdev_mmio.offset);
//    return zx::ok(test_harness->fake_mmio());
//  }
//
// Note that if you are using `//sdk/lib/driver/fake-platform-device/cpp`, it provides an
// implementation of this functionality for you. Drivers must set the `mmios` field in
// `FakePDev::Config`.
zx::result<fdf::MmioBuffer> PDevMakeMmioBufferWeak(PDev::MmioInfo& pdev_mmio,
                                                   uint32_t cache_policy);
}  // namespace internal
}  // namespace fdf

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

#endif  // LIB_DRIVER_PLATFORM_DEVICE_CPP_PDEV_H_
