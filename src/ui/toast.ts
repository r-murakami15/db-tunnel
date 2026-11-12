// 画面右下のトーストと、コピー時の小さな吹き出し。

import { h, icon } from "./dom";

export type ToastKind = "info" | "success" | "warning" | "error";

let region: HTMLDivElement | null = null;
let live: HTMLDivElement | null = null;

function ensureRegion(): HTMLDivElement {
  if (!region) {
    region = h("div", { class: "toast-region" });
    document.body.appendChild(region);
  }
  return region;
}

/** スクリーンリーダー向けの通知 */
export function announce(text: string): void {
  if (!live) {
    live = h("div", { class: "sr-only", "aria-live": "polite", role: "status" });
    document.body.appendChild(live);
  }
  live.textContent = "";
  window.setTimeout(() => {
    if (live) live.textContent = text;
  }, 30);
}

export function toast(message: string | string[], kind: ToastKind = "info"): void {
  const lines = Array.isArray(message) ? message : [message];
  const close = h(
    "button",
    { type: "button", class: "toast-close", "aria-label": "閉じる" },
    icon("x", 14),
  );
  const el = h(
    "div",
    { class: `toast toast-${kind}`, role: kind === "error" ? "alert" : "status" },
    h(
      "div",
      { class: "toast-body" },
      lines.map((l) => h("div", null, l)),
    ),
    close,
  );
  const remove = () => {
    el.classList.add("toast-leave");
    window.setTimeout(() => el.remove(), 180);
  };
  close.addEventListener("click", remove);
  ensureRegion().appendChild(el);
  const ms = kind === "error" ? 9000 : kind === "warning" ? 7000 : 4000;
  let timer = window.setTimeout(remove, ms);
  el.addEventListener("mouseenter", () => window.clearTimeout(timer));
  el.addEventListener("mouseleave", () => (timer = window.setTimeout(remove, 2500)));
}

/** ボタンのそばに「コピーしました」を短く出す */
export function flashCopied(anchor: HTMLElement, text = "コピーしました"): void {
  const r = anchor.getBoundingClientRect();
  const tip = h("div", { class: "copied-tip", "aria-hidden": "true" }, text);
  document.body.appendChild(tip);
  const w = tip.offsetWidth;
  const left = Math.min(Math.max(8, r.left + r.width / 2 - w / 2), window.innerWidth - w - 8);
  tip.style.left = `${left}px`;
  tip.style.top = `${Math.max(8, r.top - tip.offsetHeight - 6)}px`;
  announce(text);
  window.setTimeout(() => tip.classList.add("copied-tip-leave"), 1100);
  window.setTimeout(() => tip.remove(), 1350);
}
