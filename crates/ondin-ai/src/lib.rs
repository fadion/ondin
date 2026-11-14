//! The assistant's agent harnesses (`docs/design/assistant.md` §2–§4): finding a coding
//! agent's program, sealing it so it can use Ondin's MCP tools and nothing
//! else, driving its headless mode, and decoding its stream into [`Event`]s.
//!
//! Ondin holds no model and no key: the harness owns the sign-in. Every child
//! runs in a fresh run folder of its session's own, marked with
//! [`AGENT_CHILD_ENV`], with the person's other configuration set aside where
//! the harness allows it. Any skip-permissions or allow-all option is never
//! passed: a headless harness cannot ask, so its allow-list is exactly the
//! tools the chat's access level grants.

mod antigravity;
mod cursor;
mod find;
mod harness;
mod process;
mod run;
mod session;
mod stream;

pub use find::{Found, NotFound, find, warm};
pub use harness::{Harness, Seal};
pub use process::process_running;
pub use run::sweep;
pub use session::{Config, Event, Refusal, Session, Turn};

/// Set to `1` on every harness child, so `ondin` can refuse to run under one.
pub const AGENT_CHILD_ENV: &str = "ONDIN_AGENT_CHILD";

/// The name Ondin's MCP server is registered under; its tools reach Claude Code
/// as `mcp__ondin__<tool>`. Antigravity's registry is the person's own, so a
/// session there registers under this name and eight hex digits.
pub const MCP_SERVER: &str = "ondin";
