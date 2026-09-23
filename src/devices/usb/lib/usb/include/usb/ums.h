// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_UMS_H_
#define SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_UMS_H_

// clang-format off

// control request values
#define USB_REQ_RESET               0xFF
#define USB_REQ_GET_MAX_LUN         0xFE

// error codes for CSW processing
typedef uint32_t csw_status_t;
#define CSW_SUCCESS      ((csw_status_t)0)
#define CSW_FAILED       ((csw_status_t)1)
#define CSW_PHASE_ERROR  ((csw_status_t)2)
#define CSW_INVALID      ((csw_status_t)3)
#define CSW_TAG_MISMATCH ((csw_status_t)4)

// signatures in header and status
#define CBW_SIGNATURE               0x43425355
#define CSW_SIGNATURE               0x53425355

// transfer lengths
#define UMS_INQUIRY_TRANSFER_LENGTH                0x24
#define UMS_REQUEST_SENSE_TRANSFER_LENGTH          0x12
#define UMS_READ_FORMAT_CAPACITIES_TRANSFER_LENGTH 0xFC

// Command Block Wrapper
typedef struct {
    uint32_t    dCBWSignature;      // CBW_SIGNATURE
    uint32_t    dCBWTag;
    uint32_t    dCBWDataTransferLength;
    uint8_t     bmCBWFlags;
    uint8_t     bCBWLUN;
    uint8_t     bCBWCBLength;
    uint8_t     CBWCB[16];
} __PACKED ums_cbw_t;
static_assert(sizeof(ums_cbw_t) == 31, "");

// Command Status Wrapper
typedef struct {
    uint32_t    dCSWSignature;      // CSW_SIGNATURE
    uint32_t    dCSWTag;
    uint32_t    dCSWDataResidue;
    uint8_t     bmCSWStatus;
} __PACKED ums_csw_t;
static_assert(sizeof(ums_csw_t) == 13, "");

#endif  // SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_UMS_H_
