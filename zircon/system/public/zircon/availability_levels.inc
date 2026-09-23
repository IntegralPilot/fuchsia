// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains macros used by the FUCHSIA_API_LEVEL_*() macros in availability.h.
// It is a separate file to allow API level support to evolve separately from the
// API level-independent macros in that file.

// Special named API levels, which are defined in
// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0246_api_levels_are_32_bits#special_api_levels.
#define FUCHSIA_INTERNAL_LEVEL_NEXT_() 4291821568
#define FUCHSIA_INTERNAL_LEVEL_HEAD_() 4292870144
#define FUCHSIA_INTERNAL_LEVEL_PLATFORM_() 4293918720

// Published stable API levels.
//
// Only existing stable API levels should be included below to ensure `NEXT` and
// similar are used as appropriate. The levels are defined in decreasing order
// to allow newer levels to be used in conditions for defining older levels.
//
// Although sunset and retired API levels are not supported by the SDK,
// downstream code may still have conditionals based on those levels, and
// removing support for those conditionals could unnecessarily break those
// conditional, which are still correct even if obsolete. Therefore, support for
// retired API levels must be carefully considered and only applied at the
// `NEXT` API level.
//
// TODO(https://fxbug.dev/349622444): Generate the numbered levels from version_history.json.
#define FUCHSIA_INTERNAL_LEVEL_32_() 32
#define FUCHSIA_INTERNAL_LEVEL_31_() 31
#define FUCHSIA_INTERNAL_LEVEL_30_() 30
#define FUCHSIA_INTERNAL_LEVEL_29_() 29
#define FUCHSIA_INTERNAL_LEVEL_28_() 28
#define FUCHSIA_INTERNAL_LEVEL_27_() 27
#define FUCHSIA_INTERNAL_LEVEL_26_() 26
#define FUCHSIA_INTERNAL_LEVEL_25_() 25
#define FUCHSIA_INTERNAL_LEVEL_24_() 24
#define FUCHSIA_INTERNAL_LEVEL_23_() 23
#define FUCHSIA_INTERNAL_LEVEL_22_() 22
#define FUCHSIA_INTERNAL_LEVEL_21_() 21
#define FUCHSIA_INTERNAL_LEVEL_20_() 20
#define FUCHSIA_INTERNAL_LEVEL_19_() 19
#define FUCHSIA_INTERNAL_LEVEL_18_() 18
#define FUCHSIA_INTERNAL_LEVEL_17_() 17
#define FUCHSIA_INTERNAL_LEVEL_16_() 16
#define FUCHSIA_INTERNAL_LEVEL_15_() 15

// The following older levels are needed for in-tree instances of the ZX_*_SINCE() macros.
#define FUCHSIA_INTERNAL_LEVEL_14_() 14
#define FUCHSIA_INTERNAL_LEVEL_12_() 12
#define FUCHSIA_INTERNAL_LEVEL_11_() 11
#define FUCHSIA_INTERNAL_LEVEL_10_() 10
#define FUCHSIA_INTERNAL_LEVEL_9_() 9
#define FUCHSIA_INTERNAL_LEVEL_8_() 8
#define FUCHSIA_INTERNAL_LEVEL_7_() 7
#define FUCHSIA_INTERNAL_LEVEL_1_() 1
