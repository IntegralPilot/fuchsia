#!/bin/sh
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Compiles eBPF programs used by tests. Must be executed whenever .c files in
# this directory are updated.
#
# TODO(https://fxbug.dev/416736134): This is necessary to workaround the lack
# of eBPF support in clang prebuilds. Add ebpf toolchain in GN and use it
# to replace this script.

set -e

DIR=$(realpath $(dirname "$0"))
ROOT_DIR=$(realpath $(dirname "$0")/../../../../../..)

CFLAGS="-target bpf -mcpu=v2 -O2 -Wno-macro-redefined -nostdinc"
CFLAGS="$CFLAGS -I$ROOT_DIR/third_party/android/platform/bionic/libc/include"
CFLAGS="$CFLAGS -I$ROOT_DIR/third_party/android/platform/bionic/libc/kernel/uapi"
CFLAGS="$CFLAGS -I$ROOT_DIR/third_party/android/platform/bionic/libc/kernel/uapi/linux"
CFLAGS="$CFLAGS -I$ROOT_DIR/third_party/android/platform/bionic/libc/kernel/android/uapi"
CFLAGS="$CFLAGS -I$ROOT_DIR/third_party/android/platform/bionic/libc/kernel/uapi/asm-x86"
CFLAGS="$CFLAGS -I$ROOT_DIR/src/starnix/lib/ebpf_loader/include"

set -v
clang $CFLAGS -c $DIR/ebpf_test_progs.c -o $DIR/ebpf_test_progs.o
