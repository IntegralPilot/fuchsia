// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"fmt"

	"go.starlark.net/syntax"
)

// defaultBazelSelectCondition is the default match-the-rest select condition.
const defaultBazelSelectCondition = "//conditions:default"

// bazelSelectConditionToGN maps from Bazel select conditions (the keys of the
// dictionaries to select calls) to GN condition variables (the condition to
// check in if statements).
var bazelSelectConditionToGN = map[string]string{
	"@platforms//os:fuchsia":                            "is_fuchsia",
	"@platforms//os:linux":                              "is_linux",
	"@platforms//cpu:x86_64":                            `current_cpu == "x64"`,
	"@platforms//cpu:arm64":                             `current_cpu == "arm64"`,
	"@platforms//cpu:riscv64":                           `current_cpu == "riscv64"`,
	"//build/bazel/platforms:is_host_os":                "is_host",
	"//build/bazel/platforms:is_fuchsia_with_sdk_rules": "false", // GN build is always platform.
	"//build/bazel/platforms:is_fuchsia_platform":       "is_fuchsia",
	"//build/bazel/platforms:is_fuchsia_x64":            `is_fuchsia && current_cpu == "x64"`,
	"//build/bazel/platforms:is_fuchsia_arm64":          `is_fuchsia && current_cpu == "arm64"`,
	"//build/bazel/platforms:is_fuchsia_riscv64":        `is_fuchsia && current_cpu == "riscv64"`,
	"//build/bazel/platforms:is_linux_x64":              `is_linux && current_cpu == "x64"`,
	"//build/bazel/platforms:is_linux_arm64":            `is_linux && current_cpu == "arm64"`,
}

// Returns true iff select call or conditional expression are found in the subtree of `expr`.
func hasBranching(expr syntax.Expr) bool {
	expr = unwrapParenExpr(expr)
	if isSelectCall(expr) {
		return true
	}
	if _, ok := expr.(*syntax.CondExpr); ok {
		return true
	}
	binaryExpr, ok := expr.(*syntax.BinaryExpr)
	if ok {
		return hasBranching(binaryExpr.X) || hasBranching(binaryExpr.Y)
	}
	return false
}

// Returns true iff the input expression is a select call.
func isSelectCall(expr syntax.Expr) bool {
	expr = unwrapParenExpr(expr)
	fn, ok := expr.(*syntax.CallExpr)
	if !ok {
		return false
	}
	return fn.Fn.(*syntax.Ident).Name == "select"
}

// Adds additional statements needed for `sdk_headers_for_internal_use`.
// While the files in GN's `sdk_headers_for_internal_use` are included
// in `public` (or `sources`), that is not the case for Bazel's
// `hdrs_for_internal_use`. To match the GN behavior, add all the files
// specified in `hdrs_for_internal_use` to `public` in the GN target.
// For more information, see `idk_cc_source_library()`.
func handle_sdk_headers_for_internal_use(ret []string, rhs []string, indentLevel int) []string {
	ret = append(ret, indent([]string{fmt.Sprintf("public += %s", rhs[0])}, indentLevel)...)
	ret = append(ret, indent(rhs[1:], indentLevel)...)
	return ret
}

// Converts list concatenation with select calls in them to GN.
func listConcatWithSelectToGN(attrName string, expr syntax.Expr, transformers []transformer) ([]string, error) {
	for _, ts := range transformers {
		var err error
		expr, err = ts(expr)
		if err != nil {
			return nil, fmt.Errorf("applying special handler before converting list concatenation with select: %v", err)
		}
	}

	expr = unwrapParenExpr(expr)

	switch v := expr.(type) {
	case *syntax.CallExpr:
		return selectToGN(attrName, "+=", v, transformers)
	case *syntax.CondExpr:
		return condExprToGN(attrName, "+=", v, transformers)
	case *syntax.ListExpr:
		l, err := listExprToGN(v, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting list expression: %v", err)
		}
		ret := append(
			[]string{fmt.Sprintf("%s += %s", attrName, l[0])},
			l[1:]...,
		)

		if attrName == "sdk_headers_for_internal_use" {
			ret = handle_sdk_headers_for_internal_use(ret, l, 0)
		}

		return ret, nil
	case *syntax.BinaryExpr:
		if v.Op != syntax.PLUS {
			return nil, fmt.Errorf("only list concatenation are support, found unexpected operator %s", v.Op)
		}
		// Traversal order is important here. Left-first to ensure the order of list
		// items, which is important in attributes like copts.
		lhs, err := listConcatWithSelectToGN(attrName, v.X, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting lhs of concatenation: %v", err)
		}
		rhs, err := listConcatWithSelectToGN(attrName, v.Y, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting rhs of concatenation: %v", err)
		}
		return append(lhs, rhs...), nil
	case *syntax.Ident:
		l, err := identToGN(v)
		if err != nil {
			return nil, fmt.Errorf("converting identifier: %v", err)
		}
		ret := []string{fmt.Sprintf("%s += %s", attrName, l[0])}

		if attrName == "sdk_headers_for_internal_use" {
			ret = handle_sdk_headers_for_internal_use(ret, l, 0)
		}

		return ret, nil

	default:
		return nil, fmt.Errorf("converting list concatenation with select to GN, want call expression, binary expression, list expression, or identifier, got %T", expr)
	}
}

