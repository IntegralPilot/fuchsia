// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_UNITTEST_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_UNITTEST_H_

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>

#include <vector>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

namespace fhasp = fuchsia_hardware_audio_signalprocessing;

// Element ids
constexpr ElementId kAgcElementId = 2;
constexpr ElementId kDaiInterconnectElementId = 1;
constexpr ElementId kOtherDaiInterconnectElementId = 111;
constexpr ElementId kRingBufferElementId = 0;
constexpr ElementId kPacketStreamElementId = 441;
constexpr ElementId kDynamicsElementId = 42;
constexpr ElementId kEqualizerElementId = 55;
constexpr ElementId kGainElementId = 555;
constexpr ElementId kVendorSpecificElementId = 10164;
constexpr ElementId kBadElementId = 68;

// Elements
//
// Compliant elements
const fhasp::Element kAgcElement{{
    .id = kAgcElementId,
    .type = fhasp::ElementType::kAutomaticGainControl,
    // .type_specific is missing - not applicable to AGC
    .description = std::string("Test signalprocessing description for an automatic-gain-control ") +
                   std::string("element with expected functionality.  As intentionally defined, ") +
                   std::string("this description is a maximal-length 256-char string. Note that ") +
                   std::string("this string has an upper-case 'x' as the last character.  54321X"),
    .can_stop = false,
    // .can_bypass is missing (cannot be bypassed)
}};
const fhasp::Element kDaiInterconnectElement{{
    .id = kDaiInterconnectElementId,
    .type = fhasp::ElementType::kDaiInterconnect,
    .type_specific = fhasp::TypeSpecificElement::WithDaiInterconnect({{
        .plug_detect_capabilities = fhasp::PlugDetectCapabilities::kCanAsyncNotify,
    }}),
    .description = " ",
    .can_stop = true,
    .can_bypass = false,
}};
const fhasp::Element kRingBufferElement{{
    .id = kRingBufferElementId,  //
    .type = fhasp::ElementType::kRingBuffer,
    // .type_specific is missing - not applicable to RingBuffer
    // .description is missing - not tested
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)
}};
const fhasp::Element kPacketStreamElement{{
    .id = kPacketStreamElementId,  //
    .type = fhasp::ElementType::kPacketStream,
    // .type_specific is missing - not applicable to PacketStream
    // .description is missing - not tested
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)
}};
const fhasp::Element kDynamicsElement{{
    .id = kDynamicsElementId,
    .type = fhasp::ElementType::kDynamics,
    .type_specific = fhasp::TypeSpecificElement::WithDynamics({{
        .bands = {{
            {{
                1,
            }},
            {{
                2,
            }},
        }},
        .supported_controls = fhasp::DynamicsSupportedControls::kKneeWidth |
                              fhasp::DynamicsSupportedControls::kAttack |
                              fhasp::DynamicsSupportedControls::kRelease |
                              fhasp::DynamicsSupportedControls::kOutputGain |
                              fhasp::DynamicsSupportedControls::kInputGain |
                              fhasp::DynamicsSupportedControls::kLookahead |
                              fhasp::DynamicsSupportedControls::kLevelType |
                              fhasp::DynamicsSupportedControls::kLinkedChannels |
                              fhasp::DynamicsSupportedControls::kThresholdType,
    }}),
    .description = "Generic Dynamics element with all capabilities",
    .can_bypass = true,
}};
const fhasp::Element kEqualizerElement{{
    .id = kEqualizerElementId,
    .type = fhasp::ElementType::kEqualizer,
    .type_specific = fhasp::TypeSpecificElement::WithEqualizer({{
        .bands = {{
            {{
                1,
            }},
            {{
                2,
            }},
            {{
                3,
            }},
            {{
                4,
            }},
            {{
                5,
            }},
            {{
                6,
            }},
        }},
        .supported_controls = fhasp::EqualizerSupportedControls::kCanControlFrequency |
                              fhasp::EqualizerSupportedControls::kCanControlQ |
                              fhasp::EqualizerSupportedControls::kSupportsTypePeak |
                              fhasp::EqualizerSupportedControls::kSupportsTypeNotch |
                              fhasp::EqualizerSupportedControls::kSupportsTypeLowCut |
                              fhasp::EqualizerSupportedControls::kSupportsTypeHighCut |
                              fhasp::EqualizerSupportedControls::kSupportsTypeLowShelf |
                              fhasp::EqualizerSupportedControls::kSupportsTypeHighShelf,
        .can_disable_bands = true,
        .min_frequency = 0,
        .max_frequency = 24000,
        .max_q = 1000.0f,
        .min_gain_db = -60.0f,
        .max_gain_db = 60.0f,
    }}),
    .description = "Generic Equalizer element with all capabilities",
    .can_stop = true,
    .can_bypass = true,
}};
const fhasp::Element kGainElement{{
    .id = kGainElementId,
    .type = fhasp::ElementType::kGain,
    .type_specific = fhasp::TypeSpecificElement::WithGain({{
        .type = fhasp::GainType::kDecibels,
        .domain = fhasp::GainDomain::kDigital,
        .min_gain = -24.0f,
        .max_gain = 24.0f,
        .min_gain_step = 0.0f,
    }}),
    .description = "Generic Gain element with all capabilities",
    .can_stop = true,
    .can_bypass = true,
}};
const fhasp::Element kVendorSpecificElement{{
    .id = kVendorSpecificElementId,
    .type = fhasp::ElementType::kVendorSpecific,
    .type_specific = fhasp::TypeSpecificElement::WithVendorSpecific({}),
    .description = "Generic VendorSpecific element",
    .can_stop = true,
    .can_bypass = true,
}};
const fhasp::Element kOtherDaiInterconnectElement{{
    .id = kOtherDaiInterconnectElementId,
    .type = fhasp::ElementType::kDaiInterconnect,
    .type_specific = fhasp::TypeSpecificElement::WithDaiInterconnect({{
        .plug_detect_capabilities = fhasp::PlugDetectCapabilities::kHardwired,
    }}),
    .description = "Standalone DAI (with edgepair loop)",
    .can_stop = false,
    .can_bypass = false,
}};

