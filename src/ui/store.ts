// アプリ全体の状態と、変更通知の小さな仕組み。

import {
  api,
  type AppSettings,
  type ConfigInfo,
  type Prerequisites,
  type Status,
  type Target,
  type TargetRuntime,
} from "../api";
import { ENV_ORDER } from "./dom";

export type Topic = "targets" | "runtime" | "selection" | "prereq" | "view" | "config";

export const store = {
  targets: [] as Target[],
  runtimes: new Map<string, TargetRuntime>(),
  selectedId: null as string | null,
  search: "",
  prereq: null as Prerequisites | null,
  configInfo: null as ConfigInfo | null,
  settings: null as AppSettings | null,
  view: "main" as "main" | "settings",
  /** 接続コマンドを送って応答待ちの接続先 */
  connecting: new Set<string>(),
};

const subs = new Map<Topic, Set<(arg?: unknown) => void>>();

export function on(topic: Topic, fn: (arg?: unknown) => void): () => void {
  let set = subs.get(topic);
  if (!set) subs.set(topic, (set = new Set()));
  set.add(fn);
  return () => set!.delete(fn);
}

export function emit(topic: Topic, arg?: unknown): void {
  subs.get(topic)?.forEach((fn) => {
    try {
      fn(arg);
    } catch (e) {
      console.error(e);
    }
  });
}

export function idleRuntime(id: string): TargetRuntime {
  return {
    id,
    status: "idle",
    message: null,
    connectedAt: null,
    bastionId: null,
    rdsEndpoint: null,
    dbUser: null,
    hasPassword: false,
  };
}

export function runtimeOf(id: string): TargetRuntime {
  return store.runtimes.get(id) ?? idleRuntime(id);
}

/** 接続処理中または接続中（環境・ポートの変更不可） */
export function isActive(status: Status): boolean {
  return status !== "idle" && status !== "error";
}

export function selectedTarget(): Target | undefined {
  return store.targets.find((t) => t.id === store.selectedId);
}

export function sortTargets(list: Target[]): Target[] {
  return [...list].sort(
    (a, b) =>
      a.profile.localeCompare(b.profile) ||
      a.name.localeCompare(b.name, "ja") ||
      ENV_ORDER[a.env] - ENV_ORDER[b.env],
  );
}

export function setTargets(list: Target[]): void {
  store.targets = sortTargets(list);
  emit("targets");
  if (store.selectedId && !store.targets.some((t) => t.id === store.selectedId)) {
    select(store.targets[0]?.id ?? null);
  }
}

export async function refreshTargets(): Promise<void> {
  setTargets(await api.listTargets());
}

export function select(id: string | null): void {
  if (store.selectedId === id) return;
  store.selectedId = id;
  emit("selection");
  api.setLastSelected(id).catch((e) => console.warn("set_last_selected", e));
}

export function setRuntime(rt: TargetRuntime): void {
  store.runtimes.set(rt.id, rt);
  if (rt.status !== "idle") store.connecting.delete(rt.id);
  emit("runtime", rt);
}

export function setPrereq(p: Prerequisites): void {
  store.prereq = p;
  emit("prereq");
}

export function showView(view: "main" | "settings"): void {
  if (store.view === view) return;
  store.view = view;
  emit("view");
}
