// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bpf_helpers.h"

const int SO_SNDBUF = 7;

SECTION("maps")
struct bpf_map_def ringbuf = {
    .type = BPF_MAP_TYPE_RINGBUF,
    .key_size = 0,
    .value_size = 0,
    .max_entries = 4096,
    .map_flags = 0,
};

SECTION("maps")
struct bpf_map_def target_cookie = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = 4,
    .value_size = 8,
    .max_entries = 1,
    .map_flags = 0,
};

SECTION("maps")
struct bpf_map_def count = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = 4,
    .value_size = sizeof(int),
    .max_entries = 1,
    .map_flags = 0,
};

// The following definitions should match the values use by the tests.

// LINT.IfChange
// Struct stored in the `test_result` map in order to pass results to the test.
struct test_result {
  __u64 uid_gid;
  __u64 pid_tgid;

  __u64 optlen;
  __u64 optval_size;
  __u32 retval;
  __u32 get_retval;

  __u32 ether_type;
  __u32 ifindex;

  __u32 sockaddr_family;
  __u32 sockaddr_port;
  __u32 sockaddr_ip[4];

  __u32 sk_type;
  __u32 sk_protocol;
  __u32 sk_family;
  __u32 sk_state;
};

// Global variable that will be stored in a .data section (which is a BPF map).
static volatile struct {
  __u64 global_counter1;
  __u64 global_counter2;
} kGlobal = {};

const int TEST_SOCK_OPT = 12345;
// LINT.ThenChange(//src/starnix/tests/syscalls/rust/src/ebpf.rs)

SECTION("maps")
struct bpf_map_def test_result = {
    .type = BPF_MAP_TYPE_ARRAY,
    .key_size = 4,
    .value_size = sizeof(struct test_result),
    .max_entries = 1,
    .map_flags = 0,
};

int skb_test_prog(struct __sk_buff* skb) {
  // Check that the packet corresponds to the target socket. It is identified
  // by the cookie stored in the `target_cookie` map.
  int zero = 0;
  __u64* cookie = bpf_map_lookup_elem(&target_cookie, &zero);
  if (!cookie || *cookie != bpf_get_socket_cookie(skb)) {
    return 1;
  }

  // Push a message to the ringbuf to indicate that the packet was received.
  int* entry = bpf_ringbuf_reserve(&ringbuf, 4, 0);
  if (!entry) {
    return 1;
  }

  *entry = skb->len;

  struct test_result result = {
      .ether_type = skb->protocol,
      .ifindex = skb->ifindex,
  };

  // Try calling `bpf_sk_fullsock`.
  struct bpf_sock* fullsock = skb->sk ? bpf_sk_fullsock(skb->sk) : 0;
  if (fullsock) {
    result.sk_type = fullsock->type;
    result.sk_protocol = fullsock->protocol;
    result.sk_family = fullsock->family;
  }
  bpf_map_update_elem(&test_result, &zero, &result, 0);

  bpf_ringbuf_submit(entry, 0);

  return 1;
}

int sock_create_prog(struct bpf_sock* sock) {
  // Increment the global counter.
  __sync_fetch_and_add(&kGlobal.global_counter2, 2);
  __sync_fetch_and_add(&kGlobal.global_counter1, 1);

  int zero = 0;
  int* counter = bpf_map_lookup_elem(&count, &zero);
  if (!counter) {
    return 1;
  }

  __sync_fetch_and_add(counter, 1);

  struct test_result result = {
      .sk_type = sock->type,
      .sk_protocol = sock->protocol,
      .sk_family = sock->family,
      .sk_state = sock->state,
  };
  bpf_map_update_elem(&test_result, &zero, &result, 0);

  return 1;
}

int sock_release_prog(struct bpf_sock* sock) {
  // Check that the packet corresponds to the target socket. It is identified
  // by the cookie stored in the `target_cookie` map.
  int zero = 0;
  __u64* cookie = bpf_map_lookup_elem(&target_cookie, &zero);
  if (!cookie || *cookie != bpf_get_socket_cookie(sock)) {
    return 1;
  }

  int* counter = bpf_map_lookup_elem(&count, &zero);
  if (!counter) {
    return 1;
  }

  __sync_fetch_and_add(counter, 1);

  struct test_result result = {
      .uid_gid = bpf_get_current_uid_gid(),
      .pid_tgid = bpf_get_current_pid_tgid(),
      .sk_type = sock->type,
      .sk_protocol = sock->protocol,
      .sk_family = sock->family,
      .sk_state = sock->state,
  };
  bpf_map_update_elem(&test_result, &zero, &result, 0);

  return 1;
}

