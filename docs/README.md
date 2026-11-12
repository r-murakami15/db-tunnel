# DB Tunnel 設計書

Nov 12, 2026 · @村上龍平

## ドキュメント一覧

| ファイル | 内容 |
| --- | --- |
| [01-requirements.md](01-requirements.md) | 機能要件（接続先の管理、接続処理、停止、ログ表示、DB接続情報、インポート・エクスポート） |
| [02-data-design.md](02-data-design.md) | データ設計（接続先の項目、インポート・エクスポート用JSON、設定ファイルの保存場所と構造） |
| [03-security.md](03-security.md) | セキュリティ設計（暗号化と鍵の管理、守れるもの・守れないもの、秘密情報の扱い、Macで署名しない影響） |
| [04-screens.md](04-screens.md) | 画面設計（4画面の構成、画面ごとの要素、画面の決まりごと、配色） |
| [05-architecture.md](05-architecture.md) | アーキテクチャ（モジュール構成図、コマンドとイベント、接続の状態、子プロセスの扱い、主なクレート） |
| [06-error-handling.md](06-error-handling.md) | エラー処理（発生箇所ごとの画面表示と動作、タイムアウト） |
| [07-build-and-release.md](07-build-and-release.md) | ビルドと配布（成果物、CI、アップデート、初回インストール手順） |
| [08-testing-and-open-issues.md](08-testing-and-open-issues.md) | テスト方針と未決事項 |
| [09-development.md](09-development.md) | 開発環境（Dockerでの開発・テスト、Mac用アプリのビルド手順、ディレクトリ構成） |

## 1. 概要

プライベートサブネットのRDS（MySQL）へ、SSMポートフォワーディングでワンクリック接続するデスクトップアプリをRustで作り、Windows・Mac向けに社内配布する。既存の `ssm-port-forward.sh` / `.ps1` の処理をGUI化し、複数の接続先の管理と同時接続、ログ表示、設定の暗号化を加える。アプリ名は「DB Tunnel」とする。

| 項目 | 内容 |
| --- | --- |
| 対象ユーザー | 社内の開発メンバー（AWS CLIの基本操作ができる人） |
| 対応OS | Windows 10/11（x64）、macOS 13以降（Apple Silicon / Intel） |
| 実装 | Rust + Tauri 2（画面はHTML/TypeScript） |
| 配布 | Windows: NSISインストーラー（ユーザー単位）、Mac: .dmg。どちらも署名なし |
| 前提ツール | AWS CLI v2（`aws login` 対応版）、Session Manager Plugin。アプリには同梱しない |

### 既存スクリプトから引き継ぐ動作

- `aws login` によるブラウザログインと、`sts get-caller-identity` でのユーザー確認
- 踏み台EC2をタグ `Name=*<アプリ名>*-<環境>*bastion*`（running のみ）で検索
- RDSを `DBInstanceIdentifier` が `<アプリ名>` で始まり `-<環境>-mysql` で終わる条件で検索し、エンドポイントと `MasterUserSecret.SecretArn` を取得
- Secrets Managerから接続ユーザーとパスワードを取得
- `AWS-StartPortForwardingSessionToRemoteHost` で `localhost:<ローカルポート>` → `RDS:3306` に転送
- prod接続前の確認、踏み台やRDSが見つからない・複数見つかる場合のエラー停止

### 変更・追加する点

- 接続先を「表示名・プロファイル・環境・アプリ名・ローカルポート」の組で登録・管理する
- 複数の接続先に同時に接続できる（ポートは接続先ごと、初期値13306）
- AWS CLIの出力を接続先ごとのログ領域で閲覧できる
- 接続先はJSONでインポート・エクスポートでき、アプリ内の設定ファイルは暗号化して保存する
- パスワードは画面とログに平文で出さず、マスク表示とコピーで扱う

リージョンは `ap-northeast-1` 固定とする。
