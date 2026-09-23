// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file allows derived runners to quickly specialize the generic |Runner| unit tests.
// See src/sys/fuzzing/common/runner-unittest.h for details.

#ifndef RUNNER_TEST
#error "RUNNER_TEST must be defined before including this file."
#endif

// These tests do not include any intentional crashes and may be run in a normal test environment.
// See also runner-error-unittest.inc and runner-fatal-unittest.inc.

TEST_F(RUNNER_TEST, InitializeCorpus) { InitializeCorpus(); }
TEST_F(RUNNER_TEST, InitializeDictionary) { InitializeDictionary(); }
TEST_F(RUNNER_TEST, TryOneNoError) { TryOneNoError(); }
TEST_F(RUNNER_TEST, TryOneWithError) { TryOneWithError(); }
TEST_F(RUNNER_TEST, TryOneWithLeak) { TryOneWithLeak(); }
TEST_F(RUNNER_TEST, MinimizeNoError) { MinimizeNoError(); }
TEST_F(RUNNER_TEST, CleanseNoReplacement) { CleanseNoReplacement(); }
TEST_F(RUNNER_TEST, CleanseAlreadyClean) { CleanseAlreadyClean(); }
TEST_F(RUNNER_TEST, FuzzUntilError) { FuzzUntilError(); }
TEST_F(RUNNER_TEST, FuzzUntilRuns) { FuzzUntilRuns(); }
TEST_F(RUNNER_TEST, FuzzUntilTime) { FuzzUntilTime(); }
TEST_F(RUNNER_TEST, MergeSeedError) { MergeSeedError(); }
