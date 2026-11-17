// The main window: onboarding, history, the selected result with its
// screenshot, connected sources and the account.

import { button, dots, h, invoke, keys, listen, logo } from "./dom.js";
import { renderSettings } from "./settings.js";

/** How long a non-error notice stays before it fades. */
const NOTICE_SECONDS = 8;

const sidebar = document.getElementById("sidebar");
const main = document.getElementById("main");

let state = null;
let selected = null;
/** "history" shows results; "settings" shows the Settings page. */
let view = "history";
/** A kind of source to look for when Settings opens, such as "calendar". */
let sourceSearch = null;
/** Follow-up questions being typed, by entry id. */
const questions = new Map();
let confirmClear = false;
let copied = null;
/** Screenshots already fetched, by entry id. */
const screenshots = new Map();
/** What the main area last showed, so it is rebuilt only when that changes. */
let shownKey = null;
let noticeTimer = null;

function setState(next) {
  if (!next) return;
  state = next;
  if (!state.entries.some((entry) => entry.id === selected)) {
    selected = state.entries[0]?.id ?? null;
    confirmClear = false;
  }
  scheduleNotice();
  render();
}

function select(id) {
  selected = id;
  view = "history";
  confirmClear = false;
  render();
}

function openSettings(search) {
  view = "settings";
  sourceSearch = search ?? null;
  render();
}

function render() {
  if (!state) return;
  const scroll = sidebar.querySelector(".history")?.scrollTop ?? 0;
  sidebar.replaceChildren(brand(), capturePanel(), historyHeader(), historyList(), sourcesPanel(), accountPanel());
  sidebar.querySelector(".history").scrollTop = scroll;
  renderMain();
}

// Sidebar

function brand() {
  return h("div", { class: "brand" }, logo(22), "Context");
}

function capturePanel() {
  const capture = state.capturing
    ? button([dots(), "Looking at your screen"], "primary", null, { disabled: true })
    : button("Capture screen", "primary", () => invoke("capture"));
  return h("div", { class: "capture" }, capture, shortcutHint());
}

function shortcutHint() {
  const { shortcut } = state;
  switch (shortcut.kind) {
    case "registered":
      return h("div", { class: "hint" }, "or press", keys(shortcut.keys));
    case "external":
      return h("div", { class: "hint" }, "Bind “context capture” to a shortcut");
    case "unavailable":
      return h("div", { class: "hint" }, "Global shortcut unavailable");
    default:
      return h("div", { class: "hint" });
  }
}

function historyHeader() {
  const header = h("div", { class: "section" }, "History");
  if (state.entries.length === 0) return header;
  const small = (label, variant, onclick) => button(label, `${variant} small`, onclick);
  header.append(
    confirmClear
      ? h(
          "div",
          { class: "row" },
          small("Cancel", "ghost", () => {
            confirmClear = false;
            render();
          }),
          small("Clear all", "danger", () => invoke("clear_history")),
        )
      : small("Clear", "ghost", () => {
          confirmClear = true;
          render();
        }),
  );
  return header;
}

function historyList() {
  const list = h("div", { class: "history" });
  if (state.entries.length === 0) {
    list.append(h("div", { class: "empty" }, "Results you capture will collect here."));
    return list;
  }
  let day = null;
  for (const entry of state.entries) {
    const time = new Date(entry.created_at * 1000);
    const label = dayLabel(time);
    if (label !== day) {
      day = label;
      list.append(h("div", { class: "day" }, label));
    }
    const meta = entry.app ? `${entry.app} · ${clock(time)}` : clock(time);
    list.append(
      h(
        "button",
        { class: entry.id === selected && view === "history" ? "entry selected" : "entry", type: "button", onclick: () => select(entry.id) },
        h("div", { class: "headline truncate" }, entry.text.headline),
        h("div", { class: "meta truncate" }, meta),
      ),
    );
  }
  return list;
}

function sourcesPanel() {
  const { sources } = state;
  const panel = h(
    "div",
    { class: "sources" },
    h(
      "div",
      { class: "section" },
      "Sources",
      h(
        "div",
        { class: "row" },
        button("Edit", "ghost small", () => invoke("edit_sources")),
        button("Reload", "ghost small", () => invoke("reload_sources")),
      ),
    ),
  );
  if (sources.connecting) {
    panel.append(h("div", { class: "note" }, "Connecting sources…"));
    return panel;
  }
  if (sources.problem) panel.append(h("div", { class: "problem" }, sources.problem));
  if (sources.servers.length === 0 && !sources.problem) {
    panel.append(h("div", { class: "note" }, "Add MCP servers so answers can check your calendar, docs and more."));
  }
  for (const server of sources.servers) {
    let kind = "";
    let detail;
    if (server.problem) {
      kind = "error";
      detail = server.problem;
    } else if (server.enabled === 0) {
      kind = "idle";
      detail = `No read-only tools, ${server.hidden} hidden`;
    } else {
      detail = plural(server.enabled, "tool") + (server.hidden ? `, ${server.hidden} hidden` : "");
    }
    panel.append(
      h(
        "div",
        { class: `source ${kind}`, title: detail },
        h("span", { class: "dot" }),
        h("span", { class: "name truncate" }, server.name),
        h("span", { class: "detail truncate" }, detail),
      ),
    );
  }
  return panel;
}

