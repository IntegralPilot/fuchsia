// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-bus/usb-device.h"

#include <fidl/fuchsia.hardware.usb.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.hci/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fit/function.h>

#include <thread>

#include <fbl/ref_counted.h>
#include <usb/request-cpp.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/drivers/usb-bus/tests/common.h"
#include "src/devices/usb/drivers/usb-bus/usb-bus.h"
#include "src/lib/utf_conversion/utf_conversion.h"

namespace usb_bus {

class DeviceTest : public zxtest::Test {
 public:
  auto& get_fidl() { return fidl_; }

  auto& get_device() { return *device_; }

  void SetUp() override {
    auto runtime = fdf_testing::DriverRuntime::GetInstance();
    dispatcher_ =
        std::make_unique<fdf::UnownedSynchronizedDispatcher>(runtime->StartBackgroundDispatcher());
    hci_ = std::make_unique<FakeHci>((*dispatcher_)->async_dispatcher());

    timer_ = fbl::MakeRefCounted<FakeTimer>();
    timer_->set_timeout_handler([=](sync_completion_t* completion, zx_duration_t duration) {
      return sync_completion_wait(completion, duration);
    });

    auto hci_endpoints = fidl::Endpoints<fuchsia_hardware_usb_hci::UsbHci>::Create();
    auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
      auto device = fbl::MakeRefCounted<UsbDevice>(
          root_.get(), ddk::UsbHciProtocolClient(hci_->proto()), std::move(hci_endpoints.client),
          kDeviceId, kHubId, kDeviceSpeed, timer_, (*dispatcher_)->async_dispatcher());
      ASSERT_OK(device->Init((*dispatcher_)->async_dispatcher()));
      device_ = device.get();
    });
    EXPECT_TRUE(result.is_ok());
    auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_device::Device>::Create();
    fidl::BindServer((*dispatcher_)->async_dispatcher(), std::move(endpoints.server), device_);
    fidl_.Bind(std::move(endpoints.client));
  }

  void TearDown() override {
    auto result = fdf::RunOnDispatcherSync((*dispatcher_)->async_dispatcher(), [&]() {
      device_->DdkAsyncRemove();
      mock_ddk::ReleaseFlaggedDevices(root_.get());
    });
    EXPECT_TRUE(result.is_ok());
    hci_.reset();
    dispatcher_.reset();
  }

  void CancelAll() { ASSERT_OK(device_->UsbCancelAll(1)); }

  size_t get_parent_request_size() { return device_->UsbGetRequestSize(); }

  ddk::UsbProtocolClient get_usb_protocol() {
    usb_protocol_t usb;
    device_->DdkGetProtocol(ZX_PROTOCOL_USB, &usb);
    return ddk::UsbProtocolClient(&usb);
  }

  ddk::UsbBusProtocolClient get_usb_bus_protocol() {
    usb_bus_protocol_t usb;
    device_->DdkGetProtocol(ZX_PROTOCOL_USB_BUS, &usb);
    return ddk::UsbBusProtocolClient(&usb);
  }

  void set_custom_control_handling(bool enabled) { hci_->set_custom_control_handling(enabled); }
  usb::BorrowedRequestQueue<void> get_pending_requests() { return hci_->pending_requests(); }
  uint8_t get_configuration() { return hci_->configuration(); }

  void set_enable_endpoint_hook(
      fit::function<zx_status_t(uint32_t device_id, const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable)>
          hook) {
    hci_->set_enable_endpoint_hook(std::move(hook));
  }

  void set_timeout_handler(fit::function<zx_status_t(sync_completion_t*, zx_duration_t)> handler) {
    timer_->set_timeout_handler(std::move(handler));
  }

  bool get_device_reset() { return hci_->device_reset(); }
  uint8_t get_reset_endpoint() { return hci_->reset_endpoint(); }
  void SetEmptyState(bool should_return_empty) { hci_->SetEmptyState(should_return_empty); }

 protected:
  std::shared_ptr<MockDevice> root_{MockDevice::FakeRootParent()};
  fdf_testing::DriverRuntime* runtime() { return fdf_testing::DriverRuntime::GetInstance(); }
  std::unique_ptr<fdf::UnownedSynchronizedDispatcher> dispatcher_;

 private:
  fbl::RefPtr<FakeTimer> timer_;
  fidl::WireSyncClient<fuchsia_hardware_usb_device::Device> fidl_;
  std::unique_ptr<FakeHci> hci_;
  // UsbDevice context pointer owned by us through MockDdk
  UsbDevice* device_;
};

