// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"fmt"
	"os"
	"strings"

	"go.starlark.net/syntax"
)

// transformer is a function type that can be used by `exprToGN` to apply
// special transformations to expression nodes before conversion.
//
// This is useful for rewriting certain string values, recording values during
// traversal, or even restructuring the syntax tree.
type transformer func(syntax.Expr) (syntax.Expr, error)

// bazelVisibilityToGN converts Bazel visibility values [0] to GN [1].
//
// NOTE: Bazel visibility is based on package groups [2], while GN visibility is
// based on target. However it should be possible to create matching groups in
// GN for the exact same visibility control in the most granular cases.
//
// [0] https://bazel.build/concepts/visibility#visibility-specifications
// [1] https://gn.googlesource.com/gn/+/main/docs/reference.md#var_visibility
// [2] https://bazel.build/reference/be/functions#package_group
func bazelVisibilityToGN(expr syntax.Expr) (syntax.Expr, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return expr, nil
	}
	if raw, ok := overwrittenRaw(lit); ok {
		lit.Raw = raw
		return lit, nil
	}
	switch {
	case lit.Raw == `"//visibility:public"`:
		lit.Raw = `"*"`
	case lit.Raw == `"//visibility:private"`:
		lit.Raw = `":*"`
	case lit.Raw == `":__subpackages__"`:
		// The suffix match below would result in `"/*"`, which is equivalent to
		// public, so we must handle this case explicitly.
		lit.Raw = `"./*"`
	case lit.Raw == `"//:__subpackages__"`:
		// The suffix match below would result in `"///*"` so we must handle this case explicitly.
		lit.Raw = `"//*"`
	case strings.HasSuffix(lit.Raw, `:__pkg__"`):
		lit.Raw = strings.ReplaceAll(lit.Raw, `:__pkg__"`, `:*"`)
	case strings.HasSuffix(lit.Raw, `:__subpackages__"`):
		lit.Raw = strings.ReplaceAll(lit.Raw, `:__subpackages__"`, `/*"`)
	default:
		// This is a Bazel visibility on a package group, it should stay unchanged.
		// In GN there should be a target matching the path of this package group.
	}
	return lit, nil
}

// bazelCOptToGNConfig converts Bazel copts values to GN configs.
func bazelCOptToGNConfig(expr syntax.Expr) (syntax.Expr, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return expr, nil
	}
	coptWithoutQuotes := lit.Raw[1 : len(lit.Raw)-1]
	config, ok := coptToConfig[coptWithoutQuotes]
	if !ok {
		return nil, fmt.Errorf("unexpected copt %s", coptWithoutQuotes)
	}
	lit.Raw = fmt.Sprintf(`"%s"`, config)
	return lit, nil
}

// bazelDepToGN converts Bazel dependency paths to GN ones.
//
// This function is expected to encapsulate information specific to the Fuchsia
// build system. Ideally the problem solved here should be solved in the build
// system (e.g. move location of build files), so this tool packs less surprises.
func bazelDepToGN(expr syntax.Expr) (syntax.Expr, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return expr, nil
	}

	if dep, ok := overwrittenPath(lit); ok {
		lit.Raw = fmt.Sprintf(`"%s"`, dep)
		return lit, nil
	}

	rawDep := strings.Trim(lit.Raw, `"`)
	if gnDep, ok := thirdPartyBazelRepos[rawDep]; ok {
		lit.Raw = fmt.Sprintf(`"%s"`, gnDep)
		return lit, nil
	}

	if strings.HasPrefix(rawDep, "@") {
		fmt.Fprintf(os.Stderr, "WARNING: unable to translate Bazel label %s to GN\n", rawDep)
		lit.Raw = fmt.Sprintf(`"%s" # BAZEL2GN_WARNING: Unknown Bazel repository name`, rawDep)
		return lit, nil
	}

	lit.Raw = thirdPartyRustCrateVendoredRE.ReplaceAllString(
		lit.Raw,
		`"//third_party/rust_crates:`,
	)

	lit.Raw = thirdPartyRustCrateModifiedRE.ReplaceAllString(
		lit.Raw,
		`"//third_party/rust_crates:$1`,
	)

	// Replace matching Go third-party dependencies with aggregate dependencies.
	//
	// For example:
	//
	//   "//third_party/golibs:golang.org/x/crypto/ssh" -> "//third_party/golibs:golang.org/x/crypto"
	//   "//third_party/golibs:google.golang.org/protobuf/encoding/protojson" -> "//third_party/golibs:google.golang.org/protobuf"
	//
	// Note this can result in duplicate deps in GN, which should be fixed in
	// later steps in `bazel2gn` by `gn format`.
	for _, dep := range goThirdPartyAggregateDeps {
		if strings.HasPrefix(lit.Raw, fmt.Sprintf(`"%s/`, dep)) {
			lit.Raw = fmt.Sprintf(`"%s"`, dep)
			break
		}
	}

	return lit, nil
}

