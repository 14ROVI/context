# Roadmap

[Wiki home](README.md)

## Working today

- A Tauri 2 app on cross-platform crates for capture, shortcuts, the tray, secret storage and MCP, with HTML pages for the windows. The only OS-specific code is one Windows process flag, described in [Architecture](architecture.md#cross-platform-approach).
- Whole-display capture on demand, with focused-app metadata, analyzed through [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source).
- Native notifications for the answer, *nothing to add* or a problem, with progress in the tray tooltip.
- A main window with onboarding, day-grouped encrypted history with the screenshot behind each result, and clear.
- `context capture` for desktops where apps cannot register global shortcuts.
- [Sources](sources.md): MCP servers over stdio or HTTP that the model can query, limited to read-only tools unless the user lists others. Cards and history name the sources used.

Verified on Windows 11 on 17 November 2026 with a live ChatGPT plan. Sign-in, model listing (`gpt-6-luna` chosen), a vision request and a calendar tool call all worked end to end against a demo MCP server. GitHub's hosted MCP server also worked over streamable HTTP, with a token from an environment variable, 28 read-only tools and two chained `issue_read` calls. Context's own GitHub OAuth app has since been checked live using device flow: GitHub accepted authorization and Context loaded 28 read-only tools. The plan accepted function tools with `store: false`, and the model returned no reasoning items between rounds. macOS and Linux builds have not been checked.

On the same date, Google's desktop OAuth sign-in and a real Calendar event-list request succeeded. One follow-up checked both Google Calendar and GitHub, returning the event count and the repository's current commit. Both encrypted source grants survived an app restart, with Google Calendar's one tool and GitHub's 28 tools available. Google refresh and non-empty event results have not been checked live.

The Tauri UI replaced a GPUI one on 17 November 2026. GPUI needed Win32 calls to keep the card from taking focus, could not size windows to their content, and drew large images incompletely on Windows. On Windows 11 the Tauri card kept the user's app focused through a live capture, sized itself to each phase, and opened the main window on its result. The main window shows saved screenshots in full.

Later that day native notifications replaced the corner card. On Windows 11 a debug build showed the answer's headline and summary as a toast, labelled *Windows PowerShell* because a debug build has no registered app ID. The installed app, macOS and Linux have not been checked.

## Next

Work through these in order unless something more urgent comes up. Tick items off here as they land.

### Ready for other people

- [x] **Release build.** The manual **Release (Windows)** action builds the NSIS installer and can create a draft release. Not done: code signing, and an auto-updater, which needs public releases.
- [x] **Settings screen.** Provider, model, shortcut, privacy, automatic cards and start at login.
- [ ] **Account edge cases.** Token refresh is verified with a live account, and usage-limit errors link to the usage page. Still unverified: the real usage-limit response, and sign-out with token revocation.
- [ ] **macOS and Linux.** Build and check on macOS, X11 and Wayland. Deferred: Windows only for now.

### Easy sources

- [x] **OAuth for remote MCP servers.** *Needs sign-in* and **Sign in** per source, with discovery, dynamic client registration, PKCE, refresh and encrypted grants. Verified against Notion up to the consent page. Not done: Client ID Metadata Documents, which need a hosted page.
- [x] **Source catalog.** Featured sources, MCP Registry search, and an add form whose secrets are stored encrypted as `${secret:…}`. Not done: `.mcpb` bundles and Docker packages.
- [x] **Context's OAuth apps.** GitHub device flow and Google's Desktop app client are configured, their credentials are saved as repository secrets, and both browser sign-ins have completed live. GitHub token expiration is disabled. See [Sources](sources.md#contexts-oauth-apps).
- [ ] **Built-in calendar connectors.** Google Calendar's OAuth sign-in, event-list request and persisted grant are verified live. Microsoft 365 is not started. A public Google release needs OAuth verification; the app currently allows listed test users only.
- [x] **Source suggestions.** When an answer needed a source that isn't connected, the notification names the kind to connect in Settings.

### Product depth

- [x] **Automatic cards (experimental).** Off by default. New text in the focused window is found with a local pixel diff and OCR, filtered, and checked by a cheap text-only model before any screenshot is analysed. Well-known games are skipped. Verified on Windows: a new chat question produced a card about 10 seconds later, without taking focus. Tune from daily use: how often cards help, how often they interrupt, and how much usage they consume. Not yet handled: telling the user's own new messages apart from other people's, beyond what the gate model infers.
- [x] **Privacy controls.** Excluded apps, an option not to keep screenshots, and history that clears after a set number of days.
- [x] **Web search.** On by default through each provider's own search tool, with cited pages saved in history and a clear note when an answer needed search it couldn't use. Verified on Windows on 17 November 2026 with a ChatGPT plan: a TradingView NVDA chart produced a card explaining the after-hours move from NVIDIA's quarterly results, citing NVIDIA's investor page. Not yet checked live: Anthropic search, the search-off path, and clicking a cited page.
- [x] **More model providers.** OpenAI and Anthropic API keys alongside the ChatGPT plan. Not yet tested with live keys.

### Cleanup

- [x] Remove the demo calendar from the local `mcp.json`.
- [x] Review the light theme in both windows.
- [x] Add a `.gitattributes` file so line endings stay consistent across platforms.
