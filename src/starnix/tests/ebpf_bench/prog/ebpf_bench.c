// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file defines some simple programs that are used to benchmark performance
// of the eBPF runtime.
//
// Currently this file has to be compiled manually by running `compile.sh`
// in the same directory.

#include "bpf_helpers.h"

__u64 compute(struct __sk_buff* skb) {
  int buf[100];
  for (unsigned int i = 0; i < 100; i++) {
    buf[i] = i * (i + skb->len);
  }
  __u64 r = 0;
  for (unsigned int i = 0; i < 50; i++) {
    int index = (i * (i - 1)) % 100;
    r += buf[index];
  }
  return r;
}

SECTION("maps")
struct bpf_map_def hashmap = {
    .type = BPF_MAP_TYPE_HASH,
    .key_size = sizeof(int),
    .value_size = sizeof(__u64),
    .max_entries = 10000,
    .map_flags = BPF_F_NO_PREALLOC,
};

__u64 hash_map(struct __sk_buff* skb) {
  for (int i = 0; i < 10; ++i) {
    __u64 value = i + 2;
    bpf_map_update_elem(&hashmap, &i, &value, 0);
  }

  __u64 r = 0;
  for (int i = 0; i < 10; ++i) {
    __u64* value = bpf_map_lookup_elem(&hashmap, &i);
    if (value) {
      r += *value;
    }
    int missing_key = i + 200;
    bpf_map_lookup_elem(&hashmap, &missing_key);
  }

  for (int i = 0; i < 10; ++i) {
    bpf_map_delete_elem(&hashmap, &i);
  }

  return r;
}
