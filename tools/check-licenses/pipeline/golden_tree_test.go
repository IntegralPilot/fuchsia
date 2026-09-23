// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package pipeline_test

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

type captureRenderer struct {
	Projects []*pipeline.Project
	Errors   []pipeline.ComplianceError
}

func (r *captureRenderer) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	r.Projects = projects
	r.Errors = errors
	return nil
}

// TestGoldenTree_EndToEnd simulates a realistic Fuchsia repository containing
// diverse repository patterns: 1st-party files, nested third_party dirs, submodules,
// vendored Rust crates, monorepo mirrors, prebuilt toolchains, and custom license exceptions.
func TestGoldenTree_EndToEnd(t *testing.T) {
	fuchsiaDir := t.TempDir()

	// -------------------------------------------------------------
	// 1. Setup Config Assets & Patterns
	// -------------------------------------------------------------
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/config.json"), "{\n\t\"includes\": [\"tools/check-licenses/assets\"]\n}")

	// Root virtual README for Fuchsia first-party code
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/readmes/README.fuchsia"), "Name: Fuchsia\nLicense: BSD-2-Clause\nLocation: .\n")

	// Custom patterns for Classifier engine in test
	mitPattern := "Permission is hereby granted, free of charge, to any person obtaining a copy"
	apachePattern := "Licensed under the Apache License, Version 2.0"
	bsdPattern := "Redistribution and use in source and binary forms, with or without modification"
	fuchsiaPattern := "Copyright 2026 The Fuchsia Authors. All rights reserved.\nUse of this source code is governed by a BSD-style license that can be\nfound in the LICENSE file."

	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/patterns/Approved/MIT/mit.txt"), mitPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/patterns/Approved/Apache-2.0/apache.txt"), apachePattern)
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/patterns/Approved/BSD-3-Clause/bsd.txt"), bsdPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/patterns/_Header/FuchsiaCopyright/fuchsia.txt"), fuchsiaPattern)

	// Copyright extensions config
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/copyright_extensions/default.json"), map[string]interface{}{
		"copyright_extensions": map[string]interface{}{
			"extensions": []string{".cc", ".py"},
		},
	})

	// Barriers config
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/barriers/default.json"), map[string]interface{}{
		"barriers": []interface{}{
			map[string]interface{}{
				"paths": []string{"third_party", "prebuilt", "prebuilt/third_party", "vendor"},
			},
		},
	})

	// Skips config (skipping assets directory & redundant inner submodule README)
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/skips/default.json"), map[string]interface{}{
		"skips": []interface{}{
			map[string]interface{}{
				"paths": []string{
					"tools/check-licenses/assets",
					"third_party/submodule_lib/src/README.fuchsia",
				},
			},
		},
	})

	// Allowed licenses (MIT, Apache-2.0, BSD-3-Clause are approved globally)
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/allowed_licenses/Approved/MIT/default.json"), map[string]interface{}{
		"allowed_licenses": map[string]interface{}{
			"MIT": []interface{}{
				map[string]interface{}{"bug": "b/test", "paths": []string{"."}},
			},
		},
	})
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/allowed_licenses/Approved/Apache-2.0/default.json"), map[string]interface{}{
		"allowed_licenses": map[string]interface{}{
			"Apache-2.0": []interface{}{
				map[string]interface{}{"bug": "b/test", "paths": []string{"."}},
			},
		},
	})
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/allowed_licenses/Approved/BSD-3-Clause/default.json"), map[string]interface{}{
		"allowed_licenses": map[string]interface{}{
			"BSD-3-Clause": []interface{}{
				map[string]interface{}{"bug": "b/test", "paths": []string{"."}},
			},
		},
	})

	// Policy Exceptions
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/policy_exceptions/AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders/legacy.json"), map[string]interface{}{
		"policy_exceptions": map[string]interface{}{
			"AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders": []interface{}{
				map[string]interface{}{"bug": "b/test", "paths": []string{"src/legacy.cc"}},
			},
		},
	})
	writeJSON(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/configs/policy_exceptions/AllLicenseTextsMustBeRecognized/custom.json"), map[string]interface{}{
		"policy_exceptions": map[string]interface{}{
			"AllLicenseTextsMustBeRecognized": []interface{}{
				map[string]interface{}{"bug": "b/test", "paths": []string{"third_party/custom_lib/LICENSE"}},
			},
		},
	})

	// Virtual READMEs
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/readmes/prebuilt/third_party/cmake/linux-x64/README.fuchsia"), "Name: cmake\nLicense File: doc/LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "tools/check-licenses/assets/readmes/third_party/rust_crates/mirrors/google-cloud-rust/README.fuchsia"), "Name: google-cloud-rust\nLicense File: LICENSE\n")

	// -------------------------------------------------------------
	// 2. Setup Repository Tree
	// -------------------------------------------------------------
	fuchsiaHdr := "// Copyright 2026 The Fuchsia Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style license that can be\n// found in the LICENSE file.\n"

	// [Case 1: First Party Code]
	writeFile(t, filepath.Join(fuchsiaDir, "src/main.cc"), fuchsiaHdr+"\nint main() {}\n")
	writeFile(t, filepath.Join(fuchsiaDir, "src/legacy.cc"), "int legacy() {}\n") // Allowlisted missing copyright
	writeFile(t, filepath.Join(fuchsiaDir, "src/bad.cc"), "int bad() {}\n")       // NOT allowlisted missing copyright -> should error!

	// [Case 2: Nested third_party directories]
	// third_party/parent_lib/third_party/nested_child should NOT be treated as a barrier dir.
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/parent_lib/README.fuchsia"), "Name: parent_lib\nLicense File: LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/parent_lib/LICENSE"), mitPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/parent_lib/src/util.cc"), "void util() {}\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/parent_lib/third_party/nested_child/sub.cc"), "void sub() {}\n")

	// [Case 3: Standard 3P Project]
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/normal_lib/README.fuchsia"), "Name: normal_lib\nLicense File: LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/normal_lib/LICENSE"), apachePattern)
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/normal_lib/main.cc"), "void normal() {}\n")

	// [Case 4: Submodule 3P Project with redundant inner README.fuchsia]
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/submodule_lib/README.fuchsia"), "Name: submodule_lib\nLicense File: src/LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/submodule_lib/src/README.fuchsia"), "Name: submodule_inner\nLicense File: LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/submodule_lib/src/LICENSE"), mitPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/submodule_lib/src/main.cc"), "void submain() {}\n")

	// [Case 5: Vendored Rust Crate]
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/vendor/adler2/Cargo.toml"), "[package]\nname = \"adler2\"\nversion = \"2.0.0\"\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/vendor/adler2/LICENSE"), mitPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/vendor/adler2/src/lib.rs"), "pub fn adler() {}\n")

	// [Case 6: Rust Mirror Monorepo with sub-crates]
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors/google-cloud-rust/Cargo.toml"), "")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors/google-cloud-rust/LICENSE"), apachePattern)
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors/google-cloud-rust/src/pubsub/Cargo.toml"), "[package]\nname = \"pubsub\"\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors/google-cloud-rust/src/pubsub/src/lib.rs"), "pub fn pubsub() {}\n")

	// [Case 7: Prebuilt tool with Virtual README]
	writeFile(t, filepath.Join(fuchsiaDir, "prebuilt/third_party/cmake/linux-x64/doc/LICENSE"), bsdPattern)
	writeFile(t, filepath.Join(fuchsiaDir, "prebuilt/third_party/cmake/linux-x64/share/CMakeCCompilerABI.c"), "int cmake_abi() {}\n")

	// [Case 8: Prebuilt tool with NO README (Barrier project lacking README)]
	writeFile(t, filepath.Join(fuchsiaDir, "prebuilt/third_party/orphan_tool/linux-x64/bin/tool"), "binary content\n")
	writeFile(t, filepath.Join(fuchsiaDir, "prebuilt/third_party/orphan_tool/linux-x64/LICENSE"), mitPattern)

	// [Case 9: Unrecognized License Text with Policy Exception]
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/custom_lib/README.fuchsia"), "Name: custom_lib\nLicense File: LICENSE\n")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/custom_lib/LICENSE"), "Custom proprietary license agreement string that does not match standard SPDX.")
	writeFile(t, filepath.Join(fuchsiaDir, "third_party/custom_lib/lib.cc"), "void custom() {}\n")

	// -------------------------------------------------------------
	// 3. Assemble Config & Execute Pipeline
	// -------------------------------------------------------------
	builder := config.NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		t.Fatalf("Failed to assemble config: %v", err)
	}

	crawler := discover.NewCrawler(fuchsiaDir, builder.Config.Discover)
	grouper := boundary.NewGrouper(fuchsiaDir, builder.Config.Boundary)
	pruner := prune.NewPruner(nil)
	classifier, err := classify.NewClassifier(builder.Config.Classify)
	if err != nil {
		t.Fatalf("Failed to create classifier: %v", err)
	}
	validator := validate.NewValidator(fuchsiaDir, builder.Config.Validate)
	renderer := &captureRenderer{}

	orchestrator := pipeline.NewOrchestrator(
		crawler,
		grouper,
		pruner,
		classifier,
		validator,
		renderer,
	)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := orchestrator.Run(ctx, []string{fuchsiaDir}); err != nil {
		t.Fatalf("Pipeline execution failed: %v", err)
	}

	// -------------------------------------------------------------
	// 4. Assert Project Boundary Resolution
	// -------------------------------------------------------------
	projectsByRel := make(map[string]*pipeline.Project)
	for _, p := range renderer.Projects {
		rel, _ := filepath.Rel(fuchsiaDir, p.RootPath)
		projectsByRel[filepath.ToSlash(rel)] = p
	}

	// [Assertion 1: Nested third_party is NOT a barrier]
	// All files under parent_lib/third_party/nested_child belong to third_party/parent_lib
	if parentProj, ok := projectsByRel["third_party/parent_lib"]; !ok {
		t.Errorf("Expected project third_party/parent_lib to exist")
	} else {
		foundNested := false
		for _, f := range parentProj.Files {
			if strings.Contains(f.Path, "nested_child/sub.cc") {
				foundNested = true
				break
			}
		}
		if !foundNested {
			t.Errorf("Expected nested third_party file sub.cc to belong to third_party/parent_lib")
		}
	}
	if _, exists := projectsByRel["third_party/parent_lib/third_party/nested_child"]; exists {
		t.Errorf("Nested third_party directory should NOT be registered as an independent barrier project")
	}

	// [Assertion 2: Redundant inner README was skipped, outer owns the submodule]
	if _, exists := projectsByRel["third_party/submodule_lib/src"]; exists {
		t.Errorf("Skipped inner README should NOT create a subproject third_party/submodule_lib/src")
	}
	if subProj, ok := projectsByRel["third_party/submodule_lib"]; !ok {
		t.Errorf("Expected project third_party/submodule_lib to exist")
	} else {
		hasLicense := false
		for _, f := range subProj.Files {
			if strings.HasSuffix(f.Path, "src/LICENSE") && f.IsLicenseFile {
				hasLicense = true
			}
		}
		if !hasLicense {
			t.Errorf("Expected third_party/submodule_lib to own src/LICENSE as primary license file")
		}
	}

	// [Assertion 3: Rust mirrors monorepo does NOT splinter into sub-crates]
	if _, exists := projectsByRel["third_party/rust_crates/mirrors/google-cloud-rust/src/pubsub"]; exists {
		t.Errorf("Internal crate in mirror repo should NOT splinter into a separate project")
	}
	if mirrorProj, ok := projectsByRel["third_party/rust_crates/mirrors/google-cloud-rust"]; !ok {
		t.Errorf("Expected project third_party/rust_crates/mirrors/google-cloud-rust to exist")
	} else if mirrorProj.Readme == nil {
		t.Errorf("Expected mirror project to have virtual Readme attached")
	}

	// [Assertion 4: Vendored Rust crate recognized from Cargo.toml]
	if crateProj, ok := projectsByRel["third_party/rust_crates/vendor/adler2"]; !ok {
		t.Errorf("Expected vendored Rust crate third_party/rust_crates/vendor/adler2 to be a project boundary")
	} else if crateProj.Readme == nil {
		t.Errorf("Expected vendored Rust crate to have Readme metadata parsed from Cargo.toml")
	}

	// [Assertion 5: Prebuilt cmake toolchain grouped by virtual README]
	if cmakeProj, ok := projectsByRel["prebuilt/third_party/cmake/linux-x64"]; !ok {
		t.Errorf("Expected prebuilt cmake to be grouped under prebuilt/third_party/cmake/linux-x64")
	} else if cmakeProj.Readme == nil {
		t.Errorf("Expected prebuilt cmake to have virtual Readme attached")
	}

	// -------------------------------------------------------------
	// 5. Assert Compliance Errors
	// -------------------------------------------------------------
	// Exactly 2 compliance errors are expected from this golden tree:
	// 1. [AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders] on src/bad.cc
	// 2. [AllProjectsMustHaveAReadme] on prebuilt/third_party/orphan_tool
	if len(renderer.Errors) != 2 {
		var errorDescs []string
		for _, e := range renderer.Errors {
			errorDescs = append(errorDescs, fmt.Sprintf("[%s] %s (%s)", e.CheckName, e.Project, e.FilePath))
		}
		t.Fatalf("Expected exactly 2 compliance errors, got %d:\n%s", len(renderer.Errors), strings.Join(errorDescs, "\n"))
	}

	sort.Slice(renderer.Errors, func(i, j int) bool {
		return renderer.Errors[i].CheckName < renderer.Errors[j].CheckName
	})

	// Error 1: Missing Fuchsia copyright header
	err1 := renderer.Errors[0]
	if err1.CheckName != validate.PolicyFuchsiaCopyright {
		t.Errorf("Expected check %s, got: %s", validate.PolicyFuchsiaCopyright, err1.CheckName)
	}
	if !strings.HasSuffix(err1.FilePath, "src/bad.cc") {
		t.Errorf("Expected copyright error on src/bad.cc, got: %s", err1.FilePath)
	}

	// Error 2: Missing README on orphan tool
	err2 := renderer.Errors[1]
	if err2.CheckName != validate.PolicyNoReadme {
		t.Errorf("Expected check %s, got: %s", validate.PolicyNoReadme, err2.CheckName)
	}
	relOrphan, _ := filepath.Rel(fuchsiaDir, err2.Project)
	if filepath.ToSlash(relOrphan) != "prebuilt/third_party/orphan_tool" {
		t.Errorf("Expected missing README error on prebuilt/third_party/orphan_tool, got: %s", relOrphan)
	}
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}
}

func writeJSON(t *testing.T, path string, data interface{}) {
	t.Helper()
	bytes, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	writeFile(t, path, string(bytes))
}
