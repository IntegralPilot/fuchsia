#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Library to help integrate power sampling into Fuchsia e2e tests."""

import abc
import dataclasses
import enum
import math
import struct
from typing import Any, BinaryIO, Sequence

# Traces use "ticks" which is a hardware dependent time duration
TICKS_PER_NS = 0.024


@dataclasses.dataclass(frozen=True)
class PowerSamplerConfig:
    # Directory for samples output
    output_dir: str
    # Unique metric name, used in output file names.
    metric_name: str
    # Causes a no-op power sampler to be used.
    disabled: bool = False
    # Path of the measurepower tool (Optional)
    measurepower_path: str | None = None
    # Use hermetic measurepower tool (Optional)
    use_hermetic_measurepower: bool = False


class _PowerSamplerState(enum.Enum):
    INIT = 1
    STARTED = 2
    STOPPED = 3


class PowerSampler:
    """Power sampling base class.

    Usage:
    ```
    sampler:PowerSampler = create_power_sampler(...)
    sampler.start()

    ... gather traces, start and stop the power sampler, create the model ...

    sampler.stop()

    trace_model_with_power =
        power_test_utils.merge_power_data(model, sampler.extract_samples(), outpath)
    processor = PowerMetricsProcessor()
    processor.process_metrics(trace_model_with_power)
    ```
    """

    def __init__(self, config: PowerSamplerConfig):
        """Creates a PowerSampler from a config.

        Args:
            config (PowerSamplerConfig): Configuration.
        """
        self._state: _PowerSamplerState = _PowerSamplerState.INIT
        self._config = config

    def start(self) -> None:
        """Starts sampling."""
        assert self._state == _PowerSamplerState.INIT
        self._state = _PowerSamplerState.STARTED
        self._start_impl()

    def stop(self) -> None:
        """Stops sampling. Has no effect if never started or already stopped."""
        if self._state == _PowerSamplerState.STARTED:
            self._state = _PowerSamplerState.STOPPED
            self._stop_impl()

    def should_generate_load(self) -> bool:
        return False

    def has_samples(self) -> bool:
        return False

    def extract_samples(self) -> Sequence["Sample"]:
        """Return recorded samples. May be expensive and require I/O."""
        return []

    @abc.abstractmethod
    def _stop_impl(self) -> None:
        pass

    @abc.abstractmethod
    def _start_impl(self) -> None:
        pass


class Sample:
    def __init__(
        self,
        timestamp: int | str,
        current: float | str,
        voltage: float | str,
        aux_current: float | str | None = None,
        power: float | str | None = None,
        rail_id: int = 1,
        rail_name: str | None = None,
    ) -> None:
        self.timestamp = int(timestamp)
        self.current = float(current)
        self.voltage = float(voltage)
        self.aux_current = (
            float(aux_current) if aux_current is not None else None
        )
        self.power = (
            float(power) if power is not None else self.current * self.voltage
        )
        self.rail_id = rail_id
        self.rail_name = rail_name

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, Sample):
            raise NotImplementedError()
        return (
            self.timestamp == other.timestamp
            and self.current == other.current
            and self.voltage == other.voltage
            and self.aux_current == other.aux_current
            and self.power == other.power
            and self.rail_id == other.rail_id
            and self.rail_name == other.rail_name
        )

    def __repr__(self) -> str:
        return (
            f"ts: {self.timestamp}ms, current: {self.current}, "
            f"voltage: {self.voltage}, aux: {self.aux_current}, "
            f"power: {self.power}, rail_id: {self.rail_id}, rail_name: {self.rail_name}"
        )


