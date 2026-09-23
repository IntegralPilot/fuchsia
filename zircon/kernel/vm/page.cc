// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/page.h"

#include "vm/page_ffi.h"

void vm_page::dump() const { rust_vm_page_dump(this); }

uint64_t vm_page::get_count(vm_page_state state) { return rust_vm_page_get_count(state); }

void vm_page::add_to_initial_count(vm_page_state state, uint64_t n) {
  rust_vm_page_add_to_initial_count(state, n);
}
