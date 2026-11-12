// ② 設定画面

import { save } from "@tauri-apps/plugin-dialog";
import { api, type ToolStatus } from "../api";
import { errMsg, h, icon, replaceChildren } from "./dom";
import { INSTALL_URL, focusSettingsButton, installLink } from "./header";
import { emit, on, setPrereq, showView, store } from "./store";
import { toast } from "./toast";

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
  h(
    "p",
    { class: "body-text" },
    "登録済みの接続先をJSONで書き出します。メンバーへの配布に使えます。取り込みは一覧の「＋」→「JSONインポート」から行います。",
  ),
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
  h("header", { class: "app-header" }, backBtn, h("h1", { class: "settings-title" }, "設定")),
  h(
    "div",
    { class: "settings-scroll" },
    h(
      "div",
      { class: "settings-grid" },
      h("div", { class: "settings-col" }, exportCard, configCard),
      toolsCard,
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
  renderConfig();
  renderTools();
}
