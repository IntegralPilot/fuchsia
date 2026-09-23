#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
class Event(object):
    """The base class for all event objects."""


# TODO(markdr): Move these into test_runner.py
class TestEvent(Event):
    """The base class for test-related events."""

    def __init__(self):
        pass


class TestCaseEvent(TestEvent):
    """The base class for test-case-related events."""

    def __init__(self, test_class, test_case):
        super().__init__()
        self.test_class = test_class
        self.test_case = test_case

    @property
    def test_case_name(self):
        return self.test_case

    @property
    def test_class_name(self):
        return self.test_class.__class__.__name__


class TestCaseSignalEvent(TestCaseEvent):
    """The base class for test-case-signal-related events."""

    def __init__(self, test_class, test_case, test_signal):
        super().__init__(test_class, test_case)
        self.test_signal = test_signal


class TestCaseBeginEvent(TestCaseEvent):
    """The event posted when a test case has begun."""


class TestCaseEndEvent(TestCaseSignalEvent):
    """The event posted when a test case has ended."""


class TestCaseSkippedEvent(TestCaseSignalEvent):
    """The event posted when a test case has been skipped."""


class TestCaseFailureEvent(TestCaseSignalEvent):
    """The event posted when a test case has failed."""


class TestCasePassedEvent(TestCaseSignalEvent):
    """The event posted when a test case has passed."""


class TestClassEvent(TestEvent):
    """The base class for test-class-related events"""

    def __init__(self, test_class):
        super().__init__()
        self.test_class = test_class


class TestClassBeginEvent(TestClassEvent):
    """The event posted when a test class has begun testing."""


class TestClassEndEvent(TestClassEvent):
    """The event posted when a test class has finished testing."""

    def __init__(self, test_class, result):
        super().__init__(test_class)
        self.result = result
