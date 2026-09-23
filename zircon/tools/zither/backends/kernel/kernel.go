// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package kernel

import (
	"embed"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
)

//go:embed templates/*
var templates embed.FS

// Kernel sources, given by its include path. Each file has a corresponding
// template named "Generate-${file basename}".
var includePaths = []string{
	filepath.Join("lib", "syscalls", "category.inc"),
	filepath.Join("lib", "syscalls", "kernel.inc"),
	filepath.Join("lib", "syscalls", "kernel-wrappers.inc"),
	filepath.Join("lib", "syscalls", "syscall_sigs.rs"),
	filepath.Join("lib", "syscalls", "syscalls.inc"),
	filepath.Join("lib", "syscalls", "zx-syscall-numbers.h"),
}

type Generator struct {
	fidlgen.Generator
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	gen := fidlgen.NewGenerator("ZirconKernelTemplates", templates, formatter, template.FuncMap{
		"LowerCaseWithUnderscores": zither.LowerCaseWithUnderscores,
		"Increment":                func(n int) int { return n + 1 },
		"IsUserOutHandle":          isUserOutHandle,
		"KernelIncDecl": func(syscall zither.Syscall) string {
			return SyscallCDecl(syscall, PointerViewKernel, cDeclMacro)
		},
		"LastParameterIndex": func(syscall zither.Syscall) int {
			return len(syscall.Parameters) - 1
		},
		"ParameterType": func(param zither.SyscallParameter) string {
			return cDeclParameterType(param, PointerViewUserspace, false)
		},
		"PassedAsPointer":         passedAsPointer,
		"RustKernelParameterType": rustKernelParameterType,
		"RustKernelReturnType":    rustKernelReturnType,
		"SyscallIncDecl": func(syscall zither.Syscall) string {
			return SyscallCDecl(syscall, PointerViewUserspace, cDeclMacro)
		},
		"UserOutHandles": userOutHandles,
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
				syscalls = append(syscalls, syscall)
			}
		}
	}
	sort.Slice(syscalls, func(i, j int) bool {
		return strings.Compare(syscalls[i].Name, syscalls[j].Name) < 0
	})

	var outputs []string
	for _, file := range includePaths {
		output := filepath.Join(outputDir, file)
		templateName := "Generate-" + filepath.Base(file)
		if err := gen.GenerateFile(output, templateName, syscalls); err != nil {
			return nil, err
		}
		outputs = append(outputs, output)
	}
	return outputs, nil
}

//
// Template functions.
//

// Whether the parameter represents a singular, handle out parameter (that is,
// not part of an array of handle out parameters).
func isUserOutHandle(param zither.SyscallParameter) bool {
	return param.IsStrictOutput() && param.Type.Kind == zither.TypeKindHandle && !param.HasTag(zither.ParameterTagDecayedFromVector)
}

// Returns the list of user out handles.
func userOutHandles(syscall zither.Syscall) []zither.SyscallParameter {
	var outHandles []zither.SyscallParameter
	for _, param := range syscall.Parameters {
		if isUserOutHandle(param) {
			outHandles = append(outHandles, param)
		}
	}
	return outHandles
}
