// ① メイン画面の左サイドバー（検索・追加・プロファイル別一覧）

import type { Target } from "../api";
import { STATUS_LABEL, envBadge, h, icon, replaceChildren, statusDot } from "./dom";
import { openEditDialog } from "./edit-dialog";
import { on, runtimeOf, select, showView, store } from "./store";

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
  { type: "button", class: "icon-btn icon-btn-primary", "aria-label": "接続先を追加", title: "接続先を追加" },
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

function matches(t: Target, q: string): boolean {
  if (!q) return true;
  return [t.name, t.profile, t.app].some((s) => s.toLowerCase().includes(q));
}

function row(t: Target): HTMLButtonElement {
  const rt = runtimeOf(t.id);
  const selected = t.id === store.selectedId;
  const btn = h(
    "button",
    {
      type: "button",
      class: selected ? "target-row is-selected" : "target-row",
      "aria-current": selected ? "true" : null,
      "data-id": t.id,
    },
    statusDot(rt.status),
    h(
      "span",
      { class: "target-row-text" },
      h("span", { class: "target-row-name" }, t.name),
      h("span", { class: "label" }, `${STATUS_LABEL[rt.status]} · :${t.localPort}`),
    ),
    envBadge(t.env),
  );
  btn.addEventListener("click", () => select(t.id));
  return btn;
}

function render(): void {
  const focusedId = (document.activeElement as HTMLElement | null)?.closest?.(".target-row")?.getAttribute("data-id");

  if (store.targets.length === 0) {
    const toSettings = h("button", { type: "button", class: "link-btn" }, "設定・インポート");
    toSettings.addEventListener("click", () => showView("settings"));
    replaceChildren(
      listEl,
      h(
        "div",
        { class: "list-empty" },
        h("p", { class: "list-empty-title" }, "接続先がありません"),
        h("p", null, "「＋」で追加するか、", toSettings, " からJSONをインポートしてください。"),
      ),
    );
    return;
  }

  const q = store.search.trim().toLowerCase();
  const groups = new Map<string, Target[]>();
  for (const t of store.targets) {
    if (!matches(t, q)) continue;
    let g = groups.get(t.profile);
    if (!g) groups.set(t.profile, (g = []));
    g.push(t);
  }
  if (groups.size === 0) {
    replaceChildren(listEl, h("p", { class: "list-none" }, "一致する接続先がありません"));
    return;
  }
  replaceChildren(
    listEl,
    [...groups].map(([profile, ts]) => {
      const gid = `grp-${profile}`;
      return h(
        "div",
        { class: "target-group", role: "group", "aria-label": `プロファイル: ${profile}` },
        h("div", { class: "group-label", id: gid }, `プロファイル: ${profile}`),
        ts.map(row),
      );
    }),
  );
  if (focusedId) listEl.querySelector<HTMLElement>(`.target-row[data-id="${CSS.escape(focusedId)}"]`)?.focus();
}

/** 1行だけ更新（状態イベント用） */
function updateRow(id: string): void {
  const old = listEl.querySelector<HTMLElement>(`.target-row[data-id="${CSS.escape(id)}"]`);
  const t = store.targets.find((x) => x.id === id);
  if (!old || !t) return;
  const hadFocus = old === document.activeElement;
  const fresh = row(t);
  old.replaceWith(fresh);
  if (hadFocus) fresh.focus();
}

export function initSidebar(): void {
  searchIn.addEventListener("input", () => {
    store.search = searchIn.value;
    render();
  });
  addBtn.addEventListener("click", () => openEditDialog());
  on("targets", render);
  on("selection", render);
  on("runtime", (rt) => updateRow((rt as { id: string }).id));
  render();
}

export function focusSearch(): void {
  searchIn.focus();
}
