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

Work through these in order unless something more urgent comes up. Tick items off here as they land.

### Ready for other people

- [ ] **Release build.** Turn on Tauri bundling for MSI or NSIS, DMG, and AppImage or deb. Add code signing and an auto-updater, because unsigned builds are blocked by SmartScreen and Gatekeeper.
- [ ] **Settings screen.** Change the capture shortcut and model, start at login, and manage sources without editing `mcp.json` by hand.
- [ ] **Account edge cases.** Check behavior at the plan's usage limit, token refresh after the access token expires, and sign-out with token revocation.
- [ ] **macOS and Linux.** Build and check on macOS, X11 and Wayland: the card staying out of focus, screen-recording permission on macOS, WebKitGTK and the tray on Linux, and `context capture` on Wayland.

### Easy sources

- [ ] **OAuth for remote MCP servers.** Show *Needs sign-in* with a **Sign in** button when a server answers 401. Use Client ID Metadata Documents (CIMD), falling back to dynamic client registration or a pre-registered client, and keep tokens in the credential store. GitHub's server needs a registered GitHub OAuth app.
- [ ] **Source catalog.** A curated list plus search backed by the [MCP Registry](https://registry.modelcontextprotocol.io). Show a form for each server's settings, with secrets kept in the credential store. Consider `.mcpb` bundles for local servers.
- [ ] **Built-in calendar connectors.** Google Calendar and Microsoft 365 with one-click, read-only sign-in, exposed to the model as tools. This needs a Google Cloud project and OAuth verification before a public release.
- [ ] **Suggestions on the card.** When the screen asks something no source can answer, offer the connector, for example *Connect Google Calendar to check this*.

### Product depth

- [ ] **Follow-up questions** on a card or a saved result.
- [ ] **Automatic cards.** Evaluate OCR, accessibility text and local image change detection as triggers. Count model and tool requests, suppress duplicates and limit background use. Keep the shortcut as a manual trigger. Set the defaults from a daily-use trial that measures how often cards arrive in time to help, how often they interrupt, and how much usage they consume.
- [ ] **Privacy controls.** Apps that are never captured, an option not to save screenshots, and history that clears after a set number of days.
- [ ] **More model providers**, such as an API key for Claude or OpenAI, alongside the ChatGPT plan.

### Cleanup

- [ ] Replace the demo calendar in the local `mcp.json`, which points at a temporary folder, with a real source or remove it.
- [ ] Review the light theme in both windows.
- [ ] Add a `.gitattributes` file so line endings stay consistent across platforms.
