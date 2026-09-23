# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This file should be linked into the toplevel fuchsia dir.

project('fuchsia', 'cpp', 'c',
         version : '0.0.1',
         license : 'MIT OR Apache-2.0',
         default_options: 'cpp_std=c++17')

inc_top=include_directories('.')
inc_magma=include_directories('sdk/lib/magma_client/include', 'sdk/lib/magma_common/include')

subdir('src/graphics/lib/magma')

if get_option('with_tests')
    subdir('src/graphics/tests/vkreadback')
    subdir('src/graphics/magma/tests')
endif
