#include "engine.h"

#include <utility>

namespace idiolect::fcitx5 {

Engine::Engine(IpcClient& ipc_client)
    : Engine(ipc_client, DisconnectPreeditPolicy::Clear) {}

Engine::Engine(IpcClient& ipc_client, DisconnectPreeditPolicy disconnect_policy)
    : ipc_client_(ipc_client), disconnect_policy_(disconnect_policy) {}

Engine::Engine(IpcClient& ipc_client, TextCommitter& committer)
    : Engine(ipc_client, committer, DisconnectPreeditPolicy::Clear) {}

Engine::Engine(IpcClient& ipc_client, TextCommitter& committer,
               DisconnectPreeditPolicy disconnect_policy)
    : ipc_client_(ipc_client), committer_(&committer), disconnect_policy_(disconnect_policy) {}

void Engine::toggle() {
    // One direction-free intent. The daemon decides start-vs-stop and announces
    // the new state via on_recording_status; we never flip the phase locally, so
    // we can never disagree with the daemon.
    ipc_client_.toggle_recording();
}

void Engine::on_recording_status(bool recording) {
    // The daemon is the single authority for recording state; mirror it. (The
    // daemon sends the transcript before announcing recording=false, so a stop's
    // false never races ahead of on_transcript.)
    state_ = recording ? RecordingState::Recording : RecordingState::Idle;
}

void Engine::on_transcript(std::string text) {
    if (state_ != RecordingState::Recording) {
        // Unsolicited or late transcript; ignore to avoid stray commits.
        return;
    }
    visible_preedit_ = std::move(text);
    if (committer_ != nullptr) {
        committer_->commit(visible_preedit_);
    }
    // Tell the daemon to finalize the session (records a training candidate).
    ipc_client_.commit_preedit(visible_preedit_);
    visible_preedit_.clear();
    // The phase deliberately stays Recording: a stop-time transcript is always
    // followed by the daemon's recording=false push, which is what returns the
    // engine to Idle.
}

void Engine::on_partial_transcript(std::string text) {
    if (state_ != RecordingState::Recording) {
        // No live take; ignore a stray/late snippet.
        return;
    }
    // A streamed mid-take snippet: type it and keep recording. The take is ONE
    // conversation — the daemon merges the snippets and finalizes the single
    // session itself at stop, so no commit_preedit is sent here.
    if (committer_ != nullptr) {
        committer_->commit(text);
    }
}

void Engine::cancel() {
    ipc_client_.cancel_preedit();
    visible_preedit_.clear();
    state_ = RecordingState::Idle;
}

void Engine::on_error() {
    visible_preedit_.clear();
    state_ = RecordingState::Idle;
}

RecordingState Engine::state() const {
    return state_;
}

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

void Engine::recover_from_daemon_disconnect() {
    if (disconnect_policy_ == DisconnectPreeditPolicy::Clear) {
        visible_preedit_.clear();
    }
    ipc_client_.reconnect();
}

const std::string& Engine::visible_preedit() const {
    return visible_preedit_;
}

} // namespace idiolect::fcitx5