// CancelAll-specific test

TEST_F(DeviceTest, CancelAllCancelsAllRequestsThenReturns) {
  using Request = usb::CallbackRequest<sizeof(max_align_t)>;
  std::atomic<size_t> completed = 0;
  for (size_t i = 0; i < 500; i++) {
    std::optional<Request> request;
    Request::Alloc(&request, 0, 1, get_parent_request_size(),
                   [&](Request request) { completed++; });
    request->Queue(get_usb_protocol());
  }
  CancelAll();
  ASSERT_EQ(completed.load(), 500);
}

// USB protocol tests
TEST_F(DeviceTest, ControlOut) {
  auto usb = get_usb_protocol();
  uint8_t const_data[5];
  for (size_t i = 0; i < 5; i++) {
    const_data[i] = static_cast<uint8_t>(i);
  }
  set_custom_control_handling(true);
  set_timeout_handler([&](sync_completion_t* completion, zx_duration_t duration) {
    EXPECT_EQ(duration, 9001);
    auto requests = get_pending_requests();
    auto request = requests.pop();
    EXPECT_EQ(request->request()->header.ep_address, 0);
    EXPECT_EQ(request->request()->header.length, sizeof(const_data));
    void* mapped_data;
    request->Mmap(&mapped_data);
    EXPECT_EQ(0, memcmp(mapped_data, const_data, sizeof(const_data)));
    EXPECT_EQ(request->request()->setup.bm_request_type, 5);
    EXPECT_EQ(request->request()->setup.b_request, 97);
    EXPECT_EQ(request->request()->setup.w_value, 8);
    EXPECT_EQ(request->request()->setup.w_index, 12);

    request->Complete(ZX_OK, sizeof(const_data));
    return sync_completion_wait(completion, ZX_TIME_INFINITE);
  });
  ASSERT_OK(usb.ControlOut(5, 97, 8, 12, 9001, const_data, sizeof(const_data)));
}

TEST_F(DeviceTest, ControlIn) {
  auto usb = get_usb_protocol();
  uint8_t const_data[5];
  for (size_t i = 0; i < 5; i++) {
    const_data[i] = static_cast<uint8_t>(i);
  }
  set_custom_control_handling(true);
  set_timeout_handler([&](sync_completion_t* completion, zx_duration_t duration) {
    EXPECT_EQ(duration, 9001);
    auto requests = get_pending_requests();
    auto request = requests.pop();
    EXPECT_EQ(request->request()->header.ep_address, 0);
    EXPECT_EQ(request->request()->header.length, sizeof(const_data));
    void* mapped_data;
    request->Mmap(&mapped_data);
    memcpy(mapped_data, const_data, sizeof(const_data));
    EXPECT_EQ(request->request()->setup.bm_request_type, 5 | USB_DIR_IN);
    EXPECT_EQ(request->request()->setup.b_request, 97);
    EXPECT_EQ(request->request()->setup.w_value, 8);
    EXPECT_EQ(request->request()->setup.w_index, 12);

    request->Complete(ZX_OK, sizeof(const_data));
    return sync_completion_wait(completion, ZX_TIME_INFINITE);
  });
  uint8_t buffer[5];
  size_t actual;
  ASSERT_OK(usb.ControlIn(5 | USB_DIR_IN, 97, 8, 12, 9001, buffer, sizeof(buffer), &actual));
  ASSERT_EQ(0, memcmp(buffer, const_data, sizeof(buffer)));
}

