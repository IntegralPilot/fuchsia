// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS MACHINE GENERATED. DO NOT EDIT.
// Generated from the banjo.examples.tables banjo file

#pragma once


#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Forward declarations
typedef struct a a_t;
typedef struct b b_t;
typedef struct c c_t;
typedef struct d d_t;
typedef struct e e_t;
typedef struct f f_t;
typedef uint32_t g_t;
#define G_ONLINE UINT32_C(0x01)
typedef struct h h_t;

// Declarations
struct a {
    b_t foo;
};

struct b {
    a_t bar;
};

struct c {
    zx_handle_t baz;
};

struct d {
    c_t qux;
};

struct e {
    uint8_t quux;
};

struct f {
    e_t quuz;
};

struct h {
    g_t flags;
};


// Helpers


__END_CDECLS
