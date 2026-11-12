// ② 設定・インポート画面

import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type ImportMode, type ImportPreview, type ImportRow, type ToolStatus } from "../api";
import { envBadge, errMsg, h, icon, replaceChildren } from "./dom";
import { INSTALL_URL, focusSettingsButton, installLink } from "./header";
import { emit, on, refreshTargets, setPrereq, showView, store } from "./store";
import { toast } from "./toast";

const SAMPLE_JSON = `{
  "version": 1,
  "targets": [
    {
      "name": "Acme Portal",
      "profile": "acme-portal",
      "env": "dev",
      "app": "acme-portal",
      "localPort": 13306
    }
  ]
}`;

// ---------------- JSONインポート ----------------

let preview: ImportPreview | null = null;
let mode: ImportMode = "overwrite";
const checked = new Set<number>();
let importError: string | null = null;
let importBusy = false;

const importBody = h("div", { class: "import-body" });
const pickBtn = h("button", { type: "button", class: "btn" }, icon("download"), "JSONファイルを選択");

function selectable(r: ImportRow): boolean {
  return r.judgement !== "error";
}

/** 新規のみ追加モードでは上書き対象をスキップ扱いにする */
function skipped(r: ImportRow): boolean {
  return mode === "newOnly" && r.judgement === "overwrite";
}

function willImport(r: ImportRow): boolean {
  return selectable(r) && !skipped(r) && checked.has(r.index);
}

function judgementCell(r: ImportRow): HTMLElement {
  const main =
    r.judgement === "error"
      ? h("span", { class: "j-error" }, `エラー: ${r.errors.join(" / ") || "取り込めません"}`)
      : skipped(r)
        ? h("span", { class: "j-skip" }, "スキップ（既存あり）")
        : r.judgement === "overwrite"
          ? h("span", { class: "j-over" }, "上書き")
          : h("span", { class: "j-new" }, "新規");
  return h(
    "td",
    { class: "j-cell" },
    main,
    r.warnings.map((w) => h("div", { class: "row-warn" }, icon("warn", 12), h("span", null, w))),
  );
}

function renderImport(): void {
  const parts: Node[] = [];
  if (importError) {
    parts.push(h("div", { class: "msg msg-error", role: "alert" }, icon("warn", 14), h("span", null, importError)));
  }
  if (!preview) {
    if (!importError) {
      parts.push(
        h(
          "p",
          { class: "muted" },
          "接続先の一覧をJSONファイルから取り込みます。取り込む前に内容と判定（新規・上書き・エラー）を確認できます。",
        ),
      );
    }
    replaceChildren(importBody, parts);
    return;
  }

  const p = preview;
  const radio = (value: ImportMode, label: string) => {
    const r = h("input", { type: "radio", name: "import-mode", value, checked: mode === value });
    r.addEventListener("change", () => {
      if (r.checked) {
        mode = value;
        renderImport();
      }
    });
    return h("label", { class: "radio" }, r, label);
  };

  parts.push(
    h(
      "div",
      { class: "import-meta" },
      h("span", { class: "label" }, "読み込んだファイル:"),
      h("span", { class: "mono" }, `${p.fileName}（${p.rows.length}件）`),
      h("span", { class: "grow" }),
      h("div", { class: "radio-group", role: "radiogroup", "aria-label": "取り込み方法" }, [
        radio("overwrite", "同じ組み合わせは上書き"),
        radio("newOnly", "新規のみ追加"),
      ]),
    ),
  );

  if (p.warnings.length) {
    parts.push(
      h(
        "div",
        { class: "msg msg-warn" },
        icon("warn", 14),
        h(
          "div",
          null,
          p.warnings.map((w) => h("div", null, w)),
        ),
      ),
    );
  }

  const rows = p.rows.map((r) => {
    const can = selectable(r) && !skipped(r);
    const cb = h("input", {
      type: "checkbox",
      checked: can && checked.has(r.index),
      disabled: !can,
      "aria-label": can ? `${r.name} ${r.env} を取り込む` : `${r.name || "（名前なし）"} は取り込めません`,
    });
    cb.addEventListener("change", () => {
      if (cb.checked) checked.add(r.index);
      else checked.delete(r.index);
      renderImport();
      importBody.querySelector<HTMLInputElement>(`tr[data-index="${r.index}"] input`)?.focus();
    });
    return h(
      "tr",
      { class: r.judgement === "error" ? "row-error" : skipped(r) ? "row-skip" : "", "data-index": String(r.index) },
      h("td", null, cb),
      h("td", null, r.name || "—"),
      h("td", { class: "mono" }, r.profile || "—"),
      h("td", null, envBadge(r.env)),
      h("td", { class: "mono" }, r.app || "—"),
      h("td", { class: "mono" }, r.localPort === null ? "13306（省略）" : String(r.localPort)),
      judgementCell(r),
    );
  });

  parts.push(
    h(
      "div",
      { class: "table-wrap" },
      h(
        "table",
        { class: "import-table" },
        h(
          "thead",
          null,
          h(
            "tr",
            null,
            ["取込", "表示名", "プロファイル", "環境", "アプリ名", "ポート", "判定"].map((c) => h("th", { scope: "col" }, c)),
          ),
        ),
        h("tbody", null, rows),
      ),
    ),
  );

  const n = p.rows.filter(willImport).length;
  const cancel = h("button", { type: "button", class: "btn", disabled: importBusy }, "キャンセル");
  cancel.addEventListener("click", resetImport);
  const apply = h(
    "button",
    { type: "button", class: "btn btn-primary", disabled: n === 0 || importBusy },
    `${n}件を取り込む`,
  );
  apply.addEventListener("click", doApply);
  parts.push(h("div", { class: "card-actions" }, cancel, apply));
  replaceChildren(importBody, parts);
}