TEST_F(DeviceTest, RequestQueue) {
  auto usb = get_usb_protocol();
  using Request = usb::CallbackRequest<sizeof(max_align_t)>;
  std::optional<Request> request;
  sync_completion_t completion;
  usb_request_t* request_ptr;
  Request::Alloc(&request, 0, 1, get_parent_request_size(), [&](Request owned_request) {
    ASSERT_EQ(owned_request.request(), request_ptr);
    sync_completion_signal(&completion);
  });
  request_ptr = request->request();
  request->Queue(usb);
  auto requests = get_pending_requests();
  auto usb_request = requests.pop();
  usb_request->Complete(ZX_OK, 0);
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
}

TEST_F(DeviceTest, GetSpeed) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetSpeed(), kDeviceSpeed);
}

TEST_F(DeviceTest, SetInterface) {
  auto usb = get_usb_protocol();
  set_custom_control_handling(true);
  set_timeout_handler([&](sync_completion_t* completion, zx_duration_t duration) {
    EXPECT_EQ(duration, ZX_TIME_INFINITE);
    auto requests = get_pending_requests();
    auto request = requests.pop();
    EXPECT_EQ(request->request()->header.ep_address, 0);
    EXPECT_EQ(request->request()->setup.bm_request_type,
              USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE);
    EXPECT_EQ(request->request()->setup.b_request, USB_REQ_SET_INTERFACE);
    EXPECT_EQ(request->request()->setup.w_value, 5);
    EXPECT_EQ(request->request()->setup.w_index, 98);
    request->Complete(ZX_OK, 0);
    return sync_completion_wait(completion, ZX_TIME_INFINITE);
  });
  usb.SetInterface(98, 5);
}

TEST_F(DeviceTest, GetConfiguration) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetConfiguration(), 1);
}

TEST_F(DeviceTest, SetConfiguration) {
  auto usb = get_usb_protocol();
  ASSERT_OK(usb.SetConfiguration(2));
  ASSERT_EQ(get_configuration(), 2);
}

TEST_F(DeviceTest, EnableEndpoint) {
  auto usb = get_usb_protocol();
  usb_endpoint_descriptor_t epdesc;
  usb_ss_ep_comp_descriptor_t ss;
  set_enable_endpoint_hook([&](uint32_t device_id, const usb_endpoint_descriptor_t* ep_desc,
                               const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable) {
    EXPECT_EQ(device_id, kDeviceId);
    EXPECT_EQ(ep_desc, &epdesc);
    EXPECT_EQ(ss_com_desc, &ss);
    EXPECT_TRUE(enable);
    return ZX_OK;
  });
  ASSERT_OK(usb.EnableEndpoint(&epdesc, &ss, true));
}

TEST_F(DeviceTest, ResetEndpoint) {
  auto usb = get_usb_protocol();
  ASSERT_OK(usb.ResetEndpoint(97));
  ASSERT_EQ(get_reset_endpoint(), 97);
}

TEST_F(DeviceTest, ResetDevice) {
  auto usb = get_usb_protocol();
  ASSERT_OK(usb.ResetDevice());
  ASSERT_TRUE(get_device_reset());
}

TEST_F(DeviceTest, GetMaxTransferSize) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetMaxTransferSize(kTransferSizeEndpoint), kMaxTransferSize);
}

TEST_F(DeviceTest, GetDeviceId) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetDeviceId(), kDeviceId);
}

TEST_F(DeviceTest, GetDeviceDescriptor) {
  auto usb = get_usb_protocol();
  usb_device_descriptor_t descriptor;
  usb.GetDeviceDescriptor(&descriptor);
  ASSERT_EQ(descriptor.id_vendor, kVendorId);
  ASSERT_EQ(descriptor.id_product, kProductId);
  ASSERT_EQ(descriptor.b_device_class, kDeviceClass);
  ASSERT_EQ(descriptor.b_device_sub_class, kDeviceSubclass);
  ASSERT_EQ(descriptor.b_device_protocol, kDeviceProtocol);
}

TEST_F(DeviceTest, GetConfigurationDescriptorLength) {
  auto usb = get_usb_protocol();
  size_t length;
  ASSERT_OK(usb.GetConfigurationDescriptorLength(1, &length));
  ASSERT_EQ(length, sizeof(usb_configuration_descriptor_t));
}