// Convert select nodes (which are syntax.CallExpr) to GN fragments.
func selectToGN(attrName string, op string, expr *syntax.CallExpr, transformers []transformer) ([]string, error) {
	fn := expr.Fn.(*syntax.Ident)
	if fn.Name != "select" {
		return nil, fmt.Errorf("want select call, got %s", fn.Name)
	}
	if len(expr.Args) == 0 {
		return nil, fmt.Errorf("no args found for select, expect at least one arg")
	}

	var ret []string
	for _, entry := range expr.Args[0].(*syntax.DictExpr).List {
		e := entry.(*syntax.DictEntry)
		key, ok := e.Key.(*syntax.Literal)
		if !ok {
			return nil, fmt.Errorf("only literals are supported as keys in dictionaries in selects")
		}

		// key.Raw is quoted, so unquote to get the string value.
		selectCondition := key.Raw[1 : len(key.Raw)-1]

		valueInGN, err := exprToGN(e.Value, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting dictionary value in select to GN: %v", err)
		}

		if selectCondition == defaultBazelSelectCondition {
			if len(ret) == 0 {
				return nil, fmt.Errorf("default select condition %q found with no other matching cases", selectCondition)
			}
			if op == "+=" && len(valueInGN) == 2 && valueInGN[0] == "[" && valueInGN[1] == "]" {
				// The default case is just adding an empty list, so skip it.
				continue
			}
			ret[len(ret)-1] += " else {"
		} else {
			gnCondition, ok := bazelSelectConditionToGN[selectCondition]
			if !ok {
				return nil, fmt.Errorf("unknown Bazel select condition %q in select", selectCondition)
			}
			if len(ret) > 0 {
				ret[len(ret)-1] += fmt.Sprintf(" else if (%s) {", gnCondition)
			} else {
				ret = append(ret, fmt.Sprintf("if (%s) {", gnCondition))
			}
		}

		ret = append(ret, indent(
			[]string{fmt.Sprintf("%s %s %s", attrName, op, valueInGN[0])},
			1,
		)...)
		ret = append(ret, indent(valueInGN[1:], 1)...)

		if attrName == "sdk_headers_for_internal_use" {
			ret = handle_sdk_headers_for_internal_use(ret, valueInGN, 1)
		}

		ret = append(ret, "}")
	}

	if len(expr.Args) > 1 {
		noMatchError, ok := expr.Args[1].(*syntax.BinaryExpr)
		if !ok || noMatchError.X.(*syntax.Ident).Name != "no_match_error" {
			return nil, fmt.Errorf("the second arg of select must be `no_match_error`, got %q", noMatchError.X.(*syntax.Ident).Name)
		}

		errStr, ok := noMatchError.Y.(*syntax.Literal)
		if !ok {
			return nil, fmt.Errorf("value of `no_match_error` must be a literal string, got %T", noMatchError.Y)
		}

		ret[len(ret)-1] += " else {"
		ret = append(ret, indent(
			[]string{
				// NOTE: raw is quoted so we don't need to quote again.
				fmt.Sprintf("assert(false, %s)", errStr.Raw),
			}, 1)...,
		)
		ret = append(ret, "}")
	}
	return ret, nil
}

// condExprToGN converts conditional expressions (syntax.CondExpr) to GN fragments.
func condExprToGN(attrName string, op string, expr *syntax.CondExpr, transformers []transformer) ([]string, error) {
	gnConditionLines, err := exprToGN(expr.Cond, nil)
	if err != nil {
		return nil, fmt.Errorf("converting condition in cond expr to GN: %v", err)
	}
	if len(gnConditionLines) != 1 {
		return nil, fmt.Errorf("expected single line for condition, got %d lines", len(gnConditionLines))
	}
	gnCondition := gnConditionLines[0]

	var trueInGN []string
	if hasBranching(expr.True) {
		lines, err := listConcatWithSelectToGN(attrName, expr.True, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting true branch in conditional to GN: %v", err)
		}
		if op == "=" {
			trueInGN = append([]string{fmt.Sprintf("%s = []", attrName)}, lines...)
		} else {
			trueInGN = lines
		}
	} else {
		lines, err := exprToGN(expr.True, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting true branch in conditional to GN: %v", err)
		}
		trueInGN = append([]string{fmt.Sprintf("%s %s %s", attrName, op, lines[0])}, lines[1:]...)
		if attrName == "sdk_headers_for_internal_use" {
			trueInGN = handle_sdk_headers_for_internal_use(trueInGN, lines, 0)
		}
	}

	var falseInGN []string
	if hasBranching(expr.False) {
		lines, err := listConcatWithSelectToGN(attrName, expr.False, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting false branch in conditional to GN: %v", err)
		}
		if op == "=" {
			falseInGN = append([]string{fmt.Sprintf("%s = []", attrName)}, lines...)
		} else {
			falseInGN = lines
		}
	} else {
		lines, err := exprToGN(expr.False, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting false branch in conditional to GN: %v", err)
		}
		if op == "+=" && len(lines) == 2 && lines[0] == "[" && lines[1] == "]" {
			falseInGN = nil
		} else {
			falseInGN = append([]string{fmt.Sprintf("%s %s %s", attrName, op, lines[0])}, lines[1:]...)
			if attrName == "sdk_headers_for_internal_use" {
				falseInGN = handle_sdk_headers_for_internal_use(falseInGN, lines, 0)
			}
		}
	}

	var ret []string
	ret = append(ret, fmt.Sprintf("if (%s) {", gnCondition))
	ret = append(ret, indent(trueInGN, 1)...)

	if falseInGN == nil {
		ret = append(ret, "}")
	} else {
		ret = append(ret, "} else {")
		ret = append(ret, indent(falseInGN, 1)...)
		ret = append(ret, "}")
	}

	return ret, nil
}
