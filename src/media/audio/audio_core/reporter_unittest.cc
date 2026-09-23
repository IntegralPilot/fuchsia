// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/audio_core/reporter.h"

#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/testing/component_context_provider.h>

#include <optional>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/audio_core/audio_admin.h"

namespace media::audio {
namespace {

using fuchsia::media::AudioRenderUsage2;
using ::inspect::testing::BoolIs;
using ::inspect::testing::ChildrenMatch;
using ::inspect::testing::DoubleIs;
using ::inspect::testing::IntIs;
using ::inspect::testing::NameMatches;
using ::inspect::testing::NodeMatches;
using ::inspect::testing::PropertyList;
using ::inspect::testing::StringIs;
using ::inspect::testing::UintIs;
using ::testing::AllOf;
using ::testing::IsEmpty;
using ::testing::IsSupersetOf;

::testing::Matcher<const ::inspect::Hierarchy&> NodeAlive(const std::string& name) {
  return NodeMatches(AllOf(NameMatches(name),
                           PropertyList(Contains(UintIs(std::string(kTimeSinceDeathNsec), 0)))));
}

::testing::Matcher<const ::inspect::Hierarchy&> NodeDead(const std::string& name) {
  return NodeMatches(AllOf(
      NameMatches(name), Not(PropertyList(Contains(UintIs(std::string(kTimeSinceDeathNsec), 0))))));
}

class ReporterTest : public gtest::TestLoopFixture {
 public:
  ReporterTest()
      : under_test_(*component_context_provider_.context(), dispatcher(), dispatcher(), false) {}

  inspect::Hierarchy GetHierarchy() {
    zx::vmo duplicate = under_test_.inspector().DuplicateVmo();
    if (duplicate.get() == ZX_HANDLE_INVALID) {
      return inspect::Hierarchy();
    }

    auto ret = inspect::ReadFromVmo(duplicate);
    EXPECT_TRUE(ret.is_ok());
    if (ret.is_ok()) {
      return ret.take_value();
    }

    return inspect::Hierarchy();
  }

  inspect::Hierarchy GetHierarchyLazyValues() {
    fpromise::result<inspect::Hierarchy> result;
    fpromise::single_threaded_executor exec;
    exec.schedule_task(
        inspect::ReadFromInspector(under_test_.inspector())
            .then([&](fpromise::result<inspect::Hierarchy>& res) { result = std::move(res); }));
    exec.run();
    EXPECT_TRUE(result.is_ok());
    return result.take_value();
  }

  sys::testing::ComponentContextProvider component_context_provider_;
  Reporter under_test_;
};

// Test reporter initial state.
TEST_F(ReporterTest, InitialState) {
  auto hierarchy = GetHierarchy();

  // Expect metrics with default values in the root node.
  EXPECT_THAT(hierarchy,
              NodeMatches(AllOf(NameMatches("root"),
                                PropertyList(IsSupersetOf({
                                    UintIs(std::string(kConnectToDeviceFailureCount), 0),
                                    UintIs(std::string(kObtainDeviceStreamChannelFailureCount), 0),
                                    UintIs(std::string(kStartDeviceFailureCount), 0),
                                    UintIs(std::string(kApplySchedulerProfileFailureCount), 0),
                                    UintIs(std::string(kApplyMemoryProfileFailureCount), 0),
                                })))));

  // Expect empty child nodes for devices and client ports.
  EXPECT_THAT(
      hierarchy,
      ChildrenMatch(UnorderedElementsAre(
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kOutputDevices)), PropertyList(IsEmpty()),
                                  PropertyList(IsEmpty()))),
                ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kInputDevices)), PropertyList(IsEmpty()),
                                  PropertyList(IsEmpty()))),
                ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kRenderers)), PropertyList(IsEmpty()),
                                  PropertyList(IsEmpty()))),
                ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kCapturers)), PropertyList(IsEmpty()),
                                  PropertyList(IsEmpty()))),
                ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kThermalState)),
                                  PropertyList(UnorderedElementsAre(
                                      UintIs(std::string(kThermalStateCount), 1))))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                    NameMatches(kNormal),
                    Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kThermalStateTransitions))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(IsSupersetOf({
                              BoolIs(std::string(kActive), true),
                              StringIs(std::string(kState), kNormal),
                          })),
                          Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0))))))))),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kVolumeControls)),
                                  PropertyList(IsEmpty()), PropertyList(IsEmpty()))),
                ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(
                    NameMatches(std::string(kActiveUsagePolicies)),
                    PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                      DoubleIs(std::string(kDuckGainDb), 0.0),
                                                      DoubleIs(std::string(kMuteGainDb), 0.0))))),
                ChildrenMatch(Contains(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(Contains(BoolIs(std::string(kActive), true)))))))))));
}

// Test methods that update metrics in the root node.
TEST_F(ReporterTest, RootMetrics) {
  under_test_.FailedToConnectToDevice("", false, 0);
  under_test_.FailedToObtainStreamChannel("", false, 0);
  under_test_.FailedToObtainStreamChannel("", false, 0);

  under_test_.FailedToStartDevice("");
  under_test_.FailedToStartDevice("");
  under_test_.FailedToStartDevice("");

  under_test_.FailedToApplySchedulerProfile("unused profile name", /* unused error status */ 0);
  under_test_.FailedToApplySchedulerProfile("unused profile name", /* unused error status */ 0);
  under_test_.FailedToApplyMemoryProfile("unused profile name", /* unused error status */ 0);

  EXPECT_THAT(GetHierarchy(),
              NodeMatches(AllOf(NameMatches("root"),
                                PropertyList(IsSupersetOf({
                                    UintIs(std::string(kConnectToDeviceFailureCount), 1),
                                    UintIs(std::string(kObtainDeviceStreamChannelFailureCount), 2u),
                                    UintIs(std::string(kStartDeviceFailureCount), 3u),
                                    UintIs(std::string(kApplySchedulerProfileFailureCount), 2u),
                                    UintIs(std::string(kApplyMemoryProfileFailureCount), 1u),
                                })))));
}

