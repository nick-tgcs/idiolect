#include "engine.h"

#include <cstdlib>
#include <string>
#include <vector>

namespace {

using idiolect::fcitx5::Engine;
using idiolect::fcitx5::RecordingState;

class FakeIpcClient final : public idiolect::fcitx5::IpcClient {
public:
    void toggle_recording() override { messages.emplace_back("toggle_recording"); }
    void start_recording() override { messages.emplace_back("start_recording"); }
    void stop_recording() override { messages.emplace_back("stop_recording"); }
    void commit_preedit(const std::string& text) override {
        messages.emplace_back("commit:" + text);
    }
    void cancel_preedit() override { messages.emplace_back("cancel"); }
    void reconnect() override { messages.emplace_back("reconnect"); }

    std::vector<std::string> messages;
};

class FakeCommitter final : public idiolect::fcitx5::TextCommitter {
public:
    void commit(const std::string& text) override { committed.push_back(text); }
    std::vector<std::string> committed;
};

void require(bool condition) {
    if (!condition) {
        std::exit(1);
    }
}

} // namespace

int main() {
    // Happy path: a toggle is one direction-free intent that does NOT flip the
    // phase locally — the daemon's RecordingStatus push drives Idle<->Recording.
    // The transcript auto-commits into the app AND finalizes on the daemon.
    {
        FakeIpcClient ipc;
        FakeCommitter committer;
        Engine engine(ipc, committer);

        require(engine.state() == RecordingState::Idle);

        engine.toggle(); // start intent — no optimistic flip
        require(engine.state() == RecordingState::Idle);
        engine.on_recording_status(true); // daemon: recording started
        require(engine.state() == RecordingState::Recording);

        engine.toggle(); // stop intent — still no optimistic flip
        require(engine.state() == RecordingState::Recording);

        engine.on_transcript("restart traffic"); // daemon: transcript
        require(engine.state() == RecordingState::Idle);
        engine.on_recording_status(false); // daemon: mic closed (no-op now)
        require(engine.state() == RecordingState::Idle);

        require(committer.committed.size() == 1);
        require(committer.committed[0] == "restart traffic");

        require(ipc.messages.size() == 3);
        require(ipc.messages[0] == "toggle_recording");
        require(ipc.messages[1] == "toggle_recording");
        require(ipc.messages[2] == "commit:restart traffic");
    }

    // RecordingStatus is the single source of truth for the phase.
    {
        FakeIpcClient ipc;
        Engine engine(ipc);
        engine.on_recording_status(true);
        require(engine.state() == RecordingState::Recording);
        engine.on_recording_status(false);
        require(engine.state() == RecordingState::Idle);
    }

    // A transcript arriving when not recording must NOT commit (no stray text).
    {
        FakeIpcClient ipc;
        FakeCommitter committer;
        Engine engine(ipc, committer);

        engine.on_transcript("spurious");
        require(committer.committed.empty());
        require(ipc.messages.empty());
        require(engine.state() == RecordingState::Idle);
    }

    // Cancel mid-take: aborts, no commit, returns to idle.
    {
        FakeIpcClient ipc;
        FakeCommitter committer;
        Engine engine(ipc, committer);

        engine.toggle(); // start intent
        engine.on_recording_status(true);
        engine.cancel();
        require(engine.state() == RecordingState::Idle);
        require(committer.committed.empty());
        require(ipc.messages.size() == 2);
        require(ipc.messages[0] == "toggle_recording");
        require(ipc.messages[1] == "cancel");
    }

    // Daemon error mid-take resets to idle without committing.
    {
        FakeIpcClient ipc;
        FakeCommitter committer;
        Engine engine(ipc, committer);

        engine.toggle();
        engine.on_recording_status(true);
        engine.toggle();
        engine.on_error();
        require(engine.state() == RecordingState::Idle);
        require(committer.committed.empty());
    }

    return 0;
}
