#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from pathlib import Path

import build_summary
import textpb


class LabelsToDictTests(unittest.TestCase):
    def test_one_label(self) -> None:
        self.assertEqual(
            build_summary.labels_to_dict("bar=foo"), {"bar": "foo"}
        )

    def test_two_labels(self) -> None:
        self.assertEqual(
            build_summary.labels_to_dict("baz=1,spice=sugar"),
            {"baz": "1", "spice": "sugar"},
        )

    def test_duplicate_last_wins(self) -> None:
        self.assertEqual(
            build_summary.labels_to_dict("bar=foo,bar=quux"), {"bar": "quux"}
        )


class GetActionCategoryFromLabelsTests(unittest.TestCase):
    def test_cxx(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_from_labels(
                "lang=cpp,type=compile,tool=clang"
            ),
            "cxx",
        )

    def test_link(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_from_labels(
                "type=link,tool=clang"
            ),
            "link",
        )

    def test_rust(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_from_labels(
                "type=tool,toolname=rustc"
            ),
            "rust",
        )

    def test_custom_tool(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_from_labels(
                "type=tool,toolname=protoc"
            ),
            "protoc",
        )

    def test_unknown(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_from_labels("type=tool"), "other"
        )


class GetActionCategoryAndMetricTests(unittest.TestCase):
    def test_metric_for_all(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_and_metric("Foo.Metadata.Time"),
            (None, "Foo.Metadata.Time"),
        )

    def test_metric_for_one_tool(self) -> None:
        self.assertEqual(
            build_summary.get_action_category_and_metric(
                "[toolname=catter].Foo.Metadata.Time"
            ),
            ("catter", "Foo.Metadata.Time"),
        )


