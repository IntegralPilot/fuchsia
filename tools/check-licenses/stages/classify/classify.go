// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package classify

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"unicode/utf8"

	classifierLib "github.com/google/licenseclassifier/v2"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// Classifier implements pipeline.Classifier using a clean, stateless implementation
// of the Google License Classifier (v2). It does not rely on any legacy global state.
type Classifier struct {
	// The underlying Google License Classifier instance.
	Engine *classifierLib.Classifier

	// Map of extensions to classify. If empty, all files are classified.
	TargetExtensions map[string]bool
}

// NewClassifier creates a new, state-free classifier worker.
func NewClassifier(config Config) (*Classifier, error) {
	threshold := config.Threshold
	if threshold == 0.0 {
		threshold = 0.8 // default
	}

	engine := classifierLib.NewClassifier(threshold)

	for _, path := range config.PatternDirs {
		if err := engine.LoadLicenses(path); err != nil {
			return nil, fmt.Errorf("failed to load custom license patterns from %s: %w", path, err)
		}
	}

	targetExtensions := config.TargetExtensions
	if targetExtensions == nil {
		targetExtensions = make(map[string]bool)
	}

	return &Classifier{
		Engine:           engine,
		TargetExtensions: targetExtensions,
	}, nil
}

// Run listens to the input channel of FilteredProjects, reads and normalizes each file,
// executes the classification engine, and emits a ClassifiedFile struct.
func (c *Classifier) Run(ctx context.Context, in <-chan pipeline.FilteredProject) (<-chan pipeline.ClassifiedFile, error) {
	out := make(chan pipeline.ClassifiedFile, 100)

	go func() {
		defer close(out)
		defer metrics.AnalyzeDuration.Track()()

		numWorkers := runtime.NumCPU()
		if numWorkers < 1 {
			numWorkers = 1
		}
		var wg sync.WaitGroup
		wg.Add(numWorkers)

		for i := 0; i < numWorkers; i++ {
			go func() {
				defer wg.Done()
				for proj := range in {
					hasReadme := proj.Readme != nil
					isFirstParty := proj.IsFirstParty()

					for _, fileInfo := range proj.Files {
						path := fileInfo.Path
						if ctx.Err() != nil {
							return
						}

						metrics.TotalFilesProcessed.Inc()

						if fileInfo.IsNonLicense {
							metrics.FilesProcessed.Inc("skipped_non_license")
							// Emit an unclassified file
							select {
							case <-ctx.Done():
								return
							case out <- pipeline.ClassifiedFile{Path: path, ProjectRoot: proj.RootPath, IsLicenseFile: false, HasReadme: hasReadme, IsFirstParty: isFirstParty}:
							}
							continue
						}

						// If TargetExtensions is configured, skip files that don't match.
						// However, ALWAYS classify files that look like dedicated license files
						// (e.g., LICENSE, NOTICE, COPYING) regardless of their extension.
						isLicense := fileInfo.IsLicenseFile || IsLicenseFilename(path)
						if len(c.TargetExtensions) > 0 && !isLicense {
							ext := filepath.Ext(path)
							if !c.TargetExtensions[ext] {
								metrics.FilesProcessed.Inc("skipped_extension")
								// Emit an unclassified file
								select {
								case <-ctx.Done():
									return
								case out <- pipeline.ClassifiedFile{Path: path, ProjectRoot: proj.RootPath, IsLicenseFile: isLicense, HasReadme: hasReadme, IsFirstParty: isFirstParty}:
								}
								continue
							}
						}

						classified, err := c.ClassifyFile(path, proj.RootPath, isLicense, fileInfo.LicenseParser)
						if err != nil {
							log.Printf("Failed to read/classify file %s: %v\n", path, err)
							continue
						}
						classified.HasReadme = hasReadme
						classified.IsFirstParty = isFirstParty

						metrics.FilesProcessed.Inc("classified")

						if classified.IsLicenseFile {
							metrics.LicenseFilesFound.Inc()
						} else if len(classified.Matches) > 0 {
							metrics.SourceFilesWithLicenses.Inc()
						}

						select {
						case <-ctx.Done():
							return
						case out <- *classified:
						}
					}
				}
			}()
		}
		wg.Wait()
	}()

	return out, nil
}

// extractLines returns a zero-allocation slice of the original byte array
// containing only the specified 1-based start and end lines.
func extractLines(data []byte, startLine, endLine int) []byte {
	if startLine <= 0 || endLine < startLine || len(data) == 0 {
		return data
	}

	startByte := 0
	currentLine := 1

	// Find the start byte offset
	for i := 0; i < len(data) && currentLine < startLine; i++ {
		if data[i] == '\n' {
			currentLine++
			startByte = i + 1
		}
	}

	// Find the end byte offset
	endByte := startByte
	for i := startByte; i < len(data) && currentLine <= endLine; i++ {
		if data[i] == '\n' {
			currentLine++
		}
		endByte = i + 1
	}

	return data[startByte:endByte]
}

// IsLicenseFilename returns true if the file name looks like a dedicated license file.
func IsLicenseFilename(path string) bool {
	base := filepath.Base(path)
	upper := strings.ToUpper(base)
	return strings.Contains(upper, "LICENSE") || strings.Contains(upper, "NOTICE") || strings.Contains(upper, "COPYING")
}

