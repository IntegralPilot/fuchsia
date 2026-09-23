// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include "src/lib/testing/predicates/status.h"

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_metadata::test {

class FakeDriver : public fdf::DriverBase2 {
 public:
  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<FakeDriver>::initialize,
                                          fdf_internal::DriverServer2<FakeDriver>::destroy);
  }

  FakeDriver() : fdf::DriverBase2("fake") {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    return zx::ok();
  }

  void Serve(const fuchsia_hardware_test::Metadata& metadata) {
    ASSERT_OK(metadata_server_.Serve(*outgoing(), dispatcher(), metadata));
  }

  void ForwardAndServe(bool expected_is_serving) {
    zx::result is_serving = metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming_);
    ASSERT_OK(is_serving);
    ASSERT_EQ(is_serving.value(), expected_is_serving);
  }

  void ForwardAndServeFromPDev(bool expected_is_serving) {
    zx::result pdev = incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>();
    ASSERT_OK(pdev);
    zx::result is_serving =
        metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev.value());
    ASSERT_OK(is_serving);
    ASSERT_EQ(is_serving.value(), expected_is_serving);
  }

  std::optional<fuchsia_driver_framework::Offer> CreateOffer() {
    return metadata_server_.CreateOffer();
  }

 private:
  fdf_metadata::MetadataServer<fuchsia_hardware_test::Metadata> metadata_server_;
  std::shared_ptr<fdf::Namespace> incoming_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  void InitPdev(const fuchsia_hardware_test::Metadata& metadata) {
    ASSERT_FALSE(pdev_.has_value());
    fdf_fake::FakePDev& pdev = pdev_.emplace();
    pdev.AddFidlMetadata(fuchsia_hardware_test::Metadata::kSerializableName, metadata);
  }

  void InitMetadataServer(fuchsia_hardware_test::Metadata metadata) {
    ASSERT_FALSE(metadata_.has_value());
    metadata_.emplace(std::move(metadata));
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    if (pdev_.has_value()) {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.value().GetInstanceHandler(dispatcher));
      if (result.is_error()) {
        return result.take_error();
      }
    }

    if (metadata_.has_value()) {
      zx::result result = metadata_server_.Serve(to_driver_vfs, dispatcher, metadata_.value());
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

 private:
  std::optional<fdf_fake::FakePDev> pdev_;
  fdf_metadata::MetadataServer<fuchsia_hardware_test::Metadata> metadata_server_;
  std::optional<fuchsia_hardware_test::Metadata> metadata_;
};

class FixtureConfig final {
 public:
  using DriverType = FakeDriver;
  using EnvironmentType = TestEnvironment;
};

class MetadataServerTest : public ::testing::Test {
 public:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void InitPdev(const fuchsia_hardware_test::Metadata& metadata) {
    driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) { env.InitPdev(metadata); });
  }

  void InitParentMetadataServer(fuchsia_hardware_test::Metadata metadata) {
    driver_test_.RunInEnvironmentTypeContext(
        [metadata = std::move(metadata)](TestEnvironment& env) {
          env.InitMetadataServer(std::move(metadata));
        });
  }

  void StartDriver() { ASSERT_OK(driver_test_.StartDriver()); }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

// Verify `MetadataServer::Serve()` serves metadata.
TEST_F(MetadataServerTest, ServeMetadata) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "test value"});

  StartDriver();
  driver_test().RunInDriverContext([](FakeDriver& driver) {
    driver.Serve(kMetadata);

    // Verify `MetadataServer::CreateOffer()` creates an offer.
    ASSERT_TRUE(driver.CreateOffer().has_value());
  });

  zx::result metadata = fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(
      driver_test().ConnectToDriverSvcDir());
  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value(), kMetadata);
}

// Verify `MetadataServer::ForwardAndServe()` can retrieve metadata from a metadata server found in
// its incoming namespace and serve it.
TEST_F(MetadataServerTest, ForwardMetadata) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "test value"});

  InitParentMetadataServer(kMetadata);
  StartDriver();
  driver_test().RunInDriverContext([](FakeDriver& driver) {
    driver.ForwardAndServe(true);

    // Verify `MetadataServer::CreateOffer()` creates an offer.
    ASSERT_TRUE(driver.CreateOffer().has_value());
  });

  zx::result metadata = fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(
      driver_test().ConnectToDriverSvcDir());
  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value(), kMetadata);
}

// Verify `MetadataServer::ForwardAndServe()` does not serve any metadata if it fails to retrieve
// metadata from a given platform device.
TEST_F(MetadataServerTest, ForwardNonExistentMetadata) {
  StartDriver();
  driver_test().RunInDriverContext([](FakeDriver& driver) {
    driver.ForwardAndServe(false);

    // Verify `MetadataServer::CreateOffer()` does not create an offer.
    ASSERT_FALSE(driver.CreateOffer().has_value());
  });

  zx::result metadata = fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(
      driver_test().ConnectToDriverSvcDir());
  ASSERT_OK(metadata);
  ASSERT_FALSE(metadata.value().has_value());
}

// Verify `MetadataServer::ForwardAndServe()` can retrieve metadata from a given platform device and
// serve it.
TEST_F(MetadataServerTest, ForwardPDevMetadata) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "test value"});

  InitPdev(kMetadata);
  StartDriver();
  driver_test().RunInDriverContext([](FakeDriver& driver) {
    driver.ForwardAndServeFromPDev(true);

    // Verify `MetadataServer::CreateOffer()` creates an offer.
    ASSERT_TRUE(driver.CreateOffer().has_value());
  });

  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_hardware_test::Metadata>(
      driver_test().ConnectToDriverSvcDir());
  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value(), kMetadata);
}

// Verify `MetadataServer::ForwardAndServe()` does not serve any metadata if it fails to retrieve
// metadata from a given platform device.
TEST_F(MetadataServerTest, ForwardNonExistentPDevMetadata) {
  StartDriver();
  driver_test().RunInDriverContext([](FakeDriver& driver) {
    driver.ForwardAndServeFromPDev(false);

    // Verify `MetadataServer::CreateOffer()` does not create an offer.
    ASSERT_FALSE(driver.CreateOffer().has_value());
  });

  zx::result metadata = fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(
      driver_test().ConnectToDriverSvcDir());
  ASSERT_OK(metadata);
  ASSERT_FALSE(metadata.value().has_value());
}

}  // namespace fdf_metadata::test

#endif