function resetImport(): void {
  preview = null;
  importError = null;
  checked.clear();
  renderImport();
}

async function pickFile(): Promise<void> {
  let path: string | null;
  try {
    const sel = await open({ multiple: false, directory: false, filters: [{ name: "JSON", extensions: ["json"] }] });
    path = typeof sel === "string" ? sel : null;
  } catch (e) {
    toast(errMsg(e), "error");
    return;
  }
  if (!path) return;
  pickBtn.disabled = true;
  try {
    const p = await api.importPreview(path);
    preview = p;
    importError = null;
    mode = "overwrite";
    checked.clear();
    for (const r of p.rows) if (selectable(r)) checked.add(r.index);
  } catch (e) {
    preview = null;
    importError = errMsg(e);
  } finally {
    pickBtn.disabled = false;
  }
  renderImport();
}

async function doApply(): Promise<void> {
  if (!preview || importBusy) return;
  // 新規のみ追加では、チェック済みの上書き対象も渡してスキップ件数に含める
  const indices = preview.rows.filter((r) => selectable(r) && checked.has(r.index)).map((r) => r.index);
  importBusy = true;
  renderImport();
  try {
    const res = await api.importApply(mode, indices);
    toast(`追加 ${res.added}件・更新 ${res.updated}件・スキップ ${res.skipped}件`, "success");
    preview = null;
    checked.clear();
    importError = null;
    await refreshTargets().catch((e) => toast(errMsg(e), "error"));
  } catch (e) {
    importError = errMsg(e);
  } finally {
    importBusy = false;
    renderImport();
  }
}

pickBtn.addEventListener("click", pickFile);

const importCard = h(
  "section",
  { class: "card card-lg span-all", "aria-labelledby": "imp-title" },
  h("div", { class: "card-head" }, h("h2", { id: "imp-title" }, "JSONからインポート"), pickBtn),
  importBody,
);

// ---------------- JSONの形式 ----------------

const formatCard = h(
  "section",
  { class: "card card-lg" },
  h("h2", null, "JSONの形式"),
  h("pre", { class: "mono code-dark" }, SAMPLE_JSON),
  h("span", { class: "label" }, "localPort は省略可（省略時は13306）。パスワード等の秘密情報は含めません。"),
);

// ---------------- エクスポート ----------------

