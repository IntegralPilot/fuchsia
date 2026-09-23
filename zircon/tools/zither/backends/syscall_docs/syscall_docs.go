// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package syscall_docs

import (
	"embed"
	"path/filepath"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
)

//go:embed templates/*
var templates embed.FS

type Generator struct {
	fidlgen.Generator
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	gen := fidlgen.NewGenerator("SyscallMarkdownTemplates", templates, formatter, template.FuncMap{
		// Rather annoyingly, the IR preserves the one space used between the
		// FIDL comment delimiter and the intended comment.
		"EatOneSpace":              func(s string) string { return strings.TrimPrefix(s, " ") },
		"LowerCaseWithUnderscores": zither.LowerCaseWithUnderscores,
	})
	return &Generator{*gen}
}

// Not relevant to the outputs, so we pick the canonical option with minimal
// preprocessing.
func (gen Generator) DeclOrder() zither.DeclOrder { return zither.SourceDeclOrder }

func (gen Generator) DeclCallback(zither.Decl) {}

func (gen *Generator) Generate(summary zither.LibrarySummary, outputDir string) ([]string, error) {
	var outputs []string
	for _, summary := range summary.Files {
		for _, decl := range summary.Decls {
			if !decl.IsSyscallFamily() {
				continue
			}
			family := decl.AsSyscallFamily()
			for _, syscall := range family.Syscalls {
				// Internal, test-only, and @no_doc-suppressed syscalls should
				// not contribute documentation.
				if syscall.IsInternal() || syscall.Testonly || len(syscall.Comments) == 0 {
					continue
				}
				filename := zither.LowerCaseWithUnderscores(syscall) + ".md"
				output := filepath.Join(outputDir, filename)
				if err := gen.GenerateFile(output, "GenerateSyscallMarkdown", syscall); err != nil {
					return nil, err
				}
				outputs = append(outputs, output)
			}
		}
	}
	return outputs, nil
}
