// The Sources section of Settings: connected sources, a featured catalog and
// search through the MCP Registry. Text from the registry is shown as text.

import { button, dots, h, invoke } from "./dom.js";

/** Typed values in add forms, by entry id then field name. */
const forms = new Map();
/** The entry whose add form is open. */
let open = null;
let query = "";
let lastPreset = null;

/**
 * Renders the section. `preset` is a kind of source to look for, such as
 * "calendar", when Settings was opened from a card's suggestion.
 */
export function renderSources(state, preset) {
  if (preset && preset !== lastPreset) {
    lastPreset = preset;
    query = preset;
    invoke("search_catalog", { query: preset });
  }
  const { sources, catalog } = state;
  return h(
    "section",
    { class: "setting-group", id: "sources" },
    h("h2", {}, "Sources"),
    h("p", { class: "lead" }, "Sources let answers check more than the screen, such as your calendar or your team's issues. Context offers the model only read-only tools unless you allow others in mcp.json."),
    connected(sources, catalog.busy),
    catalog.signInPrompt && h("p", { class: "note", role: "status" }, catalog.signInPrompt),
    h("div", { class: "label" }, "Add a source"),
    searchBox(catalog),
    results(catalog, sources),
    h("div", { class: "catalog" }, catalog.featured.map((entry) => card(entry, sources, catalog.busy, preset))),
    h(
      "p",
      { class: "note" },
      "Advanced: ",
      h("button", { class: "inline-link", type: "button", onclick: () => invoke("edit_sources") }, "edit mcp.json"),
      " to add any MCP server by hand.",
    ),
  );
}

function connected(sources, busy) {
  if (sources.connecting) return h("div", { class: "note" }, "Connecting sources…");
  const rows = sources.servers.map((server) => {
    let detail;
    if (server.needsSignIn) detail = "Needs sign-in";
    else if (server.problem) detail = server.problem;
    else detail = `${server.enabled} ${server.enabled === 1 ? "tool" : "tools"}${server.hidden ? `, ${server.hidden} hidden` : ""}`;
    const actions = [];
    if (server.needsSignIn) {
      actions.push(button("Sign in", "primary small", () => invoke("source_sign_in", { name: server.name }), { disabled: busy }));
    } else if (server.signedIn) {
      actions.push(button("Sign out", "ghost small", () => invoke("source_sign_out", { name: server.name })));
    }
    actions.push(button("Remove", "danger small", () => invoke("remove_source", { name: server.name })));
    return h(
      "div",
      { class: "connected" },
      h("span", { class: server.needsSignIn ? "dot idle" : server.problem ? "dot error" : "dot" }),
      h("div", { class: "who" }, h("b", { class: "truncate" }, server.name), h("small", { class: "truncate", title: detail }, detail)),
      h("div", { class: "row" }, actions),
    );
  });
  if (rows.length === 0) rows.push(h("p", { class: "note" }, "No sources yet. Add one below."));
  if (sources.problem) rows.unshift(h("p", { class: "note error" }, sources.problem));
  return h("div", { class: "connected-list" }, rows, busy && h("div", { class: "asking" }, dots(), "Working on it… finish any sign-in in your browser."));
}

/** Whether a catalog entry is already connected. */
function added(entry, sources) {
  const name = entry.name.toLowerCase();
  return sources.servers.some((server) => server.name.toLowerCase() === name || server.name === slug(entry.name));
}

function slug(name) {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function card(entry, sources, busy, preset) {
  const isAdded = added(entry, sources);
  const highlight = preset && `${entry.name} ${entry.description}`.toLowerCase().includes(preset.toLowerCase());
  const action = isAdded
    ? h("span", { class: "note" }, "Added")
    : !entry.available
      ? h("span", { class: "note" }, entry.note ?? "Unavailable")
      : button(entry.signIn && entry.fields.length === 0 ? "Connect" : "Add", "secondary small", () => start(entry), { disabled: busy });
  return h(
    "div",
    { class: highlight ? "catalog-card highlight" : "catalog-card" },
    h("b", {}, entry.name),
    h("small", {}, entry.description),
    open === entry.id ? form(entry, busy) : h("div", { class: "row" }, action),
  );
}

function start(entry) {
  if (entry.fields.length === 0) {
    invoke("add_source", { entry, values: {} });
    return;
  }
  open = entry.id;
  document.dispatchEvent(new CustomEvent("context-rerender"));
}

function form(entry, busy) {
  const values = forms.get(entry.id) ?? {};
  forms.set(entry.id, values);
  const install = entry.install;
  const where =
    install.kind === "remote" ? `Connects to ${install.url}` : install.kind === "package" ? `Runs ${install.command} ${install.args.join(" ")}` : "";
  return h(
    "div",
    { class: "add-form" },
    where && h("small", { class: "mono" }, where),
    entry.fields.map((field) => {
      const input = h("input", {
        class: "field",
        type: field.secret ? "password" : "text",
        placeholder: field.description || field.name,
        autocomplete: "off",
        spellcheck: "false",
        oninput: (event) => (values[field.name] = event.target.value),
      });
      input.value = values[field.name] ?? "";
      return h("label", { class: "form-field" }, h("span", {}, field.name, field.required ? " *" : ""), input);
    }),
    h(
      "div",
      { class: "row" },
      button("Add", "primary small", () => {
        open = null;
        forms.delete(entry.id);
        invoke("add_source", { entry, values });
      }, { disabled: busy }),
      button("Cancel", "ghost small", () => {
        open = null;
        document.dispatchEvent(new CustomEvent("context-rerender"));
      }),
    ),
    h("p", { class: "note" }, "Secrets are stored encrypted on this computer, not in mcp.json."),
  );
}

function searchBox(catalog) {
  const input = h("input", {
    class: "field",
    placeholder: "Search the MCP Registry, for example calendar or jira",
    spellcheck: "false",
    oninput: (event) => (query = event.target.value),
  });
  input.value = query;
  const run = () => invoke("search_catalog", { query });
  input.addEventListener("keydown", (event) => event.key === "Enter" && run());
  return h("div", { class: "row" }, input, button(catalog.searching ? "Searching…" : "Search", "secondary", run, { disabled: catalog.searching }));
}

function results(catalog, sources) {
  if (catalog.error) return h("p", { class: "note error" }, catalog.error);
  if (!catalog.results) return "";
  if (catalog.results.length === 0) return h("p", { class: "note" }, `Nothing found for “${catalog.query}”.`);
  return h(
    "div",
    { class: "catalog" },
    catalog.results.map((entry) => card(entry, sources, catalog.busy, null)),
    h("p", { class: "note" }, "Search results come from the public MCP Registry and are run by third parties. Check a server before adding it."),
  );
}
