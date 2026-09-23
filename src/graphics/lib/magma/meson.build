# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

c_args = [
  '-fvisibility=hidden',
  '-DMAGMA_EXPORT=__attribute__((visibility("default")))',
]

cpp_args = c_args + [ '-DMAGMA_DEBUG_INTERNAL_USE_ONLY', '-fno-exceptions' ]

prog_python = import('python').find_installation('python3')
prog_objcopy = find_program('objcopy')

magma_json = files('include/magma/magma.json')
prog_cc = meson.get_compiler('c')

inc_magma_non_sdk = include_directories('../../magma')
inc_msd_intel_gen = include_directories('../../drivers/msd-intel-gen/include')

# Generate virtio_magma.h in the proper location
subdir('include/virtio')

magma_manual = 'src/libmagma_virt/magma.cc'

magma_generic_cc = custom_target(
  'magma_generic.cc',
  input : ['src/libmagma_virt/magma_generic_cc_gen.py', magma_json, magma_manual],
  output: 'magma_generic.cc',
  command: [prog_python, '@INPUT@', '@OUTPUT@']
)

files_libmagma_linux = files(
  magma_manual,
  '../../magma/lib/magma/platform/linux/linux_platform_logger.cc',
)

thread_dep = dependency('threads')

libmagma_virt = static_library(
  'magma_virt',
  sources: [files_libmagma_linux, magma_generic_cc, virtio_magma_h],
  cpp_args: cpp_args,
  c_args: c_args,
  include_directories : [inc_top, inc_magma_non_sdk, inc_magma, ],
  dependencies: thread_dep,
  install : true
)


magma_virt_relocated = custom_target('magma_virt_relocated',
  output: 'magma_virt_relocated.o',
  input: libmagma_virt,
  command: [prog_cc.cmd_array(),
      '-r',
      '-Wl,--whole-archive',
      '@INPUT@',
      '-Wl,--no-whole-archive',
      '-fno-exceptions',
      '-o',
      '@OUTPUT@',
  ],
)

magma_virt_localized = custom_target('magma_virt_localized',
  output: 'magma_virt_localized.o',
  input: magma_virt_relocated,
  command: [prog_objcopy,
      '--localize-hidden',
      '@INPUT@',
      '@OUTPUT@'
  ],
)

libmagma_virt_hermetic = static_library('magma_virt_hermetic',
  sources: magma_virt_localized,
  install: true,
)

dep_magma = declare_dependency(
  include_directories : [inc_magma, inc_msd_intel_gen],
  link_with : libmagma_virt_hermetic)

