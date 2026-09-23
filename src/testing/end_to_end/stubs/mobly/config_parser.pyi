from typing import Any
from _typeshed import Incomplete
from mobly import keys as keys
from mobly import utils as utils

ENV_MOBLY_LOGPATH: str

class MoblyConfigError(Exception): ...

def load_test_config_file(
    test_config_path: Any, tb_filters: Incomplete | None = ...
) -> Any: ...

class TestRunConfig:
    log_path: str
    test_bed_name: Incomplete
    testbed_name: Incomplete
    controller_configs: Incomplete
    user_params: Incomplete
    summary_writer: Incomplete
    test_class_name_suffix: Incomplete
    def __init__(self) -> None: ...
    def copy(self) -> Any: ...
