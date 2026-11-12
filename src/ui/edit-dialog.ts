// ③ 接続先の追加・編集ダイアログ

import { ask } from "@tauri-apps/plugin-dialog";
import { api, type Env, type Target } from "../api";
import { errMsg, h } from "./dom";
import { openModal } from "./prod-dialog";
import { isActive, on, refreshTargets, runtimeOf, select, store } from "./store";
import { toast } from "./toast";

const DEFAULT_PORT = 13306;
const APP_RE = /^[a-z0-9-]+$/;
const ENVS: Env[] = ["dev", "stg", "prod"];

let seq = 0;

interface Field {
  input: HTMLInputElement;
  error: HTMLSpanElement;
}

function field(label: string, input: HTMLInputElement, extra: Node[] = [], help?: string): { el: HTMLDivElement; f: Field } {
  const id = input.id;
  const error = h("span", { class: "field-error", id: `${id}-err`, hidden: true });
  const helpEl = help ? h("span", { class: "help", id: `${id}-help` }, help) : null;
  input.setAttribute("aria-describedby", [helpEl ? `${id}-help` : "", `${id}-err`].filter(Boolean).join(" "));
  const el = h("div", { class: "field" }, h("label", { for: id }, label), input, error, ...extra, helpEl);
  return { el, f: { input, error } };
}

function setError(f: Field, msg: string | null): void {
  f.error.textContent = msg ?? "";
  f.error.hidden = !msg;
  if (msg) f.input.setAttribute("aria-invalid", "true");
  else f.input.removeAttribute("aria-invalid");
}

