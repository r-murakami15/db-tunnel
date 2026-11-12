// メイン画面のヘッダー（ロゴ・前提ツールの状態・設定ボタン）

import { openUrl } from "@tauri-apps/plugin-opener";
import type { ToolStatus } from "../api";
import { errMsg, h, icon, replaceChildren } from "./dom";
import { on, showView, store } from "./store";
import { toast } from "./toast";

export const INSTALL_URL = {
  aws: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
  plugin: "https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html",
};

export function openDocs(url: string): void {
  openUrl(url).catch((e) => toast(errMsg(e), "error"));
}

/** インストール手順へのリンク（外部ブラウザで開く） */
export function installLink(url: string, label = "インストール手順"): HTMLButtonElement {
  const b = h("button", { type: "button", class: "link-btn" }, label, icon("external", 12));
  b.addEventListener("click", () => openDocs(url));
  return b;
}

const pillsEl = h("div", { class: "pills", "aria-live": "polite" });

function pill(name: string, tool: ToolStatus, url: string): HTMLElement {
  if (tool.found) {
    return h(
      "span",
      { class: "pill pill-ok", title: tool.path ?? "" },
      icon("check", 12, 3),
      tool.version ? `${name} ${tool.version}` : name,
    );
  }
  return h(
    "span",
    { class: "pill pill-bad", title: tool.error ?? "" },
    icon("warn", 12, 2.5),
    `${name} が見つかりません`,
    installLink(url),
  );
}

function render(): void {
  const p = store.prereq;
  if (!p) {
    replaceChildren(pillsEl, h("span", { class: "pill pill-wait" }, "前提ツールを確認しています…"));
    return;
  }
  replaceChildren(
    pillsEl,
    pill("AWS CLI", p.awsCli, INSTALL_URL.aws),
    pill("Session Manager Plugin", p.sessionManagerPlugin, INSTALL_URL.plugin),
  );
}

const settingsBtn = h("button", { type: "button", class: "btn" }, icon("gear"), "設定・インポート");
settingsBtn.addEventListener("click", () => showView("settings"));

export const headerEl = h(
  "header",
  { class: "app-header" },
  h(
    "div",
    { class: "brand" },
    h("div", { class: "logo" }, icon("db", 16, 2)),
    h("span", { class: "brand-name" }, "DB Tunnel"),
  ),
  pillsEl,
  settingsBtn,
);

export function initHeader(): void {
  on("prereq", render);
  render();
}

export function focusSettingsButton(): void {
  settingsBtn.focus();
}
