// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/input/cpp/fidl.h>
#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/ui/input3/cpp/fidl.h>
#include <fuchsia/ui/pointer/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/zx/result.h>

#include <cmath>

#include <virtio/input.h>

#include "src/virtualization/bin/vmm/device/input.h"
#include "src/virtualization/bin/vmm/device/tests/test_with_device.h"
#include "src/virtualization/bin/vmm/device/tests/virtio_queue_fake.h"

namespace {

constexpr uint16_t kNumQueues = 2;
constexpr uint16_t kQueueSize = 16;

const size_t kPageSize = zx_system_get_page_size();

constexpr auto kComponentUrl = "#meta/virtio_input.cm";

struct VirtioInputTestParam {
  std::string test_name;
  bool configure_status_queue;
};

class VirtioInputTest : public TestWithDevice,
                        public ::testing::WithParamInterface<VirtioInputTestParam> {
 protected:
  VirtioInputTest()
      : event_queue_(phys_mem_, kPageSize * kNumQueues, kQueueSize),
        status_queue_(phys_mem_, event_queue_.end(), kQueueSize) {}

  void SetUpWithInputType(fuchsia::virtualization::hardware::InputType input_type) {
    using component_testing::ChildRef;
    using component_testing::ParentRef;
    using component_testing::Protocol;
    using component_testing::RealmBuilder;
    using component_testing::RealmRoot;
    using component_testing::Route;

    constexpr auto kComponentName = "virtio_input";

    auto realm_builder = RealmBuilder::Create();
    realm_builder.AddChild(kComponentName, kComponentUrl);

    realm_builder
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::logger::LogSink::Name_},
                                Protocol{fuchsia::tracing::provider::Registry::Name_},
                            },
                        .source = ParentRef(),
                        .targets = {ChildRef{kComponentName}}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::virtualization::hardware::VirtioInput::Name_},
                            },
                        .source = ChildRef{kComponentName},
                        .targets = {ParentRef()}});

    realm_ = std::make_unique<RealmRoot>(realm_builder.Build(dispatcher()));

    fuchsia::virtualization::hardware::StartInfo start_info;
    zx_status_t status = MakeStartInfo(status_queue_.end(), &start_info);
    ASSERT_EQ(ZX_OK, status);

    input_ = realm_->component().ConnectSync<fuchsia::virtualization::hardware::VirtioInput>();

    status = input_->Start(std::move(start_info), std::move(input_type));
    ASSERT_EQ(ZX_OK, status);

    // Configure device queues.
    VirtioQueueFake* queues[kNumQueues] = {&event_queue_, &status_queue_};
    for (uint16_t i = 0; i < kNumQueues; i++) {
      auto q = queues[i];
      q->Configure(kPageSize * i, kPageSize);
      status = input_->ConfigureQueue(i, q->size(), q->desc(), q->avail(), q->used());
      ASSERT_EQ(ZX_OK, status);

      if (!GetParam().configure_status_queue) {
        break;
      }
    }

    // Finish negotiating features.
    status = input_->Ready(0);
    ASSERT_EQ(ZX_OK, status);
  }

  zx::result<std::vector<virtio_input_event_t*>> AddEventDescriptorsToChain(size_t n) {
    std::vector<virtio_input_event_t*> result(n);
    // Note: virtio-input sends only one virtio_input_event_t per chain.
    for (size_t i = 0; i < n; ++i) {
      virtio_input_event_t* event;
      zx_status_t status = DescriptorChainBuilder(event_queue_)
                               .AppendWritableDescriptor(&event, sizeof(*event))
                               .Build();
      if (status != ZX_OK) {
        return zx::error(status);
      }
      result[i] = event;
    }
    return zx::ok(std::move(result));
  }

  // Note: use of sync can be problematic here if the test environment needs to handle
  // some incoming FIDL requests.
  fuchsia::virtualization::hardware::VirtioInputSyncPtr input_;
  VirtioQueueFake event_queue_;
  VirtioQueueFake status_queue_;
  std::unique_ptr<component_testing::RealmRoot> realm_;
};

class VirtioInputKeyboardTest : public VirtioInputTest {
 protected:
  void SetUp() override {
    SetUpWithInputType(fuchsia::virtualization::hardware::InputType::WithKeyboard(
        keyboard_listener_.NewRequest()));
  }

  fuchsia::ui::input3::KeyboardListenerPtr keyboard_listener_;
};

class VirtioInputMouseTest : public VirtioInputTest, public fuchsia::ui::pointer::MouseSource {
 protected:
  void SetUp() override {
    fuchsia::ui::pointer::MouseSourceHandle mouse_client;
    binding_.Bind(mouse_client.NewRequest());
    SetUpWithInputType(
        fuchsia::virtualization::hardware::InputType::WithMouse(std::move(mouse_client)));
  }

