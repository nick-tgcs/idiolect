#pragma once

#include <string>

#include "ipc_client.h"

namespace idiolect::fcitx5 {

class Engine {
public:
    explicit Engine(IpcClient& ipc_client);

    void start_recording();
    void receive_preedit(std::string text);
    void commit_preedit();
    void cancel_preedit();

    [[nodiscard]] const std::string& visible_preedit() const;

private:
    IpcClient& ipc_client_;
    std::string visible_preedit_;
};

} // namespace idiolect::fcitx5