TEST_F(DeviceTest, GetConfigurationDescriptor) {
  auto usb = get_usb_protocol();
  usb_configuration_descriptor_t descriptor;
  size_t actual;
  ASSERT_OK(usb.GetConfigurationDescriptor(1, reinterpret_cast<uint8_t*>(&descriptor),
                                           sizeof(descriptor), &actual));
  ASSERT_EQ(actual, sizeof(descriptor));
  ASSERT_EQ(descriptor.b_configuration_value, 1);
  ASSERT_EQ(descriptor.w_total_length, sizeof(descriptor));
}

TEST_F(DeviceTest, GetDescriptorsLength) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetDescriptorsLength(), sizeof(usb_configuration_descriptor_t));
}

TEST_F(DeviceTest, GetDescriptors) {
  auto usb = get_usb_protocol();
  usb_configuration_descriptor_t descriptor;
  size_t actual;
  usb.GetDescriptors(reinterpret_cast<uint8_t*>(&descriptor), sizeof(descriptor), &actual);
  ASSERT_EQ(actual, sizeof(descriptor));
  ASSERT_EQ(descriptor.b_configuration_value, 1);
  ASSERT_EQ(descriptor.w_total_length, sizeof(descriptor));
}

TEST_F(DeviceTest, GetCurrentFrame) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetCurrentFrame(), kCurrentFrame);
}

TEST_F(DeviceTest, GetRequestSize) {
  auto usb = get_usb_protocol();
  ASSERT_EQ(usb.GetRequestSize(), kRequestSize);
}

TEST_F(DeviceTest, FidlGetSpeed) {
  auto& fidl = get_fidl();
  auto result = fidl->GetDeviceSpeed();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result.value().speed, kDeviceSpeed);
}

TEST_F(DeviceTest, FidlGetDescriptor) {
  auto& fidl = get_fidl();
  auto result = fidl->GetDeviceDescriptor();
  usb_device_descriptor_t* descriptor =
      reinterpret_cast<usb_device_descriptor_t*>(result.value().desc.data());
  ASSERT_EQ(descriptor->id_vendor, kVendorId);
  ASSERT_EQ(descriptor->id_product, kProductId);
  ASSERT_EQ(descriptor->b_device_class, kDeviceClass);
  ASSERT_EQ(descriptor->b_device_sub_class, kDeviceSubclass);
  ASSERT_EQ(descriptor->b_device_protocol, kDeviceProtocol);
}

TEST_F(DeviceTest, FidlGetConfigurationDescriptorSize) {
  auto& fidl = get_fidl();
  auto result = fidl->GetConfigurationDescriptorSize(1);
  ASSERT_TRUE(result.ok());
  ASSERT_OK(result.value().s);
  ASSERT_EQ(result.value().size, sizeof(usb_configuration_descriptor_t));
}

TEST_F(DeviceTest, FidlGetConfigurationDescriptorSize_Nullptr) {
  auto& fidl = get_fidl();
  auto result = fidl->GetConfigurationDescriptorSize(99);  // <-- intentionally bad index.
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result.value().s, ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(result.value().size, 0);
}

TEST_F(DeviceTest, FidlGetConfigurationDescriptor) {
  auto& fidl = get_fidl();
  const usb_configuration_descriptor_t* descriptor;
  auto result = fidl->GetConfigurationDescriptor(1);
  ASSERT_TRUE(result.ok());
  ASSERT_OK(result.value().s);
  ASSERT_EQ(result.value().desc.size(), sizeof(*descriptor));
  descriptor = reinterpret_cast<const usb_configuration_descriptor_t*>(result.value().desc.data());
  ASSERT_EQ(descriptor->b_configuration_value, 1);
  ASSERT_EQ(descriptor->w_total_length, sizeof(*descriptor));
}

