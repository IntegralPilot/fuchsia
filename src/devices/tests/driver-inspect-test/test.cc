// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device.inspect.test/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/async/cpp/executor.h>
#include <lib/ddk/device.h>
#include <lib/ddk/platform-defs.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fpromise/promise.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <string>

#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

namespace {
using driver_integration_test::IsolatedDevmgr;
using fuchsia_device_inspect_test::TestInspect;

class InspectTestCase : public gtest::RealLoopFixture {
 public:
  ~InspectTestCase() override = default;

  void SetUp() override {
    IsolatedDevmgr::Args args;
    board_test::DeviceEntry dev = {};
    strlcpy(dev.name, kPlatformDeviceName, sizeof(dev.name));
    dev.vid = PDEV_VID_TEST;
    dev.pid = PDEV_PID_INSPECT_TEST;
    dev.did = 0;
    args.device_list.push_back(dev);

    ASSERT_EQ(ZX_OK, IsolatedDevmgr::Create(&args, &devmgr_));

    static char path_buf[128];
    snprintf(path_buf, sizeof(path_buf), "sys/platform/%s/inspect-test", kPlatformDeviceName);
    zx::result channel = device_watcher::RecursiveWaitForFile(devmgr_.devfs_root().get(), path_buf);
    ASSERT_EQ(ZX_OK, channel.status_value());
    client_ = fidl::ClientEnd<TestInspect>{std::move(channel.value())};
  }

  const IsolatedDevmgr& devmgr() { return devmgr_; }
  const fidl::ClientEnd<TestInspect>& client() { return client_; }

  fpromise::promise<inspect::Hierarchy> ReadInspect() {
    using diagnostics::reader::ArchiveReader;
    using diagnostics::reader::SanitizeMonikerForSelectors;

    std::string moniker;
    {
      std::ostringstream builder;
      builder << "realm_builder:" << devmgr_.RealmChildName()
              << "/driver_test_realm/boot-drivers:" << kPlatformDeviceName;
      moniker = builder.str();
    }

    return fpromise::make_ok_promise(std::unique_ptr<ArchiveReader>(new ArchiveReader(
                                         dispatcher(), {SanitizeMonikerForSelectors(moniker) +
                                                        ":[name=inspect-test]root"})))
        .and_then([moniker = std::move(moniker)](std::unique_ptr<ArchiveReader>& reader) {
          return reader->SnapshotInspectUntilPresent({moniker}).then(
              [](fpromise::result<std::vector<diagnostics::reader::InspectData>, std::string>&
                     wrapped) {
                EXPECT_TRUE(wrapped.is_ok()) << wrapped.take_error();
                auto data = wrapped.take_value();
                EXPECT_EQ(1ul, data.size());
                auto& inspect_data = data[0];
                if (!inspect_data.payload().has_value()) {
                  FX_LOGS(WARNING) << "inspect_data had nullopt payload";
                }
                if (inspect_data.payload().has_value() &&
                    inspect_data.payload().value() == nullptr) {
                  FX_LOGS(WARNING) << "inspect_data had nullptr for payload";
                }
                if (inspect_data.metadata().errors.has_value()) {
                  for (const auto& e : inspect_data.metadata().errors.value()) {
                    FX_LOGS(WARNING) << e.message;
                  }
                }

                return fpromise::ok(inspect_data.TakePayload());
              });
        });
  }

  // Returns whether or not the property value matches the input.
  bool CheckStringProperty(const inspect::NodeValue& node, const std::string& property_name,
                           const std::string& expected_value) {
    const auto* actual_value = node.get_property<inspect::StringPropertyValue>(property_name);
    if (actual_value == nullptr) {
      return false;
    }
    return expected_value == actual_value->value();
  }

 private:
  static constexpr char kPlatformDeviceName[board_test::kNameLengthMax] = "test-platform-device";

  IsolatedDevmgr devmgr_;
  fidl::ClientEnd<TestInspect> client_;
};

TEST_F(InspectTestCase, ReadInspectData) {
  // Use a new connection rather than using devfs_root because devfs_root has the empty set of
  // rights.
  {
    // Check initial inspect data
    std::optional<inspect::Hierarchy> hierarchy = std::nullopt;
    async::Executor exec(dispatcher());
    exec.schedule_task(
        ReadInspect().and_then([&](inspect::Hierarchy& h) { hierarchy.emplace(std::move(h)); }));
    RunLoopUntil([&] { return hierarchy.has_value(); });

    // testBeforeDdkAdd: "OK"
    ASSERT_TRUE(CheckStringProperty(hierarchy->node(), "testBeforeDdkAdd", "OK"));
  }

  // Call test-driver to modify inspect data
  const fidl::WireResult result = fidl::WireCall(client())->ModifyInspect();
  ASSERT_TRUE(result.ok());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok()) << zx_status_get_string(response.error_value());

  // Verify new inspect data is reflected
  {
    std::optional<inspect::Hierarchy> hierarchy = std::nullopt;
    async::Executor exec(dispatcher());
    exec.schedule_task(
        ReadInspect().and_then([&](inspect::Hierarchy& h) { hierarchy.emplace(std::move(h)); }));
    RunLoopUntil([&] { return hierarchy.has_value(); });

    // Previous values
    ASSERT_TRUE(CheckStringProperty(hierarchy->node(), "testBeforeDdkAdd", "OK"));
    // New addition - testModify: "OK"
    ASSERT_TRUE(CheckStringProperty(hierarchy->node(), "testModify", "OK"));
  }
}

}  // namespace
