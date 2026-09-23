from collections.abc import Generator
from typing import Any

from _typeshed import Incomplete
from mobly import base_test as base_test
from mobly import config_parser as config_parser
from mobly import logger as logger
from mobly import records as records
from mobly import signals as signals
from mobly import utils as utils

class Error(Exception): ...

def main(argv: Incomplete | None = ...) -> None: ...
def parse_mobly_cli_args(argv: Any) -> Any: ...

class TestRunner:
    class _TestRunInfo:
        config: Incomplete
        test_class: Incomplete
        test_class_name_suffix: Incomplete
        tests: Incomplete
        def __init__(
            self,
            config: Any,
            test_class: Any,
            tests: Incomplete | None = ...,
            test_class_name_suffix: Incomplete | None = ...,
        ) -> None: ...

    class _TestRunMetaData:
        root_output_path: Incomplete
        def __init__(self, log_dir: Any, testbed_name: Any) -> None: ...
        def generate_test_run_log_path(self) -> Any: ...
        def set_start_point(self) -> None: ...
        def set_end_point(self) -> None: ...
        @property
        def run_id(self) -> Any: ...
        @property
        def time_elapsed_sec(self) -> Any: ...

    results: Incomplete
    def __init__(self, log_dir: Any, testbed_name: Any) -> None: ...
    def mobly_logger(
        self, alias: str = ..., console_level: Any = ...
    ) -> Generator[Incomplete, None, None]: ...
    def add_test_class(
        self,
        config: Any,
        test_class: Any,
        tests: Incomplete | None = ...,
        name_suffix: Incomplete | None = ...,
    ) -> None: ...
    def run(self) -> None: ...