  // fuchsia.ui.pointer.MouseSource implementation
  void Watch(WatchCallback callback) override {
    ASSERT_FALSE(watch_responder_.has_value()) << "Device has two Watch calls in flight";
    watch_responder_ = std::move(callback);
  }

  std::optional<fuchsia::ui::pointer::MouseSource::WatchCallback> TakeResponder() {
    return std::exchange(watch_responder_, std::nullopt);
  }

  fidl::Binding<fuchsia::ui::pointer::MouseSource> binding_{this};

 private:
  std::optional<fuchsia::ui::pointer::MouseSource::WatchCallback> watch_responder_;
};

TEST_P(VirtioInputKeyboardTest, Keyboard) {
  // Enqueue descriptors for the events. Do this before we inject the key event because the device
  // may choose to drop input events if there are no descriptors available:
  //
  // 5.8.6.2 Device Requirements: Device Operation
  //
  // A device MAY drop input events if the eventq does not have enough available buffers.
  auto events = AddEventDescriptorsToChain(2);
  ASSERT_TRUE(events.is_ok());

  zx_status_t status = input_->NotifyQueue(0);
  ASSERT_EQ(ZX_OK, status);

  // Inject a key event.
  std::optional<fuchsia::ui::input3::KeyEventStatus> key_status = std::nullopt;
  fuchsia::ui::input3::KeyEvent keyboard;
  keyboard.set_type(fuchsia::ui::input3::KeyEventType::PRESSED);
  keyboard.set_key(fuchsia::input::Key::A);
  keyboard_listener_->OnKeyEvent(std::move(keyboard), [&](auto result) { key_status = result; });

  // Expect the virtio event.
  status = WaitOnInterrupt();
  ASSERT_EQ(ZX_OK, status);

  // Verify we received 2 events; key press + sync.
  auto event_1 = events.value()[0];
  EXPECT_EQ(VIRTIO_INPUT_EV_KEY, event_1->type);
  EXPECT_EQ(30, event_1->code);
  EXPECT_EQ(VIRTIO_INPUT_EV_KEY_PRESSED, event_1->value);
  auto event_2 = events.value()[1];
  EXPECT_EQ(VIRTIO_INPUT_EV_SYN, event_2->type);
}

constexpr uint32_t kMouseDeviceId = 0;

fuchsia::ui::pointer::MouseEvent MakeMouseUpdate() {
  static uint64_t timestamp = 0;

  fuchsia::ui::pointer::MousePointerSample pointer_sample;
  pointer_sample.set_device_id(kMouseDeviceId);
  pointer_sample.set_position_in_viewport({0.0, 0.0});

  fuchsia::ui::pointer::MouseEvent mouse_event;
  mouse_event.set_timestamp(timestamp++);
  mouse_event.set_pointer_sample(std::move(pointer_sample));

  return mouse_event;
}

fuchsia::ui::pointer::MouseEvent MakeMouseEvent() {
  // clang-format off
  fuchsia::ui::pointer::ViewParameters view_parameters {
    .view = {
      .min = {0.0, 0.0},
      .max = {1.0, 1.0},
    },
    .viewport = {
      .min = {0.0, 0.0},
      .max = {1.0, 1.0},
    },
    .viewport_to_view_transform = {
      1.0, 0.0, 0.0,
      0.0, 1.0, 0.0,
      0.0, 0.0, 1.0,
    }
  };
  // clang-format on

  fuchsia::ui::pointer::MouseDeviceInfo mouse_device_info;
  mouse_device_info.set_id(kMouseDeviceId);
  mouse_device_info.set_buttons({1, 2});

  fuchsia::ui::pointer::MouseEvent mouse_event = MakeMouseUpdate();
  mouse_event.set_view_parameters(std::move(view_parameters));
  mouse_event.set_device_info(std::move(mouse_device_info));

  return mouse_event;
}