function accountPanel() {
  const { account } = state;
  let who;
  switch (account.state) {
    case "signedIn":
      who = h(
        "div",
        { class: "who" },
        h("span", { class: "avatar" }, account.email.slice(0, 1).toUpperCase()),
        h(
          "div",
          { style: { "min-width": "0" } },
          h("div", { class: "email truncate" }, account.email),
          h("button", { class: "link", type: "button", onclick: () => invoke("sign_out") }, "Sign out"),
        ),
      );
      break;
    case "signedOut":
      who = h(
        "div",
        { class: "who" },
        button("Connect ChatGPT", "secondary", () => invoke("sign_in"), { style: { width: "100%" } }),
      );
      break;
    case "connecting":
      who = h("div", { class: "working" }, dots(), "Waiting for your browser…");
      break;
    default:
      who = h("div", { class: "working" }, dots(), "Checking your account…");
  }
  return h(
    "div",
    { class: "account" },
    who,
    button("Settings", view === "settings" ? "ghost small active" : "ghost small", () => openSettings(null)),
    button("Quit", "ghost small", () => invoke("quit")),
  );
}

// Main area

function renderMain() {
  if (view === "settings") {
    const key = JSON.stringify([
      "settings",
      state.notice,
      state.settings,
      state.apiKeys,
      state.models,
      state.account,
      state.shortcut,
      state.sources,
      sourceSearch,
    ]);
    if (key === shownKey) return;
    shownKey = key;
    // Keep the scroll position while settings change underneath.
    const scroll = main.querySelector(".page")?.scrollTop ?? 0;
    const page = h("div", { class: "page" }, renderSettings(state, sourcesSettings()));
    main.replaceChildren(...[banner(), page].filter(Boolean));
    page.scrollTop = scroll;
    return;
  }
  const entry = state.entries.find((item) => item.id === selected);
  const key = JSON.stringify([
    state.asking,
    state.notice,
    state.historyProblem,
    entry ?? null,
    entry ? copied === entry.id : false,
    entry ? screenshots.has(entry.id) : false,
    entry ? null : [state.account, state.shortcut],
  ]);
  if (key === shownKey) return;
  shownKey = key;
  const page = h("div", { class: "page" }, entry ? detail(entry) : welcome());
  main.replaceChildren(...[banner(), page].filter(Boolean));
  if (entry?.has_screenshot && !screenshots.has(entry.id)) loadScreenshot(entry.id);
}

function banner() {
  const { notice, historyProblem } = state;
  if (notice) {
    return h(
      "div",
      { class: notice.error ? "banner error" : "banner" },
      h("span", { class: "message" }, notice.message),
      button("✕", "ghost icon", () => invoke("dismiss_notice", { id: notice.id }), { "aria-label": "Dismiss" }),
    );
  }
  if (historyProblem) return h("div", { class: "banner" }, h("span", { class: "message" }, historyProblem));
  return null;
}

function scheduleNotice() {
  clearTimeout(noticeTimer);
  const { notice } = state;
  if (notice && !notice.error) {
    noticeTimer = setTimeout(() => invoke("dismiss_notice", { id: notice.id }), NOTICE_SECONDS * 1000);
  }
}

function detail(entry) {
  const time = new Date(entry.created_at * 1000);
  const when = time.toLocaleString(undefined, { weekday: "long", day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });
  const paragraphs = entry.text.details ? entry.text.details.split("\n\n") : [];
  const shot = screenshots.get(entry.id);
  return h(
    "div",
    {},
    h("div", { class: "when" }, entry.app && h("span", { class: "chip" }, entry.app), when),
    h("h1", {}, entry.text.headline),
    entry.text.summary && h("p", { class: "summary" }, entry.text.summary),
    paragraphs.length > 0 && h("div", { class: "details" }, paragraphs.map((text) => h("p", {}, text))),
    h(
      "div",
      { class: "actions" },
      button(copied === entry.id ? "Copied" : "Copy", "secondary", async () => {
        await invoke("copy_text", { text: plainText(entry.text) });
        copied = entry.id;
        renderMain();
      }),
      button("Delete", "danger", () => invoke("delete_entry", { id: entry.id })),
    ),
    followUps(entry),
    shot && [h("div", { class: "shot-label" }, "What Context saw"), h("img", { class: "shot", src: shot, alt: "The screen Context captured" })],
    h("div", { class: "provenance" }, provenance(entry.sources ?? [])),
  );
}

