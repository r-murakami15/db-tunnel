// 画面の組み立てと起動処理

import { ask } from "@tauri-apps/plugin-dialog";
import { api, events } from "./api";
import { detailEl, initDetail } from "./ui/detail";
import { errMsg, h, icon } from "./ui/dom";
import { headerEl, initHeader } from "./ui/header";
import { initSettings, settingsEl } from "./ui/settings";
import { initSidebar, sidebarEl } from "./ui/sidebar";
import { emit, on, select, setPrereq, setRuntime, setTargets, store } from "./ui/store";
import { forgetLogs, initTerminal } from "./ui/terminal";
import { toast } from "./ui/toast";

const bannerEl = h("div", { class: "banner-area" });

function showLoadErrorBanner(message: string): void {
  const close = h("button", { type: "button", class: "icon-btn icon-btn-sm banner-close", "aria-label": "閉じる" }, icon("x", 14));
  const banner = h(
    "div",
    { class: "banner banner-warn", role: "alert" },
    icon("warn", 16),
    h(
      "div",
      { class: "banner-text" },
      h("strong", null, "設定ファイルを読み込めませんでした"),
      h("span", null, message),
    ),
    close,
  );
  close.addEventListener("click", () => banner.remove());
  bannerEl.appendChild(banner);
}

const mainScreen = h(
  "div",
  { class: "main-screen" },
  headerEl,
  bannerEl,
  h("div", { class: "main-body" }, sidebarEl, detailEl),
);

export async function start(root: HTMLElement): Promise<void> {
  settingsEl.hidden = true;
  root.append(mainScreen, settingsEl);

  on("view", () => {
    mainScreen.hidden = store.view !== "main";
    settingsEl.hidden = store.view !== "settings";
  });
  on("targets", () => forgetLogs(new Set(store.targets.map((t) => t.id))));

  // イベントは初期データの取得より先に購読しておく
  await Promise.all([
    events.onTargetStatus(setRuntime),
    initTerminal(),
    events.onQuitRequested(async (count) => {
      const yes = await ask(`接続中のセッションが ${count} 件あります。すべて切断して終了しますか？`, {
        title: "DB Tunnel",
        kind: "warning",
        okLabel: "終了する",
        cancelLabel: "キャンセル",
      });
      if (yes) api.quitApp().catch((e) => toast(errMsg(e), "error"));
    }),
  ]).catch((e) => console.error("listen", e));

  initHeader();
  initSidebar();
  initDetail();
  initSettings();

  // 前提ツールのチェックは時間がかかることがあるので待たない
  api
    .checkPrerequisites()
    .then(setPrereq)
    .catch((e) => toast(`前提ツールを確認できませんでした: ${errMsg(e)}`, "error"));

  api
    .getConfigInfo()
    .then((c) => {
      store.configInfo = c;
      emit("config");
      if (c.loadError) showLoadErrorBanner(c.loadError);
    })
    .catch((e) => console.warn("get_config_info", e));

  const [targets, runtimes, settings] = await Promise.all([
    api.listTargets().catch((e) => {
      toast(`接続先を読み込めませんでした: ${errMsg(e)}`, "error");
      return [];
    }),
    api.listRuntimes().catch(() => []),
    api.getSettings().catch(() => null),
  ]);
  store.settings = settings;
  for (const rt of runtimes) {
    // イベントで既に新しい状態が届いていれば上書きしない
    if (!store.runtimes.has(rt.id)) store.runtimes.set(rt.id, rt);
  }
  setTargets(targets);
  const last = settings?.lastSelectedId;
  const initial = last && store.targets.some((t) => t.id === last) ? last : store.targets[0]?.id ?? null;
  select(initial);
}
