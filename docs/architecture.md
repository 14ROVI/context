# Architecture

[Wiki home](README.md)

Context is a Rust app with a [Tauri 2](https://v2.tauri.app) shell. Rust does the work: capture, model requests, sources, storage and history. The windows are plain HTML, CSS and JavaScript pages in [`ui/`](../ui) that render state the Rust side publishes.

## Current flow

1. A capture starts from the global shortcut, the tray, the **Capture screen** button or `context capture`. A second `context` process hands its command to the running one through Tauri's single-instance plugin and exits.
2. The controller in [`app/mod.rs`](../src/app/mod.rs) checks the ChatGPT connection. If Context's own window is in front, it hides it first so the capture shows the app behind it.
3. The worker thread captures the active window's display with XCap and reads the focused app's name and title. It reports the capture right away. A short *Searching for context…* notification appears, and the tray tooltip reads *Reading your screen…*.
4. The worker sends the image, window metadata, the allowed [source](sources.md) tools and, unless it is turned off, the provider's [web search](sources.md#web-search) tool to the model. When the model calls a tool, the worker runs it on the MCP server, reports progress in the tray tooltip (*Checking calendar…*) and sends the result back to the model.
5. The controller saves the answer, the sources it used, the pages it cited and the screenshot to history. A native notification shows the headline and summary, *nothing to add* or the problem. Problems also appear in the main window, where a sign-in problem offers **Connect ChatGPT**.
6. The system draws the notification, so it never takes focus and follows the user's notification settings and Do Not Disturb. It has no buttons. Clicking it while it is on screen opens Context on that entry, or at Settings or the usage page for a problem.

| Code | Owns |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Command line (`open`, `capture`), debug logging and startup. |
| [`src/app/mod.rs`](../src/app/mod.rs) | Tauri setup, the controller thread and published state. |
| [`src/app/worker.rs`](../src/app/worker.rs) | The worker thread: captures, model requests, sign-in, API keys and sources. |
| [`src/app/watch.rs`](../src/app/watch.rs) | Automatic cards: notices new text in the focused window and passes likely questions on. |
| [`src/ocr.rs`](../src/ocr.rs) | Local text recognition with `ocrs`. |
| [`src/app/commands.rs`](../src/app/commands.rs) | The commands the pages call. |
| [`src/app/notify.rs`](../src/app/notify.rs) | Native notifications for answers and problems. |
| [`src/app/tray.rs`](../src/app/tray.rs) | Tray menu, progress tooltip and global shortcut registration. |
| [`ui/index.html`](../ui/index.html), [`ui/main.js`](../ui/main.js) | Main window: onboarding, history, detail with screenshot, sources and account. |
| [`ui/settings.js`](../ui/settings.js) | The Settings page. |
| [`ui/theme.css`](../ui/theme.css), [`ui/dom.js`](../ui/dom.js) | Light and dark palette, shared components and DOM helpers. |
| [`ui/markdown.js`](../ui/markdown.js) | Renders the model's light Markdown (bold, italic, code, lists, links) as elements, never as HTML. |
| [`src/capture.rs`](../src/capture.rs) | Display choice, focused-window metadata and JPEG encoding. |
| [`src/model/mod.rs`](../src/model/mod.rs) | `Assistant`, which sends a request to the provider chosen in Settings, the tool-call limit both providers share, and errors that say how to fix them. |
| [`src/model/prompt.rs`](../src/model/prompt.rs) | Instructions, the web search on and off notes, limits and parsing of the final answer, shared by every provider. |
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
| Notifications | `notify-rust` | Windows toasts stay about 25 seconds, the longest Windows allows without a button. Linux notifications stay until dismissed. macOS follows the user's alert style. The system always shows the app's name and icon; a build run from `target` posts as PowerShell on Windows and Terminal on macOS. |
| Tray | `tauri` (`tray-icon` feature) | |
| Shortcut | `tauri-plugin-global-shortcut` | Windows, macOS and X11. Wayland has no app-level grab, so users bind `context capture`. |
| Single instance | `tauri-plugin-single-instance` | Forwards `context open` and `context capture` to the running app. |
| Capture | `xcap` | On Wayland it goes through the desktop screenshot portal. |
| Text recognition | `ocrs` | Pure-Rust OCR for automatic cards. The installer bundles its two ONNX models. |
| Start at login | `tauri-plugin-autostart` | |
| Secrets | `keyring` + `ring` | An AES-256-GCM key in Credential Manager, the Keychain or the Secret Service. |
| Sources | `rmcp` + `which` | MCP clients over stdio or streamable HTTP. `which` resolves launchers such as `npx.cmd`. |

There are two exceptions. [`sources/mcp.rs`](../src/sources/mcp.rs) starts MCP servers with `CREATE_NO_WINDOW` on Windows; release builds have no console, so a console server would otherwise open a window of its own. [`app/notify.rs`](../src/app/notify.rs) names the app to post as on Windows and macOS, which `notify-rust` leaves to the caller.

## Threads and state

The controller thread owns app state and handles events from the pages, tray, shortcut, single-instance plugin and worker. After each change it publishes a snapshot to the main window as a `state` event. The page reads the current snapshot with `get_state` when it loads.

One worker thread serializes blocking work: account checks, sign-in, sign-out, API keys, model lists, connecting sources and captures. It keeps its own copy of the settings, which the controller sends after each change. The worker owns the MCP connections and drives them on a small Tokio runtime. Each model turn is one request with `store: false`, and encrypted reasoning is carried between tool rounds.

## Automatic cards

Automatic cards are off by default. When on, each stage is cheaper than the next and passes on only what is worth it:

1. **Change detection, local.** Every 2 seconds the watcher captures the focused window and compares a 192×128 greyscale thumbnail with the last one. It counts pixels that changed noticeably, so one new chat line in a large window still registers.
2. **OCR, local.** Once the window has settled, `ocrs` reads its text, in about half a second for a chat window, and keeps lines it has not seen in that window. A newly focused window's last 8 lines count as new, so a question already waiting there is noticed.
3. **Filter, local.** Lines that look like a question or request pass: a question mark, or phrases such as "can you" or "let me know". The same text is not passed on again within an hour, and at most 30 candidates go out an hour.
4. **Gate, cheap model.** The worker sends only the new text, not a screenshot, to the provider's smallest model: a nano, mini or "luna" model on OpenAI, or Claude Haiku 5.5 on Anthropic. The model must answer YES or NO.
5. **Full check.** On YES, Context captures the screen and runs the normal analysis with sources, telling the model which new text triggered it and to answer only if it can help. A short *Searching for context…* notification appears once the screen is captured, then the answer if there is one. At most 6 full checks run an hour.

Nothing leaves the computer before the gate. Excluded apps and well-known games are never captured, not even locally: in-game chat is full of questions that are not for Context, and a card would interrupt play. The game list is `GAMES` in [`src/app/watch.rs`](../src/app/watch.rs), and matches each program's reported name, such as "League of Legends", or its file name. The shortcut still works in games. The sidebar shows which app automatic cards are watching.

## Pages and trust

The pages load only from the app bundle, under a strict content security policy. They receive text and images as data, never file paths or tokens, and they cannot call tools or the network. Model, tool and screen text is always inserted as text, never as HTML.

## Agent boundary

The prompt treats the screenshot, window metadata and tool results as untrusted data. The model can decline by answering `NONE`, and can end with `NEEDS: <kind>` to suggest a source it lacked. Pages can never choose a URL to open: the usage page comes from the error Rust created, and a cited page is opened by its position in a saved entry. Context, not the model or the server, decides which tools are offered: read-only tools by default, or the user's explicit `tools` list. Calls are capped per capture. See [Sources](sources.md).
