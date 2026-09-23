import logging
from typing import Any

from _typeshed import Incomplete
from mobly import records as records
from mobly import utils as utils

LINUX_MAX_FILENAME_LENGTH: int
WINDOWS_MAX_FILENAME_LENGTH: int
WINDOWS_RESERVED_CHARACTERS_REPLACEMENTS: Incomplete
WINDOWS_RESERVED_FILENAME_REGEX: Incomplete
WINDOWS_RESERVED_FILENAME_PREFIX: str
log_line_format: str
log_line_time_format: str
log_line_timestamp_len: int
logline_timestamp_re: Incomplete

def is_valid_logline_timestamp(timestamp: Any) -> Any: ...
def logline_timestamp_comparator(t1: Any, t2: Any) -> Any: ...
def epoch_to_log_line_timestamp(
    epoch_time: Any, time_zone: Incomplete | None = ...
) -> Any: ...
def get_log_line_timestamp(delta: Incomplete | None = ...) -> Any: ...
def get_log_file_timestamp(delta: Incomplete | None = ...) -> Any: ...
def kill_test_logger(logger: Any) -> None: ...
def create_latest_log_alias(actual_path: Any, alias: Any) -> None: ...
def setup_test_logger(
    log_path: Any,
    prefix: Incomplete | None = ...,
    alias: str = ...,
    console_level: Any = ...,
) -> None: ...
def sanitize_filename(filename: Any) -> Any: ...
def normalize_log_line_timestamp(log_line_timestamp: Any) -> Any: ...

class PrefixLoggerAdapter(logging.LoggerAdapter[Any]):
    EXTRA_KEY_LOG_PREFIX: str
    _KWARGS_TYPE: Incomplete
    _PROCESS_RETURN_TYPE: Incomplete
    extra: _KWARGS_TYPE
    def process(
        self, msg: str, kwargs: Any
    ) -> Any: ...
