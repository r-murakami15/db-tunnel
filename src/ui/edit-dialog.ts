// ③ 接続先の追加・編集ダイアログ / ⑧ 複製ダイアログ（同じ部品の3つのモード）

import { ask } from "@tauri-apps/plugin-dialog";
import { api, type Env, type Target, type TargetInput } from "../api";
import { errMsg, h, icon, replaceChildren } from "./dom";
import { openModal } from "./prod-dialog";
import { isActive, on, refreshTargets, runtimeOf, select, store } from "./store";
import { toast } from "./toast";

const DEFAULT_PORT = 13306;
const NAME_MAX = 64;
const COPY_SUFFIX = " のコピー";
const APP_RE = /^[a-z0-9-]+$/;
const ENVS: Env[] = ["dev", "stg", "prod"];
const PORT_HELP = "初期値 13306。1024〜65535、ほかの接続先と同時に使う場合は重複不可";
const PORT_HELP_SAME = "複製元と同じポートです。同時に接続する場合は変更してください";

let seq = 0;

/** フォームの値（ポートは入力欄の文字列のまま） */
export interface FormValues {
  name: string;
  profile: string;
  env: Env;
  app: string;
  port: string;
}

type Mode =
  | { kind: "add" }
  | { kind: "edit"; target: Target }
  | { kind: "duplicate"; source: Target; values: FormValues };

interface Field {
  input: HTMLInputElement;
  error: HTMLSpanElement;
}

function field(label: string, input: HTMLInputElement, extra: Node[] = [], help?: string): { el: HTMLDivElement; f: Field; help: HTMLSpanElement | null } {
  const id = input.id;
  const error = h("span", { class: "field-error", id: `${id}-err`, hidden: true });
  const helpEl = help ? h("span", { class: "help", id: `${id}-help` }, help) : null;
  input.setAttribute("aria-describedby", [helpEl ? `${id}-help` : "", `${id}-err`].filter(Boolean).join(" "));
  const el = h("div", { class: "field" }, h("label", { for: id }, label), input, error, ...extra, helpEl);
  return { el, f: { input, error }, help: helpEl };
}

function setError(f: Field, msg: string | null): void {
  f.error.textContent = msg ?? "";
  f.error.hidden = !msg;
  if (msg) f.input.setAttribute("aria-invalid", "true");
  else f.input.removeAttribute("aria-invalid");
}

function valuesOf(t: Target): FormValues {
  return { name: t.name, profile: t.profile, env: t.env, app: t.app, port: String(t.localPort) };
}

/** 「<name> のコピー」。64文字に収まるよう元の名前を切り詰める */
function copyName(name: string): string {
  const base = [...name.trim()];
  const max = NAME_MAX - [...COPY_SUFFIX].length;
  return (base.length > max ? base.slice(0, max).join("").trimEnd() : base.join("")) + COPY_SUFFIX;
}

/** 追加（target なし）または編集 */
export function openEditDialog(target?: Target): void {
  openTargetDialog(target ? { kind: "edit", target } : { kind: "add" });
}

/** 複製。values を省略すると複製元の保存済みの値を使う */
export function openDuplicateDialog(source: Target, values?: FormValues): void {
  openTargetDialog({ kind: "duplicate", source, values: values ?? valuesOf(source) });
}

