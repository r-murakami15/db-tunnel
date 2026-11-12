# 3. データ設計

[目次に戻る](README.md)

## 3.1 接続先（Target）

| フィールド | 型 | 必須 | 内容 |
| --- | --- | --- | --- |
| `id` | UUID | アプリが採番 | 内部ID。JSONには出さない |
| `name` | string（1〜64文字） | 必須 | 一覧に出す表示名 |
| `profile` | string | 必須 | AWSプロファイル名 |
| `env` | `dev` \| `stg` \| `prod` | 必須 | 環境 |
| `app` | string（英小文字・数字・`-`） | 必須 | 踏み台とRDSの検索に使うプレフィックス |
| `localPort` | integer（1024〜65535） | 任意 | 省略時は13306 |

重複判定のキーは `profile` + `env` + `app` とする。インポート時の「上書き」もこのキーで判定する。

## 3.2 インポート・エクスポート用JSON

```json
{
  "version": 1,
  "targets": [
    {
      "name": "Acme Portal",
      "profile": "acme-portal",
      "env": "dev",
      "app": "acme-portal",
      "localPort": 13306
    }
  ]
}
```

- `version` が未対応の値ならファイル全体を拒否する。
- 未知のフィールドは無視し、プレビューに警告を出す。
- ファイルサイズは1MBまで、件数は500件までとする。

## 3.3 アプリの設定ファイル

| OS | 保存場所 |
| --- | --- |
| Windows | `%APPDATA%\db-tunnel\config.enc` |
| Mac | `~/Library/Application Support/db-tunnel/config.enc` |

中身は、接続先の一覧とアプリ設定（ウィンドウサイズ、最後に選んだ接続先など）をまとめたJSONを暗号化したもの。ファイルの構造は次のとおり。

| 部分 | サイズ | 内容 |
| --- | --- | --- |
| マジック | 4バイト | `RDSC` |
| 形式バージョン | 1バイト | `0x01` |
| nonce | 12バイト | 保存のたびにランダム生成 |
| 暗号文 + 認証タグ | 可変 | AES-256-GCM。追加認証データにマジックとバージョンを使う |

保存は一時ファイルに書いてからリネームし、書き込み途中で落ちても壊れないようにする。直前の版を `config.enc.bak` として1世代残す。

暗号化の鍵の管理は [4. セキュリティ設計](03-security.md) を参照。
