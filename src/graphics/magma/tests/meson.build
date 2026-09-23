# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Debian packages needed:
#   -'rapidjson-dev'
#   -'googletest'
#   -'libdrm-dev'
dl_dep = meson.get_compiler('cpp').find_library('dl', required: false)
gtest_main_dep = dependency('gtest_main', main: true)
drm_dep = dependency('libdrm', required: not get_option('android_build'))

if get_option('android_build')
  extra_link_args = ['-static-libstdc++']
else
  extra_link_args = []
endif

foreach test : ['virtmagma_unit_tests']
  files_unit_test = files(
    '../tests_linux/unit_tests/@0@.cc'.format(test)
  )
  files_unit_test += virtio_magma_h

  test_exe = executable(
    test,
    sources: files_unit_test,
    include_directories : [inc_magma_non_sdk, inc_top, inc_magma, ],
    cpp_args: cpp_args + ['-std=c++17'],
    link_with : [libmagma_virt],
    dependencies : [gtest_main_dep, dl_dep, drm_dep],
    install : false,
    link_args: extra_link_args
  )

  test(test, test_exe)
endforeach

test = 'virtmagma_conformance_tests'

files_test = files(
  'integration/test_magma.cc',
  'integration/test_magma_c.c',
  '../lib/magma_client/test_util/magma_map_cpu.cc',
)

test_magma = executable(
  test,
  sources: files_test,
  include_directories : [inc_magma_non_sdk, inc_top, inc_magma, inc_msd_intel_gen,],
  cpp_args: cpp_args + ['-DVIRTMAGMA=1'],
  link_with : [libmagma_virt],
  dependencies : [gtest_main_dep],
  install : false,
  link_args: extra_link_args,
)

test(test, test_magma)
