cmake_minimum_required(VERSION 3.16)

project(idiolect_fcitx5 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(Threads REQUIRED)

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

add_executable(e2e_ipc_bridge_test
    tests/e2e_ipc_bridge_test.cpp
)

target_link_libraries(e2e_ipc_bridge_test PRIVATE idiolect-fcitx5 Threads::Threads)

add_test(NAME e2e_ipc_bridge_test COMMAND e2e_ipc_bridge_test)

add_executable(disconnect_recovery_test
    tests/disconnect_recovery_test.cpp
)

target_link_libraries(disconnect_recovery_test PRIVATE idiolect-fcitx5)

add_test(NAME disconnect_recovery_test COMMAND disconnect_recovery_test)

add_executable(toggle_session_test
    tests/toggle_session_test.cpp
)

target_link_libraries(toggle_session_test PRIVATE idiolect-fcitx5)

add_test(NAME toggle_session_test COMMAND toggle_session_test)

add_executable(recording_status_wire_test
    tests/recording_status_wire_test.cpp
)

target_link_libraries(recording_status_wire_test PRIVATE idiolect-fcitx5 Threads::Threads)

add_test(NAME recording_status_wire_test COMMAND recording_status_wire_test)

# ---------------------------------------------------------------------------
# The real fcitx5 input-method module. Built only when the fcitx5 development
# packages are present, so the engine/IPC logic above still builds and tests on
# machines without fcitx5 (e.g. CI). On the target machine, installing
# libfcitx5core-dev / libfcitx5utils-dev / libfcitx5config-dev enables this.
# ---------------------------------------------------------------------------
find_package(Fcitx5Core QUIET)
find_package(Fcitx5Utils QUIET)
find_package(Fcitx5Config QUIET)

if(TARGET Fcitx5::Core)
    message(STATUS "Fcitx5 found: building the idiolect input-method module")

    add_library(idiolect MODULE
        src/engine.cpp
        src/ipc_client.cpp
        src/idiolect_module.cpp
    )

    target_include_directories(idiolect PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/src)
    target_link_libraries(idiolect
        PRIVATE
            Fcitx5::Core
            Fcitx5::Utils
            Fcitx5::Config
            Threads::Threads
    )
    # Produce "idiolect.so" (matches Library=idiolect in the addon .conf), not
    # "libidiolect.so".
    set_target_properties(idiolect PROPERTIES PREFIX "")

    install(TARGETS idiolect
        DESTINATION "${FCITX_INSTALL_LIBDIR}/fcitx5"
    )
    install(FILES data/idiolect-addon.conf
        RENAME idiolect.conf
        DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon"
    )
else()
    message(STATUS "Fcitx5 not found: skipping the idiolect module "
                   "(engine/IPC logic and tests still build). Install "
                   "libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev to enable it.")
endif()
