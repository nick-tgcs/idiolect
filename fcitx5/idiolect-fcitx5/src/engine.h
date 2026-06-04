#pragma once

#include <string>

#include "ipc_client.h"

namespace idiolect::fcitx5 {

enum class DisconnectPreeditPolicy {
    Clear,
    Preserve,
};

class Engine {
public:
    explicit Engine(IpcClient& ipc_client);
    Engine(IpcClient& ipc_client, DisconnectPreeditPolicy disconnect_policy);

    void start_recording();
    void receive_preedit(std::string text);
    void commit_preedit();
    void cancel_preedit();
    void recover_from_daemon_disconnect();

    [[nodiscard]] const std::string& visible_preedit() const;

private:
    IpcClient& ipc_client_;
    DisconnectPreeditPolicy disconnect_policy_;
    std::string visible_preedit_;
};

} // namespace idiolect::fcitx5