function followUps(entry) {
  const thread = entry.follow_ups ?? [];
  const asking = state.asking === entry.id;
  const box = h("textarea", {
    class: "field",
    rows: "2",
    placeholder: "Ask a follow-up about this result",
    disabled: asking,
    oninput: (event) => questions.set(entry.id, event.target.value),
  });
  box.value = questions.get(entry.id) ?? "";
  const ask = () => {
    const question = (questions.get(entry.id) ?? "").trim();
    if (!question || asking) return;
    questions.delete(entry.id);
    invoke("ask_follow_up", { id: entry.id, question });
  };
  box.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      ask();
    }
  });
  return h(
    "div",
    { class: "follow-ups" },
    thread.map((item) =>
      h(
        "div",
        { class: "follow-up" },
        h("p", { class: "question" }, item.question),
        h("p", { class: "answer" }, item.answer),
        item.sources?.length > 0 && h("p", { class: "via-note" }, `Checked ${item.sources.join(", ")}`),
      ),
    ),
    asking
      ? h("div", { class: "asking" }, dots(), "Thinking about your question…")
      : h("div", { class: "ask" }, box, button("Ask", "secondary", ask)),
  );
}

/** Settings sections for sources. Filled in by the sources catalog. */
function sourcesSettings() {
  return [];
}

async function loadScreenshot(id) {
  const url = await invoke("get_screenshot", { id });
  if (url) {
    screenshots.set(id, url);
    if (id === selected) renderMain();
  }
}

function welcome() {
  const { account, shortcut } = state;
  const signedIn = account.state === "signedIn";
  let connect;
  if (signedIn) connect = h("span", { class: "done" }, "✓ ChatGPT connected");
  else if (account.state === "connecting") connect = h("span", { class: "note" }, "Waiting for your browser…");
  else connect = button("Connect ChatGPT", "primary", () => invoke("sign_in"));
  let press;
  if (shortcut.kind === "registered") press = keys(shortcut.keys);
  else if (shortcut.kind === "external") press = h("span", { class: "note" }, "Add a desktop shortcut that runs “context capture”.");
  else press = h("span", { class: "note" }, "Use Capture screen in the sidebar or tray.");
  const example = (title, body) => h("div", { class: "example" }, h("b", {}, title), h("span", {}, body));
  return h(
    "div",
    { class: "welcome" },
    logo(44),
    h(
      "div",
      {},
      h("h1", {}, signedIn ? "Ready when you are" : "Answers for what's on your screen"),
      h(
        "p",
        { class: "lead" },
        "Press the shortcut in any app. Context reads the screen, works out what you're doing and shows a short answer in the corner, without pulling you out of your work.",
      ),
    ),
    h(
      "div",
      { class: "steps" },
      h("div", { class: "step" }, h("span", { class: "number" }, "1"), h("span", { class: "label" }, "Connect your ChatGPT plan"), connect),
      h("div", { class: "step" }, h("span", { class: "number" }, "2"), h("span", { class: "label" }, "Press the shortcut in any app"), press),
    ),
    h(
      "div",
      { class: "examples" },
      h("div", { class: "section", style: { padding: "0" } }, "Try it on"),
      example("A long chat thread", "Catch up on what was decided and what's still open."),
      example("An error message", "See what it means and what to try next."),
      example("A question someone asked you", "Get the facts from the conversation in one line."),
    ),
  );
}

// Formatting

function dayLabel(time) {
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (time.toDateString() === today.toDateString()) return "Today";
  if (time.toDateString() === yesterday.toDateString()) return "Yesterday";
  return time.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

const clock = (time) => time.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

function plural(count, noun) {
  return count === 1 ? `1 ${noun}` : `${count} ${noun}s`;
}

function plainText(text) {
  return [text.headline, text.summary, text.details].filter(Boolean).join("\n\n");
}

function provenance(sources) {
  return sources.length === 0
    ? "Based on a screenshot of your display. Check important details."
    : `Based on a screenshot of your display and ${sources.join(", ")}. Check important details.`;
}

listen("state", setState);
listen("select-entry", select);
listen("open-settings", openSettings);
invoke("get_state").then(setState);
