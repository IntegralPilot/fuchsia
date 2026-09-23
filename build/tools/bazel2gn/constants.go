// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"maps"
	"regexp"

	"go.starlark.net/syntax"
)

// indentPrefix is the string value used to indent a line by one level.
//
// NOTE: This prefix is only used by the internal representation of this package.
// The final output is formatted by `gn format`.
const indentPrefix = "\t"

// clearAnnotation is a comment annotation that indicates the assignment is an explicit clear.
const clearAnnotation = "# @bazel2gn:clear"

// skipAnnotation is a comment annotation that indicates the statement should be skipped.
const skipAnnotation = "# @bazel2gn:skip"

// pathOverwriteAnnotationPrefix is a comment annotation prefix that indicates a path should be
// overwritten in GN. In this case bazel2gn will ignore the value set in the BUILD.bazel file
// and use the value specified in the annotation in the BUILD.gn file instead. The value specified
// in the annotation will be wrapped in double quotes.
const pathOverwriteAnnotationPrefix = "# @bazel2gn:path_overwrite:"

// rawOverwriteAnnotationPrefix is a comment annotation prefix that indicates a value should be
// overwritten with a raw GN expression.
const rawOverwriteAnnotationPrefix = "# @bazel2gn:raw_overwrite:"

// valueOverwriteAnnotationPrefix is a comment annotation prefix that indicates a value of a
// dictionary entry should be overwritten in GN. In this case bazel2gn will ignore the value set
// in the BUILD.bazel file and use the value specified in the annotation in the BUILD.gn file
// instead.
const valueOverwriteAnnotationPrefix = "# @bazel2gn:value_overwrite:"

// transformerAnnotationPrefix is a comment annotation prefix that indicates a transformer
// should be applied to the next statement.
const transformerAnnotationPrefix = "# @bazel2gn:transformer="

// transformerAnnotationNames maps from transformer annotation names to the transformer functions.
var transformerAnnotationNames = map[string]transformer{
	"visibility": bazelVisibilityToGN,
	"deps":       bazelDepToGN,
	"configs":    bazelCOptToGNConfig,
	"file_paths": bazelFilePathsToGN,
}

// bazelRuleToGNTemplate maps from Bazel rule names to GN template names. They can
// be the same if Bazel and GN shared the same template name.
//
// This map is also used to check known Bazel rules that can be converted to GN.
// i.e. Bazel rules not found in this map is not supported by bazel2gn yet.
var bazelRuleToGNTemplate = map[string]string{
	// Bazel primitives
	"alias": "group",

	// Go
	"go_binary":    "go_binary",
	"go_library":   "go_library",
	"go_test":      "go_test",
	"host_go_test": "go_test",

	// Python
	"py_library": "python_library",
	"py_binary":  "python_binary",

	// Rust
	"rust_binary":      "rustc_binary",
	"rust_library":     "rustc_library",
	"rustc_binary":     "rustc_binary",
	"rustc_library":    "rustc_library",
	"rustc_proc_macro": "rustc_macro",
	"rustc_test":       "rustc_test",
	"rust_proc_macro":  "rustc_macro",

	// C++
	"cc_library":            "static_library",
	"cc_binary":             "executable",
	"fx_cc_library_headers": "library_headers",
	"fx_cc_library":         "static_library",

	// C++ Zircon
	"cc_shared_library_zx": "zx_library", // With `sdk="shared"` and `sdk_publishable` not specified.
	"cc_source_library_zx": "zx_library", // With `sdk="source"` and `sdk_publishable` not specified.
	"cc_static_library_zx": "zx_library", // With `sdk="static"` and `sdk_publishable` not specified.

	// FIDL
	"fidl_library":        "fidl",
	"zither_fidl_library": "fidl",

	// Host tools
	"cc_binary_host_tool": "executable",
	"ffx_tool":            "ffx_tool",
	"ffx_plugin":          "ffx_plugin",
	"go_binary_host_tool": "go_binary",
	"install_host_tools":  "install_host_tools",

	// IDK
	"idk_cc_shared_library":      "sdk_shared_library",
	"idk_cc_shared_library_zx":   "zx_library", // With `sdk="shared"` and `sdk_publishable = "partner"`.
	"idk_cc_source_library":      "sdk_source_set",
	"idk_cc_source_library_zx":   "zx_library", // With `sdk="source"` and `sdk_publishable = "partner"`.
	"idk_cc_static_library":      "sdk_static_library",
	"idk_cc_static_library_zx":   "zx_library", // With `sdk="static"` and `sdk_publishable = "partner"`.
	"idk_cc_binary_host_tool":    "sdk_executable_host_tool",
	"idk_go_binary_host_tool":    "sdk_go_binary_host_tool",
	"idk_rustc_binary_host_tool": "sdk_rustc_binary_host_tool",

	// Other
	"fidlgentest_go_test": "fidlgentest_go_test",
	"genrule":             "action",
	"package":             "package",
	"test_suite":          "group",
	"stamp_group":         "group",

	// `exports_files()` is a concept specific to Bazel, so there is no need to convert it.
	"exports_files": "__NO_GN_EQUIVALENT__",
}

