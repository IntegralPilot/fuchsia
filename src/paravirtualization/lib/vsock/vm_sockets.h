// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PARAVIRTUALIZATION_LIB_VSOCK_VM_SOCKETS_H_
#define SRC_PARAVIRTUALIZATION_LIB_VSOCK_VM_SOCKETS_H_

#include <sys/socket.h>

#define VMADDR_CID_ANY (-1U)
#define VMADDR_PORT_ANY (-1U)
#define VMADDR_CID_HYPERVISOR 0
#define VMADDR_CID_LOCAL 1
#define VMADDR_CID_HOST 2

// TODO(https://fxbug.dev/361855880): Figure out where this header file should live.
struct sockaddr_vm {
  sa_family_t svm_family;
  unsigned short svm_reserved1;
  unsigned int svm_port;
  unsigned int svm_cid;
  unsigned char svm_zero[sizeof(struct sockaddr) - sizeof(sa_family_t) - sizeof(unsigned short) -
                         sizeof(unsigned int) - sizeof(unsigned int)];
};

#endif  // SRC_PARAVIRTUALIZATION_LIB_VSOCK_VM_SOCKETS_H_
