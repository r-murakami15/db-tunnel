// Rust側（src-tauri/src/model.rs, commands.rs）との契約。
// コマンドはすべて失敗時に日本語のエラーメッセージ（string）で reject する。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Env = "dev" | "stg" | "prod";

export interface Target {
  id: string;
  name: string;
  profile: string;
  env: Env;
  app: string;
  localPort: number;
}

export interface TargetInput {
  id?: string | null;
  name: string;
  profile: string;
  env: Env;
  app: string;
  localPort?: number | null;
}

export type MoveDirection = "up" | "down";

export interface SaveResult {
  target: Target;
  warnings: string[];
}

export type Status = "idle" | "loggingIn" | "preparing" | "connected" | "stopping" | "error";

export interface TargetRuntime {
  id: string;
  status: Status;
  message: string | null;
  /** 接続中になった時刻（エポックミリ秒） */
  connectedAt: number | null;
  bastionId: string | null;
  rdsEndpoint: string | null;
  dbUser: string | null;
  hasPassword: boolean;
}

export interface LogLine {
  targetId: string;
  /** HH:MM:SS */
  time: string;
  source: "app" | "ssm";
  stream: "stdout" | "stderr";
  text: string;
}

export interface ToolStatus {
  found: boolean;
  path: string | null;
  version: string | null;
  error: string | null;
}

export interface Prerequisites {
  awsCli: ToolStatus;
  sessionManagerPlugin: ToolStatus;
  ok: boolean;
}

export interface AppSettings {
  lastSelectedId: string | null;
  windowWidth: number | null;
  windowHeight: number | null;
  awsCliPath: string | null;
  pluginPath: string | null;
}

export interface ConfigInfo {
  path: string;
  encryption: string;
  keychainOk: boolean;
  loadError: string | null;
}

export type ImportJudgement = "new" | "overwrite" | "error";

export interface ImportRow {
  index: number;
  name: string;
  profile: string;
  env: string;
  app: string;
  localPort: number | null;
  judgement: ImportJudgement;
  errors: string[];
  warnings: string[];
}

export interface ImportPreview {
  fileName: string;
  rows: ImportRow[];
  warnings: string[];
}

export type ImportMode = "overwrite" | "newOnly";

export interface ImportResult {
  added: number;
  updated: number;
  skipped: number;
}

export const api = {
  listTargets: () => invoke<Target[]>("list_targets"),
  saveTarget: (input: TargetInput) => invoke<SaveResult>("save_target", { input }),
  deleteTarget: (id: string) => invoke<void>("delete_target", { id }),
  /** 一覧の並び順で1つ上・下へ移動し、移動後の一覧を返す（端では何もしない） */
  moveTarget: (id: string, direction: MoveDirection) =>
    invoke<Target[]>("move_target", { id, direction }),
  /** 複製元のすぐ下に新しい接続先として追加する（input.id は無視される） */
  duplicateTarget: (sourceId: string, input: TargetInput) =>
    invoke<SaveResult>("duplicate_target", { sourceId, input }),
  listProfiles: () => invoke<string[]>("list_profiles"),

  /** 全接続先の実行時状態 */
  listRuntimes: () => invoke<TargetRuntime[]>("list_runtimes"),
  /** prod の場合は confirmedProd=true が必要（確認ダイアログの後に呼ぶ） */
  connect: (id: string, confirmedProd: boolean) => invoke<void>("connect", { id, confirmedProd }),
  disconnect: (id: string) => invoke<void>("disconnect", { id }),
  revealPassword: (id: string) => invoke<string>("reveal_password", { id }),
  /** パスワードをクリップボードへ。30秒後に同じ内容なら消去 */
  copyPassword: (id: string) => invoke<void>("copy_password", { id }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),

  getLogs: (id: string) => invoke<LogLine[]>("get_logs", { id }),
  clearLogs: (id: string) => invoke<void>("clear_logs", { id }),
  /** ログをテキストファイルに保存（path は保存ダイアログで選んだもの） */
  saveLogs: (id: string, path: string) => invoke<void>("save_logs", { id, path }),

  checkPrerequisites: () => invoke<Prerequisites>("check_prerequisites"),
  /** null で自動検出に戻す */
  setToolPaths: (awsCliPath: string | null, pluginPath: string | null) =>
    invoke<Prerequisites>("set_tool_paths", { awsCliPath, pluginPath }),

  getSettings: () => invoke<AppSettings>("get_settings"),
  setLastSelected: (id: string | null) => invoke<void>("set_last_selected", { id }),
  getConfigInfo: () => invoke<ConfigInfo>("get_config_info"),
  openConfigDir: () => invoke<void>("open_config_dir"),

  importPreview: (path: string) => invoke<ImportPreview>("import_preview", { path }),
  /** 直前の import_preview の結果から indices の行を取り込む */
  importApply: (mode: ImportMode, indices: number[]) =>
    invoke<ImportResult>("import_apply", { mode, indices }),
  exportTargets: (path: string) => invoke<number>("export_targets", { path }),

  /** 接続中のセッションをすべて終了してアプリを閉じる */
  quitApp: () => invoke<void>("quit_app"),
};

export const events = {
  onTargetStatus: (cb: (rt: TargetRuntime) => void): Promise<UnlistenFn> =>
    listen<TargetRuntime>("target-status", (e) => cb(e.payload)),
  onLogLine: (cb: (line: LogLine) => void): Promise<UnlistenFn> =>
    listen<LogLine>("log-line", (e) => cb(e.payload)),
  /** ウィンドウを閉じようとしたとき、接続中があれば発火（payload は接続中の件数）。UIで確認後 quitApp を呼ぶ */
  onQuitRequested: (cb: (activeCount: number) => void): Promise<UnlistenFn> =>
    listen<number>("quit-requested", (e) => cb(e.payload)),
};