const exportBtn = h("button", { type: "button", class: "btn" }, icon("upload"), "JSONをエクスポート");
exportBtn.addEventListener("click", async () => {
  try {
    const path = await save({
      defaultPath: "db-tunnel-targets.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await api.exportTargets(path);
    toast(`${n}件の接続先をエクスポートしました`, "success");
  } catch (e) {
    toast(errMsg(e), "error");
  }
});

const exportCard = h(
  "section",
  { class: "card card-lg" },
  h("h2", null, "エクスポート"),
  h("p", { class: "body-text" }, "登録済みの接続先をJSONで書き出します。メンバーへの配布に使えます。"),
  h("p", { class: "note-warn" }, "書き出したJSONは暗号化されません（秘密情報は含みません）"),
  h("div", null, exportBtn),
);

// ---------------- 設定ファイル ----------------

const configBody = h("div", { class: "kv-grid kv-grid-narrow" });
const configError = h("div", { class: "msg msg-warn", hidden: true });
const openDirBtn = h("button", { type: "button", class: "btn" }, icon("folder"), "フォルダを開く");
openDirBtn.addEventListener("click", () => api.openConfigDir().catch((e) => toast(errMsg(e), "error")));

function renderConfig(): void {
  const c = store.configInfo;
  if (!c) {
    replaceChildren(configBody, h("span", { class: "label" }, "読み込み中…"));
    return;
  }
  replaceChildren(
    configBody,
    h("span", { class: "label" }, "暗号化"),
    h("span", null, c.encryption),
    h("span", { class: "label" }, "鍵の保存先"),
    c.keychainOk
      ? h("span", { class: "ok-text" }, "OSキーチェーン ✓")
      : h("span", { class: "bad-text" }, "OSキーチェーンを利用できません ✗"),
    h("span", { class: "label" }, "保存場所"),
    h("span", { class: "mono small wrap-any" }, c.path),
  );
  configError.hidden = !c.loadError;
  replaceChildren(configError, icon("warn", 14), h("span", null, c.loadError ?? ""));
}

const configCard = h(
  "section",
  { class: "card card-lg" },
  h("h2", null, "設定ファイル"),
  configError,
  configBody,
  h("div", null, openDirBtn),
);

// ---------------- 前提ツール ----------------

const toolsBody = h("div", { class: "tools" });
const recheckBtn = h("button", { type: "button", class: "btn" }, icon("retry"), "再チェック");
const awsPathIn = h("input", {
  id: "aws-path",
  class: "input input-sm mono",
  type: "text",
  placeholder: "自動検出",
  autocomplete: "off",
  spellcheck: "false",
});
const pluginPathIn = h("input", {
  id: "plugin-path",
  class: "input input-sm mono",
  type: "text",
  placeholder: "自動検出",
  autocomplete: "off",
  spellcheck: "false",
});
const savePathsBtn = h("button", { type: "submit", class: "btn" }, "保存");

function toolRow(name: string, t: ToolStatus, url: string): HTMLElement[] {
  return [
    h(
      "div",
      { class: "tool-name" },
      h("span", null, name),
      t.path ? h("span", { class: "mono small muted wrap-any" }, t.path) : null,
      !t.found && t.error ? h("span", { class: "small bad-text" }, t.error) : null,
      !t.found ? h("span", { class: "small" }, installLink(url)) : null,
    ),
    t.found
      ? h("span", { class: "ok-text" }, `${t.version ?? "検出"} ✓`)
      : h("span", { class: "bad-text" }, "見つかりません ✗"),
  ];
}

function renderTools(): void {
  const p = store.prereq;
  if (!p) {
    replaceChildren(toolsBody, h("span", { class: "label" }, "確認しています…"));
    return;
  }
  replaceChildren(
    toolsBody,
    toolRow("AWS CLI", p.awsCli, INSTALL_URL.aws),
    toolRow("Session Manager Plugin", p.sessionManagerPlugin, INSTALL_URL.plugin),
  );
}

recheckBtn.addEventListener("click", async () => {
  recheckBtn.disabled = true;
  try {
    setPrereq(await api.checkPrerequisites());
    toast("前提ツールを再チェックしました", "info");
  } catch (e) {
    toast(errMsg(e), "error");
  } finally {
    recheckBtn.disabled = false;
  }
});

const pathsForm = h(
  "form",
  { class: "paths-form" },
  h("label", { for: "aws-path", class: "small-label" }, "aws のパス"),
  awsPathIn,
  h("label", { for: "plugin-path", class: "small-label" }, "session-manager-plugin のパス"),
  pluginPathIn,
  h("div", { class: "paths-actions" }, savePathsBtn),
);
pathsForm.addEventListener("submit", async (ev) => {
  ev.preventDefault();
  savePathsBtn.disabled = true;
  try {
    const p = await api.setToolPaths(awsPathIn.value.trim() || null, pluginPathIn.value.trim() || null);
    setPrereq(p);
    toast("ツールのパスを保存しました", "success");
  } catch (e) {
    toast(errMsg(e), "error");
  } finally {
    savePathsBtn.disabled = false;
  }
});

const toolsCard = h(
  "section",
  { class: "card card-lg" },
  h("div", { class: "card-head" }, h("h2", null, "前提ツール"), recheckBtn),
  toolsBody,
  h("hr", { class: "sep" }),
  pathsForm,
  h(
    "span",
    { class: "label" },
    "Finderから起動した場合 PATH が引き継がれないため、見つからないときはパスを指定してください（空欄 = 自動検出）",
  ),
);

// ---------------- 画面全体 ----------------

const backBtn = h("button", { type: "button", class: "back-link" }, icon("back"), "接続一覧へ戻る");
backBtn.addEventListener("click", () => {
  showView("main");
  focusSettingsButton();
});

export const settingsEl = h(
  "div",
  { class: "settings-screen" },
  h("header", { class: "app-header" }, backBtn, h("h1", { class: "settings-title" }, "設定・インポート")),
  h(
    "div",
    { class: "settings-scroll" },
    h(
      "div",
      { class: "settings-grid" },
      importCard,
      formatCard,
      h("div", { class: "settings-col" }, exportCard, configCard, toolsCard),
    ),
  ),
);

async function refresh(): Promise<void> {
  try {
    store.configInfo = await api.getConfigInfo();
    emit("config");
  } catch (e) {
    toast(errMsg(e), "error");
  }
  try {
    const s = await api.getSettings();
    store.settings = s;
    if (document.activeElement !== awsPathIn) awsPathIn.value = s.awsCliPath ?? "";
    if (document.activeElement !== pluginPathIn) pluginPathIn.value = s.pluginPath ?? "";
  } catch (e) {
    console.warn("get_settings", e);
  }
}

export function initSettings(): void {
  on("prereq", renderTools);
  on("config", renderConfig);
  on("view", () => {
    if (store.view === "settings") {
      void refresh();
      backBtn.focus();
    }
  });
  if (store.settings) {
    awsPathIn.value = store.settings.awsCliPath ?? "";
    pluginPathIn.value = store.settings.pluginPath ?? "";
  }
  renderImport();
  renderConfig();
  renderTools();
}
