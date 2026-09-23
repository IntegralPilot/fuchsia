// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package main implements a parser for clippy JSON files generated
// by the build system that determines which clippy findings to report based on
// a list of affected files. It integrates with shac (see //scripts/shac).
//
// Clippy runs within the build system because it needs access to each Rust
// library's full dependency tree, and it outputs files within the build
// directory containing Clippy findings, so this checker reads findings from
// those files (and assumes they've already been built) rather than running
// Clippy itself.
//
// TODO(olivernewman): Consider deleting this tool and having shac into the same
// Python code used by `fx clippy` instead.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"golang.org/x/exp/maps"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/staticanalysis"
)

type clippyReporter struct {
	buildDir      string
	checkoutDir   string
	clippyTargets []build.ClippyTarget
}

func (c *clippyReporter) report(ctx context.Context, path string) ([]*staticanalysis.Finding, error) {
	buildRelPath, err := filepath.Rel(c.buildDir, filepath.Join(c.checkoutDir, path))
	if err != nil {
		return nil, err
	}
	clippyTarget, hasClippy := c.clippyTargetForFile(buildRelPath)
	if !hasClippy {
		return nil, nil
	}

	// Make sure the Clippy output file was built.
	outputPath := filepath.Join(c.buildDir, clippyTarget.Output)
	if _, err := os.Stat(outputPath); errors.Is(err, os.ErrNotExist) {
		// TODO(olivernewman): consider making these failures blocking once
		// we're confident that the configuration is correct and the files
		// always exist when they should.
		logger.Warningf(ctx, "clippy output file %s for source file %s does not exist", clippyTarget.Output, path)
		return nil, nil
	}

	contents, err := os.ReadFile(outputPath)
	if err != nil {
		return nil, err
	}
	contents = bytes.TrimSpace(contents)
	// Consider an empty file to have no Clippy findings.
	if len(contents) == 0 {
		return nil, nil
	}

	var findings []*staticanalysis.Finding
	// The clippy file is in JSON lines format.
	for _, line := range bytes.Split(contents, []byte("\n")) {
		var result clippyResult
		if err := json.Unmarshal(line, &result); err != nil {
			return nil, fmt.Errorf("failed to unmarshal clippy output line (%q): %w", line, err)
		}

		primarySpan, ok := result.primarySpan()
		if !ok {
			// Skip any result that doesn't have any primary span. This will be
			// the case for e.g. results that have no spans because they just
			// summarize other results from the same library.
			continue
		}

		spanPath, err := c.buildPathToCheckoutPath(primarySpan.FileName)
		if err != nil {
			return nil, err
		}
		// Each Clippy output file contains the findings from an entire Rust
		// library that may contain many files, so skip findings from any
		// file besides the one we're currently checking.
		if spanPath != path {
			continue
		}

		var replacements []string

		messageLines := []string{result.Message}

		// Clippy output often contains "help" messages providing suggestions
		// for fixes and links to documentation. Append these messages to the
		// finding's text.
		for _, child := range result.Children {
			if child.Level != "help" {
				continue
			}

			if len(child.Spans) == 0 {
				messageLines = append(messageLines, fmt.Sprintf("help: %s", child.Message))
			}

			for _, span := range child.Spans {
				// Some suggestions have multiple primary spans to indicate that
				// multiple separate chunks of text need to be updated - e.g.
				// adding opening and closing parentheses around a multiline
				// block of code.
				if !span.Primary || span.SuggestedReplacement == "" {
					continue
				}
				if span.equals(primarySpan) {
					replacements = append(replacements, span.SuggestedReplacement)
					messageLines = append(messageLines, fmt.Sprintf(
						"help: %s: `%s`", child.Message, span.SuggestedReplacement,
					))
				}
			}
		}

		messageLines = append(messageLines, fmt.Sprintf("To reproduce locally, run `fx clippy -f %s`", path))

		lintID := strings.TrimPrefix(result.Code.Code, "clippy::")
		category := fmt.Sprintf("Clippy/%s/%s", result.Level, lintID)

		findings = append(findings, &staticanalysis.Finding{
			Category:     category,
			Message:      strings.Join(messageLines, "\n\n"),
			Path:         path,
			Line:         primarySpan.LineStart,
			EndLine:      primarySpan.LineEnd,
			Col:          primarySpan.ColumnStart,
			EndCol:       primarySpan.ColumnEnd,
			Replacements: replacements,
		})
	}

	return findings, nil
}

