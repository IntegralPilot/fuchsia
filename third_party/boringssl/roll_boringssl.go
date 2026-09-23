// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This script updates //third_party/boringssl/src to point to the current revision at:
//   https://boringssl.googlesource.com/boringssl/+/main
//
// It also updates the generated build files, Rust bindings, and subset of code used in Zircon.

package main

import (
	"bytes"
	"flag"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
)

// Returns the path to the boringssl directory.
func configure() string {
	log.Println("Configuring...")
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		log.Fatal("failed to find current executable")
	}
	return filepath.Dir(file)
}

// Updates BoringSSL sources and returns the revision.
func updateSources(dir, commit string) []byte {
	log.Println("Updating BoringSSL sources...")
	dir = filepath.Join(dir, "src")
	{
		cmd := exec.Command("git", "-C", dir, "fetch", "--all", "--prune")
		if err := cmd.Run(); err != nil {
			log.Fatalf("%s failed: %s", cmd.Args, err)
		}
	}
	{
		cmd := exec.Command("git", "-C", dir, "checkout", commit)
		if err := cmd.Run(); err != nil {
			log.Fatalf("%s failed: %s", cmd.Args, err)
		}
	}
	{
		cmd := exec.Command("git", "-C", dir, "rev-parse", commit)
		out, err := cmd.Output()
		if err != nil {
			log.Fatalf("%s failed: %s", cmd.Args, err)
		}
		return bytes.TrimSpace(out)
	}
}

// Create the build files for the current sources.
func generateBuildFiles(dir string) {
	log.Printf("Generating build files...")
	cmd := exec.Command("python3", filepath.Join("src", "util", "generate_build_files.py"), "gn", "bazel")
	cmd.Dir = dir
	if err := cmd.Run(); err != nil {
		log.Fatalf("%s failed: %s", cmd.Args, err)
	}
}

// Regenerates the Rust bindings.
func generateRustBindings(dir string) {
	log.Printf("Generating Rust bindings...")
	cmd := exec.Command(filepath.Join("rust", "bssl-sys", "generate.py"))
	cmd.Dir = dir
	if err := cmd.Run(); err != nil {
		log.Fatalf("%s failed: %s", cmd.Args, err)
	}
}

// Updates the Revision and Upstream Revision fields in README.fuchsia.
func updateReadMe(dir string, sha1 []byte) {
	const readmeName = "README.fuchsia"
	log.Printf("Updating %s...", readmeName)
	readmePath := filepath.Join(dir, readmeName)
	content, err := os.ReadFile(readmePath)
	if err != nil {
		log.Fatalf("failed to read %s: %s", readmeName, err)
	}
	reRev := regexp.MustCompile(`(?m)^(Revision:\s*)[0-9a-fA-F]+`)
	if !reRev.Match(content) {
		log.Fatalf("failed to find Revision field in %s", readmeName)
	}
	content = reRev.ReplaceAll(content, []byte("${1}"+string(sha1)))
	reUpstream := regexp.MustCompile(`(?m)^(Upstream Revision:\s*https://\S+?/\+/)[0-9a-fA-F]+`)
	if !reUpstream.Match(content) {
		log.Fatalf("failed to find Upstream Revision field in %s", readmeName)
	}
	content = reUpstream.ReplaceAll(content, []byte("${1}"+string(sha1)))
	if err := os.WriteFile(readmePath, content, 0644); err != nil {
		log.Fatalf("failed to write %s: %s", readmeName, err)
	}
}

// Updates the BoringSSL revision in //manifests/third_party/all.
func updateManifest(dir string, sha1 []byte) {
	const manifestName = "//manifests/third_party/all"
	log.Printf("Updating %s...", manifestName)
	manifestPath := filepath.Join(dir, "..", "..", "manifests", "third_party", "all")
	content, err := os.ReadFile(manifestPath)
	if err != nil {
		log.Fatalf("failed to read %s: %s", manifestName, err)
	}
	re := regexp.MustCompile(`(<project\s+name="boringssl"[\s\S]*?revision=")[0-9a-fA-F]+(")`)
	if !re.Match(content) {
		log.Fatalf("failed to find boringssl project tag in %s", manifestName)
	}
	newContent := re.ReplaceAll(content, []byte("${1}"+string(sha1)+"${2}"))
	if err := os.WriteFile(manifestPath, newContent, 0644); err != nil {
		log.Fatalf("failed to write %s: %s", manifestName, err)
	}
}

func main() {
	commit := flag.String("commit", "origin/main", "Upstream commit-ish to check out")

	flag.Parse()

	log.SetFlags(log.Lmicroseconds)

	dir := configure()
	sha1 := updateSources(dir, *commit)

	log.Printf("Commit resolved to %s", sha1)

	generateBuildFiles(dir)
	generateRustBindings(dir)
	updateReadMe(dir, sha1)
	updateManifest(dir, sha1)

	log.Println()
	log.Println("To test, run:")
	log.Println("  $ fx set ... --with //third_party/boringssl:tests")
	log.Println("  $ fx build")
	log.Println("  $ fx serve")
	log.Println("  $ fx test boringssl_tests")
	log.Println()
	log.Println("To verify Zircon linkage (see zircon-unused.c):")
	log.Println("  $ fx --dir out/bringup.x64.no_opt set bringup.x64 --args=optimize='\"none\"'")
	log.Println("  $ fx build")
	log.Println("  $ fx use out/default")

	log.Println("If tests pass; commit the changes in //third_party/boringssl (you may need to bypass the CQ)")
	log.Println("Then, update the BoringSSL revision in the internal integration repository")
}
