// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn_test

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestVisibilityConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "public",
			bazel: `go_library(
	name = "test",
	visibility = [
		"//visibility:public",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		"*",
	]
}`,
		},
		{
			name: "private",
			bazel: `go_library(
	name = "test",
	visibility = [
		"//visibility:private",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		":*",
	]
}`,
		},
		{
			name: "pkg and subpackages",
			bazel: `go_library(
	name = "test",
	visibility = [
		":__pkg__",
		":__subpackages__",
		"//:__pkg__",
		"//:__subpackages__",
		"//path/to/foo:__pkg__",
		"//path/to/bar:__subpackages__",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		":*",
		"./*",
		"//:*",
		"//*",
		"//path/to/foo:*",
		"//path/to/bar/*",
	]
}`,
		},
		{
			name: "package group is unchanged",
			bazel: `go_library(
	name = "test",
	visibility = [
		"//path/to/foo:__pkg__",
		"//path/to/bar:bar",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		"//path/to/foo:*",
		"//path/to/bar:bar",
	]
}`,
		},
		{
			name: "skip list elements",
			bazel: `go_library(
	name = "test",
	visibility = [
		# Ensure handling of special Bazel identifiers does not interfere with the annotation.
		":__pkg__",  # @bazel2gn:skip
		":__subpackages__",  # @bazel2gn:skip
		"//:__pkg__",  # @bazel2gn:skip
		"//:__subpackages__",  # @bazel2gn:skip
		# Normal labels.
		"//path/to/foo:__pkg__",
		"//path/to/bar:bar",  # @bazel2gn:skip
		"//path/to/baz:baz",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		"//path/to/foo:*",
		"//path/to/baz:baz",
	]
}`,
		},
		{
			name: "skip the entire 'visibility' attribute",
			bazel: `go_library(
	name = "test",
	visibility = [
		# Ensure handling of special Bazel identifiers does not interfere with the annotation.
		":__pkg__",
		":__subpackages__",
		"//:__pkg__",
		"//:__subpackages__",
		# Normal label.
		"//path/to/bar:bar",
	],  # @bazel2gn:skip
)`,
			wantGN: `go_library("test") {
}`,
		},
		{
			name: "overwrite a list item",
			bazel: `go_library(
	name = "test",
	visibility = [
		"//path/to/foo:__pkg__",  # @bazel2gn:raw_overwrite:"//*"
		"//path/to/bar:bar",  # @bazel2gn:skip
		"//redundant/path/in/gn:__pkg__",
	],
)`,
			wantGN: `go_library("test") {
	visibility = [
		"//*",
		"//redundant/path/in/gn:*",
	]
}`,
		},
		{
			name: "overwrite the entire list",
			bazel: `go_library(
	name = "test",
	visibility = [
		"//path/to/foo:__pkg__",  # @bazel2gn:raw_overwrite:"//path/that/should_be_ignored/*"
		"//path/to/bar:bar",  # @bazel2gn:skip
		"//redundant/path/in/gn:__pkg__",  # @bazel2gn:path_overwrite://another/path/that/should_be_ignored/*
	],  # @bazel2gn:raw_overwrite:[ "//*" ]
)`,
			wantGN: `go_library("test") {
	visibility = [ "//*" ]
}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\nBazel source:\n%s", diff, tc.bazel)
			}
		})
	}
}

func TestDepsConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "rust third-party",
			bazel: `rustc_library(
	name = "test",
	deps = [
		"//third_party/rust_crates/vendor:foo",
		"//third_party/rust_crates/ask2patch/bar",
		"//third_party/rust_crates/forks/baz-quux-0.4.2:baz_quux",
		"//path/to/dep",
	],
)`,
			wantGN: `rustc_library("test") {
	deps = [
		"//third_party/rust_crates:foo",
		"//third_party/rust_crates:bar",
		"//third_party/rust_crates:baz-quux-0.4.2",
		"//path/to/dep",
	]
}`,
		},
		{
			name: "go third-party",
			bazel: `go_library(
	name = "test",
	deps = [
		"//third_party/golibs:github.com/foo/bar",
		"//third_party/golibs:github.com/foo/bar/baz",
		"//third_party/golibs:golang.org/x/crypto/ssh",
		"//third_party/golibs:golang.org/x/crypto_alt",
		"//third_party/golibs:google.golang.org/protobuf/encoding/protojson",
		"//third_party/golibs:google.golang.org/protobuf/reflect/protoreflect",
		"//path/to/dep",
	],
)`,
			wantGN: `go_library("test") {
	deps = [
		"//third_party/golibs:github.com/foo/bar",
		"//third_party/golibs:github.com/foo/bar/baz",
		"//third_party/golibs:golang.org/x/crypto",
		"//third_party/golibs:golang.org/x/crypto_alt",
		"//third_party/golibs:google.golang.org/protobuf",
		"//third_party/golibs:google.golang.org/protobuf",
		"//path/to/dep",
	]
}`,
		},
		{
			name: "third-party targets",
			bazel: `cc_library(
	name = "test",
	deps = [
		"@com_google_googletest//:gtest",
		"@com_googletest_gtest//:gtest",
		"@com_google_googletest//:gtest_prod",
		"@com_google_googletest//:gtest_main",
		"@com_google_googletest//:gmock",
		"@googletest//:gtest",
		"@re2//:re2",
		"@boringssl//:crypto",
		"@boringssl//:ssl",
		"@zlib//:zlib",
		"@rapidjson//:rapidjson",
		"@com_google_protobuf//:protobuf",
	],
)`,
			wantGN: `static_library("test") {
	public_deps = [
		"//third_party/googletest:gtest",
		"//third_party/googletest:gtest",
		"//third_party/googletest:gtest_prod",
		"//third_party/googletest:gtest_main",
		"//third_party/googletest:gmock",
		"//third_party/googletest:gtest",
		"//third_party/re2",
		"//third_party/boringssl:crypto",
		"//third_party/boringssl:ssl",
		"//third_party/zlib",
		"//third_party/rapidjson",
		"//third_party/protobuf:protobuf",
	]
}`,
		},
		{
			name: "untranslated bazel repo target label",
			bazel: `cc_library(
	name = "test",
	deps = [
		"@unknown_repo//:some_target",
	],
)`,
			wantGN: `static_library("test") {
	public_deps = [
		"@unknown_repo//:some_target" # BAZEL2GN_WARNING: Unknown Bazel repository name,
	]
}`,
		},
		{
			name: "overwritten deps",
			bazel: `go_library(
	name = "test",
	deps = [
		"//path/to:foo", # @bazel2gn:path_overwrite://path/to/foo_overwritten
		"//path/to:bar", # @bazel2gn:path_overwrite:bar_overwritten
		"//path/to:baz",
	],
)`,
			wantGN: `go_library("test") {
	deps = [
		"//path/to/foo_overwritten",
		"bar_overwritten",
		"//path/to:baz",
	]
}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\nBazel source:\n%s", diff, tc.bazel)
			}
		})
	}
}

