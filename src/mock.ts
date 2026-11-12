// ブラウザ確認用のモックバックエンド（npm run dev を Tauri の外で開いたときだけ読み込む）。
// URLパラメータ: ?missing=aws|plugin|both  前提ツールが無い状態 / ?loadError=1  設定読み込みエラー / ?empty=1  接続先なし
//               ?importBad=1  JSONインポートのファイル選択で壊れたファイルを返す

import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  AppSettings,
  ImportPreview,
  ImportRow,
  LogLine,
  MoveDirection,
  Prerequisites,
  Target,
  TargetInput,
  TargetRuntime,
} from "./api";

const params = new URLSearchParams(location.search);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const now = () => {
  const d = new Date();
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
};

let targets: Target[] = params.has("empty")
  ? []
  : [
      { id: "t-dev", name: "Acme Portal", profile: "acme-portal", env: "dev", app: "acme-portal", localPort: 13306 },
      { id: "t-stg", name: "Acme Portal", profile: "acme-portal", env: "stg", app: "acme-portal", localPort: 13306 },
      { id: "t-prod", name: "Acme Portal", profile: "acme-portal", env: "prod", app: "acme-portal", localPort: 13308 },
      { id: "t-bill", name: "Billing API", profile: "billing", env: "dev", app: "billing-api", localPort: 13307 },
    ];

const runtimes = new Map<string, TargetRuntime>();
const logs = new Map<string, LogLine[]>();
const settings: AppSettings = {
  lastSelectedId: "t-dev",
  windowWidth: null,
  windowHeight: null,
  awsCliPath: null,
  pluginPath: null,
};

function idle(id: string): TargetRuntime {
  return { id, status: "idle", message: null, connectedAt: null, bastionId: null, rdsEndpoint: null, dbUser: null, hasPassword: false };
}
function rt(id: string): TargetRuntime {
  return runtimes.get(id) ?? idle(id);
}
function setRt(next: TargetRuntime): void {
  runtimes.set(next.id, next);
  void emit("target-status", next);
}
function log(id: string, source: "app" | "ssm", text: string, stream: "stdout" | "stderr" = "stdout", time = now()): void {
  const line: LogLine = { targetId: id, time, source, stream, text };
  const arr = logs.get(id) ?? [];
  arr.push(line);
  if (arr.length > 5000) arr.shift();
  logs.set(id, arr);
  void emit("log-line", line);
}
function rdsOf(t: Target): string {
  return `${t.app}-${t.env}-mysql.xxxx.ap-northeast-1.rds.amazonaws.com`;
}
function label(t: Target): string {
  return `${t.name} (${t.env})`;
}

