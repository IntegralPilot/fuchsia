// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bpf_helpers.h"

static volatile __u64 static_variable = 0;

SECTION("maps")
struct bpf_map_def hashmap = {
    .type = BPF_MAP_TYPE_HASH,
    .key_size = sizeof(int),
    .value_size = sizeof(__u64),
    .max_entries = 100,
    .map_flags = BPF_F_NO_PREALLOC,
};

SECTION("maps")
struct bpf_map_def array = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = sizeof(int),
    .value_size = sizeof(int),
    .max_entries = 2,
    .map_flags = BPF_F_NO_PREALLOC,
};

int test_prog(struct __sk_buff *skb) {
  __sync_fetch_and_add(&static_variable, 1);

  int ifindex = skb->ifindex;

  __u64 *count = bpf_map_lookup_elem(&hashmap, &ifindex);
  if (count) {
    *count += 1;
  } else {
    __u64 n = 1;
    bpf_map_update_elem(&hashmap, &ifindex, &n, 0);
  }

  int index = 2;
  bpf_map_update_elem(&array, &index, &index, 0);

  return 1;
}
