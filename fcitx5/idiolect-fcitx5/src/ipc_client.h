#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace idiolect::fcitx5 {

std::uint16_t client_protocol_version();
std::vector<std::string> client_features();

class IpcClient {
public:
    virtual ~IpcClient() = default;

    /// One direction-free "toggle the recording" intent; the daemon decides
    /// start-vs-stop and announces the result via a RecordingStatus push.
    virtual void toggle_recording() = 0;
    virtual void start_recording() = 0;
    virtual void stop_recording() = 0;
    virtual void commit_preedit(const std::string& text) = 0;
    virtual void cancel_preedit() = 0;
    virtual void reconnect() = 0;
};

/// A message received from the daemon, classified for the event-loop reader.
enum class ServerMessageKind {
    Preedit,         ///< PreeditUpdate — `text` holds the transcript to commit.
    Error,           ///< Error — `text` holds the human-readable message.
    RecordingStatus, ///< RecordingStatus — `recording` holds the authoritative state.
    Other,           ///< Anything else (e.g. ServerHello); ignored by the addon.
};

struct ServerMessage {
    ServerMessageKind kind;
    std::string text;
    bool recording = false; ///< Meaningful only when `kind == RecordingStatus`.
};

class UnixSocketIpcClient final : public IpcClient {
public:
    explicit UnixSocketIpcClient(const std::string& socket_path);
    ~UnixSocketIpcClient() override;

    UnixSocketIpcClient(const UnixSocketIpcClient&) = delete;
    UnixSocketIpcClient& operator=(const UnixSocketIpcClient&) = delete;

    void toggle_recording() override;
    void start_recording() override;
    void stop_recording() override;
    void commit_preedit(const std::string& text) override;
    void cancel_preedit() override;
    void reconnect() override;

    /// Blocking read of a single PreeditUpdate (used by tests).
    std::string read_preedit_update();

    /// The connected socket fd, for registering with an external event loop.
    [[nodiscard]] int fd() const;

    /// Non-blocking: drains available bytes and returns the next complete
    /// server message if one is buffered, or nullopt if more data is needed.
    /// Throws if the daemon closed the connection or the socket errored.
    std::optional<ServerMessage> poll_server_message();

    [[nodiscard]] std::uint16_t negotiated_protocol_version() const;
    [[nodiscard]] const std::vector<std::string>& accepted_features() const;

private:
    void connect_and_negotiate(const std::string& socket_path);
    void close_socket();
    void send_json_line(const std::string& line) const;
    std::string read_json_line() const;

    std::string socket_path_;
    int socket_fd_ = -1;
    std::string read_buffer_;
    std::uint16_t negotiated_protocol_version_ = 0;
    std::vector<std::string> accepted_features_;
};

} // namespace idiolect::fcitx5
