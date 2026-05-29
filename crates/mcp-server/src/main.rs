// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use mcp_server::client::{
    HttpControlPlaneClient, HttpControlPlaneClientConfig, ReqwestControlPlaneTransport,
};
use mcp_server::http::{HttpTransportConfig, serve_http};
use mcp_server::server::McpServer;
use std::io;
use std::sync::Arc;

fn main() -> io::Result<()> {
    let transport =
        ReqwestControlPlaneTransport::new().map_err(|e| io::Error::other(e.to_string()))?;
    let server = Arc::new(McpServer::new(HttpControlPlaneClient::new(
        HttpControlPlaneClientConfig::from_env()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
        std::sync::Arc::new(transport),
    )));
    let http_cfg = HttpTransportConfig::from_env()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if http_cfg.enabled {
        let runtime = tokio::runtime::Runtime::new()?;
        return runtime.block_on(serve_http(server, http_cfg));
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    server.serve_stdio(stdin.lock(), &mut stdout)
}
