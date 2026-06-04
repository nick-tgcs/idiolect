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
    virtual void reconnect() = 0;
};

class UnixSocketIpcClient final : public IpcClient {
public:
    explicit UnixSocketIpcClient(const std::string& socket_path);
    ~UnixSocketIpcClient() override;

    UnixSocketIpcClient(const UnixSocketIpcClient&) = delete;
    UnixSocketIpcClient& operator=(const UnixSocketIpcClient&) = delete;

    void start_recording() override;
    void commit_preedit(const std::string& text) override;
    void cancel_preedit() override;
    void reconnect() override;

    std::string read_preedit_update();

    [[nodiscard]] std::uint16_t negotiated_protocol_version() const;
    [[nodiscard]] const std::vector<std::string>& accepted_features() const;

private:
    void connect_and_negotiate(const std::string& socket_path);
    void close_socket();
    void send_json_line(const std::string& line) const;
    std::string read_json_line() const;

    std::string socket_path_;
    int socket_fd_ = -1;
    std::uint16_t negotiated_protocol_version_ = 0;
    std::vector<std::string> accepted_features_;
};

} // namespace idiolect::fcitx5
