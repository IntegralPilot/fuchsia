# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
NDK_ROOT?=$(HOME)/android-ndk-r29
CLANG?=$(NDK_ROOT)/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi35-clang++
SYSROOT?=$(NDK_ROOT)/toolchains/llvm/prebuilt/linux-x86_64/sysroot
FUCHSIA_ROOT=../../../../..
DEFINES=\
	-DVULKAN_HPP_NO_EXCEPTIONS=1 \
	-DVULKAN_HPP_NO_SPACESHIP_OPERATOR=1
INCLUDES=\
	-I$(FUCHSIA_ROOT) \
	-I$(FUCHSIA_ROOT)/third_party/googletest/src/googletest/include \
	-I$(FUCHSIA_ROOT)/third_party/googletest/src/googletest \
	-I$(FUCHSIA_ROOT)/third_party/Vulkan-Headers/src/include
SOURCES=\
	$(FUCHSIA_ROOT)/src/graphics/tests/vkreadback/android/main.cc \
	$(FUCHSIA_ROOT)/src/graphics/tests/vkreadback/*.cc \
	$(FUCHSIA_ROOT)/src/graphics/tests/common/vulkan_context.cc \
	$(FUCHSIA_ROOT)/src/graphics/tests/common/utils.cc \
	$(FUCHSIA_ROOT)/third_party/googletest/src/googletest/src/gtest-all.cc
LIBS=-lvulkan

vkreadback: $(SOURCES) Makefile
	$(CLANG) --sysroot=$(SYSROOT) -static-libstdc++ -std=c++20 -o vkreadback $(SOURCES) $(INCLUDES) $(DEFINES) $(LIBS)

install: vkreadback
	adb push vkreadback /storage && adb shell chmod +x /storage/vkreadback
