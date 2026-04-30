use crate::core::mcp;

pub fn run() {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
    rt.block_on(async {
        if let Err(e) = mcp::start_server().await {
            eprintln!("Server error: {}", e);
        }
    });
}