RBE_METRICS_TXT_DATA = """
num_records: 47
stats: {
	name: "CompletionStatus"
	counts_by_value: {
		name: "STATUS_NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "STATUS_REMOTE_EXECUTION"
		count: 45
	}
}
stats: {
	name: "LocalMetadata.EventTimes.AtomicOutputOverheadMillis"
	count: 45
	outliers: {
		command_id: "1c2a12e4-3cbc3101"
		value: 1
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 1
	}
	percentile95: 1
	average: 0.08888888888888889
}
stats: {
	name: "LocalMetadata.EventTimes.CPPInputProcessorMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 275
	}
	outliers: {
		command_id: "1c2a12e4-6f7c7849"
		value: 271
	}
	median: 239
	percentile75: 253
	percentile85: 260
	percentile95: 270
	average: 216.70212765957447
}
stats: {
	name: "LocalMetadata.EventTimes.InputProcessorWaitMillis"
	count: 47
}
stats: {
	name: "LocalMetadata.EventTimes.LocalCommandExecutionMillis"
	count: 2
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 10330
	}
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 10047
	}
	median: 10330
	percentile75: 10330
	percentile85: 10330
	percentile95: 10330
	average: 10188.5
}
stats: {
	name: "LocalMetadata.EventTimes.LocalCommandQueuedMillis"
	count: 2
}
stats: {
	name: "LocalMetadata.EventTimes.ProcessInputsMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 307
	}
	outliers: {
		command_id: "1c2a12e4-6f7c7849"
		value: 303
	}
	median: 272
	percentile75: 285
	percentile85: 292
	percentile95: 301
	average: 248.10638297872342
}
stats: {
	name: "LocalMetadata.EventTimes.ProxyExecutionMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 28953
	}
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 26916
	}
	median: 12248
	percentile75: 14475
	percentile85: 15016
	percentile95: 25648
	average: 11568.851063829787
}
stats: {
	name: "LocalMetadata.EventTimes.WrapperOverheadMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-59ee7648"
		value: 61
	}
	outliers: {
		command_id: "1c2a12e4-2acc7a82"
		value: 61
	}
	median: 27
	percentile75: 29
	percentile85: 29
	percentile95: 59
	average: 28.893617021276597
}
stats: {
	name: "LocalMetadata.ExecutedLocally"
	count: 2
}
stats: {
	name: "LocalMetadata.Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
}
stats: {
	name: "RemoteMetadata.EventTimes.CheckActionCacheMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 248
	}
	outliers: {
		command_id: "1c2a12e4-458a7e63"
		value: 194
	}
	median: 39
	percentile75: 45
	percentile85: 49
	percentile95: 88
	average: 47.851063829787236
}
stats: {
	name: "RemoteMetadata.EventTimes.ComputeMerkleTreeMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 893
	}
	outliers: {
		command_id: "1c2a12e4-7447be93"
		value: 869
	}
	median: 796
	percentile75: 822
	percentile85: 836
	percentile95: 855
	average: 805.7446808510638
}
stats: {
	name: "RemoteMetadata.EventTimes.DownloadResultsMillis"
	count: 45
	outliers: {
		command_id: "1c2a12e4-cd0cc4aa"
		value: 143
	}
	outliers: {
		command_id: "1c2a12e4-cc0daf2c"
		value: 132
	}
	median: 49
	percentile75: 93
	percentile85: 109
	percentile95: 129
	average: 64.97777777777777
}
stats: {
	name: "RemoteMetadata.EventTimes.ExecuteRemotelyMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 25567
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 18720
	}
	median: 10934
	percentile75: 12951
	percentile85: 13762
	percentile95: 18123
	average: 9794.68085106383
}
stats: {
	name: "RemoteMetadata.EventTimes.ServerQueuedMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-3c25a847"
		value: 17
	}
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 14
	}
	median: 9
	percentile75: 11
	percentile85: 12
	percentile95: 14
	average: 9.191489361702128
}
stats: {
	name: "RemoteMetadata.EventTimes.ServerWorkerExecutionMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 24872
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 17943
	}
	median: 10297
	percentile75: 12277
	percentile85: 12831
	percentile95: 17062
	average: 9110.404255319148
}
stats: {
	name: "RemoteMetadata.EventTimes.ServerWorkerInputFetchMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-d563e7ab"
		value: 366
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 290
	}
	median: 108
	percentile75: 140
	percentile85: 164
	percentile95: 262
	average: 121.29787234042553
}
stats: {
	name: "RemoteMetadata.EventTimes.ServerWorkerMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 25315
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 18479
	}
	median: 10770
	percentile75: 12694
	percentile85: 13490
	percentile95: 17905
	average: 9553.851063829787
}
stats: {
	name: "RemoteMetadata.EventTimes.ServerWorkerOutputUploadMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cd0cc4aa"
		value: 341
	}
	outliers: {
		command_id: "1c2a12e4-8e33b72c"
		value: 154
	}
	median: 69
	percentile75: 85
	percentile85: 139
	percentile95: 146
	average: 82.91489361702128
}
stats: {
	name: "RemoteMetadata.EventTimes.UploadInputsMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-2acc7a82"
		value: 727
	}
	outliers: {
		command_id: "1c2a12e4-4fcd9f90"
		value: 419
	}
	median: 143
	percentile75: 164
	percentile85: 212
	percentile95: 239
	average: 166.25531914893617
}
stats: {
	name: "RemoteMetadata.TotalOutputBytes"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "RemoteMetadata.LogicalBytesDownloaded"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "RemoteMetadata.LogicalBytesUploaded"
	count: 327048
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 33008
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 33008
	}
	median: 5432
	percentile75: 5749
	percentile85: 5864
	percentile95: 18399
	average: 6958.468085106383
}
stats: {
	name: "RemoteMetadata.NumInputDirectories"
	count: 12952
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 372
	}
	outliers: {
		command_id: "1c2a12e4-f47e9b45"
		value: 362
	}
	median: 289
	percentile75: 320
	percentile85: 324
	percentile95: 345
	average: 275.5744680851064
}
stats: {
	name: "RemoteMetadata.NumInputFiles"
	count: 44646
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 1240
	}
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 1235
	}
	median: 1029
	percentile75: 1082
	percentile85: 1145
	percentile95: 1226
	average: 949.9148936170212
}
stats: {
	name: "RemoteMetadata.NumOutputFiles"
	count: 92
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 2
	}
	outliers: {
		command_id: "1c2a12e4-a85b51f3"
		value: 2
	}
	median: 2
	percentile75: 2
	percentile85: 2
	percentile95: 2
	average: 1.9574468085106382
}
stats: {
	name: "RemoteMetadata.RealBytesDownloaded"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "RemoteMetadata.RealBytesUploaded"
	count: 327048
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 33008
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 33008
	}
	median: 5432
	percentile75: 5749
	percentile85: 5864
	percentile95: 18399
	average: 6958.468085106383
}
stats: {
	name: "RemoteMetadata.Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "SUCCESS"
		count: 45
	}
}
stats: {
	name: "RemoteMetadata.TotalInputBytes"
	count: 8443562883
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 192203614
	}
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 190111791
	}
	median: 181259894
	percentile75: 182335622
	percentile85: 184712342
	percentile95: 187618652
	average: 1.7965027410638297e+08
}
stats: {
	name: "RemoteMetadata.TotalOutputBytes"
	count: 35777071
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 761214.2765957447
}
stats: {
	name: "Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "SUCCESS"
		count: 45
	}
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].CompletionStatus"
	counts_by_value: {
		name: "STATUS_NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "STATUS_REMOTE_EXECUTION"
		count: 45
	}
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.AtomicOutputOverheadMillis"
	count: 45
	outliers: {
		command_id: "1c2a12e4-3cbc3101"
		value: 1
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 1
	}
	percentile95: 1
	average: 0.08888888888888889
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.CPPInputProcessorMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 275
	}
	outliers: {
		command_id: "1c2a12e4-6f7c7849"
		value: 271
	}
	median: 239
	percentile75: 253
	percentile85: 260
	percentile95: 270
	average: 216.70212765957447
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.InputProcessorWaitMillis"
	count: 47
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.LocalCommandExecutionMillis"
	count: 2
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 10330
	}
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 10047
	}
	median: 10330
	percentile75: 10330
	percentile85: 10330
	percentile95: 10330
	average: 10188.5
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.LocalCommandQueuedMillis"
	count: 2
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.ProcessInputsMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 307
	}
	outliers: {
		command_id: "1c2a12e4-6f7c7849"
		value: 303
	}
	median: 272
	percentile75: 285
	percentile85: 292
	percentile95: 301
	average: 248.10638297872342
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.ProxyExecutionMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 28953
	}
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 26916
	}
	median: 12248
	percentile75: 14475
	percentile85: 15016
	percentile95: 25648
	average: 11568.851063829787
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.EventTimes.WrapperOverheadMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-59ee7648"
		value: 61
	}
	outliers: {
		command_id: "1c2a12e4-2acc7a82"
		value: 61
	}
	median: 27
	percentile75: 29
	percentile85: 29
	percentile95: 59
	average: 28.893617021276597
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.ExecutedLocally"
	count: 2
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].LocalMetadata.Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.CheckActionCacheMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 248
	}
	outliers: {
		command_id: "1c2a12e4-458a7e63"
		value: 194
	}
	median: 39
	percentile75: 45
	percentile85: 49
	percentile95: 88
	average: 47.851063829787236
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ComputeMerkleTreeMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 893
	}
	outliers: {
		command_id: "1c2a12e4-7447be93"
		value: 869
	}
	median: 796
	percentile75: 822
	percentile85: 836
	percentile95: 855
	average: 805.7446808510638
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.DownloadResultsMillis"
	count: 45
	outliers: {
		command_id: "1c2a12e4-cd0cc4aa"
		value: 143
	}
	outliers: {
		command_id: "1c2a12e4-cc0daf2c"
		value: 132
	}
	median: 49
	percentile75: 93
	percentile85: 109
	percentile95: 129
	average: 64.97777777777777
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ExecuteRemotelyMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 25567
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 18720
	}
	median: 10934
	percentile75: 12951
	percentile85: 13762
	percentile95: 18123
	average: 9794.68085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ServerQueuedMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-3c25a847"
		value: 17
	}
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 14
	}
	median: 9
	percentile75: 11
	percentile85: 12
	percentile95: 14
	average: 9.191489361702128
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ServerWorkerExecutionMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 24872
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 17943
	}
	median: 10297
	percentile75: 12277
	percentile85: 12831
	percentile95: 17062
	average: 9110.404255319148
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ServerWorkerInputFetchMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-d563e7ab"
		value: 366
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 290
	}
	median: 108
	percentile75: 140
	percentile85: 164
	percentile95: 262
	average: 121.29787234042553
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ServerWorkerMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 25315
	}
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 18479
	}
	median: 10770
	percentile75: 12694
	percentile85: 13490
	percentile95: 17905
	average: 9553.851063829787
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.ServerWorkerOutputUploadMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-cd0cc4aa"
		value: 341
	}
	outliers: {
		command_id: "1c2a12e4-8e33b72c"
		value: 154
	}
	median: 69
	percentile75: 85
	percentile85: 139
	percentile95: 146
	average: 82.91489361702128
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.EventTimes.UploadInputsMillis"
	count: 47
	outliers: {
		command_id: "1c2a12e4-2acc7a82"
		value: 727
	}
	outliers: {
		command_id: "1c2a12e4-4fcd9f90"
		value: 419
	}
	median: 143
	percentile75: 164
	percentile85: 212
	percentile95: 239
	average: 166.25531914893617
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.TotalOutputBytes"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.LogicalBytesDownloaded"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.LogicalBytesUploaded"
	count: 327048
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 33008
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 33008
	}
	median: 5432
	percentile75: 5749
	percentile85: 5864
	percentile95: 18399
	average: 6958.468085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.NumInputDirectories"
	count: 12952
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 372
	}
	outliers: {
		command_id: "1c2a12e4-f47e9b45"
		value: 362
	}
	median: 289
	percentile75: 320
	percentile85: 324
	percentile95: 345
	average: 275.5744680851064
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.NumInputFiles"
	count: 44646
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 1240
	}
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 1235
	}
	median: 1029
	percentile75: 1082
	percentile85: 1145
	percentile95: 1226
	average: 949.9148936170212
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.NumOutputFiles"
	count: 92
	outliers: {
		command_id: "1c2a12e4-1fdf7bca"
		value: 2
	}
	outliers: {
		command_id: "1c2a12e4-a85b51f3"
		value: 2
	}
	median: 2
	percentile75: 2
	percentile85: 2
	percentile95: 2
	average: 1.9574468085106382
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.RealBytesDownloaded"
	count: 35608332
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 757624.085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.RealBytesUploaded"
	count: 327048
	outliers: {
		command_id: "1c2a12e4-8df6e4b4"
		value: 33008
	}
	outliers: {
		command_id: "1c2a12e4-ec7fb7e0"
		value: 33008
	}
	median: 5432
	percentile75: 5749
	percentile85: 5864
	percentile95: 18399
	average: 6958.468085106383
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "SUCCESS"
		count: 45
	}
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.TotalInputBytes"
	count: 8443562883
	outliers: {
		command_id: "1c2a12e4-5fc72963"
		value: 192203614
	}
	outliers: {
		command_id: "1c2a12e4-cfde6b4a"
		value: 190111791
	}
	median: 181259894
	percentile75: 182335622
	percentile85: 184712342
	percentile95: 187618652
	average: 1.7965027410638297e+08
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].RemoteMetadata.TotalOutputBytes"
	count: 35777071
	outliers: {
		command_id: "1c2a12e4-d56ac563"
		value: 2633657
	}
	outliers: {
		command_id: "1c2a12e4-eb41eb70"
		value: 2395373
	}
	median: 657753
	percentile75: 1051777
	percentile85: 1264361
	percentile95: 1786261
	average: 761214.2765957447
}
stats: {
	name: "[compiler=clang,lang=cpp,type=compile].Result.Status"
	counts_by_value: {
		name: "NON_ZERO_EXIT"
		count: 2
	}
	counts_by_value: {
		name: "SUCCESS"
		count: 45
	}
}
tool_version: "0.141.1.29a9d3c"
invocation_ids: "user:9cef0a51-1b70-4552-826e-1a13f9308359"
machine_info: {
	num_cpu: 96
	ram_mbs: 181310
	os_family: "linux"
	arch: "amd64"
}
build_latency: 28
"""

