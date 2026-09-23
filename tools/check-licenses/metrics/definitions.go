// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

var (
	// --- DOMAIN METRICS ---

	LicenseDetected = RegisterCounter(
		"license_detected",
		"Number of licenses found",
		"spdx_id", "policy_category", "ecosystem",
	)

	LicenseFilesFound = RegisterCounter(
		"license_files_found",
		"Number of license files found",
	)

	SourceFilesWithLicenses = RegisterCounter(
		"source_files_with_licenses",
		"Number of source files found containing licenses",
	)

	TotalFilesProcessed = RegisterCounter(
		"total_files_processed",
		"Total number of files processed",
	)

	// --- OPERATIONAL METRICS ---

	PhaseDuration = RegisterTimer(
		"phase_duration",
		"Time spent executing major phases",
	)

	FilesProcessed = RegisterCounter(
		"files_processed",
		"Number of files processed",
		"extension", "status", // status: "analyzed", "skipped", "cached", "error"
	)

	DirectoriesProcessed = RegisterCounter(
		"directories_processed",
		"Number of directories processed",
		"status", // status: "analyzed", "skipped", "missing_project"
	)

	SymlinksProcessed = RegisterCounter(
		"symlinks_processed",
		"Number of symlinks processed",
		"status",
	)

	ProjectsProcessed = RegisterCounter(
		"projects_processed",
		"Number of projects processed",
		"status", // status: "discovered", "filtered", "skipped", "custom_init", "readme_error", "cache_hit"
	)

	TemplatesProcessed = RegisterCounter(
		"templates_processed",
		"Number of templates processed",
		"status",
	)

	// --- FILE PACKAGE METRICS ---

	DiskReadDuration = RegisterTimer(
		"disk_read_duration",
		"Time spent reading files from disk",
	)

	ClassifierDuration = RegisterTimer(
		"classifier_duration",
		"Time spent executing the License Classifier",
	)

	NoticeParsingDuration = RegisterTimer(
		"notice_parsing_duration",
		"Time spent parsing and deduplicating upstream NOTICE files",
	)

	TransliterationDuration = RegisterTimer(
		"transliteration_duration",
		"Time spent transliterating legacy encodings to UTF-8",
	)

	FileTruncation = RegisterCounter(
		"file_truncation",
		"Number of regular files truncated vs read fully",
		"status", // status: "truncated", "full"
	)

	NoticeSegments = RegisterCounter(
		"notice_segments",
		"Number of raw segments found vs unique segments kept",
		"status", // status: "raw", "unique"
	)

	FileEncoding = RegisterCounter(
		"file_encoding",
		"Number of files with valid UTF-8 vs legacy encodings",
		"encoding", // encoding: "utf8", "legacy"
	)

	// --- DIRECTORY PACKAGE METRICS ---

	DirectoryTraversalDuration = RegisterTimer(
		"directory_traversal_duration",
		"Time spent crawling the directory structure",
	)

	ReadmeParsingDuration = RegisterTimer(
		"readme_parsing_duration",
		"Time spent parsing README.fuchsia files",
	)

	BoundaryDetectionType = RegisterCounter(
		"boundary_detection_type",
		"How the tool decided to start a new project",
		"detection_type", // detection_type: "explicit_fuchsia", "explicit_chromium", "fallback_go", "fallback_rust", "fallback_dart", "inherited"
	)

	OrphanedEntities = RegisterCounter(
		"orphaned_entities",
		"Number of files or directories attributed to UnknownProject",
		"type", // type: "file", "directory"
	)

	// --- PROJECT PACKAGE METRICS ---

	MissingMetadata = RegisterCounter(
		"missing_metadata",
		"Number of projects missing required fields",
		"field", // field: "name", "license", "url"
	)

	FilterDuration = RegisterTimer(
		"filter_duration",
		"Time spent executing GN and filtering the build graph",
	)

	AnalyzeDuration = RegisterTimer(
		"analyze_duration",
		"Total time spent spinning up goroutines to analyze all filtered projects",
	)

	// --- README PACKAGE METRICS ---

	ReadmeParseDuration = RegisterTimer(
		"readme_parse_duration",
		"Time spent unmarshaling text proto files or Cargo.toml",
	)

	GitCommandDuration = RegisterTimer(
		"git_command_duration",
		"Time spent running git commands to discover upstream URLs",
	)

	MalformedReadmeLines = RegisterCounter(
		"malformed_readme_lines",
		"Number of lines in a README.fuchsia file that the parser could not understand",
		"type", // type: "unknown_directive", "parse_error"
	)

	DeprecatedDirectives = RegisterCounter(
		"deprecated_directives",
		"Usage of legacy syntax in README.fuchsia files",
		"directive",
	)

	ReadmeGenerationType = RegisterCounter(
		"readme_generation_type",
		"How a Readme struct was created",
		"type", // type: "explicit", "synthesized_rust", "synthesized_go", "synthesized_dart"
	)

	ReadmeCacheHits = RegisterCounter(
		"readme_cache_hits",
		"How often reading a README.fuchsia file was avoided because it was in memory",
		"status", // status: "hit"
	)

	// --- RESULT PACKAGE METRICS ---

	TotalRuntime = RegisterTimer(
		"total_runtime",
		"Total wall time of execution",
	)

	ValidationErrors = RegisterCounter(
		"validation_errors",
		"Number of compliance validation errors before allowlist",
		"check_name",
	)

	AllowlistHits = RegisterCounter(
		"allowlist_hits",
		"Number of compliance validation errors ignored via allowlist",
		"check_name",
	)

	LicenseDeduplication = RegisterCounter(
		"license_deduplication",
		"Number of raw license texts found vs unique texts kept",
		"type", // type: "raw_texts", "unique_texts"
	)

	ChecksDuration = RegisterTimer(
		"checks_duration",
		"Time spent running compliance checks",
	)

	TemplateExpansionDuration = RegisterTimer(
		"template_expansion_duration",
		"Time spent expanding notice templates",
	)

	SpdxGenerationDuration = RegisterTimer(
		"spdx_generation_duration",
		"Time spent generating SPDX SBOM",
	)
)
