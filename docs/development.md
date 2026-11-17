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

## Use Context

1. Choose **Connect ChatGPT** in the window, card or tray. Sign in and allow plan usage in your browser. See [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source) for eligible plans.
2. In any app, press **Ctrl+Alt+Space** (**⌃⌥Space** on macOS). If another app holds it, Context uses **Ctrl+Alt+Shift+Space**. The window shows the active shortcut.
3. A card opens in the corner while Context reads the screen. It shows the answer when ready. Choose **Open details** for the full text, or **Copy**.
4. Optionally, connect [sources](sources.md) such as a calendar so answers can check more than the screen.

The **Capture screen** button hides Context and captures the app behind it. On Wayland, add a desktop shortcut that runs `context capture`. Running `context` again brings the open window forward.

Closing the window hides it, and Context keeps running in the tray. Click the tray icon to bring it back. Quit from the window or tray. Set `CONTEXT_MODEL` before launch to choose another image-capable model on your plan. Manage plan usage in [ChatGPT settings](https://chatgpt.com/settings/usage).

History keeps the latest 100 results in your local app-data folder. Records are encrypted with a key stored in your system credential store. Results show the app that was in focus when the desktop reports it. Each result keeps the screenshot it came from, encrypted in a `screenshots` subfolder. You can copy or delete a result, or clear all history. Deleting a result deletes its screenshot.

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
6. In `mcp.json`, add a stdio MCP server with one read-only tool and one other tool, plus an entry with a bad command. Choose **Reload**. The sidebar should show *1 tool, 1 hidden* and the bad command's error. Then capture a screen the tool can help with. The card should show *Checking …* and name the source.

Protected content may be missing from captures.
