// Setup Coach Voice — identifiers. Never change the FUID once released.
#pragma once

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/vst/vsttypes.h"

namespace setupcoach {

static const Steinberg::FUID kVoiceUID(0x2FE397E4, 0xAF0F4DA3, 0xA3CC5EA0, 0x70FE052D);

enum ParamIds : Steinberg::Vst::ParamID {
    kBypassId = 0,
};

}  // namespace setupcoach
