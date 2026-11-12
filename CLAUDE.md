# DB Tunnel — 開発メモ

- 設計書は docs/（README.md が目次）。仕様の判断は docs を正とする。
- ホストにRustを入れない。cargo / npm はすべて `docker compose run --rm dev bash -lc '…'` で実行する（docs/09-development.md）。
- Mac用アプリは `scripts/build-mac.sh`（コンテナ内で cargo-zigbuild によるクロスビルド → ホストで .app 組み立てとアドホック署名）。
- 画面とRustの契約は `src/api.ts` と `src-tauri/src/model.rs`。片方を変えたら必ずもう片方も合わせる。
- UIは日本語のみ。パスワードやシークレットはログ・画面に平文で出さない。
- ブランチは main のみ。
