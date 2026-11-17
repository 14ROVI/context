// A small Markdown renderer for model text. It builds elements directly and
// sets every piece of text as text, so nothing in the input is parsed as HTML.
// Supports paragraphs, line breaks, headings, bullet and numbered lists, code
// blocks, and inline bold, italic, code and links. Links show their text with
// the address on hover; they are not followed, since the webview must not
// navigate away.

import { h } from "./dom.js";

const INLINE = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*|__[^_\n]+__)|(\*[^*\s][^*\n]*\*|_[^_\s][^_\n]*_)|(\[[^\]\n]+\]\([^)\s]+\))/g;

/** Renders inline Markdown (bold, italic, code, links) as a list of nodes. */
export function inline(text) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    const [token, code, bold, italic, link] = match;
    if (code) nodes.push(h("code", {}, code.slice(1, -1)));
    else if (bold) nodes.push(h("strong", {}, inline(bold.slice(2, -2))));
    else if (italic) nodes.push(h("em", {}, inline(italic.slice(1, -1))));
    else if (link) {
      const [, label, url] = link.match(/^\[(.+)\]\((.+)\)$/);
      nodes.push(h("span", { class: "link", title: url }, inline(label)));
    } else nodes.push(token);
    last = match.index + token.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

/** Renders a block of Markdown as a list of block elements. */
export function markdown(text) {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks = [];
  let paragraph = [];
  let list = null;

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    const children = [];
    paragraph.forEach((line, i) => {
      if (i > 0) children.push(h("br"));
      children.push(inline(line));
    });
    blocks.push(h("p", {}, children));
    paragraph = [];
  };
  const flushList = () => {
    if (list) blocks.push(list.element);
    list = null;
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    if (trimmed.startsWith("```")) {
      flushParagraph();
      flushList();
      const code = [];
      while (++i < lines.length && !lines[i].trim().startsWith("```")) code.push(lines[i]);
      blocks.push(h("pre", {}, h("code", {}, code.join("\n"))));
      continue;
    }
    if (!trimmed) {
      flushParagraph();
      flushList();
      continue;
    }
    const heading = trimmed.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      flushParagraph();
      flushList();
      blocks.push(h("p", { class: "heading" }, inline(heading[2])));
      continue;
    }
    const item = trimmed.match(/^([-*+]|\d+[.)])\s+(.*)$/);
    if (item) {
      flushParagraph();
      const tag = /\d/.test(item[1]) ? "ol" : "ul";
      if (!list || list.tag !== tag) {
        flushList();
        list = { tag, element: h(tag) };
      }
      list.element.append(h("li", {}, inline(item[2])));
      continue;
    }
    if (list && /^\s/.test(line)) {
      // An indented line continues the previous list item.
      list.element.lastChild.append(" ", ...inline(trimmed));
      continue;
    }
    flushList();
    paragraph.push(trimmed);
  }
  flushParagraph();
  flushList();
  return blocks;
}
