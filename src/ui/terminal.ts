// ターミナル出力パネル。接続先ごとのログをクライアント側にも保持し、追記は差分だけDOMに足す。

import { save } from "@tauri-apps/plugin-dialog";
import { api, events, type LogLine, type Target } from "../api";
import { errMsg, h } from "./dom";
import { flashCopied, toast } from "./toast";

const MAX_LINES = 5000;

const logs = new Map<string, LogLine[]>();
/** 読み込み中の接続先 → その間に届いたイベント */
const pending = new Map<string, LogLine[]>();

let currentId: string | null = null;
let current: Target | null = null;
let queue: LogLine[] = [];
let rafId = 0;

const titleEl = h("h2", { class: "term-title" });
const autoScroll = h("input", { type: "checkbox", checked: true });
const copyBtn = h("button", { type: "button", class: "term-btn" }, "コピー");
const clearBtn = h("button", { type: "button", class: "term-btn" }, "クリア");
const saveBtn = h("button", { type: "button", class: "term-btn" }, "保存");
const body = h("div", {
  class: "term-body mono",
  role: "log",
  "aria-label": "ターミナル出力",
  tabindex: 0,
});
const emptyEl = h("div", { class: "term-empty" }, "ログはまだありません");

export const terminalEl = h(
  "section",
  { class: "terminal", "aria-labelledby": "term-title" },
  h(
    "div",
    { class: "term-bar" },
    titleEl,
    h("label", { class: "term-check" }, autoScroll, "自動スクロール"),
    copyBtn,
    clearBtn,
    saveBtn,
  ),
  body,
  emptyEl,
);
titleEl.id = "term-title";

function lineEl(l: LogLine): HTMLDivElement {
  return h(
    "div",
    { class: l.stream === "stderr" ? "ll ll-err" : "ll" },
    h("span", { class: "ll-time" }, l.time),
    " ",
    h("span", { class: `ll-src ll-${l.source === "ssm" ? "ssm" : "app"}` }, `[${l.source}]`),
    " ",
    h("span", { class: "ll-text" }, l.text),
  );
}

function lineText(l: LogLine): string {
  return `${l.time} [${l.source}] ${l.text}`;
}

function scrollToEnd(): void {
  if (autoScroll.checked) body.scrollTop = body.scrollHeight;
}

function updateEmpty(): void {
  const n = currentId ? logs.get(currentId)?.length ?? 0 : 0;
  emptyEl.hidden = n > 0 || (currentId !== null && pending.has(currentId));
  body.hidden = n === 0;
}

function renderAll(): void {
  cancelAnimationFrame(rafId);
  rafId = 0;
  queue = [];
  body.textContent = "";
  const lines = currentId ? logs.get(currentId) ?? [] : [];
  const frag = document.createDocumentFragment();
  for (const l of lines) frag.appendChild(lineEl(l));
  body.appendChild(frag);
  updateEmpty();
  body.scrollTop = body.scrollHeight;
}

function flush(): void {
  rafId = 0;
  if (!queue.length) return;
  const frag = document.createDocumentFragment();
  for (const l of queue) frag.appendChild(lineEl(l));
  queue = [];
  body.appendChild(frag);
  let extra = body.childElementCount - MAX_LINES;
  while (extra-- > 0 && body.firstChild) body.removeChild(body.firstChild);
  updateEmpty();
  scrollToEnd();
}

function push(arr: LogLine[], line: LogLine): void {
  arr.push(line);
  if (arr.length > MAX_LINES) arr.splice(0, arr.length - MAX_LINES);
}

function onLine(line: LogLine): void {
  const buf = pending.get(line.targetId);
  if (buf) {
    buf.push(line);
    return;
  }
  const arr = logs.get(line.targetId);
  if (!arr) return; // まだ表示していない接続先は、選んだときにまとめて取得する
  push(arr, line);
  if (line.targetId === currentId) {
    queue.push(line);
    // ウィンドウが隠れている間は rAF が止まるので、待ち行列も上限で切る
    if (queue.length > MAX_LINES) queue.splice(0, queue.length - MAX_LINES);
    if (!rafId) rafId = requestAnimationFrame(flush);
  }
}

function sameLine(a: LogLine, b: LogLine): boolean {
  return a.time === b.time && a.source === b.source && a.stream === b.stream && a.text === b.text;
}

async function load(id: string): Promise<void> {
  if (logs.has(id) || pending.has(id)) return;
  pending.set(id, []);
  updateEmpty();
  let snapshot: LogLine[] = [];
  try {
    snapshot = await api.getLogs(id);
  } catch (e) {
    toast(`ログを取得できませんでした: ${errMsg(e)}`, "error");
  }
  const buffered = pending.get(id) ?? [];
  pending.delete(id);
  const arr = snapshot.slice(-MAX_LINES);
  // 取得中に届いた行のうち、スナップショットに含まれていないものだけを足す
  const tail = arr.slice(-buffered.length - 20);
  for (const l of buffered) {
    if (!tail.some((t) => sameLine(t, l))) push(arr, l);
  }
  logs.set(id, arr);
  if (id === currentId) renderAll();
}

export function setTerminalTarget(t: Target | null): void {
  current = t;
  updateTitle(t);
  const id = t?.id ?? null;
  if (id === currentId) return;
  currentId = id;
  renderAll();
  if (id) void load(id);
}

export function updateTitle(t: Target | null): void {
  if (t && t.id === current?.id) current = t;
  titleEl.textContent = t ? `ターミナル出力 — ${t.name} (${t.env})` : "ターミナル出力";
}

/** 削除された接続先のログを捨てる */
export function forgetLogs(ids: Set<string>): void {
  for (const id of [...logs.keys()]) if (!ids.has(id)) logs.delete(id);
}

autoScroll.addEventListener("change", scrollToEnd);

copyBtn.addEventListener("click", async () => {
  if (!currentId) return;
  const text = (logs.get(currentId) ?? []).map(lineText).join("\n");
  try {
    await api.copyText(text);
    flashCopied(copyBtn);
  } catch (e) {
    toast(errMsg(e), "error");
  }
});

clearBtn.addEventListener("click", async () => {
  const id = currentId;
  if (!id) return;
  try {
    await api.clearLogs(id);
    logs.set(id, []);
    if (id === currentId) renderAll();
  } catch (e) {
    toast(errMsg(e), "error");
  }
});

saveBtn.addEventListener("click", async () => {
  const t = current;
  if (!t) return;
  const safeName = t.name.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, "_");
  try {
    const path = await save({
      defaultPath: `db-tunnel-${safeName}-${t.env}.log`,
      filters: [{ name: "Log", extensions: ["log", "txt"] }],
    });
    if (!path) return;
    await api.saveLogs(t.id, path);
    toast("ログを保存しました", "success");
  } catch (e) {
    toast(errMsg(e), "error");
  }
});

export async function initTerminal(): Promise<void> {
  await events.onLogLine(onLine);
  updateEmpty();
}