// Test methods that add and remove devices.
TEST_F(ReporterTest, AddRemoveDevices) {
  std::vector<Reporter::Container<Reporter::OutputDevice, Reporter::kObjectsToCache>::Ptr> outputs;
  std::vector<Reporter::Container<Reporter::InputDevice, Reporter::kObjectsToCache>::Ptr> inputs;
  outputs.reserve(5);
  for (size_t k = 0; k < 5; k++) {
    outputs.push_back(under_test_.CreateOutputDevice(fxl::StringPrintf("output_device_%lu", k),
                                                     fxl::StringPrintf("output_thread_%lu", k)));
  }
  inputs.reserve(5);
  for (size_t k = 0; k < 5; k++) {
    inputs.push_back(under_test_.CreateInputDevice(fxl::StringPrintf("input_device_%lu", k),
                                                   fxl::StringPrintf("input_thread_%lu", k)));
  }

  EXPECT_THAT(GetHierarchyLazyValues(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeAlive("output_device_0"), NodeAlive("output_device_1"),
                            NodeAlive("output_device_2"), NodeAlive("output_device_3"),
                            NodeAlive("output_device_4")))),
                  AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeAlive("input_device_0"), NodeAlive("input_device_1"),
                            NodeAlive("input_device_2"), NodeAlive("input_device_3"),
                            NodeAlive("input_device_4")))),
              })));

  outputs[0].Drop();
  outputs[1].Drop();
  outputs[2].Drop();
  outputs[3].Drop();
  inputs[0].Drop();
  inputs[1].Drop();
  inputs[2].Drop();
  inputs[3].Drop();

  EXPECT_THAT(GetHierarchyLazyValues(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeDead("output_device_0"), NodeDead("output_device_1"),
                            NodeDead("output_device_2"), NodeDead("output_device_3"),
                            NodeAlive("output_device_4")))),
                  AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeDead("input_device_0"), NodeDead("input_device_1"),
                            NodeDead("input_device_2"), NodeDead("input_device_3"),
                            NodeAlive("input_device_4")))),
              })));

  outputs[4].Drop();
  inputs[4].Drop();

  // Garbage collect [0].
  EXPECT_THAT(GetHierarchyLazyValues(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeDead("output_device_1"), NodeDead("output_device_2"),
                            NodeDead("output_device_3"), NodeDead("output_device_4")))),
                  AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeDead("input_device_1"), NodeDead("input_device_2"),
                            NodeDead("input_device_3"), NodeDead("input_device_4")))),
              })));
}

// Test methods that change device metrics.
TEST_F(ReporterTest, DeviceMetrics) {
  constexpr std::string kTestOutputDevice = "output_device";
  constexpr std::string kTestInputDevice = "input_device";
  constexpr std::string kTestOutputThread = "output_thread";
  constexpr std::string kTestInputThread = "input_thread";

  auto output_device = under_test_.CreateOutputDevice(kTestOutputDevice, kTestOutputThread);
  auto input_device = under_test_.CreateInputDevice(kTestInputDevice, kTestInputThread);

  // Note: GetHierachy uses ReadFromVmo, which cannot read lazy values.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(UnorderedElementsAre(
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(AllOf(NameMatches(kTestOutputDevice),
                                      PropertyList(UnorderedElementsAre(StringIs(
                                          std::string(kMixerThreadName), kTestOutputThread))))),
                    ChildrenMatch(UnorderedElementsAre(
                        NodeMatches(AllOf(NameMatches(std::string(kDriver)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kInitialInternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentInternalDelayNsec), 0),
                                              IntIs(std::string(kInternalDelayChangedAt), 0),
                                              UintIs(std::string(kInitialExternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentExternalDelayNsec), 0),
                                              IntIs(std::string(kExternalDelayChangedAt), 0),
                                              UintIs(std::string(kDriverTransferBytes), 0),
                                              StringIs(std::string(kName), kUnknown))))),
                        NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                          PropertyList(UnorderedElementsAre(
                                              StringIs(std::string(kSampleFormat), kUnknown),
                                              UintIs(std::string(kChannels), 0),
                                              UintIs(std::string(kFramesPerSecond), 0))))),
                        NodeMatches(AllOf(NameMatches(std::string(kDeviceGain)),
                                          PropertyList(UnorderedElementsAre(
                                              DoubleIs(std::string(kGainDb), 0.0),
                                              BoolIs(std::string(kMuted), false),
                                              BoolIs(std::string(kAgcSupported), false),
                                              BoolIs(std::string(kAgcEnabled), false))))),
                        NodeMatches(AllOf(NameMatches(std::string(kDeviceUnderflows)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kCount), 0),
                                              UintIs(std::string(kDurationNsec), 0),
                                              UintIs(std::string(kSessionCount), 0))))),
                        NodeMatches(AllOf(NameMatches(std::string(kPipelineUnderflows)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kCount), 0),
                                              UintIs(std::string(kDurationNsec), 0),
                                              UintIs(std::string(kSessionCount), 0))))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(AllOf(NameMatches(kTestInputDevice),
                                      PropertyList(UnorderedElementsAre(StringIs(
                                          std::string(kMixerThreadName), kTestInputThread))))),
                    ChildrenMatch(UnorderedElementsAre(
                        NodeMatches(AllOf(NameMatches(std::string(kDriver)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kInitialInternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentInternalDelayNsec), 0),
                                              IntIs(std::string(kInternalDelayChangedAt), 0),
                                              UintIs(std::string(kInitialExternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentExternalDelayNsec), 0),
                                              IntIs(std::string(kExternalDelayChangedAt), 0),
                                              UintIs(std::string(kDriverTransferBytes), 0),
                                              StringIs(std::string(kName), kUnknown))))),
                        NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                          PropertyList(UnorderedElementsAre(
                                              StringIs(std::string(kSampleFormat), kUnknown),
                                              UintIs(std::string(kChannels), 0),
                                              UintIs(std::string(kFramesPerSecond), 0))))),
                        NodeMatches(AllOf(NameMatches(std::string(kDeviceGain)),
                                          PropertyList(UnorderedElementsAre(
                                              DoubleIs(std::string(kGainDb), 0.0),
                                              BoolIs(std::string(kMuted), false),
                                              BoolIs(std::string(kAgcSupported), false),
                                              BoolIs(std::string(kAgcEnabled), false))))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kRenderers))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(NameMatches(std::string(kCapturers))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kThermalState)),
                                  PropertyList(UnorderedElementsAre(
                                      UintIs(std::string(kThermalStateCount), 1))))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                    NameMatches(kNormal),
                    Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kThermalStateTransitions))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(IsSupersetOf({
                              BoolIs(std::string(kActive), true),
                              StringIs(std::string(kState), kNormal),
                          })),
                          Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kVolumeControls))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(
                    NameMatches(std::string(kActiveUsagePolicies)),
                    PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                      DoubleIs(std::string(kDuckGainDb), 0.0),
                                                      DoubleIs(std::string(kMuteGainDb), 0.0))))),
                ChildrenMatch(Contains(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(Contains(BoolIs(std::string(kActive), true)))))))))));

  output_device->StartSession(zx::time(0));
  output_device->DeviceUnderflow(zx::time(10), zx::time(15));
  output_device->DeviceUnderflow(zx::time(25), zx::time(30));
  output_device->StopSession(zx::time(50));
  output_device->StartSession(zx::time(90));
  output_device->DeviceUnderflow(zx::time(91), zx::time(92));
  output_device->PipelineUnderflow(zx::time(93), zx::time(96));
  output_device->StopSession(zx::time(100));

  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kOutputDevices))),
                  ChildrenMatch(Contains(ChildrenMatch(IsSupersetOf({
                      NodeMatches(AllOf(NameMatches(std::string(kDeviceUnderflows)),
                                        PropertyList(UnorderedElementsAre(
                                            UintIs(std::string(kCount), 3),
                                            UintIs(std::string(kDurationNsec), 11),
                                            UintIs(std::string(kSessionCount), 2))))),
                      NodeMatches(AllOf(
                          NameMatches(std::string(kPipelineUnderflows)),
                          PropertyList(UnorderedElementsAre(
                              UintIs(std::string(kCount), 1), UintIs(std::string(kDurationNsec), 3),
                              UintIs(std::string(kSessionCount), 2))))),
                  }))))))));
}

