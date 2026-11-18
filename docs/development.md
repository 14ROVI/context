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

The **Release** GitHub Action builds Context for Windows, macOS and Linux on demand. Run it from the repository's Actions tab, or:

```sh
gh workflow run release.yml
gh workflow run release.yml -f publish=true
```

Each run uploads the Windows NSIS installer, a universal macOS `.dmg`, and Linux `.deb`, `.rpm` and `.AppImage` bundles as artifacts. With `publish=true` it also creates a GitHub release for the version in `tauri.conf.json`, tagged at the commit that was built. Bump that version, and the one in `Cargo.toml`, before publishing; a release tag can only be used once. To build the installer locally, run `npx --yes @tauri-apps/cli@2 build --bundles nsis`. The installer bundles the OCR models from `models/`, which are not in git; the workflow downloads them and checks their SHA-256 hashes, and a local build needs them there too (`text-detection.onnx` and `text-recognition.onnx` from the [ocrs](https://github.com/robertknight/ocrs) project). No build is code-signed yet, so Windows SmartScreen warns before the installer runs and macOS Gatekeeper blocks the app until you allow it under **Privacy & Security**. There is no auto-updater, because a private repository's releases are not public for an updater to fetch.

## Use Context

1. Choose **Connect ChatGPT** in the window or tray. Sign in and allow plan usage in your browser. See [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source) for eligible plans. To use an API key instead, see [Settings](#settings).
2. In any app, press **Ctrl+Alt+Space** (**⌃⌥Space** on macOS). If another app holds it, Context uses **Ctrl+Alt+Shift+Space**. The window shows the active shortcut.
3. A *Searching for context…* notification appears, and the tray tooltip shows progress while Context reads the screen. A notification shows the answer when ready. Click it for the full text.
4. Optionally, connect [sources](sources.md) such as a calendar so answers can check more than the screen. When an answer needed a source you have not connected, the notification names the kind to connect in Settings.

The **Capture screen** button hides Context and captures the app behind it. On Wayland, add a desktop shortcut that runs `context capture`. Running `context` again brings the open window forward.

Closing the window hides it, and Context keeps running in the tray. Click the tray icon to bring it back. Quit from the tray. When a plan or API usage limit is reached, a notification says so.

History keeps results forever by default, in your local app-data folder. In Settings you can keep them for 1, 7, 30 or 90 days instead. Records are encrypted with a key stored in your system credential store. Results don't record which app was in focus. Each result keeps the screenshot it came from, encrypted in a `screenshots` subfolder. You can clear all history, which deletes the screenshots too.

## Settings

Open Settings by clicking your account at the bottom of the sidebar, or **Settings** there when you're not signed in. Changes save right away to `settings.json` in the config folder, next to `mcp.json`.

| Section | Options |
| --- | --- |
| Model | A ChatGPT plan, an OpenAI API key, or an Anthropic API key for Claude. API keys are encrypted like the account and never shown again. Pick a model, or keep the default: Context picks a vision model on a plan, and Claude Opus 5.5 on Anthropic. |
| Shortcut | Pick a preset or type one such as `Ctrl+Shift+K`. If it is taken, Context falls back to a default and says so. |
| Privacy | Keep or skip screenshots with results, how long to keep history (forever by default), and apps Context never captures. An excluded app's capture is dropped before anything is sent. |
| Automatic cards | Experimental. Context notices new text in the focused window, such as a friend's question in a chat, and shows a card only when it can help. See [How automatic cards decide](architecture.md#automatic-cards). |
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

1. Start signed out, then run `context capture`. A notification should say to connect ChatGPT, and the window should offer **Connect ChatGPT**.
2. Connect, open a chat or document, and press the shortcut. The tray tooltip should show progress, then a notification shows the answer without taking focus.
3. Click the notification and confirm the main window opens on the result with its screenshot.
4. Quit and restart to confirm history survives.
5. Run `context` while Context is open. The existing window should come forward.
6. In Settings, switch the shortcut and confirm the sidebar shows the new one. Add the focused app to the excluded list, capture, and confirm the notification says it is excluded.
7. In `mcp.json`, add a stdio MCP server with one read-only tool and one other tool, plus an entry with a bad command. Choose **reload** in Settings. Sources should show *1 tool, 1 hidden* and the bad command's error. Then capture a screen the tool can help with. The tray tooltip should show *Checking …*, and the entry in history should name the source.

Protected content may be missing from captures.
