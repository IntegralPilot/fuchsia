// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"errors"
	"fmt"
	"strings"

	"go.starlark.net/syntax"
)

// Parse takes a path to a starlark file and returns a parsed AST.
//
// This delegates to the starlark parser library we are using.
// Create a function wrapper here to capture the settings and modes we use during parsing.
func Parse(path string) (*syntax.File, error) {
	opts := syntax.FileOptions{
		// Empty means default file-level settings for parsing.
	}
	return opts.Parse(path, nil, syntax.RetainComments)
}

// indent indents input lines by input levels.
func indent(lines []string, level int) []string {
	var indented []string
	prefix := strings.Repeat(indentPrefix, level)
	for _, l := range lines {
		indented = append(indented, prefix+l)
	}
	return indented
}

// unwrapParenExpr removes any surrounding parentheses from an expression.
func unwrapParenExpr(expr syntax.Expr) syntax.Expr {
	for {
		paren, ok := expr.(*syntax.ParenExpr)
		if !ok {
			return expr
		}
		expr = paren.X
	}
}

// StmtToGN converts a Bazel statement [0] to GN.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#statements
func StmtToGN(stmt syntax.Stmt) ([]string, error) {
	// Skip the statement if it is mark for skipping by users.
	shouldSkip, err := hasSkipAnnotation(stmt)
	if err != nil {
		return nil, fmt.Errorf("failed to check skip annotation: %v", err)
	}
	if shouldSkip {
		return nil, nil
	}

	switch v := stmt.(type) {
	case *syntax.LoadStmt:
		// Load statements are ignored during conversion.
		return nil, nil
	case *syntax.ExprStmt:
		return exprToGN(v.X, nil)
	case *syntax.AssignStmt:
		return assignStmtToGN(v)
	default:
		return nil, fmt.Errorf("statement of type %T is not supported to be converted to GN, node details: %#v", stmt, stmt)
	}
}

// exprToGN converts a Bazel expression [0] to GN.
//
// It applies input transformers first before delegating to more specific
// conversion functions based on expression type.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#expressions
func exprToGN(expr syntax.Expr, transformers []transformer) ([]string, error) {
	for _, ts := range transformers {
		var err error
		expr, err = ts(expr)
		if err != nil {
			return nil, fmt.Errorf("applying special handler before converting expression: %v", err)
		}
	}

	switch v := expr.(type) {
	case *syntax.CallExpr:
		// NOTE: I'm not sure whether we need to plumb transformers here, so far it
		// is not necessary. callExprToGN should be a top-level entry point for
		// macro and rules.
		return callExprToGN(v)
	case *syntax.BinaryExpr:
		return binaryExprToGN(v, transformers)
	case *syntax.Ident:
		return identToGN(v)
	case *syntax.Literal:
		return []string{v.Raw}, nil
	case *syntax.ListExpr:
		return listExprToGN(v, transformers)
	case *syntax.DictExpr:
		return dictExprToGN(v, transformers)
	case *syntax.ParenExpr:
		return exprToGN(v.X, transformers)
	default:
		return nil, fmt.Errorf("expression of type %T is not supported when converting to GN, node details: %#v", expr, expr)
	}
}

// identToGN converts a Bazel identifier to GN.
func identToGN(ident *syntax.Ident) ([]string, error) {
	val, ok := specialIdentifiers[ident.Name]
	if !ok {
		val = ident.Name
	}
	return []string{val}, nil
}

