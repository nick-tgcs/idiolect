#include "ipc_client.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace idiolect::fcitx5 {
namespace {

std::runtime_error system_error(const std::string& action) {
    return std::runtime_error(action + ": " + std::strerror(errno));
}

void require_contains(const std::string& line, const std::string& needle,
                      const std::string& message) {
    if (line.find(needle) == std::string::npos) {
        throw std::runtime_error(message);
    }
}

std::string json_escape(const std::string& input) {
    std::string output;
    output.reserve(input.size());

    for (const char character : input) {
        switch (character) {
        case '"':
            output += "\\\"";
            break;
        case '\\':
            output += "\\\\";
            break;
        case '\n':
            output += "\\n";
            break;
        case '\r':
            output += "\\r";
            break;
        case '\t':
            output += "\\t";
            break;
        default:
            output.push_back(character);
            break;
        }
    }

    return output;
}

std::string extract_string_field(const std::string& line, const std::string& key) {
    const std::string prefix = "\"" + key + "\":\"";
    const auto start = line.find(prefix);
    if (start == std::string::npos) {
        throw std::runtime_error("message missing \"" + key + "\" field");
    }

    std::string output;
    bool escaping = false;
    for (std::size_t index = start + prefix.size(); index < line.size(); ++index) {
        const char character = line[index];
        if (escaping) {
            switch (character) {
            case 'n':
                output.push_back('\n');
                break;
            case 'r':
                output.push_back('\r');
                break;
            case 't':
                output.push_back('\t');
                break;
            default:
                output.push_back(character);
                break;
            }
            escaping = false;
            continue;
        }
        if (character == '\\') {
            escaping = true;
            continue;
        }
        if (character == '"') {
            return output;
        }
        output.push_back(character);
    }

    throw std::runtime_error("string field \"" + key + "\" is unterminated");
}

/// Extract a JSON boolean field (e.g. `"recording":true`). Unlike a string field
/// the value is an unquoted literal, so we match `true`/`false` after the key.
bool extract_bool_field(const std::string& line, const std::string& key) {
    const std::string prefix = "\"" + key + "\":";
    const auto start = line.find(prefix);
    if (start == std::string::npos) {
        throw std::runtime_error("message missing \"" + key + "\" field");
    }
    return line.find("true", start + prefix.size()) == start + prefix.size();
}

std::vector<std::string> parse_accepted_features(const std::string& line) {
    std::vector<std::string> features;
    for (const auto& feature : client_features()) {
        if (line.find("\"" + feature + "\"") != std::string::npos) {
            features.push_back(feature);
        }
    }
    return features;
}

} // namespace

std::uint16_t client_protocol_version() {
    return 1;
}

std::vector<std::string> client_features() {
    // The daemon is authoritative for recording state; ask for its pushes.
    return {"preedit", "commit", "recording_status"};
}

UnixSocketIpcClient::UnixSocketIpcClient(const std::string& socket_path) : socket_path_(socket_path) {
    connect_and_negotiate(socket_path_);
}

UnixSocketIpcClient::~UnixSocketIpcClient() {
    close_socket();
}

void UnixSocketIpcClient::toggle_recording() {
    send_json_line("{\"type\":\"ToggleRecording\"}\n");
}

void UnixSocketIpcClient::start_recording() {
    send_json_line("{\"type\":\"StartRecording\"}\n");
}

void UnixSocketIpcClient::stop_recording() {
    send_json_line("{\"type\":\"StopRecording\"}\n");
}

void UnixSocketIpcClient::commit_preedit(const std::string& text) {
    send_json_line("{\"type\":\"CommitPreedit\",\"payload\":{\"text\":\"" + json_escape(text) +
                   "\"}}\n");
}

void UnixSocketIpcClient::cancel_preedit() {
    send_json_line("{\"type\":\"CancelPreedit\"}\n");
}

void UnixSocketIpcClient::reconnect() {
    close_socket();
    connect_and_negotiate(socket_path_);
}

std::string UnixSocketIpcClient::read_preedit_update() {
    const std::string line = read_json_line();
    require_contains(line, "\"type\":\"PreeditUpdate\"", "server should send PreeditUpdate");
    return extract_string_field(line, "text");
}

