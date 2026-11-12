// ① メイン画面の右側（選択中の接続先の詳細）

import { api, type Target, type TargetRuntime } from "../api";
import { STATUS_LABEL, envBadge, errMsg, formatElapsed, h, icon, replaceChildren, statusDot } from "./dom";
import { openEditDialog } from "./edit-dialog";
import { confirmProd } from "./prod-dialog";
import { on, runtimeOf, selectedTarget, showView, store } from "./store";
import { setTerminalTarget, terminalEl, updateTitle } from "./terminal";
import { flashCopied, toast } from "./toast";

const HOST = "127.0.0.1";
const MASK = "••••••••••••";
const REVEAL_MS = 30_000;

const headEl = h("section", { class: "detail-head" });
const cardsEl = h("section", { class: "detail-cards" });
const emptyEl = h("div", { class: "detail-empty" });

export const detailEl = h("main", { class: "detail", id: "main" }, emptyEl, headEl, cardsEl, terminalEl);

// パスワードの一時表示
let revealed: { id: string; value: string; timer: number } | null = null;

function hideRevealed(): void {
  if (revealed) {
    window.clearTimeout(revealed.timer);
    revealed = null;
  }
}

// ---- 操作 ----

async function doConnect(t: Target): Promise<void> {
  if (store.connecting.has(t.id)) return;
  let confirmed = false;
  if (t.env === "prod") {
    if (!(await confirmProd(t))) return;
    confirmed = true;
  }
  store.connecting.add(t.id);
  render();
  try {
    await api.connect(t.id, confirmed);
  } catch (e) {
    toast(errMsg(e), "error");
  } finally {
    store.connecting.delete(t.id);
    render();
  }
}

async function doDisconnect(t: Target): Promise<void> {
  try {
    await api.disconnect(t.id);
  } catch (e) {
    toast(errMsg(e), "error");
  }
}

async function copy(btn: HTMLElement, text: string): Promise<void> {
  try {
    await api.copyText(text);
    flashCopied(btn);
  } catch (e) {
    toast(errMsg(e), "error");
  }
}

// ---- 描画 ----

function statusLine(t: Target, rt: TargetRuntime): HTMLElement {
  const cls = `status-line status-${rt.status}`;
  if (rt.status === "connected") {
    const elapsed = h("span", { "data-elapsed": String(rt.connectedAt ?? Date.now()) });
    elapsed.textContent = formatElapsed(Date.now() - (rt.connectedAt ?? Date.now()));
    return h(
      "div",
      { class: cls },
      statusDot(rt.status),
      h("span", null, "接続中（", elapsed, `） · localhost:${t.localPort} → RDS:3306`),
    );
  }
  if (rt.status === "error") {
    return h("div", { class: cls, role: "alert" }, statusDot(rt.status), h("span", null, rt.message || "エラー"));
  }
  return h(
    "div",
    { class: cls },
    statusDot(rt.status),
    h("span", null, rt.message ? `${STATUS_LABEL[rt.status]} · ${rt.message}` : STATUS_LABEL[rt.status]),
  );
}

function actionButtons(t: Target, rt: TargetRuntime): HTMLElement[] {
  const edit = h("button", { type: "button", class: "btn", "data-fk": "edit" }, icon("edit"), "編集");
  edit.addEventListener("click", () => openEditDialog(t));

  const prereqOk = store.prereq?.ok ?? false;
  const prereqTitle = store.prereq
    ? prereqOk
      ? null
      : "前提ツールが見つからないため接続できません"
    : "前提ツールを確認しています…";

  let main: HTMLButtonElement;
  switch (rt.status) {
    case "connected":
    case "loggingIn":
    case "preparing":
      main = h("button", { type: "button", class: "btn btn-danger", "data-fk": "main" }, icon("stop"), "停止");
      main.addEventListener("click", () => doDisconnect(t));
      break;
    case "stopping":
      main = h("button", { type: "button", class: "btn btn-danger", disabled: true, "data-fk": "main" }, icon("stop"), "停止中…");
      break;
    default: {
      const retry = rt.status === "error";
      const pending = store.connecting.has(t.id);
      main = h(
        "button",
        {
          type: "button",
          class: "btn btn-primary",
          disabled: !prereqOk || pending,
          title: prereqTitle,
          "data-fk": "main",
        },
        icon(retry ? "retry" : "play"),
        pending ? "接続しています…" : retry ? "再接続" : "接続",
      );
      main.addEventListener("click", () => doConnect(t));
    }
  }
  return [edit, main];
}

function kv(label: string, value: string, cls = "mono"): HTMLElement[] {
  return [h("span", { class: "label" }, label), h("span", { class: cls }, value)];
}

function copyButton(label: string, getText: () => string, disabled = false): HTMLButtonElement {
  const b = h(
    "button",
    { type: "button", class: "icon-btn", "aria-label": `${label}をコピー`, title: `${label}をコピー`, disabled, "data-fk": `copy-${label}` },
    icon("copy"),
  );
  b.addEventListener("click", () => copy(b, getText()));
  return b;
}

function settingsCard(t: Target, rt: TargetRuntime): HTMLElement {
  return h(
    "div",
    { class: "card" },
    h("h2", { class: "card-title" }, "接続先の設定"),
    h(
      "div",
      { class: "kv-grid" },
      kv("プロファイル", t.profile),
      kv("環境", t.env),
      kv("アプリ名", t.app),
      kv("ローカルポート", String(t.localPort)),
      kv("踏み台", rt.bastionId || "—"),
      kv("RDS", rt.rdsEndpoint || "—", "mono wrap-any"),
    ),
  );
}

