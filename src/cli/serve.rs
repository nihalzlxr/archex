pub async fn run() -> anyhow::Result<()> {
    crate::core::mcp::start_server().await
}