// Non-compliant elements
const fhasp::Element kElementNoId{{
    // .id is missing - this is non-compliant

    .type = fhasp::ElementType::kAutomaticGainControl,
    // .type_specific is missing - not applicable to AGC
    // .description is missing - allowed
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)
}};
const fhasp::Element kElementNoType{{
    // .type is missing - this is non-compliant

    .id = kBadElementId,
    // .type_specific is missing - not applicable, depending on type
    // .description is missing - allowed
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)
}};
const fhasp::Element kElementWithoutRequiredTypeSpecific{{
    .id = kBadElementId,
    .type = fhasp::ElementType::kDaiInterconnect,
    // .description is missing - allowed
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)

    // .type_specific is missing - non-compliant (required for DAI)
}};
const fhasp::Element kElementWrongTypeSpecific{{
    .id = kBadElementId,
    .type = fhasp::ElementType::kDynamics,
    // .description is missing - allowed
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)

    .type_specific = fhasp::TypeSpecificElement::WithDaiInterconnect({{
        .plug_detect_capabilities = fhasp::PlugDetectCapabilities::kCanAsyncNotify,
    }}),  // .type_specific does not match .type (Dynamics)
}};
const fhasp::Element kElementEmptyDescription{{
    .id = kBadElementId,
    .type = fhasp::ElementType::kAutomaticGainControl,
    // .type_specific is missing - not applicable to AGC
    // .can_stop is missing (always started)
    // .can_bypass is missing (cannot be bypassed)

    .description = "",  // .description cannot be empty (should instead be omitted)
}};
const fhasp::Element kElementCannotStop{{
    .id = kBadElementId,
    .type = fhasp::ElementType::kAutomaticGainControl,
    // .type_specific is missing - not applicable, depending on type
    // .description is missing - allowed
    // .can_bypass is missing (cannot be bypassed)

    .can_stop = false,  // valid, but later we will try to tell the element to stop
}};
const fhasp::Element kElementCannotBypass{{
    .id = kBadElementId,
    .type = fhasp::ElementType::kAutomaticGainControl,
    // .type_specific is missing - not applicable, depending on type
    // .description is missing - allowed
    // .can_stop is missing (always started)

    .can_bypass = false,  // valid, but later we will try to tell the element to bypass
}};

