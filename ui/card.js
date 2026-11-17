// The corner card: renders the phase the app sends, sizes the window to fit,
// pauses while hovered and closes itself.

import { button, dots, h, invoke, listen, logo } from "./dom.js";

const LIFETIME = { thinking: 120, ready: 20, empty: 5, problem: 12 };
/** How long a hovered card stays after the pointer leaves. */
const HOVER_GRACE = 3;

const root = document.getElementById("card");
let view = null;
let closesAt = 0;
let hovered = false;
let copied = false;

root.addEventListener("mouseenter", () => (hovered = true));
root.addEventListener("mouseleave", () => {
  hovered = false;
  closesAt = Math.max(closesAt, Date.now() + HOVER_GRACE * 1000);
});

setInterval(() => {
  if (view && !hovered && Date.now() >= closesAt) invoke("card_close");
}, 250);

function show(next) {
  if (!next) return;
  if (!view || next.generation !== view.generation) {
    closesAt = Date.now() + LIFETIME[next.phase] * 1000;
    copied = false;
  }
  view = next;
  render();
}

function render() {
  root.replaceChildren(header(), body(), actions());
  root.hidden = false;
  // Report the card's height so the window fits it exactly.
  requestAnimationFrame(() => invoke("card_resize", { height: root.offsetHeight }));
}

function header() {
  return h(
    "div",
    { class: "header" },
    logo(16),
    h("span", { class: "name" }, "Context"),
    h("span", { class: "app truncate" }, view.app ? `· ${view.app}` : ""),
    button("✕", "ghost icon", () => invoke("card_close"), { "aria-label": "Dismiss" }),
  );
}

function body() {
  switch (view.phase) {
    case "thinking":
      return h(
        "div",
        { class: "body" },
        h("div", { class: "status" }, dots(), h("p", { class: "title" }, view.status ?? "Reading your screen…")),
        h("p", { class: "text" }, "Keep working. The answer will appear here."),
        h("div", { class: "skeletons" }, h("div", { class: "skeleton", style: { width: "90%" } }), h("div", { class: "skeleton", style: { width: "60%" } })),
      );
    case "ready":
      return h(
        "div",
        { class: "body" },
        h("p", { class: "title" }, view.entry.text.headline),
        view.entry.text.summary && h("p", { class: "text" }, view.entry.text.summary),
      );
    case "empty":
      return h(
        "div",
        { class: "body" },
        h("p", { class: "title" }, "Nothing to add here"),
        h("p", { class: "text" }, "Context didn't find anything useful on this screen."),
      );
    case "problem":
      return h(
        "div",
        { class: "body" },
        h(
          "p",
          { class: view.problem.signIn ? "title" : "title error" },
          view.problem.signIn ? "Connect ChatGPT to continue" : "Context couldn't help with this one",
        ),
        h("p", { class: "text" }, view.problem.message),
      );
  }
}

function actions() {
  if (view.phase === "ready") {
    const { entry } = view;
    const sources = entry.sources ?? [];
    return h(
      "div",
      { class: "actions" },
      button("Open details", "primary", () => invoke("open_entry", { id: entry.id })),
      button(copied ? "Copied" : "Copy", "secondary", async () => {
        await invoke("copy_text", { text: plainText(entry.text) });
        copied = true;
        render();
      }),
      sources.length > 0 &&
        h("span", { class: "via" }, h("span", { class: "chip", title: sources.join(", ") }, viaLabel(sources))),
    );
  }
  if (view.phase === "problem") {
    return h(
      "div",
      { class: "actions" },
      view.problem.signIn
        ? button("Connect ChatGPT", "primary", () => invoke("sign_in"))
        : button("Open Context", "secondary", () => invoke("open_main")),
    );
  }
  return "";
}

function viaLabel(sources) {
  return sources.length === 1 ? `via ${sources[0]}` : `via ${sources[0]} +${sources.length - 1}`;
}

function plainText(text) {
  return [text.headline, text.summary, text.details].filter(Boolean).join("\n\n");
}

listen("card", show);
invoke("get_card").then(show);