// attrsToOmitByRules stores a mapping from known Bazel rules to attributes to
// omit when converting them to GN.
var attrsToOmitByRules = map[string]map[string]bool{
	"go_library": {
		// In GN we default cgo to true when compiling Go code, and explicitly disable
		// it in very few places. However, in Bazel, cgo defaults to false, and
		// require users to explicitly set when C sources are included.
		"cgo": true,

		// go_library.gni doesn't allow overwriting `importpath`, and forces it to be the same as the
		// directory path from FUCHSIA_DIR. This limits our ability to define multiple go_libraries in
		// the same BUILD file. go_library.gni gets around this with `source_dir`, which does not
		// exist in Bazel. So we omit syncing this attribute and let GN infer it, which handles nested
		// directories correctly. In GN we omit `importpath` for almost all non-third-party go_libraries
		// anyways.
		//
		// This is safe to do because incorrectly configured importpath would cause Go compilation to
		// fail, so we will be alerted when importpaths are wrong in Bazel.
		"importpath": true,
	},
	"py_library": {
		// `imports` attribute is a Bazel-only concept. Our GN python_library.gni template
		// handles import paths differently than Bazel.
		"imports": true,
	},
	"genrule": {
		// bazel2gn ignores the `tools` attribute of genrule, and tries to parse it out of the `cmd`
		// attribute. It is the caller's responsibility to ensure that the `cmd` attribute contains
		// the correct tools. Also note since `genrule` is converted to `action` in GN, the `cmd`
		// attribute is converted to `script` and `args` in GN, so only one `tool` is supported.
		"tools": true,
	},
	// TODO(https://fxbug.dev/457605523): Support `includes` conversion to `configs` in GN.
	// Currently the only use case is to set `includes = ["../.."]`, which is covered by
	// `"//build/config:default_include_dirs"` in GN.
	"cc_library":               {"includes": true},
	"fx_cc_library":            {"includes": true},
	"idk_cc_source_library":    {"includes": true},
	"idk_cc_shared_library_zx": {"version_script": true},
	// We do not need to include the "stamp" destination in GN because it is implicitly created by
	// the rule.
	"stamp_group": {"stamp": true},
	// The Bazel test target attribute `size` is not supported in GN.
	"go_test":      {"size": true},
	"host_go_test": {"size": true},
	"rustc_test":   {"size": true},
}

// Common Bazel attributes that use different names in GN.
var commonAttrMap = map[string]string{
	"srcs": "sources",
	"hdrs": "public",
}

var aliasAttrMap = map[string]string{
	"actual": "public_deps",
}