TEST_F(DeviceTest, FidlGetStringDescriptor_Empty) {
  auto& fidl = get_fidl();
  char golden[128];
  size_t dest_len = 128;
  utf16_to_utf8(reinterpret_cast<const uint16_t*>(kStringDescriptors[0][0]), 7,
                reinterpret_cast<uint8_t*>(golden), &dest_len);
  SetEmptyState(true);
  auto result = fidl->GetStringDescriptor(1, MakeConstant<uint16_t, 2>("EN"));
  ASSERT_TRUE(result.value().desc.empty());
  ASSERT_EQ(result.value().s, ZX_ERR_INTERNAL);
}

TEST_F(DeviceTest, FidlGetStringDescriptor) {
  auto& fidl = get_fidl();
  char golden[128];
  {
    size_t dest_len = 128;
    utf16_to_utf8(reinterpret_cast<const uint16_t*>(kStringDescriptors[0][0]), 7,
                  reinterpret_cast<uint8_t*>(golden), &dest_len);
    auto expected = MakeConstant<uint16_t, 2>("EN");
    auto result = fidl->GetStringDescriptor(1, MakeConstant<uint16_t, 2>("EN"));
    ASSERT_TRUE(result.ok());
    ASSERT_OK(result.value().s);
    ASSERT_EQ(result.value().actual_lang_id, expected);
    ASSERT_EQ(result.value().desc.size(), dest_len);  // this is way off for some reason
    ASSERT_EQ(memcmp(result.value().desc.data(), golden, dest_len), 0);
  }

  {
    size_t dest_len = 128;
    utf16_to_utf8(reinterpret_cast<const uint16_t*>(kStringDescriptors[0][1]), 6,
                  reinterpret_cast<uint8_t*>(golden), &dest_len);
    auto expected = MakeConstant<uint16_t, 2>("ES");
    auto result = fidl->GetStringDescriptor(1, MakeConstant<uint16_t, 2>("ES"));
    ASSERT_TRUE(result.ok());
    ASSERT_OK(result.value().s);
    ASSERT_EQ(result.value().actual_lang_id, expected);
    ASSERT_EQ(result.value().desc.size(), dest_len);
    ASSERT_EQ(memcmp(result.value().desc.data(), golden, dest_len), 0);
  }

  {
    size_t dest_len = 128;
    utf16_to_utf8(reinterpret_cast<const uint16_t*>(kStringDescriptors[1][0]), 6,
                  reinterpret_cast<uint8_t*>(golden), &dest_len);
    auto expected = MakeConstant<uint16_t, 2>("EN");
    auto result = fidl->GetStringDescriptor(2, MakeConstant<uint16_t, 2>("EN"));
    ASSERT_TRUE(result.ok());
    ASSERT_OK(result.value().s);
    ASSERT_EQ(result.value().actual_lang_id, expected);
    ASSERT_EQ(result.value().desc.size(), dest_len);
    ASSERT_EQ(memcmp(result.value().desc.data(), golden, dest_len), 0);
  }

  {
    size_t dest_len = 128;
    utf16_to_utf8(reinterpret_cast<const uint16_t*>(kStringDescriptors[1][1]), 11,
                  reinterpret_cast<uint8_t*>(golden), &dest_len);
    auto expected = MakeConstant<uint16_t, 2>("ES");
    auto result = fidl->GetStringDescriptor(2, MakeConstant<uint16_t, 2>("ES"));
    ASSERT_TRUE(result.ok());
    ASSERT_OK(result.value().s);
    ASSERT_EQ(result.value().actual_lang_id, expected);
    ASSERT_EQ(result.value().desc.size(), dest_len);
    ASSERT_EQ(memcmp(result.value().desc.data(), golden, dest_len), 0);
  }
}

TEST_F(DeviceTest, UsbGetStringDescriptor_BufferTooSmall) {
  auto& device = get_device();
  uint16_t lang_id[2];
  uint8_t desc[128];
  size_t actual;

  // The value here is intentionally chosen to be undersized.
  size_t small = 3;

  zx_status_t status = device.UsbGetStringDescriptor(
      1, 1, lang_id, reinterpret_cast<uint8_t*>(&desc), small, &actual);

  EXPECT_EQ(status, ZX_ERR_BUFFER_TOO_SMALL);
  EXPECT_GT(actual, small);
}