int setsockopt_prog(struct bpf_sockopt* sockopt) {
  if (sockopt->optname == TEST_SOCK_OPT) {
    __u64 buffer_size = sockopt->optval_end - sockopt->optval;
    struct test_result result = {
        .optlen = sockopt->optlen,
        .optval_size = buffer_size,
        .get_retval = bpf_get_retval(),
    };
    if (sockopt->sk) {
      result.sk_type = sockopt->sk->type;
      result.sk_protocol = sockopt->sk->protocol;
      result.sk_family = sockopt->sk->family;
      result.sk_state = sockopt->sk->state;
    }
    int zero = 0;
    bpf_map_update_elem(&test_result, &zero, &result, 0);

    char v = 0;
    if (sockopt->optval_end > sockopt->optval + sizeof(char)) {
      v = *(char*)sockopt->optval;
    }

    switch (v) {
      case 0:
        // set `optlen=-1` to bypass the kernels implementation of the syscall.
        sockopt->optlen = -1;
        break;

      case 1:
        // Increase optlen beyond the buffer size. This is expected to result in EFAULT.
        sockopt->optlen = buffer_size + 1;
        break;

      case 2:
        // Returning 0 should result in EPERM.
        return 0;
    }

    return 1;
  }

  if (sockopt->optname == SO_SNDBUF) {
    if (sockopt->optval_end < sockopt->optval + sizeof(int)) {
      return 1;
    }
    int* v = (int*)sockopt->optval;

    // Override the value.
    if (*v == 55555) {
      *v = 66666;
      sockopt->optlen = 4;
    } else {
      sockopt->optlen = 0;
    }
    return 1;
  }

  sockopt->optlen = 0;
  return 1;
}

int getsockopt_prog(struct bpf_sockopt* sockopt) {
  __u64 buffer_size = sockopt->optval_end - sockopt->optval;

  struct test_result result = {
      .optlen = sockopt->optlen,
      .optval_size = buffer_size,
      .retval = sockopt->retval,
      .get_retval = bpf_get_retval(),
  };
  if (sockopt->sk) {
    result.sk_type = sockopt->sk->type;
    result.sk_protocol = sockopt->sk->protocol;
    result.sk_family = sockopt->sk->family;
    result.sk_state = sockopt->sk->state;
  }
  int zero = 0;
  bpf_map_update_elem(&test_result, &zero, &result, 0);

  if (sockopt->optname == TEST_SOCK_OPT) {
    switch (buffer_size) {
      case 2:
        // Fail with the original error.
        break;
      case 3:
        // If the program returns 0 then the original error code should
        // still be returned to the user (instead of EPERM).
        return 0;
      case 4:
        // Override an error set by the syscall implementation.
        sockopt->retval = 0;
        if (sockopt->optval + 4 <= sockopt->optval_end) {
          *(int*)sockopt->optval = 42;
        }
        sockopt->optlen = 4;
        break;
      case 5:
        // `getsockopt` is not allowed to set `optlen = -1`. The original
        // ENOPROTOOPT should be returned.
        sockopt->optlen = -1;
        break;
    }

    return 1;
  }

  if (sockopt->optname == SO_SNDBUF) {
    switch (buffer_size) {
      case 55:
        if (sockopt->optval + 8 < sockopt->optval_end) {
          // Try overriding result with a larger value.
          *(__u64*)sockopt->optval = 0x1234567890abcdef;
        }
        sockopt->optlen = 8;
        break;

      case 56:
        // Reject the call should result in EPERM.
        return 0;

      case 57:
        // `getsockopt` is not allowed to set `optlen = -1`. Should result in
        // EFAULT.
        sockopt->optlen = -1;
        break;

      case 58:
        // Try changing retval to an error.
        sockopt->retval = -5;
        break;

      case 59:
        // Set return value by calling `bpf_set_retval`.
        bpf_set_retval(-5);
        break;

      case 60:
        // Return without changing anything.
        break;

      case 61:
        // Set optlen = 0. Should result in the original being returned to the
        // caller.
        sockopt->optlen = 0;
        break;
    }
  }

  return 1;
}

