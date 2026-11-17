# Sources

[Wiki home](README.md)

Sources let an answer go beyond the screen. A source is an [MCP](https://modelcontextprotocol.io) server, or a connector built into Context such as Google Calendar. When the screen raises a question a source can answer, such as a friend asking whether you're free on Monday, the model calls that source's tools before it writes the card.

## Add a source

Open **Settings** and go to **Sources**.

- **Featured** sources connect in one step. Choose **Connect**, then sign in on the provider's page in your browser. Notion, Linear, Sentry and Figma register Context with themselves automatically. Google Calendar and GitHub use Context's own registered apps; see [Context's OAuth apps](#contexts-oauth-apps).
- **Search** finds servers in the public [MCP Registry](https://registry.modelcontextprotocol.io). Hosted servers connect over streamable HTTP. npm and PyPI packages run with `npx` or `uvx`, which need Node.js or uv installed. Context shows the address or command before adding, and asks for any settings the server needs. Registry servers are run by third parties, so check one before adding it.
- When an answer needed a source you have not connected, the card offers **Connect calendar** or similar. That opens Settings at Sources, with the catalog filtered to that kind.

Connected sources show their tools, or *Needs sign-in* with a **Sign in** button. **Sign out** forgets the sign-in, and **Remove** deletes the source with its sign-in and saved settings.

## mcp.json

Featured and search results are saved to `mcp.json`, which you can also edit by hand. Choose **Edit** under **Sources** in the sidebar, save, then choose **Reload**.

| Platform | `mcp.json` location |
| --- | --- |
| Windows | `%APPDATA%\Context\config\mcp.json` |
| macOS | `~/Library/Application Support/Context/mcp.json` |
| Linux | `~/.config/context/mcp.json` |

The file uses the same `mcpServers` shape as other MCP clients:

```json
{
  "mcpServers": {
    "notion": { "url": "https://mcp.notion.com/mcp" },
    "github": {
      "url": "https://api.githubcopilot.com/mcp/readonly",
      "headers": { "Authorization": "Bearer ${GITHUB_TOKEN}" }
    },
    "calendar": {
      "command": "npx",
      "args": ["-y", "your-calendar-mcp-server"],
      "env": { "CALENDAR_TOKEN": "${secret:calendar/CALENDAR_TOKEN}" }
    },
    "team-docs": {
      "url": "https://example.com/mcp",
      "oauth": { "clientId": "…", "scopes": "read" },
      "tools": ["search_documents", "get_document"]
    }
  }
}
```

| Field | Meaning |
| --- | --- |
| `command`, `args`, `env` | Start a local server that speaks MCP over stdio. Commands resolve on `PATH`, including `npx.cmd`-style launchers on Windows. |
| `url`, `headers` | Connect to a remote server over streamable HTTP. |
| `oauth` | A registered OAuth app, for a server that cannot register Context itself: `clientId`, optional `clientSecret` and `scopes`. |
| `tools` | The exact tools the model may call. Without it, see below. |
| `disabled` | `true` keeps the entry but skips it. |

Values can reference secrets instead of holding them:

- `${secret:NAME}` is a value Context stores encrypted. Settings typed into the add form are stored this way.
- `${NAME}` is an environment variable, read at launch. An unset variable shows as that source's error.

Values written directly in `mcp.json` are plain text. Prefer tokens scoped to read access. Context's own built-in connectors, such as Google Calendar, are listed in `settings.json`, not `mcp.json`.

## Signing in

Remote servers that answer `401` show *Needs sign-in*. Signing in follows the MCP authorization specification:

1. Read the server's protected resource metadata, from the `resource_metadata` address in its 401 or the well-known location.
2. Read the authorization server's metadata, trying the RFC 8414 and OpenID locations.
3. Use the `oauth` client from `mcp.json`, Context's own app for that provider, or register Context with the provider (dynamic client registration).
4. Open the provider's page in the browser, with PKCE, the server as the `resource`, and the scopes the server's 401 asked for. A one-time listener on `127.0.0.1` receives the result.

Sign-ins are stored encrypted, one per server address, and refreshed before they expire. Client ID Metadata Documents (CIMD) are not used yet, because they need a page Context hosts on the web.

GitHub uses its [device flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow) instead of a callback. Context opens GitHub in your browser and keeps a one-time code visible in Settings until sign-in finishes. Enter that code and approve Context. It polls at GitHub's requested interval, slows down when asked, and reports declined or expired sign-ins.

## Google Calendar

The built-in connector asks Google for read access to events only (`calendar.events.readonly`). It offers the model one tool, `list_events`, for up to 31 days at a time on the primary calendar, in local time. Events marked free and invitations you declined are labelled.

## Context's OAuth apps

Google and GitHub do not let apps register themselves, so Context ships its own OAuth apps. Their credentials are compiled in from environment variables, and are never committed. GitHub's device flow needs only its public client ID. Google's desktop client also requires a client secret, which cannot be confidential in a desktop app.

| Variable | App |
| --- | --- |
| `CONTEXT_GOOGLE_CLIENT_ID`, `CONTEXT_GOOGLE_CLIENT_SECRET` | A Google Cloud OAuth client of type **Desktop app**, in a project with the Google Calendar API enabled. The consent screen must list the `calendar.events.readonly` scope. While the app is in testing, only listed test users can sign in. A public release needs Google's verification. |
| `CONTEXT_GITHUB_CLIENT_ID` | A GitHub OAuth app with **Enable Device Flow** checked. No client secret is needed. Keep **Token expiration** disabled under Optional features. Context asks for `repo read:org`; these OAuth scopes include write access, while Context's read-only MCP endpoint and tool policy restrict model calls. |

The release workflow reads these as repository secrets. For a local build, set them in the shell before `cargo build`, or before launching Context. Without them, Google Calendar shows as unavailable and GitHub asks for a personal access token instead.

For Google, use a separate Cloud project, enable the Google Calendar API, configure the consent screen and add your account as a test user, then create a **Desktop app** client. Add its ID and secret as the two repository secrets above. Google [supports loopback callbacks and PKCE for desktop apps](https://developers.google.com/identity/protocols/oauth2/native-app). While an external app is in Testing, Calendar [refresh tokens expire after seven days](https://developers.google.com/identity/protocols/oauth2#expiration); sign in again when that happens.

The registered apps are **Context** on GitHub and **Context desktop** in Google Cloud project `context-515016`. The Google app remains in Testing, with the maintainer's account listed as a test user. The three repository secrets above are configured for release builds.

### GitHub without OAuth

GitHub's hosted MCP server also accepts a personal access token. Without Context's GitHub app, the featured GitHub entry asks for one and stores it encrypted. A fine-grained token with read access to the repositories you want, or the output of `gh auth token`, works.

### Protocol versions

Context offers the newest MCP protocol version first. If the handshake fails, it retries once offering `2025-11-25`, the last version that uses `initialize`. GitHub's hosted server needs this: it negotiates down from the newest version, then rejects its own session with `400 invalid session`.

## What the model may call

Context decides which tools the model sees:

- With a `tools` list, only those tools are offered.
- Without one, only tools the server marks read-only (`readOnlyHint`) are offered.
- Built-in connectors are read-only.

Settings and the sidebar show how many tools each source offers and how many it hides. To allow a tool that changes data, list it in `tools`. Do that only for tools you trust the model to call without asking.

Each capture allows up to 8 tool calls over 4 model turns, with a 20-second limit per call. Tool output is cut at 12,000 characters. The model treats tool results as untrusted data, like the screen, and is told not to use a tool for something it was not built for.

## While Context answers

The card shows what Context is checking, for example *Checking Google Calendar…*. The answer names the sources it used, on the card and in history. If the plan or model refuses tools, Context answers from the screen alone.

| Code | Owns |
| --- | --- |
| [`src/sources/mod.rs`](../src/sources/mod.rs) | The list of sources, the tool policy, calls, sign-in and sign-out. |
| [`src/sources/mcp.rs`](../src/sources/mcp.rs) | `mcp.json`, MCP connections, 401 detection and `${…}` values. |
| [`src/sources/oauth.rs`](../src/sources/oauth.rs) | Discovery, client registration, PKCE, GitHub device flow, refresh and stored grants. |
| [`src/sources/google.rs`](../src/sources/google.rs) | The Google Calendar connector. |
| [`src/sources/catalog.rs`](../src/sources/catalog.rs) | Featured sources, MCP Registry search, and adding or removing servers. |
| [`ui/sources.js`](../ui/sources.js) | The Sources section of Settings. |
