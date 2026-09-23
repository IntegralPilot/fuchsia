// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file defines eBPF programs used for tests in Netstack3. Currently
// it has to be compiled manually by running `compile.sh` in the same
// directory as this file.

#include <sys/endian.h>

#include <linux/if_ether.h>

#include "bpf_helpers.h"

// Struct stored in the `state_array` must match the structs defined in
// `ebpf_test_util/src/lib.rs`.
// LINT.IfChange

// Configuration for the test program. If either field is not zero then the
// test program will ignore all packets except UDP packets with the matching
// `src_port` and `dst_port` fields.
struct test_config {
  // Source port to match. Any port matches if set to 0.
  uint16_t src_port;
  // Destination port to match. Any port matches if set to 0.
  uint16_t dst_port;
};

// Struct written by the program in the `state_array` in order to pass results
// back to the test.
struct test_result {
  uint64_t cookie;
  uint32_t uid;
  uint32_t ifindex;
  uint32_t ether_type;
  uint32_t mark;
  uint16_t src_port;
  uint16_t dst_port;
  uint8_t ip_proto;
  uint8_t _padding[3];
};

// Struct stored in the `state_array` to pass configuration from the test to
// the program and results from the program to the test.
struct test_program_state {
  struct test_config config;
  uint8_t _padding[4];
  struct test_result result;
};
// LINT.ThenChange(//src/connectivity/network/testing/ebpf_test_util/src/lib.rs)

SECTION("maps")
struct bpf_map_def state_array = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = sizeof(int),
    .value_size = sizeof(struct test_program_state),
    .max_entries = 1,
    .map_flags = BPF_F_NO_PREALLOC,
};

#define IPPROTO_UDP 17

// Protocol field offset in IPv4 and IPv6 headers.
#define IPV4_PROTO_OFFSET 9
#define IPV6_PROTO_OFFSET 6

static __always_inline inline uint8_t get_ip_proto(struct __sk_buff *skb) {
  uint8_t ip_proto;
  if (skb->protocol == htons(ETH_P_IP)) {
    bpf_skb_load_bytes_relative(skb, IPV4_PROTO_OFFSET, &ip_proto, 1, BPF_HDR_START_NET);
  } else if (skb->protocol == htons(ETH_P_IPV6)) {
    bpf_skb_load_bytes_relative(skb, IPV6_PROTO_OFFSET, &ip_proto, 1, BPF_HDR_START_NET);
  } else {
    return 0;
  }
  return ip_proto;
}

// Extracts `src_port` and `dst_prot` fields from  IPv4 and IPv6 UDP packets.
static __always_inline inline void get_src_dst_port(struct __sk_buff *skb, uint16_t *src_port,
                                                    uint16_t *dst_port) {
  *src_port = 0;
  *dst_port = 0;

  if (get_ip_proto(skb) != IPPROTO_UDP) {
    return;
  }

  uint8_t L4_offset = 0;
  if (skb->protocol == htons(ETH_P_IP)) {
    uint8_t ver_len;
    bpf_skb_load_bytes_relative(skb, 0, &ver_len, 1, BPF_HDR_START_NET);
    L4_offset = (ver_len & 0x0F) * 4;
  } else if (skb->protocol == htons(ETH_P_IPV6)) {
    L4_offset = 40;
  } else {
    return;
  }

  uint16_t port;
  bpf_skb_load_bytes_relative(skb, L4_offset, &port, 2, BPF_HDR_START_NET);
  *src_port = ntohs(port);

  bpf_skb_load_bytes_relative(skb, L4_offset + 2, &port, 2, BPF_HDR_START_NET);
  *dst_port = ntohs(port);
}

int skb_test_prog(struct __sk_buff *skb) {
  int index = 0;
  struct test_program_state *state = bpf_map_lookup_elem(&state_array, &index);
  if (state == 0) {
    // Unexpected: We should always be able to look up first entry in the array.
    return 1;
  }

  uint16_t src_port, dst_port;
  get_src_dst_port(skb, &src_port, &dst_port);

  state->result.src_port = src_port + 1;
  state->result.dst_port = dst_port + 1;

  if ((state->config.src_port != 0 && state->config.src_port != src_port) ||
      (state->config.dst_port != 0 && state->config.dst_port != dst_port)) {
    return 0;
  }

  state->result.uid = bpf_get_socket_uid(skb);
  state->result.cookie = bpf_get_socket_cookie(skb);
  state->result.ether_type = ntohs(skb->protocol);
  state->result.ifindex = skb->ifindex;
  state->result.mark = skb->mark;
  state->result.ip_proto = get_ip_proto(skb);

  return 1;
}
