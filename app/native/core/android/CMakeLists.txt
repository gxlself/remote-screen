# 由 ubrn 生成后手工改过，勿重新生成覆盖
cmake_minimum_required(VERSION 3.9.0)
project(RsCore)

set (CMAKE_VERBOSE_MAKEFILE ON)
set (CMAKE_CXX_STANDARD 17)

set(RS_MOBILE_LIB_DIR "${CMAKE_SOURCE_DIR}/src/main/jniLibs" CACHE PATH
    "Directory containing <abi>/librs_mobile.a")
set(MY_RUST_LIB "${RS_MOBILE_LIB_DIR}/${ANDROID_ABI}/librs_mobile.a")
if(NOT EXISTS "${MY_RUST_LIB}")
  message(FATAL_ERROR "librs_mobile.a not found at ${MY_RUST_LIB}; run app/rust/build-android.sh or set RS_MOBILE_ANDROID_LIBS")
endif()

# Resolve the path to the uniffi-bindgen-react-native package
execute_process(
    COMMAND node -p "require.resolve('uniffi-bindgen-react-native/package.json')"
    OUTPUT_VARIABLE UNIFFI_BINDGEN_PATH
    OUTPUT_STRIP_TRAILING_WHITESPACE
)
# Get the directory; get_filename_component and cmake_path will normalize
# paths with Windows path separators.
get_filename_component(UNIFFI_BINDGEN_PATH "${UNIFFI_BINDGEN_PATH}" DIRECTORY)

# Specifies a path to native header files.
include_directories(
    ../cpp
    ../../../cpp/generated
    ${CMAKE_SOURCE_DIR}/../../../../crates/rs-mobile/include
    ${CMAKE_SOURCE_DIR}/../../../android/rsjni

    ${UNIFFI_BINDGEN_PATH}/cpp/includes
)

file(GLOB RS_JNI_SOURCES CONFIGURE_DEPENDS
    ${CMAKE_SOURCE_DIR}/../../../android/rsjni/*.cpp)

add_library(react-native-rs-core            SHARED
    ../cpp/react-native-rs-core.cpp
    ../../../cpp/generated/rs_mobile.cpp
    cpp-adapter.cpp
    ${RS_JNI_SOURCES}
)

# Set C++ compiler flags
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -O2")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fexceptions")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -frtti")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fstack-protector-all")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Wall")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Werror")

# Set linker flags for 16KB page size alignment (required for Android 15+)
set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} -Wl,-z,max-page-size=16384")

add_library(my_rust_lib STATIC IMPORTED)
set_target_properties(my_rust_lib PROPERTIES IMPORTED_LOCATION "${MY_RUST_LIB}")

# Add ReactAndroid libraries, being careful to account for different versions.
find_package(ReactAndroid REQUIRED CONFIG)
find_library(LOGCAT log)
find_library(ANDROID_LIB android)
find_library(AAUDIO_LIB aaudio)
# flate2's libz-sys backend (pulled in with IronRDP) leaves zlib to the final link.
find_library(ZLIB_LIB z)

# REACTNATIVE_MERGED_SO seems to be only be set in a build.gradle.kt file,
# which we don't use. Thus falling back to version number sniffing.
if (ReactAndroid_VERSION_MINOR GREATER_EQUAL 76)
  set(REACTNATIVE_MERGED_SO true)
endif()

# https://github.com/react-native-community/discussions-and-proposals/discussions/816
# This if-then-else can be removed once this library does not support version below 0.76
if (REACTNATIVE_MERGED_SO)
  target_link_libraries(react-native-rs-core ReactAndroid::reactnative)
else()
  target_link_libraries(react-native-rs-core
    ReactAndroid::turbomodulejsijni
    ReactAndroid::react_nativemodule_core
  )
endif()

find_package(fbjni REQUIRED CONFIG)
target_link_libraries(
  react-native-rs-core
  fbjni::fbjni
  ReactAndroid::jsi
  ${LOGCAT}
  ${ANDROID_LIB}
  ${AAUDIO_LIB}
  my_rust_lib
  ${ZLIB_LIB}
)