// Collections of Elements
//
// Compliant collections of elements
const std::vector<fhasp::Element> kElements{
    kDaiInterconnectElement, kAgcElement,          kDynamicsElement,
    kRingBufferElement,      kPacketStreamElement,
};
const std::vector<fhasp::Element> kElementsWithLoopDai{
    kDaiInterconnectElement,      kAgcElement, kDynamicsElement, kRingBufferElement,
    kOtherDaiInterconnectElement,
};
const std::vector<fhasp::Element> kElementsWithPacketStream{
    kDaiInterconnectElement,
    kPacketStreamElement,
};

// Non-compliant collections of elements
const std::vector<fhasp::Element> kEmptyElements{};
const std::vector<fhasp::Element> kElementsDuplicateId{
    kDaiInterconnectElement,
    kDaiInterconnectElement,
};
const std::vector<fhasp::Element> kElementsWithNoId{
    kDaiInterconnectElement, kAgcElement, kDynamicsElement, kRingBufferElement, kElementNoId,
};
const std::vector<fhasp::Element> kElementsWithNoType{
    kDaiInterconnectElement, kAgcElement, kDynamicsElement, kRingBufferElement, kElementNoType,
};
const std::vector<fhasp::Element> kElementsWithNoRequiredTypeSpecific{
    kDaiInterconnectElement,
    kAgcElement,
    kDynamicsElement,
    kRingBufferElement,
    kElementWithoutRequiredTypeSpecific,
};
const std::vector<fhasp::Element> kElementsWithWrongTypeSpecific{
    kDaiInterconnectElement,   kAgcElement, kDynamicsElement, kRingBufferElement,
    kElementWrongTypeSpecific,
};
const std::vector<fhasp::Element> kElementsWithEmptyDescription{
    kDaiInterconnectElement,  kAgcElement, kDynamicsElement, kRingBufferElement,
    kElementEmptyDescription,
};

const std::unordered_map<ElementId, ElementRecord> kEmptyElementMap{};

