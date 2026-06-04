#!/usr/bin/env bash
set -euo pipefail

cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build --target e2e_ipc_bridge_test
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure -R '^e2e_ipc_bridge_test$'
