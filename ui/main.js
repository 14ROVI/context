// The main window: onboarding, history, the selected result with its
// screenshot, connected sources and the account.

import { button, dots, h, invoke, keys, listen, logo } from "./dom.js";
import { inline, markdown } from "./markdown.js";
import { renderSettings } from "./settings.js";
import { renderSources } from "./sources.js";

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
let confirmClear = false;
/** Screenshots already fetched, by entry id. */
const screenshots = new Map();
/** What the main area last showed, so it is rebuilt only when that changes. */
let shownKey = null;
let noticeTimer = null;
/** Bumped when a page module needs a redraw without a state change. */
let rerenders = 0;
setUpResizer();

document.addEventListener("context-rerender", () => {
  rerenders += 1;
  renderMain();
});

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
  if (search) requestAnimationFrame(() => document.getElementById("sources")?.scrollIntoView({ block: "start" }));
}

function render() {
  if (!state) return;
  const scroll = sidebar.querySelector(".history")?.scrollTop ?? 0;
  sidebar.replaceChildren(capturePanel(), historyHeader(), historyList(), accountPanel());
  sidebar.querySelector(".history").scrollTop = scroll;
  renderMain();
}

// Sidebar

function capturePanel() {
  const capture = state.capturing
    ? button([dots(), "Looking at your screen"], "primary", null, { disabled: true })
    : button("Capture screen", "primary", () => invoke("capture"));
  const watching =
    state.settings.autoCards &&
    h(
      "div",
      { class: "watching", title: "Automatic cards read new text on this computer and check it with a small model before looking closer." },
      h("span", { class: "pulse" }),
      state.watching ? `Automatic cards: watching ${state.watching}` : "Automatic cards on",
    );
  return h("div", { class: "capture" }, capture, shortcutHint(), watching);
}

function shortcutHint() {
  const { shortcut } = state;
  switch (shortcut.kind) {
    case "registered":
      return h("div", { class: "hint" }, h("span", {}, "or press"), keys(shortcut.keys));
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
    const meta = clock(time);
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

function accountPanel() {
  const { account } = state;
  let who;
  switch (account.state) {
    case "signedIn":
      // The profile row is the way into Settings.
      return h(
        "div",
        { class: "account" },
        h(
          "button",
          {
            type: "button",
            class: view === "settings" ? "who profile active" : "who profile",
            title: "Settings",
            onclick: () => openSettings(null),
          },
          h("span", { class: "avatar" }, account.email.slice(0, 1).toUpperCase()),
          h("span", { class: "email truncate" }, account.email),
        ),
      );
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
  );
}

/**
 * Lets the sidebar's edge be dragged to resize it. The width is kept in
 * local storage; double-clicking the edge goes back to the default.
 */
function setUpResizer() {
  const KEY = "sidebar-width";
  const resizer = document.getElementById("resizer");
  const apply = (width) => {
    if (width) document.body.style.setProperty("--sidebar-width", `${width}px`);
    else document.body.style.removeProperty("--sidebar-width");
  };
  const store = (width) => {
    try {
      if (width) localStorage.setItem(KEY, String(width));
      else localStorage.removeItem(KEY);
    } catch {}
  };
  try {
    apply(Number(localStorage.getItem(KEY)) || null);
  } catch {}

  resizer.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    resizer.setPointerCapture(event.pointerId);
    resizer.classList.add("dragging");
    let width = null;
    const move = (e) => {
      width = Math.round(Math.min(Math.max(e.clientX, 200), 520, window.innerWidth / 2));
      apply(width);
    };
    const end = () => {
      resizer.classList.remove("dragging");
      resizer.removeEventListener("pointermove", move);
      if (width) store(width);
    };
    resizer.addEventListener("pointermove", move);
    resizer.addEventListener("pointerup", end, { once: true });
    resizer.addEventListener("pointercancel", end, { once: true });
  });
  // Stops the drag from selecting text in the main area.
  resizer.addEventListener("mousedown", (event) => event.preventDefault());
  resizer.addEventListener("dblclick", () => {
    apply(null);
    store(null);
  });
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
      state.catalog,
      sourceSearch,
      rerenders,
    ]);
    if (key === shownKey) return;
    shownKey = key;
    // Keep the scroll position while settings change underneath.
    const scroll = main.querySelector(".page")?.scrollTop ?? 0;
    const page = h("div", { class: "page" }, renderSettings(state, renderSources(state, sourceSearch)));
    main.replaceChildren(...[banner(), page].filter(Boolean));
    page.scrollTop = scroll;
    return;
  }
  const entry = state.entries.find((item) => item.id === selected);
  const key = JSON.stringify([
    state.notice,
    state.historyProblem,
    entry ?? null,
    entry ? screenshots.has(entry.id) : false,
    entry ? null : [state.account, state.shortcut, state.settings.provider, state.apiKeys],
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
    h("div", { class: "when" }, when),
    h("h1", {}, entry.text.headline),
    entry.text.summary && h("p", { class: "summary" }, inline(entry.text.summary)),
    entry.text.details && h("div", { class: "details markdown" }, markdown(entry.text.details)),
    shot && [h("div", { class: "shot-label" }, "What Context saw"), h("img", { class: "shot", src: shot, alt: "The screen Context captured" })],
    h("div", { class: "provenance" }, provenance(entry.sources ?? [])),
  );
}

async function loadScreenshot(id) {
  const url = await invoke("get_screenshot", { id });
  if (url) {
    screenshots.set(id, url);
    if (id === selected) renderMain();
  }
}

function welcome() {
  const { account, shortcut, settings, apiKeys } = state;
  const provider = settings.provider;
  const ready = provider === "chatGpt" ? account.state === "signedIn" : apiKeys[provider];
  let setup;
  let connect;
  if (provider === "chatGpt") {
    setup = "Connect your ChatGPT plan";
    if (ready) connect = h("span", { class: "done" }, "✓ ChatGPT connected");
    else if (account.state === "connecting") connect = h("span", { class: "note" }, "Waiting for your browser…");
    else connect = button("Connect ChatGPT", "primary", () => invoke("sign_in"));
  } else {
    setup = `Add your ${provider === "openAi" ? "OpenAI" : "Anthropic"} API key`;
    connect = ready
      ? h("span", { class: "done" }, "✓ API key saved")
      : button("Open settings", "primary", () => openSettings(null));
  }
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
      h("h1", {}, ready ? "Ready when you are" : "Answers for what's on your screen"),
      h(
        "p",
        { class: "lead" },
        "Press the shortcut in any app. Context reads the screen, works out what you're doing and shows a short answer in the corner, without pulling you out of your work.",
      ),
    ),
    h(
      "div",
      { class: "steps" },
      h("div", { class: "step" }, h("span", { class: "number" }, "1"), h("span", { class: "label" }, setup), connect),
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


function provenance(sources) {
  return sources.length === 0
    ? "Based on a screenshot of your display. Check important details."
    : `Based on a screenshot of your display and ${sources.join(", ")}. Check important details.`;
}

listen("state", setState);
listen("select-entry", select);
listen("open-settings", openSettings);
invoke("get_state").then(setState);