// ---- 初期状態（モックアップと同じ） ----
if (!params.has("empty")) {
  const seed: [string, "app" | "ssm", string, ("stdout" | "stderr")?][] = [
    ["20:41:02", "app", "プロファイル acme-portal で aws login を実行します"],
    ["20:41:02", "app", "ブラウザでアカウントを選択してください…"],
    ["20:41:15", "app", "ログイン: arn:aws:sts::123456789012:assumed-role/Developer/taro"],
    ["20:41:16", "app", "踏み台を検索しました: i-0123456789abcdef0"],
    ["20:41:17", "app", "RDSを検索しました: acme-portal-dev-mysql.xxxx.ap-northeast-1.rds.amazonaws.com"],
    ["20:41:18", "app", "シークレットを取得しました（パスワードはログに出力しません）"],
    ["20:41:18", "app", "ポートフォワーディングを開始します（localhost:13306 → RDS:3306）"],
    ["20:41:19", "ssm", "Starting session with SessionId: taro-0abc123def456"],
    ["20:41:19", "ssm", "Port 13306 opened for sessionId taro-0abc123def456."],
    ["20:41:19", "ssm", "Waiting for connections..."],
    ["20:43:52", "ssm", "Connection accepted for session [taro-0abc123def456]"],
  ];
  logs.set(
    "t-dev",
    seed.map(([time, source, text, stream]) => ({ targetId: "t-dev", time, source, stream: stream ?? "stdout", text })),
  );
  runtimes.set("t-dev", {
    id: "t-dev",
    status: "connected",
    message: null,
    connectedAt: Date.now() - (12 * 60 + 34) * 1000,
    bastionId: "i-0123456789abcdef0",
    rdsEndpoint: "acme-portal-dev-mysql.xxxx.ap-northeast-1.rds.amazonaws.com",
    dbUser: "admin",
    hasPassword: true,
  });
  logs.set("t-bill", [
    { targetId: "t-bill", time: now(), source: "app", stream: "stdout", text: "プロファイル billing で aws login を実行します" },
    { targetId: "t-bill", time: now(), source: "ssm", stream: "stderr", text: "Attempting to automatically open the SSO authorization page in your default browser." },
    { targetId: "t-bill", time: now(), source: "app", stream: "stdout", text: "ブラウザでアカウントを選択してください…" },
  ]);
  runtimes.set("t-bill", { ...idle("t-bill"), status: "loggingIn", message: "ブラウザでアカウントを選択してください" });
  // しばらくするとログインがタイムアウトしてエラーになる
  setTimeout(() => {
    if (rt("t-bill").status !== "loggingIn") return;
    log("t-bill", "app", "ログインがタイムアウトしました（120秒）", "stderr");
    setRt({ ...idle("t-bill"), status: "error", message: "ログインがタイムアウトしました。もう一度接続してください。" });
  }, 20000);
}

// ---- 接続のシミュレーション ----
const cancelled = new Set<string>();

async function simulateConnect(t: Target): Promise<void> {
  const id = t.id;
  cancelled.delete(id);
  const step = async (ms: number) => {
    await sleep(ms);
    if (cancelled.has(id)) throw new Error("cancelled");
  };
  try {
    setRt({ ...idle(id), status: "loggingIn", message: "ブラウザでアカウントを選択してください" });
    log(id, "app", `プロファイル ${t.profile} で aws login を実行します`);
    log(id, "app", "ブラウザでアカウントを選択してください…");
    await step(1500);
    log(id, "app", "ログイン: arn:aws:sts::123456789012:assumed-role/Developer/taro");
    setRt({ ...idle(id), status: "preparing", message: "踏み台とRDSを検索しています" });
    await step(500);
    log(id, "app", "踏み台を検索しました: i-0fedcba9876543210");
    await step(400);
    log(id, "app", `RDSを検索しました: ${rdsOf(t)}`);
    await step(400);
    log(id, "app", "シークレットを取得しました（パスワードはログに出力しません）");
    log(id, "app", `ポートフォワーディングを開始します（localhost:${t.localPort} → RDS:3306）`);
    await step(500);
    log(id, "ssm", "Starting session with SessionId: taro-0f1e2d3c4b5a");
    log(id, "ssm", `Port ${t.localPort} opened for sessionId taro-0f1e2d3c4b5a.`);
    log(id, "ssm", "Waiting for connections...");
    setRt({
      id,
      status: "connected",
      message: null,
      connectedAt: Date.now(),
      bastionId: "i-0fedcba9876543210",
      rdsEndpoint: rdsOf(t),
      dbUser: "admin",
      hasPassword: true,
    });
  } catch {
    /* 停止された */
  }
}

async function simulateDisconnect(id: string): Promise<void> {
  const cur = rt(id);
  if (cur.status === "idle") return;
  cancelled.add(id);
  setRt({ ...cur, status: "stopping", message: null });
  log(id, "app", "セッションを停止しています…");
  await sleep(700);
  log(id, "app", "セッションを停止しました");
  setRt(idle(id));
}