// clippyTargetForFile returns the Clippy output target for the library that a
// given source file is included in. If the file is not associated with any
// Clippy target, returns false.
func (c *clippyReporter) clippyTargetForFile(buildRelPath string) (target build.ClippyTarget, ok bool) {
	for _, target := range c.clippyTargets {
		if target.DisableClippy {
			continue
		}
		if slices.Contains(target.Sources, buildRelPath) {
			return target, true
		}
	}
	return build.ClippyTarget{}, false
}

// buildPathToCheckoutPath converts a path relative to the build directory into
// a source-relative path with platform-agnostic separators.
//
// E.g. "../../src/foo/bar.py" -> "src/foo/bar.py".
func (c *clippyReporter) buildPathToCheckoutPath(path string) (string, error) {
	absPath := filepath.Clean(filepath.Join(c.buildDir, path))
	path, err := filepath.Rel(c.checkoutDir, absPath)
	if err != nil {
		return "", err
	}
	return filepath.ToSlash(path), nil
}

// clippyResult represents one clippy finding.
type clippyResult struct {
	// Message is the human-readable explanation of the finding.
	Message string     `json:"message"`
	Code    clippyCode `json:"code"`
	// Level is the severity of the finding, e.g. "warning".
	Level string `json:"level"`
	// Spans is the file ranges that the finding applies to.
	Spans []clippySpan `json:"spans"`
	// Children lists auxiliary information that may be helpful for
	// understanding the lint result (help messages, references to related code,
	// etc.).
	Children []clippyResult `json:"children"`
}

// primarySpan returns the first (presumed only) primary span, if any, of this
// clippy result. It returns false if no primary span was found.
func (cr *clippyResult) primarySpan() (clippySpan, bool) {
	var primarySpans []clippySpan
	for _, s := range cr.Spans {
		if s.Primary {
			primarySpans = append(primarySpans, s)
		}
	}
	if len(primarySpans) == 0 {
		return clippySpan{}, false
	}
	// Some Clippy lints such as `uninit_vec` have multiple primary spans.
	// We'll always use the first one.
	return primarySpans[0], true
}

type clippyCode struct {
	// Code is the codename for this result type plus a "clippy::" prefix, e.g.
	// "clippy::too_many_arguments".
	Code string `json:"code"`
}

type clippySpan struct {
	// Path relative to the build dir.
	FileName string `json:"file_name"`

	// Whether this is the primary span for the associated Clippy result.
	Primary bool `json:"is_primary"`

	// Suggested replacement text to fix the lint (optional).
	SuggestedReplacement string `json:"suggested_replacement"`

	LineStart int `json:"line_start"`
	LineEnd   int `json:"line_end"`

	ColumnStart int `json:"column_start"`
	ColumnEnd   int `json:"column_end"`
}

func (s clippySpan) asMap() map[string]string {
	return map[string]string{
		"file_name":    s.FileName,
		"line_start":   strconv.Itoa(s.LineStart),
		"line_end":     strconv.Itoa(s.LineEnd),
		"column_start": strconv.Itoa(s.ColumnStart),
		"column_end":   strconv.Itoa(s.ColumnEnd),
	}
}

func (s clippySpan) equals(other clippySpan) bool {
	return maps.Equal(s.asMap(), other.asMap())
}

func mainImpl() error {
	var filesJSON string
	var checkoutDir string
	var buildDir string
	flag.StringVar(&filesJSON, "files-json", "", "JSON manifest of files to analyze")
	flag.StringVar(&checkoutDir, "checkout-dir", "", "Path to the Fuchsia checkout root")
	flag.StringVar(&buildDir, "build-dir", "", "Path to the Fuchsia build directory")
	flag.Parse()

	mod, err := build.NewModules(buildDir)
	if err != nil {
		return err
	}

	var paths []string
	if err := jsonutil.ReadFromFile(filesJSON, &paths); err != nil {
		return fmt.Errorf("failed to read -files-json: %w", err)
	}
	cr := clippyReporter{
		checkoutDir:   checkoutDir,
		buildDir:      buildDir,
		clippyTargets: mod.ClippyTargets(),
	}

	findings := []*staticanalysis.Finding{}
	for _, path := range paths {
		f, err := cr.report(context.Background(), path)
		if err != nil {
			return err
		}
		findings = append(findings, f...)
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(findings)
}

func main() {
	if err := mainImpl(); err != nil {
		log.Fatal(err)
	}
}
