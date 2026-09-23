// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <linux/bpf.h>

static void *(*const bpf_map_lookup_elem)(void *map,
                                          const void *key) = (void *)BPF_FUNC_map_lookup_elem;
static long (*const bpf_map_update_elem)(void *map, const void *key, const void *value,
                                         __u64 flags) = (void *)BPF_FUNC_map_update_elem;
static long (*const bpf_map_delete_elem)(void *map,
                                         const void *key) = (void *)BPF_FUNC_map_delete_elem;
static __u64 (*bpf_get_socket_cookie)(void *context) = (void *)BPF_FUNC_get_socket_cookie;
static __u32 (*bpf_get_socket_uid)(struct __sk_buff *skb) = (void *)BPF_FUNC_get_socket_uid;
static int (*bpf_skb_load_bytes_relative)(const struct __sk_buff *skb, int off, void *to, int len,
                                          int start_hdr) = (void *)BPF_FUNC_skb_load_bytes_relative;
static void *(*const bpf_ringbuf_reserve)(void *ringbuf, __u64 size,
                                          __u64 flags) = (void *)BPF_FUNC_ringbuf_reserve;
static void (*const bpf_ringbuf_submit)(void *data, __u64 flags) = (void *)BPF_FUNC_ringbuf_submit;
static void (*const bpf_ringbuf_discard)(void *data,
                                         __u64 flags) = (void *)BPF_FUNC_ringbuf_discard;
static struct bpf_sock *(*const bpf_sk_fullsock)(struct bpf_sock *sock) = (void *)
    BPF_FUNC_sk_fullsock;
static __u64 (*const bpf_get_current_uid_gid)() = (void *)BPF_FUNC_get_current_uid_gid;
static __u64 (*const bpf_get_current_pid_tgid)() = (void *)BPF_FUNC_get_current_pid_tgid;
static int (*const bpf_set_retval)(int retval) = (void *)BPF_FUNC_set_retval;
static int (*const bpf_get_retval)() = (void *)BPF_FUNC_get_retval;
static void *(*const bpf_sk_storage_get)(void *map, void *sk, void *value,
                                         __u64 flags) = (void *)BPF_FUNC_sk_storage_get;

struct bpf_map_def {
  enum bpf_map_type type;
  unsigned int key_size;
  unsigned int value_size;
  unsigned int max_entries;
  unsigned int map_flags;
};

#define BPF_HDR_START_NET 1
#define BPF_SK_STORAGE_GET_F_CREATE 1
#define BPF_F_NO_PREALLOC 1

#define SECTION(name) __attribute__((section(name), used))