// ---- 前提ツール ----
function prereq(): Prerequisites {
  const missing = params.get("missing") ?? "";
  const awsMissing = (missing === "aws" || missing === "both") && !settings.awsCliPath;
  const pluginMissing = (missing === "plugin" || missing === "both") && !settings.pluginPath;
  const awsCli = awsMissing
    ? { found: false, path: null, version: null, error: "PATH 上に aws が見つかりません" }
    : { found: true, path: settings.awsCliPath ?? "/usr/local/bin/aws", version: "2.31.5", error: null };
  const sessionManagerPlugin = pluginMissing
    ? { found: false, path: null, version: null, error: "PATH 上に session-manager-plugin が見つかりません" }
    : {
        found: true,
        path: settings.pluginPath ?? "/usr/local/bin/session-manager-plugin",
        version: "1.2.694.0",
        error: null,
      };
  return { awsCli, sessionManagerPlugin, ok: awsCli.found && sessionManagerPlugin.found };
}

// ---- 入力チェック（バックエンドと同じ規則） ----
function validate(input: TargetInput): string | null {
  const name = input.name.trim();
  if ([...name].length < 1 || [...name].length > 64) return "表示名は1〜64文字で入力してください";
  if (!input.profile.trim()) return "プロファイルを入力してください";
  if (!/^[a-z0-9-]+$/.test(input.app)) return "アプリ名は英小文字・数字・「-」のみ使えます";
  const port = input.localPort ?? 13306;
  if (!Number.isInteger(port) || port < 1024 || port > 65535) return "ポートは1024〜65535で指定してください";
  return null;
}

function saveTarget(input: TargetInput, insertAfter: string | null) {
  const err = validate(input);
  if (err) throw err;
  const app = input.app.trim();
  const profile = input.profile.trim();
  const dup = targets.find((t) => t.id !== input.id && t.profile === profile && t.env === input.env && t.app === app);
  if (dup) throw `同じプロファイル・環境・アプリ名の接続先がすでにあります（「${dup.name}」）`;
  const port = input.localPort ?? 13306;
  if (input.id) {
    const cur = targets.find((t) => t.id === input.id);
    if (!cur) throw "接続先が見つかりません";
    const st = rt(cur.id).status;
    if (st !== "idle" && st !== "error" && (cur.env !== input.env || cur.localPort !== port)) {
      throw "接続中は環境とポートを変更できません";
    }
  }
  const target: Target = {
    id: input.id ?? `t-${Math.random().toString(36).slice(2, 9)}`,
    name: input.name.trim(),
    profile: input.profile.trim(),
    env: input.env,
    app,
    localPort: port,
  };
  if (input.id) {
    targets = targets.map((t) => (t.id === input.id ? target : t));
  } else {
    // 新規は末尾、複製は複製元のすぐ下
    const at = insertAfter ? targets.findIndex((t) => t.id === insertAfter) + 1 : targets.length;
    targets = [...targets.slice(0, at), target, ...targets.slice(at)];
  }
  const warnings = targets
    .filter((t) => t.id !== target.id && t.localPort === port)
    .map((t) => `ポート ${port} はほかの接続先（${label(t)}）と重複しています。同時には接続できません。`);
  return { target, warnings };
}

let lastPreview: ImportPreview | null = null;

