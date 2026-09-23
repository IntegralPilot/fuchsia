# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from typing import Any

from fuchsia_async_extension import AsyncBaseTestClass, get_loop, retry
from mobly import base_test, config_parser, records, signals


class FakeWriter:
    def dump(self, *args: Any, **kwargs: Any) -> None:
        pass


class StubTest(AsyncBaseTestClass):
    def __init__(self, controllers: dict[str, Any] | None = None) -> None:
        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"
        if controllers:
            fake_config.controller_configs = controllers
        super().__init__(fake_config)
        self.call_order: list[str] = []
        self.current_test_info = records.TestResultRecord(
            "test_fake", "fake_class"
        )

    async def pre_run(self) -> None:
        self.call_order.append("pre_run_async")

    async def setup_class(self) -> None:
        self.call_order.append("setup_class_async")

    async def teardown_class(self) -> None:
        self.call_order.append("teardown_class_async")

    async def setup_test(self) -> None:
        self.call_order.append("setup_test_async")

    async def teardown_test(self) -> None:
        self.call_order.append("teardown_test_async")

    async def on_fail(self, record: Any) -> None:
        self.call_order.append("on_fail_async")

    async def on_pass(self, record: Any) -> None:
        self.call_order.append("on_pass_async")

    async def on_skip(self, record: Any) -> None:
        self.call_order.append("on_skip_async")

    async def test_fake(self) -> None:
        self.call_order.append("test_fake_async")


class FakeSubclassTest(StubTest):
    async def setup_test(self) -> None:
        self.call_order.append("subclass_setup_test_before")
        await super().setup_test()
        self.call_order.append("subclass_setup_test_after")


class FakeSetupClassSubclassTest(StubTest):
    async def setup_class(self) -> None:
        self.call_order.append("subclass_setup_class_before")
        await super().setup_class()
        self.call_order.append("subclass_setup_class_after")


