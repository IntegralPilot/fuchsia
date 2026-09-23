// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/assert.h>

extern "C" void __cxa_pure_virtual();

extern "C" void __cxa_pure_virtual() { ZX_PANIC("pure virtual called"); }

// This is the name used by the MSVC C++ ABI as used on UEFI.
extern "C" [[gnu::alias("__cxa_pure_virtual")]] void _purecall();
