// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn_test

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestSelectConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "simple select",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "simple_select",
	srcs = select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"@platforms//os:linux": [ "linux.rs" ],
		"//conditions:default": [ "default.rs" ],
	}),
)`,
			wantGN: `rustc_library("simple_select") {
	if (is_fuchsia) {
		sources = [
			"fuchsia.rs",
		]
	} else if (is_linux) {
		sources = [
			"linux.rs",
		]
	} else {
		sources = [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "select with empty non-default",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "select_with_empty_non_default",
	hdrs = select({
		"@platforms//os:fuchsia": [],
		"//conditions:default": [ "default.h" ],
	}),
	srcs = [ "common.rs" ] + select({
		"@platforms//os:fuchsia": [],
		"//conditions:default": [ "default.rs" ],
	}),
)`,
			wantGN: `rustc_library("select_with_empty_non_default") {
	if (is_fuchsia) {
		public = [
		]
	} else {
		public = [
			"default.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (is_fuchsia) {
		sources += [
		]
	} else {
		sources += [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "select with empty default",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "select_with_empty_default",
	hdrs = select({
		"@platforms//os:fuchsia": [ "fuchsia.h" ],
		"//conditions:default": [],
	}),
	srcs = [ "common.rs" ] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"//conditions:default": [],
	}),
)`,
			wantGN: `rustc_library("select_with_empty_default") {
	if (is_fuchsia) {
		public = [
			"fuchsia.h",
		]
	} else {
		public = [
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	}
}`,
		},
		{
			name: "select with skipped non-default",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "select_with_skipped_non_default",
	hdrs = select({
		"@platforms//os:fuchsia": [
			# @bazel2gn:skip
			"fuchsia.h",
		],
		"//conditions:default": [ "default.h" ],
	}),
	srcs = [ "common.rs" ] + select({
		"@platforms//os:fuchsia": [
			# @bazel2gn:skip
			"fuchsia.rs",
		],
		"//conditions:default": [ "default.rs" ],
	}),
)`,
			wantGN: `rustc_library("select_with_skipped_non_default") {
	if (is_fuchsia) {
		public = [
		]
	} else {
		public = [
			"default.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (is_fuchsia) {
		sources += [
		]
	} else {
		sources += [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "select with skipped default",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "select_with_skipped_default",
	hdrs = select({
		"@platforms//os:fuchsia": [ "fuchsia.h" ],
		"//conditions:default": [
				# @bazel2gn:skip
				"default.h",
		],
	}),
	srcs = [ "common.rs" ] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"//conditions:default": [
				# @bazel2gn:skip
				"default.rs",
		],
	}),
)`,
			wantGN: `rustc_library("select_with_skipped_default") {
	if (is_fuchsia) {
		public = [
			"fuchsia.h",
		]
	} else {
		public = [
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	}
}`,
		},
		{
			name: "simple cond expr",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "simple_cond",
	hdrs = [ "some_feature.h" ] if some_build_arg else [ "default.h" ],
	srcs = [ "common.rs" ] + ([ "some_feature.rs" ] if some_build_arg else [ "default.rs" ]),
)`,
			wantGN: `rustc_library("simple_cond") {
	if (some_build_arg) {
		public = [
			"some_feature.h",
		]
	} else {
		public = [
			"default.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_build_arg) {
		sources += [
			"some_feature.rs",
		]
	} else {
		sources += [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "cond expr with empty if",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "cond_with_empty_if",
	hdrs = [] if some_build_arg else [ "default.h" ],
	srcs = [ "common.rs" ] + ([] if some_build_arg else [ "default.rs" ]),
)`,
			wantGN: `rustc_library("cond_with_empty_if") {
	if (some_build_arg) {
		public = [
		]
	} else {
		public = [
			"default.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_build_arg) {
		sources += [
		]
	} else {
		sources += [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "cond expr with empty else",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "cond_with_empty_else",
	hdrs = [ "some_feature.h" ] if some_build_arg else [],
	srcs = [ "common.rs" ] + ([ "some_feature.rs" ] if some_build_arg else []),
)`,
			wantGN: `rustc_library("cond_with_empty_else") {
	if (some_build_arg) {
		public = [
			"some_feature.h",
		]
	} else {
		public = [
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_build_arg) {
		sources += [
			"some_feature.rs",
		]
	}
}`,
		},
		{
			// This test behaves incorrectly, not skipping the "some_feature.*" files.
			// TODO(https://fxbug.dev/521482733): Fix the implementation and update this test.
			name: "cond expr with skipped if",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "cond_with_skipped_if",
	hdrs = [
		# @bazel2gn:skip
		"some_feature.h",
	] if some_build_arg else [ "default.h" ],
	srcs = [ "common.rs" ] + ([
		# @bazel2gn:skip
		"some_feature.rs",
	] if some_build_arg else [ "default.rs" ]),
)`,
			wantGN: `rustc_library("cond_with_skipped_if") {
	if (some_build_arg) {
		public = [
			"some_feature.h",
		]
	} else {
		public = [
			"default.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_build_arg) {
		sources += [
			"some_feature.rs",
		]
	} else {
		sources += [
			"default.rs",
		]
	}
}`,
		},
		{
			name: "cond expr with skipped else",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "cond_with_skipped_else",
	hdrs = [ "some_feature.h" ] if some_build_arg else [
		# @bazel2gn:skip
		"default.h",
	],
	srcs = [ "common.rs" ] + ([ "some_feature.rs" ] if some_build_arg else [
		# @bazel2gn:skip
		"default.rs",
	]),
)`,
			wantGN: `rustc_library("cond_with_skipped_else") {
	if (some_build_arg) {
		public = [
			"some_feature.h",
		]
	} else {
		public = [
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_build_arg) {
		sources += [
			"some_feature.rs",
		]
	}
}`,
		},
		{
			name: "cond expr mixed with select in list concatenation",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "mixed_cond_select",
	hdrs = [
		"common_for_condition.h",
	] + select({
		"@platforms//os:fuchsia": [ "fuchsia.h" ],
		"//conditions:default": [ "linux.h" ],
	}) + [
		"bar.h",
	] if some_condition else [ "baz.h" ],
	srcs = [ "common.rs" ] + ([
		"common_for_condition.rs",
	] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"//conditions:default": [ "linux.rs" ],
	}) + [
		"bar.rs",
	] if some_condition else [ "baz.rs" ]),
)`,
			wantGN: `rustc_library("mixed_cond_select") {
	if (some_condition) {
		public = []
		public += [
			"common_for_condition.h",
		]
		if (is_fuchsia) {
			public += [
				"fuchsia.h",
			]
		} else {
			public += [
				"linux.h",
			]
		}
		public += [
			"bar.h",
		]
	} else {
		public = [
			"baz.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (some_condition) {
		sources += [
			"common_for_condition.rs",
		]
		if (is_fuchsia) {
			sources += [
				"fuchsia.rs",
			]
		} else {
			sources += [
				"linux.rs",
			]
		}
		sources += [
			"bar.rs",
		]
	} else {
		sources += [
			"baz.rs",
		]
	}
}`,
		},
		{
			name: "cond expr surrounded by parens",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "cond_with_parens",
	hdrs = select({
		"@platforms//os:fuchsia": [ "fuchsia.h" ],
		"//conditions:default": [ "linux.h" ],
	}) + ([
		"bar.h",
	] if some_condition else [ "baz.h" ]),
	srcs = [
		"common.rs",
	] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"//conditions:default": [ "linux.rs" ],
	}) + ([
		"bar.rs",
	] if some_condition else [ "baz.rs" ]),
)`,
			wantGN: `rustc_library("cond_with_parens") {
	public = []
	if (is_fuchsia) {
		public += [
			"fuchsia.h",
		]
	} else {
		public += [
			"linux.h",
		]
	}
	if (some_condition) {
		public += [
			"bar.h",
		]
	} else {
		public += [
			"baz.h",
		]
	}
	sources = []
	sources += [
		"common.rs",
	]
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	} else {
		sources += [
			"linux.rs",
		]
	}
	if (some_condition) {
		sources += [
			"bar.rs",
		]
	} else {
		sources += [
			"baz.rs",
		]
	}
}`,
		},
		{
			name: "two selects one condition",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "two_selects_one_condition",
	hdrs = select({
		"@platforms//os:fuchsia": [ "fuchsia.h" ],
		"//conditions:default": [ "linux.h" ],
	}) if some_condition else select({
		"@platforms//os:fuchsia": [ "other_fuchsia.h" ],
		"//conditions:default": [ "other_linux.h" ],
	}),
	srcs = [ "common.rs" ] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"//conditions:default": [ "linux.rs" ],
	}) if some_condition else select({
		"@platforms//os:fuchsia": [ "other_fuchsia.rs" ],
		"//conditions:default": [ "other_linux.rs" ],
	}),
)`,
			wantGN: `rustc_library("two_selects_one_condition") {
	if (some_condition) {
		public = []
		if (is_fuchsia) {
			public += [
				"fuchsia.h",
			]
		} else {
			public += [
				"linux.h",
			]
		}
	} else {
		public = []
		if (is_fuchsia) {
			public += [
				"other_fuchsia.h",
			]
		} else {
			public += [
				"other_linux.h",
			]
		}
	}
	if (some_condition) {
		sources = []
		sources += [
			"common.rs",
		]
		if (is_fuchsia) {
			sources += [
				"fuchsia.rs",
			]
		} else {
			sources += [
				"linux.rs",
			]
		}
	} else {
		sources = []
		if (is_fuchsia) {
			sources += [
				"other_fuchsia.rs",
			]
		} else {
			sources += [
				"other_linux.rs",
			]
		}
	}
}`,
		},
		{
			name: "list concatenation",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "list_concatenation",
	srcs = [
		"foo.rs",
	] + [
		"bar.rs",
	] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"@platforms//os:linux": [ "linux.rs" ],
	}),
)`,
			wantGN: `rustc_library("list_concatenation") {
	sources = []
	sources += [
		"foo.rs",
	]
	sources += [
		"bar.rs",
	]
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	} else if (is_linux) {
		sources += [
			"linux.rs",
		]
	}
}`,
		},
		{
			name: "list concatenation with variable",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

list_of_files = ["bar.rs"]

rust_library(
	name = "list_concatenation_with_variable",
	srcs = [
		"foo.rs",
	] + list_of_files + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"@platforms//os:linux": [ "linux.rs" ],
	}),
)`,
			wantGN: `list_of_files = [
	"bar.rs",
]

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "list_of_files" ])

rustc_library("list_concatenation_with_variable") {
	sources = []
	sources += [
		"foo.rs",
	]
	sources += list_of_files
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	} else if (is_linux) {
		sources += [
			"linux.rs",
		]
	}
}`,
		},
		{
			name: "select no_match_error",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "no_match_error",
	srcs = [
		"foo.rs",
	] + select(
		{
			"@platforms//os:fuchsia": [ "bar.rs" ],
			"@platforms//os:linux": [ "baz.rs" ],
		},
		no_match_error = "unknown platform!",
	),
)`,
			wantGN: `rustc_library("no_match_error") {
	sources = []
	sources += [
		"foo.rs",
	]
	if (is_fuchsia) {
		sources += [
			"bar.rs",
		]
	} else if (is_linux) {
		sources += [
			"baz.rs",
		]
	} else {
		assert(false, "unknown platform!")
	}
}`,
		},
		{
			name: "multiple selects",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "multiple_selects",
	srcs = [
		"foo.rs",
	] + select(
		{
			"@platforms//os:fuchsia": [ "bar.rs" ],
			"@platforms//os:linux": [ "baz.rs" ],
		},
		no_match_error = "unknown platform!",
	) + [
		"yet_another_foo.rs",
	] + select(
		{
			"@platforms//os:fuchsia": [ "yet_another_bar.rs" ],
		},
	),
)`,
			wantGN: `rustc_library("multiple_selects") {
	sources = []
	sources += [
		"foo.rs",
	]
	if (is_fuchsia) {
		sources += [
			"bar.rs",
		]
	} else if (is_linux) {
		sources += [
			"baz.rs",
		]
	} else {
		assert(false, "unknown platform!")
	}
	sources += [
		"yet_another_foo.rs",
	]
	if (is_fuchsia) {
		sources += [
			"yet_another_bar.rs",
		]
	}
}`,
		},
		{
			name: "consecutive selects",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "consecutive_selects",
	srcs = select(
		{
			"@platforms//os:fuchsia": [ "foo.rs" ],
			"@platforms//os:linux": [ "bar.rs" ],
		},
		no_match_error = "unknown platform!",
	) + select(
		{
			"@platforms//os:linux": [ "baz.rs" ],
			"//conditions:default": [ "qux.rs" ],
		},
	),
)`,
			wantGN: `rustc_library("consecutive_selects") {
	sources = []
	if (is_fuchsia) {
		sources += [
			"foo.rs",
		]
	} else if (is_linux) {
		sources += [
			"bar.rs",
		]
	} else {
		assert(false, "unknown platform!")
	}
	if (is_linux) {
		sources += [
			"baz.rs",
		]
	} else {
		sources += [
			"qux.rs",
		]
	}
}`,
		},
		{
			name: "branching assignment",
			bazel: `_FOO_DEPS = [
	"bar",
] + select({
	"@platforms//os:fuchsia": ["fuchsia_bar"],
	"//conditions:default": ["host_bar"],
})`,
			wantGN: `_FOO_DEPS = []