class FuchsiaAsyncExtensionTest(unittest.TestCase):
    def test_lifecycle_methods_are_synchronous(self) -> None:
        test_obj = StubTest()

        # Test Mobly framework calling them synchronously
        test_obj.pre_run()  # type: ignore[unused-coroutine]
        self.assertIn("pre_run_async", test_obj.call_order)

        test_obj.setup_class()  # type: ignore[unused-coroutine]
        self.assertIn("setup_class_async", test_obj.call_order)

        test_obj.setup_test()  # type: ignore[unused-coroutine]
        self.assertIn("setup_test_async", test_obj.call_order)

        test_obj.teardown_test()  # type: ignore[unused-coroutine]
        self.assertIn("teardown_test_async", test_obj.call_order)

        test_obj.teardown_class()  # type: ignore[unused-coroutine]
        self.assertIn("teardown_class_async", test_obj.call_order)

        test_obj.on_fail(None)  # type: ignore[unused-coroutine]
        self.assertIn("on_fail_async", test_obj.call_order)

        test_obj.on_pass(None)  # type: ignore[unused-coroutine]
        self.assertIn("on_pass_async", test_obj.call_order)

        test_obj.on_skip(None)  # type: ignore[unused-coroutine]
        self.assertIn("on_skip_async", test_obj.call_order)

    def test_test_methods_are_synchronous(self) -> None:
        test_obj = StubTest()

        test_obj.test_fake()  # type: ignore[unused-coroutine]
        self.assertIn("test_fake_async", test_obj.call_order)

    def test_super_calls_work_inside_async_context(self) -> None:
        test_obj = FakeSubclassTest()

        test_obj.setup_test()  # type: ignore[unused-coroutine]

        self.assertEqual(
            test_obj.call_order,
            [
                "subclass_setup_test_before",
                "setup_test_async",
                "subclass_setup_test_after",
            ],
        )

    def test_super_setup_class_calls_work_inside_async_context(self) -> None:
        test_obj = FakeSetupClassSubclassTest()

        test_obj.setup_class()  # type: ignore[unused-coroutine]

        self.assertEqual(
            test_obj.call_order,
            [
                "subclass_setup_class_before",
                "setup_class_async",
                "subclass_setup_class_after",
            ],
        )

    def test_retry_sync(self) -> None:
        @retry(count=3)
        def my_sync_test() -> str:
            return "sync_ok"

        self.assertEqual(my_sync_test(), "sync_ok")
        self.assertEqual(getattr(my_sync_test, base_test.ATTR_MAX_RETRY_CNT), 3)

    def test_retry_async(self) -> None:
        @retry(count=3)
        async def my_async_test() -> str:
            return "async_ok"

        self.assertEqual(
            getattr(my_async_test, base_test.ATTR_MAX_RETRY_CNT), 3
        )

        res = get_loop().run_until_complete(my_async_test())
        self.assertEqual(res, "async_ok")

    def test_generate_tests_sync(self) -> None:
        call_order: list[str] = []

        class SyncGenerateTest(AsyncBaseTestClass):
            async def pre_run(self) -> None:
                def sync_logic(arg: str) -> None:
                    call_order.append(f"sync_logic_{arg}")

                self.generate_tests(
                    sync_logic, lambda arg: f"test_sync_{arg}", [("a",), ("b",)]
                )

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"

        test_obj = SyncGenerateTest(fake_config)
        test_obj.summary_writer = FakeWriter()
        test_obj.run()

        self.assertIn("sync_logic_a", call_order)
        self.assertIn("sync_logic_b", call_order)

    def test_generate_tests_async(self) -> None:
        call_order: list[str] = []

        class AsyncGenerateTest(AsyncBaseTestClass):
            async def pre_run(self) -> None:
                async def async_logic(arg: str) -> None:
                    call_order.append(f"async_logic_{arg}")

                self.generate_tests(
                    async_logic,
                    lambda arg: f"test_async_{arg}",
                    [("a",), ("b",)],
                )

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"

        test_obj = AsyncGenerateTest(fake_config)
        test_obj.summary_writer = FakeWriter()
        test_obj.run()

        self.assertIn("async_logic_a", call_order)
        self.assertIn("async_logic_b", call_order)

    def test_register_controller_sync(self) -> None:
        call_order: list[str] = []

        class FakeSyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeSync"

            @classmethod
            def create(cls, configs: Any) -> list[str]:
                call_order.append("sync_create")
                return ["obj1"]

            @classmethod
            def destroy(cls, objects: Any) -> None:
                call_order.append("sync_destroy")

            @classmethod
            def get_info(cls, objects: Any) -> list[dict[str, str]]:
                call_order.append("sync_info")
                return [{"info": "data"}]

        class SyncControllerTest(AsyncBaseTestClass):
            async def setup_class(self) -> None:
                await self.register_controller(FakeSyncController)

            def test_empty(self) -> None:
                pass

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"
        fake_config.controller_configs = {"FakeSync": [{}]}

        test_obj = SyncControllerTest(fake_config)
        test_obj.summary_writer = FakeWriter()
        test_obj.run()

        self.assertIn("sync_create", call_order)
        self.assertIn("sync_destroy", call_order)

    def test_register_controller_async(self) -> None:
        call_order: list[str] = []

        class FakeAsyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeAsync"

            @classmethod
            async def create(cls, configs: Any) -> list[Any]:
                call_order.append("async_create")
                return ["obj1"]

            @classmethod
            async def destroy(cls, objects: Any) -> None:
                call_order.append("async_destroy")

            @classmethod
            async def get_info(cls, objects: Any) -> list[dict[str, str]]:
                call_order.append("async_info")
                return [{"info": "data"}]

        class AsyncControllerTest(AsyncBaseTestClass):
            async def setup_class(self) -> None:
                await self.register_controller(FakeAsyncController)

            def test_empty(self) -> None:
                pass

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"
        fake_config.controller_configs = {"FakeAsync": [{}]}

        test_obj = AsyncControllerTest(fake_config)
        test_obj.summary_writer = FakeWriter()
        test_obj.run()

        self.assertIn("async_create", call_order)
        self.assertIn("async_destroy", call_order)

    def test_mobly_register_controller_missing_config(self) -> None:
        class FakeSyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeSync"

            @classmethod
            def create(cls, configs: Any) -> list[str]:
                return ["obj1"]

            @classmethod
            def destroy(cls, objects: Any) -> None:
                pass

        class MoblyTest(base_test.BaseTestClass):
            def setup_class(self) -> None:
                self.register_controller(FakeSyncController, required=True)

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"

        test_obj = MoblyTest(fake_config)
        test_obj.summary_writer = FakeWriter()

        with self.assertRaises(signals.ControllerError):
            test_obj.setup_class()

    def test_async_register_controller_missing_config(self) -> None:
        class FakeAsyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeAsync"

            @classmethod
            async def create(cls, configs: Any) -> list[str]:
                return ["obj1"]

            @classmethod
            async def destroy(cls, objects: Any) -> None:
                pass

        class AsyncTest(AsyncBaseTestClass):
            async def setup_class(self) -> None:
                await self.register_controller(
                    FakeAsyncController, required=True
                )

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"

        test_obj = AsyncTest(fake_config)
        test_obj.summary_writer = FakeWriter()

        with self.assertRaises(signals.ControllerError):
            test_obj.setup_class()  # type: ignore[unused-coroutine]

    def test_mobly_register_controller_min_number(self) -> None:
        class FakeSyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeSync"

            @classmethod
            def create(cls, configs: Any) -> list[str]:
                return ["obj1"]

            @classmethod
            def destroy(cls, objects: Any) -> None:
                pass

        class MoblyTest(base_test.BaseTestClass):
            def setup_class(self) -> None:
                self.register_controller(FakeSyncController, min_number=2)

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"
        fake_config.controller_configs = {"FakeSync": [{}]}

        test_obj = MoblyTest(fake_config)
        test_obj.summary_writer = FakeWriter()

        with self.assertRaises(signals.ControllerError):
            test_obj.setup_class()

    def test_async_register_controller_min_number(self) -> None:
        class FakeAsyncController:
            MOBLY_CONTROLLER_CONFIG_NAME = "FakeAsync"

            @classmethod
            async def create(cls, configs: Any) -> list[str]:
                return ["obj1"]

            @classmethod
            async def destroy(cls, objects: Any) -> None:
                pass

        class AsyncTest(AsyncBaseTestClass):
            async def setup_class(self) -> None:
                await self.register_controller(
                    FakeAsyncController, min_number=2
                )

        fake_config = config_parser.TestRunConfig()
        fake_config.testbed_name = "FakeTestbed"
        fake_config.log_path = "/tmp"
        fake_config.controller_configs = {"FakeAsync": [{}]}

        test_obj = AsyncTest(fake_config)
        test_obj.summary_writer = FakeWriter()

        with self.assertRaises(signals.ControllerError):
            test_obj.setup_class()  # type: ignore[unused-coroutine]


if __name__ == "__main__":
    unittest.main()
