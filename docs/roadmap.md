# Roadmap

[Wiki home](README.md)

## Working today

- A Tauri 2 app on cross-platform crates for capture, shortcuts, the tray, secret storage and MCP, with HTML pages for the windows. The only OS-specific code is one Windows process flag, described in [Architecture](architecture.md#cross-platform-approach).
- Whole-display capture on demand, with focused-app metadata, analyzed through [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source).
- A corner card that shows progress, the answer, *nothing to add* or a problem, without taking focus.
- A main window with onboarding, day-grouped encrypted history with the screenshot behind each result, copy, delete and clear.
- `context capture` for desktops where apps cannot register global shortcuts.
- [Sources](sources.md): MCP servers over stdio or HTTP that the model can query, limited to read-only tools unless the user lists others. Cards and history name the sources used.

Verified on Windows 11 on 17 November 2026 with a live ChatGPT plan. Sign-in, model listing (`gpt-6-luna` chosen), a vision request and a calendar tool call all worked end to end against a demo MCP server. GitHub's hosted MCP server also worked over streamable HTTP, with a token from an environment variable, 28 read-only tools and two chained `issue_read` calls. The plan accepted function tools with `store: false`, and the model returned no reasoning items between rounds. macOS and Linux builds have not been checked.

The Tauri UI replaced a GPUI one on 17 November 2026. GPUI needed Win32 calls to keep the card from taking focus, could not size windows to their content, and drew large images incompletely on Windows. On Windows 11 the Tauri card kept the user's app focused through a live capture, sized itself to each phase, and opened the main window on its result. The main window shows saved screenshots in full.

## Next

1. Check plan limit behavior, sign-out with token revocation, and token refresh after the access token expires.
2. Build and check on macOS, X11 and Wayland, including the card's focus behavior and tray menus.
3. Prove sources against real servers: calendar availability (for example, "are you free Monday?" in a chat), a team thread or document lookup, and public research. Add OAuth sign-in for remote MCP servers, with tokens in the credential store. Servers without automatic client registration, such as GitHub, need a pre-registered OAuth app.
4. Evaluate OCR, accessibility text and local image change detection as triggers for automatic cards. Count model and tool requests, suppress duplicates and limit background use. Keep the shortcut as a manual trigger.

The first daily-use trial should measure how often cards arrive in time to help, how often they interrupt, and how much usage they consume. Set the automatic trigger defaults from that trial.
