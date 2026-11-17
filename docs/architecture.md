# Architecture

[Wiki home](README.md)

Context is a Rust app with a [Tauri 2](https://v2.tauri.app) shell. Rust does the work: capture, model requests, sources, storage and history. The windows are plain HTML, CSS and JavaScript pages in [`ui/`](../ui) that render state the Rust side publishes.

## Current flow

1. A capture starts from the global shortcut, the tray, the **Capture screen** button or `context capture`. A second `context` process hands its command to the running one through Tauri's single-instance plugin and exits.
2. The controller in [`app/mod.rs`](../src/app/mod.rs) checks the ChatGPT connection. If Context's own window is in front, it hides it first so the capture shows the app behind it.
3. The worker thread captures the active window's display with XCap and reads the focused app's name and title. It reports the capture right away, and the corner card opens in its *reading* state.
4. The worker sends the image, window metadata and the allowed [source](sources.md) tools to the Responses API. When the model calls a tool, the worker runs it on the MCP server, reports progress to the card (*Checking calendar…*) and sends the result back to the model.
5. The controller saves the answer, the sources it used and the screenshot to history. The card shows the answer, *nothing to add* or the problem. A sign-in problem offers **Connect ChatGPT** in the card.
6. **Open details** brings up the main window on that entry. The card never takes focus. It sizes itself to its text, pauses while hovered and closes itself.

| Code | Owns |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Command line (`open`, `capture`), debug logging and startup. |
| [`src/app/mod.rs`](../src/app/mod.rs) | Tauri setup, the controller thread and published state. |
| [`src/app/worker.rs`](../src/app/worker.rs) | The worker thread: captures, model requests, sign-in, API keys and sources. |
| [`src/app/watch.rs`](../src/app/watch.rs) | Automatic cards: notices new text in the focused window and passes likely questions on. |
| [`src/ocr.rs`](../src/ocr.rs) | Local text recognition with `ocrs`. |
| [`src/app/commands.rs`](../src/app/commands.rs) | The commands the pages call. |
| [`src/app/card.rs`](../src/app/card.rs) | The card window: creation, phases, placement and size. |
| [`src/app/tray.rs`](../src/app/tray.rs) | Tray menu and global shortcut registration. |
| [`ui/index.html`](../ui/index.html), [`ui/main.js`](../ui/main.js) | Main window: onboarding, history, detail with screenshot, sources and account. |
| [`ui/card.html`](../ui/card.html), [`ui/card.js`](../ui/card.js) | Card rendering, measuring, timers and actions. |
| [`ui/settings.js`](../ui/settings.js) | The Settings page. |
| [`ui/theme.css`](../ui/theme.css), [`ui/dom.js`](../ui/dom.js) | Light and dark palette, shared components and DOM helpers. |
| [`src/capture.rs`](../src/capture.rs) | Display choice, focused-window metadata and JPEG encoding. |
| [`src/model/mod.rs`](../src/model/mod.rs) | `Assistant`, which sends a request to the provider chosen in Settings, the tool-call limit both providers share, and errors that say how to fix them. |
| [`src/model/prompt.rs`](../src/model/prompt.rs) | Instructions, limits and parsing of the final answer, shared by every provider. |
| [`src/model/chatgpt/`](../src/model/chatgpt) | Sign in with ChatGPT: browser OAuth, account registration, token refresh and sign-out. |
| [`src/model/openai.rs`](../src/model/openai.rs) | The Responses API for a ChatGPT plan or an OpenAI API key, with the tool-calling loop. |
| [`src/model/anthropic.rs`](../src/model/anthropic.rs) | The Anthropic Messages API for Claude, with the tool-use loop, over raw HTTP since Rust has no official SDK. |
| [`src/settings.rs`](../src/settings.rs) | `settings.json`: provider, model, shortcut, privacy, automatic cards and startup. |
| [`src/sources/`](../src/sources) | Sources: MCP servers and built-in connectors, OAuth sign-in, the catalog and the tool policy. See [Sources](sources.md#while-context-answers). |
| [`ui/sources.js`](../ui/sources.js) | The Sources section of Settings. |
| [`src/history.rs`](../src/history.rs) | Entry format, text limits, retention and screenshots. |
| [`src/storage.rs`](../src/storage.rs) | Data folder and encrypted records. |
| [`src/callback.rs`](../src/callback.rs) | The localhost listener that browser sign-ins for ChatGPT and sources return to. |

## Cross-platform approach

Context relies on cross-platform crates and does not branch on the target OS. Each crate picks its own backend.

| Concern | Crate | Notes |
| --- | --- | --- |
| Windows and pages | `tauri` | WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux. The theme follows the system appearance. |
| Card without focus | `tauri` | An unfocusable, always-on-top, transparent window placed in the display's work area. |
| Tray | `tauri` (`tray-icon` feature) | |
| Shortcut | `tauri-plugin-global-shortcut` | Windows, macOS and X11. Wayland has no app-level grab, so users bind `context capture`. |
| Single instance | `tauri-plugin-single-instance` | Forwards `context open` and `context capture` to the running app. |
| Capture | `xcap` | On Wayland it goes through the desktop screenshot portal. |
| Clipboard | `arboard` | Copies from the card, which never has focus. |
| Text recognition | `ocrs` | Pure-Rust OCR for automatic cards. The installer bundles its two ONNX models. |
| Start at login | `tauri-plugin-autostart` | |
| Secrets | `keyring` + `ring` | An AES-256-GCM key in Credential Manager, the Keychain or the Secret Service. |
| Sources | `rmcp` + `which` | MCP clients over stdio or streamable HTTP. `which` resolves launchers such as `npx.cmd`. |

The one exception is in [`sources/mcp.rs`](../src/sources/mcp.rs), which starts MCP servers with `CREATE_NO_WINDOW` on Windows. Release builds have no console, so a console server would otherwise open a window of its own.

## Threads and state

The controller thread owns app state and handles events from the pages, tray, shortcut, single-instance plugin and worker. After each change it publishes a snapshot to the main window as a `state` event, and card changes as `card` events. Pages read the current snapshot with `get_state` or `get_card` when they load.

One worker thread serializes blocking work: account checks, sign-in, sign-out, API keys, model lists, connecting sources and captures. It keeps its own copy of the settings, which the controller sends after each change. The worker owns the MCP connections and drives them on a small Tokio runtime. Each model turn is one request with `store: false`, and encrypted reasoning is carried between tool rounds.

## Automatic cards

Automatic cards are off by default. When on, each stage is cheaper than the next and passes on only what is worth it:

1. **Change detection, local.** Every 2 seconds the watcher captures the focused window and compares a 192×128 greyscale thumbnail with the last one. It counts pixels that changed noticeably, so one new chat line in a large window still registers.
2. **OCR, local.** Once the window has settled, `ocrs` reads its text, in about half a second for a chat window, and keeps lines it has not seen in that window. A newly focused window's last 8 lines count as new, so a question already waiting there is noticed.
3. **Filter, local.** Lines that look like a question or request pass: a question mark, or phrases such as "can you" or "let me know". The same text is not passed on again within an hour, and at most 30 candidates go out an hour.
4. **Gate, cheap model.** The worker sends only the new text, not a screenshot, to the provider's smallest model: a nano, mini or "luna" model on OpenAI, or Claude Haiku 5.5 on Anthropic. The model must answer YES or NO.
5. **Full check.** On YES, Context captures the screen and runs the normal analysis with sources, telling the model which new text triggered it and to answer only if it can help. A card appears only with an answer, without taking focus. At most 6 full checks run an hour.

Nothing leaves the computer before the gate. Excluded apps and well-known games are never captured, not even locally: in-game chat is full of questions that are not for Context, and a card would interrupt play. The game list is `GAMES` in [`src/app/watch.rs`](../src/app/watch.rs), and matches each program's reported name, such as "League of Legends", or its file name. The shortcut still works in games. The sidebar shows which app automatic cards are watching.

## Pages and trust

The pages load only from the app bundle, under a strict content security policy. They receive text and images as data, never file paths or tokens, and they cannot call tools or the network. Model, tool and screen text is always inserted as text, never as HTML.

## Agent boundary

The prompt treats the screenshot, window metadata and tool results as untrusted data. The model can decline by answering `NONE`, and can end with `NEEDS: <kind>` to suggest a source it lacked. Pages can never choose a URL to open: the usage page comes from the error Rust created. Context, not the model or the server, decides which tools are offered: read-only tools by default, or the user's explicit `tools` list. Calls are capped per capture. See [Sources](sources.md).
