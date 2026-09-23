// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_DEVICE_PARTITIONER_H_
#define SRC_FIRMWARE_PAVER_DEVICE_PARTITIONER_H_

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <stdbool.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <span>
#include <string_view>
#include <utility>
#include <vector>

#include <fbl/string.h>
#include <fbl/unique_fd.h>

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/config.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/paver-context.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

// Whether the device uses the new or legacy partition scheme.
enum class PartitionScheme { kNew, kLegacy };

enum class Partition {
  kUnknown,
  kBootloaderA,
  kBootloaderB,
  kBootloaderR,
  kZirconA,
  kZirconB,
  kZirconR,
  kSysconfig,
  kVbMetaA,
  kVbMetaB,
  kVbMetaR,
  kAbrMeta,
  kFuchsiaVolumeManager,
};

const char* PartitionName(Partition partition, PartitionScheme scheme);
std::optional<uuid::Uuid> PartitionTypeGuid(Partition partition, PartitionScheme scheme);

constexpr char kOpaqueVolumeContentType[] = "opaque_volume";

// Operations on a specific partition take two identifiers, a partition type
// and a content type.
//
// The first is the conceptual partition type. This may not necessarily map 1:1
// with on-disk partitions. For example, some devices have multiple bootloader
// stages which live in different partitions on-disk but which would both have
// type kBootloader.
//
// The second is a device-specific string identifying the contents the caller
// wants to read/write. The backend uses this to decide which on-disk partitions
// to use and where the content lives in them. The default content type is
// null or empty.
struct PartitionSpec {
 public:
  // Creates a spec with the given partition and default (null) content type.
  explicit constexpr PartitionSpec(Partition partition)
      : PartitionSpec(partition, std::string_view()) {}

  // Creates a spec with the given partition and content type.
  constexpr PartitionSpec(Partition partition, std::string_view content_type)
      : partition(partition), content_type(content_type) {}

  // Returns a descriptive string for logging.
  //
  // Does not necessary match the on-disk partition name, just meant to
  // indicate the conceptual partition type in a device-agnostic way.
  fbl::String ToString() const;

  Partition partition;
  std::string_view content_type;
};

inline bool SpecMatches(const PartitionSpec& a, const PartitionSpec& b) {
  return a.partition == b.partition && a.content_type == b.content_type;
}

// Abstract device partitioner definition.
// This class defines common APIs for interacting with a device partitioner.
class DevicePartitioner {
 public:
  virtual ~DevicePartitioner() = default;

  // Creates the ABR client for the partitioner.
  // NOTE: The ABR client must not outlive the partitioner.
  virtual zx::result<std::unique_ptr<abr::Client>> CreateAbrClient() const = 0;

  // Returns an accessor for the block devices managed by the partitioner.
  virtual const paver::BlockDevices& Devices() const = 0;

  // Returns the service root for the partitioner.
  virtual fidl::UnownedClientEnd<fuchsia_io::Directory> SvcRoot() const = 0;

  // Checks if the device supports the given partition spec.
  //
  // This is the only function that will definitively say whether a spec is
  // supported or not. Other partition functions may return ZX_ERR_NOT_SUPPORTED
  // on unsupported spec, but they may also return it for other reasons such as
  // a lower-level driver error. They also may return other errors even if given
  // an unsupported spec if something else goes wong.
  virtual bool SupportsPartition(const PartitionSpec& spec) const = 0;

  // Returns a PartitionClient matching |spec| if one exists.
  virtual zx::result<std::unique_ptr<PartitionClient>> FindPartition(
      const PartitionSpec& spec) const = 0;

  // Reset partition tables to a board-specific initial state.
  // This is only supported on some boards; in general it is preferred to use fastboot to
  // reinitialize partition tables.
  virtual zx::result<> ResetPartitionTables() const = 0;

  // Determine if the given data file is a valid image for this device.
  //
  // This analysis is best-effort only, providing only basic safety checks.
  virtual zx::result<> ValidatePayload(const PartitionSpec& spec,
                                       std::span<const uint8_t> data) const = 0;

  // Flush all buffered write to persistant storage.
  virtual zx::result<> Flush() const = 0;

  // Called by paver when Lifetime::Stop event is received
  // Can be used to update ABR oneshot flags for reboot.
  virtual zx::result<> OnStop() const = 0;
};

class DevicePartitionerFactory {
 public:
  // Factory method which automatically returns the correct DevicePartitioner implementation based
  // factories registered with it.
  static zx::result<std::unique_ptr<DevicePartitioner>> Create(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context);

  static void Register(std::unique_ptr<DevicePartitionerFactory> factory);

  virtual ~DevicePartitionerFactory() = default;

 private:
  // This method is overridden by derived classes that implement different kinds
  // of DevicePartitioners.
  virtual zx::result<std::unique_ptr<DevicePartitioner>> New(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) = 0;

  static std::vector<std::unique_ptr<DevicePartitionerFactory>>* registered_factory_list();
};

// DevicePartitioner implementation for devices which have fixed partition maps (e.g. ARM
// devices). It will not attempt to write a partition map of any kind to the device.
// Assumes legacy partition layout structure (e.g. ZIRCON-A, ZIRCON-B,
// ZIRCON-R).
class FixedDevicePartitioner : public DevicePartitioner {
 public:
  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const BlockDevices& devices, fidl::ClientEnd<fuchsia_io::Directory> svc_root);

  const BlockDevices& Devices() const override { return devices_; }

  fidl::UnownedClientEnd<fuchsia_io::Directory> SvcRoot() const override {
    return svc_root_.borrow();
  }

  bool SupportsPartition(const PartitionSpec& spec) const override;

  zx::result<std::unique_ptr<PartitionClient>> FindPartition(
      const PartitionSpec& spec) const override;

  zx::result<> ResetPartitionTables() const override;

  zx::result<> ValidatePayload(const PartitionSpec& spec,
                               std::span<const uint8_t> data) const override;

  zx::result<> Flush() const override { return zx::ok(); }
  zx::result<> OnStop() const override { return zx::ok(); }

 protected:
  zx::result<std::unique_ptr<abr::Client>> CreateAbrClient() const override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  FixedDevicePartitioner(BlockDevices devices, fidl::ClientEnd<fuchsia_io::Directory> svc_root)
      : devices_(std::move(devices)), svc_root_(std::move(svc_root)) {}

  BlockDevices devices_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
};

class DefaultPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_DEVICE_PARTITIONER_H_
