// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_COMPOSITE_TEST_HELPER_H_
#define SRC_DEVICES_USB_DRIVERS_USB_COMPOSITE_TEST_HELPER_H_

#include <fuchsia/hardware/usb/cpp/banjo-mock.h>

// Some test utilities in "fuchsia/hardware/usb/function/cpp/banjo-mock.h" expect the following
// operators to be implemented.
bool operator==(const usb_request_complete_callback_t& lhs,
                const usb_request_complete_callback_t& rhs) {
  // Comparison of these struct is not useful. Return true always.
  return true;
}

bool operator==(const usb_ss_ep_comp_descriptor_t& lhs, const usb_ss_ep_comp_descriptor_t& rhs) {
  // Comparison of these struct is not useful. Return true always.
  return true;
}

bool operator==(const usb_endpoint_descriptor_t& lhs, const usb_endpoint_descriptor_t& rhs) {
  // Comparison of these struct is not useful. Return true always.
  return true;
}

bool operator==(const usb_request_t& lhs, const usb_request_t& rhs) {
  // Only comparing endpoint address. Use ExpectCallWithMatcher for more specific
  // comparisons.
  return lhs.header.ep_address == rhs.header.ep_address;
}

namespace usb_composite {

class MockUsb : public ddk::MockUsb {
 public:
  zx_status_t UsbEnableEndpoint(const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc,
                                bool enable) override {
    std::tuple<zx_status_t> ret = mock_enable_endpoint_.Call(
        ep_desc ? *ep_desc : usb_endpoint_descriptor_t{},
        ss_com_desc ? *ss_com_desc : usb_ss_ep_comp_descriptor_t{}, enable);
    return std::get<0>(ret);
  }
};

}  // namespace usb_composite

#endif  // SRC_DEVICES_USB_DRIVERS_USB_COMPOSITE_TEST_HELPER_H_
