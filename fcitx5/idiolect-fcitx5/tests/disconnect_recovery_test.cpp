#include "engine.h"

#include <cstdlib>
#include <string>
#include <stdexcept>
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

class FailingReconnectClient final : public idiolect::fcitx5::IpcClient {
public:
    void toggle_recording() override {}
    void start_recording() override {}
    void stop_recording() override {}
    void commit_preedit(const std::string&) override {}
    void cancel_preedit() override {}

    void reconnect() override {
        throw std::runtime_error("reconnect failed");
    }
};

void require(bool condition) {
    if (!condition) {
        std::exit(1);
    }
}

} // namespace

int main() {
    FakeIpcClient clear_client;
    idiolect::fcitx5::Engine clear_engine(
        clear_client, idiolect::fcitx5::DisconnectPreeditPolicy::Clear);
    clear_engine.receive_preedit("restart traffic");
    clear_engine.recover_from_daemon_disconnect();

    require(clear_engine.visible_preedit().empty());
    require(clear_client.messages.size() == 1);
    require(clear_client.messages[0] == "reconnect");

    FakeIpcClient preserve_client;
    idiolect::fcitx5::Engine preserve_engine(
        preserve_client, idiolect::fcitx5::DisconnectPreeditPolicy::Preserve);
    preserve_engine.receive_preedit("restart Traefik");
    preserve_engine.recover_from_daemon_disconnect();

    require(preserve_engine.visible_preedit() == "restart Traefik");
    require(preserve_client.messages.size() == 1);
    require(preserve_client.messages[0] == "reconnect");

    preserve_engine.commit_preedit();
    require(preserve_client.messages.size() == 2);
    require(preserve_client.messages[1] == "commit:restart Traefik");
    require(preserve_engine.visible_preedit().empty());

    FailingReconnectClient failing_client;
    idiolect::fcitx5::Engine failing_engine(
        failing_client, idiolect::fcitx5::DisconnectPreeditPolicy::Clear);
    failing_engine.receive_preedit("stale draft");
    try {
        failing_engine.recover_from_daemon_disconnect();
        require(false);
    } catch (const std::runtime_error&) {
        require(failing_engine.visible_preedit().empty());
    }

    return 0;
}
