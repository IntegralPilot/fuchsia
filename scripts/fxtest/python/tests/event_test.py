# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest

import environment
import event
import selection_types
import test_list_file
import tests_json_file


class TestEvents(unittest.IsolatedAsyncioTestCase):
    def test_invalid_payload(self) -> None:
        """Test that an event payload may have only one field set."""
        self.assertRaises(
            ValueError,
            lambda: event.EventPayloadUnion(
                start_timestamp=10, parse_flags={"foo": "bar"}
            ),
        )

    async def test_empty_run(self) -> None:
        """Test that a simple run consisting of start/stop is populated correctly."""
        recorder = event.EventRecorder()
        recorder.emit_init()
        recorder.emit_end()

        raw_events = [e async for e in recorder.iter()]
        events = [(e.id, e.starting, e.ending, e.payload) for e in raw_events]

        start_timestamp = recorder._system_time_start

        self.assertListEqual(
            events,
            [
                (
                    0,
                    True,
                    None,
                    event.EventPayloadUnion(start_timestamp=start_timestamp),
                ),
                (0, None, True, None),
            ],
        )
        self.assertEqual(
            raw_events[0].timestamp, recorder._monotonic_time_start
        )

    async def test_empty_run_async(self) -> None:
        """Test a start/end run, but ensure that we receive at least one event asynchronously."""
        recorder = event.EventRecorder()
        recorder.emit_init()

        # Use this async condition to trigger writing the end event.
        # We trigger this condition after the first EventRecorder event is
        # returned. This means that the end event must be asynchronously queued
        # and read.
        wait_to_emit = asyncio.Event()

        async def emit_end_task() -> None:
            await wait_to_emit.wait()
            recorder.emit_end()

        task = asyncio.create_task(emit_end_task())

        events = []
        async for e in recorder.iter():
            events.append(e)
            wait_to_emit.set()

        await task

        self.assertEqual(len(events), 2)

    async def test_iter_queue_cleanup(self) -> None:
        """Test that iterating over EventRecorder cleans up its queue from _queues on completion."""
        recorder = event.EventRecorder()
        recorder.emit_init()
        self.assertEqual(len(recorder._queues), 0)

        async for _ in recorder.iter():
            self.assertEqual(len(recorder._queues), 1)
            recorder.emit_end()

        self.assertEqual(len(recorder._queues), 0)

    async def test_full_example(self) -> None:
        """Run through a representative example of events for fx test"""

        recorder = event.EventRecorder()
        recorder.emit_init()

        recorder.emit_parse_flags({"test": True})
        recorder.emit_process_env(
            environment.ExecutionEnvironment(
                fuchsia_dir=".",
                out_dir=".",
                log_file=None,
                test_json_file=".",
                disabled_ctf_tests_file="",
            )
        )
        recorder.emit_instruction_message("This is just a test")
        recorder.emit_info_message("About to do testing")
        recorder.emit_warning_message("Still about to test, but in yellow")
        recorder.emit_verbatim_message("Print verbatim stuff")
        build_id = recorder.emit_build_start(["//test:one"])
        recorder.emit_end(id=build_id)
        preflight_id = recorder.emit_event_group(
            "Preflight checks", queued_events=2
        )
        pre_id_1 = recorder.emit_program_start(
            "check", ["--one"], parent=preflight_id
        )
        pre_id_2 = recorder.emit_program_start(
            "check", ["--two"], parent=preflight_id
        )
        recorder.emit_program_output(
            pre_id_1, "Check OK", event.ProgramOutputStream.STDOUT
        )
        recorder.emit_program_output(
            pre_id_2,
            "Check warning, but still OK",
            event.ProgramOutputStream.STDERR,
        )
        recorder.emit_program_termination(pre_id_1, 0)
        recorder.emit_program_termination(
            pre_id_2, 1, "Had a check error, continuing..."
        )
        parse_id = recorder.emit_start_file_parsing(
            "tests.json", "/home/tests.json"
        )
        recorder.emit_end(id=parse_id)
        recorder.emit_test_file_loaded(
            [
                tests_json_file.TestEntry(
                    tests_json_file.TestSection(
                        "my-test", "//src/my-test", "linux"
                    )
                ),
                tests_json_file.TestEntry(
                    tests_json_file.TestSection(
                        "other-test", "//src/other-test", "linux"
                    )
                ),
            ],
            "/home/tests.json",
        )
        recorder.emit_test_selections(
            selection_types.TestSelections(
                selected=[
                    test_list_file.Test(
                        tests_json_file.TestEntry(
                            tests_json_file.TestSection(
                                "my-test", "//src/my-test", "linux"
                            )
                        ),
                        test_list_file.TestListEntry("my-test", []),
                    ),
                ],
                selected_but_not_run=[],
                best_score={
                    "my-test": 1,
                    "other-test": 0,
                },
                group_matches=[],
                fuzzy_distance_threshold=3,
            )
        )
        test_group_id = recorder.emit_test_group(1)
        suite_id = recorder.emit_test_suite_started(
            "my-test", hermetic=True, parent=test_group_id
        )
        recorder.emit_test_suite_ended(
            suite_id, event.TestSuiteStatus.PASSED, None
        )
        recorder.emit_end(id=test_group_id)
        recorder.emit_end()

        events = [e async for e in recorder.iter()]

        for e in events:
            string_output = recorder.event_string(e)
            self.assertFalse(
                "BUG" in string_output,
                f"Bug found in event output: {string_output}",
            )
            if e.starting:
                self.assertTrue(
                    ":S]" in string_output,
                    f"Expected S marker: {string_output}",
                )
            if e.ending:
                self.assertTrue(
                    ":E]" in string_output,
                    f"Expected E marker: {string_output}",
                )
            if not e.starting and not e.ending and e.id is not None:
                self.assertTrue(
                    ":I]" in string_output,
                    f"Expected I marker: {string_output}",
                )
            if e.id is None:
                self.assertTrue(
                    "[_______]" in string_output,
                    f"Expected blank id line: {string_output}",
                )

            if e.id is not None:
                id_format = f"[{e.id:05}:"
                self.assertTrue(id_format in string_output, f"Expected ")

        round_trip_events = [
            event.Event.from_dict(e.to_dict()) for e in events  # type:ignore
        ]
        self.assertListEqual(events, round_trip_events)


