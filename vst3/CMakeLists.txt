cmake_minimum_required(VERSION 3.25.0)

# Setup Coach Voice — VST3 effect for OBS (HPF, EQ, de-esser, mono) + DSP tests.
#
#   cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
#   cmake --build build --config Release
#   ctest --test-dir build -C Release
#
# Options:
#   SC_BUILD_PLUGIN   build the VST3 plug-in (default ON on Windows/macOS, OFF elsewhere)
#   SC_BUILD_TESTS    build the DSP tests (default ON)
#   VST3SDK_DIR       use a local VST 3 SDK checkout instead of fetching it

if(WIN32 OR APPLE)
    set(SC_PLUGIN_DEFAULT ON)
else()
    set(SC_PLUGIN_DEFAULT OFF)
endif()
option(SC_BUILD_PLUGIN "Build the VST3 plug-in" ${SC_PLUGIN_DEFAULT})
option(SC_BUILD_TESTS "Build the DSP tests" ON)
set(VST3SDK_DIR "" CACHE PATH "Local VST 3 SDK checkout (optional)")

# VST 3 SDK options must be set before the SDK is added.
set(SMTG_ENABLE_VST3_PLUGIN_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SMTG_ENABLE_VST3_HOSTING_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SMTG_ENABLE_VSTGUI_SUPPORT OFF CACHE BOOL "" FORCE)
set(SMTG_CREATE_PLUGIN_LINK OFF CACHE BOOL "" FORCE)

project(SetupCoachVoice VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include(FetchContent)

# nlohmann/json (MIT), header-only, release tarball (no git history needed).
FetchContent_Declare(nlohmann_json
    URL https://github.com/nlohmann/json/releases/download/v3.11.3/json.tar.xz
    URL_HASH SHA256=d6c65aca6b1ed68e7a182f4757257b107ae403032760ed6ef121c9d55e81757d
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_MakeAvailable(nlohmann_json)

add_library(sc_dsp INTERFACE)
target_include_directories(sc_dsp INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/dsp)
target_link_libraries(sc_dsp INTERFACE nlohmann_json::nlohmann_json)
target_compile_features(sc_dsp INTERFACE cxx_std_17)
find_package(Threads REQUIRED)
target_link_libraries(sc_dsp INTERFACE Threads::Threads)

if(SC_BUILD_TESTS)
    enable_testing()
    add_executable(sc_dsp_tests tests/test_dsp.cpp)
    target_link_libraries(sc_dsp_tests PRIVATE sc_dsp)
    target_compile_definitions(sc_dsp_tests PRIVATE SC_FIXTURE_DIR="${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures")
    if(MSVC)
        target_compile_options(sc_dsp_tests PRIVATE /W4 /permissive-)
    else()
        target_compile_options(sc_dsp_tests PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    add_test(NAME sc_dsp_tests COMMAND sc_dsp_tests)
endif()

if(SC_BUILD_PLUGIN)
    # Steinberg VST 3 SDK — MIT licensed since 3.8.0. Pinned release tag.
    if(VST3SDK_DIR)
        set(vst3sdk_SOURCE_DIR ${VST3SDK_DIR})
    else()
        FetchContent_Declare(vst3sdk
            GIT_REPOSITORY https://github.com/steinbergmedia/vst3sdk.git
            GIT_TAG v3.8.0_build_66
            GIT_SHALLOW TRUE
            GIT_SUBMODULES base cmake pluginterfaces public.sdk
            GIT_PROGRESS FALSE
            # Populate only; the SDK is added below with our own binary dir.
            # (Avoids the single-argument FetchContent_Populate deprecated in CMake 3.30.)
            SOURCE_SUBDIR _populate_only)
        FetchContent_MakeAvailable(vst3sdk)
    endif()

    add_subdirectory(${vst3sdk_SOURCE_DIR} ${PROJECT_BINARY_DIR}/vst3sdk EXCLUDE_FROM_ALL)
    smtg_enable_vst3_sdk()

    smtg_add_vst3plugin(SetupCoachVoice
        ${vst3sdk_SOURCE_DIR}/public.sdk/source/vst/vstsinglecomponenteffect.cpp
        ${vst3sdk_SOURCE_DIR}/public.sdk/source/vst/vstsinglecomponenteffect.h
        plugin/ids.h
        plugin/version.h
        plugin/voice_effect.h
        plugin/voice_effect.cpp
        plugin/factory.cpp)
    target_link_libraries(SetupCoachVoice PRIVATE sdk sc_dsp)
    target_compile_features(SetupCoachVoice PUBLIC cxx_std_17)
    if(WIN32)
        target_sources(SetupCoachVoice PRIVATE plugin/win32resource.rc)
    endif()
endif()

# Run Steinberg's validator on the built bundle as a test (when available).
if(SC_BUILD_PLUGIN AND SC_BUILD_TESTS AND TARGET validator)
    add_test(NAME vst3_validator
        COMMAND validator "${CMAKE_BINARY_DIR}/VST3/$<CONFIG>/SetupCoachVoice.vst3")
endif()
