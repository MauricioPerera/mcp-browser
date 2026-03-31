// MCP Browser — A browser with built-in MCP server
//
// Architecture:
// 1. Main window: navigation bar (local HTML)
// 2. HTTP server on localhost:3001 for MCP (Axum)
// 3. Navigation command: loads URL in webview with injected MCP bridge script
// 4. MCP bridge: page registers tools → Rust receives → HTTP responds to agents

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use axum::{
    extract::State,
    http::Method,
    response::Json,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, Mutex};
use tower_http::cors::{Any, CorsLayer};

type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>;

#[derive(Clone)]
struct AppState {
    app: AppHandle,
    pending: PendingRequests,
}

#[derive(Debug, Serialize, Deserialize)]
struct NavigatePayload {
    url: String,
}

// The MCP bridge script that gets injected into every page
const MCP_BRIDGE_SCRIPT: &str = r#"
(function() {
  // Wait for Tauri API
  function waitForTauri(cb) {
    if (window.__TAURI__) return cb();
    setTimeout(() => waitForTauri(cb), 100);
  }

  waitForTauri(function() {
    // Prevent double listener registration (but allow tool re-registration)
    var listenerAlreadySet = !!window.__MCP_BRIDGE_LOADED__;
    window.__MCP_BRIDGE_LOADED__ = true;

    const { listen } = window.__TAURI__.event;
    const { invoke } = window.__TAURI__.core;

    // MCP tool registry — preserve custom tools if already registered
    if (!window.__MCP_TOOLS__) window.__MCP_TOOLS__ = [];
    if (!window.__MCP_HANDLERS__) window.__MCP_HANDLERS__ = {};

    // Public API for web pages to register tools
    window.mcpTool = function(name, description, schema, handler) {
      window.__MCP_TOOLS__.push({
        name: name,
        description: description,
        inputSchema: { type: 'object', properties: schema || {} }
      });
      window.__MCP_HANDLERS__[name] = handler;
      console.log('[MCP Browser] Tool registered: ' + name);
    };

    window.mcpResource = function(uri, description, handler) {
      // TODO: resources
    };

    // Handle MCP requests from Rust backend
    async function handleMcpRequest(body) {
      try {
        var result;
        switch (body.method) {
          case 'initialize':
            result = {
              protocolVersion: '2024-11-05',
              capabilities: { tools: {} },
              serverInfo: { name: document.title || 'MCP Browser', version: '0.1.0' },
            };
            break;
          case 'notifications/initialized':
            return null;
          case 'tools/list':
            result = { tools: window.__MCP_TOOLS__ || [] };
            break;
          case 'tools/call':
            var name = body.params.name;
            var args = body.params.arguments || {};
            var handler = window.__MCP_HANDLERS__[name];
            if (!handler) throw new Error('Tool not found: ' + name);
            var raw = await handler(args);
            result = { content: [{ type: 'text', text: typeof raw === 'string' ? raw : JSON.stringify(raw) }] };
            break;
          case 'ping':
            result = {};
            break;
          default:
            throw new Error('Unsupported method: ' + body.method);
        }
        if (result === null) return null;
        return { jsonrpc: '2.0', id: body.id, result: result };
      } catch (e) {
        return { jsonrpc: '2.0', id: body.id, error: { code: -32603, message: e.message } };
      }
    }

    // Listen for MCP requests from Rust (only once)
    if (!listenerAlreadySet) listen('mcp-request', async function(event) {
      var body = event.payload;
      var response = await handleMcpRequest(body);
      if (response) {
        invoke('mcp_response', { id: String(body.id), response: response });
      }
    });

    console.log('[MCP Browser] Bridge loaded. Use mcpTool() to register tools.');

    // Auto-register basic tools (skip if already registered by a previous injection)
    if (window.__MCP_HANDLERS__['page_title']) {
      console.log('[MCP Browser] Auto-tools already present, skipping.');
    } else {
    mcpTool('page_title', 'Get the page title', {}, function() {
      return document.title;
    });
    mcpTool('page_url', 'Get the current URL', {}, function() {
      return window.location.href;
    });
    mcpTool('page_text', 'Get page text content', {}, function() {
      return document.body.innerText.slice(0, 5000);
    });
    mcpTool('page_html', 'Get page HTML', {}, function() {
      return document.documentElement.outerHTML.slice(0, 10000);
    });
    mcpTool('query_selector', 'Query DOM elements', { selector: { type: 'string' } }, function(args) {
      var els = document.querySelectorAll(args.selector);
      return JSON.stringify(Array.from(els).slice(0, 20).map(function(el) {
        return { tag: el.tagName, id: el.id, text: (el.textContent || '').slice(0, 100) };
      }));
    });
    mcpTool('navigate', 'Navigate to a URL', { url: { type: 'string' } }, async function(args) {
      await invoke('navigate', { url: args.url });
      return 'Navigating to ' + args.url;
    });
    } // end auto-tools guard
  });
})();
"#;