RBE_METRICS_TXT_LINES = RBE_METRICS_TXT_DATA.split("\n")


class BuildSummaryTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self.parsed_input = textpb.parse(RBE_METRICS_TXT_LINES)


class LoadRbeMetricsTest(BuildSummaryTestBase):
    def test_no_stats(self) -> None:
        rbe_data = build_summary.load_rbe_metrics({})  # empty file
        self.assertEqual(rbe_data.status_metrics, {})
        self.assertEqual(rbe_data.bandwidth_metrics, {})

    def test_load(self) -> None:
        rbe_data = build_summary.load_rbe_metrics(self.parsed_input)
        self.assertEqual(
            rbe_data.status_metrics["CompletionStatus"]["cxx"][
                "STATUS_NON_ZERO_EXIT"
            ],
            2,
        )
        self.assertEqual(
            rbe_data.bandwidth_metrics["RemoteMetadata.RealBytesDownloaded"][
                "cxx"
            ],
            35608332,
        )


class PrepareSummaryTableTest(BuildSummaryTestBase):
    def test_empty(self) -> None:
        # Test for empty metrics.
        rbe_data = build_summary.load_rbe_metrics({})
        table = build_summary.prepare_summary_table(rbe_data)
        found_status_total = False
        for row in table:
            if row[0] == "total":  # under CompletionStatus
                self.assertEqual(row[1], 0)
                found_status_total = True

        self.assertTrue(found_status_total)

    def test_prepare(self) -> None:
        rbe_data = build_summary.load_rbe_metrics(self.parsed_input)
        table = build_summary.prepare_summary_table(rbe_data)
        found_bytes_downloaded = False
        for row in table:
            if row[0] == "BytesDownloaded":
                self.assertEqual(row[1], "34.0 MiB")  # formatted
                found_bytes_downloaded = True

        self.assertTrue(found_bytes_downloaded)


