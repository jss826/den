# Chat v2 Design — Channel-based Architecture

## Problem

The current Chat tab wraps Claude Code with `claude -p --stream-json`.
This is fundamentally unstable because `-p` is designed for single-shot execution,
not interactive conversation. Key issues:

- Process dies after each response, requiring `--continue` to restart every turn
- No `/compact`, `/cost`, `/model` or other slash commands
- Permission handling requires a custom MCP gate (complex, fragile)
- Context overflow = session death with no recovery
- Interrupt = process kill, not graceful pause

## Solution

Replace the entire Chat implementation with Claude Code's official
**Channels API** (v2.1.80+). Den becomes a channel server that pushes
messages into a running interactive Claude Code session.

### Core principle

**Den does not manage Claude Code's lifecycle.**
Claude Code manages itself as a normal interactive process.
Den only pushes messages in and receives replies through the channel protocol.

## Architecture

```
┌─ Chat UI (browser) ───────────────────────────┐
│  Message input + display + permission dialogs  │
└────────────────┬──────────────────────────────┘
                 │ HTTP/WS
┌─ den backend ──┴──────────────────────────────┐
│  /api/channel/message   ← UI sends message    │
│  /api/channel/poll      ← channel server gets  │
│  /api/channel/reply     ← channel server posts │
│  /api/channel/ws        ← UI receives replies  │
│  /api/channel/permission ← permission relay    │
│  /api/channel/verdict   ← UI sends decision   │
│  Message queue + WS broadcast                  │
└────────────────┬──────────────────────────────┘
                 │ HTTP (localhost)
┌─ den-channel ──┴──────────────────────────────┐
│  MCP server (den --channel-server)             │
│  Capabilities:                                 │
│    claude/channel (push messages)              │
│    claude/channel/permission (relay approvals) │
│  Tools:                                        │
│    reply (Claude sends messages back)          │
└────────────────┬──────────────────────────────┘
                 │ stdio (MCP JSON-RPC 2.0)
┌─ Claude Code ──┴──────────────────────────────┐
│  claude --channels server:den-channel          │
│  Interactive mode (NOT -p)                     │
│  All features work: /compact /cost /model etc  │
└────────────────────────────────────────────────┘
```

## Components

### 1. den-channel (`src/channel.rs`, `den --channel-server`)

MCP server that Claude Code spawns as a subprocess. Communicates with
Claude Code via stdio (JSON-RPC 2.0) and with den backend via HTTP.

**Capabilities:**
- `claude/channel`: registers notification listener
- `claude/channel/permission`: opts into permission relay
- `tools`: exposes `reply` tool

**Environment variables (set by den when generating MCP config):**
- `DEN_CHANNEL_API_URL`: den backend URL (e.g., `http://127.0.0.1:3131`)
- `DEN_CHANNEL_TOKEN`: random token for authentication
- `DEN_CHANNEL_SESSION_ID`: session identifier

**Behavior:**
1. On startup: connect to Claude Code via stdio, declare capabilities
2. Poll `GET /api/channel/poll` for pending messages from Chat UI
3. When message available: emit `notifications/claude/channel` to Claude Code
4. When Claude calls `reply` tool: POST reply to `/api/channel/reply`
5. When permission_request received: POST to `/api/channel/permission`
6. Poll `/api/channel/verdict` for user's decision, emit back to Claude Code

**Implementation:** Rust (part of den binary, like existing `--mcp-gate`).
MCP protocol is JSON-RPC 2.0 over stdin/stdout — straightforward to implement.

### 2. den backend channel API (`src/chat/`)

Lightweight message broker between Chat UI and den-channel.

**State:**
```rust
struct ChannelState {
    /// Pending messages from UI → channel server
    message_queue: Mutex<VecDeque<ChannelMessage>>,
    /// Pending permission requests from channel server → UI
    permission_requests: Mutex<HashMap<String, PermissionRequest>>,
    /// Broadcast channel for replies → UI WebSocket
    reply_tx: broadcast::Sender<String>,
    /// Session token for authenticating channel server
    token: String,
}
```

**Endpoints:**

| Endpoint | Method | From | To | Purpose |
|----------|--------|------|----|---------|
| `/api/channel/message` | POST | Chat UI | queue | User sends message |
| `/api/channel/poll` | GET | den-channel | queue | Fetch pending messages |
| `/api/channel/reply` | POST | den-channel | WS broadcast | Claude's reply |
| `/api/channel/ws` | WS | Chat UI | — | Real-time replies + permissions |
| `/api/channel/permission` | POST | den-channel | WS broadcast | Permission prompt |
| `/api/channel/verdict` | POST | Chat UI | den-channel (via poll) | Approve/deny |