// Tauri command: navigate to URL and re-inject MCP bridge
#[tauri::command]
async fn navigate(app: AppHandle, url: String) -> Result<String, String> {
    let window = app.get_webview_window("main")
        .ok_or("Window not found")?;

    let parsed_url: tauri::Url = url.parse()
        .map_err(|e: url::ParseError| format!("Invalid URL: {}", e))?;

    window.navigate(parsed_url)
        .map_err(|e| format!("Navigation failed: {}", e))?;

    // Re-inject MCP bridge after navigation completes
    // Use a delay to wait for the new page to load
    let win = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
        let _ = win.eval(MCP_BRIDGE_SCRIPT);
        println!("MCP bridge re-injected after navigation");
        // Retry injection in case page was slow
        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
        let _ = win.eval(MCP_BRIDGE_SCRIPT);
    });

    Ok(format!("Navigating to {}", url))
}

// Tauri command: WebView sends MCP response back to HTTP server
#[tauri::command]
async fn mcp_response(
    id: String,
    response: serde_json::Value,
    state: tauri::State<'_, PendingRequests>,
) -> Result<(), String> {
    let mut pending = state.lock().await;
    if let Some(sender) = pending.remove(&id) {
        sender.send(response).map_err(|_| "Failed to send response".to_string())?;
    }
    Ok(())
}

// HTTP handler: receives JSON-RPC from agents
async fn handle_mcp(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let id = body.get("id").map(|v| v.to_string()).unwrap_or_else(|| "0".to_string());

    let (tx, rx) = oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        pending.insert(id.clone(), tx);
    }

    if let Err(e) = state.app.emit("mcp-request", body.clone()) {
        return Json(serde_json::json!({
            "jsonrpc": "2.0", "id": body.get("id"),
            "error": { "code": -32603, "message": format!("WebView error: {}", e) }
        }));
    }

    match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
        Ok(Ok(response)) => Json(response),
        Ok(Err(_)) => Json(serde_json::json!({
            "jsonrpc": "2.0", "id": body.get("id"),
            "error": { "code": -32603, "message": "Response channel dropped" }
        })),
        Err(_) => Json(serde_json::json!({
            "jsonrpc": "2.0", "id": body.get("id"),
            "error": { "code": -32603, "message": "Timeout" }
        })),
    }
}

fn main() {
    let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
    let pending_clone = pending.clone();

    tauri::Builder::default()
        .manage(pending.clone())
        .invoke_handler(tauri::generate_handler![navigate, mcp_response])
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Inject MCP bridge into initial page
            let window = app.get_webview_window("main").unwrap();
            let _ = window.eval(MCP_BRIDGE_SCRIPT);

            // Start HTTP MCP server
            let state = AppState {
                app: app_handle,
                pending: pending_clone,
            };

            tauri::async_runtime::spawn(async move {
                let cors = CorsLayer::new()
                    .allow_methods([Method::POST, Method::OPTIONS])
                    .allow_origin(Any)
                    .allow_headers(Any);

                let router = Router::new()
                    .route("/mcp", post(handle_mcp))
                    .layer(cors)
                    .with_state(state);

                let listener = match tokio::net::TcpListener::bind("127.0.0.1:4567").await {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("Port 9876 busy: {}. Trying 9877...", e);
                        tokio::net::TcpListener::bind("127.0.0.1:4568").await
                            .expect("Failed to bind MCP server on 9876 or 9877")
                    }
                };

                println!("MCP server: http://127.0.0.1:4567/mcp");
                axum::serve(listener, router).await.expect("MCP server failed");
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running application");
}
