# texturecraft-automation

Agent-facing automation for TextureCraft (layer L4: engine, render and formats; no UI toolkit).
The protocol and tool reference is [`docs/control-protocol.md`](../../docs/control-protocol.md).

## Public API

```rust
// An in-process session with file access and windowless renders.
pub struct Headless { pub session: Session, .. }
impl Headless {
    pub fn new(session: Session, fs: Filesystem) -> Headless;
    pub fn default_session() -> Result<Session, AutomationError>;          // material ball, 1024
    pub fn command_run(&mut self, id: &str, params: Value) -> Result<Value, AutomationError>;
    pub fn batch(&mut self, steps: &[Value]) -> Value;                      // stops at the first error
    pub fn open(&mut self, path: &str) -> Result<Value, AutomationError>;   // .tcraft or mesh
    pub fn import_mesh(&mut self, path: &str, resolution: Option<u32>, name: Option<&str>) -> Result<Value, AutomationError>;
    pub fn save(&mut self, path: Option<&str>) -> Result<Value, AutomationError>;
    pub fn export(&mut self, params: &Value) -> Result<Value, AutomationError>;
    pub fn render_png(&mut self, o: &RenderOptions) -> Result<(u32, u32, Vec<u8>), AutomationError>;
    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, AutomationError>; // JSON-lines methods
}
pub enum Filesystem { Denied, TrustedLocal, Workspace(AuthorizedWorkspace) }

// Read and write roots as cap-std directory capabilities: `..`, absolute paths, drive
// prefixes, device names and symlink/junction escapes fail.
pub struct AuthorizedWorkspace;
pub fn workspace::authorize_command(id: &str, params: &Value) -> Result<(), AutomationError>;

pub struct TexturecraftMcp;            // rmcp server: ::headless(Headless) | ::bridge(BridgeClient)
pub struct BridgeClient;               // client of `texturecraft --control <port>`
pub mod rpc;                           // serve_lines (stdio), serve_tcp (loopback + token)
pub mod security;                      // tokens, bounded line reads, connection limit
pub mod budgets;                       // reply ceilings: 8 MiB JSON, 5 MiB / 2048 px PNG
```

## Rules

- Untrusted callers (MCP, `serve`) reach files only through `Filesystem::Workspace`; path-taking
  `file.*` commands are refused by `authorize_command`. The CLI's own subcommands use
  `Filesystem::TrustedLocal` (the user typed the paths).
- Renders create the GPU device on first use and return an error when there is no adapter; a
  wgpu panic is caught and reported, and the device is recreated on the next render.
- Tests: `tests/agent_tasks.rs` drives the MCP server in process (tokio duplex) through ten
  agent tasks, the workspace boundary and junk params for every tool.