// Test method Device::SetGainInfo.
TEST_F(ReporterTest, DeviceSetGainInfo) {
  constexpr std::string kTestOutputDevice = "output_device";
  constexpr std::string kTestOutputThread = "output_thread";

  auto output_device = under_test_.CreateOutputDevice(kTestOutputDevice, kTestOutputThread);

  // Expect initial device metric values.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(UnorderedElementsAre(
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(AllOf(NameMatches(kTestOutputDevice),
                                      PropertyList(UnorderedElementsAre(StringIs(
                                          std::string(kMixerThreadName), kTestOutputThread))))),
                    ChildrenMatch(Contains(NodeMatches(AllOf(
                        NameMatches(std::string(kDeviceGain)),
                        PropertyList(UnorderedElementsAre(
                            DoubleIs(std::string(kGainDb), 0.0), BoolIs(std::string(kMuted), false),
                            BoolIs(std::string(kAgcSupported), false),
                            BoolIs(std::string(kAgcEnabled), false))))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kInputDevices))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(NameMatches(std::string(kRenderers))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(NameMatches(std::string(kCapturers))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(NameMatches(std::string(kThermalState)),
                                  PropertyList(UnorderedElementsAre(
                                      UintIs(std::string(kThermalStateCount), 1))))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                    NameMatches(kNormal),
                    Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kThermalStateTransitions))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(IsSupersetOf({
                              BoolIs(std::string(kActive), true),
                              StringIs(std::string(kState), kNormal),
                          })),
                          Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0))))))))),
          AllOf(NodeMatches(NameMatches(std::string(kVolumeControls))), ChildrenMatch(IsEmpty())),
          AllOf(NodeMatches(AllOf(
                    NameMatches(std::string(kActiveUsagePolicies)),
                    PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                      DoubleIs(std::string(kDuckGainDb), 0.0),
                                                      DoubleIs(std::string(kMuteGainDb), 0.0))))),
                ChildrenMatch(Contains(NodeMatches(
                    AllOf(NameMatches("1"),
                          PropertyList(Contains(BoolIs(std::string(kActive), true)))))))))));

  fuchsia::media::AudioGainInfo gain_info_a{
      .gain_db = -1.0f,
      .flags = fuchsia::media::AudioGainInfoFlags::MUTE |
               fuchsia::media::AudioGainInfoFlags::AGC_SUPPORTED |
               fuchsia::media::AudioGainInfoFlags::AGC_ENABLED};

  output_device->SetGainInfo(gain_info_a, {});

  // Expect initial device metric values to remain, since no AudioGainValidFlags were set.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(NameMatches(kTestOutputDevice)),
                    ChildrenMatch(Contains(NodeMatches(AllOf(
                        NameMatches(std::string(kDeviceGain)),
                        PropertyList(UnorderedElementsAre(
                            DoubleIs(std::string(kGainDb), 0.0), BoolIs(std::string(kMuted), false),
                            BoolIs(std::string(kAgcSupported), false),
                            BoolIs(std::string(kAgcEnabled), false))))))))))))));

  output_device->SetGainInfo(gain_info_a, fuchsia::media::AudioGainValidFlags::GAIN_VALID);

  // Expect a gain change.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(
          Contains(AllOf(
              NodeMatches(NameMatches(std::string(kOutputDevices))),
              ChildrenMatch(UnorderedElementsAre(AllOf(
                  NodeMatches(NameMatches(kTestOutputDevice)),
                  ChildrenMatch(Contains(NodeMatches(AllOf(
                      NameMatches(std::string(kDeviceGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), -1.0), BoolIs(std::string(kMuted), false),
                          BoolIs(std::string(kAgcSupported), false),
                          BoolIs(std::string(kAgcEnabled), false))))))))))))));

  output_device->SetGainInfo(gain_info_a, fuchsia::media::AudioGainValidFlags::MUTE_VALID);

  // Expect a mute change.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(NameMatches(kTestOutputDevice)),
                    ChildrenMatch(Contains(NodeMatches(AllOf(
                        NameMatches(std::string(kDeviceGain)),
                        PropertyList(UnorderedElementsAre(
                            DoubleIs(std::string(kGainDb), -1.0), BoolIs(std::string(kMuted), true),
                            BoolIs(std::string(kAgcSupported), false),
                            BoolIs(std::string(kAgcEnabled), false))))))))))))));

  output_device->SetGainInfo(gain_info_a, fuchsia::media::AudioGainValidFlags::AGC_VALID);

  // Expect an AGC change.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(NameMatches(kTestOutputDevice)),
                    ChildrenMatch(Contains(NodeMatches(AllOf(
                        NameMatches(std::string(kDeviceGain)),
                        PropertyList(UnorderedElementsAre(
                            DoubleIs(std::string(kGainDb), -1.0), BoolIs(std::string(kMuted), true),
                            BoolIs(std::string(kAgcSupported), true),
                            BoolIs(std::string(kAgcEnabled), true))))))))))))));

  fuchsia::media::AudioGainInfo gain_info_b{
      .gain_db = -2.0f, .flags = fuchsia::media::AudioGainInfoFlags::AGC_SUPPORTED};
  output_device->SetGainInfo(gain_info_b, fuchsia::media::AudioGainValidFlags::GAIN_VALID |
                                              fuchsia::media::AudioGainValidFlags::MUTE_VALID |
                                              fuchsia::media::AudioGainValidFlags::AGC_VALID);

  // Expect all changes.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(
          Contains(AllOf(
              NodeMatches(NameMatches(std::string(kOutputDevices))),
              ChildrenMatch(UnorderedElementsAre(AllOf(
                  NodeMatches(NameMatches(kTestOutputDevice)),
                  ChildrenMatch(Contains(NodeMatches(AllOf(
                      NameMatches(std::string(kDeviceGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), -2.0), BoolIs(std::string(kMuted), false),
                          BoolIs(std::string(kAgcSupported), true),
                          BoolIs(std::string(kAgcEnabled), false))))))))))))));
}

