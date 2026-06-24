#include "idiolect_module.h"

#include <cstdlib>
#include <exception>
#include <string>

#include <fcitx-config/iniparser.h>
#include <fcitx-utils/log.h>
#include <fcitx/addonmanager.h>

namespace idiolect::fcitx5 {
namespace {

constexpr char kConfigPath[] = "conf/idiolect.conf";

std::string default_socket_path() {
    if (const char* runtime = std::getenv("XDG_RUNTIME_DIR");
        runtime != nullptr && runtime[0] != '\0') {
        return std::string(runtime) + "/idiolect.sock";
    }
    if (const char* home = std::getenv("HOME"); home != nullptr && home[0] != '\0') {
        return std::string(home) + "/.local/run/idiolect/idiolect.sock";
    }
    return "/tmp/idiolect.sock";
}

} // namespace

void Fcitx5TextCommitter::setTarget(fcitx::InputContext* ic) {
    target_ = ic != nullptr ? ic->watch() : fcitx::TrackableObjectReference<fcitx::InputContext>{};
}

void Fcitx5TextCommitter::commit(const std::string& text) {
    fcitx::InputContext* ic = target_.get();
    if (ic == nullptr) {
        ic = instance_->mostRecentInputContext();
    }
    if (ic != nullptr && !text.empty()) {
        ic->commitString(text);
    }
}

IdiolectModule::IdiolectModule(fcitx::Instance* instance) : instance_(instance) {
    reloadConfig();
    committer_ = std::make_unique<Fcitx5TextCommitter>(instance_);

    // Global hotkey: watch key events before any input method sees them, so the
    // toggle works regardless of which IM is active.
    keyHandler_ = instance_->watchEvent(
        fcitx::EventType::InputContextKeyEvent, fcitx::EventWatcherPhase::PreInputMethod,
        [this](fcitx::Event& event) { onKeyEvent(static_cast<fcitx::KeyEvent&>(event)); });
}

IdiolectModule::~IdiolectModule() {
    teardownConnection();
}

std::string IdiolectModule::resolveSocketPath() const {
    const std::string& configured = config_.socketPath.value();
    return configured.empty() ? default_socket_path() : configured;
}

bool IdiolectModule::ensureConnected() {
    if (ipc_) {
        return true;
    }
    try {
        ipc_ = std::make_unique<UnixSocketIpcClient>(resolveSocketPath());
    } catch (const std::exception& error) {
        FCITX_ERROR() << "idiolect: cannot reach daemon at " << resolveSocketPath() << ": "
                      << error.what();
        ipc_.reset();
        return false;
    }

    engine_ = std::make_unique<Engine>(*ipc_, *committer_);
    ioEvent_ = instance_->eventLoop().addIOEvent(
        ipc_->fd(), fcitx::IOEventFlag::In,
        [this](fcitx::EventSourceIO*, int, fcitx::IOEventFlags) {
            onSocketReadable();
            return true;
        });
    return true;
}

void IdiolectModule::teardownConnection() {
    ioEvent_.reset();
    engine_.reset();
    ipc_.reset();
}

void IdiolectModule::onKeyEvent(fcitx::KeyEvent& keyEvent) {
    if (keyEvent.isRelease()) {
        return;
    }

    const fcitx::Key key = keyEvent.key();
    if (key.checkKeyList(config_.toggleKey.value())) {
        if (!ensureConnected()) {
            keyEvent.filterAndAccept();
            return;
        }
        // Commit into the context where the user pressed the key.
        committer_->setTarget(keyEvent.inputContext());
        engine_->toggle();
        keyEvent.filterAndAccept();
        return;
    }

    // Only swallow the cancel key while a take is in progress, so Escape behaves
    // normally everywhere else.
    if (engine_ && engine_->state() != RecordingState::Idle &&
        key.checkKeyList(config_.cancelKey.value())) {
        engine_->cancel();
        keyEvent.filterAndAccept();
    }
}

void IdiolectModule::onSocketReadable() {
    try {
        while (const auto message = ipc_->poll_server_message()) {
            switch (message->kind) {
            case ServerMessageKind::Preedit:
                // A streamed mid-take snippet types and keeps recording; a
                // take-final transcript commits and finalizes with the daemon.
                // Display-only snippets (partial+review) feed the IBus
                // engine's live review dialog — this addon has no dialog, so
                // it must skip them rather than type review-held text.
                if (message->partial) {
                    if (!message->review) {
                        engine_->on_partial_transcript(message->text);
                    }
                } else {
                    engine_->on_transcript(message->text);
                }
                break;
            case ServerMessageKind::Error:
                FCITX_ERROR() << "idiolect: daemon error: " << message->text;
                engine_->on_error();
                break;
            case ServerMessageKind::RecordingStatus:
                // The daemon is authoritative for recording state; mirror it.
                engine_->on_recording_status(message->recording);
                break;
            case ServerMessageKind::Other:
                break;
            }
        }
    } catch (const std::exception& error) {
        FCITX_INFO() << "idiolect: daemon connection closed (" << error.what()
                     << "); will reconnect on next toggle";
        // Defer teardown so we never destroy the IO source from inside its own
        // callback.
        deferredTeardown_ = instance_->eventLoop().addDeferEvent([this](fcitx::EventSource*) {
            teardownConnection();
            deferredTeardown_.reset();
            return true;
        });
    }
}

void IdiolectModule::reloadConfig() {
    fcitx::readAsIni(config_, kConfigPath);
}

void IdiolectModule::setConfig(const fcitx::RawConfig& raw) {
    config_.load(raw, true);
    fcitx::safeSaveAsIni(config_, kConfigPath);
}

fcitx::AddonInstance* IdiolectModuleFactory::create(fcitx::AddonManager* manager) {
    return new IdiolectModule(manager->instance());
}

} // namespace idiolect::fcitx5

FCITX_ADDON_FACTORY(idiolect::fcitx5::IdiolectModuleFactory)
