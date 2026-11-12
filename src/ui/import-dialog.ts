// ⑨ JSONインポートのダイアログ（「＋」→「JSONインポート」から開く）

import { open } from "@tauri-apps/plugin-dialog";
import { api, type ImportMode, type ImportPreview, type ImportRow } from "../api";
import { envBadge, errMsg, h, icon, replaceChildren } from "./dom";
import { openModal } from "./prod-dialog";
import { refreshTargets } from "./store";
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

let seq = 0;
let opening = false;

/** ファイル選択ダイアログで JSON を選ぶ。キャンセル時は null */
async function pickJson(): Promise<string | null> {
  try {
    const sel = await open({ multiple: false, directory: false, filters: [{ name: "JSON", extensions: ["json"] }] });
    return typeof sel === "string" ? sel : null;
  } catch (e) {
    toast(errMsg(e), "error");
    return null;
  }
}

/** ファイルを選び、プレビューできたらインポートダイアログを開く */
export async function startImport(): Promise<void> {
  if (opening) return;
  opening = true;
  try {
    const path = await pickJson();
    if (!path) return;
    let preview: ImportPreview;
    try {
      preview = await api.importPreview(path);
    } catch (e) {
      toast(errMsg(e), "error");
      return;
    }
    openImportDialog(preview);
  } finally {
    opening = false;
  }
}

function selectable(r: ImportRow): boolean {
  return r.judgement !== "error";
}

function openImportDialog(initial: ImportPreview): void {
  const p = `imp${++seq}`;
  let preview: ImportPreview | null = initial;
  let mode: ImportMode = "overwrite";
  const checked = new Set<number>();
  let error: string | null = null;
  let busy = false;

  const resetChecks = () => {
    checked.clear();
    if (preview) for (const r of preview.rows) if (selectable(r)) checked.add(r.index);
  };
  resetChecks();

  /** 新規のみ追加モードでは上書き対象をスキップ扱いにする */
  const skipped = (r: ImportRow) => mode === "newOnly" && r.judgement === "overwrite";
  const willImport = (r: ImportRow) => selectable(r) && !skipped(r) && checked.has(r.index);

  const body = h("div", { class: "dialog-body import-body" });
  const pickBtn = h("button", { type: "button", class: "btn btn-sm" }, icon("download"), "別のファイルを選択");
  const cancelBtn = h("button", { type: "button", class: "btn btn-lg" }, "キャンセル");
  const applyBtn = h("button", { type: "button", class: "btn btn-lg btn-primary" });

  const formatDetails = h(
    "details",
    { class: "format-details" },
    h("summary", null, "JSONの形式"),
    h("pre", { class: "mono code-dark" }, SAMPLE_JSON),
    h("span", { class: "label" }, "localPort は省略可（省略時は13306）。パスワード等の秘密情報は含めません。"),
  );

  const dialog = h(
    "dialog",
    { class: "modal modal-import", "aria-labelledby": `${p}-title` },
    h("div", { class: "dialog-head dialog-head-row" }, h("h1", { id: `${p}-title` }, "JSONインポート"), pickBtn),
    body,
    h("div", { class: "dialog-foot dialog-foot-end" }, h("div", { class: "dialog-actions" }, cancelBtn, applyBtn)),
  );

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

  function render(): void {
    const parts: Node[] = [];
    if (error) {
      parts.push(h("div", { class: "msg msg-error", role: "alert" }, icon("warn", 14), h("span", null, error)));
    }
    if (preview) {
      const pv = preview;
      const radio = (value: ImportMode, label: string) => {
        const r = h("input", { type: "radio", name: `${p}-mode`, value, checked: mode === value, disabled: busy });
        r.addEventListener("change", () => {
          if (r.checked) {
            mode = value;
            render();
          }
        });
        return h("label", { class: "radio" }, r, label);
      };

      parts.push(
        h(
          "div",
          { class: "import-meta" },
          h("span", { class: "label" }, "読み込んだファイル:"),
          h("span", { class: "mono" }, `${pv.fileName}（${pv.rows.length}件）`),
          h("span", { class: "grow" }),
          h("div", { class: "radio-group", role: "radiogroup", "aria-label": "取り込み方法" }, [
            radio("overwrite", "同じ組み合わせは上書き"),
            radio("newOnly", "新規のみ追加"),
          ]),
        ),
      );

      if (pv.warnings.length) {
        parts.push(
          h(
            "div",
            { class: "msg msg-warn" },
            icon("warn", 14),
            h(
              "div",
              null,
              pv.warnings.map((w) => h("div", null, w)),
            ),
          ),
        );
      }

      const rows = pv.rows.map((r) => {
        const can = selectable(r) && !skipped(r);
        const cb = h("input", {
          type: "checkbox",
          checked: can && checked.has(r.index),
          disabled: !can || busy,
          "aria-label": can ? `${r.name} ${r.env} を取り込む` : `${r.name || "（名前なし）"} は取り込めません`,
        });
        cb.addEventListener("change", () => {
          if (cb.checked) checked.add(r.index);
          else checked.delete(r.index);
          render();
          body.querySelector<HTMLInputElement>(`tr[data-index="${r.index}"] input`)?.focus();
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
    }
    parts.push(formatDetails);
    // 作り直してもスクロール位置を保つ
    const scroll = body.scrollTop;
    replaceChildren(body, parts);
    body.scrollTop = scroll;

    const n = preview ? preview.rows.filter(willImport).length : 0;
    applyBtn.textContent = `${n}件を取り込む`;
    applyBtn.disabled = n === 0 || busy;
    cancelBtn.disabled = busy;
    pickBtn.disabled = busy;
  }

  pickBtn.addEventListener("click", async () => {
    if (busy) return;
    const path = await pickJson();
    if (!path) return;
    busy = true;
    render();
    try {
      preview = await api.importPreview(path);
      error = null;
      resetChecks();
    } catch (e) {
      // 古いプレビューのまま取り込まないように破棄する
      preview = null;
      checked.clear();
      error = errMsg(e);
      toast(error, "error");
    } finally {
      busy = false;
      render();
    }
  });

  applyBtn.addEventListener("click", async () => {
    if (!preview || busy) return;
    // 新規のみ追加では、チェック済みの上書き対象も渡してスキップ件数に含める
    const indices = preview.rows.filter((r) => selectable(r) && checked.has(r.index)).map((r) => r.index);
    busy = true;
    render();
    try {
      const res = await api.importApply(mode, indices);
      toast(`追加 ${res.added}件・更新 ${res.updated}件・スキップ ${res.skipped}件`, "success");
      busy = false;
      dialog.close();
      await refreshTargets().catch((e) => toast(errMsg(e), "error"));
    } catch (e) {
      error = errMsg(e);
      busy = false;
      render();
    }
  });

  cancelBtn.addEventListener("click", () => dialog.close());
  dialog.addEventListener("cancel", (ev) => {
    if (busy) ev.preventDefault();
  });

  render();
  openModal(dialog);
}