// Test the method that updates the delays reported by the device.
TEST_F(ReporterTest, DeviceDelays) {
  constexpr std::string kTestOutputDevice = "output_device";
  constexpr std::string kTestInputDevice = "input_device";
  constexpr std::string kTestOutputThread = "output_thread";
  constexpr std::string kTestInputThread = "input_thread";

  auto output_device = under_test_.CreateOutputDevice(kTestOutputDevice, kTestOutputThread);
  auto input_device = under_test_.CreateInputDevice(kTestInputDevice, kTestInputThread);

  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(IsSupersetOf({
          AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(NameMatches(kTestOutputDevice)),
                    ChildrenMatch(IsSupersetOf({
                        NodeMatches(AllOf(NameMatches(std::string(kDriver)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kInitialInternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentInternalDelayNsec), 0),
                                              IntIs(std::string(kInternalDelayChangedAt), 0),
                                              UintIs(std::string(kInitialExternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentExternalDelayNsec), 0),
                                              IntIs(std::string(kExternalDelayChangedAt), 0),
                                              UintIs(std::string(kDriverTransferBytes), 0),
                                              StringIs(std::string(kName), kUnknown))))),
                    })))))),
          AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                ChildrenMatch(UnorderedElementsAre(AllOf(
                    NodeMatches(NameMatches(kTestInputDevice)),
                    ChildrenMatch(IsSupersetOf({
                        NodeMatches(AllOf(NameMatches(std::string(kDriver)),
                                          PropertyList(UnorderedElementsAre(
                                              UintIs(std::string(kInitialInternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentInternalDelayNsec), 0),
                                              IntIs(std::string(kInternalDelayChangedAt), 0),
                                              UintIs(std::string(kInitialExternalDelayNsec), 0),
                                              UintIs(std::string(kCurrentExternalDelayNsec), 0),
                                              IntIs(std::string(kExternalDelayChangedAt), 0),
                                              UintIs(std::string(kDriverTransferBytes), 0),
                                              StringIs(std::string(kName), kUnknown))))),
                    })))))),
      })));

  // For output device, update internal delay; external delay is unknown (and thus not updated).
  const auto kChangeTime1 = 7654321ull;
  const auto kIntDelay1 = 4321ull;
  // For input device, update internal and external delays.
  const auto kChangeTime2 = 1234ull;
  const auto kIntDelay2 = 1234567ull;
  const auto kExtDelay2 = 654321ull;

  output_device->UpdateDelays(zx::time(kChangeTime1), zx::nsec(kIntDelay1), std::nullopt);
  input_device->UpdateDelays(zx::time(kChangeTime2), zx::nsec(kIntDelay2), zx::nsec(kExtDelay2));

  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                        ChildrenMatch(UnorderedElementsAre(AllOf(
                            NodeMatches(NameMatches(kTestOutputDevice)),
                            ChildrenMatch(IsSupersetOf({
                                NodeMatches(AllOf(
                                    NameMatches(std::string(kDriver)),
                                    PropertyList(UnorderedElementsAre(
                                        UintIs(std::string(kInitialInternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentInternalDelayNsec), kIntDelay1),
                                        IntIs(std::string(kInternalDelayChangedAt), kChangeTime1),
                                        UintIs(std::string(kInitialExternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentExternalDelayNsec), 0),
                                        IntIs(std::string(kExternalDelayChangedAt), 0),
                                        UintIs(std::string(kDriverTransferBytes), 0),
                                        StringIs(std::string(kName), kUnknown))))),
                            })))))),
                  AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                        ChildrenMatch(UnorderedElementsAre(AllOf(
                            NodeMatches(NameMatches(kTestInputDevice)),
                            ChildrenMatch(IsSupersetOf({
                                NodeMatches(AllOf(
                                    NameMatches(std::string(kDriver)),
                                    PropertyList(UnorderedElementsAre(
                                        UintIs(std::string(kInitialInternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentInternalDelayNsec), kIntDelay2),
                                        IntIs(std::string(kInternalDelayChangedAt), kChangeTime2),
                                        UintIs(std::string(kInitialExternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentExternalDelayNsec), kExtDelay2),
                                        IntIs(std::string(kExternalDelayChangedAt), kChangeTime2),
                                        UintIs(std::string(kDriverTransferBytes), 0),
                                        StringIs(std::string(kName), kUnknown))))),
                            })))))),
              })));

  // For output, update both delays at a time less than previous change. Internal delay should not
  // change, but external delay should (its most recent value is the initial value at time 0).
  const auto kChangeTime3 = 654321ull;  // < kChangeTime1
  const auto kIntDelay3 = 54321ull;
  const auto kExtDelay3 = 12345ull;
  // For input, update internal delay only.
  const auto kChangeTime4 = 12345678ull;
  const auto kIntDelay4 = 123456ull;

  output_device->UpdateDelays(zx::time(kChangeTime3), zx::nsec(kIntDelay3), zx::nsec(kExtDelay3));
  input_device->UpdateDelays(zx::time(kChangeTime4), zx::nsec(kIntDelay4), std::nullopt);

  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kOutputDevices))),
                        ChildrenMatch(UnorderedElementsAre(AllOf(
                            NodeMatches(NameMatches(kTestOutputDevice)),
                            ChildrenMatch(IsSupersetOf({
                                NodeMatches(AllOf(
                                    NameMatches(std::string(kDriver)),
                                    PropertyList(UnorderedElementsAre(
                                        UintIs(std::string(kInitialInternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentInternalDelayNsec), kIntDelay1),
                                        IntIs(std::string(kInternalDelayChangedAt), kChangeTime1),
                                        UintIs(std::string(kInitialExternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentExternalDelayNsec), kExtDelay3),
                                        IntIs(std::string(kExternalDelayChangedAt), kChangeTime3),
                                        UintIs(std::string(kDriverTransferBytes), 0),
                                        StringIs(std::string(kName), kUnknown))))),
                            })))))),
                  AllOf(NodeMatches(NameMatches(std::string(kInputDevices))),
                        ChildrenMatch(UnorderedElementsAre(AllOf(
                            NodeMatches(NameMatches(kTestInputDevice)),
                            ChildrenMatch(IsSupersetOf({
                                NodeMatches(AllOf(
                                    NameMatches(std::string(kDriver)),
                                    PropertyList(UnorderedElementsAre(
                                        UintIs(std::string(kInitialInternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentInternalDelayNsec), kIntDelay4),
                                        IntIs(std::string(kInternalDelayChangedAt), kChangeTime4),
                                        UintIs(std::string(kInitialExternalDelayNsec), 0),
                                        UintIs(std::string(kCurrentExternalDelayNsec), kExtDelay2),
                                        IntIs(std::string(kExternalDelayChangedAt), kChangeTime2),
                                        UintIs(std::string(kDriverTransferBytes), 0),
                                        StringIs(std::string(kName), kUnknown))))),
                            })))))),
              })));
}

// Test methods that add and remove client ports.
TEST_F(ReporterTest, AddRemoveClientPorts) {
  std::vector<Reporter::Container<Reporter::Renderer, Reporter::kObjectsToCache>::Ptr> renderers;
  std::vector<Reporter::Container<Reporter::Capturer, Reporter::kObjectsToCache>::Ptr> capturers;
  renderers.reserve(5);
  for (size_t k = 0; k < 5; k++) {
    renderers.push_back(under_test_.CreateRenderer());
  }
  capturers.reserve(5);
  for (size_t k = 0; k < 5; k++) {
    capturers.push_back(under_test_.CreateCapturer(fxl::StringPrintf("capture_thread_%lu", k)));
  }

  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(IsSupersetOf({
          AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                ChildrenMatch(UnorderedElementsAre(NodeAlive("1"), NodeAlive("2"), NodeAlive("3"),
                                                   NodeAlive("4"), NodeAlive("5")))),
          AllOf(NodeMatches(NameMatches(std::string(kCapturers))),
                ChildrenMatch(UnorderedElementsAre(NodeAlive("1"), NodeAlive("2"), NodeAlive("3"),
                                                   NodeAlive("4"), NodeAlive("5")))),
      })));

  renderers[0].Drop();
  renderers[1].Drop();
  renderers[2].Drop();
  renderers[3].Drop();
  capturers[0].Drop();
  capturers[1].Drop();
  capturers[2].Drop();
  capturers[3].Drop();

  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(IsSupersetOf({
          AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                ChildrenMatch(UnorderedElementsAre(NodeDead("1"), NodeDead("2"), NodeDead("3"),
                                                   NodeDead("4"), NodeAlive("5")))),
          AllOf(NodeMatches(NameMatches(std::string(kCapturers))),
                ChildrenMatch(UnorderedElementsAre(NodeDead("1"), NodeDead("2"), NodeDead("3"),
                                                   NodeDead("4"), NodeAlive("5")))),
      })));

  renderers[4].Drop();
  capturers[4].Drop();

  // Garbage collect [0].
  EXPECT_THAT(GetHierarchyLazyValues(),
              ChildrenMatch(IsSupersetOf({
                  AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                        ChildrenMatch(UnorderedElementsAre(NodeDead("2"), NodeDead("3"),
                                                           NodeDead("4"), NodeDead("5")))),
                  AllOf(NodeMatches(NameMatches(std::string(kCapturers))),
                        ChildrenMatch(UnorderedElementsAre(NodeDead("2"), NodeDead("3"),
                                                           NodeDead("4"), NodeDead("5")))),
              })));
}

