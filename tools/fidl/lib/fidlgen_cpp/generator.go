// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"bytes"
	"io/fs"
	"log"
	"path"
	"regexp"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

var clangFormatArgs = []string{"--assume-filename=src.cpp", "--style={BasedOnStyle: google, ColumnLimit: 0}"}

type Generator struct {
	gen       *fidlgen.Generator
	flags     *CmdlineFlags
	formatter fidlgen.Formatter
}

func NewGenerator(flags *CmdlineFlags, templates fs.FS, extraFuncs template.FuncMap) *Generator {
	gen := &Generator{flags: flags}

	funcs := mergeFuncMaps(commonTemplateFuncs, extraFuncs)
	funcs = mergeFuncMaps(funcs, template.FuncMap{
		"Filename": func(name string, library fidlgen.LibraryIdentifier) string {
			return gen.generateFilename(name, library)
		},
		"ExperimentEnabled": func(experiment string) bool {
			return gen.ExperimentEnabled(experiment)
		},
	})

	formatter := NewFormatter(flags.clangFormatPath)
	gen.gen = fidlgen.NewGenerator(flags.name, templates, formatter, funcs)
	return gen
}

func NewFormatter(clangFormatPath string) fidlgen.Formatter {
	// TODO(https://fxbug.dev/42058951): Investigate clang-format memory usage on large files.
	clang_format := fidlgen.NewFormatterWithSizeLimit(128*1024, clangFormatPath, clangFormatArgs...)
	return removeEmptyBlocks{clang_format}
}

func (gen *Generator) ExperimentEnabled(experiment string) bool {
	return gen.flags.ExperimentEnabled(experiment)
}

type filenameData struct {
	Library fidlgen.LibraryIdentifier
}

func (fd *filenameData) joinLibraryParts(separator string) string {
	ss := make([]string, len(fd.Library))
	for i, s := range fd.Library {
		ss[i] = string(s)
	}
	return strings.Join(ss, separator)
}

func (fd *filenameData) LibraryDots() string {
	return fd.joinLibraryParts(".")
}

func (fd *filenameData) LibrarySlashes() string {
	return fd.joinLibraryParts("/")
}

func (gen *Generator) generateFilename(file string, library fidlgen.LibraryIdentifier) string {
	fn, err := gen.gen.ExecuteTemplate("Filename:"+file, &filenameData{library})
	if err != nil {
		log.Fatalf("Error generating filename for %s: %v", file, err)
	}
	return string(fn)
}

func (gen *Generator) GenerateFiles(tree *Root, files []string) {
	for _, f := range files {
		fn := path.Join(gen.flags.root, gen.generateFilename(f, tree.Library))
		err := gen.gen.GenerateFile(fn, "File:"+f, tree)
		if err != nil {
			log.Fatalf("Error generating %s: %v", fn, err)
		}
	}
}

type removeEmptyBlocks struct {
	formatter fidlgen.Formatter
}

func (f removeEmptyBlocks) Format(source []byte) ([]byte, error) {
	for {
		l := len(source)
		source = removeBlankLines(source)
		source = collapseIfdefs(source)
		source = removeEmptyNamespaces(source)
		if len(source) == l {
			break
		}
	}
	return f.formatter.Format(source)
}

var removeBlankLinesRE = regexp.MustCompile(`(?m)^\s*$[\r\n]*`)

func removeBlankLines(source []byte) []byte {
	return removeBlankLinesRE.ReplaceAll(source, []byte("\n"))
}

var collapseIfdefsRE1 = regexp.MustCompile(`(?m)#endif  // __Fuchsia__\s+#ifdef __Fuchsia__`)
var collapseIfdefsRE2 = regexp.MustCompile(`(?m)#ifdef __Fuchsia__\s+#endif  // __Fuchsia__`)
var collapseIfdefsRE3 = regexp.MustCompile(`(?m)#endif  // __FIDL_SUPPORT_HANDLES\s+#if __FIDL_SUPPORT_HANDLES`)
var collapseIfdefsRE4 = regexp.MustCompile(`(?m)#if __FIDL_SUPPORT_HANDLES\s+#endif  // __FIDL_SUPPORT_HANDLES`)

func collapseIfdefs(source []byte) []byte {
	return collapseIfdefsRE4.ReplaceAll(
		collapseIfdefsRE3.ReplaceAll(
			collapseIfdefsRE2.ReplaceAll(
				collapseIfdefsRE1.ReplaceAll(source, nil), nil), nil), nil)
}

var removeEmptyNamespacesOuterRE = regexp.MustCompile(`(?m)namespace [a-z0-9_]+ \{\s*\}  // namespace [a-z0-9_]+`)

func removeEmptyNamespaces(source []byte) []byte {
	return removeEmptyNamespacesOuterRE.ReplaceAllFunc(source, func(match []byte) []byte {
		re := regexp.MustCompile(`(?m)namespace ([a-z0-9_]+) \{\s*\}  // namespace ([a-z0-9_]+)`)
		m := re.FindSubmatch(match)
		if len(m) != 3 {
			panic("namespace regex returned the wrong number of groups")
		}
		if bytes.Equal(m[1], m[2]) {
			return nil
		}
		return match
	})
}
