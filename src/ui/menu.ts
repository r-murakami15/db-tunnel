// ポップアップメニュー（一覧の右クリックメニュー・「＋」の追加メニュー）。
// 同時に開くのは1つだけ。Escape・枠外クリック・スクロール・ウィンドウのフォーカス喪失・選択後に閉じる。

import { h, icon } from "./dom";

export interface MenuItem {
  label: string;
  icon?: string;
  /** アイコンを角丸のタイルで表示する（追加メニュー用） */
  tile?: "primary" | "plain";
  disabled?: boolean;
  /** 無効の理由など（ツールチップ） */
  title?: string;
  action?: () => void;
  /** サブメニュー（ホバー・クリック・→キーで開く） */
  submenu?: MenuItem[];
}

export type MenuAnchor =
  /** 指定位置（右クリックした位置など）。はみ出す場合は左・上に反転 */
  | { kind: "point"; x: number; y: number }
  /** 要素の下に右揃え。はみ出す場合は上に出す */
  | { kind: "below-end"; el: HTMLElement };

export interface MenuOptions {
  /** 開いているメニューの識別用 */
  id: string;
  label: string;
  items: MenuItem[];
  anchor: MenuAnchor;
  /** true なら最初の項目にフォーカス（キーボードで開いたとき）。false ならメニュー自体にフォーカス */
  focusFirst: boolean;
  /** 閉じたときにフォーカスを戻す先 */
  returnFocus: () => HTMLElement | null | undefined;
  /** この要素の上での pointerdown は枠外クリックとして扱わない（開閉ボタン用） */
  toggle?: HTMLElement;
  className?: string;
  onClose?: () => void;
}

interface Built {
  el: HTMLDivElement;
  buttons: HTMLButtonElement[];
}

interface OpenMenu {
  opts: MenuOptions;
  root: Built;
  sub: Built | null;
  subParent: HTMLButtonElement | null;
  detach: () => void;
}

const EDGE = 8;

let current: OpenMenu | null = null;

export function openMenuId(): string | null {
  return current?.opts.id ?? null;
}

function isDisabled(b: HTMLButtonElement): boolean {
  return b.getAttribute("aria-disabled") === "true";
}

function build(items: MenuItem[], label: string, className: string, onActivate: (b: HTMLButtonElement, item: MenuItem) => void, onHover: (b: HTMLButtonElement, item: MenuItem) => void): Built {
  const el = h("div", { class: `menu ${className}`, role: "menu", "aria-label": label, tabindex: -1 });
  const buttons = items.map((item) => {
    const b = h(
      "button",
      {
        type: "button",
        class: item.tile ? "mi mi-tall" : "mi",
        role: "menuitem",
        tabindex: -1,
        title: item.title ?? null,
        "aria-disabled": item.disabled ? "true" : null,
        "aria-haspopup": item.submenu ? "menu" : null,
        "aria-expanded": item.submenu ? "false" : null,
      },
      item.icon
        ? item.tile
          ? h("span", { class: `mi-tile mi-tile-${item.tile}` }, icon(item.icon, 14, item.tile === "primary" ? 2.5 : 2))
          : icon(item.icon, 14)
        : null,
      h("span", { class: "mi-label" }, item.label),
      item.submenu ? icon("chevronRight", 12, 2.5) : null,
    );
    b.addEventListener("click", () => onActivate(b, item));
    b.addEventListener("mouseenter", () => onHover(b, item));
    return b;
  });
  el.append(...buttons);
  return { el, buttons };
}

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(v, max));
}

/** ウィンドウ内に収まるように配置する */
function place(el: HTMLElement, anchor: MenuAnchor): void {
  const w = el.offsetWidth;
  const hgt = el.offsetHeight;
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  let left: number;
  let top: number;
  if (anchor.kind === "point") {
    left = anchor.x + w > vw - EDGE ? anchor.x - w : anchor.x;
    top = anchor.y + hgt > vh - EDGE ? anchor.y - hgt : anchor.y;
  } else {
    const r = anchor.el.getBoundingClientRect();
    left = r.right - w;
    top = r.bottom + 6;
    if (top + hgt > vh - EDGE) top = r.top - 6 - hgt;
  }
  el.style.left = `${clamp(left, EDGE, vw - w - EDGE)}px`;
  el.style.top = `${clamp(top, EDGE, vh - hgt - EDGE)}px`;
}

function placeSub(sub: HTMLElement, root: HTMLElement, parent: HTMLElement): void {
  const w = sub.offsetWidth;
  const hgt = sub.offsetHeight;
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const mr = root.getBoundingClientRect();
  const pr = parent.getBoundingClientRect();
  let left = mr.right + 4;
  if (left + w > vw - EDGE) left = mr.left - 4 - w;
  let top = pr.top - 6;
  if (top + hgt > vh - EDGE) top = pr.bottom + 6 - hgt;
  sub.style.left = `${clamp(left, EDGE, vw - w - EDGE)}px`;
  sub.style.top = `${clamp(top, EDGE, vh - hgt - EDGE)}px`;
}

function focusItem(b: HTMLButtonElement | undefined): void {
  b?.focus({ preventScroll: true });
}

/** メニューを閉じる。フォーカスがメニュー内にあれば呼び出し元へ戻す */
export function closeMenu(): void {
  const m = current;
  if (!m) return;
  current = null;
  m.detach();
  const active = document.activeElement;
  const focusInside =
    !active || active === document.body || m.root.el.contains(active) || !!m.sub?.el.contains(active);
  m.root.el.remove();
  m.sub?.el.remove();
  if (focusInside) m.opts.returnFocus()?.focus({ preventScroll: false });
  m.opts.onClose?.();
}

