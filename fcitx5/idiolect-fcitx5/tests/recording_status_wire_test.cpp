// Wire-level coverage for the daemon-authoritative recording protocol on the
// fcitx5 client: the client advertises `recording_status`, sends a direction-free
// `ToggleRecording`, and classifies the daemon's `RecordingStatus` push (including
// the JSON-bool payload) via poll_server_message. Driven against a minimal server
// over a real unix socket.

#include "ipc_client.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cerrno>
#include <chrono>
#include <cstring>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>

namespace {

void require(bool condition, const std::string& message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

std::string temp_socket_path() {
    return "/tmp/idiolect-fcitx5-status-" + std::to_string(::getpid()) + ".sock";
}

std::string read_line(int fd) {
    std::string line;
    char byte = '\0';
    while (true) {
        const ssize_t got = ::recv(fd, &byte, 1, 0);
        require(got > 0, "server should read a line");
        line.push_back(byte);
        if (byte == '\n') {
            return line;
        }
    }
}

void write_line(int fd, const std::string& line) {
    const char* data = line.data();
    std::size_t remaining = line.size();
    while (remaining > 0) {
        const ssize_t written = ::send(fd, data, remaining, 0);
        require(written > 0, "server should write a line");
        data += written;
        remaining -= static_cast<std::size_t>(written);
    }
}

} // namespace

int main() {
    const std::string socket_path = temp_socket_path();

    const int server_fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
    require(server_fd >= 0, "server socket should open");
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    require(socket_path.size() < sizeof(address.sun_path), "socket path should fit");
    std::strncpy(address.sun_path, socket_path.c_str(), sizeof(address.sun_path) - 1);
    ::unlink(socket_path.c_str());
    require(::bind(server_fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
            "server should bind");
    require(::listen(server_fd, 1) == 0, "server should listen");

    std::string toggle_line;
    std::exception_ptr server_error;
    std::thread server_thread([&]() {
        try {
            const int client_fd = ::accept(server_fd, nullptr, nullptr);
            require(client_fd >= 0, "server should accept");

            const std::string hello = read_line(client_fd);
            require(hello.find("\"type\":\"ClientHello\"") != std::string::npos,
                    "expected ClientHello");
            require(hello.find("\"recording_status\"") != std::string::npos,
                    "client should request recording_status");

            write_line(client_fd,
                       "{\"type\":\"ServerHello\",\"payload\":{\"protocol_version\":1,"
                       "\"accepted_features\":[\"preedit\",\"commit\",\"recording_status\"]}}\n");

            // The daemon's authoritative push that the client must mirror.
            write_line(client_fd,
                       "{\"type\":\"RecordingStatus\",\"payload\":{\"recording\":true}}\n");

            // A streamed mid-take snippet (partial), a display-only review-mode
            // snippet (partial+review — fcitx5 must NOT type these; it has no
            // review dialog), and a take-final transcript from an older daemon
            // that never writes the fields.
            write_line(client_fd,
                       "{\"type\":\"PreeditUpdate\",\"payload\":{\"text\":\" and more\","
                       "\"review\":false,\"partial\":true}}\n");
            write_line(client_fd,
                       "{\"type\":\"PreeditUpdate\",\"payload\":{\"text\":\"display only\","
                       "\"review\":true,\"partial\":true}}\n");
            write_line(client_fd,
                       "{\"type\":\"PreeditUpdate\",\"payload\":{\"text\":\"restart traffic\"}}\n");

            // The client's direction-free intent.
            toggle_line = read_line(client_fd);
            ::close(client_fd);
        } catch (...) {
            server_error = std::current_exception();
        }
    });

    try {
        idiolect::fcitx5::UnixSocketIpcClient client(socket_path);

        bool has_status_feature = false;
        for (const auto& feature : client.accepted_features()) {
            if (feature == "recording_status") {
                has_status_feature = true;
            }
        }
        require(has_status_feature, "client should negotiate recording_status");

        std::optional<idiolect::fcitx5::ServerMessage> message;
        for (int attempt = 0; attempt < 500 && !message; ++attempt) {
            message = client.poll_server_message();
            if (!message) {
                std::this_thread::sleep_for(std::chrono::milliseconds(10));
            }
        }
        require(message.has_value(), "client should receive the status push");
        require(message->kind == idiolect::fcitx5::ServerMessageKind::RecordingStatus,
                "message should classify as RecordingStatus");
        require(message->recording, "the JSON-bool payload should decode to true");

        const auto next_message = [&]() {
            std::optional<idiolect::fcitx5::ServerMessage> polled;
            for (int attempt = 0; attempt < 500 && !polled; ++attempt) {
                polled = client.poll_server_message();
                if (!polled) {
                    std::this_thread::sleep_for(std::chrono::milliseconds(10));
                }
            }
            require(polled.has_value(), "client should receive the next push");
            return *polled;
        };

        // A streamed snippet is classified as a PARTIAL preedit…
        const auto partial = next_message();
        require(partial.kind == idiolect::fcitx5::ServerMessageKind::Preedit,
                "snippet should classify as Preedit");
        require(partial.partial, "the snippet must carry partial=true");
        require(!partial.review, "a typed snippet is not display-only");
        require(partial.text == " and more", "snippet text should decode");

        // …a review-mode snippet is display-only (partial AND review)…
        const auto display_only = next_message();
        require(display_only.kind == idiolect::fcitx5::ServerMessageKind::Preedit,
                "display-only snippet should classify as Preedit");
        require(display_only.partial && display_only.review,
                "review-mode snippets carry partial+review");

        // …and a preedit without the fields (an older daemon) is take-final.
        const auto final_preedit = next_message();
        require(final_preedit.kind == idiolect::fcitx5::ServerMessageKind::Preedit,
                "transcript should classify as Preedit");
        require(!final_preedit.partial, "a missing partial field must mean final");
        require(!final_preedit.review, "a missing review field must mean direct");

        client.toggle_recording();
    } catch (...) {
        server_thread.join();
        ::close(server_fd);
        ::unlink(socket_path.c_str());
        throw;
    }

    server_thread.join();
    ::close(server_fd);
    ::unlink(socket_path.c_str());

    if (server_error) {
        std::rethrow_exception(server_error);
    }
    require(toggle_line.find("\"type\":\"ToggleRecording\"") != std::string::npos,
            "client should send ToggleRecording");

    return 0;
}