// TODO(https://fxbug.dev/434067533): Fix flaky test.
TEST_P(VirtioInputMouseTest, DISABLED_PointerMove) {
  std::array<float, 2> position = {0.25, 0.50};
  fuchsia::ui::pointer::MouseEvent mouse_event = MakeMouseEvent();
  *mouse_event.mutable_pointer_sample()->mutable_position_in_viewport() = position;

  // Right after device initialization it should be waiting on the first Watch call.
  binding_.WaitForMessage();
  RunLoopUntilIdle();
  auto responder = TakeResponder();
  ASSERT_TRUE(responder.has_value());

  // Add event descriptors before we send the mouse events lest the device drop them.
  auto result = AddEventDescriptorsToChain(3);
  ASSERT_TRUE(result.is_ok());
  auto events = result.value();
  zx_status_t status = input_->NotifyQueue(0);
  ASSERT_EQ(ZX_OK, status);

  // Inject the mouse event.
  std::vector<fuchsia::ui::pointer::MouseEvent> mouse_events;
  mouse_events.emplace(mouse_events.end(), std::move(mouse_event));
  (*responder)(std::move(mouse_events));

  // Expect the virtio event.
  status = WaitOnInterrupt();
  ASSERT_EQ(ZX_OK, status);

  // Check for an accurate translation of the mouse event
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[0]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_X, events[0]->code);
  EXPECT_EQ(static_cast<uint32_t>(std::floor(kInputAbsMaxX * position[0])), events[0]->value);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[1]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_Y, events[1]->code);
  EXPECT_EQ(static_cast<uint32_t>(std::floor(kInputAbsMaxY * position[1])), events[1]->value);
  EXPECT_EQ(VIRTIO_INPUT_EV_SYN, events[2]->type);
}

TEST_P(VirtioInputMouseTest, DISABLED_PointerClick) {
  // Right after device initialization it should be waiting on the first Watch call.
  binding_.WaitForMessage();
  RunLoopUntilIdle();
  auto responder = TakeResponder();
  ASSERT_TRUE(responder.has_value());

  // Add event descriptors before we send the mouse events lest the device drop them.
  auto result = AddEventDescriptorsToChain(8);
  ASSERT_TRUE(result.is_ok());
  auto events = result.value();
  zx_status_t status = input_->NotifyQueue(0);
  ASSERT_EQ(ZX_OK, status);

  // Inject left mouse button pressed
  {
    fuchsia::ui::pointer::MouseEvent mouse_event = MakeMouseEvent();
    *mouse_event.mutable_pointer_sample()->mutable_pressed_buttons() = {1};

    std::vector<fuchsia::ui::pointer::MouseEvent> mouse_events;
    mouse_events.emplace(mouse_events.end(), std::move(mouse_event));
    (*responder)(std::move(mouse_events));

    // Expect the virtio event.
    status = WaitOnInterrupt();
    ASSERT_EQ(ZX_OK, status);
  }

  // get next watch request
  binding_.WaitForMessage();
  RunLoopUntilIdle();
  responder = TakeResponder();
  ASSERT_TRUE(responder.has_value());

  // Inject no mouse buttons pressed
  {
    fuchsia::ui::pointer::MouseEvent mouse_event = MakeMouseUpdate();
    *mouse_event.mutable_pointer_sample()->mutable_pressed_buttons() = {};

    std::vector<fuchsia::ui::pointer::MouseEvent> mouse_events;
    mouse_events.emplace(mouse_events.end(), std::move(mouse_event));
    (*responder)(std::move(mouse_events));

    // Expect the virtio event.
    status = WaitOnInterrupt();
    ASSERT_EQ(ZX_OK, status);
  }

  constexpr uint32_t kButtonMouseCode = 0x110;

  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[0]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_X, events[0]->code);
  EXPECT_EQ(0u, events[0]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[1]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_Y, events[1]->code);
  EXPECT_EQ(0u, events[1]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_KEY, events[2]->type);
  EXPECT_EQ(kButtonMouseCode, events[2]->code);
  EXPECT_EQ(VIRTIO_INPUT_EV_KEY_PRESSED, events[2]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_SYN, events[3]->type);

  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[4]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_X, events[4]->code);
  EXPECT_EQ(0u, events[4]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_ABS, events[5]->type);
  EXPECT_EQ(VIRTIO_INPUT_EV_ABS_Y, events[5]->code);
  EXPECT_EQ(0u, events[5]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_KEY, events[6]->type);
  EXPECT_EQ(kButtonMouseCode, events[6]->code);
  EXPECT_EQ(VIRTIO_INPUT_EV_KEY_RELEASED, events[6]->value);

  EXPECT_EQ(VIRTIO_INPUT_EV_SYN, events[7]->type);
}

INSTANTIATE_TEST_SUITE_P(VirtioInputKeyboardComponentsTest, VirtioInputKeyboardTest,
                         testing::Values(VirtioInputTestParam{"statusq", true},
                                         VirtioInputTestParam{"nostatusq", false}),
                         [](const testing::TestParamInfo<VirtioInputTestParam>& info) {
                           return info.param.test_name;
                         });

INSTANTIATE_TEST_SUITE_P(VirtioInputMouseComponentsTest, VirtioInputMouseTest,
                         testing::Values(VirtioInputTestParam{"statusq", true},
                                         VirtioInputTestParam{"nostatusq", false}),
                         [](const testing::TestParamInfo<VirtioInputTestParam>& info) {
                           return info.param.test_name;
                         });

}  // namespace