// assignStmtToGN converts a Bazel assignment statement [0] to GN.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#assignments
func assignStmtToGN(stmt *syntax.AssignStmt) ([]string, error) {
	lhs, err := exprToGN(stmt.LHS, nil)
	if err != nil {
		return nil, fmt.Errorf("converting lhs of assignment statement: %v", err)
	}
	if len(lhs) == 0 {
		return nil, errors.New("lhs of assignment statement is unexpectedly empty")
	}

	transformers, err := transformersFromComments(stmt.Comments())
	if err != nil {
		return nil, err
	}

	var ret []string
	if hasBranching(stmt.RHS) {
		lc, err := listConcatWithSelectToGN(lhs[0], stmt.RHS, transformers)
		if err != nil {
			return nil, err
		}
		if stmt.Op == syntax.EQ {
			ret = append(ret, fmt.Sprintf("%s = []", lhs[0]))
		}
		ret = append(ret, lc...)
	} else {
		rhs, err := exprToGN(stmt.RHS, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting rhs of assignment statement: %v", err)
		}
		if len(rhs) == 0 {
			return nil, errors.New("rhs of assignment statement is unexpectedly empty")
		}

		ret = append(ret, fmt.Sprintf("%s %s %s", lhs[0], opToGN(stmt.Op), rhs[0]))
		ret = append(ret, rhs[1:]...)
	}

	ret = append(ret, []string{
		"",
		`# To avoid "Assignment had no effect" from GN.`,
		`# It's possible this variable is only used in if conditions (e.g. is_host).`,
		fmt.Sprintf(`not_needed([ "%s" ])`, lhs[0]),
		"",
	}...)

	return ret, nil
}

// targetCompatibleWithToGNConditions converts a Bazel `target_compatible_with`
// expression to GN conditions.
//
// It supports identifiers representing a list of constraints and lists of
// constraints containing literals and identifiers.
// It does not support concatenating multiple lists of constraints.
func targetCompatibleWithToGNConditions(expr syntax.Expr) ([]string, error) {
	switch v := expr.(type) {
	case *syntax.Ident:
		// An identifier not in a list. It must be a variable representing a list.
		gnCondition, ok := bazelConstraintListVarsToGNConditions[v.Name]
		if !ok {
			return nil, fmt.Errorf("unsupported target_compatible_with variable: %v", v.Name)
		}
		return []string{gnCondition}, nil
	case *syntax.ListExpr:
		var gnConditions []string
		for _, elm := range v.List {
			switch elm := elm.(type) {
			case *syntax.Literal:
				val, ok := elm.Value.(string)
				if !ok {
					return nil, fmt.Errorf("unexpected literal type in target_compatible_with: %T", elm.Value)
				}
				gnCondition, ok := bazelConstraintsToGNConditions[val]
				if !ok {
					return nil, fmt.Errorf("unsupported target_compatible_with variable: %v", val)
				}
				gnConditions = append(gnConditions, gnCondition)
			case *syntax.Ident:
				gnCondition, ok := bazelConstraintsToGNConditions[elm.Name]
				if !ok {
					return nil, fmt.Errorf("unsupported target_compatible_with variable: %v", elm.Name)
				}
				gnConditions = append(gnConditions, gnCondition)
			default:
				return nil, fmt.Errorf("unsupported expression type in target_compatible_with list: %T", elm)
			}
		}
		return gnConditions, nil
	default:
		return nil, fmt.Errorf("unsupported type %T as value to target_compatible_with in Bazel, node details: %#v", expr, expr)
	}
}