EXPECTED_BUILD_SUMMARY = """=== Remote build summary (from: build/rbe/build_summary.py reproxy_logdir)
[by action type]                cxx       all
CompletionStatus
  STATUS_NON_ZERO_EXIT            2         2
  STATUS_REMOTE_EXECUTION        45        45
total                            47        47

OutputBytes                34.1 MiB  34.1 MiB
BytesDownloaded            34.0 MiB  34.0 MiB
BytesUploaded             319.4 KiB 319.4 KiB
"""


class PrintBuildSummaryTest(BuildSummaryTestBase):
    def test_build_summary(self) -> None:
        rbe_data = build_summary.load_rbe_metrics(self.parsed_input)
        # Ignore the === line because {script_rel} is different in .pyz.
        self.assertEqual(
            [
                line.strip()
                for line in build_summary.build_summary_lines(
                    rbe_data, "reproxy_logdir"
                )
            ][1:],
            [line.strip() for line in EXPECTED_BUILD_SUMMARY.splitlines()][1:],
        )


class ArrangeMetricsJsonTest(BuildSummaryTestBase):
    def test_metrics(self) -> None:
        rbe_data = build_summary.load_rbe_metrics(self.parsed_input)
        json_data = build_summary.arrange_metrics_json(rbe_data)
        self.assertEqual(
            json_data,
            {
                "execution_statuses": {
                    "all": {
                        "STATUS_NON_ZERO_EXIT": 2,
                        "STATUS_REMOTE_EXECUTION": 45,
                    },
                    "cxx": {
                        "STATUS_NON_ZERO_EXIT": 2,
                        "STATUS_REMOTE_EXECUTION": 45,
                    },
                },
                "data_sizes_bytes": {
                    "TotalOutputBytes": {"all": 35777071, "cxx": 35777071},
                    "LogicalBytesDownloaded": {
                        "all": 35608332,
                        "cxx": 35608332,
                    },
                    "LogicalBytesUploaded": {"all": 327048, "cxx": 327048},
                    "RealBytesDownloaded": {"all": 35608332, "cxx": 35608332},
                    "RealBytesUploaded": {"all": 327048, "cxx": 327048},
                },
            },
        )


class MainArgsParsingTest(unittest.TestCase):
    def test_logdir(self) -> None:
        log_dir = "/tmp/foo/bar/reproxy.logs"
        args = build_summary._MAIN_ARG_PARSER.parse_args([log_dir])
        self.assertEqual(args.reproxy_logdir, Path(log_dir))

    def test_format_json(self) -> None:
        log_dir = "/tmp/foo/bar/reproxy.logs"
        args = build_summary._MAIN_ARG_PARSER.parse_args(
            ["--format=json", log_dir]
        )
        self.assertEqual(args.format, "json")


if __name__ == "__main__":
    unittest.main()
