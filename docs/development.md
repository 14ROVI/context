# Development

[Wiki home](README.md)

## Run Context

Use a current stable Rust toolchain. From the repository root, run:

```sh
cargo run --locked
```

| Platform | Prerequisites |
| --- | --- |
| Windows | MSVC Rust and Visual Studio Build Tools with C++ desktop tools. The WebView2 runtime ships with Windows 11. |
| macOS | Xcode and its command-line tools. Allow screen recording for Context, or for the terminal launching it, when macOS asks. |
| Linux | A graphical session with a compositor, WebKitGTK 4.1 and a running Secret Service keyring. Wayland needs a screenshot portal. The tray icon needs AppIndicator support. |

No Node.js or Tauri CLI is needed: the pages in `ui/` are plain files that `cargo build` embeds.

On Ubuntu, Context and its dependencies use these development packages:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libssl-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev \
  libdbus-1-dev libpipewire-0.3-dev libwayland-dev libx11-dev libxcb1-dev libxrandr-dev
```

Debug builds on Windows keep a console for logs. Release builds run without one.

## Release a build

The **Release (Windows)** GitHub Action builds the Windows installer on demand. Run it from the repository's Actions tab, or:

```sh
gh workflow run release.yml
gh workflow run release.yml -f publish=true
```

Each run uploads `Context_<version>_x64-setup.exe` as an artifact. With `publish=true` it also creates a draft GitHub release for the version in `tauri.conf.json`. Bump that version, and the one in `Cargo.toml`, before publishing. To build the installer locally, run `npx --yes @tauri-apps/cli@2 build --bundles nsis`. The installer is not code-signed yet, so Windows SmartScreen warns before it runs. There is no auto-updater, because a private repository's releases are not public for an updater to fetch.

## Use Context

1. Choose **Connect ChatGPT** in the window, card or tray. Sign in and allow plan usage in your browser. See [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source) for eligible plans. To use an API key instead, see [Settings](#settings).
2. In any app, press **Ctrl+Alt+Space** (**⌃⌥Space** on macOS). If another app holds it, Context uses **Ctrl+Alt+Shift+Space**. The window shows the active shortcut.
3. A card opens in the corner while Context reads the screen. It shows the answer when ready. Choose **Open details** for the full text, or **Copy**.
4. In the detail view, ask a follow-up question about any result. Context answers with the same screenshot and sources.
5. Optionally, connect [sources](sources.md) such as a calendar so answers can check more than the screen. When an answer needed a source you have not connected, the card offers to connect one.

The **Capture screen** button hides Context and captures the app behind it. On Wayland, add a desktop shortcut that runs `context capture`. Running `context` again brings the open window forward.

Closing the window hides it, and Context keeps running in the tray. Click the tray icon to bring it back. Quit from the window or tray. When a plan or API usage limit is reached, the card links to the usage page.

History keeps the latest 100 results in your local app-data folder. Records are encrypted with a key stored in your system credential store. Results show the app that was in focus when the desktop reports it. Each result keeps the screenshot it came from, encrypted in a `screenshots` subfolder. You can copy or delete a result, or clear all history. Deleting a result deletes its screenshot.

## Settings

Open **Settings** at the bottom of the sidebar. Changes save right away to `settings.json` in the config folder, next to `mcp.json`.

| Section | Options |
| --- | --- |
| Model | A ChatGPT plan, an OpenAI API key, or an Anthropic API key for Claude. API keys are encrypted like the account and never shown again. Pick a model, or keep the default: Context picks a vision model on a plan, and Claude Opus 5.5 on Anthropic. |
| Shortcut | Pick a preset or type one such as `Ctrl+Shift+K`. If it is taken, Context falls back to a default and says so. |
| Privacy | Keep or skip screenshots with results, how long to keep history, and apps Context never captures. An excluded app's capture is dropped before anything is sent. |
| Automatic cards | Experimental. Context watches which window is focused, without capturing, and runs a quiet check when you settle on a new window or conversation. A card appears only when it can help. At most 6 checks an hour, and none for excluded apps. |
| Startup | Start Context when you sign in. |

On Anthropic, Context sends the Messages API's server-side refusal fallback, so a request a safety classifier declines is retried on a fallback model.

Version 0.2 uses new encrypted record files. It does not read `chatgpt.bin` or `history.bin` from 0.1, so connect ChatGPT again after upgrading. You can delete those two files.

## Check a change

```sh
cargo fmt --check
cargo clippy --locked -- -D warnings
cargo build --locked
```

To diagnose sign-in, model or tool problems, set `CONTEXT_DEBUG=1` before launching. Context then logs the sign-in callback, token errors, the models your plan lists, each model round, each tool call and the MCP library's transport messages to stderr. It never logs tokens, screenshots or tool output.

For a manual check:

1. Start signed out, then run `context capture`. A card should offer **Connect ChatGPT**.
2. Connect, open a chat or document, and press the shortcut. The card should show progress without taking focus, then the answer.
3. Choose **Open details**. Copy and delete the result.
4. Quit and restart to confirm history survives.
5. Run `context` while Context is open. The existing window should come forward.
6. In Settings, switch the shortcut and confirm the sidebar shows the new one. Add the focused app to the excluded list, capture, and confirm the card says it is excluded.
7. Ask a follow-up in the detail view and confirm the answer appears under the result.
8. In `mcp.json`, add a stdio MCP server with one read-only tool and one other tool, plus an entry with a bad command. Choose **Reload**. The sidebar should show *1 tool, 1 hidden* and the bad command's error. Then capture a screen the tool can help with. The card should show *Checking …* and name the source.

Protected content may be missing from captures.
