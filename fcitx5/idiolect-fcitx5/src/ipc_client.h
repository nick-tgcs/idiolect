#pragma once

#include <string>

namespace idiolect::fcitx5 {

class IpcClient {
public:
    virtual ~IpcClient() = default;

    virtual void start_recording() = 0;
    virtual void commit_preedit(const std::string& text) = 0;
    virtual void cancel_preedit() = 0;
};

} // namespace idiolect::fcitx5
