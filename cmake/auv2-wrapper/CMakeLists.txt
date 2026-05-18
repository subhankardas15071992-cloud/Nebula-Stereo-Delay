cmake_minimum_required(VERSION 3.21)

project(NebulaStereoDelayAUV2Wrapper LANGUAGES C CXX OBJC OBJCXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(NEBULA_PLUGIN_NAME "Nebula Stereo Delay" CACHE STRING "Plugin display name")
set(NEBULA_PLUGIN_VERSION "1.0.0" CACHE STRING "Plugin bundle version")
set(NEBULA_CLAP_BUNDLE_PATH "" CACHE PATH "Path to the built CLAP bundle")
set(NEBULA_AUV2_OUTPUT_DIR "${CMAKE_BINARY_DIR}/dist" CACHE PATH "Directory for the AUv2 component")
set(NEBULA_AUV2_BUNDLE_IDENTIFIER "audio.nebula.NebulaStereoDelay.auv2" CACHE STRING "AUv2 bundle identifier")
set(NEBULA_AUV2_MANUFACTURER_NAME "Nebula Audio" CACHE STRING "AUv2 manufacturer name")
set(NEBULA_AUV2_MANUFACTURER_CODE "NbAu" CACHE STRING "AUv2 four-character manufacturer code")
set(NEBULA_AUV2_SUBTYPE_CODE "NsDl" CACHE STRING "AUv2 four-character subtype code")

if(NOT APPLE)
    message(FATAL_ERROR "The Nebula Stereo Delay AUv2 wrapper can only be built on macOS")
endif()

if(NOT IS_DIRECTORY "${NEBULA_CLAP_BUNDLE_PATH}")
    message(FATAL_ERROR "NEBULA_CLAP_BUNDLE_PATH must point to a built .clap bundle")
endif()

if(NOT EXISTS "${NEBULA_CLAP_BUNDLE_PATH}/Contents/Info.plist")
    message(FATAL_ERROR "NEBULA_CLAP_BUNDLE_PATH does not look like a macOS CLAP bundle")
endif()

set(CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES TRUE CACHE BOOL "Download clap-wrapper SDK dependencies" FORCE)
set(CLAP_WRAPPER_BUILD_TESTS OFF CACHE BOOL "Do not build clap-wrapper tests" FORCE)
set(CLAP_WRAPPER_COPY_AFTER_BUILD OFF CACHE BOOL "Do not install clap-wrapper outputs" FORCE)

set(CLAP_WRAPPER_SOURCE_DIR "" CACHE PATH "Optional local clap-wrapper checkout")

if(CLAP_WRAPPER_SOURCE_DIR)
    add_subdirectory("${CLAP_WRAPPER_SOURCE_DIR}" "${CMAKE_BINARY_DIR}/clap-wrapper")
else()
    include(FetchContent)
    FetchContent_Declare(
        clap_wrapper
        GIT_REPOSITORY https://github.com/free-audio/clap-wrapper.git
        GIT_TAG v0.14.0
        GIT_SHALLOW TRUE
    )
    FetchContent_MakeAvailable(clap_wrapper)
endif()

file(MAKE_DIRECTORY "${NEBULA_AUV2_OUTPUT_DIR}")

add_library(nebula_stereo_delay_auv2 MODULE)
set_target_properties(nebula_stereo_delay_auv2 PROPERTIES
    LIBRARY_OUTPUT_DIRECTORY "${NEBULA_AUV2_OUTPUT_DIR}"
)

target_add_auv2_wrapper(
    TARGET nebula_stereo_delay_auv2
    OUTPUT_NAME "${NEBULA_PLUGIN_NAME}"
    BUNDLE_IDENTIFIER "${NEBULA_AUV2_BUNDLE_IDENTIFIER}"
    BUNDLE_VERSION "${NEBULA_PLUGIN_VERSION}"
    MANUFACTURER_NAME "${NEBULA_AUV2_MANUFACTURER_NAME}"
    MANUFACTURER_CODE "${NEBULA_AUV2_MANUFACTURER_CODE}"
    SUBTYPE_CODE "${NEBULA_AUV2_SUBTYPE_CODE}"
    INSTRUMENT_TYPE "aufx"
    MACOS_EMBEDDED_CLAP_LOCATION "${NEBULA_CLAP_BUNDLE_PATH}"
)
