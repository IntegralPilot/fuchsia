// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_PERIPHERAL_H_
#define SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_PERIPHERAL_H_

#include <stdint.h>

// LINT.IfChange
// Google's USB Vendor ID.
#define GOOGLE_USB_VID 0x18D1

// USB Product ID for Zircon CDC Ethernet Function.
#define GOOGLE_USB_CDC_PID 0xA020

// USB Product ID for Zircon USB Mass Storage Function.
#define GOOGLE_USB_UMS_PID 0xA021

// USB Product ID for Zircon USB Function Test.
#define GOOGLE_USB_FUNCTION_TEST_PID 0xA022

// USB Product ID for CDC Ethernet and Function Test composite device.
#define GOOGLE_USB_CDC_AND_FUNCTION_TEST_PID 0xA023

// USB Product ID for Zircon RNDIS Ethernet Function.
#define GOOGLE_USB_RNDIS_PID 0xA024

// USB Product ID for ADB Function.
#define GOOGLE_USB_ADB_PID 0xA025

// USB Product ID for CDC Ethernet and ADB composite device.
#define GOOGLE_USB_CDC_AND_ADB_PID 0xA026

// USB Product ID for CDC and Fastboot composite device.
#define GOOGLE_USB_CDC_AND_FASTBOOT_PID 0xA027

// USB Product ID for VSOCK Bridge Function.
#define GOOGLE_USB_VSOCK_BRIDGE_PID 0xA028

// USB Product ID for CDC and VSOCK Bridge composite device.
#define GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID 0xA029

// USB Product ID for VSOCK Bridge and ADB composite device.
#define GOOGLE_USB_ADB_AND_VSOCK_BRIDGE_PID 0xA02A

// USB Product ID for CDC Ethernet, VSOCK Bridge, and ADB composite device.
#define GOOGLE_USB_CDC_AND_ADB_AND_VSOCK_BRIDGE_PID 0xA02B

// USB Product ID for CDC Ethernet, ADB, and Fastboot composite device.
#define GOOGLE_USB_CDC_AND_ADB_AND_FASTBOOT_PID 0xA02C

// USB Product ID for Fastboot function
#define GOOGLE_USB_FASTBOOT_PID 0x4ee0
// LINT.ThenChange(//src/devices/usb/bin/usb-cli/src/config.rs)

#endif  // SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_PERIPHERAL_H_
