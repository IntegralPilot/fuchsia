// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cli

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/config"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/parser"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

// A Generator generates code given GIDL IR, FIDL IR, and config.
type Generator func(ir.All, fidlgen.Root, config.GeneratorConfig) ([]byte, error)

type listOfStrings []string

func (l *listOfStrings) String() string {
	return strings.Join(*l, " ")
}

func (l *listOfStrings) Set(s string) error {
	*l = append(*l, s)
	return nil
}

func parseGidlIr(filename string) ir.All {
	f, err := os.Open(filename)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	config := parser.Config{
		Languages:   ir.AllLanguages(),
		WireFormats: ir.AllWireFormats(),
	}
	result, err := parser.NewParser(filename, f, config).Parse()
	if err != nil {
		panic(err)
	}
	return result
}

func parseFidlJSONIr(filename string) fidlgen.Root {
	bytes, err := os.ReadFile(filename)
	if err != nil {
		panic(err)
	}
	var result fidlgen.Root
	if err := json.Unmarshal(bytes, &result); err != nil {
		panic(err)
	}
	return result
}

func Main(generatorMap map[ir.OutputType]map[ir.Language]Generator) {
	// Omit timestamps from logs.
	log.SetFlags(0)

	var allOutputTypes, allLanguages []string
	allLanguagesMap := map[ir.Language]struct{}{}
	for outputType, subMap := range generatorMap {
		allOutputTypes = append(allOutputTypes, string(outputType))
		for language := range subMap {
			if _, ok := allLanguagesMap[language]; !ok {
				allLanguagesMap[language] = struct{}{}
				allLanguages = append(allLanguages, string(language))
			}
		}
	}

	jsonPath := flag.String("json", "",
		"relative path to the FIDL intermediate representation.")
	outputTypeStr := flag.String("type", "",
		fmt.Sprintf("output type (one of: %s)", strings.Join(allOutputTypes, ", ")))
	languageStr := flag.String("language", "",
		fmt.Sprintf("target language (one of: %s)", strings.Join(allLanguages, ", ")))
	outputPath := flag.String("out", "", "path to write output to")
	rustBenchmarksFidlLibrary := flag.String("rust-benchmarks-fidl-library", "",
		"name for the fidl library used in the rust benchmarks")
	cppBenchmarksFidlLibrary := flag.String("cpp-benchmarks-fidl-library", "",
		"name for the fidl library used in the cpp benchmarks")
	fuzzerCorpusHostDir := flag.String("fuzzer-corpus-host-dir", "",
		"output directory for fuzzer_corpus")
	fuzzerCorpusPackageDataDir := flag.String("fuzzer-corpus-package-data-dir", "",
		"directory to which fuzzer_corpus output files are mapped in their fuchsia package's data directory")
	var filterTypes listOfStrings
	flag.Var(&filterTypes, "filter-type", "List of types to filter to (used with -type measure_tape)")
	flag.Parse()

	if flag.NArg() == 0 {
		log.Fatal("must provide at least one .gidl file")
	}
	if *jsonPath == "" {
		log.Fatal("must provide -json")
	}
	if *outputPath == "" {
		log.Fatal("must provide -out")
	}
	outputType := ir.OutputType(*outputTypeStr)
	subGeneratorMap, ok := generatorMap[outputType]
	if !ok {
		log.Fatalf("%s: invalid output type", *outputTypeStr)
	}
	language := ir.Language(*languageStr)
	if _, ok := allLanguagesMap[language]; !ok {
		log.Fatalf("%s: invalid language", *languageStr)
	}
	generator, ok := subGeneratorMap[language]
	if !ok {
		log.Fatalf("-type %s -language %s: unsupported combination", *outputTypeStr, *languageStr)
	}

	root := parseFidlJSONIr(*jsonPath)
	var parsedGidlFiles []ir.All
	for _, path := range flag.Args() {
		parsedGidlFiles = append(parsedGidlFiles, parseGidlIr(path))
	}
	gidl := ir.FilterByLanguage(ir.Merge(parsedGidlFiles), language)
	ir.ValidateByOutputType(gidl, outputType)

	// For simplicity, we do not allow FIDL that GIDL depends on to have
	// dependent libraries, with the exception of zx. This makes it much simpler
	// to have everything in the IR, and avoid cross-references.
	if len(root.Libraries) == 1 && root.Libraries[0].Name == "zx" {
		root.Libraries = make([]fidlgen.Library, 0)
	}
	if len(root.Libraries) != 0 {
		var libs []string
		for _, l := range root.Libraries {
			libs = append(libs, string(l.Name))
		}
		log.Fatalf("GIDL does not work with FIDL libraries with dependents, found: %s",
			strings.Join(libs, ","))
	}

	config := config.GeneratorConfig{
		RustBenchmarksFidlLibrary:  *rustBenchmarksFidlLibrary,
		CppBenchmarksFidlLibrary:   *cppBenchmarksFidlLibrary,
		FuzzerCorpusHostDir:        *fuzzerCorpusHostDir,
		FuzzerCorpusPackageDataDir: *fuzzerCorpusPackageDataDir,
		FilterTypes:                filterTypes,
	}
	mainFile, err := generator(gidl, root, config)
	if err != nil {
		log.Fatal(err)
	}

	if language == ir.LanguageFuzzerCorpus {
		// The fuzzer corpus manifest must always be written so that the build
		// system tries to rebuild the package. The individual files within the
		// corpus aren't tracked by the build system.
		err = os.WriteFile(*outputPath, mainFile, 0666)
	} else {
		err = fidlgen.WriteFileIfChanged(*outputPath, mainFile)
	}
	if err != nil {
		log.Fatal(err)
	}
}