// Tests methods that change renderer metrics, that aren't tested in other cases.
TEST_F(ReporterTest, RendererMetrics) {
  auto renderer = under_test_.CreateRenderer();

  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kRenderers))),
          ChildrenMatch(UnorderedElementsAre(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kInitialMinLeadTimeNsec), 0),
                                    UintIs(std::string(kCurrentMinLeadTimeNsec), 0),
                                    IntIs(std::string(kMinLeadTimeChangedAt), 0),
                                    StringIs(std::string(kUsage), kDefault))))),
              ChildrenMatch(UnorderedElementsAre(
                  NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                    PropertyList(UnorderedElementsAre(
                                        StringIs(std::string(kSampleFormat), kUnknown),
                                        UintIs(std::string(kChannels), 0),
                                        UintIs(std::string(kFramesPerSecond), 0))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), 0.0), BoolIs(std::string(kMuted), false),
                          UintIs(std::string(kCallsToSetGainWithRamp), 0),
                          DoubleIs(std::string(kCompleteStreamGainDb), 0.0))))),
                  NodeMatches(AllOf(NameMatches(std::string(kPresentationTimestamps)),
                                    PropertyList(UnorderedElementsAre(
                                        DoubleIs(std::string(kPtsContinuityThresholdSec), 0.0),
                                        UintIs(std::string(kPtsUnitsDenominator), 1),
                                        UintIs(std::string(kPtsUnitsNumerator), 1'000'000'000))))),
                  AllOf(NodeMatches(NameMatches(std::string(kPayloadBuffers))),
                        ChildrenMatch(IsEmpty())),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kPacketQueueUnderflows)),
                      PropertyList(UnorderedElementsAre(UintIs(std::string(kCount), 0),
                                                        UintIs(std::string(kDurationNsec), 0),
                                                        UintIs(std::string(kSessionCount), 0))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kContinuityUnderflows)),
                      PropertyList(UnorderedElementsAre(UintIs(std::string(kCount), 0),
                                                        UintIs(std::string(kDurationNsec), 0),
                                                        UintIs(std::string(kSessionCount), 0))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kTimestampUnderflows)),
                      PropertyList(UnorderedElementsAre(
                          UintIs(std::string(kCount), 0), UintIs(std::string(kDurationNsec), 0),
                          UintIs(std::string(kSessionCount), 0))))))))))))));

  renderer->SetUsage(RenderUsage::MEDIA);
  renderer->SetFormat(
      Format::Create({
                         .sample_format = fuchsia::media::AudioSampleFormat::SIGNED_16,
                         .channels = 2,
                         .frames_per_second = 48000,
                     })
          .take_value());

  renderer->AddPayloadBuffer(0, 4096);
  renderer->AddPayloadBuffer(10, 8192);
  renderer->SendPacket(fuchsia::media::StreamPacket{
      .payload_buffer_id = 10,
  });

  renderer->SetGain(-1.0);
  renderer->SetMute(true);
  renderer->SetGainWithRamp(-1.0, zx::sec(1), fuchsia::media::audio::RampType::SCALE_LINEAR);
  renderer->SetGainWithRamp(-1.0, zx::sec(1), fuchsia::media::audio::RampType::SCALE_LINEAR);
  renderer->SetCompleteGain(-6.0);

  renderer->SetPtsContinuityThreshold(5.0);
  renderer->SetPtsUnits(1234567, 3);

  renderer->StartSession(zx::time(0));

  renderer->PacketQueueUnderflow(zx::time(10), zx::time(15));

  renderer->ContinuityUnderflow(zx::time(20), zx::time(30));
  renderer->ContinuityUnderflow(zx::time(40), zx::time(50));

  renderer->TimestampUnderflow(zx::time(0), zx::time(15));
  renderer->TimestampUnderflow(zx::time(30), zx::time(45));
  renderer->TimestampUnderflow(zx::time(60), zx::time(75));

  renderer->StopSession(zx::time(100));

  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kRenderers))),
          ChildrenMatch(UnorderedElementsAre(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kInitialMinLeadTimeNsec), 0),
                                    UintIs(std::string(kCurrentMinLeadTimeNsec), 0),
                                    IntIs(std::string(kMinLeadTimeChangedAt), 0),
                                    StringIs(std::string(kUsage), "RenderUsage::MEDIA"))))),
              ChildrenMatch(UnorderedElementsAre(
                  NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                    PropertyList(UnorderedElementsAre(
                                        StringIs(std::string(kSampleFormat), kSampleFormatInt16),
                                        UintIs(std::string(kChannels), 2),
                                        UintIs(std::string(kFramesPerSecond), 48000))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), -1.0), BoolIs(std::string(kMuted), true),
                          UintIs(std::string(kCallsToSetGainWithRamp), 2),
                          DoubleIs(std::string(kCompleteStreamGainDb), -6.0))))),
                  NodeMatches(AllOf(NameMatches(std::string(kPresentationTimestamps)),
                                    PropertyList(UnorderedElementsAre(
                                        DoubleIs(std::string(kPtsContinuityThresholdSec), 5.0),
                                        UintIs(std::string(kPtsUnitsDenominator), 3),
                                        UintIs(std::string(kPtsUnitsNumerator), 1234567))))),
                  AllOf(NodeMatches(NameMatches(std::string(kPayloadBuffers))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeMatches(
                                AllOf(NameMatches("0"), PropertyList(UnorderedElementsAre(
                                                            UintIs(std::string(kSize), 4096),
                                                            UintIs(std::string(kPackets), 0))))),
                            NodeMatches(AllOf(NameMatches("10"),
                                              PropertyList(UnorderedElementsAre(
                                                  UintIs(std::string(kSize), 8192),
                                                  UintIs(std::string(kPackets), 1)))))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kPacketQueueUnderflows)),
                      PropertyList(UnorderedElementsAre(UintIs(std::string(kCount), 1),
                                                        UintIs(std::string(kDurationNsec), 5),
                                                        UintIs(std::string(kSessionCount), 1))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kContinuityUnderflows)),
                      PropertyList(UnorderedElementsAre(UintIs(std::string(kCount), 2),
                                                        UintIs(std::string(kDurationNsec), 20),
                                                        UintIs(std::string(kSessionCount), 1))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kTimestampUnderflows)),
                      PropertyList(UnorderedElementsAre(
                          UintIs(std::string(kCount), 3), UintIs(std::string(kDurationNsec), 45),
                          UintIs(std::string(kSessionCount), 1))))))))))))));
}