class TestEventSpan(unittest.IsolatedAsyncioTestCase):
    def _create_event_span(
        self, payload: event.EventPayloadUnion | None
    ) -> event.EventSpan:
        e = event.Event(
            id=event.Id(1), timestamp=0, starting=True, payload=payload
        )
        return event.EventSpan(start_time=0, start_event=e)

    def test_classify_event_payload(self) -> None:
        """Test the classify function can correctly classify event with payloads"""
        payload = event.EventPayloadUnion(
            parsing_file=event.FileParsingPayload("name", "path")
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.PARSING,
        )

        payload = event.EventPayloadUnion(
            program_execution=event.ProgramExecutionPayload(
                "cmd", ["--flag", "dldist"], {}
            )
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.SEARCHING,
        )

        payload = event.EventPayloadUnion(
            test_suite_started=event.TestSuiteStartedPayload("testsuiteA")
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.TESTING,
        )

        payload = event.EventPayloadUnion(build_targets=["//foo"])
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.BUILDING,
        )

        payload = event.EventPayloadUnion(
            program_execution=event.ProgramExecutionPayload(
                "cmd", ["--flag"], {}
            )
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.OTHERS,
        )
        self.assertEqual(
            self._create_event_span(None).category,
            event.EventStatCategory.OTHERS,
        )

        payload = event.EventPayloadUnion(
            event_group=event.EventGroupPayload("group")
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.IGNORE,
        )

        payload = event.EventPayloadUnion(test_group=event.TestGroupPayload(1))
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.IGNORE,
        )

        payload = event.EventPayloadUnion(
            program_execution=event.ProgramExecutionPayload(
                "fx", ["--dir", "out/default", "serve"], {}
            )
        )
        self.assertEqual(
            self._create_event_span(payload).category,
            event.EventStatCategory.IGNORE,
        )