// ElementStates
const fhasp::ElementState kGenericElementState{{
    // .type_specific is unspecified
    // .enabled (deprecated) is unspecified
    // .latency (deprecated) is unspecified
    .vendor_specific_data = {{
        8,
        7,
        6,
        5,
        4,
        3,
        2,
        1,
        0,
    }},
    .started = true,
    .bypassed = false,
    .processing_delay = ZX_USEC(333),
}};
const fhasp::SettableElementState kSettableGenericElementState{{
    // .type_specific is unspecified
    .vendor_specific_data = {{
        8,
        7,
        6,
        5,
        4,
        3,
        2,
        1,
        0,
    }},
    .started = true,
    .bypassed = false,
}};
const fhasp::ElementState kDynamicsElementState{{
    .type_specific = fhasp::TypeSpecificElementState::WithDynamics({{
        .band_states = {{
            {{
                .id = 1,
                .min_frequency = 0,
                .max_frequency = 24000,
                .threshold_db = -10.0f,
                .threshold_type = fhasp::ThresholdType::kAbove,
                .ratio = 10.0f,
                .knee_width_db = 1.0f,
                .attack = ZX_USEC(1),
                .release = ZX_USEC(10),
                .output_gain_db = -1.0f,
                .input_gain_db = 0.0f,
                .level_type = fhasp::LevelType::kPeak,
                .lookahead = ZX_USEC(10),
                .linked_channels = true,
            }},
            {{
                .id = 2,
                .min_frequency = 220,
                .max_frequency = 222,
                .threshold_db = -20.0f,
                .threshold_type = fhasp::ThresholdType::kBelow,
                .ratio = 20.0f,
            }},
        }},
    }}),
    .started = true,
}};
const fhasp::SettableElementState kSettableDynamicsElementState{{
    .type_specific = fhasp::SettableTypeSpecificElementState::WithDynamics({{
        .band_states = {{
            {{
                .id = 1,
                .min_frequency = 0,
                .max_frequency = 24000,
                .threshold_db = -10.0f,
                .threshold_type = fhasp::ThresholdType::kAbove,
                .ratio = 10.0f,
                .knee_width_db = 1.0f,
                .attack = ZX_USEC(1),
                .release = ZX_USEC(10),
                .output_gain_db = -1.0f,
                .input_gain_db = 0.0f,
                .level_type = fhasp::LevelType::kPeak,
                .lookahead = ZX_USEC(10),
                .linked_channels = true,
            }},
            {{
                .id = 2,
                .min_frequency = 220,
                .max_frequency = 222,
                .threshold_db = -20.0f,
                .threshold_type = fhasp::ThresholdType::kBelow,
                .ratio = 20.0f,
            }},
        }},
    }}),
    .started = true,
}};
const fhasp::ElementState kDaiInterconnectElementState{{
    .type_specific = fhasp::TypeSpecificElementState::WithDaiInterconnect({{
        .plug_state = fhasp::PlugState{{
            .plugged = true,
            .plug_state_time = 0,
        }},
        .external_delay = 0,
    }}),
    .started = true,
}};
const fhasp::ElementState kEqualizerElementState{{
    .type_specific = fhasp::TypeSpecificElementState::WithEqualizer({{
        .band_states = {{
            {{
                .id = 1,
                .type = fhasp::EqualizerBandType::kPeak,
                .frequency = 110,
                .q = 1.0f,
                .gain_db = 1.0f,
                .enabled = true,
            }},
            {{
                .id = 2,
                .type = fhasp::EqualizerBandType::kNotch,
                .frequency = 220,
                .q = 2.0f,
                .enabled = true,
            }},
            {{
                .id = 3,
                .type = fhasp::EqualizerBandType::kLowCut,
                .frequency = 330,
                .q = 3.0f,
                .enabled = true,
            }},
            {{
                .id = 4,
                .type = fhasp::EqualizerBandType::kHighCut,
                .frequency = 440,
                .q = 4.0f,
                .enabled = true,
            }},
            {{
                .id = 5,
                .type = fhasp::EqualizerBandType::kLowShelf,
                .frequency = 550,
                .q = 5.0f,
                .gain_db = 5.0f,
                .enabled = true,
            }},
            {{
                .id = 6,
                .type = fhasp::EqualizerBandType::kHighShelf,
                .frequency = 660,
                .q = 6.0f,
                .gain_db = 6.0f,
                .enabled = true,
            }},
        }},
    }}),
    .started = true,
}};
const fhasp::SettableElementState kSettableEqualizerElementState{{
    .type_specific = fhasp::SettableTypeSpecificElementState::WithEqualizer({{
        .band_states = {{
            {{
                .id = 1,
                .type = fhasp::EqualizerBandType::kPeak,
                .frequency = 110,
                .q = 1.0f,
                .gain_db = 1.0f,
                .enabled = true,
            }},
            {{
                .id = 2,
                .type = fhasp::EqualizerBandType::kNotch,
                .frequency = 220,
                .q = 2.0f,
                .enabled = true,
            }},
            {{
                .id = 3,
                .type = fhasp::EqualizerBandType::kLowCut,
                .frequency = 330,
                .q = 3.0f,
                .enabled = true,
            }},
            {{
                .id = 4,
                .type = fhasp::EqualizerBandType::kHighCut,
                .frequency = 440,
                .q = 4.0f,
                .enabled = true,
            }},
            {{
                .id = 5,
                .type = fhasp::EqualizerBandType::kLowShelf,
                .frequency = 550,
                .q = 5.0f,
                .gain_db = 5.0f,
                .enabled = true,
            }},
            {{
                .id = 6,
                .type = fhasp::EqualizerBandType::kHighShelf,
                .frequency = 660,
                .q = 6.0f,
                .gain_db = 6.0f,
                .enabled = true,
            }},
        }},
    }}),
    .started = true,
}};
const fhasp::ElementState kGainElementState{{
    .type_specific = fhasp::TypeSpecificElementState::WithGain({{
        .gain = 0.0f,
    }}),
    .started = true,
}};
const fhasp::SettableElementState kSettableGainElementState{{
    .type_specific = fhasp::SettableTypeSpecificElementState::WithGain({{
        .gain = 0.0f,
    }}),
    .started = true,
}};
const fhasp::ElementState kVendorSpecificElementState{{
    .type_specific = fhasp::TypeSpecificElementState::WithVendorSpecific({}),
    .vendor_specific_data = {{
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
    }},
    .started = true,
}};
const fhasp::SettableElementState kSettableVendorSpecificElementState{{
    .type_specific = fhasp::SettableTypeSpecificElementState::WithVendorSpecific({}),
    .vendor_specific_data = {{
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
    }},
    .started = true,
}};
const fhasp::ElementState kElementStateStopped{{
    .started = false,
}};
const fhasp::SettableElementState kSettableElementStateStopped{{
    .started = false,
}};
const fhasp::ElementState kElementStateBypassed{{
    .started = true,
    .bypassed = true,
}};
const fhasp::SettableElementState kSettableElementStateBypassed{{
    .started = true,
    .bypassed = true,
}};
const fhasp::SettableElementState kSettableElementStateEmpty{
    // .started is not required, so this is compliant
};

