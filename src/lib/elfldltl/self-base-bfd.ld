/* Copyright 2025 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

/*
 * This is included as an input linker script when the library is used.  See
 * self-base.ld, which is used with others linkers.  This version is used
 * instead with GNU (BFD) ld because on some targets such as aarch64-elf, the
 * definition using __ehdr_start doesn't work when it should.  This version
 * instead uses a linker script expression that works for BFD in uses that
 * have come up, but not necessarily for other linkers.
 */
PROVIDE_HIDDEN(elfldltl.kBase = SEGMENT_START("text-segment", 0));
