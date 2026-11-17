// The Settings page. Changes save right away; text fields save when they
// lose focus or on Enter. API keys are sent to Rust once and never shown again.

import { button, h, invoke, keys } from "./dom.js";

const PROVIDERS = [
  { id: "chatGpt", label: "ChatGPT plan", note: "Uses your ChatGPT subscription through Sign in with ChatGPT." },
  { id: "openAi", label: "OpenAI API key", note: "Billed to your OpenAI API account." },
  { id: "anthropic", label: "Claude (Anthropic API key)", note: "Billed to your Anthropic API account. Defaults to Claude Opus 5.5." },
];

const SHORTCUTS = ["Ctrl+Alt+Space", "Ctrl+Alt+Shift+Space", "Ctrl+Shift+Space", "Alt+Shift+C"];

const RETENTION = [
  [0, "Until the latest 100 results"],
  [1, "1 day"],
  [7, "7 days"],
  [30, "30 days"],
  [90, "90 days"],
];

/** Text the user is typing, kept across re-renders. */
const drafts = { apiKey: "", shortcut: null, excluded: null };

/**
 * Renders the page. `extras` adds sections owned by other modules, such as
 * the sources catalog.
 */
export function renderSettings(state, extras = []) {
  return h(
    "div",
    { class: "settings" },
    h("h1", {}, "Settings"),
    modelSection(state),
    extras,
    shortcutSection(state),
    privacySection(state),
    automaticSection(state),
    startupSection(state),
  );
}

function save(state, change) {
  const settings = structuredClone(state.settings);
  change(settings);
  invoke("save_settings", { settings });
}

function section(title, description, ...children) {
  return h(
    "section",
    { class: "setting-group" },
    h("h2", {}, title),
    description && h("p", { class: "lead" }, description),
    children,
  );
}

function toggle(label, note, checked, onchange) {
  return h(
    "label",
    { class: "toggle" },
    h("input", { type: "checkbox", checked, onchange: (event) => onchange(event.target.checked) }),
    h("span", {}, h("b", {}, label), note && h("small", {}, note)),
  );
}

function modelSection(state) {
  const { settings, apiKeys, models, account } = state;
  const provider = settings.provider;
  const choices = h(
    "div",
    { class: "choices" },
    PROVIDERS.map((option) =>
      h(
        "label",
        { class: option.id === provider ? "choice selected" : "choice" },
        h("input", {
          type: "radio",
          name: "provider",
          checked: option.id === provider,
          onchange: () => save(state, (next) => {
            next.provider = option.id;
            next.model = null;
          }),
        }),
        h("span", {}, h("b", {}, option.label), h("small", {}, option.note)),
      ),
    ),
  );

  let credentials;
  if (provider === "chatGpt") {
    credentials =
      account.state === "signedIn"
        ? h("p", { class: "status" }, `Connected as ${account.email}.`)
        : h("div", { class: "row" }, button("Connect ChatGPT", "primary", () => invoke("sign_in")));
  } else {
    const saved = provider === "openAi" ? apiKeys.openAi : apiKeys.anthropic;
    const input = h("input", {
      type: "password",
      class: "field",
      placeholder: saved ? "Saved. Paste a new key to replace it." : "Paste your API key",
      autocomplete: "off",
      spellcheck: "false",
      oninput: (event) => (drafts.apiKey = event.target.value),
    });
    input.value = drafts.apiKey;
    const store = () => {
      if (!drafts.apiKey.trim()) return;
      invoke("set_api_key", { provider, key: drafts.apiKey });
      drafts.apiKey = "";
    };
    input.addEventListener("keydown", (event) => event.key === "Enter" && store());
    credentials = h(
      "div",
      { class: "row" },
      input,
      button("Save key", "secondary", store),
      saved && button("Remove", "danger", () => invoke("set_api_key", { provider, key: null })),
    );
  }

  const listed = models.provider === provider ? models.list : [];
  const current = settings.model ?? "";
  const options = [h("option", { value: "", selected: current === "" }, "Default")];
  for (const id of new Set([...listed, ...(current ? [current] : [])])) {
    options.push(h("option", { value: id, selected: id === current }, id));
  }
  const select = h(
    "select",
    {
      class: "field",
      onchange: (event) => save(state, (next) => (next.model = event.target.value || null)),
    },
    options,
  );
  let modelNote = "";
  if (models.provider === provider && models.loading) modelNote = "Loading models…";
  else if (models.provider === provider && models.error) modelNote = models.error;
  const modelRow = h(
    "div",
    { class: "row" },
    select,
    button("Load models", "ghost", () => invoke("list_models", { provider })),
  );

  return section(
    "Model",
    "Which service reads your screen and answers.",
    choices,
    credentials,
    h("div", { class: "label" }, "Model"),
    modelRow,
    modelNote && h("p", { class: models.error ? "note error" : "note" }, modelNote),
  );
}

