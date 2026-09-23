// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <err.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <linux/capability.h>
#include <linux/prctl.h>

namespace {

bool HasCapabilityEffective(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  if (syscall(SYS_capget, &header, &caps) != 0) {
    err(EXIT_FAILURE, "capget");
  }
  return caps[CAP_TO_INDEX(cap)].effective & CAP_TO_MASK(cap);
}

bool HasCapabilityPermitted(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  if (syscall(SYS_capget, &header, &caps) != 0) {
    err(EXIT_FAILURE, "capget");
  }
  return caps[CAP_TO_INDEX(cap)].permitted & CAP_TO_MASK(cap);
}

bool HasCapabilityInheritable(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  if (syscall(SYS_capget, &header, &caps) != 0) {
    err(EXIT_FAILURE, "capget");
  }
  return caps[CAP_TO_INDEX(cap)].inheritable & CAP_TO_MASK(cap);
}

bool HasCapabilityAmbient(int cap) {
  int res = prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, cap, 0, 0);
  if (res == -1) {
    err(EXIT_FAILURE, "prctl PR_CAP_AMBIENT");
  }

  return res == 1;
}

bool HasCapabilityBounding(int cap) {
  int res = prctl(PR_CAPBSET_READ, cap, 0, 0, 0);
  if (res == -1) {
    err(EXIT_FAILURE, "prctl PR_CAPBSET_READ");
  }

  return res == 1;
}

int b2d(bool value) { return value ? 1 : 0; }

int MaxCapabilitySupported() {
  FILE* fp = fopen("/proc/sys/kernel/cap_last_cap", "r");
  if (fp == nullptr) {
    err(EXIT_FAILURE, "opening cap_last_cap");
  }
  int cap_num = -1;
  int n = fscanf(fp, "%d\n", &cap_num);
  fclose(fp);

  if (n != 1) {
    fprintf(stderr, "Could not parse cap_last_cap file.\n");
    exit(EXIT_FAILURE);
  }
  return cap_num;
}

void PrintCapabilities() {
  fprintf(stdout, "CAP_NUM,EFFECTIVE,PERMITTED,INHERITABLE,BOUNDING,AMBIENT\n");

  const int cap_last_cap = MaxCapabilitySupported();

  for (int capability = 0; capability <= cap_last_cap; capability++) {
    fprintf(stdout, "%d,%d,%d,%d,%d,%d\n", capability, b2d(HasCapabilityEffective(capability)),
            b2d(HasCapabilityPermitted(capability)), b2d(HasCapabilityInheritable(capability)),
            b2d(HasCapabilityBounding(capability)), b2d(HasCapabilityAmbient(capability)));
  }
}

void PrintSecurebits() {
  int res = prctl(PR_GET_SECUREBITS);
  if (res == -1) {
    err(EXIT_FAILURE, "prctl(PR_GET_SECUREBITS)");
  }

  fprintf(stdout, "%x\n", res);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc == 0) {
    exit(EXIT_FAILURE);
  }

  if (argc != 2) {
    fprintf(stderr, "Usage: %s <command>\n", argv[0]);
    fprintf(stderr, "commands:\n");
    fprintf(stderr, "\tsecurebits: print the securebits flags\n");
    exit(EXIT_FAILURE);
  }

  if (strcmp(argv[1], "securebits") == 0) {
    PrintSecurebits();
  } else if (strcmp(argv[1], "capabilities") == 0) {
    PrintCapabilities();
  } else {
    fprintf(stderr, "Invalid command: %s\n", argv[1]);
    exit(EXIT_FAILURE);
  }

  return 0;
}