const handlers: Record<string, (a: any) => unknown> = {
  list_targets: () => targets.map((t) => ({ ...t })),
  list_profiles: () => ["acme-portal", "billing", "default", "sandbox"],
  list_runtimes: () => targets.map((t) => rt(t.id)),

  save_target: ({ input }: { input: TargetInput }) => saveTarget(input, null),

  move_target: ({ id, direction }: { id: string; direction: MoveDirection }) => {
    const i = targets.findIndex((t) => t.id === id);
    if (i < 0) throw `接続先が見つかりません（ID: ${id}）`;
    const j = direction === "up" ? i - 1 : i + 1;
    if (j >= 0 && j < targets.length) {
      const next = [...targets];
      [next[i], next[j]] = [next[j], next[i]];
      targets = next;
    }
    return targets.map((t) => ({ ...t }));
  },

  duplicate_target: ({ sourceId, input }: { sourceId: string; input: TargetInput }) => {
    if (!targets.some((t) => t.id === sourceId)) throw `接続先が見つかりません（ID: ${sourceId}）`;
    return saveTarget({ ...input, id: null }, sourceId);
  },

  delete_target: ({ id }: { id: string }) => {
    const st = rt(id).status;
    if (st !== "idle" && st !== "error") throw "接続中の接続先は削除できません。停止してから削除してください。";
    targets = targets.filter((t) => t.id !== id);
    runtimes.delete(id);
    logs.delete(id);
  },

  connect: ({ id, confirmedProd }: { id: string; confirmedProd: boolean }) => {
    const t = targets.find((x) => x.id === id);
    if (!t) throw "接続先が見つかりません";
    if (!prereq().ok) throw "AWS CLI または Session Manager Plugin が見つかりません";
    if (t.env === "prod" && !confirmedProd) throw "prod への接続には確認が必要です";
    const st = rt(id).status;
    if (st !== "idle" && st !== "error") throw "すでに接続処理中です";
    const busy = targets.find((x) => x.id !== id && x.localPort === t.localPort && rt(x.id).status !== "idle" && rt(x.id).status !== "error");
    if (busy) throw `ポート ${t.localPort} は「${label(busy)}」が使用中です。停止するか、ポートを変更してください。`;
    void simulateConnect(t);
  },
  disconnect: ({ id }: { id: string }) => {
    void simulateDisconnect(id);
  },
  reveal_password: ({ id }: { id: string }) => {
    if (!rt(id).hasPassword) throw "パスワードはまだ取得していません";
    return "s3cr3t-P@ssw0rd!";
  },
  copy_password: ({ id }: { id: string }) => {
    if (!rt(id).hasPassword) throw "パスワードはまだ取得していません";
    console.info("[mock] copy_password", id);
  },
  copy_text: ({ text }: { text: string }) => {
    console.info("[mock] copy_text", text);
    navigator.clipboard?.writeText(text).catch(() => undefined);
  },

  get_logs: async ({ id }: { id: string }) => {
    await sleep(80);
    return [...(logs.get(id) ?? [])];
  },
  clear_logs: ({ id }: { id: string }) => {
    logs.set(id, []);
  },
  save_logs: ({ id, path }: { id: string; path: string }) => console.info("[mock] save_logs", id, path),

  check_prerequisites: async () => {
    await sleep(300);
    return prereq();
  },
  set_tool_paths: ({ awsCliPath, pluginPath }: { awsCliPath: string | null; pluginPath: string | null }) => {
    settings.awsCliPath = awsCliPath;
    settings.pluginPath = pluginPath;
    return prereq();
  },
  get_settings: () => ({ ...settings }),
  set_last_selected: ({ id }: { id: string | null }) => {
    settings.lastSelectedId = id;
  },
  get_config_info: () => ({
    path: "~/Library/Application Support/db-tunnel/config.enc",
    encryption: "AES-256-GCM",
    keychainOk: true,
    loadError: params.has("loadError")
      ? "config.enc を復号できませんでした。バックアップ（config.enc.bak）から読み込みました。"
      : null,
  }),
  open_config_dir: () => console.info("[mock] open_config_dir"),

  import_preview: ({ path }: { path: string }) => {
    if (path.includes("bad")) throw "JSONの形式が正しくありません（version 2 には対応していません）";
    const rows: ImportRow[] = [
      { index: 0, name: "Acme Portal", profile: "acme-portal", env: "dev", app: "acme-portal", localPort: 13306, judgement: "overwrite", errors: [], warnings: [] },
      { index: 1, name: "Acme Portal", profile: "acme-portal", env: "stg", app: "acme-portal", localPort: 13306, judgement: "new", errors: [], warnings: ["ポート 13306 はほかの接続先と重複しています"] },
      { index: 2, name: "Acme Portal", profile: "acme-portal", env: "prod", app: "acme-portal", localPort: 13308, judgement: "new", errors: [], warnings: [] },
      { index: 3, name: "Inventory", profile: "inventory", env: "stg", app: "inventory", localPort: null, judgement: "new", errors: [], warnings: [] },
      { index: 4, name: "[別システム名]", profile: "[profile]", env: "dev", app: "", localPort: 80, judgement: "error", errors: ["アプリ名が空", "ポートは1024〜65535"], warnings: [] },
    ];
    // 既存と比較して判定を付け直す
    for (const r of rows) {
      if (r.judgement === "error") continue;
      r.judgement = targets.some((t) => t.profile === r.profile && t.env === r.env && t.app === r.app) ? "overwrite" : "new";
    }
    lastPreview = { fileName: path.split(/[\\/]/).pop() ?? path, rows, warnings: ["未知のフィールド「comment」は無視します"] };
    return lastPreview;
  },
  import_apply: ({ mode, indices }: { mode: "overwrite" | "newOnly"; indices: number[] }) => {
    if (!lastPreview) throw "先にファイルを選択してください";
    let added = 0;
    let updated = 0;
    let skipped = 0;
    for (const i of indices) {
      const r = lastPreview.rows.find((x) => x.index === i);
      if (!r || r.judgement === "error") {
        skipped++;
        continue;
      }
      const existing = targets.find((t) => t.profile === r.profile && t.env === r.env && t.app === r.app);
      if (existing) {
        if (mode === "newOnly") {
          skipped++;
          continue;
        }
        Object.assign(existing, { name: r.name, localPort: r.localPort ?? 13306 });
        updated++;
      } else {
        targets.push({
          id: `t-${Math.random().toString(36).slice(2, 9)}`,
          name: r.name,
          profile: r.profile,
          env: r.env as Target["env"],
          app: r.app,
          localPort: r.localPort ?? 13306,
        });
        added++;
      }
    }
    lastPreview = null;
    return { added, updated, skipped };
  },
  export_targets: ({ path }: { path: string }) => {
    console.info("[mock] export_targets", path);
    return targets.length;
  },
  quit_app: () => console.info("[mock] quit_app"),

  // ---- プラグイン ----
  "plugin:dialog|open": () =>
    // 実行中に URL を書き換えても切り替わるよう毎回読む
    new URLSearchParams(location.search).has("importBad") ? "/Users/taro/Downloads/bad-targets.json" : "/Users/taro/Downloads/targets.json",
  "plugin:dialog|save": ({ options }: { options: { defaultPath?: string } }) =>
    `/Users/taro/Downloads/${options?.defaultPath ?? "untitled"}`,
  "plugin:dialog|message": ({ message, buttons }: { message: string; buttons: unknown }) => {
    const yes = window.confirm(message);
    if (buttons === "YesNo") return yes ? "Yes" : "No";
    const b = buttons as { OkCancelCustom?: [string, string] } | undefined;
    const [ok, cancel] = b?.OkCancelCustom ?? ["Ok", "Cancel"];
    return yes ? ok : cancel;
  },
  "plugin:opener|open_url": ({ url }: { url: string }) => console.info("[mock] open_url", url),
};

export function installMock(): void {
  mockIPC(
    async (cmd, payload) => {
      const fn = handlers[cmd];
      if (!fn) {
        console.warn("[mock] 未対応のコマンド", cmd, payload);
        return null;
      }
      await sleep(40);
      return fn(payload ?? {});
    },
    { shouldMockEvents: true },
  );
  // 終了確認の動作確認用: コンソールで __mockQuit() を呼ぶ
  (window as any).__mockQuit = () => {
    const n = [...runtimes.values()].filter((r) => r.status !== "idle" && r.status !== "error").length;
    void emit("quit-requested", n);
  };
  console.info("[mock] モックバックエンドで動作しています");
}
