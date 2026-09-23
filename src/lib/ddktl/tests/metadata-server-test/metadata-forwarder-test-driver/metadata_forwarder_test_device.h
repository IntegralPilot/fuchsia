// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_FORWARDER_TEST_DRIVER_METADATA_FORWARDER_TEST_DEVICE_H_
#define SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_FORWARDER_TEST_DRIVER_METADATA_FORWARDER_TEST_DEVICE_H_

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <ddktl/device.h>
#include <ddktl/metadata_server.h>

namespace ddk::test {

class MetadataForwarderTestDevice;
using MetadataForwarderTestDeviceType =
    ddk::Device<MetadataForwarderTestDevice,
                ddk::Messageable<fuchsia_hardware_test::MetadataForwarder>::Mixin>;

// This driver's purpose is to forward metadata from its parent driver
// (//src/lib/ddktl/tests/metadata-server-test/metadata-sender-test-driver:driver) to its children
// (//src/lib/ddktl/tests/metadata-server-test/metadata-retriever-test-driver:driver) using
// `ddk::MetadataServer`.
class MetadataForwarderTestDevice final : public MetadataForwarderTestDeviceType {
 public:
  static zx_status_t Bind(void* ctx, zx_device_t* dev);

  explicit MetadataForwarderTestDevice(zx_device_t* parent)
      : MetadataForwarderTestDeviceType(parent) {}

  // ddk::Device mixin
  void DdkRelease();

  // fuchsia.hardware.test/MetadataForwarder implementation.
  void ForwardMetadata(ForwardMetadataCompleter::Sync& completer) override;

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> InitOutgoing();

 private:
  async_dispatcher_t* dispatcher_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};
  component::OutgoingDirectory outgoing_{dispatcher_};
  ddk::MetadataServer<fuchsia_hardware_test::wire::Metadata> metadata_server_;
};

}  // namespace ddk::test

#endif  // SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_FORWARDER_TEST_DRIVER_METADATA_FORWARDER_TEST_DEVICE_H_
