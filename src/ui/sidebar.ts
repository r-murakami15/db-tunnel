// ① メイン画面の左サイドバー（検索・追加メニュー・接続先の一覧・右クリックメニュー）

import { api, type MoveDirection, type Target } from "../api";
import { STATUS_LABEL, envBadge, errMsg, h, icon, replaceChildren, statusDot } from "./dom";
import { openDuplicateDialog, openEditDialog } from "./edit-dialog";
import { startImport } from "./import-dialog";
import { closeMenu, openMenu, openMenuId } from "./menu";
import { on, runtimeOf, select, setTargets, store } from "./store";
import { toast } from "./toast";

const ADD_MENU = "add";
const ROW_MENU = "row";

const searchIn = h("input", {
  id: "search",
  class: "input input-sm",
  type: "search",
  placeholder: "名前・プロファイルで検索",
  autocomplete: "off",
  spellcheck: "false",
});
const addBtn = h(
  "button",
  {
    type: "button",
    class: "icon-btn icon-btn-primary add-btn",
    "aria-label": "追加メニューを開く",
    title: "追加・インポート",
    "aria-haspopup": "menu",
    "aria-expanded": "false",
  },
  icon("plus", 16, 2.5),
);
const listEl = h("nav", { class: "target-list", "aria-label": "接続先一覧" });

export const sidebarEl = h(
  "aside",
  { class: "sidebar" },
  h(
    "div",
    { class: "sidebar-tools" },
    h("label", { for: "search", class: "sr-only" }, "接続先を検索"),
    searchIn,
    addBtn,
  ),
  listEl,
  h(
    "div",
    { class: "lock-note" },
    icon("lock", 14),
    h("span", null, "設定ファイルは暗号化済み（鍵はOSキーチェーンに保存）"),
  ),
);

/** 右クリックメニューを開いている接続先（行に印を付ける） */
let menuTargetId: string | null = null;

function matches(t: Target, q: string): boolean {
  if (!q) return true;
  return [t.name, t.profile, t.app].some((s) => s.toLowerCase().includes(q));
}

function rowEl(id: string): HTMLElement | null {
  return listEl.querySelector<HTMLElement>(`.target-row[data-id="${CSS.escape(id)}"]`);
}

function row(t: Target): HTMLButtonElement {
  const rt = runtimeOf(t.id);
  const selected = t.id === store.selectedId;
  const cls = ["target-row", selected && "is-selected", t.id === menuTargetId && "is-menu-target"].filter(Boolean).join(" ");
  const btn = h(
    "button",
    {
      type: "button",
      class: cls,
      "aria-current": selected ? "true" : null,
      "data-id": t.id,
    },
    statusDot(rt.status),
    h(
      "span",
      { class: "target-row-text" },
      h("span", { class: "target-row-name" }, t.name),
      h("span", { class: "label target-row-sub" }, `${t.profile} · ${STATUS_LABEL[rt.status]} · :${t.localPort}`),
    ),
    envBadge(t.env),
  );
  btn.addEventListener("click", () => select(t.id));
  return btn;
}

function emptyMessage(): HTMLElement {
  const imp = h("button", { type: "button", class: "link-btn" }, "JSONインポート");
  imp.addEventListener("click", () => void startImport());
  return h(
    "div",
    { class: "list-empty" },
    h("p", { class: "list-empty-title" }, "接続先がありません"),
    h("p", null, "＋ から追加するか、", imp, "で取り込んでください"),
  );
}

function render(): void {
  const focusedId = (document.activeElement as HTMLElement | null)?.closest?.(".target-row")?.getAttribute("data-id");

  if (store.targets.length === 0) {
    replaceChildren(listEl, emptyMessage());
    return;
  }

  const q = store.search.trim().toLowerCase();
  const shown = store.targets.filter((t) => matches(t, q));
  if (shown.length === 0) {
    replaceChildren(listEl, h("p", { class: "list-none" }, "一致する接続先がありません"));
    return;
  }
  replaceChildren(listEl, shown.map(row));
  if (focusedId) rowEl(focusedId)?.focus();
}

/** 1行だけ更新（状態イベント用） */
function updateRow(id: string): void {
  const old = rowEl(id);
  const t = store.targets.find((x) => x.id === id);
  if (!old || !t) return;
  const hadFocus = old === document.activeElement;
  const fresh = row(t);
  old.replaceWith(fresh);
  if (hadFocus) fresh.focus();
}

// ---------------- 右クリックメニュー ----------------