// readFile reads a file from disk. For known license files (like LICENSE or NOTICE),
// it reads the entire file. For standard source files (like .cc or .py), it attempts
// to extract only the comment header at the top of the file to save CPU and memory
// without risk of truncating mid-word.
// Returns the extracted bytes and an error.
func (c *Classifier) ReadFile(path string, isLicense bool) ([]byte, error) {
	// Explicit license files must be read in their entirety
	if isLicense {
		return os.ReadFile(path)
	}

	// For standard source files, extract the semantic comment header
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var buf bytes.Buffer
	scanner := bufio.NewScanner(f)

	// A basic heuristic: if we hit a blank line or a line that isn't a comment
	// (or shebang) within the first 100 lines, we assume the copyright header is over.
	lineCount := 0
	inHeader := true

	for scanner.Scan() && inHeader && lineCount < 150 {
		line := scanner.Text()
		trimmed := strings.TrimSpace(line)

		// Let blank lines pass through if they are near the top,
		// but don't count them as breaking the header unless we've seen text
		if trimmed == "" {
			buf.WriteString(line + "\n")
			lineCount++
			continue
		}

		// Comment prefixes common in Fuchsia
		isComment := strings.HasPrefix(trimmed, "//") ||
			strings.HasPrefix(trimmed, "#") ||
			strings.HasPrefix(trimmed, "/*") ||
			strings.HasPrefix(trimmed, "*") ||
			strings.HasPrefix(trimmed, "<!--")

		if isComment {
			buf.WriteString(line + "\n")
		} else {
			// We hit actual source code, the header is over.
			inHeader = false
		}
		lineCount++
	}

	if err := scanner.Err(); err != nil {
		return nil, err
	}

	// If we somehow didn't find any comments, fallback to just returning
	// what we read so far to prevent missing a weirdly formatted license.
	return buf.Bytes(), nil
}

// windows1252 is a lookup table used to translate the upper 128 bytes (0x80 to 0xFF)
// of the legacy Windows-1252 character encoding into modern Unicode code points.
var windows1252 = [128]rune{
	0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
	0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0xFFFD, 0x017D, 0xFFFD,
	0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
	0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
	0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
	0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
	0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
	0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
	0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
	0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
	0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
	0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
	0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
	0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
	0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
	0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
}

// forceUTF8 checks if the provided byte slice is valid UTF-8.
// If not, it assumes Windows-1252 and decodes it.
func forceUTF8(data []byte) []byte {
	if utf8.Valid(data) {
		return data
	}

	var buf bytes.Buffer
	for _, b := range data {
		if b < 0x80 {
			buf.WriteByte(b)
		} else {
			buf.WriteRune(windows1252[b-0x80])
		}
	}
	return buf.Bytes()
}

// ClassifyFile reads a file from disk, normalizes it, chunks it, and runs the classification engine.
func (c *Classifier) ClassifyFile(path string, projectRoot string, isLicense bool, parser string) (*pipeline.ClassifiedFile, error) {
	// 1. Read the file (with truncation logic)
	rawBytes, err := c.ReadFile(path, isLicense)
	if err != nil {
		return nil, err
	}

	// 2. Normalize text to UTF-8
	normalizedText := forceUTF8(rawBytes)

	// 3. Chunk the text based on the file-level LicenseParser
	var chunks [][]byte
	if parser == "" {
		parser = "Single License" // Default parser behavior
	}

	if strings.HasPrefix(parser, "Delimiter: ") {
		delim := strings.TrimPrefix(parser, "Delimiter: ")
		if len(delim) > 0 {
			blocks := bytes.Split(normalizedText, []byte(delim))
			for _, block := range blocks {
				block = bytes.TrimSpace(block)
				if len(block) > 0 {
					chunks = append(chunks, block)
				}
			}
			chunks = deduplicateChunks(chunks)
		}
	} else {
		switch parser {
		case "Android":
			chunks = parseAndroid(normalizedText)
		case "Chromium":
			chunks = parseChromium(normalizedText)
		case "Flutter":
			chunks = parseFlutter(normalizedText)
		case "Google":
			chunks = parseGoogle(normalizedText)
		case "OneDelimiter":
			chunks = parseOneDelimiter(normalizedText)
		default:
			chunks = [][]byte{normalizedText}
		}
	}

	// 4. Classify each chunk and extract data
	var matches []pipeline.LicenseMatch
	for _, chunk := range chunks {
		func() {
			defer metrics.ClassifierDuration.Track()()
			results := c.Engine.Match(chunk)
			for _, match := range results.Matches {
				if match.Name != "" && !strings.HasPrefix(match.MatchType, "_") {
					metrics.LicenseDetected.Inc(match.Name, "unrecognized", "unrecognized")
					matches = append(matches, pipeline.LicenseMatch{
						SPDXID:      match.Name,
						MatchType:   match.MatchType,
						PatternName: match.Variant,
						StartLine:   match.StartLine,
						EndLine:     match.EndLine,
						Text:        extractLines(chunk, match.StartLine, match.EndLine),
					})
				}
			}
		}()
	}

	return &pipeline.ClassifiedFile{
		Path:          path,
		ProjectRoot:   projectRoot,
		IsLicenseFile: isLicense,
		AnalyzedText:  normalizedText,
		Matches:       matches,
	}, nil
}
