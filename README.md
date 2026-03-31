# MCP Browser

Custom browser with built-in MCP server -- any page you visit exposes tools to AI agents.

**Download:** [GitHub Releases](https://github.com/MauricioPerera/mcp-browser/releases) (Windows, macOS, Linux)

## What is this?

MCP Browser is a Tauri-based desktop browser (Rust + WebView) that runs an HTTP MCP server on localhost. When you navigate to any web page, the browser automatically injects a bridge script that registers tools accessible to AI agents via the Model Context Protocol. Web pages can also register their own custom tools using the `mcpTool()` API.

The result: point the browser at any URL, and an AI agent can immediately read, query, and interact with that page through standard MCP tool calls.

## How it works

```
Agent (curl / SDK)
       |
       | POST /mcp (JSON-RPC)
       v
  HTTP Server (Axum, port 4567)
       |
       | oneshot channel
       v
  Rust Backend (Tauri)
       |
       | emit("mcp-request")
       v
  Tauri IPC Bridge
       |
       | event listener
       v
  WebView (injected JS bridge)
       |
       | window.__MCP_HANDLERS__[toolName](args)
       v
  JS Tool Handler (auto or custom)
       |
       | invoke("mcp_response")
       v
  Rust Backend --> HTTP Response --> Agent
```

1. An agent sends a JSON-RPC request to `POST http://127.0.0.1:4567/mcp`
2. The Axum HTTP server receives it and emits a Tauri event (`mcp-request`) into the WebView
3. The injected MCP bridge script handles the request, dispatching to the registered tool handler
4. The tool handler runs in the page context (full DOM access) and returns a result
5. The result travels back through Tauri IPC to the HTTP server, which responds to the agent

## Verified test results

| Test | Result |
|------|--------|
| Navigate to `example.com` | `page_title` returns `"Example Domain"` |
| Navigate to `browser-mcp.pages.dev/mcp-shop-demo.html` | `tools/list` returns 12 tools (6 auto + 6 custom) |
| `shop_search` with query `"phones"` | Returns iPhone 17 and Samsung Galaxy S26 |
| `shop_add_to_cart` | Cart updated successfully |

## Auto-registered tools

Every page gets these 6 tools injected automatically:

| Tool | Description |
|------|-------------|
| `page_title` | Get the page title |
| `page_url` | Get the current URL |
| `page_text` | Get page text content (first 5000 chars) |
| `page_html` | Get page HTML (first 10000 chars) |
| `query_selector` | Query DOM elements by CSS selector (returns up to 20 matches) |
| `navigate` | Navigate to a URL |

## Custom tools

Web pages can register their own tools using the `mcpTool()` API. The bridge script exposes this globally in every page:

```js
mcpTool("search", "Search products", { query: { type: "string" } },
  (args) => searchProducts(args.query)
);
```

**Signature:** `mcpTool(name, description, inputSchemaProperties, handler)`

- `name` -- tool name (string)
- `description` -- what the tool does (string)
- `inputSchemaProperties` -- object describing parameters (JSON Schema `properties` format)
- `handler` -- async or sync function receiving `{ args }`, returns string or object

Custom tools appear alongside auto-registered tools in `tools/list` responses.

## MCP endpoint

```
POST http://127.0.0.1:4567/mcp
Content-Type: application/json
```

Standard JSON-RPC 2.0 over HTTP. Supported methods:

- `initialize` -- handshake, returns server info and capabilities
- `tools/list` -- list all registered tools (auto + custom)
- `tools/call` -- invoke a tool by name with arguments
- `ping` -- health check

**Example:**

```bash
# List tools
curl -s -X POST http://127.0.0.1:4567/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

# Get page title
curl -s -X POST http://127.0.0.1:4567/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"page_title","arguments":{}}}'

# Navigate
curl -s -X POST http://127.0.0.1:4567/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"https://example.com"}}}'
```

## Download

Pre-built binaries are available on [GitHub Releases](https://github.com/MauricioPerera/mcp-browser/releases):

| Platform | Format | File |
|----------|--------|------|
| **Windows** | Installer | `MCP Browser_x.x.x_x64-setup.exe` |
| **Windows** | MSI | `MCP Browser_x.x.x_x64_en-US.msi` |
| **macOS (Apple Silicon)** | DMG | `MCP Browser_x.x.x_aarch64.dmg` |
| **macOS (Intel)** | DMG | `MCP Browser_x.x.x_x64.dmg` |
| **Linux** | Debian | `mcp-browser_x.x.x_amd64.deb` |
| **Linux** | AppImage | `mcp-browser_x.x.x_amd64.AppImage` |

### Quick start

1. Download the installer for your platform from [Releases](https://github.com/MauricioPerera/mcp-browser/releases)
2. Install and launch MCP Browser
3. Navigate to any URL
4. From another terminal, test the MCP server:

```bash
curl -s -X POST http://127.0.0.1:4567/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

### Connect with Claude Code

```bash
claude mcp add mcp-browser --transport http --url http://127.0.0.1:4567/mcp
```

Now Claude Code can read, query, and interact with any page open in MCP Browser.

## Build from source

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Tauri CLI v2](https://v2.tauri.app/start/create-project/) -- `cargo install tauri-cli --version "^2"`
- Platform-specific WebView dependencies (see [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))

### Build and run

```bash
cd src-tauri
cargo tauri dev      # development mode with hot reload
cargo tauri build    # production build
```

Or build directly with cargo:

```bash
cd src-tauri
cargo build          # debug build
cargo build --release
```

### Stack

- **Runtime:** Tauri v2 (Rust + system WebView)
- **HTTP server:** Axum 0.7 with tower-http CORS
- **IPC:** Tauri event system + command handlers
- **Protocol:** MCP over HTTP (JSON-RPC 2.0), protocol version `2024-11-05`

## Relation to browser-mcp.js

This project shares the same `mcpTool()` API as [browser-mcp.js](https://browser-mcp.pages.dev), the browser extension that turns any Chrome tab into an MCP server. The key difference is the runtime:

| | browser-mcp.js | MCP Browser (this project) |
|---|---|---|
| Runtime | Chrome extension | Tauri desktop app |
| Transport | stdio | HTTP (localhost:4567) |
| WebView | Chrome tab | System WebView (WebView2/WebKit) |
| Tool injection | Content script | Tauri eval injection |

Web pages written for browser-mcp.js work in MCP Browser without changes -- the `mcpTool()` call signature is identical.

## License

MIT

## Author

Mauricio Perera