int udprecv6_prog(struct bpf_sock_addr* sockaddr) {
  // Check that the packet corresponds to the target socket. It is identified
  // by the cookie stored in the `target_cookie` map.
  int zero = 0;
  __u64* cookie = bpf_map_lookup_elem(&target_cookie, &zero);
  if (!cookie || *cookie != bpf_get_socket_cookie(sockaddr)) {
    return 1;
  }

  int* counter = bpf_map_lookup_elem(&count, &zero);
  if (!counter) {
    return 1;
  }

  __sync_fetch_and_add(counter, 1);

  struct test_result result = {
      .sockaddr_port = sockaddr->user_port,
      .sockaddr_family = sockaddr->user_family,
  };
  result.sockaddr_ip[0] = sockaddr->user_ip6[0];
  result.sockaddr_ip[1] = sockaddr->user_ip6[1];
  result.sockaddr_ip[2] = sockaddr->user_ip6[2];
  result.sockaddr_ip[3] = sockaddr->user_ip6[3];
  if (sockaddr->sk) {
    result.sk_type = sockaddr->sk->type;
    result.sk_protocol = sockaddr->sk->protocol;
    result.sk_family = sockaddr->sk->family;
    result.sk_state = sockaddr->sk->state;
  }

  bpf_map_update_elem(&test_result, &zero, &result, 0);

  return 1;
}

int udpsend4_prog(struct bpf_sock_addr* sockaddr) {
  // Check that the packet corresponds to the target socket. It is identified
  // by the cookie stored in the `target_cookie` map.
  int zero = 0;
  __u64* cookie = bpf_map_lookup_elem(&target_cookie, &zero);
  if (!cookie || *cookie != bpf_get_socket_cookie(sockaddr)) {
    return 1;
  }

  int* counter = bpf_map_lookup_elem(&count, &zero);
  if (!counter) {
    return 1;
  }

  __sync_fetch_and_add(counter, 1);

  struct test_result result = {
      .sockaddr_port = sockaddr->user_port,
      .sockaddr_family = sockaddr->user_family,
  };
  result.sockaddr_ip[0] = sockaddr->user_ip4;
  if (sockaddr->sk) {
    result.sk_type = sockaddr->sk->type;
    result.sk_protocol = sockaddr->sk->protocol;
    result.sk_family = sockaddr->sk->family;
    result.sk_state = sockaddr->sk->state;
  }
  bpf_map_update_elem(&test_result, &zero, &result, 0);

  // Fail sendmsg() with EFAIL.
  return 0;
}

SECTION("maps")
struct bpf_map_def sk_storage_map = {
    .type = BPF_MAP_TYPE_SK_STORAGE,
    .key_size = sizeof(int),
    .value_size = sizeof(__u32),
    .max_entries = 0,
    .map_flags = BPF_F_NO_PREALLOC,
};

int sock_create_sk_storage_prog(struct bpf_sock* sock) {
  __u32 value = 1;
  __u32* storage = bpf_sk_storage_get(&sk_storage_map, sock, &value, BPF_SK_STORAGE_GET_F_CREATE);
  if (!storage) {
    return 0;
  }
  return 1;
}

int setsockopt_sk_storage_prog(struct bpf_sockopt* sockopt) {
  if (sockopt->optname != TEST_SOCK_OPT) {
    return 1;
  }

  __u32* storage = bpf_sk_storage_get(&sk_storage_map, sockopt->sk, 0, 0);
  if (!storage) {
    return 1;
  }

  // set `optlen=-1` to bypass the kernels implementation of the syscall.
  sockopt->optlen = -1;

  *storage += 2;
  return 1;
}

int connect_sk_storage_prog(struct bpf_sock_addr* sockaddr) {
  __u32* storage = bpf_sk_storage_get(&sk_storage_map, sockaddr->sk, 0, 0);
  if (!storage) {
    return 1;
  }

  *storage += 4;

  return 1;
}

int test_ringbuf_reserve_overflow_sock(struct bpf_sock* sock) {
  int zero = 0;
  void* entry = bpf_ringbuf_reserve(&ringbuf, 0x100000008ULL, 0);
  struct test_result result = {};
  if (!entry) {
    result.retval = 1;
  } else {
    result.retval = 2;
    bpf_ringbuf_discard(entry, 0);
  }
  bpf_map_update_elem(&test_result, &zero, &result, 0);
  return 1;
}