def append_power_data(
    fxt_path: str,
    power_samples: Sequence[Sample],
    starting_ticks: int,
) -> None:
    """
    Append a list of power samples to the trace at `fxt_path`.

    Args:
        fxt_path: the fxt file to write to
        power_samples: the samples to append
        starting_ticks: offset from the beginning of the trace in "ticks"
    """
    print(f"Aligning Power Trace to start at {starting_ticks} ticks")
    with open(fxt_path, "ab") as merged_trace:
        # Virtual koids have a top bit as 1, the remaining doesn't matter as long as it's unique.
        fake_process_koid = 0x8C01_1EC7_EDDA_7A10  # CollectedData10
        fake_thread_koid = 0x8C01_1EC7_EDDA_7A20  # CollectedData20
        fake_thread_ref = 0xFF

        BYTES_PER_WORD = 8

        class InlineString:
            def __init__(self, s: str):
                self.num_words: int = math.ceil(len(s) / BYTES_PER_WORD)

                # Inline fxt ids have their top bit set to 1.
                # The remaining bits indicate the number of inline bytes are valid.
                self.ref: int = 0x8000 | len(s)

                # Pad the word with 0x00 bytes to fill a full multiple of words.
                size = self.num_words * BYTES_PER_WORD
                assert (
                    len(s) <= size
                ), f"String {s} is longer than {size} bytes."
                tail_zeros = "\0" * (size - len(s))
                b = bytes(s + tail_zeros, "utf-8")
                assert (
                    len(b) == size
                ), f"Binary string {b!r} must be {size} bytes."
                self.data: bytes = b

        # See //docs/reference/tracing/trace-format for the below trace format
        def thread_record_header(thread_ref: int) -> int:
            thread_record_type = 3
            thread_record_size_words = 3
            return (
                thread_ref << 16
                | thread_record_size_words << 4
                | thread_record_type
            )

        def kernel_object_record_header(
            num_args: int, name_ref: int, obj_type: int, size_words: int
        ) -> int:
            kernel_object_record_header_type = 7
            return (
                num_args << 40
                | name_ref << 24
                | obj_type << 16
                | size_words << 4
                | kernel_object_record_header_type
            )

        # The a fake process and thread records
        merged_trace.write(
            thread_record_header(fake_thread_ref).to_bytes(8, "little")
        )
        merged_trace.write(fake_process_koid.to_bytes(8, "little"))
        merged_trace.write(fake_thread_koid.to_bytes(8, "little"))

        ZX_OBJ_TYPE_PROCESS = 1
        ZX_OBJ_TYPE_THREAD = 2

        # Name the fake process
        process_name = InlineString("Power Measurements")
        merged_trace.write(
            kernel_object_record_header(
                0,
                process_name.ref,
                ZX_OBJ_TYPE_PROCESS,
                process_name.num_words + 2,  # 1 word header, 1 word koid
            ).to_bytes(8, "little")
        )
        merged_trace.write(fake_process_koid.to_bytes(8, "little"))
        merged_trace.write(process_name.data)

        # Name the fake thread
        thread_name = InlineString("Power Measurements")
        merged_trace.write(
            kernel_object_record_header(
                0,
                thread_name.ref,
                ZX_OBJ_TYPE_THREAD,
                thread_name.num_words + 2,  # 1 word header, 1 word koid
            ).to_bytes(8, "little")
        )
        merged_trace.write(fake_thread_koid.to_bytes(8, "little"))
        merged_trace.write(thread_name.data)

        # Initialization record sets the expected ticks per second.
        init_record_type = 1
        init_record_size = 2
        init_record_header = (init_record_size << 4) | init_record_type
        merged_trace.write(init_record_header.to_bytes(8, "little"))
        merged_trace.write(
            int(TICKS_PER_NS * 1000 * 1000 * 1000).to_bytes(8, "little")
        )

        def counter_event_header(
            name_id: int,
            category_id: int,
            thread_ref: int,
            num_args: int,
            record_words: int,
        ) -> int:
            counter_event_type = 1 << 16
            event_record_type = 4
            return (
                (name_id << 48)
                | (category_id << 32)
                | (thread_ref << 24)
                | (num_args << 20)
                | counter_event_type
                | record_words << 4
                | event_record_type
            )

        # Now write our sample data as counter events into the trace
        for sample in power_samples:
            category = InlineString("Metrics")
            name = InlineString(sample.rail_name or "Metrics")

            # We will be providing either 3 or 4 arguments, depending on whether
            # or not this sample has raw aux current data in it.
            arg_count = 4 if sample.aux_current is not None else 3
            words_per_arg = (
                category.num_words + name.num_words + 1  # 1 word for header.
            )
            arg_words = arg_count * words_per_arg

            # Counter events can store up to 15 args only.
            assert arg_count <= 15

            # Emit the counter track
            merged_trace.write(
                counter_event_header(
                    name.ref,
                    category.ref,
                    0xFF,
                    arg_count,
                    # 1 word counter, 1 word ts,
                    # 2 words inline strings
                    # |arg_words| words of arguments,
                    # 1 word counter id = 5 + |arg_words|
                    5 + arg_words,
                ).to_bytes(8, "little")
            )
            timestamp_ticks = int(
                (sample.timestamp * TICKS_PER_NS) + starting_ticks
            )
            assert (
                timestamp_ticks >= 0
            ), f"timestamp_ticks must be positive: {timestamp_ticks} {sample.__dict__}"
            merged_trace.write(timestamp_ticks.to_bytes(8, "little"))
            merged_trace.write(category.data)
            merged_trace.write(name.data)

            def double_argument_header(name_ref: int, size: int) -> int:
                argument_type = 5
                return name_ref << 16 | size << 4 | argument_type

            def write_double_argument(
                trace_file: BinaryIO, name: str, data: float
            ) -> None:
                # Words are 8 bytes. 2 for header and data. Variable for name.
                name_string = InlineString(name)
                words_total = 2 + name_string.num_words
                trace_file.write(
                    double_argument_header(
                        name_string.ref, words_total
                    ).to_bytes(8, "little")
                )
                trace_file.write(name_string.data)
                s = struct.pack("d", data)
                trace_file.write(s)

            write_double_argument(merged_trace, "Voltage", sample.voltage)
            write_double_argument(merged_trace, "Current", sample.current)
            write_double_argument(merged_trace, "Power", sample.power)

            if sample.aux_current is not None:
                write_double_argument(
                    merged_trace, "Raw Aux", sample.aux_current
                )

            # Write the counter_id, taken as the power rail's ID.
            merged_trace.write(sample.rail_id.to_bytes(8, "little"))