_FOO_DEPS += [
	"bar",
]
if (is_fuchsia) {
	_FOO_DEPS += [
		"fuchsia_bar",
	]
} else {
	_FOO_DEPS += [
		"host_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
		{
			name: "branching assignment without common element",
			bazel: `_FOO_DEPS = select({
	"@platforms//os:fuchsia": ["fuchsia_bar"],
	"//conditions:default": ["host_bar"],
})`,
			wantGN: `_FOO_DEPS = []
if (is_fuchsia) {
	_FOO_DEPS += [
		"fuchsia_bar",
	]
} else {
	_FOO_DEPS += [
		"host_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
		{
			name: "branching assignment with empty non-default",
			bazel: `_FOO_DEPS = [
	"bar",
] + select({
	"@platforms//os:fuchsia": [],
	"//conditions:default": ["host_bar"],
})`,
			wantGN: `_FOO_DEPS = []
_FOO_DEPS += [
	"bar",
]
if (is_fuchsia) {
	_FOO_DEPS += [
	]
} else {
	_FOO_DEPS += [
		"host_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
		{
			name: "branching assignment with empty default",
			bazel: `_FOO_DEPS = [
	"bar",
] + select({
	"@platforms//os:fuchsia": ["fuchsia_bar"],
	"//conditions:default": [],
})`,
			wantGN: `_FOO_DEPS = []
_FOO_DEPS += [
	"bar",
]
if (is_fuchsia) {
	_FOO_DEPS += [
		"fuchsia_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
		{
			name: "branching assignment with skipped non-default",
			bazel: `_FOO_DEPS = [
	"bar",
] + select({
	"@platforms//os:fuchsia": [
		# @bazel2gn:skip
		"fuchsia_bar",
	],
	"//conditions:default": ["host_bar"],
})`,
			wantGN: `_FOO_DEPS = []
_FOO_DEPS += [
	"bar",
]
if (is_fuchsia) {
	_FOO_DEPS += [
	]
} else {
	_FOO_DEPS += [
		"host_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
		{
			name: "branching assignment with skipped default",
			bazel: `_FOO_DEPS = [
	"bar",
] + select({
	"@platforms//os:fuchsia": ["fuchsia_bar"],
	"//conditions:default": [
		# @bazel2gn:skip
		"host_bar",
	],
})`,
			wantGN: `_FOO_DEPS = []
_FOO_DEPS += [
	"bar",
]
if (is_fuchsia) {
	_FOO_DEPS += [
		"fuchsia_bar",
	]
}

# To avoid "Assignment had no effect" from GN.
# It's possible this variable is only used in if conditions (e.g. is_host).
not_needed([ "_FOO_DEPS" ])
`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\n\n====== Bazel source ======\n\n%s\n\n====== Converted GN source ======\n\n%s\n", diff, tc.bazel, gotGN)
			}
		})
	}
}

func TestSelectConversionErrors(t *testing.T) {
	for _, tc := range []struct {
		name  string
		bazel string
	}{
		{
			name: "unsupported operator",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "unsupported_operator",
  srcs = select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"@platforms//os:linux": [ "linux.rs" ],
		"//conditions:default": [ "default.rs" ],
	}) - [ "minux.rs" ],
)`,
		},
		{
			name: "unsupported select condition",
			bazel: `load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
	name = "unsupported_condition",
	srcs = select({
		"unknown_condition": [],
		"//conditions:default": [ "default.rs" ],
	}),
)`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			if _, err := bazelToGN(f); err == nil {
				t.Fatalf("Unexpected success converting Bazel build targets, want failure")
			}
		})
	}
}
