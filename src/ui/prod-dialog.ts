// ④ prod接続の確認ダイアログ

import type { Target } from "../api";
import { h, icon } from "./dom";

export function openModal(dialog: HTMLDialogElement): void {
  document.body.appendChild(dialog);
  dialog.addEventListener("close", () => {
    dialog.remove();
    // 開く前にフォーカスしていた要素が描き直しで消えていたら、選択中の行へ戻す
    window.setTimeout(() => {
      if (document.activeElement && document.activeElement !== document.body) return;
      if (document.querySelector("dialog[open]")) return;
      document.querySelector<HTMLElement>(".target-row.is-selected")?.focus();
    }, 0);
  });
  dialog.showModal();
}

/** prod接続の確認。接続するなら true */
export function confirmProd(target: Target): Promise<boolean> {
  return new Promise((resolve) => {
    const cancel = h("button", { type: "button", class: "btn btn-lg", autofocus: true }, "キャンセル");
    const ok = h("button", { type: "button", class: "btn btn-lg btn-danger" }, "接続する");
    const warn = icon("warn", 22);
    warn.setAttribute("role", "img");
    warn.setAttribute("aria-label", "警告");
    warn.removeAttribute("aria-hidden");
    warn.classList.add("prod-warn-icon");

    const dialog = h(
      "dialog",
      {
        class: "modal modal-prod",
        role: "alertdialog",
        "aria-labelledby": "pc-title",
        "aria-describedby": "pc-desc",
      },
      h(
        "div",
        { class: "prod-body" },
        h("div", { class: "prod-title" }, warn, h("h1", { id: "pc-title" }, "本番環境（prod）に接続します")),
        h(
          "div",
          { class: "prod-facts" },
          h("span", { class: "prod-key" }, "接続先"),
          h("span", { class: "prod-name" }, target.name),
          h("span", { class: "prod-key" }, "プロファイル"),
          h("span", { class: "mono" }, target.profile),
          h("span", { class: "prod-key" }, "ローカルポート"),
          h("span", { class: "mono" }, String(target.localPort)),
        ),
        h("p", { id: "pc-desc", class: "prod-desc" }, "本番データベースへの操作は慎重に行ってください。続行しますか？"),
        h("div", { class: "dialog-actions-end" }, cancel, ok),
      ),
    );

    let result = false;
    cancel.addEventListener("click", () => dialog.close());
    ok.addEventListener("click", () => {
      result = true;
      dialog.close();
    });
    // 枠外クリックでは閉じない（誤操作防止）。Escape は標準動作でキャンセル扱い。
    dialog.addEventListener("close", () => resolve(result));
    openModal(dialog);
    cancel.focus();
  });
}
