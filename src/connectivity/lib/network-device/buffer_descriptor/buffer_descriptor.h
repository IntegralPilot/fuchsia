// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_LIB_NETWORK_DEVICE_BUFFER_DESCRIPTOR_BUFFER_DESCRIPTOR_H_
#define SRC_CONNECTIVITY_LIB_NETWORK_DEVICE_BUFFER_DESCRIPTOR_BUFFER_DESCRIPTOR_H_

#include <stdint.h>

// Nomenclature:
// Inbound = Device->Client on Rx, Client->Device on Tx
// Return = Client->Device on Rx, Device->Client on Tx

// Flags and constants are found on the definition of the
// fuchsia.hardware.network FIDL library.

// LINT.IfChange

// TODO(https://github.com/rust-lang/rust-bindgen/issues/316): Remove redundant
// definition when Rust bindgen can handle this.
#define __NETWORK_DEVICE_DESCRIPTOR_VERSION (1)
#define NETWORK_DEVICE_DESCRIPTOR_VERSION ((uint8_t)__NETWORK_DEVICE_DESCRIPTOR_VERSION)

// A buffer descriptor, which contains a region of the data VMO that can be used
// to store data plus associated metadata.
//
// The region of the VMO described by a buffer descriptor has the form:
// | head | data | tail |.
//
// The three regions have lengths |head_length|, |data_length|, |tail_length|
// and always start |offset| bytes from the start of the data VMO.
//
// A data-carrying buffer's payload is ALWAYS in the |data| region of the
// described memory space, but the owner of the buffer is always guaranteed (by
// the FIFO contract) to be the sole accessor of the entire allocated region.
//
// When descriptors are chained using |chain_length| and |nxt|, |head_length|
// may only be different than 0 for the first buffer and |tail_length| may only
// be different than 0 for the last buffer.
typedef struct buffer_descriptor {
  // Frame type, as defined by NetworkDevice FIDL.
  uint8_t frame_type;
  // The number of following descriptors in the linked list started at |next|.
  uint8_t chain_length;
  // The index of the next descriptor to use, ignored if |chain_length| is 0.
  uint16_t nxt;

  // Hardware acceleration metadata.
  union accel_metadata {
    // Partial checksum metadata.
    struct tx_partial_csum {
      // The offset in bytes from the start of the frame to where the device
      // must begin computing the checksum.
      uint16_t start;
      // The offset in bytes from the start of the checksummed portion of the
      // buffer to where the device must place the checksum result. The checksum
      // field itself is included in the checksum.
      uint16_t offset;
    } tx_partial_csum;
    // The number of full checksums, minus one, verified by the device.
    uint16_t rx_full_csums_verified;
  } accel_metadata;

  // Frame's device port identifier.
  struct port_id {
    // Base identifier.
    uint8_t base;
    // Port identifier salt. Frame must be discarded if salt information doesn't
    // match current port expectations.
    uint8_t salt;
  } port_id;

  // Data VMO ID where this buffer resides.
  uint8_t vmo_id;

  // Reserved for future expansion. Maintains 64-bit word alignment.
  uint8_t _reserved;

  // Scratch space for the client, the device must not modify this field or
  // depend on its value.
  uint8_t client_opaque_data[4];

  // Buffer offset in data VMO
  uint64_t offset;

  // Offset of payload in VMO region.
  //
  // For data-carrying buffer, the payload always starts |head_length| after
  // the VMO |offset|.
  uint16_t head_length;
  // The number of bytes available at the end of this buffer after
  // |data_length|.
  uint16_t tail_length;
  // Length of data written in VMO buffer, in bytes.
  //
  // Set by Client on Tx and by Device on Rx for data-carrying buffers. For
  // scratch Rx space buffers, |data_length| is the total available space,
  // starting from |head_length|.
  uint32_t data_length;

  // Inbound flags, set by Client on Tx and by Server on Rx.
  uint32_t inbound_flags;
  // Return flags, set by Client on Rx and by Server on Tx.
  uint32_t return_flags;
} buffer_descriptor_t;

// Notify humans to update Rust bindings because there's no bindgen automation.
// TODO(https://fxbug.dev/42153476): Remove lint when no longer necessary.
// LINT.ThenChange(/src/connectivity/lib/network-device/rust/src/session/buffer/sys.rs)

#endif  // SRC_CONNECTIVITY_LIB_NETWORK_DEVICE_BUFFER_DESCRIPTOR_BUFFER_DESCRIPTOR_H_
