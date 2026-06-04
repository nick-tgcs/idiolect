cmake_minimum_required(VERSION 3.16)

project(idiolect_fcitx5 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_library(idiolect-fcitx5 SHARED
    src/engine.cpp
    src/ipc_client.cpp
)

target_include_directories(idiolect-fcitx5
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/src
)

enable_testing()

add_executable(preedit_session_test
    tests/preedit_session_test.cpp
)

target_link_libraries(preedit_session_test PRIVATE idiolect-fcitx5)

add_test(NAME preedit_session_test COMMAND preedit_session_test)