// Non-compliant ElementStates
const fhasp::ElementState kElementStateEmpty{
    // .started (required) is unspecified
};

// Topology ids
constexpr TopologyId kTopologyDaiAgcDynRbId = 0;
constexpr TopologyId kTopologyDaiRbId = 10;
constexpr TopologyId kTopologyRbDaiId = 7;
constexpr TopologyId kTopologyPsDaiId = 68;
constexpr TopologyId kBadTopologyId = 42;
constexpr TopologyId kTopologyDaiAgcDynRbAndLoopId = 123;

// EdgePairs
//
// Compliant edge pairs
const fhasp::EdgePair kEdgeDaiAgc{{
    .processing_element_id_from = kDaiInterconnectElementId,
    .processing_element_id_to = kAgcElementId,
}};
const fhasp::EdgePair kEdgeAgcDyn{{
    .processing_element_id_from = kAgcElementId,
    .processing_element_id_to = kDynamicsElementId,
}};
const fhasp::EdgePair kEdgeDynRb{{
    .processing_element_id_from = kDynamicsElementId,
    .processing_element_id_to = kRingBufferElementId,
}};
const fhasp::EdgePair kEdgeDaiRb{{
    .processing_element_id_from = kDaiInterconnectElementId,
    .processing_element_id_to = kRingBufferElementId,
}};
const fhasp::EdgePair kEdgeRbDai{{
    .processing_element_id_from = kRingBufferElementId,
    .processing_element_id_to = kDaiInterconnectElementId,
}};
const fhasp::EdgePair kEdgePsDai{{
    .processing_element_id_from = kPacketStreamElementId,
    .processing_element_id_to = kDaiInterconnectElementId,
}};
const fhasp::EdgePair kEdgeToSelfAllowed{{
    .processing_element_id_from = kOtherDaiInterconnectElementId,
    .processing_element_id_to = kOtherDaiInterconnectElementId,
}};

// Non-compliant edge pairs (or ones that will be used in non-compliant ways)
const fhasp::EdgePair kEdgeToSelfWrongType{{
    .processing_element_id_from = kRingBufferElementId,
    .processing_element_id_to = kRingBufferElementId,
}};
// Other edges will refer to this self-referential element, which is not permitted.
const fhasp::EdgePair kEdgeToSelfMentionedElsewhere{{
    .processing_element_id_from = kDaiInterconnectElementId,
    .processing_element_id_to = kDaiInterconnectElementId,
}};
const fhasp::EdgePair kEdgeUnknownId{{
    .processing_element_id_from = kRingBufferElementId,
    .processing_element_id_to = kBadElementId,
}};

// Topologies
//
// Compliant topologies
const fhasp::Topology kTopologyDaiAgcDynRb{{
    .id = kTopologyDaiAgcDynRbId,
    .processing_elements_edge_pairs = {{
        kEdgeDaiAgc,
        kEdgeAgcDyn,
        kEdgeDynRb,
    }},
}};
const fhasp::Topology kTopologyDaiRb{{
    .id = kTopologyDaiRbId,
    .processing_elements_edge_pairs = {{
        kEdgeDaiRb,
    }},
}};
const fhasp::Topology kTopologyRbDai{{
    .id = kTopologyRbDaiId,
    .processing_elements_edge_pairs = {{
        kEdgeRbDai,
    }},
}};
const fhasp::Topology kTopologyPsDai{{
    .id = kTopologyPsDaiId,
    .processing_elements_edge_pairs = {{
        kEdgePsDai,
    }},
}};
const fhasp::Topology kTopologyDaiAgcDynRbAndLoop{{
    .id = kTopologyDaiAgcDynRbAndLoopId,
    .processing_elements_edge_pairs = {{
        kEdgeDaiAgc,
        kEdgeAgcDyn,
        kEdgeDynRb,
        kEdgeToSelfAllowed,
    }},
}};