export function openEditDialog(target?: Target): void {
  const isEdit = !!target;
  const p = `ed${++seq}`;

  const nameIn = h("input", {
    id: `${p}-name`,
    class: "input",
    type: "text",
    maxlength: 64,
    autocomplete: "off",
    value: target?.name ?? "",
    autofocus: true,
  });
  const listId = `${p}-profiles`;
  const datalist = h("datalist", { id: listId });
  const profileIn = h("input", {
    id: `${p}-profile`,
    class: "input mono",
    type: "text",
    list: listId,
    autocomplete: "off",
    spellcheck: "false",
    value: target?.profile ?? "",
    placeholder: "プロファイル名",
  });
  const appIn = h("input", {
    id: `${p}-app`,
    class: "input mono",
    type: "text",
    autocomplete: "off",
    spellcheck: "false",
    value: target?.app ?? "",
    placeholder: "例: acme-portal",
  });
  const portIn = h("input", {
    id: `${p}-port`,
    class: "input mono input-port",
    type: "number",
    min: 1024,
    max: 65535,
    step: 1,
    value: String(target?.localPort ?? DEFAULT_PORT),
  });

  // 環境（セグメント型3択、実体はラジオボタン）
  let env: Env = target?.env ?? "dev";
  const envRadios: HTMLInputElement[] = [];
  const envSegs = ENVS.map((e) => {
    const r = h("input", { type: "radio", name: `${p}-env`, value: e, checked: e === env });
    envRadios.push(r);
    r.addEventListener("change", () => {
      if (r.checked) {
        env = e;
        paintEnv();
        updatePreview();
      }
    });
    return h("label", { class: `seg seg-${e}` }, r, e);
  });
  const paintEnv = () => envSegs.forEach((s, i) => s.classList.toggle("seg-on", ENVS[i] === env));
  paintEnv();
  const envSet = h(
    "fieldset",
    { class: "field fieldset" },
    h("legend", null, "環境"),
    h("div", { class: "segmented" }, envSegs),
  );

  // 検索条件のプレビュー
  const pvBastion = h("span", { class: "mono" });
  const pvRds = h("span", { class: "mono" });
  const preview = h(
    "div",
    { class: "search-preview", "aria-live": "polite" },
    h("span", { class: "help strong" }, "この条件で検索します"),
    h("span", null, h("span", { class: "help" }, "踏み台:"), " ", pvBastion),
    h("span", null, h("span", { class: "help" }, "RDS:"), " ", pvRds),
  );
  const updatePreview = () => {
    const app = appIn.value.trim() || "<アプリ名>";
    pvBastion.textContent = `tag:Name = *${app}*-${env}*bastion*`;
    pvRds.textContent = `${app}* … -${env}-mysql`;
  };
  updatePreview();

  const fName = field("表示名", nameIn);
  const fProfile = field("プロファイル", profileIn, [datalist], "~/.aws/config のプロファイルから選択（手入力も可）");
  const fApp = field("アプリ名（検索プレフィックス）", appIn, [preview]);
  const fPort = field("ローカルポート", portIn, [], "初期値 13306。1024〜65535、ほかの接続先と同時に使う場合は重複不可");

  const activeNote = h(
    "p",
    { class: "note-warn", hidden: true },
    "接続中は環境とポートを変更できません（停止してから変更）",
  );
  const formError = h("div", { class: "form-error", role: "alert", hidden: true });

  const form = h(
    "form",
    { class: "dialog-form", novalidate: true, id: `${p}-form` },
    formError,
    fName.el,
    fProfile.el,
    envSet,
    fApp.el,
    fPort.el,
    activeNote,
  );

  const cancelBtn = h("button", { type: "button", class: "btn btn-lg" }, "キャンセル");
  const saveBtn = h("button", { type: "submit", class: "btn btn-lg btn-primary", form: `${p}-form` }, "保存");
  const deleteBtn = isEdit ? h("button", { type: "button", class: "btn btn-lg btn-danger-outline" }, "削除") : h("span");

  const dialog = h(
    "dialog",
    { class: "modal modal-edit", "aria-labelledby": `${p}-title` },
    h("div", { class: "dialog-head" }, h("h1", { id: `${p}-title` }, isEdit ? "接続先を編集" : "接続先を追加")),
    form,
    h("div", { class: "dialog-foot" }, deleteBtn, h("div", { class: "dialog-actions" }, cancelBtn, saveBtn)),
  );

  // 接続中は環境・ポートを変更不可
  const applyActive = () => {
    const active = isEdit && isActive(runtimeOf(target!.id).status);
    envRadios.forEach((r) => (r.disabled = active));
    envSet.classList.toggle("is-disabled", active);
    portIn.disabled = active;
    activeNote.hidden = !active;
    if (active) {
      // 変更途中の値は元に戻す
      env = target!.env;
      envRadios.forEach((r) => (r.checked = r.value === env));
      portIn.value = String(target!.localPort);
      paintEnv();
      updatePreview();
    }
  };
  applyActive();
  const unsub = on("runtime", (rt) => {
    if (isEdit && (rt as { id: string }).id === target!.id) applyActive();
  });

  appIn.addEventListener("input", () => {
    updatePreview();
    if (fApp.f.input.getAttribute("aria-invalid")) validate();
  });
  [nameIn, profileIn, portIn].forEach((i) =>
    i.addEventListener("input", () => {
      if (i.getAttribute("aria-invalid")) validate();
    }),
  );

  api
    .listProfiles()
    .then((list) => {
      for (const name of list) datalist.appendChild(h("option", { value: name }));
    })
    .catch((e) => console.warn("list_profiles", e));

  function validate(): { ok: boolean; port: number | null } {
    let ok = true;
    const name = nameIn.value.trim();
    const nameLen = [...name].length;
    const nameErr = nameLen === 0 ? "表示名を入力してください" : nameLen > 64 ? "表示名は64文字以内で入力してください" : null;
    setError(fName.f, nameErr);
    if (nameErr) ok = false;

    const profileErr = profileIn.value.trim() ? null : "プロファイルを入力してください";
    setError(fProfile.f, profileErr);
    if (profileErr) ok = false;

    const app = appIn.value.trim();
    const appErr = !app
      ? "アプリ名を入力してください"
      : !APP_RE.test(app)
        ? "アプリ名は英小文字・数字・「-」のみ使えます"
        : null;
    setError(fApp.f, appErr);
    if (appErr) ok = false;

    let port: number | null = null;
    const raw = portIn.value.trim();
    let portErr: string | null = null;
    if (raw === "") {
      port = null; // 省略時は 13306
    } else {
      const n = Number(raw);
      if (!Number.isInteger(n) || n < 1024 || n > 65535) portErr = "ポートは1024〜65535の整数で入力してください";
      else port = n;
    }
    // 入力欄が数値として解釈できない文字を含むと value は "" になる
    if (raw === "" && portIn.validity.badInput) portErr = "ポートは1024〜65535の整数で入力してください";
    setError(fPort.f, portErr);
    if (portErr) ok = false;
    return { ok, port };
  }

  function focusFirstInvalid(): void {
    const bad = form.querySelector<HTMLInputElement>('[aria-invalid="true"]');
    bad?.focus();
  }

  let busy = false;
  const setBusy = (b: boolean) => {
    busy = b;
    saveBtn.disabled = b;
    cancelBtn.disabled = b;
    if (deleteBtn instanceof HTMLButtonElement) deleteBtn.disabled = b;
  };

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (busy) return;
    formError.hidden = true;
    const v = validate();
    if (!v.ok) {
      focusFirstInvalid();
      return;
    }
    setBusy(true);
    try {
      const res = await api.saveTarget({
        id: target?.id ?? null,
        name: nameIn.value.trim(),
        profile: profileIn.value.trim(),
        env,
        app: appIn.value.trim(),
        localPort: v.port,
      });
      if (res.warnings.length) toast(res.warnings, "warning");
      else toast(isEdit ? "接続先を保存しました" : "接続先を追加しました", "success");
      dialog.close();
      await refreshTargets().catch((e) => toast(errMsg(e), "error"));
      select(res.target.id);
    } catch (e) {
      formError.textContent = errMsg(e);
      formError.hidden = false;
    } finally {
      setBusy(false);
    }
  });

  cancelBtn.addEventListener("click", () => dialog.close());
  dialog.addEventListener("cancel", (ev) => {
    if (busy) ev.preventDefault();
  });

  if (deleteBtn instanceof HTMLButtonElement) {
    deleteBtn.addEventListener("click", async () => {
      const yes = await ask(`「${target!.name}（${target!.env}）」を削除しますか？\nこの操作は元に戻せません。`, {
        title: "DB Tunnel",
        kind: "warning",
        okLabel: "削除",
        cancelLabel: "キャンセル",
      });
      if (!yes) return;
      setBusy(true);
      try {
        await api.deleteTarget(target!.id);
        toast("接続先を削除しました", "success");
        dialog.close();
        if (store.selectedId === target!.id) {
          const rest = store.targets.filter((t) => t.id !== target!.id);
          select(rest[0]?.id ?? null);
        }
        await refreshTargets().catch((e) => toast(errMsg(e), "error"));
      } catch (e) {
        formError.textContent = errMsg(e);
        formError.hidden = false;
      } finally {
        setBusy(false);
      }
    });
  }

  dialog.addEventListener("close", unsub);
  openModal(dialog);
  nameIn.focus();
}