int UnixSocketIpcClient::fd() const {
    return socket_fd_;
}

std::optional<ServerMessage> UnixSocketIpcClient::poll_server_message() {
    // Drain whatever is available without blocking, accumulating into the line
    // buffer. The event loop calls this in a loop until it returns nullopt.
    char chunk[512];
    while (true) {
        const ssize_t got = ::recv(socket_fd_, chunk, sizeof(chunk), MSG_DONTWAIT);
        if (got > 0) {
            read_buffer_.append(chunk, static_cast<std::size_t>(got));
            continue;
        }
        if (got == 0) {
            throw std::runtime_error("daemon closed the connection");
        }
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            break; // no more data right now
        }
        if (errno == EINTR) {
            continue;
        }
        throw system_error("recv");
    }

    const auto newline = read_buffer_.find('\n');
    if (newline == std::string::npos) {
        return std::nullopt;
    }
    const std::string line = read_buffer_.substr(0, newline + 1);
    read_buffer_.erase(0, newline + 1);

    if (line.find("\"type\":\"PreeditUpdate\"") != std::string::npos) {
        return ServerMessage{ServerMessageKind::Preedit, extract_string_field(line, "text"), false};
    }
    if (line.find("\"type\":\"Error\"") != std::string::npos) {
        return ServerMessage{ServerMessageKind::Error, extract_string_field(line, "message"), false};
    }
    if (line.find("\"type\":\"RecordingStatus\"") != std::string::npos) {
        return ServerMessage{ServerMessageKind::RecordingStatus, {},
                             extract_bool_field(line, "recording")};
    }
    return ServerMessage{ServerMessageKind::Other, {}, false};
}

std::uint16_t UnixSocketIpcClient::negotiated_protocol_version() const {
    return negotiated_protocol_version_;
}

const std::vector<std::string>& UnixSocketIpcClient::accepted_features() const {
    return accepted_features_;
}

void UnixSocketIpcClient::connect_and_negotiate(const std::string& socket_path) {
    socket_fd_ = ::socket(AF_UNIX, SOCK_STREAM, 0);
    if (socket_fd_ < 0) {
        throw system_error("socket");
    }

    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    if (socket_path.size() >= sizeof(address.sun_path)) {
        close_socket();
        throw std::runtime_error("socket path is too long");
    }
    std::strncpy(address.sun_path, socket_path.c_str(), sizeof(address.sun_path) - 1);

    if (::connect(socket_fd_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
        const auto error = system_error("connect");
        close_socket();
        throw error;
    }

    send_json_line(
        "{\"type\":\"ClientHello\",\"payload\":{\"client_name\":\"idiolect-fcitx5\","
        "\"protocol_version\":1,\"features\":[\"preedit\",\"commit\",\"recording_status\"]}}\n");

    const std::string server_hello = read_json_line();
    require_contains(server_hello, "\"type\":\"ServerHello\"", "server should send ServerHello");
    require_contains(server_hello, "\"protocol_version\":1", "server should accept protocol version 1");

    negotiated_protocol_version_ = client_protocol_version();
    accepted_features_ = parse_accepted_features(server_hello);
}

void UnixSocketIpcClient::close_socket() {
    if (socket_fd_ >= 0) {
        ::close(socket_fd_);
        socket_fd_ = -1;
    }
}

void UnixSocketIpcClient::send_json_line(const std::string& line) const {
    const char* data = line.data();
    std::size_t remaining = line.size();
    while (remaining > 0) {
        const ssize_t written = ::send(socket_fd_, data, remaining, 0);
        if (written <= 0) {
            throw system_error("send");
        }
        data += written;
        remaining -= static_cast<std::size_t>(written);
    }
}

std::string UnixSocketIpcClient::read_json_line() const {
    std::string line;
    char byte = '\0';
    while (true) {
        const ssize_t read = ::recv(socket_fd_, &byte, 1, 0);
        if (read <= 0) {
            throw system_error("recv");
        }
        line.push_back(byte);
        if (byte == '\n') {
            return line;
        }
    }
}

} // namespace idiolect::fcitx5
