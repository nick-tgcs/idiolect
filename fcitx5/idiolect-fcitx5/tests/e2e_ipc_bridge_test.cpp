#include "ipc_client.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>
#include <vector>

namespace {

void require(bool condition, const std::string& message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

std::string temp_socket_path() {
    return "/tmp/idiolect-fcitx5-e2e-" + std::to_string(::getpid()) + ".sock";
}

class TestServer {
public:
    explicit TestServer(std::string socket_path) : socket_path_(std::move(socket_path)) {
        server_fd_ = ::socket(AF_UNIX, SOCK_STREAM, 0);
        require(server_fd_ >= 0, "socket should open");

        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        require(socket_path_.size() < sizeof(address.sun_path), "socket path should fit");
        std::strncpy(address.sun_path, socket_path_.c_str(), sizeof(address.sun_path) - 1);

        ::unlink(socket_path_.c_str());
        require(::bind(server_fd_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
                "server socket should bind");
        require(::listen(server_fd_, 1) == 0, "server socket should listen");
    }

    ~TestServer() {
        if (server_fd_ >= 0) {
            ::close(server_fd_);
        }
        ::unlink(socket_path_.c_str());
    }

    TestServer(const TestServer&) = delete;
    TestServer& operator=(const TestServer&) = delete;

    void run() {
        const int client_fd = ::accept(server_fd_, nullptr, nullptr);
        require(client_fd >= 0, "server should accept client");

        try {
            const std::string hello = read_line(client_fd);
            require(hello.find("\"type\":\"ClientHello\"") != std::string::npos,
                    "client should send ClientHello");
            require(hello.find("\"protocol_version\":1") != std::string::npos,
                    "client should send protocol version 1");
            require(hello.find("\"preedit\"") != std::string::npos,
                    "client should request preedit feature");
            require(hello.find("\"commit\"") != std::string::npos,
                    "client should request commit feature");
            require(hello.find("\"recording_status\"") != std::string::npos,
                    "client should request recording_status feature");

            write_line(client_fd,
                       "{\"type\":\"ServerHello\",\"payload\":{\"protocol_version\":1,"
                       "\"accepted_features\":[\"preedit\",\"commit\",\"recording_status\"]}}\n");

            const std::string start = read_line(client_fd);
            require(start.find("\"type\":\"StartRecording\"") != std::string::npos,
                    "client should start recording");

            write_line(client_fd,
                       "{\"type\":\"PreeditUpdate\",\"payload\":{\"text\":\"restart traffic\"}}\n");

            const std::string commit = read_line(client_fd);
            require(commit.find("\"type\":\"CommitPreedit\"") != std::string::npos,
                    "client should commit preedit");
            require(commit.find("\"text\":\"restart traffic\"") != std::string::npos,
                    "client should commit received text");

            const std::string cancel = read_line(client_fd);
            require(cancel.find("\"type\":\"CancelPreedit\"") != std::string::npos,
                    "client should cancel preedit");
        } catch (...) {
            ::close(client_fd);
            throw;
        }

        ::close(client_fd);
    }

private:
    static std::string read_line(int fd) {
        std::string line;
        char byte = '\0';
        while (true) {
            const ssize_t read = ::recv(fd, &byte, 1, 0);
            require(read > 0, "server should read client line");
            line.push_back(byte);
            if (byte == '\n') {
                return line;
            }
        }
    }

    static void write_line(int fd, const std::string& line) {
        const char* data = line.data();
        std::size_t remaining = line.size();
        while (remaining > 0) {
            const ssize_t written = ::send(fd, data, remaining, 0);
            require(written > 0, "server should write line");
            data += written;
            remaining -= static_cast<std::size_t>(written);
        }
    }

    std::string socket_path_;
    int server_fd_ = -1;
};

} // namespace

int main() {
    const std::string socket_path = temp_socket_path();
    TestServer server(socket_path);
    std::exception_ptr server_error;
    std::thread server_thread([&server, &server_error]() {
        try {
            server.run();
        } catch (...) {
            server_error = std::current_exception();
        }
    });

    try {
        idiolect::fcitx5::UnixSocketIpcClient client(socket_path);
        require(client.negotiated_protocol_version() == idiolect::fcitx5::client_protocol_version(),
                "client should negotiate protocol version 1");
        require(client.accepted_features() == idiolect::fcitx5::client_features(),
                "client should expose accepted features");

        client.start_recording();
        const std::string preedit = client.read_preedit_update();
        require(preedit == "restart traffic", "client should read preedit update");
        client.commit_preedit(preedit);
        client.cancel_preedit();
    } catch (...) {
        server_thread.join();
        throw;
    }

    server_thread.join();
    if (server_error) {
        std::rethrow_exception(server_error);
    }

    return 0;
}
