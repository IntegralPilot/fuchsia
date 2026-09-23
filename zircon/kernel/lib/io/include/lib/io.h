// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_IO_INCLUDE_LIB_IO_H_
#define ZIRCON_KERNEL_LIB_IO_INCLUDE_LIB_IO_H_

#include <stdio.h>
#include <sys/types.h>
#include <zircon/compiler.h>

#include <fbl/intrusive_double_list.h>
#include <ktl/array.h>
#include <ktl/string_view.h>

// Back door to directly write to the kernel serial port.
void serial_write(ktl::string_view str);

extern FILE gSerialFile;
extern FILE gStdoutUnbuffered;
extern FILE gStdoutNoPersist;

#endif  // ZIRCON_KERNEL_LIB_IO_INCLUDE_LIB_IO_H_
