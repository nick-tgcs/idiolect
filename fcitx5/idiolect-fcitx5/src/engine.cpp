#include "engine.h"

#include <utility>

namespace idiolect::fcitx5 {

Engine::Engine(IpcClient& ipc_client) : ipc_client_(ipc_client) {}

void Engine::start_recording() {
    ipc_client_.start_recording();
}

void Engine::receive_preedit(std::string text) {
    visible_preedit_ = std::move(text);
}

void Engine::commit_preedit() {
    ipc_client_.commit_preedit(visible_preedit_);
    visible_preedit_.clear();
}

void Engine::cancel_preedit() {
    ipc_client_.cancel_preedit();
    visible_preedit_.clear();
}

const std::string& Engine::visible_preedit() const {
    return visible_preedit_;
}

} // namespace idiolect::fcitx5
