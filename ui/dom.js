// Small helpers shared by the pages. Text from the model, tools or the screen
// is only ever set as text, never parsed as HTML.

const tauri = window.__TAURI__;

export const invoke = (command, args) => tauri.core.invoke(command, args);
export const listen = (event, handler) => tauri.event.listen(event, (e) => handler(e.payload));

/** Builds an element. `props` sets attributes, `class`, `on*` handlers and `style`. */
export function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") element.className = value;
    else if (key === "style") {
      // Keys are CSS property names, such as `--size` or `aspect-ratio`.
      for (const [name, css] of Object.entries(value)) element.style.setProperty(name, css);
    }
    else if (key.startsWith("on")) element.addEventListener(key.slice(2).toLowerCase(), value);
    else element.setAttribute(key, value === true ? "" : value);
  }
  append(element, children);
  return element;
}

function append(element, children) {
  for (const child of children.flat(Infinity)) {
    if (child === undefined || child === null || child === false) continue;
    element.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

export function button(label, variant, onclick, extra = {}) {
  return h("button", { class: `button ${variant}`, type: "button", onclick, ...extra }, label);
}

export const logo = (size) => h("span", { class: "logo", style: { "--size": `${size}px` } });

export const dots = () => h("span", { class: "dots" }, h("i"), h("i"), h("i"));

/** Shows a shortcut such as `Ctrl+Alt+Space` as separate key caps. */
export function keys(shortcut) {
  const mac = navigator.userAgent.includes("Mac");
  const label = mac
    ? shortcut.replace("Ctrl+", "⌃").replace("Alt+", "⌥").replace("Shift+", "⇧")
    : shortcut;
  const parts = mac ? [...label.replace("Space", "")].concat("Space") : label.split("+");
  return h("span", { class: "keys" }, parts.filter(Boolean).map((key) => h("kbd", {}, key)));
}
