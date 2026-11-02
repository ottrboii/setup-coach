#pragma once

#include "pluginterfaces/base/fplatform.h"

#define SC_MAJOR 0
#define SC_MINOR 1
#define SC_BUGFIX 0
#define SC_BUILD 1

#define SC_STR_(x) #x
#define SC_STR(x) SC_STR_(x)

#define FULL_VERSION_STR SC_STR(SC_MAJOR) "." SC_STR(SC_MINOR) "." SC_STR(SC_BUGFIX) "." SC_STR(SC_BUILD)
#define VERSION_STR SC_STR(SC_MAJOR) "." SC_STR(SC_MINOR) "." SC_STR(SC_BUGFIX)

#define stringPluginName "Setup Coach Voice"
#define stringOriginalFilename "SetupCoachVoice.vst3"
#if SMTG_PLATFORM_64
#define stringFileDescription stringPluginName " VST3 (64Bit)"
#else
#define stringFileDescription stringPluginName " VST3"
#endif
#define stringCompanyName "Ottr Gaming Influencer AB"
#define stringCompanyWeb ""
#define stringCompanyEmail ""
#define stringLegalCopyright "Copyright (c) 2026 Ottr Gaming Influencer AB"
#define stringLegalTrademarks "VST is a trademark of Steinberg Media Technologies GmbH"
