# DB Tunnel

プライベートサブネットのRDS（MySQL）へ、AWS SSMポートフォワーディングでワンクリック接続するデスクトップアプリ（Rust + Tauri 2）。

- 複数の接続先を登録し、同時に接続できる
- `aws login` → 踏み台・RDSの検索 → Secrets Managerからの接続情報取得 → ポートフォワーディングまでを自動で行う
- AWS CLI の出力を接続先ごとに表示し、パスワードはマスク表示・30秒で消えるコピーで扱う
- 接続先はJSONでインポート・エクスポートでき、アプリ内の設定は AES-256-GCM で暗号化（鍵はOSキーチェーン）

## 前提

- AWS CLI v2（`aws login` 対応版）
- Session Manager Plugin

## Macでビルドして使う

ホストにRustは不要（Dockerコンテナ内でビルドする）。Docker Desktop と Xcode Command Line Tools が必要。

```bash
scripts/build-mac.sh
open "dist-mac/DB Tunnel.app"
```

詳細は [docs/09-development.md](docs/09-development.md)。

## ドキュメント

設計書は [docs/](docs/README.md) を参照。