// ccCommonAttrMap maps from attribute names common in Bazel CC rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var ccCommonAttrMap = map[string]string{
	"copts": "configs", // Strings are converted to configs by `bazelCOptToGNConfig()`.
}

// ccLibAttrMap maps from attribute names in Bazel CC library rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var ccLibAttrMap = mustMergeMaps(ccCommonAttrMap, map[string]string{
	"deps":                "public_deps",
	"implementation_deps": "deps",
})

// rustCommonAttrMap maps from attribute names common in Bazel Rust rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var rustCommonAttrMap = map[string]string{
	"compile_data":         "inputs",
	"data":                 "non_rust_deps",
	"crate_features":       "features",
	"with_host_unit_tests": "with_unit_tests",
	"rustc_flags":          "rustflags",
	"crate_root":           "source_root",
	"rustc_env":            "rustenv",
}

// rustBinAttrMap maps from attribute name in Bazel Rust binary rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var rustBinAttrMap = mustMergeMaps(rustCommonAttrMap, map[string]string{
	"crate_name": "output_name",
})

// fidlAttrMap maps from attribute names in Bazel FIDL rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var fidlAttrMap = map[string]string{
	"category":     "sdk_category",
	"deps":         "public_deps",
	"library_name": "name",
}

// idkAttrMap maps from attribute name in Bazel IDK rules to GN parameter names.
// This map only includes attributes that have different names in Bazel and GN.
var idkAttrMap = map[string]string{
	"api_area":      "sdk_area",
	"api_file_path": "api",
	"idk_name":      "sdk_name",

	// This renames the variables, but they must be made a `+=` inside a
	// conditional block in `attrAssignmentToGN()`. The default
	// platform-independent variable must be specified before these variables
	// in the `BUILD.bazel` file. This may require using
	// `# buildifier: leave-alone` above the macro.
	"fuchsia_deps":                "public_deps",
	"fuchsia_implementation_deps": "deps",
	"non_fuchsia_deps":            "public_deps",

	// These are not identical because files in `hdrs_for_internal_use` need
	// to be added to GN's `public` as well. This takes care of populating
	// `sdk_headers_for_internal_use`, and `attrAssignmentToGN()` adds the files
	// to `public`.
	"hdrs_for_internal_use": "sdk_headers_for_internal_use",
}

// Lists of attribute names that are specific to Fuchsia and non-Fuchsia builds.
// These are used to add appropriate conditions in the GN file.
var idkFuchsiaSpecificAttrs = map[string]bool{
	"fuchsia_deps":                true,
	"fuchsia_implementation_deps": true,
}
var idkNonFuchsiaSpecificAttrs = map[string]bool{
	"non_fuchsia_deps": true,
}

// Maps from attribute name in Bazel IDK rules to GN `zx_library()` parameter
// names for attributes unique to `zx_library()` instances that are in the IDK.
// Use `idkZxAttrMap` instead.
// This map only includes attributes that have different names in Bazel and GN.
var zxInIDKAttrMap = map[string]string{
	"category": "sdk_publishable",
}

// installHostToolAttrMap maps from attribute name in Bazel install host tool rules to GN parameter names.
var installHostToolAttrMap = map[string]string{
	"implementation_deps": "deps",
	"tool_output_names":   "outputs",
}

// genruleAttrMap maps from attribute name in Bazel genrule to GN parameter names.
var genruleAttrMap = map[string]string{
	"outs": "outputs",
}

// idkCcAttrMap maps from attribute name in Bazel IDK C++ rules to GN parameter names.
var idkCcAttrMap = mustMergeMaps(idkAttrMap, ccLibAttrMap)

// idkZxAttrMap maps from attribute name in Bazel IDK C++ ZX rules to GN parameter names.
var idkZxAttrMap = mustMergeMaps(idkCcAttrMap, zxInIDKAttrMap)

// idkFIDLAttrMap maps from attribute name in Bazel IDK FIDL rules to GN parameter names.
var idkFIDLAttrMap = mustMergeMaps(idkAttrMap, fidlAttrMap)

