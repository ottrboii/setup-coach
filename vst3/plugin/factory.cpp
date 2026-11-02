// Setup Coach Voice — plug-in factory.
#include "ids.h"
#include "version.h"
#include "voice_effect.h"

#include "public.sdk/source/main/pluginfactory.h"

using namespace Steinberg;
using namespace Steinberg::Vst;

BEGIN_FACTORY_DEF(stringCompanyName, stringCompanyWeb, stringCompanyEmail)

DEF_CLASS2(INLINE_UID_FROM_FUID(setupcoach::kVoiceUID),
           PClassInfo::kManyInstances,
           kVstAudioEffectClass,
           stringPluginName,
           0,  // single-component effects cannot be distributed
           "Fx|EQ",
           FULL_VERSION_STR,
           kVstVersionString,
           setupcoach::VoiceEffect::createInstance)

END_FACTORY
