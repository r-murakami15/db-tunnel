# 6. アーキテクチャ

[目次に戻る](README.md)

Tauri 2で、画面はWebView（HTML/TypeScript）、処理はRustで書く。AWSとのやり取りはすべてAWS CLIを子プロセスとして呼び、Rust側の接続管理が接続ごとの状態と子プロセスを持つ。

## 構成図（5モジュールと外部要素）

```mermaid
flowchart LR
    UI["WebView UI<br/>(HTML / TypeScript)"]

    subgraph Rust["Rust（Tauri 2）"]
        CMD["commands<br/>Tauriコマンド層"]
        STORE["store<br/>設定の暗号化・キーチェーン"]
        CONN["connection manager<br/>接続先ごとの状態と子プロセス"]
        AWSCLI["aws_cli<br/>CLI呼び出し・出力の解析・マスク"]
        LOG["log buffer<br/>ログの保持"]
    end

    CLI["AWS CLI /<br/>session-manager-plugin"]
    KC["OSのキーチェーン"]
    FILE["config.enc"]
    AWS["AWS<br/>(EC2 / RDS / Secrets Manager / SSM)"]

    UI -- "コマンド" --> CMD
    CMD -- "イベント<br/>(target-status / log-line)" --> UI

    CMD --> STORE
    CMD --> CONN
    CONN --> AWSCLI
    CONN --> LOG
    AWSCLI --> LOG

    STORE -.-> KC
    STORE -.-> FILE
    AWSCLI -.-> CLI
    CLI -.-> AWS

    classDef external stroke-dasharray: 5 5;
    class CLI,KC,FILE,AWS external;
```

画面はコマンドで処理を頼み、状態の変化とログの行はイベントで受け取る。点線の要素はアプリの外にある。

## 6.1 コマンドとイベント

| 種類 | 名前 | 内容 |
| --- | --- | --- |
| コマンド | `list_targets` / `save_target` / `delete_target` | 接続先の一覧・保存・削除 |
| コマンド | `connect(id)` / `disconnect(id)` | 接続の開始・停止 |
| コマンド | `reveal_password(id)` / `copy_password(id)` | 表示またはコピーするときだけパスワードを画面に渡す |
| コマンド | `import_preview(path)` / `import_apply` / `export(path)` | インポートとエクスポート |
| コマンド | `check_prerequisites` | AWS CLIとPluginの有無とバージョン |
| イベント | `target-status` | 接続先ID、状態、メッセージ |
| イベント | `log-line` | 接続先ID、時刻、出どころ（app / ssm）、stdout / stderr、本文 |

## 6.2 接続の状態

状態は「未接続 → ログイン待ち → 準備中（検索・シークレット取得） → 接続中」と進み、停止で未接続に戻る。どの段階でも失敗すれば「エラー」になり、再接続ボタンで最初からやり直す。

## 6.3 子プロセスの扱い

- `tokio::process` で起動し、stdoutとstderrを1行ずつ非同期に読んでログに流す。シェルを経由せず、引数は配列で渡す。
- `start-session` の `--parameters` はJSONではなく `host=…,portNumber=3306,localPortNumber=…` の形式で渡す（Windowsでの引用符の崩れを避けるため。既存のPowerShell版と同じ）。
- Windows: 子プロセスをJob Objectに入れ、アプリが落ちても session-manager-plugin まで終了させる。コンソール窓が出ないよう `CREATE_NO_WINDOW` を付ける。
- Mac: 子プロセスを専用のプロセスグループで起動し、停止時はグループにSIGTERM、3秒後に残っていればSIGKILLを送る。
- Macでは、Finderから起動したアプリはシェルのPATHを引き継がない。`/usr/local/bin`、`/opt/homebrew/bin` も探し、見つからなければ設定画面でパスを指定できるようにする。
- ポートの空き確認は `127.0.0.1:<port>` へのbindを試して判定する。

## 6.4 主なクレート

`tauri`、`tokio`、`serde` / `serde_json`、`aes-gcm`、`rand`、`zeroize`、`keyring`、`uuid`、`thiserror`、Tauriプラグイン（`dialog`、`clipboard-manager`、`updater`）、Windowsは `windows-sys`、Macは `nix`。
