// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package resultdb

import (
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	resultpb "go.chromium.org/luci/resultdb/proto/v1"
	sinkpb "go.chromium.org/luci/resultdb/sink/proto/v1"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/durationpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

const (
	// Test ID is limited to 512 bytes.
	// https://source.chromium.org/chromium/infra/infra_superproject/+/main:infra/go/src/go.chromium.org/luci/resultdb/pbutil/test_id.go;l=508;drc=14e57c2183912ea2a9c93cd19dbe6eb7347283e8
	MaxTestIDLength = 512
	// Failure reason is limited to 1024 bytes.
	// https://source.chromium.org/chromium/infra/infra_superproject/+/main:infra/go/src/go.chromium.org/luci/resultdb/pbutil/test_result.go;l=44;drc=bfb50731e1b97d7ca771fb2d31dc7338c3db40f5
	MaxFailureReasonLength = 1024
	// MaxFailureReasonTotalSize is the maximum total protobuf size of a failure reason (16384 bytes).
	MaxFailureReasonTotalSize = 16384
	// MaxBatchSize is the maximum number of items (results or exonerations) reported in a single request.
	MaxBatchSize = 250
	// DefaultRepo is the Gitiles repository URL for the main Fuchsia repository.
	DefaultRepo = "https://fuchsia.googlesource.com/fuchsia"
	// VendorGoogleRepo is the Gitiles repository URL for the Google vendor repository.
	VendorGoogleRepo = "https://turquoise-internal.googlesource.com/vendor/google"
	// VendorGooglePathPrefix is the path prefix for the Google vendor repository.
	VendorGooglePathPrefix = "//vendor/google"
	// MaxLocationRepoLength is the maximum length in bytes for a test location repository URL (256 bytes).
	// //third_party/luci-go/resultdb/proto/v1/test_metadata.proto:106
	// https://source.chromium.org/chromium/infra/infra_superproject/+/main:infra/go/src/go.chromium.org/luci/resultdb/proto/v1/test_metadata.proto;l=107
	MaxLocationRepoLength = 256
	// MaxLocationFileNameLength is the maximum length in bytes for a test location file name (512 bytes).
	// //third_party/luci-go/resultdb/proto/v1/test_metadata.proto:113
	// https://source.chromium.org/chromium/infra/infra_superproject/+/main:infra/go/src/go.chromium.org/luci/resultdb/proto/v1/test_metadata.proto;l=114
	MaxLocationFileNameLength = 512
)

// ParseSummary unmarshals the summary.json file content into runtests.TestSummary struct.
func ParseSummary(filePath string) (*runtests.TestSummary, error) {
	content, err := os.ReadFile(filePath)
	if err != nil {
		return nil, err
	}
	var summary runtests.TestSummary
	if err := json.Unmarshal(content, &summary); err != nil {
		return nil, err
	}
	return &summary, nil
}

// SummaryToResultSink converts runtests.TestSummary data into an array of result_sink TestResult and TestExoneration.
func SummaryToResultSink(s *runtests.TestSummary, tags []*resultpb.StringPair, outputRoot string) ([]*sinkpb.TestResult, []*sinkpb.TestExoneration, []string) {
	if len(outputRoot) == 0 {
		outputRoot, _ = os.Getwd()
	}
	rootPath, _ := filepath.Abs(outputRoot)
	var r []*sinkpb.TestResult
	var exonerations []*sinkpb.TestExoneration
	var ts []string
	for _, test := range s.Tests {
		if len(test.Cases) > 0 {
			testCases, testExonerations, testsSkipped := testCaseToResultSink(test.Cases, tags, &test, rootPath)
			r = append(r, testCases...)
			exonerations = append(exonerations, testExonerations...)
			ts = append(ts, testsSkipped...)
		}
		// TODO(b/502613208): If a top-level test passes but has a nested test case that
		// is exonerated, it currently reports both a "PASS" status and an exoneration.
		// This is redundant since ResultDB generally ignores exonerations for passing tests.
		if testResult, testExoneration, testSkipped, err := testDetailsToResultSink(tags, &test, rootPath); err == nil {
			if testResult == nil {
				panic("testResult shouldn't be nil when err is nil")
			}
			r = append(r, testResult)
			if testExoneration != nil {
				exonerations = append(exonerations, testExoneration)
			}
			if testSkipped != "" {
				panic(fmt.Sprintf("testSkipped should be empty when err is nil, got: %q", testSkipped))
			}
		} else {
			if testSkipped != "" {
				ts = append(ts, testSkipped)
			}
		}
	}
	return r, exonerations, ts
}

// InvocationLevelArtifacts creates resultdb artifacts for invocation-level files to be sent to ResultDB.
func InvocationLevelArtifacts(outputRoot string, invocationArtifacts []string) map[string]*sinkpb.Artifact {
	if len(outputRoot) == 0 {
		outputRoot, _ = os.Getwd()
	}
	rootPath, _ := filepath.Abs(outputRoot)
	artifacts := map[string]*sinkpb.Artifact{}

	// TODO(ihuh): Remove once these are passed in through recipes.
	if len(invocationArtifacts) == 0 {
		invocationArtifacts = []string{
			"infra_and_test_std_and_klog.txt",
			"serial_log.txt",
			"syslog.txt",
			"triage_output",
		}
	}
	for _, invocationArtifact := range invocationArtifacts {
		artifactFile := filepath.Join(rootPath, invocationArtifact)
		if isReadable(artifactFile) {
			artifacts[invocationArtifact] = &sinkpb.Artifact{
				Body:        &sinkpb.Artifact_FilePath{FilePath: artifactFile},
				ContentType: "text/plain",
			}
		}
	}
	return artifacts
}

func ProcessSummaries(summaries []string, tags []*resultpb.StringPair, outputRoot string) ([]*sinkpb.ReportTestResultsRequest, []*sinkpb.ReportTestExonerationsRequest, []string, error) {
	var requests []*sinkpb.ReportTestResultsRequest
	var exonerationRequests []*sinkpb.ReportTestExonerationsRequest
	var allTestsSkipped []string

	for _, summaryFile := range summaries {
		summary, err := ParseSummary(summaryFile)
		if err != nil {
			return nil, nil, nil, err
		}
		testResults, exonerations, testsSkipped := SummaryToResultSink(summary, tags, outputRoot)
		requests = append(requests, createTestResultsRequests(testResults, MaxBatchSize)...)
		exonerationRequests = append(exonerationRequests, createTestExonerationsRequests(exonerations, MaxBatchSize)...)
		allTestsSkipped = append(allTestsSkipped, testsSkipped...)
	}

	return requests, exonerationRequests, allTestsSkipped, nil
}

// artifactName returns a unique name to correspond to the file which
// will be uploaded as a resultDB artifact.
func artifactName(file string) string {
	re := regexp.MustCompile(`[^a-zA-Z0-9-_.\/]`)
	invalidChars := re.FindAllString(file, -1)
	for _, ch := range invalidChars {
		file = strings.ReplaceAll(file, ch, "_")
	}
	return file
}

// set the TestMetadata on TestResult
func setTestMetadata(r *sinkpb.TestResult, testDetail runtests.TestDetails, displayName string) {
	r.TestMetadata = &resultpb.TestMetadata{
		Name: displayName,
	}
	if testDetail.Metadata.ComponentID > 0 {
		r.TestMetadata.BugComponent = &resultpb.BugComponent{
			System: &resultpb.BugComponent_IssueTracker{
				IssueTracker: &resultpb.IssueTrackerComponent{
					ComponentId: int64(testDetail.Metadata.ComponentID),
				},
			},
		}
	}
	if len(testDetail.Metadata.Owners) > 0 {
		listOfOwners := testDetail.Metadata.Owners
		truncatedListOfOwners := listOfOwners
		if len(listOfOwners) > 5 {
			truncatedListOfOwners = listOfOwners[:5]
		}
		owners := strings.Join(truncatedListOfOwners, ",")
		r.Tags = append(r.Tags, &resultpb.StringPair{Key: "owners", Value: owners})
	}
	label := testDetail.SourceLabel
	if label == "" {
		label = testDetail.GNLabel
	}
	if len(label) > 0 {
		loc, err := parseTestLocation(label)
		if err != nil {
			log.Printf("[Warn] Failed to parse test location for %q: %v", label, err)
		} else {
			r.TestMetadata.Location = loc
		}
	}
}

// parseTestLocation parses a test's source label and resolves
// the source code directory or file path within the repository, returning a
// resultpb.TestLocation suitable for ResultDB / Milo Gitiles source linking.
func parseTestLocation(label string) (*resultpb.TestLocation, error) {
	// Strip toolchain suffix (e.g. "(//build/toolchain:x64)") from label.
	cleaned := strings.TrimSpace(label)
	if cleaned == "" {
		return nil, fmt.Errorf("label is empty")
	}

	if idx := strings.LastIndex(cleaned, "("); idx != -1 {
		candidate := cleaned[idx:]
		if strings.HasPrefix(candidate, "(//") || strings.HasPrefix(candidate, "(@//") || strings.HasPrefix(candidate, "(@@//") {
			if !strings.HasSuffix(candidate, ")") {
				return nil, fmt.Errorf("malformed toolchain in label %q", label)
			}
			cleaned = strings.TrimSpace(cleaned[:idx])
			if cleaned == "" {
				return nil, fmt.Errorf("label %q is empty after stripping toolchain", label)
			}
		} else if strings.HasSuffix(cleaned, ")") {
			// If the label ends with a closing parenthesis but does not look like a valid toolchain,
			// treat it as a malformed toolchain or unmatched parenthesis.
			return nil, fmt.Errorf("malformed toolchain in label %q", label)
		}
	} else if strings.HasSuffix(cleaned, ")") {
		return nil, fmt.Errorf("malformed toolchain in label %q", label)
	}

	// Validate label does not contain whitespace.
	if strings.ContainsAny(cleaned, " \t\n\r") {
		return nil, fmt.Errorf("label %q contains whitespace", label)
	}

	// Normalize Bazel/GN prefixes so the label starts with "//".
	for _, prefix := range []string{"@@//", "@//"} {
		if strings.HasPrefix(cleaned, prefix) {
			cleaned = "//" + strings.TrimPrefix(cleaned, prefix)
			break
		}
	}

	if !strings.HasPrefix(cleaned, "//") {
		// Support and normalize single leading slash (e.g. "/src/...") to "//" defensively
		// to tolerate tool or input variations, although GN and Bazel labels standardly begin with "//".
		if strings.HasPrefix(cleaned, "/") {
			cleaned = "/" + cleaned
		} else {
			return nil, fmt.Errorf("label %q must start with '//'", label)
		}
	}

	// Extract the source file or directory path from the cleaned label.
	parts := strings.SplitN(cleaned, ":", 2)
	dirPart := strings.TrimRight(parts[0], "/")
	if dirPart == "" {
		dirPart = "//"
	}

	fileName := dirPart
	if len(parts) == 2 {
		targetPart := parts[1]
		ext := path.Ext(targetPart)
		// If the target has a file extension and is not a compiled component manifest (.cm),
		// treat it as a file inside the package directory. We link to the directory for
		// .cm targets because the manifest source (usually .cml) resides in that package
		// directory, whereas the .cm itself is a build artifact.
		if ext != "" && ext != ".cm" {
			if dirPart == "//" {
				fileName = "//" + targetPart
			} else {
				fileName = fmt.Sprintf("%s/%s", dirPart, targetPart)
			}
		}
	}

	// Clean path using the path package (not filepath, since build labels always use forward slashes).
	// Strip leading slashes before cleaning so that path.Clean treats it as an unrooted relative path.
	// This preserves any leading ".." components so we can detect paths that escape the repository root,
	// and avoids Go's path.Clean collapsing double leading slashes into a single slash.
	cleanRel := path.Clean(strings.TrimLeft(fileName, "/"))
	if cleanRel == ".." || strings.HasPrefix(cleanRel, "../") {
		return nil, fmt.Errorf("label path %q escapes repository root: %s", label, cleanRel)
	}

	repoPath := "//"
	if cleanRel != "." && cleanRel != "" {
		repoPath = "//" + cleanRel
	}

	// For vendor/google, route to turquoise-internal.googlesource.com/vendor/google
	// and make path relative to that repository root.
	repo := DefaultRepo
	filePath := repoPath
	if strings.HasPrefix(repoPath, VendorGooglePathPrefix+"/") {
		repo = VendorGoogleRepo
		filePath = "//" + strings.TrimPrefix(repoPath, VendorGooglePathPrefix+"/")
	} else if repoPath == VendorGooglePathPrefix {
		repo = VendorGoogleRepo
		filePath = "//"
	}

	// Validate field lengths according to ResultDB TestLocation specifications.
	if len(repo) > MaxLocationRepoLength {
		return nil, fmt.Errorf("repo %q exceeds max length %d bytes (%d bytes)", repo, MaxLocationRepoLength, len(repo))
	}
	if len(filePath) > MaxLocationFileNameLength {
		return nil, fmt.Errorf("file path %q exceeds max length %d bytes (%d bytes)", filePath, MaxLocationFileNameLength, len(filePath))
	}

	// Keeping Line defaulting to 0 denotes an unknown or unspecified line number in ResultDB (which uses 1-based line numbers).
	return &resultpb.TestLocation{
		Repo:     repo,
		FileName: filePath,
	}, nil
}

// testCaseToResultSink converts TestCaseResult defined in //tools/testing/runtests/runtests.go
// to ResultSink's TestResult. A testcase will not be converted if test result cannot be
// mapped to result_sink.Status.
func testCaseToResultSink(testCases []runtests.TestCaseResult, tags []*resultpb.StringPair, testDetail *runtests.TestDetails, outputRoot string) ([]*sinkpb.TestResult, []*sinkpb.TestExoneration, []string) {
	var testResults []*sinkpb.TestResult
	var testExonerations []*sinkpb.TestExoneration
	var testsSkipped []string

	// Ignore the failure reason kind and error. We only check the top-level test status
	// to see if it passed, which would mean that a failed result for a test case is
	// expected and thus should be reported as a passed result.
	testStatus, _, _ := resultDBStatus(testDetail.Status)

	for _, testCase := range testCases {
		testID := fmt.Sprintf("%s/%s:%s", testDetail.Name, testCase.SuiteName, testCase.CaseName)
		if len(testID) > MaxTestIDLength {
			log.Printf("[ERROR] Skip uploading to ResultDB due to test_id exceeding %d bytes max limit: %q", MaxTestIDLength, testID)
			testsSkipped = append(testsSkipped, testID)
			continue
		}

		testCaseTags := append([]*resultpb.StringPair{
			{Key: "format", Value: testCase.Format},
			{Key: "is_test_case", Value: "true"},
		}, tags...)
		for _, tag := range testCase.Tags {
			testCaseTags = append(testCaseTags, &resultpb.StringPair{
				Key: tag.Key, Value: tag.Value,
			})
		}
		r := sinkpb.TestResult{
			TestId: testID,
			Tags:   testCaseTags,
		}
		testCaseStatus, testCaseFailureReasonKind, err := resultDBStatus(testCase.Status)
		if err != nil {
			log.Printf("[Warn] Skip uploading testcase: %s to ResultDB due to error: %v", testID, err)
			continue
		}
		if testStatus != resultpb.TestResult_PASSED && testCaseStatus == resultpb.TestResult_FAILED {
			r.FailureReason = toResultDBFailureReason(testCase.FailureReason, testCaseFailureReasonKind)
		} else if testCaseStatus == resultpb.TestResult_SKIPPED {
			r.SkippedReason = &resultpb.SkippedReason{Kind: resultpb.SkippedReason_DISABLED_AT_DECLARATION}
		} else if testStatus == resultpb.TestResult_PASSED {
			testCaseStatus = testStatus
		}
		r.StatusV2 = testCaseStatus
		r.StartTime = timestamppb.New(testDetail.StartTime)
		if testCase.Duration > 0 {
			r.Duration = durationpb.New(testCase.Duration)
		}
		r.Artifacts = make(map[string]*sinkpb.Artifact)
		for _, of := range testCase.OutputFiles {
			outputFile := filepath.Join(outputRoot, testDetail.OutputDir, of)
			if isReadable(outputFile) {
				r.Artifacts[artifactName(of)] = &sinkpb.Artifact{
					Body: &sinkpb.Artifact_FilePath{FilePath: outputFile},
				}
			} else {
				log.Printf("[Warn] outputFile: %s is not readable, skip.", outputFile)
			}
		}
		setTestMetadata(&r, *testDetail, testCase.DisplayName)
		testResults = append(testResults, &r)

		if testCase.Status == runtests.TestExonerated {
			testExonerations = append(testExonerations, &sinkpb.TestExoneration{
				TestId:          testID,
				ExplanationHtml: fmt.Sprintf("Test case %s was exonerated in the test summary.", testCase.CaseName),
				Reason:          resultpb.ExonerationReason_NOT_CRITICAL,
			})
		}
	}
	return testResults, testExonerations, testsSkipped
}

// testDetailsToResultSink converts TestDetail defined in /tools/testing/runtests/runtests.go
// to ResultSink's TestResult. Returns an error if a test result cannot be mapped to
// result_sink.Status
func testDetailsToResultSink(tags []*resultpb.StringPair, testDetail *runtests.TestDetails, outputRoot string) (*sinkpb.TestResult, *sinkpb.TestExoneration, string, error) {
	if len(testDetail.Name) > MaxTestIDLength {
		log.Printf("[ERROR] Skip uploading to ResultDB due to test_id exceeding %d bytes max limit: %q", MaxTestIDLength, testDetail.Name)
		return nil, nil, testDetail.Name, fmt.Errorf("The test name exceeds %d bytes max limit: %q ", MaxTestIDLength, testDetail.Name)
	}

	testTags := append([]*resultpb.StringPair{
		{Key: "gn_label", Value: testDetail.GNLabel},
		// Most consumers should use `source_label` rather than `gn_label` since
		// it better corresponds to the location of the test's source code for
		// Bazel tests.
		{Key: "source_label", Value: testDetail.SourceLabel},
		{Key: "test_case_count", Value: strconv.Itoa(len(testDetail.Cases))},
		{Key: "affected", Value: strconv.FormatBool(testDetail.Affected)},
		{Key: "is_top_level_test", Value: "true"},
	}, tags...)
	for _, tag := range testDetail.Tags {
		testTags = append(testTags, &resultpb.StringPair{
			Key: tag.Key, Value: tag.Value,
		})
	}
	r := sinkpb.TestResult{
		TestId: testDetail.Name,
		Tags:   testTags,
	}
	testStatus, failureReasonKind, err := resultDBStatus(testDetail.Status)
	if err != nil {
		log.Printf("[Warn] Skip uploading test target: %s to ResultDB due to error: %v", testDetail.Name, err)
		return nil, nil, "", err
	}
	r.StatusV2 = testStatus
	if testStatus == resultpb.TestResult_FAILED {
		r.FailureReason = toResultDBFailureReason(testDetail.FailureReason, failureReasonKind)
	} else if testStatus == resultpb.TestResult_SKIPPED {
		r.SkippedReason = &resultpb.SkippedReason{Kind: resultpb.SkippedReason_OTHER, ReasonMessage: "skipped because unaffected"}
	}

	r.StartTime = timestamppb.New(testDetail.StartTime)
	if testDetail.DurationMillis > 0 {
		r.Duration = durationpb.New(time.Duration(testDetail.DurationMillis) * time.Millisecond)
	}
	r.Artifacts = make(map[string]*sinkpb.Artifact)
	for _, of := range testDetail.OutputFiles {
		outputFile := filepath.Join(outputRoot, testDetail.OutputDir, of)
		if isReadable(outputFile) {
			r.Artifacts[artifactName(of)] = &sinkpb.Artifact{
				Body: &sinkpb.Artifact_FilePath{FilePath: outputFile},
			}
		} else {
			log.Printf("[Warn] outputFile: %s is not readable, skip.", outputFile)
		}
	}

	setTestMetadata(&r, *testDetail, "")

	if testDetail.Status == runtests.TestExonerated {
		return &r, &sinkpb.TestExoneration{
			TestId:          testDetail.Name,
			ExplanationHtml: fmt.Sprintf("Test target %s was exonerated in the test summary.", testDetail.Name),
			Reason:          resultpb.ExonerationReason_NOT_CRITICAL,
		}, "", nil
	}

	return &r, nil, "", nil
}

func resultDBStatus(result runtests.TestStatus) (resultpb.TestResult_Status, resultpb.FailureReason_Kind, error) {
	switch result {
	case runtests.TestSuccess:
		return resultpb.TestResult_PASSED, resultpb.FailureReason_KIND_UNSPECIFIED, nil
	case runtests.TestFailure:
		return resultpb.TestResult_FAILED, resultpb.FailureReason_ORDINARY, nil
	case runtests.TestSkipped:
		return resultpb.TestResult_SKIPPED, resultpb.FailureReason_KIND_UNSPECIFIED, nil
	case runtests.TestAborted:
		return resultpb.TestResult_FAILED, resultpb.FailureReason_TIMEOUT, nil
	case runtests.TestInfraFailure:
		return resultpb.TestResult_FAILED, resultpb.FailureReason_CRASH, nil
	case runtests.TestExonerated:
		return resultpb.TestResult_FAILED, resultpb.FailureReason_ORDINARY, nil
	}
	return resultpb.TestResult_STATUS_UNSPECIFIED, resultpb.FailureReason_KIND_UNSPECIFIED, fmt.Errorf("cannot map Result: %s to result_sink test_result status", result)
}

func toResultDBFailureReason(fr *runtests.FailureReason, defaultKind resultpb.FailureReason_Kind) *resultpb.FailureReason {
	res := &resultpb.FailureReason{
		Kind: defaultKind,
	}
	if fr == nil || len(fr.Errors) == 0 {
		return res
	}

	// Copy over errors to the resultpb.FailureReason.
	res.Errors = make([]*resultpb.FailureReason_Error, 0, len(fr.Errors))
	for i, e := range fr.Errors {
		pbErr := &resultpb.FailureReason_Error{
			Message: truncateString(e.Message, MaxFailureReasonLength),
		}
		res.Errors = append(res.Errors, pbErr)
		if proto.Size(res) > MaxFailureReasonTotalSize {
			res.Errors = res.Errors[:len(res.Errors)-1]
			res.TruncatedErrorsCount = int32(len(fr.Errors) - i)
			break
		}
	}
	return res
}

func isReadable(p string) bool {
	if len(p) == 0 {
		return false
	}
	info, err := os.Stat(p)
	if err != nil {
		return false
	}
	if info.IsDir() {
		return false
	}
	f, err := os.Open(p)
	if err != nil {
		return false
	}
	_ = f.Close()
	return true
}

func truncateString(str string, maxLength int) string {
	if len(str) <= maxLength {
		return str
	}
	// We want to append "..." to maxLength, which takes up 3 spaces. If maxLength is less than that, just return empty.
	if maxLength <= 3 {
		return ""
	}
	runes := []rune(str)
	byteCount := 0
	for _, char := range runes {
		if byteCount+len(string(char)) > (maxLength - 3) {
			if byteCount == 0 {
				return ""
			}
			return str[:byteCount] + "..."
		}
		byteCount = byteCount + len(string(char))
	}
	return str
}

// createTestResultsRequests breaks an array of sinkpb.TestResult into an array of sinkpb.ReportTestResultsRequest
// using the specified chunk size limit.
func createTestResultsRequests(results []*sinkpb.TestResult, chunkSize int) []*sinkpb.ReportTestResultsRequest {
	return chunkSlice(results, chunkSize, func(chunk []*sinkpb.TestResult) *sinkpb.ReportTestResultsRequest {
		return &sinkpb.ReportTestResultsRequest{TestResults: chunk}
	})
}

// createTestExonerationsRequests breaks an array of sinkpb.TestExoneration into an array of sinkpb.ReportTestExonerationsRequest
// using the specified chunk size limit.
func createTestExonerationsRequests(exonerations []*sinkpb.TestExoneration, chunkSize int) []*sinkpb.ReportTestExonerationsRequest {
	return chunkSlice(exonerations, chunkSize, func(chunk []*sinkpb.TestExoneration) *sinkpb.ReportTestExonerationsRequest {
		return &sinkpb.ReportTestExonerationsRequest{TestExonerations: chunk}
	})
}

// chunkSlice generic helper splits a slice of T into chunks of the specified size,
// and wraps each chunk into a request R using the provided wrapper function.
func chunkSlice[T any, R any](items []T, chunkSize int, wrapper func([]T) R) []R {
	if len(items) == 0 {
		return nil
	}
	totalChunks := (len(items)-1)/chunkSize + 1
	requests := make([]R, totalChunks)
	for i := 0; i < totalChunks; i++ {
		start := i * chunkSize
		end := start + chunkSize
		if end > len(items) {
			end = len(items)
		}
		// Use 3-index slicing to set capacity to length for safety
		requests[i] = wrapper(items[start:end:end])
	}
	return requests
}
