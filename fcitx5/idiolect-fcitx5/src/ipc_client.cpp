#include "ipc_client.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
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

std::string extract_text_payload(const std::string& line) {
    const std::string prefix = "\"text\":\"";
    const auto start = line.find(prefix);
    if (start == std::string::npos) {
        throw std::runtime_error("preedit update missing text payload");
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

    throw std::runtime_error("preedit update text payload is unterminated");
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
    return {"preedit", "commit"};
}

UnixSocketIpcClient::UnixSocketIpcClient(const std::string& socket_path) {
    socket_fd_ = ::socket(AF_UNIX, SOCK_STREAM, 0);
    if (socket_fd_ < 0) {
        throw system_error("socket");
    }

    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    if (socket_path.size() >= sizeof(address.sun_path)) {
        ::close(socket_fd_);
        socket_fd_ = -1;
        throw std::runtime_error("socket path is too long");
    }
    std::strncpy(address.sun_path, socket_path.c_str(), sizeof(address.sun_path) - 1);

    if (::connect(socket_fd_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
        const auto error = system_error("connect");
        ::close(socket_fd_);
        socket_fd_ = -1;
        throw error;
    }

    send_json_line(
        "{\"type\":\"ClientHello\",\"payload\":{\"client_name\":\"idiolect-fcitx5\","
        "\"protocol_version\":1,\"features\":[\"preedit\",\"commit\"]}}\n");

    const std::string server_hello = read_json_line();
    require_contains(server_hello, "\"type\":\"ServerHello\"", "server should send ServerHello");
    require_contains(server_hello, "\"protocol_version\":1", "server should accept protocol version 1");

    negotiated_protocol_version_ = client_protocol_version();
    accepted_features_ = parse_accepted_features(server_hello);
}

UnixSocketIpcClient::~UnixSocketIpcClient() {
    if (socket_fd_ >= 0) {
        ::close(socket_fd_);
    }
}

void UnixSocketIpcClient::start_recording() {
    send_json_line("{\"type\":\"StartRecording\"}\n");
}

void UnixSocketIpcClient::commit_preedit(const std::string& text) {
    send_json_line("{\"type\":\"CommitPreedit\",\"payload\":{\"text\":\"" + json_escape(text) +
                   "\"}}\n");
}

void UnixSocketIpcClient::cancel_preedit() {
    send_json_line("{\"type\":\"CancelPreedit\"}\n");
}

std::string UnixSocketIpcClient::read_preedit_update() {
    const std::string line = read_json_line();
    require_contains(line, "\"type\":\"PreeditUpdate\"", "server should send PreeditUpdate");
    return extract_text_payload(line);
}

std::uint16_t UnixSocketIpcClient::negotiated_protocol_version() const {
    return negotiated_protocol_version_;
}

const std::vector<std::string>& UnixSocketIpcClient::accepted_features() const {
    return accepted_features_;
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
