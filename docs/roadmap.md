# Roadmap

[Wiki home](README.md)

## Working today

- A GPUI app built on cross-platform crates for capture, shortcuts, the tray, secret storage and MCP. The only OS-specific code is two small Windows workarounds, described in [Architecture](architecture.md#cross-platform-approach).
- Whole-display capture on demand, with focused-app metadata, analyzed through [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source).
- A corner card that shows progress, the answer, *nothing to add* or a problem, without taking focus.
- A main window with onboarding, day-grouped encrypted history, copy, delete and clear.
- `context capture` for desktops where apps cannot register global shortcuts.
- [Sources](sources.md): MCP servers over stdio or HTTP that the model can query, limited to read-only tools unless the user lists others. Cards and history name the sources used.

Verified on Windows 11 on 17 November 2026 with a live ChatGPT plan. Sign-in, model listing (`gpt-6-luna` chosen), a vision request and a calendar tool call all worked end to end against a demo MCP server. GitHub's hosted MCP server also worked over streamable HTTP, with a token from an environment variable, 28 read-only tools and two chained `issue_read` calls. The plan accepted function tools with `store: false`, and the model returned no reasoning items between rounds. macOS and Linux builds have not been checked.

Known issue: the detail view draws the saved screenshot offset. GPUI 0.2 draws images larger than about 640px incompletely on Windows, and the tile workaround is unfinished. A Tauri prototype of the UI is being evaluated on the `tauri-prototype` branch.

## Next

1. Check plan limit behavior, sign-out with token revocation, and token refresh after the access token expires.
2. Build and check on macOS, X11 and Wayland, including the card's focus behavior and tray menus.
3. Prove sources against real servers: calendar availability (for example, "are you free Monday?" in a chat), a team thread or document lookup, and public research. Add OAuth sign-in for remote MCP servers, with tokens in the credential store. Servers without automatic client registration, such as GitHub, need a pre-registered OAuth app.
4. Evaluate OCR, accessibility text and local image change detection as triggers for automatic cards. Count model and tool requests, suppress duplicates and limit background use. Keep the shortcut as a manual trigger.

The first daily-use trial should measure how often cards arrive in time to help, how often they interrupt, and how much usage they consume. Set the automatic trigger defaults from that trial.
