// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_TEST_DEVICE_H_
#define SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_TEST_DEVICE_H_

#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <ddktl/device.h>

namespace ddk::test {

class MetadataTestDevice;
using MetadataTestDeviceType = ddk::Device<MetadataTestDevice>;

// This device's purpose is to allow multiple metadata_retriever and metadata_forwarder drivers to
// access the metadata_sender's metadata.
//
// The metadata_sender driver will create a MetadataTestDevice for every metadata_retriever or
// metadata_forwarder driver instance it wants to create. The MetadataTestDevice instance will
// create a child device that either a metadata_retriever or metadata_forwarder driver can bind to.
class MetadataTestDevice final : public MetadataTestDeviceType {
 public:
  static zx_status_t Create(zx_device_t* parent, const char* device_name,
                            const char* device_purpose,
                            std::optional<fidl::ClientEnd<fuchsia_io::Directory>> outgoing);

  explicit MetadataTestDevice(zx_device_t* parent) : MetadataTestDeviceType(parent) {}

  zx_status_t Init(const char* device_name, const char* device_purpose,
                   std::optional<fidl::ClientEnd<fuchsia_io::Directory>> outgoing);

  // ddk::Device mixin
  void DdkRelease();
};

}  // namespace ddk::test

#endif  // SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_TEST_DEVICE_H_
