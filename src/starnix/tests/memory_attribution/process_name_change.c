// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define _GNU_SOURCE
#include <stdio.h>
#include <sys/prctl.h>
#include <unistd.h>

#include <linux/prctl.h>

#define BUFFERSIZE 10

// This program creates one anonymous memory mapping then sleeps,
// such that the test can validate memory attribution reporting.
// When it receives a SIGINT signal, it exits.
int main(void) {
  fprintf(stdout, "process_name_change started\n");
  fflush(stdout);

  char buffer[BUFFERSIZE];
  fgets(buffer, BUFFERSIZE, stdin);

  prctl(PR_SET_NAME, "new_name");

  fprintf(stdout, "process_name_change name changed\n");
  fflush(stdout);

  fgets(buffer, BUFFERSIZE, stdin);

  return 0;
}
