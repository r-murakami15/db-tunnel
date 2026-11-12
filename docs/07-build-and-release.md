# 8. ビルドと配布

[目次に戻る](README.md)

GitHub Actionsでタグを打つとWindows版とMac版をビルドし、インストーラーをリリースに添付する。コード署名はしない。

## 8.1 成果物

| OS | 形式 | 内容 |
| --- | --- | --- |
| Windows | NSISインストーラー（`*_x64-setup.exe`） | ユーザー単位インストール（管理者権限不要）。WebView2がなければ自動で入れる。スタートメニュー登録とアンインストーラー付き |
| Mac | `.dmg`（Universal: Apple Silicon + Intel） | `.app` をアプリケーションフォルダへドラッグ |

## 8.2 CI

- トリガー: `v*` 形式のタグ（例: `v1.0.0`）。バージョンは `tauri.conf.json` と `Cargo.toml` で揃える。
- ジョブ: `windows-latest` と `macos-latest` のマトリクスで `tauri-apps/tauri-action` を実行する。Macは `--target universal-apple-darwin` でビルドする。
- プルリクエストでは `cargo fmt --check`、`cargo clippy`、`cargo test`、フロントエンドの型チェックを実行する。

## 8.3 アップデート

- Tauriのアップデーター機能を使い、起動時に新しい版を確認して「更新」ボタンを出す。
- 更新ファイルにはアップデーター用の署名（OSのコード署名とは別物で、無料で作れる鍵）を付け、改ざんされた更新を拒否する。秘密鍵はGitHubのSecretsに置く。
- 更新情報（`latest.json`）と更新ファイルは、社内から認証なしで取得できる場所に置く必要がある。置き場所は未決定（[9章](08-testing-and-open-issues.md#92-未決事項)）。

## 8.4 初回インストール手順（利用者向け手順書に記載）

### Windows

1. `*_x64-setup.exe` を実行する。
2. 「WindowsによってPCが保護されました」と出たら、「詳細情報」→「実行」を押す。
3. AWS CLIとSession Manager Pluginが未導入なら、アプリの案内に従って入れる。

### Mac

1. `.dmg` を開き、アプリをアプリケーションフォルダへドラッグする。
2. アプリを開くと「開けません」と表示されるので、「完了」で閉じる。
3. 「システム設定」→「プライバシーとセキュリティ」を開き、下部の「このまま開く」を押してパスワードを入力する（macOS 15以降は右クリックの「開く」では回避できない）。
4. 初回とアップデート後に「キーチェーンへのアクセスを許可しますか」と出たら、「常に許可」を押す。

ターミナルに慣れた人向けには、手順2〜3の代わりに `xattr -dr com.apple.quarantine "/Applications/DB Tunnel.app"` も案内する。

キーチェーンの確認が出る理由は [4.4 Macで署名しないことの影響](03-security.md#44-macで署名しないことの影響) を参照。