function shortcutSection(state) {
  const { settings, shortcut } = state;
  if (shortcut.kind === "external") {
    return section(
      "Shortcut",
      "Your desktop doesn't let apps register shortcuts. Add a desktop shortcut that runs “context capture”.",
    );
  }
  // The shortcut in use, which differs from the preference when another app
  // already holds the preferred one.
  const active = shortcut.kind === "registered" ? shortcut.keys : settings.shortcut;
  const input = h("input", {
    class: "field",
    value: drafts.shortcut ?? active,
    spellcheck: "false",
    oninput: (event) => (drafts.shortcut = event.target.value),
  });
  const apply = (value) => {
    drafts.shortcut = null;
    if (value && value !== active) save(state, (next) => (next.shortcut = value));
  };
  input.addEventListener("change", (event) => apply(event.target.value.trim()));
  const presets = h(
    "div",
    { class: "row wrap" },
    SHORTCUTS.map((keysLabel) =>
      h(
        "button",
        {
          type: "button",
          class: keysLabel === active ? "preset selected" : "preset",
          onclick: () => apply(keysLabel),
        },
        keys(keysLabel),
      ),
    ),
  );
  return section(
    "Shortcut",
    "Press this in any app to capture the screen. Type your own, such as Ctrl+Shift+K.",
    presets,
    h("div", { class: "row" }, input),
    active !== settings.shortcut &&
      h("p", { class: "note" }, `${settings.shortcut} is taken by another app, so Context uses ${active}.`),
    shortcut.kind === "unavailable" && h("p", { class: "note error" }, "No shortcut could be registered. Try another one."),
  );
}

function privacySection(state) {
  const { settings } = state;
  const excluded = h("textarea", {
    class: "field",
    rows: "3",
    placeholder: "One app per line, for example 1Password",
    spellcheck: "false",
    oninput: (event) => (drafts.excluded = event.target.value),
  });
  excluded.value = drafts.excluded ?? settings.excludedApps.join("\n");
  excluded.addEventListener("change", (event) => {
    drafts.excluded = null;
    const apps = event.target.value
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
    save(state, (next) => (next.excludedApps = apps));
  });
  const retention = h(
    "select",
    {
      class: "field",
      onchange: (event) => save(state, (next) => (next.historyDays = Number(event.target.value))),
    },
    RETENTION.map(([days, label]) => h("option", { value: String(days), selected: days === settings.historyDays }, label)),
  );
  return section(
    "Privacy",
    "Everything Context saves stays encrypted on this computer.",
    toggle(
      "Keep screenshots with results",
      "Shows what Context saw in each result. Turn off to save only the text.",
      settings.saveScreenshots,
      (value) => save(state, (next) => (next.saveScreenshots = value)),
    ),
    h("div", { class: "label" }, "Keep history for"),
    h("div", { class: "row" }, retention),
    h("div", { class: "label" }, "Never capture these apps"),
    excluded,
    h("p", { class: "note" }, "Context drops the capture before anything leaves your computer. Names match partially and ignore case."),
  );
}

function automaticSection(state) {
  return section(
    "Automatic cards",
    null,
    toggle(
      "Offer cards without the shortcut (experimental)",
      "When you settle on a new window or conversation, Context checks it quietly and shows a card only if it can help. At most 6 checks an hour, and none for excluded apps. Each check uses your plan or API usage.",
      state.settings.autoCards,
      (value) => save(state, (next) => (next.autoCards = value)),
    ),
  );
}

function startupSection(state) {
  return section(
    "Startup",
    null,
    toggle("Start Context when you sign in", null, state.settings.startAtLogin, (value) =>
      save(state, (next) => (next.startAtLogin = value)),
    ),
  );
}
