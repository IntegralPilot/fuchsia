// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>

#include <android-base/unique_fd.h>
#include <cutils/ashmem.h>

/*
 * Fuchsia equivalent of Android ashmem_create_region. On Android on Starnix,
 * this used memfd_create_region, which ends up calling memfd_create. This
 * creates a Vmo backed file. To ensure compatibility, this function will
 * directly create a VMO, and wrap it into a fd.
 */
int ashmem_create_region(const char *name, size_t size) {
  android::base::unique_fd fd(memfd_create(name, 0));
  if (fd == -1) {
    return -1;
  }
  if (ftruncate(fd, size) == -1) {
    return -1;
  }
  return fd.release();
}

/*
 * Fuchsia equivalent of Android ashmem_set_prot_region. As Vmo cannot be
 * sealed in a compatible way with Android, this will fail if PROT_WRITE is not
 * in prot.
 */
int ashmem_set_prot_region(int fd, int prot) {
  // Following android implementation of memfd_set_prot_region, only proceed if
  // an fd needs to be write-protected.
  if (prot & PROT_WRITE) {
    return 0;
  }
  errno = ENOSYS;
  return -1;
}