async function move(id: string, direction: MoveDirection): Promise<void> {
  try {
    setTargets(await api.moveTarget(id, direction));
  } catch (e) {
    toast(errMsg(e), "error");
  }
}

/** at を省略するとキーボードから開いたものとして行の位置に出す */
function openRowMenu(id: string, at?: { x: number; y: number }): void {
  closeMenu();
  const t = store.targets.find((x) => x.id === id);
  if (!t) return;
  select(id);
  const index = store.targets.findIndex((x) => x.id === id);
  const last = store.targets.length - 1;
  const searching = store.search.trim() !== "";

  let anchor = at;
  if (!anchor) {
    const r = rowEl(id)?.getBoundingClientRect();
    anchor = r ? { x: r.left + 32, y: r.bottom - 6 } : { x: 0, y: 0 };
  }

  const fresh = () => store.targets.find((x) => x.id === id);
  menuTargetId = id;
  rowEl(id)?.classList.add("is-menu-target");

  openMenu({
    id: ROW_MENU,
    label: `${t.name}（${t.env}）の操作`,
    anchor: { kind: "point", ...anchor },
    focusFirst: !at,
    returnFocus: () => rowEl(id),
    items: [
      {
        label: "編集",
        icon: "edit",
        action: () => {
          const cur = fresh();
          if (cur) openEditDialog(cur);
        },
      },
      {
        label: "複製",
        icon: "copy",
        action: () => {
          const cur = fresh();
          if (cur) openDuplicateDialog(cur);
        },
      },
      {
        label: "移動",
        icon: "move",
        disabled: searching,
        title: searching ? "検索中は並べ替えできません" : undefined,
        submenu: [
          { label: "上へ", icon: "arrowUp", disabled: index <= 0, action: () => void move(id, "up") },
          { label: "下へ", icon: "arrowDown", disabled: index >= last, action: () => void move(id, "down") },
        ],
      },
    ],
    onClose: () => {
      menuTargetId = null;
      rowEl(id)?.classList.remove("is-menu-target");
    },
  });
}

// ---------------- 追加メニュー ----------------

function toggleAddMenu(focusFirst: boolean): void {
  if (openMenuId() === ADD_MENU) {
    closeMenu();
    return;
  }
  addBtn.setAttribute("aria-expanded", "true");
  openMenu({
    id: ADD_MENU,
    label: "追加メニュー",
    anchor: { kind: "below-end", el: addBtn },
    focusFirst,
    returnFocus: () => addBtn,
    toggle: addBtn,
    className: "menu-add",
    items: [
      { label: "追加", icon: "plus", tile: "primary", action: () => openEditDialog() },
      { label: "JSONインポート", icon: "download", tile: "plain", action: () => void startImport() },
    ],
    onClose: () => addBtn.setAttribute("aria-expanded", "false"),
  });
}

export function initSidebar(): void {
  searchIn.addEventListener("input", () => {
    store.search = searchIn.value;
    render();
  });

  // キーボード操作（Enter/Space）の click は detail が 0
  addBtn.addEventListener("click", (ev) => toggleAddMenu(ev.detail === 0));
  addBtn.addEventListener("keydown", (ev) => {
    if (ev.key === "ArrowDown" && openMenuId() !== ADD_MENU) {
      ev.preventDefault();
      toggleAddMenu(true);
    }
  });

  listEl.addEventListener("contextmenu", (ev) => {
    const r = (ev.target as HTMLElement).closest<HTMLElement>(".target-row");
    if (!r) return;
    ev.preventDefault();
    const id = r.dataset.id!;
    // キーボードで開いた直後に届く contextmenu は無視する
    if (openMenuId() === ROW_MENU && menuTargetId === id) return;
    const fromKeyboard = ev.clientX === 0 && ev.clientY === 0;
    openRowMenu(id, fromKeyboard ? undefined : { x: ev.clientX, y: ev.clientY });
  });
  listEl.addEventListener("keydown", (ev) => {
    if (ev.key !== "ContextMenu" && !(ev.key === "F10" && ev.shiftKey)) return;
    const r = (ev.target as HTMLElement).closest<HTMLElement>(".target-row");
    if (!r) return;
    ev.preventDefault();
    openRowMenu(r.dataset.id!);
  });

  on("targets", render);
  on("selection", render);
  on("runtime", (rt) => updateRow((rt as { id: string }).id));
  render();
}

export function focusSearch(): void {
  searchIn.focus();
}