// Tests methods that change renderer minimum lead time metrics.
TEST_F(ReporterTest, RendererMinLeadTime) {
  auto renderer = under_test_.CreateRenderer();
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(
                  AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                        ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                            NameMatches("1"), PropertyList(IsSupersetOf({
                                                  UintIs(std::string(kInitialMinLeadTimeNsec), 0),
                                                  UintIs(std::string(kCurrentMinLeadTimeNsec), 0),
                                                  IntIs(std::string(kMinLeadTimeChangedAt), 0),
                                              }))))))))));

  // SetInitialMinLeadTime is optional; UpdateMinLeadTime can be called immediately.
  constexpr auto kCurrentMinLeadTime1 = 321ull;
  constexpr auto kTimeOfMinLeadTimeChange1 = 123ll;
  renderer->UpdateMinLeadTime(zx::nsec(kCurrentMinLeadTime1), zx::time(kTimeOfMinLeadTimeChange1));
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                    NameMatches("1"),
                    PropertyList(IsSupersetOf({
                        UintIs(std::string(kInitialMinLeadTimeNsec), 0),  // Retains value from ctor
                        UintIs(std::string(kCurrentMinLeadTimeNsec), kCurrentMinLeadTime1),
                        IntIs(std::string(kMinLeadTimeChangedAt), kTimeOfMinLeadTimeChange1),
                    }))))))))));

  // We expect the initial and current values to change, and the time-of-update to be reset.
  constexpr auto kInitialMinLeadTime2 = 1'000'000ull;
  renderer->SetInitialMinLeadTime(zx::nsec(kInitialMinLeadTime2));
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(
                  AllOf(NodeMatches(NameMatches(std::string(kRenderers))),
                        ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                            NameMatches("1"),
                            PropertyList(IsSupersetOf({
                                UintIs(std::string(kInitialMinLeadTimeNsec), kInitialMinLeadTime2),
                                UintIs(std::string(kCurrentMinLeadTimeNsec), kInitialMinLeadTime2),
                                IntIs(std::string(kMinLeadTimeChangedAt), 0),  // Was reset
                            }))))))))));

  // We expect the current value and time-of-update value to change.
  constexpr auto kCurrentMinLeadTime3 = 12'345'678ull;
  constexpr auto kTimeOfMinLeadTimeChange3 = 987'654'321ll;
  renderer->UpdateMinLeadTime(zx::nsec(kCurrentMinLeadTime3), zx::time(kTimeOfMinLeadTimeChange3));
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kRenderers))),
                  ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                      NameMatches("1"),
                      PropertyList(IsSupersetOf({
                          UintIs(std::string(kInitialMinLeadTimeNsec), kInitialMinLeadTime2),
                          UintIs(std::string(kCurrentMinLeadTimeNsec), kCurrentMinLeadTime3),
                          IntIs(std::string(kMinLeadTimeChangedAt), kTimeOfMinLeadTimeChange3),
                      }))))))))));

  // The time-of-update is before the previous one, so we expect no change.
  constexpr auto kCurrentMinLeadTime4 = 1'234'567ull;
  constexpr auto kTimeOfMinLeadTimeChange4 = 87'654'321ll;  // less than kTimeOfMinLeadTimeChange3
  renderer->UpdateMinLeadTime(zx::nsec(kCurrentMinLeadTime4), zx::time(kTimeOfMinLeadTimeChange4));
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kRenderers))),
                  ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                      NameMatches("1"),
                      PropertyList(IsSupersetOf({
                          UintIs(std::string(kInitialMinLeadTimeNsec), kInitialMinLeadTime2),
                          UintIs(std::string(kCurrentMinLeadTimeNsec), kCurrentMinLeadTime3),
                          IntIs(std::string(kMinLeadTimeChangedAt), kTimeOfMinLeadTimeChange3),
                      }))))))))));
}

// Tests methods that change capturer metrics, that aren't tested in other cases.
TEST_F(ReporterTest, CapturerMetrics) {
  constexpr std::string kTestInputThread = "input_thread";

  auto capturer = under_test_.CreateCapturer(kTestInputThread);

  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kCapturers))),
          ChildrenMatch(UnorderedElementsAre(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kInitialPresentationDelayNsec), 0),
                                    UintIs(std::string(kCurrentPresentationDelayNsec), 0),
                                    IntIs(std::string(kPresentationDelayChangedAt), 0),
                                    StringIs(std::string(kUsage), kDefault),
                                    StringIs(std::string(kMixerThreadName), kTestInputThread))))),
              ChildrenMatch(UnorderedElementsAre(
                  NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                    PropertyList(UnorderedElementsAre(
                                        StringIs(std::string(kSampleFormat), kUnknown),
                                        UintIs(std::string(kChannels), 0),
                                        UintIs(std::string(kFramesPerSecond), 0))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), 0.0), BoolIs(std::string(kMuted), false),
                          UintIs(std::string(kCallsToSetGainWithRamp), 0),
                          DoubleIs(std::string(kCompleteStreamGainDb), 0.0))))),
                  AllOf(NodeMatches(NameMatches(std::string(kPayloadBuffers))),
                        ChildrenMatch(IsEmpty())),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kCaptureOverflows)),
                      PropertyList(UnorderedElementsAre(
                          UintIs(std::string(kCount), 0), UintIs(std::string(kDurationNsec), 0),
                          UintIs(std::string(kSessionCount), 0))))))))))))));

  capturer->SetUsage(CaptureUsage::FOREGROUND);
  capturer->SetFormat(
      Format::Create({
                         .sample_format = fuchsia::media::AudioSampleFormat::SIGNED_16,
                         .channels = 2,
                         .frames_per_second = 48000,
                     })
          .take_value());

  capturer->AddPayloadBuffer(0, 4096);
  capturer->AddPayloadBuffer(10, 8192);
  capturer->SendPacket(fuchsia::media::StreamPacket{
      .payload_buffer_id = 10,
  });

  capturer->SetGain(-1.0);
  capturer->SetMute(true);
  capturer->SetGainWithRamp(-1.0, zx::sec(1), fuchsia::media::audio::RampType::SCALE_LINEAR);
  capturer->SetGainWithRamp(-1.0, zx::sec(1), fuchsia::media::audio::RampType::SCALE_LINEAR);

  capturer->StartSession(zx::time(0));

  capturer->Overflow(zx::time(60), zx::time(65));

  capturer->StopSession(zx::time(100));

  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kCapturers))),
          ChildrenMatch(UnorderedElementsAre(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kInitialPresentationDelayNsec), 0),
                                    UintIs(std::string(kCurrentPresentationDelayNsec), 0),
                                    IntIs(std::string(kPresentationDelayChangedAt), 0),
                                    StringIs(std::string(kUsage), "CaptureUsage::FOREGROUND"),
                                    StringIs(std::string(kMixerThreadName), kTestInputThread))))),
              ChildrenMatch(UnorderedElementsAre(
                  NodeMatches(AllOf(NameMatches(std::string(kFormat)),
                                    PropertyList(UnorderedElementsAre(
                                        StringIs(std::string(kSampleFormat), kSampleFormatInt16),
                                        UintIs(std::string(kChannels), 2),
                                        UintIs(std::string(kFramesPerSecond), 48000))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kGain)),
                      PropertyList(UnorderedElementsAre(
                          DoubleIs(std::string(kGainDb), -1.0), BoolIs(std::string(kMuted), true),
                          UintIs(std::string(kCallsToSetGainWithRamp), 2),
                          DoubleIs(std::string(kCompleteStreamGainDb), 0.0))))),
                  AllOf(NodeMatches(NameMatches(std::string(kPayloadBuffers))),
                        ChildrenMatch(UnorderedElementsAre(
                            NodeMatches(
                                AllOf(NameMatches("0"), PropertyList(UnorderedElementsAre(
                                                            UintIs(std::string(kSize), 4096),
                                                            UintIs(std::string(kPackets), 0))))),
                            NodeMatches(AllOf(NameMatches("10"),
                                              PropertyList(UnorderedElementsAre(
                                                  UintIs(std::string(kSize), 8192),
                                                  UintIs(std::string(kPackets), 1)))))))),
                  NodeMatches(AllOf(
                      NameMatches(std::string(kCaptureOverflows)),
                      PropertyList(UnorderedElementsAre(
                          UintIs(std::string(kCount), 1), UintIs(std::string(kDurationNsec), 5),
                          UintIs(std::string(kSessionCount), 1))))))))))))));
}