// callExprToGN converts a Bazel call expression [0] to GN. These calls should
// be macro or Bazel rules known to the converter.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#function-and-method-calls
func callExprToGN(expr *syntax.CallExpr) ([]string, error) {
	fn := expr.Fn.(*syntax.Ident)
	bazelRule := fn.Name
	gnTemplateName, ok := bazelRuleToGNTemplate[bazelRule]
	if !ok {
		return nil, fmt.Errorf("%s is not a known Bazel rule to convert to GN", bazelRule)
	}

	if gnTemplateName == "__NO_GN_EQUIVALENT__" {
		return nil, nil
	}

	// TODO(jayzhuang): Handle package level settings, e.g. visibility.
	if bazelRule == "package" {
		return nil, nil
	}

	attrsToOmit := attrsToOmitByRules[bazelRule]

	// Loops through all arguments to handle special ones first.
	var name string
	var remainingArgs []*syntax.BinaryExpr
	var wrappingConditions []string
	for _, arg := range expr.Args {
		binaryExpr, ok := arg.(*syntax.BinaryExpr)
		if !ok || binaryExpr.Op != syntax.EQ {
			return nil, fmt.Errorf("only attribute assignments (e.g. `attr = value`) are allowed in Bazel targets to be converted to GN, got %#v", arg)
		}

		// Skip the binary expression if it is marked for skipping.
		shouldSkip, err := hasSkipAnnotation(binaryExpr)
		if err != nil {
			return nil, fmt.Errorf("failed to check skip annotation: %v", err)
		}
		if shouldSkip {
			continue
		}

		ident, ok := binaryExpr.X.(*syntax.Ident)
		if !ok {
			return nil, fmt.Errorf("unexpected node type on the left hand side of binary expression in target definition, want syntax.Ident, got %T", binaryExpr.X)
		}
		if attrsToOmit[ident.Name] {
			continue
		}
		if ident.Name == "name" {
			lines, err := exprToGN(binaryExpr.Y, nil)
			if err != nil {
				return nil, fmt.Errorf("converting target name: %v", err)
			}
			name = strings.Join(lines, "\n")

			continue
		}
		if ident.Name == "target_compatible_with" {
			var err error
			wrappingConditions, err = targetCompatibleWithToGNConditions(binaryExpr.Y)
			if err != nil {
				return nil, fmt.Errorf("converting Bazel target_compatible_with to GN conditions: %v", err)
			}
			continue
		}
		isCCLibrary := bazelRule == "cc_library" || bazelRule == "fx_cc_library"
		if isCCLibrary && ident.Name == "alwayslink" {
			// Convert `cc_library` or `fx_cc_library` with `alwayslink = True` to `source_set`.
			if rhsIdent, ok := binaryExpr.Y.(*syntax.Ident); ok && rhsIdent.Name == "True" {
				gnTemplateName = "source_set"
			}
			// Regardless of its value, `alwayslink` is not field in GN, so ignore it.
			continue
		}
		remainingArgs = append(remainingArgs, binaryExpr)
	}
	if name == "" {
		return nil, errors.New("missing `name` attribute in Bazel target")
	}

	ret := []string{fmt.Sprintf("%s(%s) {", gnTemplateName, name)}

	// Loop through all args again to actually build the content of this target.
	for _, arg := range remainingArgs {
		lines, err := attrAssignmentToGN(arg, bazelRule)
		if err != nil {
			return nil, fmt.Errorf("converting Bazel attribute to GN: %v", err)
		}
		ret = append(ret, indent(lines, 1)...)
	}

	if extra, ok := extraGnExpressionByRules[bazelRule]; ok {
		ret = append(ret, indent([]string{extra}, 1)...)
	}

	ret = append(ret, "}")
	if len(wrappingConditions) > 0 {
		ret = append([]string{
			fmt.Sprintf("if (%s) {", strings.Join(wrappingConditions, " && ")),
		}, indent(ret, 1)...)
		ret = append(ret, "}")
	}
	return ret, nil
}

// convertAttrName converts input Bazel attrName to the corresponding parameter name in GN.
//
// This function takes the current Bazel rule being converted into consideration.
func convertAttrName(attrName, bazelRule string) string {
	ret, ok := commonAttrMap[attrName]
	if ok {
		return ret
	}
	ruleAttrMap, ok := attrMapsByRules[bazelRule]
	if !ok {
		return attrName
	}
	ret, ok = ruleAttrMap[attrName]
	if ok {
		return ret
	}
	return attrName
}

// hasClearAnnotation returns true if the input expr has a clear annotation comment.
//
// This function only picks up suffix comments, for example:
//
// ```
//
//	# This comment is NOT considered
//	copts = [] # This comment is considered
//
//	copts = [ # This comment is NOT considered
//	] # This comment is considered
//
// ```
func hasClearAnnotation(expr syntax.Expr) bool {
	comments := expr.Comments()
	if comments != nil {
		for _, c := range comments.Suffix {
			if c.Text == clearAnnotation {
				return true
			}
		}
	}
	return false
}

