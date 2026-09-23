# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("//build/bazel/rules/rust:rustc_binary.bzl", _rustc_binary = "rustc_binary")
load("//build/bazel/rules/rust:rustc_embed_files.bzl", _rustc_embed_files = "rustc_embed_files")
load("//build/bazel/rules/rust:rustc_library.bzl", _rustc_library = "rustc_library")
load("//build/bazel/rules/rust:rustc_proc_macro.bzl", _rustc_proc_macro = "rustc_proc_macro")
load("//build/bazel/rules/rust:rustc_test.bzl", _rustc_test = "rustc_test")

rustc_binary = _rustc_binary
rustc_embed_files = _rustc_embed_files
rustc_library = _rustc_library
rustc_proc_macro = _rustc_proc_macro
rustc_test = _rustc_test
