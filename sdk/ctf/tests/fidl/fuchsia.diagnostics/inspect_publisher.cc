// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/inspect/component/cpp/component.h>

// This trivial component writes a single property and node of every possible type in inspect for
// the sake of verifying that the accessor API is stable in CTS.
int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto inspector =
      std::make_unique<inspect::ComponentInspector>(loop.dispatcher(), inspect::PublishOptions{});

  auto numeric_properties = inspector->root().CreateChild("numeric");
  numeric_properties.RecordInt("int", -1);
  numeric_properties.RecordUint("uint", 1);
  numeric_properties.RecordDouble("double", 1.5);
  numeric_properties.RecordBool("bool", true);

  inspector->root().RecordLazyValues("buffers_lazy_values", []() {
    inspect::Inspector inspector;
    auto buffer_properties = inspector.GetRoot().CreateChild("buffers");
    buffer_properties.CreateString("string", "foo", &inspector);
    buffer_properties.CreateByteVector("bytes", std::vector<uint8_t>({1, 2, 3}), &inspector);
    inspector.emplace(std::move(buffer_properties));
    return fpromise::make_ok_promise(std::move(inspector));
  });

  inspector->root().RecordLazyNode("arrays", []() {
    inspect::Inspector inspector;
    auto ints = inspector.GetRoot().CreateIntArray("ints", 2);
    ints.Set(0, -1);
    inspector.emplace(std::move(ints));

    auto uints = inspector.GetRoot().CreateUintArray("uints", 3);
    uints.Set(1, 2);
    inspector.emplace(std::move(uints));

    auto doubles = inspector.GetRoot().CreateDoubleArray("doubles", 4);
    doubles.Set(2, 3.5);
    inspector.emplace(std::move(doubles));

    return fpromise::make_ok_promise(std::move(inspector));
  });

  auto linear_histograms = inspector->root().CreateChild("linear_histgorams");
  auto int_linear_hist = linear_histograms.CreateLinearIntHistogram("int", /*floor*/ -10,
                                                                    /*step_size*/ 2, /*buckets*/ 3);
  int_linear_hist.Insert(-5);

  auto uint_linear_hist = linear_histograms.CreateLinearUintHistogram(
      "uint", /*floor*/ 1, /*step_size*/ 2, /*buckets*/ 3);
  uint_linear_hist.Insert(3);

  auto double_linear_hist = linear_histograms.CreateLinearDoubleHistogram(
      "double", /*floor*/ 1.5, /*step_size*/ 2.5, /*buckets*/ 3);
  double_linear_hist.Insert(4.5);

  auto exp_histograms = inspector->root().CreateChild("exponential_histograms");
  auto int_exp_hist = exp_histograms.CreateExponentialIntHistogram(
      "int", /*floor*/ -10, /*initial_step*/ 2, /*step_multiplier*/ 3, /*buckets*/ 3);
  int_exp_hist.Insert(-5);

  auto uint_exp_hist = exp_histograms.CreateExponentialUintHistogram(
      "uint", /*floor*/ 1, /*initial_step*/ 2, /*step_multiplier*/ 3, /*buckets*/ 3);
  uint_exp_hist.Insert(3);

  auto double_exp_hist = exp_histograms.CreateExponentialDoubleHistogram(
      "double", /*floor*/ 1.5, /*initial_step*/ 2, /*step_multiplier*/ 3.5, /*buckets*/ 3);
  double_exp_hist.Insert(4.5);

  loop.Run();
  return 0;
}
