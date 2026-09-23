// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	"path/filepath"
	"sort"
	"strings"

	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
	"golang.org/x/exp/maps"
	"gopkg.in/yaml.v2"
)

func checkDuplicateName(name string, entries map[string]TOCEntry) bool {
	_, ok := entries[name]
	return ok
}

func filterEntries(entries []TOCEntry, docType SupportedDocType) []TOCEntry {
	var filteredEntries []TOCEntry
	for _, entry := range entries {
		if entry.DocType == docType {
			filteredEntries = append(filteredEntries, entry)
		}
	}

	sort.Slice(filteredEntries, func(i, j int) bool {
		return filteredEntries[i].Title < filteredEntries[j].Title
	})
	return filteredEntries
}

// There are several places in our bazel rules where we wrap a rule with a
// macro. However, these end up as starlark functions in our docs which is not
// what we want. This rule figures out if a starlark function is trying to be a
// macro or an actual function.
func docTypeForStarlarkFunction(f *pb.StarlarkFunctionInfo) SupportedDocType {
	params := f.GetParameter()

	if len(params) > 0 && params[0].GetName() == "name" {
		return DocTypeRule
	}

	return DocTypeStarlarkFunction
}

type docStringer interface {
	GetDocString() string
}

func shortDescription(v docStringer) string {
	return strings.Split(v.GetDocString(), "\n")[0]
}

func RenderModuleInfo(roots []pb.ModuleInfo, renderer Renderer, fileProvider FileProvider, basePath string) {
	fileProvider.Open()

	mkPathFn := func(s string) string {
		return filepath.Join("/", basePath, s)
	}

	entries := make(map[string]TOCEntry)

	for _, moduleInfo := range roots {

		// Render all of our rules
		for _, rule := range moduleInfo.GetRuleInfo() {
			fileName := rule.RuleName + ".md"
			if checkDuplicateName(fileName, entries) {
				continue
			}
			if err := renderer.RenderRuleInfo(rule, fileProvider.NewFile(fileName)); err != nil {
				panic(err)
			}
			entries[fileName] = NewTOCEntry(rule.RuleName, fileName, shortDescription(rule), DocTypeRule, mkPathFn)
		}

		// Render all of our providers
		for _, provider := range moduleInfo.GetProviderInfo() {
			fileName := provider.ProviderName + ".md"
			checkDuplicateName(fileName, entries)
			if err := renderer.RenderProviderInfo(provider, fileProvider.NewFile(fileName)); err != nil {
				panic(err)
			}
			entries[fileName] = NewTOCEntry(provider.ProviderName, fileName, shortDescription(provider), DocTypeProvider, mkPathFn)
		}

		// Render all of our starlark functions
		for _, funcInfo := range moduleInfo.GetFuncInfo() {
			fileName := funcInfo.FunctionName + ".md"
			checkDuplicateName(fileName, entries)
			if err := renderer.RenderStarlarkFunctionInfo(funcInfo, fileProvider.NewFile(fileName)); err != nil {
				panic(err)
			}
			entries[fileName] = NewTOCEntry(funcInfo.FunctionName, fileName, shortDescription(funcInfo), docTypeForStarlarkFunction(funcInfo), mkPathFn)
		}

		// Render all of our rules
		for _, repoRule := range moduleInfo.GetRepositoryRuleInfo() {
			fileName := repoRule.RuleName + ".md"
			checkDuplicateName(fileName, entries)
			if err := renderer.RenderRepositoryRuleInfo(repoRule, fileProvider.NewFile(fileName)); err != nil {
				panic(err)
			}
			entries[fileName] = NewTOCEntry(repoRule.RuleName, fileName, shortDescription(repoRule), DocTypeRepositoryRule, mkPathFn)
		}
	}

	// Render our README.md
	tocEntries := maps.Values(entries)
	renderer.RenderReadme(&tocEntries, fileProvider.NewFile("README.md"))

	toc := []TOCEntry{
		{
			Title: "Overview",
			Path:  mkPathFn("README.md"),
		},
		{
			Title: "API",
			Sections: []TOCEntry{
				{
					Title:    "Rules",
					Sections: filterEntries(tocEntries, DocTypeRule),
				},
				{
					Title:    "Providers",
					Sections: filterEntries(tocEntries, DocTypeProvider),
				},
				{
					Title:    "Starlark Functions",
					Sections: filterEntries(tocEntries, DocTypeStarlarkFunction),
				},
				{
					Title:    "Repository Rules",
					Sections: filterEntries(tocEntries, DocTypeRepositoryRule),
				},
			},
		},
	}

	// Make our table of contents
	if yamlData, err := yaml.Marshal(&map[string]any{"toc": toc}); err == nil {
		tocWriter := fileProvider.NewFile("_toc.yaml")
		tocWriter.Write(yamlData)
	}

	fileProvider.Close()
}
