use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHello {
    pub client_name: String,
    pub protocol_version: u16,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub accepted_features: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ClientHello, ServerHello};

    #[test]
    fn hello_messages_round_trip_with_protocol_version() {
        let hello = ClientHello {
            client_name: "idiolect-fcitx5".to_owned(),
            protocol_version: 1,
            features: vec!["preedit".to_owned(), "commit".to_owned()],
        };
        let json = serde_json::to_string(&hello).expect("hello should serialize");
        let decoded: ClientHello = serde_json::from_str(&json).expect("hello should deserialize");
        assert_eq!(decoded.protocol_version, 1);

        let ack = ServerHello {
            protocol_version: 1,
            accepted_features: vec!["preedit".to_owned()],
        };
        let ack_json = serde_json::to_string(&ack).expect("server hello should serialize");
        let decoded_ack: ServerHello =
            serde_json::from_str(&ack_json).expect("server hello should deserialize");
        assert_eq!(decoded_ack.accepted_features, ["preedit"]);
    }
}