function openTargetDialog(mode: Mode): void {
  const target = mode.kind === "edit" ? mode.target : undefined;
  const isEdit = mode.kind === "edit";
  const isDup = mode.kind === "duplicate";
  const p = `ed${++seq}`;

  const init: FormValues =
    mode.kind === "edit"
      ? valuesOf(mode.target)
      : mode.kind === "duplicate"
        ? { ...mode.values, name: copyName(mode.values.name.trim() || mode.source.name) }
        : { name: "", profile: "", env: "dev", app: "", port: String(DEFAULT_PORT) };

  const nameIn = h("input", {
    id: `${p}-name`,
    class: "input",
    type: "text",
    maxlength: NAME_MAX,
    autocomplete: "off",
    value: init.name,
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
    value: init.profile,
    placeholder: "プロファイル名",
  });
  const appIn = h("input", {
    id: `${p}-app`,
    class: "input mono",
    type: "text",
    autocomplete: "off",
    spellcheck: "false",
    value: init.app,
    placeholder: "例: acme-portal",
  });
  const portIn = h("input", {
    id: `${p}-port`,
    class: "input mono input-port",
    type: "number",
    min: 1024,
    max: 65535,
    step: 1,
    value: init.port,
  });

  // 環境（セグメント型3択、実体はラジオボタン）
  let env: Env = init.env;
  const envRadios: HTMLInputElement[] = [];
  const envSegs = ENVS.map((e) => {
    const r = h("input", {
      type: "radio",
      name: `${p}-env`,
      value: e,
      checked: e === env,
      "aria-describedby": `${p}-env-err`,
    });
    envRadios.push(r);
    r.addEventListener("change", () => {
      if (r.checked) {
        env = e;
        paintEnv();
        updatePreview();
        checkDuplicate();
      }
    });
    return h("label", { class: `seg seg-${e}` }, r, e);
  });
  const paintEnv = () => envSegs.forEach((s, i) => s.classList.toggle("seg-on", ENVS[i] === env));
  paintEnv();
  const segmented = h("div", { class: "segmented" }, envSegs);
  // 同じ組み合わせ（プロファイル・環境・アプリ名）の警告。入力のたびに更新するので読み上げる
  const dupError = h("span", { class: "dup-error", id: `${p}-env-err`, "aria-live": "polite" });
  const envSet = h("fieldset", { class: "field fieldset" }, h("legend", null, "環境"), segmented, dupError);

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
  const fPort = field("ローカルポート", portIn, [], PORT_HELP);

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

  const title = isDup ? "接続先を複製" : isEdit ? "接続先を編集" : "接続先を追加";
  const subtitle =
    mode.kind === "duplicate"
      ? h(
          "span",
          { class: "help", id: `${p}-sub` },
          `「${mode.source.name}（${mode.source.env}）」の内容をコピーして新しい接続先を作ります`,
        )
      : null;

  const cancelBtn = h("button", { type: "button", class: "btn btn-lg" }, "キャンセル");
  const saveBtn = h(
    "button",
    { type: "submit", class: "btn btn-lg btn-primary", form: `${p}-form` },
    isDup ? "追加" : "保存",
  );
  const deleteBtn = isEdit ? h("button", { type: "button", class: "btn btn-lg btn-danger-outline" }, "削除") : null;
  const dupBtn = isEdit
    ? h("button", { type: "button", class: "btn btn-lg btn-primary-outline" }, icon("copy"), "複製")
    : null;

  const dialog = h(
    "dialog",
    {
      class: "modal modal-edit",
      "aria-labelledby": `${p}-title`,
      "aria-describedby": subtitle ? `${p}-sub` : null,
    },
    h("div", { class: "dialog-head dialog-head-stack" }, h("h1", { id: `${p}-title` }, title), subtitle),
    form,
    h(
      "div",
      { class: "dialog-foot" },
      h("div", { class: "dialog-actions" }, deleteBtn, dupBtn),
      h("div", { class: "dialog-actions" }, cancelBtn, saveBtn),
    ),
  );

  // 接続中は環境・ポートを変更不可（編集のみ。複製は新しい接続先なので変更できる）
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
      checkDuplicate();
    }
  };

  // 複製時、ポートが複製元と同じなら注意を出す
  const updatePortHelp = () => {
    if (mode.kind !== "duplicate" || !fPort.help) return;
    const raw = portIn.value.trim();
    const port = raw === "" ? DEFAULT_PORT : Number(raw);
    fPort.help.textContent = port === mode.source.localPort ? PORT_HELP_SAME : PORT_HELP;
  };

  let busy = false;
  let duplicateOf: Target | undefined;

  /** 同じ組み合わせの接続先が既にあれば、環境の下にエラーを出して保存ボタンを無効にする */
  function checkDuplicate(): void {
    const profile = profileIn.value.trim();
    const app = appIn.value.trim();
    duplicateOf =
      profile && app
        ? store.targets.find((t) => t.id !== target?.id && t.profile === profile && t.env === env && t.app === app)
        : undefined;
    segmented.classList.toggle("is-invalid", !!duplicateOf);
    if (duplicateOf) {
      replaceChildren(
        dupError,
        icon("alertCircle", 14),
        h(
          "span",
          null,
          `同じ組み合わせ（${profile} / ${env} / ${app}）が既にあります。プロファイル・環境・アプリ名のどれかを変更してください`,
        ),
      );
    } else {
      dupError.textContent = "";
    }
    saveBtn.disabled = busy || !!duplicateOf;
  }

  applyActive();
  updatePortHelp();
  checkDuplicate();
  const unsubs = [
    on("runtime", (rt) => {
      if (isEdit && (rt as { id: string }).id === target!.id) applyActive();
    }),
    on("targets", checkDuplicate),
  ];

  appIn.addEventListener("input", () => {
    updatePreview();
    checkDuplicate();
    if (fApp.f.input.getAttribute("aria-invalid")) validate();
  });
  profileIn.addEventListener("input", checkDuplicate);
  portIn.addEventListener("input", updatePortHelp);
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
    const nameErr =
      nameLen === 0 ? "表示名を入力してください" : nameLen > NAME_MAX ? "表示名は64文字以内で入力してください" : null;
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

  const setBusy = (b: boolean) => {
    busy = b;
    saveBtn.disabled = b || !!duplicateOf;
    cancelBtn.disabled = b;
    if (deleteBtn) deleteBtn.disabled = b;
    if (dupBtn) dupBtn.disabled = b;
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
    checkDuplicate();
    if (duplicateOf) {
      profileIn.focus();
      return;
    }
    setBusy(true);
    try {
      const input: TargetInput = {
        id: target?.id ?? null,
        name: nameIn.value.trim(),
        profile: profileIn.value.trim(),
        env,
        app: appIn.value.trim(),
        localPort: v.port,
      };
      const res =
        mode.kind === "duplicate"
          ? await api.duplicateTarget(mode.source.id, { ...input, id: null })
          : await api.saveTarget(input);
      if (res.warnings.length) toast(res.warnings, "warning");
      else toast(isDup ? "接続先を複製しました" : isEdit ? "接続先を保存しました" : "接続先を追加しました", "success");
      setBusy(false);
      dialog.close();
      await refreshTargets().catch((e) => toast(errMsg(e), "error"));
      select(res.target.id);
      const active = document.activeElement;
      if (!active || active === document.body || active.classList.contains("target-row")) {
        document.querySelector<HTMLElement>(`.target-row[data-id="${CSS.escape(res.target.id)}"]`)?.focus();
      }
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

  // 編集中の内容（未保存の変更も含む）を引き継いで複製ダイアログへ切り替える
  dupBtn?.addEventListener("click", () => {
    if (busy || !target) return;
    const values: FormValues = {
      name: nameIn.value,
      profile: profileIn.value.trim(),
      env,
      app: appIn.value.trim(),
      port: portIn.value,
    };
    dialog.close();
    openDuplicateDialog(target, values);
  });

  deleteBtn?.addEventListener("click", async () => {
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
      setBusy(false);
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

  dialog.addEventListener("close", () => unsubs.forEach((u) => u()));
  openModal(dialog);
  nameIn.focus();
  if (isDup) nameIn.select();
}
