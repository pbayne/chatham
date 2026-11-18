//! `chatham-mcp` — Chatham's MCP server over stdio (#172).
//!
//! Built from the app's own crate so it reuses `db` and the real domain types, and
//! shipped inside the bundle at `Chatham.app/Contents/MacOS/chatham-mcp`. It opens the
//! app's SQLite database directly, which is what makes it work whether or not Chatham
//! is running, with no port, no token and no auth code — the filesystem permissions
//! on the application-support directory are the authorization.
//!
//! Initialization does nothing but open the database and answer. Client handshake
//! budgets go as low as ten seconds, so there is no eager indexing, no embedding
//! warm-up, and emphatically no attempt to launch the app.
//!
//! Everything it prints on stdout is protocol. Diagnostics go to stderr, because a
//! stray line on stdout is a framing error that kills the session.

use humla_lib::mcp;
use rmcp::transport::io::stdio;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() {
    let Some(path) = mcp::db_path() else {
        eprintln!("chatham-mcp: could not locate Chatham's data directory");
        std::process::exit(1);
    };
    // A missing database means Chatham has never run on this machine. Said plainly
    // rather than surfaced as a SQLite error, because the fix is "open Chatham once",
    // and because opening would otherwise CREATE an empty library and answer every
    // question with a confident, wrong "you have no notes".
    if !path.exists() {
        eprintln!(
            "chatham-mcp: no Chatham library at {} — open Chatham once to create it",
            path.display()
        );
        std::process::exit(1);
    }
    let conn = match humla_lib::mcp::open_db(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chatham-mcp: could not open {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let service = match mcp::server::ChathamMcp::new(conn).serve(stdio()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("chatham-mcp: failed to start: {e}");
            std::process::exit(1);
        }
    };
    // Runs until the client closes stdin, which is how every MCP client stops a
    // stdio server.
    if let Err(e) = service.waiting().await {
        eprintln!("chatham-mcp: stopped: {e}");
    }
}