// Non-compliant topologies
const fhasp::Topology kTopologyMissingId{{
    .processing_elements_edge_pairs = {{
        kEdgeDaiRb,
    }},
}};
const fhasp::Topology kTopologyMissingEdgePairs{{
    .id = kBadTopologyId,
}};
const fhasp::Topology kTopologyEmptyEdgePairs{{
    .id = kBadTopologyId,
    .processing_elements_edge_pairs = {{}},
}};
const fhasp::Topology kTopologyUnknownElementId{{
    .id = kBadTopologyId,
    .processing_elements_edge_pairs = {{
        kEdgeDaiAgc,
        kEdgeAgcDyn,
        kEdgeDynRb,
        kEdgeUnknownId,
    }},
}};
const fhasp::Topology kTopologyEdgePairLoopWrongElementType{{
    .id = kBadTopologyId,
    .processing_elements_edge_pairs = {{
        kEdgeToSelfWrongType,
    }},
}};
const fhasp::Topology kTopologyEdgePairLoopNotExclusive{{
    .id = kBadTopologyId,
    .processing_elements_edge_pairs = {{
        kEdgeDaiAgc,
        kEdgeAgcDyn,
        kEdgeDynRb,
        kEdgeToSelfMentionedElsewhere,
    }},
}};
const fhasp::Topology kTopologyTerminalNotEndpoint{{
    .id = kBadTopologyId,
    .processing_elements_edge_pairs = {{
        kEdgeRbDai,
        kEdgeDaiAgc,
        kEdgeAgcDyn,
    }},
}};

// Collections of topologies
//
// Compliant collections
const std::vector<fhasp::Topology> kTopologies{
    kTopologyDaiAgcDynRb,
    kTopologyDaiRb,
    kTopologyRbDai,
    kTopologyPsDai,
};
const std::vector<fhasp::Topology> kTopologiesWithLoop{
    kTopologyDaiAgcDynRb,
    kTopologyDaiRb,
    kTopologyRbDai,
    kTopologyDaiAgcDynRbAndLoop,
};

// Non-compliant collections
const std::vector<fhasp::Topology> kEmptyTopologies{};
const std::vector<fhasp::Topology> kTopologiesWithDuplicateId{
    kTopologyDaiAgcDynRb,
    kTopologyDaiAgcDynRb,
};
const std::vector<fhasp::Topology> kTopologiesWithoutAllElements{kTopologyDaiRb};
const std::vector<fhasp::Topology> kTopologiesWithMissingId{
    kTopologyDaiAgcDynRb,
    kTopologyMissingId,
};
const std::vector<fhasp::Topology> kTopologiesWithMissingEdgePairs{
    kTopologyDaiAgcDynRb,
    kTopologyMissingEdgePairs,
};
const std::vector<fhasp::Topology> kTopologiesWithEmptyEdgePairs{
    kTopologyDaiAgcDynRb,
    kTopologyEmptyEdgePairs,
};
const std::vector<fhasp::Topology> kTopologiesWithUnknownElementId{
    kTopologyDaiAgcDynRb,
    kTopologyUnknownElementId,
};
const std::vector<fhasp::Topology> kTopologiesWithLoopWrongType{
    kTopologyDaiAgcDynRb,
    kTopologyEdgePairLoopWrongElementType,
};
const std::vector<fhasp::Topology> kTopologiesWithLoopNonExclusive{
    kTopologyDaiAgcDynRb,
    kTopologyEdgePairLoopNotExclusive,
};
const std::vector<fhasp::Topology> kTopologiesWithTerminalNotEndpoint{
    kTopologyDaiAgcDynRb,
    kTopologyTerminalNotEndpoint,
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_UNITTEST_H_
