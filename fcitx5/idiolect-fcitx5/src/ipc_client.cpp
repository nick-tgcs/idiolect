#include "ipc_client.h"

namespace idiolect::fcitx5 {

std::uint16_t client_protocol_version() {
    return 1;
}

std::vector<std::string> client_features() {
    return {"preedit", "commit"};
}

} // namespace idiolect::fcitx5
