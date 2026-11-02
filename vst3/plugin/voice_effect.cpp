// Setup Coach Voice — single-component VST3 effect implementation.
#include "voice_effect.h"
#include "ids.h"

#include "base/source/fstreamer.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/vstspeaker.h"

using namespace Steinberg;
using namespace Steinberg::Vst;

namespace setupcoach {

VoiceEffect::VoiceEffect() = default;
VoiceEffect::~VoiceEffect() = default;

tresult PLUGIN_API VoiceEffect::initialize(FUnknown* context) {
    tresult result = SingleComponentEffect::initialize(context);
    if (result != kResultOk) return result;

    addAudioInput(STR16("Mic In"), SpeakerArr::kStereo);
    addAudioOutput(STR16("Voice Out"), SpeakerArr::kStereo);

    parameters.addParameter(STR16("Bypass"), nullptr, 1, 0,
                            ParameterInfo::kCanAutomate | ParameterInfo::kIsBypass, kBypassId);

    // Settings come from the Setup Coach app via chain.json; watch it off the
    // audio thread for the whole lifetime of the instance.
    watcher_ = std::make_unique<ChainWatcher>(chain_);
    watcher_->start();
    return kResultOk;
}

tresult PLUGIN_API VoiceEffect::terminate() {
    if (watcher_) watcher_->stop();
    watcher_.reset();
    return SingleComponentEffect::terminate();
}

tresult PLUGIN_API VoiceEffect::setActive(TBool /*state*/) {
    chain_.collectGarbage();
    return kResultOk;
}

tresult PLUGIN_API VoiceEffect::setProcessing(TBool /*state*/) { return kResultOk; }

tresult PLUGIN_API VoiceEffect::setupProcessing(ProcessSetup& setup) {
    chain_.prepare(setup.sampleRate, setup.maxSamplesPerBlock);
    return SingleComponentEffect::setupProcessing(setup);
}

tresult PLUGIN_API VoiceEffect::setBusArrangements(SpeakerArrangement* inputs, int32 numIns,
                                                   SpeakerArrangement* outputs, int32 numOuts) {
    if (numIns != 1 || numOuts != 1) return kResultFalse;
    int32 inCh = SpeakerArr::getChannelCount(inputs[0]);
    int32 outCh = SpeakerArr::getChannelCount(outputs[0]);
    if ((inCh == 1 || inCh == 2) && (outCh == 1 || outCh == 2)) {
        if (auto* in = FCast<AudioBus>(audioInputs.at(0))) {
            in->setArrangement(inputs[0]);
            in->setName(inCh == 1 ? STR16("Mic In (Mono)") : STR16("Mic In"));
        }
        if (auto* out = FCast<AudioBus>(audioOutputs.at(0))) {
            out->setArrangement(outputs[0]);
            out->setName(outCh == 1 ? STR16("Voice Out (Mono)") : STR16("Voice Out"));
        }
        return kResultTrue;
    }
    return kResultFalse;
}

tresult PLUGIN_API VoiceEffect::canProcessSampleSize(int32 symbolicSampleSize) {
    return symbolicSampleSize == kSample32 ? kResultTrue : kResultFalse;
}

tresult PLUGIN_API VoiceEffect::process(ProcessData& data) {
    if (IParameterChanges* changes = data.inputParameterChanges) {
        for (int32 i = 0; i < changes->getParameterCount(); ++i) {
            IParamValueQueue* q = changes->getParameterData(i);
            if (!q || q->getParameterId() != kBypassId) continue;
            ParamValue v;
            int32 offset;
            if (q->getPoint(q->getPointCount() - 1, offset, v) == kResultTrue) bypass_ = v > 0.5;
        }
    }
    chain_.setHostBypass(bypass_);

    if (data.numSamples <= 0 || data.numInputs == 0 || data.numOutputs == 0) return kResultOk;
    AudioBusBuffers& in = data.inputs[0];
    AudioBusBuffers& out = data.outputs[0];
    if (!in.channelBuffers32 || !out.channelBuffers32) return kResultOk;

    chain_.processToOutputs(in.channelBuffers32, in.numChannels, out.channelBuffers32, out.numChannels,
                            data.numSamples);
    out.silenceFlags = 0;
    return kResultOk;
}

tresult PLUGIN_API VoiceEffect::setState(IBStream* state) {
    if (!state) return kResultFalse;
    IBStreamer s(state, kLittleEndian);
    int32 version = 0, bypass = 0;
    if (!s.readInt32(version) || !s.readInt32(bypass)) return kResultFalse;
    bypass_ = bypass != 0;
    setParamNormalized(kBypassId, bypass_ ? 1.0 : 0.0);
    return kResultOk;
}

tresult PLUGIN_API VoiceEffect::getState(IBStream* state) {
    if (!state) return kResultFalse;
    IBStreamer s(state, kLittleEndian);
    s.writeInt32(1);  // state version
    s.writeInt32(bypass_ ? 1 : 0);
    return kResultOk;
}

}  // namespace setupcoach