TEST_F(DeviceTest, FidlSetInterface) {
  auto& fidl = get_fidl();
  set_custom_control_handling(true);
  set_timeout_handler([&](sync_completion_t* completion, zx_duration_t duration) {
    EXPECT_EQ(duration, ZX_TIME_INFINITE);
    auto requests = get_pending_requests();
    auto request = requests.pop();
    EXPECT_EQ(request->request()->header.ep_address, 0);
    EXPECT_EQ(request->request()->setup.bm_request_type,
              USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE);
    EXPECT_EQ(request->request()->setup.b_request, USB_REQ_SET_INTERFACE);
    EXPECT_EQ(request->request()->setup.w_value, 5);
    EXPECT_EQ(request->request()->setup.w_index, 98);
    request->Complete(ZX_OK, 0);
    return sync_completion_wait(completion, ZX_TIME_INFINITE);
  });
  auto result = fidl->SetInterface(98, 5);
  ASSERT_TRUE(result.ok());
  ASSERT_OK(result.value().s);
}

TEST_F(DeviceTest, FidlGetDeviceId) {
  auto& fidl = get_fidl();
  auto result = fidl->GetDeviceId();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result.value().device_id, kDeviceId);
}

TEST_F(DeviceTest, FidlGetHubDeviceId) {
  auto& fidl = get_fidl();
  auto result = fidl->GetHubDeviceId();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result.value().hub_device_id, kHubId);
}

TEST_F(DeviceTest, FidlGetConfiguration) {
  auto& fidl = get_fidl();
  auto result = fidl->GetConfiguration();
  ASSERT_TRUE(result.ok());
  ASSERT_EQ(result.value().configuration, 1);
}

TEST_F(DeviceTest, FidlSetConfiguration) {
  auto& fidl = get_fidl();
  auto result = fidl->SetConfiguration(2);
  ASSERT_TRUE(result.ok());
  ASSERT_OK(result.value().s);
  ASSERT_EQ(get_configuration(), 2);
}

// A fake HCI that pretends to be a device that does dodgy things with
// configuration descriptors: namely, changing the size they claim to be
// depending on how many requests for config descriptors have been made
// previously.
class EvilFakeHci : public FakeHci {
 public:
  EvilFakeHci(async_dispatcher_t* dispatcher, uint16_t initial_config_length,
              uint16_t subsequent_config_length)
      : FakeHci(dispatcher),
        initial_config_length_(initial_config_length),
        subsequent_config_length_(subsequent_config_length) {}

  void UsbHciRequestQueue(usb_request_t* usb_request_,
                          const usb_request_complete_callback_t* complete_cb_) override {
    usb::BorrowedRequest<void> request(usb_request_, *complete_cb_, sizeof(usb_request_t));
    EXPECT_EQ(request.request()->header.ep_address, 0);
    EXPECT_EQ(request.request()->setup.bm_request_type,
              USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
    EXPECT_EQ(request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);

    if (request.request()->header.ep_address == 0) {
      if ((request.request()->setup.bm_request_type ==
           (USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)) &&
          (request.request()->setup.b_request == USB_REQ_GET_DESCRIPTOR)) {
        uint8_t type = static_cast<uint8_t>(request.request()->setup.w_value >> 8);
        uint8_t index = static_cast<uint8_t>(request.request()->setup.w_value);
        switch (type) {
          case USB_DT_DEVICE: {
            usb_device_descriptor_t* descriptor;
            request.Mmap(reinterpret_cast<void**>(&descriptor));
            descriptor->b_num_configurations = 2;
            descriptor->id_vendor = kVendorId;
            descriptor->id_product = kProductId;
            descriptor->b_device_class = kDeviceClass;
            descriptor->b_device_sub_class = kDeviceSubclass;
            descriptor->b_device_protocol = kDeviceProtocol;
            request.Complete(ZX_OK, sizeof(*descriptor));
          }
            return;
          case USB_DT_CONFIG: {
            usb_configuration_descriptor_t* descriptor;
            request.Mmap(reinterpret_cast<void**>(&descriptor));
            // Use the config descriptor lengths described in the constructor
            // arguments.
            descriptor->w_total_length =
                (config_descriptor_request_count_ % 2 == 0 ? initial_config_length_
                                                           : subsequent_config_length_);
            config_descriptor_request_count_++;
            descriptor->b_configuration_value = static_cast<uint8_t>(index + 1);
            request.Complete(ZX_OK, sizeof(*descriptor));
          }
            return;
        }
      }

      // The host should not send us any requests (like attempting to set a configuration)
      // after we do questionable things with wTotalLength.
      request.Complete(ZX_ERR_INVALID_ARGS, 0);
      return;
    }
    pending_requests_.push(std::move(request));
  }

 private:
  int config_descriptor_request_count_ = 0;
  uint16_t initial_config_length_;
  uint16_t subsequent_config_length_;
  usb::BorrowedRequestQueue<void> pending_requests_;
};

class EvilDeviceTest : public zxtest::Test {
 public:
  EvilDeviceTest() : hci_(nullptr) {}

