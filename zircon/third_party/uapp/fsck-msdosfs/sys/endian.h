/* Copyright 2026 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

/*
 * Fuchsia Portability Shim: <sys/endian.h>
 * Provides little-endian decoding and encoding helper functions (le16dec,
 * le16enc, le32dec, le32enc) required by FreeBSD fsck_msdosfs source files.
 */

#ifndef ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_ENDIAN_H_
#define ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_ENDIAN_H_

#include <stdint.h>

static inline uint16_t le16dec(const void *pp) {
  const uint8_t *p = (const uint8_t *)pp;
  return (uint16_t)(((uint16_t)p[0]) | ((uint16_t)p[1] << 8));
}

static inline void le16enc(void *pp, uint16_t u) {
  uint8_t *p = (uint8_t *)pp;
  p[0] = (uint8_t)(u & 0xff);
  p[1] = (uint8_t)((u >> 8) & 0xff);
}

static inline uint32_t le32dec(const void *pp) {
  const uint8_t *p = (const uint8_t *)pp;
  return ((uint32_t)p[0]) | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static inline void le32enc(void *pp, uint32_t u) {
  uint8_t *p = (uint8_t *)pp;
  p[0] = (uint8_t)(u & 0xff);
  p[1] = (uint8_t)((u >> 8) & 0xff);
  p[2] = (uint8_t)((u >> 16) & 0xff);
  p[3] = (uint8_t)((u >> 24) & 0xff);
}

#endif  // ZIRCON_THIRD_PARTY_UAPP_FSCK_MSDOSFS_SYS_ENDIAN_H_
