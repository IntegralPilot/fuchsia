// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_I2C_I2C_TEST_ENV_H_
#define SRC_DEVICES_I2C_DRIVERS_I2C_I2C_TEST_ENV_H_

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/wire_test_base.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <format>

#include "src/devices/i2c/drivers/i2c/fake-i2c-impl.h"
#include "src/devices/i2c/drivers/i2c/i2c.h"
#include "src/lib/testing/predicates/status.h"

namespace i2c {

class GenericMetadataServer final : public fidl::WireServer<fuchsia_driver_metadata::Metadata> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher,
                     std::string service_name,
                     const fuchsia_driver_metadata::Dictionary& metadata) {
    auto persisted = fidl::Persist(metadata);
    if (persisted.is_error()) {
      return zx::error(persisted.error_value().status());
    }
    metadata_bytes_ = std::move(persisted.value());

    fuchsia_driver_metadata::Service::InstanceHandler handler({
        .metadata = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });

    return outgoing.component().AddService(std::move(handler), std::move(service_name));
  }

 private:
  void GetPersistedMetadata(GetPersistedMetadataCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(metadata_bytes_));
  }

  std::vector<uint8_t> metadata_bytes_;
  fidl::ServerBindingGroup<fuchsia_driver_metadata::Metadata> bindings_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  TestEnvironment() : i2c_impl_(1024) {}

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    if (i2c_metadata_.has_value()) {
      if (zx::result result = metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              i2c_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (generic_metadata_.has_value()) {
      if (zx::result result = generic_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              "fuchsia.hardware.i2c.businfo.I2CBusMetadata", generic_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    // Add the i2c service.
    if (zx::result result = to_driver_vfs.AddService<fuchsia_hardware_i2cimpl::Service>(
            i2c_impl_.CreateInstanceHandler());
        result.is_error()) {
      return result.take_error();
    }
    return zx::ok();
  }

  void AddMetadata(fuchsia_hardware_i2c_businfo::I2CBusMetadata metadata) {
    i2c_metadata_.emplace(std::move(metadata));
  }

  void InitGeneric(const fuchsia_hardware_i2c_businfo::I2CBusMetadata& metadata) {
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    if (metadata.bus_id().has_value()) {
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          "provider", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                          static_cast<int64_t>(metadata.bus_id().value()))));
    }
    if (metadata.channels().has_value()) {
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          "channels._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                 static_cast<int64_t>(metadata.channels()->size()))));
      for (size_t i = 0; i < metadata.channels()->size(); ++i) {
        const auto& c = metadata.channels()->at(i);
        if (c.address().has_value()) {
          entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
              std::format("channels.{}.address", i),
              fuchsia_driver_metadata::DictionaryValue::WithInt64(
                  static_cast<int64_t>(c.address().value()))));
        }
        if (c.global_id().has_value()) {
          entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
              std::format("channels.{}.id", i), fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                                    static_cast<int64_t>(c.global_id().value()))));
        }
        if (c.name().has_value()) {
          entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
              std::format("channels.{}.name", i),
              fuchsia_driver_metadata::DictionaryValue::WithStr(c.name().value())));
        }
      }
    }
    generic_metadata_ = fuchsia_driver_metadata::Dictionary{{.entries = std::move(entries)}};
  }

  FakeI2cImpl& i2c_impl() { return i2c_impl_; }

 private:
  fdf_metadata::MetadataServer<fuchsia_hardware_i2c_businfo::I2CBusMetadata> metadata_server_;
  FakeI2cImpl i2c_impl_;
  std::optional<fuchsia_hardware_i2c_businfo::I2CBusMetadata> i2c_metadata_;
  GenericMetadataServer generic_metadata_server_;
  std::optional<fuchsia_driver_metadata::Dictionary> generic_metadata_;
};

class TestConfig final {
 public:
  using DriverType = I2cDriver;
  using EnvironmentType = TestEnvironment;
};

}  // namespace i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_I2C_I2C_TEST_ENV_H_