  void SetUp() override {
    auto runtime = fdf_testing::DriverRuntime::GetInstance();
    dispatcher_ =
        std::make_unique<fdf::UnownedSynchronizedDispatcher>(runtime->StartBackgroundDispatcher());
  }

  void TearDown() override { dispatcher_.reset(); }

 protected:
  std::shared_ptr<MockDevice> root_{MockDevice::FakeRootParent()};
  fdf_testing::DriverRuntime* runtime() { return fdf_testing::DriverRuntime::GetInstance(); }
  std::unique_ptr<fdf::UnownedSynchronizedDispatcher> dispatcher_;
  std::unique_ptr<EvilFakeHci> hci_;
};

TEST_F(EvilDeviceTest, GetConfigurationDescriptorTooShortRejected) {
  // We expect this device to fail to initialize because wTotalLength is too
  // short -- 1 byte is shorter than the minimal config descriptor length, so
  // such a response is invalid.
  hci_ = std::make_unique<EvilFakeHci>((*dispatcher_)->async_dispatcher(), 1, 1);
  fbl::RefPtr<FakeTimer> timer = fbl::MakeRefCounted<FakeTimer>();
  timer->set_timeout_handler([=](sync_completion_t* completion, zx_duration_t duration) {
    return sync_completion_wait(completion, duration);
  });

  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_hci::UsbHci>::Create();
  auto device = fbl::MakeRefCounted<UsbDevice>(
      root_.get(), ddk::UsbHciProtocolClient(hci_->proto()), std::move(endpoints.client), kDeviceId,
      kHubId, kDeviceSpeed, timer, loop.dispatcher());
  auto result = device->Init(loop.dispatcher());
  ASSERT_EQ(result, ZX_ERR_IO);
}

TEST_F(EvilDeviceTest, GetConfigurationDescriptorDifferentSizesAreRejected) {
  // We expect this device to fail to initialize because when we request its
  // configuration descriptors, the wTotalSize value we get back changes between
  // the first (size-fetching) request and second (full descriptor-fetching) request.
  hci_ = std::make_unique<EvilFakeHci>((*dispatcher_)->async_dispatcher(),
                                       sizeof(usb_configuration_descriptor_t), 65535);
  fbl::RefPtr<FakeTimer> timer = fbl::MakeRefCounted<FakeTimer>();
  timer->set_timeout_handler([=](sync_completion_t* completion, zx_duration_t duration) {
    return sync_completion_wait(completion, duration);
  });

  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  std::shared_ptr<MockDevice> root = MockDevice::FakeRootParent();
  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_hci::UsbHci>();
  ASSERT_OK(endpoints);
  auto device = fbl::MakeRefCounted<UsbDevice>(root.get(), ddk::UsbHciProtocolClient(hci_->proto()),
                                               std::move(endpoints->client), kDeviceId, kHubId,
                                               kDeviceSpeed, timer, loop.dispatcher());
  auto result = device->Init(loop.dispatcher());
  ASSERT_EQ(result, ZX_ERR_IO);
}
}  // namespace usb_bus