export function openMenu(opts: MenuOptions): void {
  closeMenu();

  const closeSub = (focusParent: boolean) => {
    const m = current;
    if (!m || !m.sub) return;
    m.sub.el.remove();
    m.subParent?.setAttribute("aria-expanded", "false");
    m.subParent?.classList.remove("is-open");
    if (focusParent) focusItem(m.subParent ?? undefined);
    m.sub = null;
    m.subParent = null;
  };

  const openSub = (parent: HTMLButtonElement, item: MenuItem, focusFirst: boolean) => {
    const m = current;
    if (!m || !item.submenu || isDisabled(parent)) return;
    if (m.subParent === parent && m.sub) {
      if (focusFirst) focusItem(m.sub.buttons.find((b) => !isDisabled(b)) ?? m.sub.buttons[0]);
      return;
    }
    closeSub(false);
    const sub = build(item.submenu, item.label, `${opts.className ?? ""} menu-sub`, activate, hoverSub);
    sub.el.style.visibility = "hidden";
    document.body.appendChild(sub.el);
    placeSub(sub.el, m.root.el, parent);
    sub.el.style.visibility = "";
    sub.el.addEventListener("keydown", onKey);
    m.sub = sub;
    m.subParent = parent;
    parent.setAttribute("aria-expanded", "true");
    parent.classList.add("is-open");
    if (focusFirst) focusItem(sub.buttons.find((b) => !isDisabled(b)) ?? sub.buttons[0]);
  };

  function activate(b: HTMLButtonElement, item: MenuItem): void {
    if (isDisabled(b)) return;
    if (item.submenu) {
      openSub(b, item, true);
      return;
    }
    closeMenu();
    item.action?.();
  }

  function hoverRoot(b: HTMLButtonElement, item: MenuItem): void {
    focusItem(b);
    if (item.submenu && !isDisabled(b)) openSub(b, item, false);
    else closeSub(false);
  }

  function hoverSub(b: HTMLButtonElement): void {
    focusItem(b);
  }

  const root = build(opts.items, opts.label, opts.className ?? "", activate, hoverRoot);

  function onKey(ev: KeyboardEvent): void {
    const m = current;
    if (!m) return;
    const inSub = !!m.sub && m.sub.el.contains(ev.target as Node);
    const menu = inSub ? m.sub! : m.root;
    const idx = menu.buttons.indexOf(document.activeElement as HTMLButtonElement);
    const n = menu.buttons.length;
    switch (ev.key) {
      case "ArrowDown":
        focusItem(menu.buttons[idx < 0 ? 0 : (idx + 1) % n]);
        break;
      case "ArrowUp":
        focusItem(menu.buttons[idx < 0 ? n - 1 : (idx - 1 + n) % n]);
        break;
      case "Home":
        focusItem(menu.buttons[0]);
        break;
      case "End":
        focusItem(menu.buttons[n - 1]);
        break;
      case "ArrowRight": {
        if (inSub || idx < 0) break;
        const item = opts.items[idx];
        if (item.submenu) openSub(menu.buttons[idx], item, true);
        break;
      }
      case "ArrowLeft":
        if (inSub) closeSub(true);
        break;
      case "Escape":
        if (inSub) closeSub(true);
        else closeMenu();
        break;
      case "Tab":
        closeMenu();
        break;
      default:
        return;
    }
    ev.preventDefault();
    ev.stopPropagation();
  }
  root.el.addEventListener("keydown", onKey);

  const onPointerDown = (ev: PointerEvent) => {
    const m = current;
    if (!m) return;
    const t = ev.target as Node;
    if (m.root.el.contains(t) || m.sub?.el.contains(t) || opts.toggle?.contains(t)) return;
    closeMenu();
  };
  // 開く直前のフォーカス移動などによるスクロールは無視する（scroll イベントは非同期に届く）
  const openedAt = performance.now();
  const onScroll = (ev: Event) => {
    if (performance.now() - openedAt < 150) return;
    // 呼び出し元を含む領域（一覧やページ全体）がスクロールしたときだけ閉じる。
    // ターミナルの自動スクロールなど無関係な領域のスクロールでは閉じない
    const t = ev.target;
    const anchor = opts.returnFocus();
    if (t === document || (t instanceof Node && !!anchor && t.contains(anchor))) closeMenu();
  };
  const onBlur = () => closeMenu();
  document.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("scroll", onScroll, true);
  window.addEventListener("resize", onBlur);
  window.addEventListener("blur", onBlur);

  current = {
    opts,
    root,
    sub: null,
    subParent: null,
    detach: () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onBlur);
      window.removeEventListener("blur", onBlur);
    },
  };

  root.el.style.visibility = "hidden";
  document.body.appendChild(root.el);
  place(root.el, opts.anchor);
  root.el.style.visibility = "";
  if (opts.focusFirst) focusItem(root.buttons.find((b) => !isDisabled(b)) ?? root.buttons[0]);
  else root.el.focus({ preventScroll: true });
}

const NON_TEXT_INPUTS = new Set(["checkbox", "radio", "button", "submit", "reset", "image", "file", "range", "color"]);

/** WebView 標準の右クリックメニューを抑止する（文字入力欄は除く） */
export function suppressNativeContextMenu(): void {
  document.addEventListener("contextmenu", (ev) => {
    if (ev.defaultPrevented) return;
    const t = ev.target as HTMLElement | null;
    if (t instanceof HTMLTextAreaElement || t?.isContentEditable) return;
    if (t instanceof HTMLInputElement && !NON_TEXT_INPUTS.has(t.type)) return;
    ev.preventDefault();
  });
}
