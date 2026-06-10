//! Idiolect IBus engine binary. Built only with `--features ibus-engine`.

#[tokio::main]
async fn main() -> zbus::Result<()> {
    idiolect_ibus::ibus::run().await
}
