import type { Issue } from "./protocol.ts";

export interface ProbeResult {
  title: string;
  scroll_width: number;
  client_width: number;
  issues: Issue[];
  /** href attribute values of up to 40 anchors (never form values or text). */
  links: string[];
}

/**
 * Runs INSIDE the page (serialized by Playwright's `page.evaluate`), so it must be self-contained:
 * no imports, no closures over module scope. DOM checks only (no contrast, no ARIA tree): this is a
 * smoke test for common mistakes, not an accessibility audit. It reads structure, never form values
 * or text content beyond a short selector label.
 */
export function domProbe(): ProbeResult {
  const MAX_SAMPLES = 5;
  const issues: Issue[] = [];
  const label = (el: Element): string => {
    const id = el.getAttribute("id");
    const cls = (el.getAttribute("class") ?? "").trim().split(/\s+/)[0] ?? "";
    return (el.tagName.toLowerCase() + (id ? `#${id}` : cls ? `.${cls}` : "")).slice(0, 80);
  };
  const add = (rule: string, els: Element[]): void => {
    if (els.length > 0) issues.push({ rule, count: els.length, samples: els.slice(0, MAX_SAMPLES).map(label) });
  };
  const hasName = (el: Element): boolean =>
    (el.textContent ?? "").trim().length > 0 ||
    el.hasAttribute("aria-label") ||
    el.hasAttribute("aria-labelledby") ||
    el.hasAttribute("title") ||
    el.querySelector("img[alt]:not([alt=''])") !== null;

  add("a11y.img_alt", Array.from(document.querySelectorAll("img:not([alt])")));
  add(
    "a11y.input_label",
    Array.from(document.querySelectorAll("input:not([type=hidden]):not([type=submit]):not([type=button]):not([type=reset]):not([type=image]), select, textarea")).filter((el) => {
      if (el.hasAttribute("aria-label") || el.hasAttribute("aria-labelledby") || el.hasAttribute("title")) return false;
      if (el.closest("label") !== null) return false;
      const id = el.getAttribute("id");
      return !(id !== null && id !== "" && document.querySelector(`label[for="${CSS.escape(id)}"]`) !== null);
    }),
  );
  add("a11y.button_name", Array.from(document.querySelectorAll("button, [role=button]")).filter((el) => !hasName(el)));
  add("a11y.link_name", Array.from(document.querySelectorAll("a[href]")).filter((el) => !hasName(el)));
  if (!document.documentElement.getAttribute("lang")) add("a11y.lang", [document.documentElement]);
  if ((document.title ?? "").trim() === "") add("a11y.title", [document.documentElement]);
  if (document.querySelector("meta[name=viewport]") === null) add("responsive.viewport_meta", [document.documentElement]);

  const vw = document.documentElement.clientWidth;
  const wide = Array.from(document.body?.querySelectorAll("*") ?? []).filter((el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.right > vw + 1;
  });
  add("layout.element_overflow", wide.slice(0, 200));

  return {
    title: (document.title ?? "").slice(0, 120),
    scroll_width: document.documentElement.scrollWidth,
    client_width: vw,
    issues,
    links: Array.from(document.querySelectorAll("a[href]")).slice(0, 40).map((a) => a.getAttribute("href") ?? ""),
  };
}
