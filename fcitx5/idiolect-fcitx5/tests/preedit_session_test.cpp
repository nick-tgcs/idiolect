#include "engine.h"

#include <cstdlib>
#include <string>
#include <vector>

namespace {

class FakeIpcClient final : public idiolect::fcitx5::IpcClient {
public:
    void toggle_recording() override {
        messages.emplace_back("toggle_recording");
    }

    void start_recording() override {
        messages.emplace_back("start_recording");
    }

    void stop_recording() override {
        messages.emplace_back("stop_recording");
    }

    void commit_preedit(const std::string& text) override {
        messages.emplace_back("commit:" + text);
    }

    void cancel_preedit() override {
        messages.emplace_back("cancel");
    }

    void reconnect() override {
        messages.emplace_back("reconnect");
    }

    std::vector<std::string> messages;
};

void require(bool condition) {
    if (!condition) {
        std::exit(1);
    }
}

} // namespace

int main() {
    FakeIpcClient ipc_client;
    idiolect::fcitx5::Engine engine(ipc_client);

    engine.start_recording();
    engine.receive_preedit("restart Traefik");
    require(engine.visible_preedit() == "restart Traefik");

    engine.commit_preedit();

    require(ipc_client.messages.size() == 2);
    require(ipc_client.messages[0] == "start_recording");
    require(ipc_client.messages[1] == "commit:restart Traefik");
    require(engine.visible_preedit().empty());

    engine.receive_preedit("pending draft");
    engine.recover_from_daemon_disconnect();
    require(engine.visible_preedit().empty());
    require(ipc_client.messages.size() == 3);
    require(ipc_client.messages[2] == "reconnect");

    require(idiolect::fcitx5::client_protocol_version() == 1);
    const auto features = idiolect::fcitx5::client_features();
    require(features.size() == 3);
    require(features[0] == "preedit");
    require(features[1] == "commit");
    require(features[2] == "recording_status");

    return 0;
}
