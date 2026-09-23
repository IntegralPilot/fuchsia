// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DEVICE_H_
#define SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DEVICE_H_

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <ddktl/device.h>
#include <ddktl/metadata_server.h>

namespace ddk::test {

class MetadataSenderTestDevice;
using MetadataSenderTestDeviceType =
    ddk::Device<MetadataSenderTestDevice,
                ddk::Messageable<fuchsia_hardware_test::MetadataSender>::Mixin>;

// This driver's purpose is to serve metadata to its children using `ddk::MetadataServer`.
class MetadataSenderTestDevice final : public MetadataSenderTestDeviceType {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent, const char* name, zx_handle_t handle);

  explicit MetadataSenderTestDevice(zx_device_t* parent) : MetadataSenderTestDeviceType(parent) {}

  // ddk::Device mixin
  void DdkRelease();

  // fuchsia.hardware.test/MetadataSender implementation.
  void SetMetadata(SetMetadataRequestView request, SetMetadataCompleter::Sync& completer) override;
  void AddMetadataRetrieverDevice(AddMetadataRetrieverDeviceRequestView request,
                                  AddMetadataRetrieverDeviceCompleter::Sync& completer) override;
  void AddMetadataForwarderDevice(AddMetadataForwarderDeviceCompleter::Sync& completer) override;

  zx_status_t Init();

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> ServeOutgoing();

 private:
  zx::result<std::string> AddMetadataDevice(const char* device_purpose, bool expose_metadata);

  async_dispatcher_t* dispatcher_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};
  component::OutgoingDirectory outgoing_{dispatcher_};
  ddk::MetadataServer<fuchsia_hardware_test::wire::Metadata> metadata_server_;
  size_t num_metadata_devices_ = 0;
};

}  // namespace ddk::test

#endif  // SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DEVICE_H_
