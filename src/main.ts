async function boot(): Promise<void> {
  // Tauri の外（ブラウザで npm run dev）ではモックのバックエンドを使う。
  // 本番ビルドでは import.meta.env.DEV が false になり、モックはバンドルに含まれない。
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installMock } = await import("./mock");
    installMock();
  }
  const { start } = await import("./app");
  await start(document.getElementById("app")!);
}

boot().catch((e) => {
  console.error(e);
  const el = document.getElementById("app");
  if (el) el.textContent = `起動に失敗しました: ${String(e)}`;
});
