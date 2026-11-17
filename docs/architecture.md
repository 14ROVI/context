# Architecture

[Wiki home](README.md)

## Current flow

1. A capture starts from the global shortcut, the tray, the **Capture screen** button or `context capture`. A second `context` process forwards its command to the running one through [`instance.rs`](../src/instance.rs).
2. The controller in [`desktop/mod.rs`](../src/desktop/mod.rs) checks the ChatGPT connection. If Context's own window is in front, it minimizes it first so the capture shows the app behind it.
3. The worker thread captures the active window's display with XCap and reads the focused app's name and title. It reports the capture right away, and the corner card opens in its *reading* state.
4. The worker sends the image, window metadata and the allowed [source](sources.md) tools to the Responses API. When the model calls a tool, the worker runs it on the MCP server, reports progress to the card (*Checking calendar…*) and sends the result back to the model.
5. The controller saves the answer and the sources it used to history. It updates the card to show the answer, *nothing to add* or the problem. A sign-in problem offers **Connect ChatGPT** in the card.
6. **Open details** brings up the main window on that entry. The card never takes focus. It pauses while hovered and closes itself.

| Code | Owns |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | Command line (`open`, `capture`) and startup. |
| [`src/instance.rs`](../src/instance.rs) | Single-instance lock and the loopback command channel. |
| [`src/desktop/mod.rs`](../src/desktop/mod.rs) | Controller, worker thread, events and app state. |
| [`src/desktop/main_window.rs`](../src/desktop/main_window.rs) | Main window: onboarding, history list, detail view, account and notices. |
| [`src/desktop/card.rs`](../src/desktop/card.rs) | Corner card phases, placement, auto-close and actions. |
| [`src/desktop/theme.rs`](../src/desktop/theme.rs) | Light and dark palettes, buttons, key caps and loading indicators. |
| [`src/desktop/tray.rs`](../src/desktop/tray.rs) | Tray menu and global shortcut registration. |
| [`src/desktop/native.rs`](../src/desktop/native.rs) | Showing the card without focus on Windows. |
| [`src/capture.rs`](../src/capture.rs) | Display choice, focused-window metadata and JPEG encoding. |
| [`src/chatgpt/auth.rs`](../src/chatgpt/auth.rs) | Browser OAuth, account registration, token refresh and sign-out. |
| [`src/chatgpt/responses.rs`](../src/chatgpt/responses.rs) | Model choice, prompt, the response stream and the tool-calling loop. |
| [`src/sources.rs`](../src/sources.rs) | `mcp.json`, MCP connections (stdio and HTTP), the tool policy and tool calls. |
| [`src/history.rs`](../src/history.rs) | Entry format, text limits and retention. |
| [`src/storage.rs`](../src/storage.rs) | Data folder and encrypted records. |

## Cross-platform approach

Context relies on cross-platform crates and does not branch on the target OS. Each crate picks its own backend.

| Concern | Crate | Notes |
| --- | --- | --- |
| UI | `gpui` | Same views on every platform. The theme follows the system appearance. |
| Capture | `xcap` | On Wayland it goes through the desktop screenshot portal. |
| Shortcut | `global-hotkey` | Windows, macOS and X11. Wayland has no app-level grab, so users bind `context capture`. |
| Tray | `tray-icon` | Uses the `ksni` StatusNotifier backend on Linux, so GTK is not required. |
| Secrets | `keyring` + `ring` | An AES-256-GCM key in Credential Manager, the Keychain or the Secret Service. |
| Sources | `rmcp` + `which` | MCP clients over stdio or streamable HTTP. `which` resolves launchers such as `npx.cmd`. |

There are two exceptions, both on Windows:

- [`native.rs`](../src/desktop/native.rs). GPUI 0.2 activates pop-up windows on Windows, so the card is shown there with Win32 calls. Remove it when GPUI supports non-activating pop-ups.
- [`sources.rs`](../src/sources.rs) starts MCP servers with `CREATE_NO_WINDOW`. Release builds have no console, so a console server would otherwise open a window of its own.

## Threads and state

The UI thread owns all state. One worker thread serializes blocking work: account checks, sign-in, sign-out, connecting sources and analysis. The worker owns the MCP connections and drives them on a small Tokio runtime. Each model turn is one request with `store: false`, and encrypted reasoning is carried between tool rounds. The tray, shortcut and instance listener only send events to the controller.

## Agent boundary

The prompt treats the screenshot, window metadata and tool results as untrusted data. The model can decline by answering `NONE`. Context, not the model or the server, decides which tools are offered: read-only tools by default, or the user's explicit `tools` list. Calls are capped per capture. See [Sources](sources.md).