// Tests methods that change capturer presentation delay metrics.
TEST_F(ReporterTest, CapturerPresentationDelay) {
  constexpr std::string kTestInputThread = "input_thread";

  auto capturer = under_test_.CreateCapturer(kTestInputThread);
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kCapturers))),
                  ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                      NameMatches("1"), PropertyList(IsSupersetOf({
                                            UintIs(std::string(kInitialPresentationDelayNsec), 0),
                                            UintIs(std::string(kCurrentPresentationDelayNsec), 0),
                                            IntIs(std::string(kPresentationDelayChangedAt), 0),
                                        }))))))))));

  // SetInitialPresentationDelay is optional; UpdatePresentationDelay can be called immediately.
  constexpr auto kCurrentPresDelay1 = 432ull;
  constexpr auto kTimeOfPresDelayChange1 = 234ll;
  capturer->UpdatePresentationDelay(zx::nsec(kCurrentPresDelay1),
                                    zx::time(kTimeOfPresDelayChange1));
  EXPECT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kCapturers))),
                  ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                      NameMatches("1"),
                      PropertyList(IsSupersetOf({
                          UintIs(std::string(kInitialPresentationDelayNsec),
                                 0),  // Retains value from ctor.
                          UintIs(std::string(kCurrentPresentationDelayNsec), kCurrentPresDelay1),
                          IntIs(std::string(kPresentationDelayChangedAt), kTimeOfPresDelayChange1),
                      }))))))))));

  // We expect the initial and current values to change, and the time-of-update to be reset.
  constexpr auto kInitialPresentationDelay2 = 2'000'000ull;
  capturer->SetInitialPresentationDelay(zx::nsec(kInitialPresentationDelay2));
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kCapturers))),
          ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
              NameMatches("1"),
              PropertyList(IsSupersetOf({
                  UintIs(std::string(kInitialPresentationDelayNsec), kInitialPresentationDelay2),
                  UintIs(std::string(kCurrentPresentationDelayNsec), kInitialPresentationDelay2),
                  IntIs(std::string(kPresentationDelayChangedAt), 0),  // Was reset
              }))))))))));

  // We expect the current value and time-of-update value to change.
  constexpr auto kCurrentPresDelay3 = 23'456'789ull;
  constexpr auto kTimeOfPresDelayChange3 = 876'543'210ll;
  capturer->UpdatePresentationDelay(zx::nsec(kCurrentPresDelay3),
                                    zx::time(kTimeOfPresDelayChange3));
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kCapturers))),
          ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
              NameMatches("1"),
              PropertyList(IsSupersetOf({
                  UintIs(std::string(kInitialPresentationDelayNsec), kInitialPresentationDelay2),
                  UintIs(std::string(kCurrentPresentationDelayNsec), kCurrentPresDelay3),
                  IntIs(std::string(kPresentationDelayChangedAt), kTimeOfPresDelayChange3),
              }))))))))));

  // The time-of-update is before the previous one, so we expect no change.
  constexpr auto kCurrentPresDelay4 = 2'345'678ull;
  constexpr auto kTimeOfPresDelayChange4 = 76'543'210ll;  // Less than kTime...Change3
  capturer->UpdatePresentationDelay(zx::nsec(kCurrentPresDelay4),
                                    zx::time(kTimeOfPresDelayChange4));
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kCapturers))),
          ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
              NameMatches("1"),
              PropertyList(IsSupersetOf({
                  UintIs(std::string(kInitialPresentationDelayNsec), kInitialPresentationDelay2),
                  UintIs(std::string(kCurrentPresentationDelayNsec), kCurrentPresDelay3),
                  IntIs(std::string(kPresentationDelayChangedAt), kTimeOfPresDelayChange3),
              }))))))))));
}

// Tests ThermalStateTracker methods.
TEST_F(ReporterTest, SetThermalStateMetrics) {
  under_test_.SetNumThermalStates(3);
  under_test_.SetThermalState(0);
  // Expect first thermal state metric values.
  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kThermalState)),
              PropertyList(UnorderedElementsAre(UintIs(std::string(kThermalStateCount), 3))))),
          ChildrenMatch(UnorderedElementsAre(NodeMatches(
              AllOf(NameMatches(kNormal),
                    Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))))));
  // Expect second thermal state metric values, with first thermal state metrics stored.
  under_test_.SetThermalState(2);
  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kThermalState)),
              PropertyList(UnorderedElementsAre(UintIs(std::string(kThermalStateCount), 3))))),
          ChildrenMatch(UnorderedElementsAre(
              NodeMatches(AllOf(NameMatches(kNormal), Not(PropertyList(Contains(UintIs(
                                                          std::string(kTotalDurationNsec), 0)))))),
              NodeMatches(AllOf(
                  NameMatches("2"),
                  Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))))));
  // Expect values to be unchanged, since state 2 has already been triggered.
  under_test_.SetThermalState(2);
  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kThermalState)),
              PropertyList(UnorderedElementsAre(UintIs(std::string(kThermalStateCount), 3))))),
          ChildrenMatch(UnorderedElementsAre(
              NodeMatches(AllOf(NameMatches(kNormal), Not(PropertyList(Contains(UintIs(
                                                          std::string(kTotalDurationNsec), 0)))))),
              NodeMatches(AllOf(
                  NameMatches("2"),
                  Not(PropertyList(Contains(UintIs(std::string(kTotalDurationNsec), 0))))))))))));
}

