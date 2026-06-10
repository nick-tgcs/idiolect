#pragma once

#include <memory>
#include <string>

#include <fcitx-config/configuration.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/handlertable.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/instance.h>

#include "engine.h"
#include "ipc_client.h"

namespace idiolect::fcitx5 {

/// Configurable hotkeys for the addon. Persisted to
/// ~/.config/fcitx5/conf/idiolect.conf and editable via fcitx5-configtool.
FCITX_CONFIGURATION(
    IdiolectConfig,
    fcitx::Option<fcitx::KeyList> toggleKey{
        this, "ToggleKey", _("Toggle dictation"), {fcitx::Key("Super+T")}};
    fcitx::Option<fcitx::KeyList> cancelKey{
        this, "CancelKey", _("Cancel dictation"), {fcitx::Key("Escape")}};
    fcitx::Option<std::string> socketPath{
        this, "SocketPath",
        _("Daemon socket path (empty = $XDG_RUNTIME_DIR/idiolect.sock)"), ""};);

/// Commits recognized text into the currently focused application by calling
/// fcitx5's InputContext::commitString. The target is captured when a take
/// stops; if it is gone by the time the transcript arrives, it falls back to
/// the most-recently-focused input context.
class Fcitx5TextCommitter final : public TextCommitter {
public:
    explicit Fcitx5TextCommitter(fcitx::Instance* instance) : instance_(instance) {}

    void setTarget(fcitx::InputContext* ic);
    void commit(const std::string& text) override;

private:
    fcitx::Instance* instance_;
    fcitx::TrackableObjectReference<fcitx::InputContext> target_;
};

/// fcitx5 background module: registers a global hotkey that toggles dictation
/// against the idiolect daemon and types the result into the focused app.
class IdiolectModule final : public fcitx::AddonInstance {
public:
    explicit IdiolectModule(fcitx::Instance* instance);
    ~IdiolectModule() override;

    void reloadConfig() override;
    const fcitx::Configuration* getConfig() const override { return &config_; }
    void setConfig(const fcitx::RawConfig& raw) override;

private:
    bool ensureConnected();
    void teardownConnection();
    void onKeyEvent(fcitx::KeyEvent& keyEvent);
    void onSocketReadable();
    std::string resolveSocketPath() const;

    fcitx::Instance* instance_;
    IdiolectConfig config_;
    std::unique_ptr<UnixSocketIpcClient> ipc_;
    std::unique_ptr<Fcitx5TextCommitter> committer_;
    std::unique_ptr<Engine> engine_;
    std::unique_ptr<fcitx::EventSourceIO> ioEvent_;
    std::unique_ptr<fcitx::EventSource> deferredTeardown_;
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> keyHandler_;
};

class IdiolectModuleFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance* create(fcitx::AddonManager* manager) override;
};

} // namespace idiolect::fcitx5