function dbCard(t: Target, rt: TargetRuntime): HTMLElement {
  const port = String(t.localPort);
  const user = rt.dbUser;
  const hasPw = rt.hasPassword;
  const showing = revealed && revealed.id === t.id ? revealed.value : null;

  const pwText = h("span", { class: showing ? "mono pw-plain" : "mono pw-mask" }, showing ?? MASK);
  const eye = h(
    "button",
    {
      type: "button",
      class: "icon-btn icon-btn-sm",
      "aria-label": showing ? "パスワードを隠す" : "パスワードを表示",
      title: showing ? "パスワードを隠す" : "パスワードを表示",
      "aria-pressed": showing ? "true" : "false",
      disabled: !hasPw,
      "data-fk": "eye",
    },
    icon(showing ? "eyeOff" : "eye"),
  );
  eye.addEventListener("click", async () => {
    if (revealed && revealed.id === t.id) {
      hideRevealed();
      render();
      return;
    }
    try {
      const value = await api.revealPassword(t.id);
      if (store.selectedId !== t.id) return;
      hideRevealed();
      revealed = {
        id: t.id,
        value,
        timer: window.setTimeout(() => {
          revealed = null;
          render();
        }, REVEAL_MS),
      };
      render();
    } catch (e) {
      toast(errMsg(e), "error");
    }
  });

  const pwCopy = h(
    "button",
    { type: "button", class: "icon-btn", "aria-label": "パスワードをコピー", title: "パスワードをコピー", disabled: !hasPw, "data-fk": "copy-pw" },
    icon("copy"),
  );
  pwCopy.addEventListener("click", async () => {
    try {
      await api.copyPassword(t.id);
      flashCopied(pwCopy);
    } catch (e) {
      toast(errMsg(e), "error");
    }
  });

  const cmd = `mysql -h ${HOST} -P ${port} -u ${user ?? "<user>"} -p`;

  return h(
    "div",
    { class: "card" },
    h("h2", { class: "card-title" }, "DB接続情報"),
    h(
      "div",
      { class: "db-grid" },
      h("span", { class: "label" }, "Host"),
      h("span", { class: "mono" }, HOST),
      copyButton("Host", () => HOST),
      h("span", { class: "label" }, "Port"),
      h("span", { class: "mono" }, port),
      copyButton("Port", () => port),
      h("span", { class: "label" }, "User"),
      h("span", { class: "mono" }, user ?? "—"),
      copyButton("User", () => user ?? "", !user),
      h("span", { class: "label" }, "Password"),
      h("span", { class: "pw-cell" }, pwText, eye),
      pwCopy,
    ),
    h(
      "div",
      { class: "cmd-box" },
      h("code", { class: "mono" }, cmd),
      copyButton("接続コマンド", () => cmd),
    ),
    h("span", { class: "label" }, "コピーしたパスワードは30秒後にクリップボードから消去"),
  );
}

function renderEmpty(): void {
  if (store.targets.length === 0) {
    const add = h("button", { type: "button", class: "btn btn-primary" }, icon("plus", 14, 2.5), "接続先を追加");
    add.addEventListener("click", () => openEditDialog());
    const imp = h("button", { type: "button", class: "btn" }, icon("download"), "JSONからインポート");
    imp.addEventListener("click", () => showView("settings"));
    replaceChildren(
      emptyEl,
      h("div", { class: "empty-icon" }, icon("db", 28, 1.6)),
      h("h1", null, "接続先がまだありません"),
      h("p", null, "「＋」で接続先を追加するか、「設定・インポート」からJSONファイルを取り込んでください。"),
      h("div", { class: "empty-actions" }, add, imp),
    );
  } else {
    replaceChildren(
      emptyEl,
      h("div", { class: "empty-icon" }, icon("db", 28, 1.6)),
      h("p", null, "左の一覧から接続先を選択してください。"),
    );
  }
}

export function render(): void {
  const t = selectedTarget();
  const has = !!t;
  emptyEl.hidden = has;
  headEl.hidden = !has;
  cardsEl.hidden = !has;
  terminalEl.hidden = !has;
  if (!t) {
    hideRevealed();
    renderEmpty();
    setTerminalTarget(null);
    return;
  }
  const rt = runtimeOf(t.id);
  if (revealed && (revealed.id !== t.id || rt.status !== "connected" || !rt.hasPassword)) hideRevealed();

  const fk = (document.activeElement as HTMLElement | null)?.getAttribute?.("data-fk");
  const focusInDetail = !!fk && (headEl.contains(document.activeElement) || cardsEl.contains(document.activeElement));

  replaceChildren(
    headEl,
    h(
      "div",
      { class: "detail-title-wrap" },
      h("div", { class: "detail-title" }, h("h1", null, t.name), envBadge(t.env)),
      statusLine(t, rt),
    ),
    actionButtons(t, rt),
  );
  replaceChildren(cardsEl, settingsCard(t, rt), dbCard(t, rt));
  updateTitle(t);

  if (focusInDetail) {
    const el = detailEl.querySelector<HTMLElement>(`[data-fk="${CSS.escape(fk!)}"]`);
    if (el && !(el as HTMLButtonElement).disabled) el.focus();
  }
}

function tick(): void {
  const el = headEl.querySelector<HTMLElement>("[data-elapsed]");
  if (el) el.textContent = formatElapsed(Date.now() - Number(el.dataset.elapsed));
}

export function initDetail(): void {
  on("selection", () => {
    hideRevealed();
    setTerminalTarget(selectedTarget() ?? null);
    render();
  });
  on("targets", render);
  on("prereq", render);
  on("runtime", (rt) => {
    if ((rt as TargetRuntime).id === store.selectedId) render();
  });
  window.setInterval(tick, 1000);
  setTerminalTarget(selectedTarget() ?? null);
  render();
}