// attrAssignmentToGN converts a Bazel assignment [0] to GN. These assignments
// are used to assign values to fields during target definitions in Bazel.
//
// This function handles the special cases in attribute assignments, e.g. when
// select calls are involved. This is done through applying node transformers
// funcs during the traversal.
//
// NOTE: Assignment is a special binary expression with operator =.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#assignments
func attrAssignmentToGN(expr *syntax.BinaryExpr, bazelRule string) ([]string, error) {
	lhs, ok := expr.X.(*syntax.Ident)
	if !ok {
		return nil, fmt.Errorf("expecting an identifier on the left hand side of attribute assignment, got %T", expr.X)
	}
	attrName := convertAttrName(lhs.Name, bazelRule)

	// First, handle overwrite annotations for the entire right hand side.
	//
	// This requires determining which assignment operator to use now even though
	// we may not end up using it.
	op, ok := attrGNAssignmentOps[attrName]
	if !ok {
		op = "="
	}

	if idkFuchsiaSpecificAttrs[lhs.Name] || idkNonFuchsiaSpecificAttrs[lhs.Name] {
		op = "+="
	}

	// By default "configs" uses += to concatenate values set in Bazel with =.
	// Check for clear annotation to explicitly clear configs in GN, which
	// requires using =.
	//
	// TODO(https://fxbug.dev/430953918): Figure out a better way to handle configs and public_configs conversion.
	if attrName == "configs" && hasClearAnnotation(expr) {
		op = "="
	}

	// Now handle the overwrite annotation. The right hand side will be exactly what is in the
	// annotation. No transformations, special handling of select() or conditional expressions,
	// or wrapping (implemented below) will be performed.
	if raw, ok := overwrittenRaw(expr); ok {
		return []string{fmt.Sprintf("%s %s %s", attrName, op, raw)}, nil
	}

	// Intercept genrule cmd assignment and convert it directly.
	//
	// NOTE: This means bazel2gn does NOT support select calls in `cmd` currently.
	if bazelRule == "genrule" && attrName == "cmd" {
		return genruleCmdToGN(expr.Y)
	}

	// Intercept tags attribute to sync assert_no_deps.
	if attrName == "tags" {
		return tagsToGN(expr.Y)
	}

	// Intercept rustenv attribute to convert dict to list of strings.
	if attrName == "rustenv" {
		return rustenvToGN(expr.Y)
	}

	transformers, err := transformersFromComments(expr.Comments())
	if err != nil {
		return nil, err
	}

	switch attrName {
	case "visibility":
		transformers = append(transformers, bazelVisibilityToGN)
	case "deps", "test_deps", "proc_macro_deps", "args_deps", "plugin_deps":
		transformers = append(transformers, bazelDepToGN)
	case "public_deps":
		transformers = append(transformers, bazelDepToGN)
		if bazelRule == "alias" {
			// `alias` rules map from a single path in bazel to a list in GN
			transformers = append(transformers, bazelExprToGNList)
		}
	case "configs":
		transformers = append(transformers, bazelCOptToGNConfig)
	case "api", "outputs", "sources", "inputs", "args_sources":
		transformers = append(transformers, bazelFilePathsToGN)
	case "ldflags":
		transformers = append(transformers, bazelLdflagsToGN)
	}

	rhsY := unwrapParenExpr(expr.Y)

	// This is a simple `attr = select(...)`, convert in-place.
	if isSelectCall(rhsY) {
		return selectToGN(attrName, op, rhsY.(*syntax.CallExpr), transformers)
	}

	if condExpr, ok := rhsY.(*syntax.CondExpr); ok {
		return condExprToGN(attrName, op, condExpr, transformers)
	}

	// It is not a simple `select` call on the RHS, and `select`s are found in
	// subtree, so assume this is list concatenation with `select`s in them.
	//
	// NOTE: Currently `select`s and `if`s are only supported in list concatenation when
	// they are used in binary expressions. Other usages will fail this call.
	if hasBranching(rhsY) {
		lc, err := listConcatWithSelectToGN(attrName, rhsY, transformers)
		if err != nil {
			return nil, err
		}
		// Start with an empty list so it's easy to += new elements from later
		// conversions.
		return append([]string{fmt.Sprintf("%s %s []", attrName, op)}, lc...), nil
	}

	rhs, err := exprToGN(expr.Y, transformers)
	if err != nil {
		return nil, fmt.Errorf("converting rhs of binary expression: %v", err)
	}
	if len(rhs) == 0 {
		return nil, errors.New("rhs hand side of binary expression is unexpectedly empty")
	}

	ret := []string{fmt.Sprintf("%s %s %s", attrName, op, rhs[0])}
	ret = append(ret, rhs[1:]...)

	// Handle any additional work necessary for specific assignments.
	if attrName == "sdk_headers_for_internal_use" {
		ret = handle_sdk_headers_for_internal_use(ret, rhs, 0)
	}

	// Wrap the entire assignment in a condition if appropriate.
	// This should be the last thing before returning the result.
	switch {
	case idkFuchsiaSpecificAttrs[lhs.Name]:
		ret = append([]string{"if (is_fuchsia) {"}, indent(ret, 1)...)
		ret = append(ret, "}")
	case idkNonFuchsiaSpecificAttrs[lhs.Name]:
		ret = append([]string{"if (!is_fuchsia) {"}, indent(ret, 1)...)
		ret = append(ret, "}")
	}

	return ret, nil
}