### 3. Chat UI (`frontend/js/chat.js`)

Simplified frontend. No stream-json parsing, no process lifecycle management.

**Responsibilities:**
- Send messages via `POST /api/channel/message`
- Receive replies via WebSocket (`/api/channel/ws`)
- Render replies with DenMarkdown
- Display permission request cards
- Send verdicts via `POST /api/channel/verdict`
- Session settings (permission mode, auto-approve rules)
- Browser push notifications for permission requests

**Not responsible for (unlike current chat.js):**
- Process spawn/kill
- stream-json event parsing
- Session state tracking (idle/thinking/streaming)
- MCP gate management
- Auto-restart logic
- History persistence (Claude Code handles this)

### 4. Claude Code startup

Den starts Claude Code in a PTY session (using existing PTY infrastructure):

```
claude --channels server:den-channel --permission-mode <user-choice> --verbose
```

The PTY session is a special terminal session tagged as "chat-backend".
It could be visible in the Terminal tab for debugging, or hidden.

## Permission handling

### 3-layer approach

**Layer 1: Permission Mode** (Claude Code native)

Selected by user when starting a Chat session:
- `default`: all operations require approval
- `acceptEdits`: file edits auto-approved, Bash requires approval
- `auto`: classifier judges each action (Team plan required)
- `bypassPermissions`: all auto-approved (isolated environments only)

Passed as `--permission-mode <mode>` when starting Claude Code.

**Layer 2: Permission Relay** (Channel protocol)

When a permission prompt occurs (even in auto mode when classifier blocks):
1. Claude Code sends `permission_request` to den-channel
2. den-channel forwards to den backend
3. Chat UI shows approval card with tool name, description, input preview
4. User clicks Allow/Deny
5. Verdict flows back to Claude Code

**Layer 3: Auto-decide / Queue**

When user is not present:
- Default: **queue** — permission request stays pending, Claude blocks
- Optional: auto-decide based on session allowlist
- Push notification sent to browser

### Session allowlist

User can check "Allow all X this session" when approving:
- den-channel maintains a session-scoped allowlist
- Matching future requests auto-approved without UI round-trip
- Allowlist cleared when session ends

## What gets deleted

| File | Lines | Description |
|------|-------|-------------|
| `src/chat/manager.rs` | ~926 | ChatManager, ChatSession, process spawn |
| `src/chat/api.rs` | ~822 | stream-json relay, WS handler |
| `src/chat/permission.rs` | ~144 | MCP gate state management |
| `src/mcp_gate.rs` | ~300 | Permission gate MCP server |
| `frontend/js/chat.js` | ~2483 | stream-json parsing, state management |
| **Total** | **~4675** | |

## What gets created

| File | Est. lines | Description |
|------|-----------|-------------|
| `src/channel.rs` | ~200 | MCP server (JSON-RPC over stdio) |
| `src/chat/mod.rs` | ~50 | Module definition |
| `src/chat/api.rs` | ~300 | Channel API endpoints + WS |
| `src/chat/state.rs` | ~100 | ChannelState (message queue, broadcast) |
| `frontend/js/chat.js` | ~600 | Send/receive + permission UI + settings |
| **Total** | **~1250** | |

## Implementation phases

### Phase 1: Verification

Build minimal den-channel + backend API. Test with:
```
claude --dangerously-load-development-channels server:den-channel
```

Verify:
- [x] Message push: UI → den-channel → Claude Code
- [x] Reply: Claude Code → reply tool → den-channel → UI
- [x] Permission relay: request → UI → verdict → Claude Code

### Phase 2: Chat UI rebuild

- Delete old Chat code
- Build new Chat UI (message list, input, markdown rendering)
- Permission approval cards + push notifications
- Session settings (permission mode selection)

### Phase 3: Integration

- Auto-start Claude Code from Chat tab (PTY spawn)
- Session management (start/stop/reconnect)
- Remote/Relay proxy for channel API

## References

- [Claude Code Channels](https://code.claude.com/docs/en/channels)
- [Channels Reference](https://code.claude.com/docs/en/channels-reference)
- [Permission Modes](https://code.claude.com/docs/en/permission-modes)
- [Remote Control](https://code.claude.com/docs/en/remote-control)

## Prior art

- **cmux**: Terminal MUX, Claude Code runs natively, `send`/`read-screen` for control
- **TAKT**: YAML workflow, `-p` for single-shot tasks chained via files
- **Collaborator**: Electron + tmux session persistence
- All three avoid wrapping Claude Code with `-p` for interactive use