// idkRustBinAttrMap maps from attribute name in Bazel IDK Rust binary rules to GN parameter names.
var idkRustBinAttrMap = mustMergeMaps(idkAttrMap, rustBinAttrMap)

// pythonBinAttrMap maps from attribute name in Bazel Python binary rules to GN parameter names.
var pythonBinAttrMap = map[string]string{
	"main": "main_source",
}

// A mapping from Bazel rule names to attribute mappings.
// Attribute mappings map from Bazel rule attributes that use different names in GN.
var attrMapsByRules = map[string]map[string]string{
	// Bazel
	"alias": aliasAttrMap,

	// Python
	"py_binary": pythonBinAttrMap,

	// C++
	"cc_library":    ccLibAttrMap,
	"cc_binary":     ccCommonAttrMap,
	"fx_cc_library": ccLibAttrMap,

	// C++ Zircon
	"cc_shared_library_zx": ccLibAttrMap,
	"cc_source_library_zx": ccLibAttrMap,
	"cc_static_library_zx": ccLibAttrMap,

	// Rust
	"rust_binary":      rustBinAttrMap,
	"rust_library":     rustCommonAttrMap,
	"rust_proc_macro":  rustCommonAttrMap,
	"rustc_binary":     rustBinAttrMap,
	"rustc_library":    rustCommonAttrMap,
	"rustc_proc_macro": rustCommonAttrMap,
	"rustc_test":       rustCommonAttrMap,

	// FIDL
	"fidl_library":        idkFIDLAttrMap,
	"zither_fidl_library": fidlAttrMap,

	// IDK
	"idk_cc_shared_library":      idkCcAttrMap,
	"idk_cc_shared_library_zx":   idkZxAttrMap,
	"idk_cc_source_library":      idkCcAttrMap,
	"idk_cc_source_library_zx":   idkZxAttrMap,
	"idk_cc_static_library":      idkCcAttrMap,
	"idk_cc_static_library_zx":   idkZxAttrMap,
	"idk_cc_binary_host_tool":    idkAttrMap,
	"idk_go_binary_host_tool":    idkAttrMap,
	"idk_rustc_binary_host_tool": idkRustBinAttrMap,

	// Tools
	"ffx_tool":           rustBinAttrMap,
	"install_host_tools": installHostToolAttrMap,

	// Others
	"genrule": genruleAttrMap,
	"test_suite": {
		"tests": "deps",
	},
}

var extraGnExpressionByRules = map[string]string{
	"cc_shared_library_zx":     `sdk = "shared"`,
	"cc_source_library_zx":     `sdk = "source"`,
	"cc_static_library_zx":     `sdk = "static"`,
	"idk_cc_shared_library_zx": `sdk = "shared"`,
	"idk_cc_source_library_zx": `sdk = "source"`,
	"idk_cc_static_library_zx": `sdk = "static"`,
	"test_suite":               `testonly = true`,
}

// These identifiers with the same meanings are represented differently in Bazel
// and GN. specialIdentifiers maps from their Bazel representations to GN
// representations.
var specialIdentifiers = map[string]string{
	"True":  "true",
	"False": "false",
}

// specialTokens maps from special tokens in Bazel to their GN equivalents.
var specialTokens = map[syntax.Token]string{
	syntax.AND: "&&",
	syntax.OR:  "||",
}

// The following map from Bazel constraints to GN conditions.
//
// The first contains single constraints that appear in a list.
// The second contains variables representing an entire list.
//
// Only add new values when necessary as there are often more appropriate ways
// to express the same logic. For example, "HOST_CONSTRAINTS" is more
// appropriate than "@platforms//os:linux" in most cases. Also,
// "//build/bazel/platforms:fuchsia_platform_x64" may be more appropriate
// than using a list of "@platforms//os:fuchsia" and "@platforms//cpu:x86_64".
var bazelConstraintsToGNConditions = map[string]string{
	"@platforms//os:fuchsia": "is_fuchsia",
}
var bazelConstraintListVarsToGNConditions = map[string]string{
	"HOST_CONSTRAINTS":    "is_host",
	"HOST_OS_CONSTRAINTS": "is_host",
}