// binaryExprToGN converts a general Bazel binary expression [0] to GN.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#binary-operators
func binaryExprToGN(expr *syntax.BinaryExpr, transformers []transformer) ([]string, error) {
	lhs, err := exprToGN(expr.X, transformers)
	if err != nil {
		return nil, fmt.Errorf("converting lhs of binary expression: %v", err)
	}
	if len(lhs) == 0 {
		return nil, errors.New("lhs of binary expression is unexpectedly empty")
	}

	rhs, err := exprToGN(expr.Y, transformers)
	if err != nil {
		return nil, fmt.Errorf("converting rhs of binary expression: %v", err)
	}
	if len(rhs) == 0 {
		return nil, errors.New("rhs hand side of binary expression is unexpectedly empty")
	}

	ret := lhs[:len(lhs)-1]
	ret = append(ret, fmt.Sprintf("%s %s %s", lhs[len(lhs)-1], opToGN(expr.Op), rhs[0]))
	ret = append(ret, rhs[1:]...)
	return ret, nil
}

func opToGN(op syntax.Token) string {
	ret, ok := specialTokens[op]
	if !ok {
		return op.String()
	}
	return ret
}

// listExprToGN converts a Bazel list expression [0] to GN.
//
// [0] https://github.com/bazelbuild/starlark/blob/master/spec.md#lists
func listExprToGN(expr *syntax.ListExpr, transformers []transformer) ([]string, error) {
	ret := []string{"["}

	for _, elm := range expr.List {
		shouldSkip, err := hasSkipAnnotation(elm)
		if err != nil {
			return nil, fmt.Errorf("failed to check skip annotation: %v", err)
		}
		if shouldSkip {
			continue
		}

		elmLines, err := exprToGN(elm, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting list element: %v", err)
		}
		if len(elmLines) == 0 {
			continue
		}
		elmLines[len(elmLines)-1] = elmLines[len(elmLines)-1] + ","
		ret = append(ret, indent(elmLines, 1)...)
	}

	ret = append(ret, "]")
	return ret, nil
}