func TestPathOverwriteAnnotation(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "overwritten paths",
			bazel: `go_library(
	name = "test",
	srcs = [
		"foo.go", # @bazel2gn:path_overwrite:foo_overwritten.go
		"bar.go", # @bazel2gn:path_overwrite://path/to/bar_overwritten.go
	],
	outputs = [
		"foo.out", # @bazel2gn:path_overwrite:${target_out_dir}/foo.out
	],
)`,
			wantGN: `go_library("test") {
	sources = [
		"foo_overwritten.go",
		"//path/to/bar_overwritten.go",
	]
	outputs = [
		"${target_out_dir}/foo.out",
	]
}`,
		},
		{
			name: "mixed paths",
			bazel: `go_library(
	name = "test",
	srcs = [
		"foo.go",
		"bar.go", # @bazel2gn:path_overwrite:bar_overwritten.go
		"baz.go",
	],
	outputs = [
		"foo.out", # @bazel2gn:path_overwrite:${target_out_dir}/foo.out
		"bar.out",
	]
)`,
			wantGN: `go_library("test") {
	sources = [
		"foo.go",
		"bar_overwritten.go",
		"baz.go",
	]
	outputs = [
		"${target_out_dir}/foo.out",
		"bar.out",
	]
}`,
		},
		{
			name: "colon_in_path",
			bazel: `go_library(
	name = "test",
	srcs = [
		"//path/to:foo.go",
		"//path/to:bar.go", # @bazel2gn:path_overwrite://path/to/bar_overwritten.go
	],
	outputs = [
		"//path/for:foo.out",
		"//path/for:bar.out", # @bazel2gn:path_overwrite:${target_out_dir}/bar.out
	],
	deps = [
		"//path/to:baz",
	]
)`,
			wantGN: `go_library("test") {
	sources = [
		"//path/to/foo.go",
		"//path/to/bar_overwritten.go",
	]
	outputs = [
		"//path/for/foo.out",
		"${target_out_dir}/bar.out",
	]
	deps = [
		"//path/to:baz",
	]
}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\nBazel source:\n%s", diff, tc.bazel)
			}
		})
	}
}

func TestLdflagsConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "raw_overwrite ldflags",
			bazel: `cc_library(
	name = "test",
	ldflags = [
		"-Wl,--something", # @bazel2gn:raw_overwrite:"-Wl,--something_overwritten"
		"-Wl,--another",
	],
)`,
			wantGN: `static_library("test") {
	ldflags = [
		"-Wl,--something_overwritten",
		"-Wl,--another",
	]
}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\nBazel source:\n%s", diff, tc.bazel)
			}
		})
	}
}

func TestLdflagsConversionErrors(t *testing.T) {
	for _, tc := range []struct {
		name  string
		bazel string
	}{
		{
			name: "path_overwrite ldflags",
			bazel: `cc_library(
	name = "test",
	ldflags = [
		"-Wl,--something", # @bazel2gn:path_overwrite:-Wl,--something_overwritten
	],
)`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			_, err := bazelToGN(f)
			if err == nil {
				t.Errorf("Unexpected success converting Bazel targets. Bazel source:\n%s", tc.bazel)
			}
		})
	}
}
