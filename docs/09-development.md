# 開発環境

[目次に戻る](README.md)

ホストマシンにはRustを入れず、Dockerコンテナ（`docker/Dockerfile`、`compose.yaml` の `dev` サービス）で開発する。コンテナには Rust・Node.js・Zig・cargo-zigbuild が入っており、Linux上でのテストと、macOS向けのクロスビルドの両方を行える。

## 必要なもの（ホスト）

| ツール | 用途 |
| --- | --- |
| Docker Desktop | 開発コンテナの実行 |
| Xcode Command Line Tools（`xcode-select --install`） | macOS SDK（コンテナへコピーして使う）、`codesign`、`lipo`、`hdiutil` |
| AWS CLI v2・Session Manager Plugin | アプリの動作確認用（ビルドには不要） |

## コンテナの構成

| 項目 | 内容 |
| --- | --- |
| イメージ | `db-tunnel-dev:latest`（`rust:1.90-bookworm` + Tauri のLinux依存 + Node.js 20 + Zig 0.13 + cargo-zigbuild） |
| ソース | リポジトリを `/work` にマウント |
| 名前付きボリューム | `cargo-target`（ビルド成果物、`CARGO_TARGET_DIR=/cargo-target`）、`cargo-registry` / `cargo-git`（依存キャッシュ）、`node-modules`（Linux用の node_modules）、`macos-sdk`（macOS SDK） |

ビルド成果物と `node_modules` をホストと分けているのは、Mac上のバインドマウントが遅いことと、ネイティブバイナリ（`@tauri-apps/cli` など）がホストとコンテナで異なるため。

## よく使うコマンド

```bash
# イメージのビルド（初回のみ・Dockerfile変更時）
docker compose build dev

# フロントエンドの依存インストールと型チェック
docker compose run --rm dev bash -lc 'npm install && npm run typecheck'

# Rust のフォーマット・Lint・テスト（PRで実行するものと同じ）
docker compose run --rm dev bash -lc 'cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test'

# 画面だけをブラウザで確認（Rustなし。モックのバックエンドで動く）
docker compose run --rm -p 1420:1420 dev bash -lc 'npm install && npx vite --host 0.0.0.0'
# → http://localhost:1420
```

## Mac用アプリのビルド

```bash
scripts/build-mac.sh            # このMacのCPU向け → dist-mac/DB Tunnel.app
scripts/build-mac.sh --dmg      # .dmg も作る
scripts/build-mac.sh --universal --dmg   # Apple Silicon + Intel
```

スクリプトの流れは次のとおり。

1. 開発イメージを用意する。
2. ホストの Command Line Tools から macOS SDK を `macos-sdk` ボリュームへコピーする（初回のみ。Dockerのファイル共有設定は不要で、`tar` をパイプで流し込む）。
3. `scripts/patch-sdk-tbd.sh` で SDK の `.tbd` を補正する。macOS 26 以降の SDK は一部の `.tbd` に `arm64e-macos` しか書かれておらず、Zig のリンカが arm64 向けのシンボルを解決できないため `arm64-macos` を併記する（コピーしたSDKだけを変更する）。
4. コンテナ内で `npm run build` → `cargo zigbuild --release --target <aarch64|x86_64>-apple-darwin --features tauri/custom-protocol`。
5. ホストで `.app` を組み立て（`Info.plist`・アイコン）、`codesign --sign -` でアドホック署名する。Apple Silicon では署名のないバイナリは起動できないため必須。配布用のコード署名ではない。
6. `--dmg` 指定時は `hdiutil` で `.dmg` を作る。

> `tauri build` のバンドラーは macOS 上でしか `.app` / `.dmg` を作れないため、ローカルではこのスクリプトで代替する。リリース用のビルドは GitHub Actions の macOS ランナーで `tauri-apps/tauri-action` を使う（[07-build-and-release.md](07-build-and-release.md)）。

### 起動

```bash
open "dist-mac/DB Tunnel.app"
```

ローカルでビルドしたアプリは隔離属性（quarantine）が付かないため、「開けません」のダイアログは出ない。初回起動時に「キーチェーンへのアクセスを許可しますか」と出たら「常に許可」を押す（[03-security.md](03-security.md) 4.4）。

## ディレクトリ構成

```
.
├── compose.yaml / docker/Dockerfile   開発コンテナ
├── scripts/                            Mac用ビルドスクリプト
├── index.html, src/                    画面（Vite + TypeScript）
│   ├── api.ts                          Rust側コマンド・イベントとの契約
│   └── mock.ts                         ブラウザ確認用のモックバックエンド
├── src-tauri/                          Rust（Tauri 2）
│   ├── src/model.rs                    画面とやり取りする型
│   ├── src/commands.rs                 Tauriコマンド
│   ├── src/store/                      設定ファイルの暗号化・キーチェーン
│   ├── src/import_export.rs            JSONのインポート・エクスポート
│   ├── src/connection.rs               接続管理（状態・接続処理）
│   ├── src/aws.rs                      AWS CLIの呼び出し・前提ツール確認
│   ├── src/process.rs                  子プロセスの起動・終了（OS別）
│   ├── src/logbuf.rs                   ログ保持とマスク
│   └── tests/                          結合テスト（ダミーの aws スクリプトを使う）
└── docs/                               設計書
```

## AWS CLI のダミーでのテスト

結合テストは `src-tauri/tests/fixtures/` のダミー `aws` スクリプトに差し替えて、接続処理の各ステップとエラー分岐を確認する（[08-testing-and-open-issues.md](08-testing-and-open-issues.md)）。アプリ本体でも設定画面の「前提ツール」で `aws` のパスを指定できる。
