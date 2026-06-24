#pragma once

#include <string>

#include "ipc_client.h"

namespace idiolect::fcitx5 {

enum class DisconnectPreeditPolicy {
    Clear,
    Preserve,
};

/// Sink for recognized text: the real implementation types it into the focused
/// application via fcitx5; tests use a fake. Mirrors the IpcClient abstraction
/// so the Engine's logic is unit-testable without fcitx5 installed.
class TextCommitter {
public:
    virtual ~TextCommitter() = default;
    virtual void commit(const std::string& text) = 0;
};

/// Recording phase. `Idle` and `Recording` mirror the daemon's authoritative
/// recording state — the engine never sets them optimistically; they change only
/// when [`Engine::on_recording_status`] arrives. The daemon owns the microphone,
/// so it is the single source of truth.
enum class RecordingState {
    Idle,      ///< The daemon reports no recording in progress.
    Recording, ///< The daemon reports the microphone is open.
};

class Engine {
public:
    explicit Engine(IpcClient& ipc_client);
    Engine(IpcClient& ipc_client, DisconnectPreeditPolicy disconnect_policy);
    Engine(IpcClient& ipc_client, TextCommitter& committer);
    Engine(IpcClient& ipc_client, TextCommitter& committer,
           DisconnectPreeditPolicy disconnect_policy);

    // Toggle-based dictation (used by the fcitx5 module).
    /// Send one direction-free toggle intent to the daemon. The daemon decides
    /// start-vs-stop and announces the result via on_recording_status; the engine
    /// never flips its own phase optimistically.
    void toggle();
    /// The daemon's authoritative recording state changed (its RecordingStatus
    /// push). This is the single source of truth for the Idle/Recording phase.
    void on_recording_status(bool recording);
    /// Called when the daemon delivers the final transcript: commit it into the
    /// focused app (via the committer) and tell the daemon to finalize.
    void on_transcript(std::string text);
    /// Called for each streamed mid-take snippet (a PARTIAL preedit): commit it
    /// into the focused app and keep recording — the daemon finalizes the
    /// merged take itself at stop, so nothing is sent back.
    void on_partial_transcript(std::string text);
    /// Abort an in-progress take.
    void cancel();
    /// The daemon reported an error mid-take; return to idle.
    void on_error();
    [[nodiscard]] RecordingState state() const;

    // Lower-level operations retained for existing call sites/tests.
    void start_recording();
    void receive_preedit(std::string text);
    void commit_preedit();
    void cancel_preedit();
    void recover_from_daemon_disconnect();

    [[nodiscard]] const std::string& visible_preedit() const;

private:
    IpcClient& ipc_client_;
    TextCommitter* committer_ = nullptr;
    DisconnectPreeditPolicy disconnect_policy_;
    RecordingState state_ = RecordingState::Idle;
    std::string visible_preedit_;
};

} // namespace idiolect::fcitx5
