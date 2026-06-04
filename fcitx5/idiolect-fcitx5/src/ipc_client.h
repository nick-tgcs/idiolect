#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace idiolect::fcitx5 {

std::uint16_t client_protocol_version();
std::vector<std::string> client_features();

class IpcClient {
public:
    virtual ~IpcClient() = default;

    virtual void start_recording() = 0;
    virtual void commit_preedit(const std::string& text) = 0;
    virtual void cancel_preedit() = 0;
};

} // namespace idiolect::fcitx5
