// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef VIRTIO_NET_H_
#define VIRTIO_NET_H_

#include <stdint.h>
#include <zircon/compiler.h>

// clang-format off

#define VIRTIO_NET_F_CSUM                   ((uint64_t)1 << 0)
#define VIRTIO_NET_F_GUEST_CSUM             ((uint64_t)1 << 1)
#define VIRTIO_NET_F_CNTRL_GUEST_OFFLOADS   ((uint64_t)1 << 2)
#define VIRTIO_NET_F_MAC                    ((uint64_t)1 << 5)
#define VIRTIO_NET_F_GSO                    ((uint64_t)1 << 6)
#define VIRTIO_NET_F_GUEST_TSO4             ((uint64_t)1 << 7)
#define VIRTIO_NET_F_GUEST_TSO6             ((uint64_t)1 << 8)
#define VIRTIO_NET_F_GUEST_ECN              ((uint64_t)1 << 9)
#define VIRTIO_NET_F_GUEST_UFO              ((uint64_t)1 << 10)
#define VIRTIO_NET_F_HOST_TSO4              ((uint64_t)1 << 11)
#define VIRTIO_NET_F_HOST_TSO6              ((uint64_t)1 << 12)
#define VIRTIO_NET_F_HOST_ECN               ((uint64_t)1 << 13)
#define VIRTIO_NET_F_HOST_UFO               ((uint64_t)1 << 14)
#define VIRTIO_NET_F_MRG_RXBUF              ((uint64_t)1 << 15)
#define VIRTIO_NET_F_STATUS                 ((uint64_t)1 << 16)
#define VIRTIO_NET_F_CTRL_VQ                ((uint64_t)1 << 17)
#define VIRTIO_NET_F_CTRL_RX                ((uint64_t)1 << 18)
#define VIRTIO_NET_F_CTRL_VLAN              ((uint64_t)1 << 19)
#define VIRTIO_NET_F_GUEST_ANNOUNCE         ((uint64_t)1 << 21)
#define VIRTIO_NET_F_MQ                     ((uint64_t)1 << 22)
#define VIRTIO_NET_F_CTRL_MAC_ADDR          ((uint64_t)1 << 23)

#define VIRTIO_NET_HDR_F_NEEDS_CSUM 1u

#define VIRTIO_NET_HDR_GSO_NONE     0u
#define VIRTIO_NET_HDR_GSO_TCPV4    1u
#define VIRTIO_NET_HDR_GSO_UDP      3u
#define VIRTIO_NET_HDR_GSO_TCPV6    4u
#define VIRTIO_NET_HDR_GSO_ECN      0x80u

#define VIRTIO_NET_S_LINK_UP        1u
#define VIRTIO_NET_S_ANNOUNCE       2u

// clang-format on

__BEGIN_CDECLS

#define VIRTIO_ETH_MAC_SIZE 6

typedef struct virtio_net_config {
  uint8_t mac[VIRTIO_ETH_MAC_SIZE];
  uint16_t status;
  uint16_t max_virtqueue_pairs;
} __PACKED virtio_net_config_t;

typedef struct virtio_legacy_net_hdr {
  uint8_t flags;
  uint8_t gso_type;
  uint16_t hdr_len;
  uint16_t gso_size;
  uint16_t csum_start;
  uint16_t csum_offset;
} __PACKED virtio_legacy_net_hdr_t;

// Only if |VIRTIO_NET_F_MRG_RXBUF| or |VIRTIO_F_VERSION_1|.
typedef struct virtio_net_hdr {
  virtio_legacy_net_hdr_t base;
  uint16_t num_buffers;
} __PACKED virtio_net_hdr_t;

__END_CDECLS

#endif  // VIRTIO_NET_H_