// Test caching of ThermalStates up to limit Reporter::kThermalStatesToCache == 8.
TEST_F(ReporterTest, CacheThermalStateTransitions) {
  // Reporter initializes thermal state to 0.
  under_test_.SetThermalState(1);  // ThermalState 2, first cached
  under_test_.SetThermalState(2);
  under_test_.SetThermalState(0);
  under_test_.SetThermalState(1);
  under_test_.SetThermalState(2);
  under_test_.SetThermalState(1);
  under_test_.SetThermalState(2);
  under_test_.SetThermalState(2);  // Skip duplicate.
  under_test_.SetThermalState(0);  // ThermalState 9, final cached
  under_test_.SetThermalState(1);  // ThermalState 10, alive

  // Expect most recent 8 thermal state metric values.
  EXPECT_THAT(
      GetHierarchyLazyValues(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches(std::string(kThermalStateTransitions))),
                ChildrenMatch(UnorderedElementsAre(
                    NodeMatches(
                        AllOf(NameMatches("2"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "1"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("3"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "2"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("4"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), kNormal),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("5"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "1"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("6"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "2"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("7"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "1"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("8"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), "2"),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(
                        AllOf(NameMatches("9"),
                              PropertyList(IsSupersetOf({
                                  BoolIs(std::string(kActive), false),
                                  StringIs(std::string(kState), kNormal),
                              })),
                              Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0)))))),
                    NodeMatches(AllOf(
                        NameMatches("10"),
                        PropertyList(IsSupersetOf({
                            BoolIs(std::string(kActive), true),
                            StringIs(std::string(kState), "1"),
                        })),
                        Not(PropertyList(Contains(UintIs(std::string(kDurationNsec), 0))))))))))));
}

// Test VolumeControl methods.
TEST_F(ReporterTest, VolumeControlMetrics) {
  auto volume_control = under_test_.CreateVolumeControl();

  // Expect initial volume control metrics.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kVolumeControls))),
          ChildrenMatch(Contains(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kClientCount), 0),
                                    StringIs(std::string(kName), kUnknownNoClients))))),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kVolumeSettings))),
                  ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                      NameMatches("1"), PropertyList(UnorderedElementsAre(
                                            BoolIs(std::string(kActive), true),
                                            BoolIs(std::string(kMuted), false),
                                            DoubleIs(std::string(kVolume), 0.0)))))))))))))))));

  volume_control->SetVolumeMute(0.5, true);
  volume_control->AddBinding("RenderUsage::MEDIA");
  volume_control->AddBinding("RenderUsage::MEDIA");

  // Expect |volume_control| settings to be reflected, with past volume settings cached.
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kVolumeControls))),
          ChildrenMatch(Contains(AllOf(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(UnorderedElementsAre(
                                    UintIs(std::string(kClientCount), 2),
                                    StringIs(std::string(kName), "RenderUsage::MEDIA"))))),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kVolumeSettings))),
                  ChildrenMatch(UnorderedElementsAre(
                      NodeMatches(AllOf(
                          NameMatches("1"),
                          PropertyList(UnorderedElementsAre(BoolIs(std::string(kActive), false),
                                                            BoolIs(std::string(kMuted), false),
                                                            DoubleIs(std::string(kVolume), 0.0))))),
                      NodeMatches(AllOf(
                          NameMatches("2"),
                          PropertyList(UnorderedElementsAre(
                              BoolIs(std::string(kActive), true), BoolIs(std::string(kMuted), true),
                              DoubleIs(std::string(kVolume), 0.5)))))))))))))))));
}

// Test methods that change audio policy metrics.
TEST_F(ReporterTest, AudioPolicyMetrics) {
  // Expect behavior gains to be logged, and initial active audio policy to have no active usages.
  under_test_.SetAudioPolicyBehaviorGain(
      AudioAdmin::BehaviorGain({.none_gain_db = 0., .duck_gain_db = -10., .mute_gain_db = -100.}));
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kActiveUsagePolicies)),
              PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                DoubleIs(std::string(kDuckGainDb), -10.0),
                                                DoubleIs(std::string(kMuteGainDb), -100.0))))),
          ChildrenMatch(Contains(NodeMatches(AllOf(
              NameMatches("1"), PropertyList(Contains(BoolIs(std::string(kActive), true)))))))))));

  // Structures to hold active usages and usage behaviors.
  std::vector<fuchsia::media::Usage2> active_usages;
  std::array<fuchsia::media::Behavior, fuchsia::media::RENDER_USAGE2_COUNT> render_usage_behaviors;
  std::array<fuchsia::media::Behavior, fuchsia::media::CAPTURE_USAGE2_COUNT>
      capture_usage_behaviors;
  render_usage_behaviors.fill(fuchsia::media::Behavior::NONE);
  capture_usage_behaviors.fill(fuchsia::media::Behavior::NONE);

  // Expect active RenderUsage::MEDIA to be logged, with default policy NONE.
  active_usages.push_back(ToFidlUsage2(RenderUsage::MEDIA));
  under_test_.UpdateActiveUsagePolicy(active_usages, render_usage_behaviors,
                                      capture_usage_behaviors);
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kActiveUsagePolicies)),
              PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                DoubleIs(std::string(kDuckGainDb), -10.0),
                                                DoubleIs(std::string(kMuteGainDb), -100.0))))),
          ChildrenMatch(UnorderedElementsAre(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(Contains(BoolIs(std::string(kActive), false))))),
              NodeMatches(AllOf(
                  NameMatches("2"),
                  PropertyList(UnorderedElementsAre(BoolIs(std::string(kActive), true),
                                                    StringIs("RenderUsage::MEDIA", kNone)))))))))));

  // Expect active RenderUsage::MEDIA and CaptureUsage::SYSTEM_AGENT to be logged, with DUCK applied
  // to MEDIA.
  active_usages.push_back(ToFidlUsage2(CaptureUsage::SYSTEM_AGENT));
  render_usage_behaviors[static_cast<int>(AudioRenderUsage2::MEDIA)] =
      fuchsia::media::Behavior::DUCK;
  under_test_.UpdateActiveUsagePolicy(active_usages, render_usage_behaviors,
                                      capture_usage_behaviors);
  EXPECT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(AllOf(
              NameMatches(std::string(kActiveUsagePolicies)),
              PropertyList(UnorderedElementsAre(DoubleIs(std::string(kNoneGainDb), 0.0),
                                                DoubleIs(std::string(kDuckGainDb), -10.0),
                                                DoubleIs(std::string(kMuteGainDb), -100.0))))),
          ChildrenMatch(UnorderedElementsAre(
              NodeMatches(AllOf(NameMatches("1"),
                                PropertyList(Contains(BoolIs(std::string(kActive), false))))),
              NodeMatches(AllOf(NameMatches("2"), PropertyList(UnorderedElementsAre(
                                                      BoolIs(std::string(kActive), false),
                                                      StringIs("RenderUsage::MEDIA", kNone))))),
              NodeMatches(AllOf(NameMatches("3"),
                                PropertyList(UnorderedElementsAre(
                                    BoolIs(std::string(kActive), true),
                                    StringIs("RenderUsage::MEDIA", kDuck),
                                    StringIs("CaptureUsage::SYSTEM_AGENT", kNone)))))))))));
}
}  // namespace
}  // namespace media::audio