// overwrittenPath returns the path overwritten by comments, if any.
//
// It returns the path and true if the path is overwritten, otherwise it returns
// an empty string and false.
func overwrittenPath(lit *syntax.Literal) (string, bool) {
	comments := lit.Comments()
	if comments != nil {
		for _, c := range comments.Suffix {
			if strings.HasPrefix(c.Text, pathOverwriteAnnotationPrefix) {
				return strings.TrimSpace(c.Text[len(pathOverwriteAnnotationPrefix):]), true
			}
		}
	}
	return "", false
}

// overwrittenRaw returns the raw expression overwritten by comments, if any.
//
// It returns the raw expression and true if it is overwritten, otherwise it returns
// an empty string and false.
func overwrittenRaw(node syntax.Node) (string, bool) {
	comments := node.Comments()
	if comments != nil {
		for _, c := range comments.Suffix {
			if strings.HasPrefix(c.Text, rawOverwriteAnnotationPrefix) {
				return strings.TrimSpace(c.Text[len(rawOverwriteAnnotationPrefix):]), true
			}
		}
	}
	return "", false
}

// overwrittenValue returns the value overwritten by comments, if any.
//
// It returns the value and true if the value is overwritten, otherwise it returns
// an empty string and false.
func overwrittenValue(entry *syntax.DictEntry) (string, bool) {
	if comments := entry.Comments(); comments != nil {
		for _, c := range comments.Suffix {
			if strings.HasPrefix(c.Text, valueOverwriteAnnotationPrefix) {
				return strings.TrimSpace(c.Text[len(valueOverwriteAnnotationPrefix):]), true
			}
		}
	}
	return "", false
}

// bazelFilePathsToGN converts Bazel file paths to GN file paths, handling
// overwritten paths.
func bazelFilePathsToGN(expr syntax.Expr) (syntax.Expr, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return expr, nil
	}
	if path, ok := overwrittenPath(lit); ok {
		lit.Raw = fmt.Sprintf(`"%s"`, path)
	}
	lit.Raw = strings.Replace(lit.Raw, ":", "/", 1)
	return lit, nil
}

// bazelLdflagsToGN converts Bazel ldflags to GN, supporting overwrites.
func bazelLdflagsToGN(expr syntax.Expr) (syntax.Expr, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return expr, nil
	}
	if flag, ok := overwrittenRaw(lit); ok {
		lit.Raw = flag
		return lit, nil
	}
	if _, ok := overwrittenPath(lit); ok {
		// TODO(https://fxbug.dev/543568916): Replace with a general solution for unhandled annotations.
		return nil, fmt.Errorf("use %q instead of %q to overwrite `ldflags`",
			rawOverwriteAnnotationPrefix, pathOverwriteAnnotationPrefix)
	}
	return lit, nil
}

// listifiedAnnotation is a comment annotation we add to expressions after we
// convert them to a list. This prevents subsequent transformations from
// wrapping the already-listified value in another layer of list.
const listifiedAnnotation = "# @bazel2gn:listified"

func addListifiedAnnotation(expr syntax.Expr) {
	expr.AllocComments()
	comments := expr.Comments()
	comments.Before = append(comments.Before, syntax.Comment{Text: listifiedAnnotation})
}

func bazelExprToGNList(expr syntax.Expr) (syntax.Expr, error) {
	expr = unwrapParenExpr(expr)
	if comments := expr.Comments(); comments != nil {
		for _, c := range comments.Before {
			if c.Text == listifiedAnnotation {
				return expr, nil
			}
		}
	}

	addListifiedAnnotation(expr)

	list := syntax.ListExpr{List: []syntax.Expr{expr}}
	addListifiedAnnotation(&list)

	return &list, nil
}