// thirdPartyRustCrateVendoredRE matches Bazel third-party Rust crate dependency prefixes for purely
// vendored crates. It is used to extract the crate name from the dependency path.
var thirdPartyRustCrateVendoredRE = regexp.MustCompile(`^"\/\/third_party\/rust_crates\/vendor:`)

// thirdPartyRustCrateModifiedRE matches Bazel third-party Rust crate dependency prefixes for
// modified crates. It is used to extract the directory name from the dependency path.
var thirdPartyRustCrateModifiedRE = regexp.MustCompile(`^"\/\/third_party\/rust_crates\/.+\/([a-zA-Z0-9_\.-]+):?[^"]*`)

//go:embed third_party_target_map.json
var thirdPartyTargetMapJSON []byte

// thirdPartyBazelRepos maps from Bazel third-party repository and target labels to their GN equivalent
// dependency paths. The key is the Bazel label, and the value is the GN dependency path.
var thirdPartyBazelRepos = func() map[string]string {
	var m map[string]string
	if err := json.Unmarshal(thirdPartyTargetMapJSON, &m); err != nil {
		panic(fmt.Sprintf("Failed to unmarshal third_party_target_map.json: %v", err))
	}
	return m
}()

// coptToConfig maps from Bazel copt values to configs to use in GN.
var coptToConfig = map[string]string{
	"-Wno-implicit-fallthrough": "//build/config:Wno-implicit-fallthrough",
	"-Wno-vla-cxx-extension":    "//build/config:Wno-vla-cxx-extension",
	"-Wno-conversion":           "//build/config:Wno-conversion",

	// The following are GN `configs` rather than `copt` values. These must be
	// allowed because they both appear as `configs` by the time this map is used.
	// TODO(https://fxbug.dev/421888626): Properly specify the relevant flags in
	// Bazel and convert those to GN configs.
	":ring-config": ":ring-config",
	"//build/config/fuchsia:no_cpp_standard_library": "//build/config/fuchsia:no_cpp_standard_library",
	"//build/config:all_source":                      "//build/config:all_source",
}

// attrGNAssignmentOps maps from GN attribute names to the assignment operators to use in GN.
//
// NOTE: Entries in this map should be clearly documented.
var attrGNAssignmentOps = map[string]string{
	// `configs` in GN are rarely (never?) empty lists because we set them in BUILDCONFIG.gn.
	// Trying to overwrite a non-empty list in GN with a non-empty value will fail.
	// Simply replacing assignment with `+=` works for the initial use cases we need.
	// More complex mechanism may be required if we need to selectively overwrite config assignments.
	"configs": "+=",
}

// goThirdPartyAggregateDeps lists known third-party GN targets that include sources of multiple Go
// libraries in a single target. These deps need to be handled specially because in Bazel we have a
// 1:1 mapping between go_library targets and Go libraries.
var goThirdPartyAggregateDeps = []string{
	"//third_party/golibs:golang.org/x/crypto",
	"//third_party/golibs:gonum.org/v1/gonum",
	"//third_party/golibs:google.golang.org/protobuf",
	"//third_party/golibs:gvisor.dev/gvisor",
}

// mustMergeMaps merges two input maps and return a new one with keys and values from
// both inputs.
//
// This function panics if m1 and m2 have duplicate keys. All these cases should be
// caught at build time so it's OK to panic here.
func mustMergeMaps(m1, m2 map[string]string) map[string]string {
	var m = maps.Clone(m1)
	for k, v := range m2 {
		if _, ok := m[k]; ok {
			panic(fmt.Sprintf("Duplicate key when merging maps: %q", k))
		}
		m[k] = v
	}
	return m
}
