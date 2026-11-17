# Sources

[Wiki home](README.md)

Sources let an answer go beyond the screen. Each source is an [MCP](https://modelcontextprotocol.io) server. When the screen raises a question a source can answer, such as a friend asking whether you're free on Monday, the model calls that source's tools before it writes the card.

## Add a source

Choose **Edit** under **Sources** in the sidebar. Context opens `mcp.json`, creating it first if needed. Add servers, save, then choose **Reload**.

| Platform | `mcp.json` location |
| --- | --- |
| Windows | `%APPDATA%\Context\config\mcp.json` |
| macOS | `~/Library/Application Support/Context/mcp.json` |
| Linux | `~/.config/context/mcp.json` |

The file uses the same `mcpServers` shape as other MCP clients:

```json
{
  "mcpServers": {
    "github": {
      "url": "https://api.githubcopilot.com/mcp/readonly",
      "headers": { "Authorization": "Bearer ${GITHUB_TOKEN}" }
    },
    "calendar": {
      "command": "npx",
      "args": ["-y", "your-calendar-mcp-server"],
      "env": { "CALENDAR_TOKEN": "${CALENDAR_TOKEN}" }
    },
    "team-docs": {
      "url": "https://example.com/mcp",
      "tools": ["search_documents", "get_document"]
    }
  }
}
```

| Field | Meaning |
| --- | --- |
| `command`, `args`, `env` | Start a local server that speaks MCP over stdio. Commands resolve on `PATH`, including `npx.cmd`-style launchers on Windows. |
| `url`, `headers` | Connect to a remote server over streamable HTTP. |
| `tools` | The exact tools the model may call. Without it, see below. |
| `disabled` | `true` keeps the entry but skips it. |

Any value can reference an environment variable as `${NAME}`. Use this for tokens, so `mcp.json` holds no secrets. Context reads the variable at launch, and an unset variable shows as that source's error. Set it as a user environment variable, then restart Context. Values written directly in `mcp.json` are stored as plain text. Prefer tokens scoped to read access.

Remote servers that need an OAuth sign-in are not supported yet. For those, use a token header where the server allows it.

### GitHub

GitHub hosts an official MCP server. Its `/readonly` endpoint offers only read tools, which all carry `readOnlyHint`. The `github` entry above is enough. For `GITHUB_TOKEN`, use a fine-grained personal access token with read access to the repositories you want, or the output of `gh auth token` if you use the GitHub CLI.

### Protocol versions

Context offers the newest MCP protocol version first. If the handshake fails, it retries once offering `2025-11-25`, the last version that uses `initialize`. GitHub's hosted server needs this. It negotiates down from the newest version, then rejects its own session with `400 invalid session`.

## What the model may call

Context decides which tools the model sees:

- With a `tools` list, only those tools are offered.
- Without one, only tools the server marks read-only (`readOnlyHint`) are offered.

The sidebar shows how many tools each source offers and how many it hides. To allow a tool that changes data, list it in `tools`. Do that only for tools you trust the model to call without asking.

Each capture allows up to 8 tool calls over 4 model turns, with a 20-second limit per call. Tool output is cut at 12,000 characters. The model treats tool results as untrusted data, like the screen.

## While Context answers

The card shows what Context is checking, for example *Checking calendar…*. The answer names the sources it used, on the card and in history. If the ChatGPT plan or model refuses tools, Context answers from the screen alone.

Code: [`src/sources.rs`](../src/sources.rs) owns configuration, connections and the tool policy. [`src/chatgpt/responses.rs`](../src/chatgpt/responses.rs) runs the tool loop.
