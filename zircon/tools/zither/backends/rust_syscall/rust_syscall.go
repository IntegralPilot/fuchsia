// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust_syscall

import (
	"embed"
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither/backends/rust"
)

//go:embed templates/*
var templates embed.FS

type Generator struct {
	fidlgen.Generator
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	gen := fidlgen.NewGenerator("RustSyscallTemplates", templates, formatter, template.FuncMap{
		"LowerCaseWithUnderscores": rust.LowerCaseWithUnderscores,
		"ParameterType":            parameterType,
		"ReturnType":               returnType,
		"LastParameterIndex":       func(syscall zither.Syscall) int { return len(syscall.Parameters) - 1 },
	})
	return &Generator{*gen}
}

func (gen Generator) DeclOrder() zither.DeclOrder { return zither.SourceDeclOrder }

func (gen Generator) DeclCallback(zither.Decl) {}

func (gen *Generator) Generate(summary zither.LibrarySummary, outputDir string) ([]string, error) {
	var syscalls []zither.Syscall
	for _, summary := range summary.Files {
		for _, decl := range summary.Decls {
			if !decl.IsSyscallFamily() {
				continue
			}
			for _, syscall := range decl.AsSyscallFamily().Syscalls {
				if syscall.IsInternal() {
					continue
				}
				syscalls = append(syscalls, syscall)
			}
		}
	}
	sort.Slice(syscalls, func(i, j int) bool {
		return strings.Compare(syscalls[i].Name, syscalls[j].Name) < 0
	})

	outputDir = filepath.Join(outputDir, strings.Join(summary.Library.Parts(), "-"), "src")
	definitions := filepath.Join(outputDir, "definitions.rs")
	if err := gen.GenerateFile(definitions, "GenerateDefinitions", syscalls); err != nil {
		return nil, err
	}
	return []string{definitions}, nil
}

func typeName(desc zither.TypeDescriptor) string {
	return rust.DescribeType(desc, rust.CaseStyleSyscall)
}

//
// Template functions.
//

func parameterType(param zither.SyscallParameter) string {
	// Structs and non-inputs should be passed as pointers.
	kind := param.Type.Kind
	if !kind.IsPointerLike() && (!param.IsStrictInput() || kind == zither.TypeKindStruct) {
		elementType := param.Type
		elementType.Mutable = !param.IsStrictInput()
		return typeName(zither.TypeDescriptor{
			Kind:        zither.TypeKindPointer,
			ElementType: &elementType,
		})
	}
	return typeName(param.Type)
}

func returnType(syscall zither.Syscall) string {
	if syscall.ReturnType == nil {
		panic(fmt.Sprintf("%s does not have a return type", syscall.Name))
	}
	return typeName(*syscall.ReturnType)
}