// dictExprToGN converts a Bazel dictionary expression to a GN scope.
func dictExprToGN(expr *syntax.DictExpr, transformers []transformer) ([]string, error) {
	ret := []string{"{"}

	for _, entry := range expr.List {
		entryDictEntry, ok := entry.(*syntax.DictEntry)
		if !ok {
			return nil, fmt.Errorf("unexpected node type in dictionary entry: %T", entry)
		}

		key, err := exprToGN(entryDictEntry.Key, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting dictionary key: %v", err)
		}

		value, err := exprToGN(entryDictEntry.Value, transformers)
		if err != nil {
			return nil, fmt.Errorf("converting dictionary value: %v", err)
		}

		// In GN, keys are identifiers, so they should not be quoted.
		// Starlark dictionary keys are strings, so they are quoted.
		// We need to remove the quotes from the key.
		key[0] = strings.Trim(key[0], `"`)

		lines := []string{fmt.Sprintf("%s = %s", key[0], value[0])}
		if len(value) > 1 {
			lines = append(lines, value[1:]...)
		}
		ret = append(ret, indent(lines, 1)...)
	}

	ret = append(ret, "}")
	return ret, nil
}

// hasSkipAnnotation returns true if the given node has a skip annotation.
//
// A skip annotation is a comment that exactly matches `@bazel2gn:skip`, and
// right above or after the node on the same line. The node will be skipped
// during conversion if the annotation is found.
//
// For example, these are skip annotations:
//
// ```
//
//	# Some other comment
//	# @bazel2gn:skip
//	go_library(
//	  ...
//	  # @bazel2gn:skip
//	  attr = val,  # @bazel2gn:skip
//	)
//
// ```
//
// And these are NOT skip annotations:
//
// ```
//
//	# @bazel2gn:skip
//	# Skip annotation should be on the same line or right above the node.
//	go_library(
//		...
//	)
//	# @bazel2gn:skip
//
// ```
func hasSkipAnnotation(node syntax.Node) (bool, error) {
	comments := node.Comments()
	if comments == nil {
		return false, nil
	}
	if len(comments.Before) > 0 {
		// For comments before, match the last line.
		if comments.Before[len(comments.Before)-1].Text == skipAnnotation {
			return true, nil
		}
	}
	if len(comments.Suffix) > 0 {
		// For suffixes, match the comment immediately following.
		if comments.Suffix[0].Text == skipAnnotation {
			return true, nil
		}
	}
	allComments := [][]syntax.Comment{
		comments.Before,
		comments.After,
	}
	for _, c := range allComments {
		if hasAnnotation(c, skipAnnotation) {
			return false, fmt.Errorf("found skip annotation in unexpected location, it should be on the line right above the target definition to take effect")
		}
	}
	return false, nil
}

func hasAnnotation(comments []syntax.Comment, annotation string) bool {
	for _, comment := range comments {
		if comment.Text == annotation {
			return true
		}
	}
	return false
}

// transformersFromAnnotations returns the list of transformers to apply to a
// target definition based on the annotations found in the comments.
func transformersFromAnnotations(comments []syntax.Comment) ([]transformer, error) {
	transformers := []transformer{}
	for _, comment := range comments {
		if strings.HasPrefix(comment.Text, transformerAnnotationPrefix) {
			n := strings.TrimPrefix(comment.Text, transformerAnnotationPrefix)
			transformer, ok := transformerAnnotationNames[n]
			if !ok {
				return nil, fmt.Errorf("unknown transformer: %s", n)
			}
			transformers = append(transformers, transformer)
		}
	}
	return transformers, nil
}

func transformersFromComments(comments *syntax.Comments) ([]transformer, error) {
	if comments == nil {
		return nil, nil
	}
	before, err := transformersFromAnnotations(comments.Before)
	if err != nil {
		return nil, err
	}
	suffix, err := transformersFromAnnotations(comments.Suffix)
	if err != nil {
		return nil, err
	}
	return append(before, suffix...), nil
}
