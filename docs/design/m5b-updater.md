# M5b 設計: 自動更新（mklm-update / helper の更新要求 / NSIS / リリースの署名）

| 項目 | 内容 |
|---|---|
| 対象 | マイルストーン M5 の後半（M5b）: アップデーターとリリースの署名（計画 4.2〜4.4、6 章の M5） |
| 根拠 | 承認済みプラン 2.1〜2.2、4.2〜4.4、5 章（4.x は MSI 向けに書かれている。インストーラーは 2026-09-28 のユーザーの決定で NSIS）。M2 設計（`docs/design/m2-engine.md`）の D.9、E、G.1、I.18。M3 設計（`docs/design/m3-gui.md`）の A.4、B.5、B.17、D、E、F。M5a の実機テスト（`docs/research/m5-install-tests.md`）。main の 37989f8 のコード |
| ユーザーの決定（M5b の依頼） | 完全な自動更新（ダウンロード、署名の検証、サイレント インストール）。minisign で署名した `latest.json`。**秘密鍵はメンテナーがオフラインで保管し、GitHub の secrets には置かない**。公開鍵を 2 本（通常用とバックアップ用）埋め込み、`key_id`、`revoked_keys`、`issued_at`（巻き戻しの防止）、`expires`（凍結の検知）を持つ。確認とダウンロードは自動、インストールは利用者がボタンを押したときだけ。UAC の事前説明あり（未署名のため）。当面バイナリは署名しない。インストーラーは NSIS 3.12（WiX は使わない）。「まず使えるもの」を優先するが、更新の経路は安全に直結するので、検証の正しさは譲らない |
| 状態 | 設計（未レビュー）。この段階ではコードを変えない。新しい開発機に Rust ツールチェーン、MSVC Build Tools、NSIS がまだ入っていないため。骨組み（G.2 の WP-0）は次の段階 |
| 読み手 | M5b を分担して実装する人（G 章、H 章）とレビューする人。指摘に使えるよう、すべての節に番号を付けた |

識別子、コード、コマンドは英語のまま書く。「計画」は承認済みプラン、「m2 D.9」「m3 F.5」は M2 / M3 設計の節を指す。H1 と H2 は D 章で定義する helper の 2 つのプロセスを指す。「未確認」と書いたものは L 章にまとめた。

---

## 0. 前提と方針

### 0.1 main（37989f8）にすでにあるもの

| もの | 内容 | M5b への影響 |
|---|---|---|
| NSIS のインストーラー（`installer/nsis/mklm.nsi`） | `%ProgramFiles%\SHIN DATA CENTER\MKLM` に固定。`/S` でサイレント。実行中の検査はパスで行う: `$INSTDIR\mklm-helper.exe` と `$INSTDIR\mklm-cli.exe` を追記モードで開けなければ、MessageBox（`/SD` の既定）を出して `Quit`。動いている GUI には `"$INSTDIR\mklm.exe" --quit` で終了を頼む | 終了コードを決めていない（`Quit` の終了コードは NSIS の既定任せ）。D.9 で全部の拒否に `SetErrorLevel` を付ける。helper が `$INSTDIR` で動いている間はインストーラーが拒否するので、更新を実行する helper は別の場所のコピーでなければならない（D.4、D.7） |
| `installer/build-installer.ps1` | リリースの 3 つの exe をビルドし、`dist\MKLM-Setup-<v>-<arch>.exe` と `SHA256SUMS` を作る | デバッグのリハーサル用の `-Profile dev` を足す（F.6） |
| `.github/workflows/release.yml` | タグ `vX.Y.Z` → x64 と ARM64 のインストーラー（ARM64 は windows-latest でのクロスコンパイル）と `SHA256SUMS` を載せた**下書き**のリリース | 署名はメンテナーが下書きに足してから公開する（B.5）。鍵の検査などを足す（G.6） |
| helper（`apps/mklm-helper`） | `requireAdministrator`。固定のコマンドラインは 2 つ（パイプのセッションと `--uninstall-restore`）。HKCU と `%APPDATA%` を読まない。ネットワークのクレートを持たない | 3 つ目の固定のコマンドライン `--run-update <run-id>`（D.7）と、パイプの新しい要求（D.3） |
| パイプの約束（`mklm-ipc`） | `PROTOCOL_VERSION` 2。フレームの上限 `MAX_FRAME_LEN` 256 KiB。呼び出し元と helper のビルド ID の完全一致 | 版を 3 に上げる。ビルド ID の対象に `mklm-update` を足す（G.2） |
| 保護されたフォルダー（`mklm_win::protected_dir`） | `DataDir::Updates`（`…\MKLM\Updates`、`PRIVATE_DIR_SDDL`: SYSTEM と Administrators だけ）は定義済みで未使用。所有者、DACL、リパースポイントの検証と、先回りして作られた階層の隔離（m2 D.9、S1） | 実行ごとのサブフォルダーを、明示的な DACL で作る（D.5） |
| 書き込みのロック | `%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock` を `LockFileEx`。ジャーナルに open な操作がある間は更新しない（m2 D.9） | H1 と H2 がそれぞれロックを取り、ジャーナルを確かめる（D.4、D.7） |
| 多重起動の防止（m3 F.1） | `activate` と `quit` だけの小さなパイプ。セッション中の `quit` は `busy` | H2 がほかのセッションの MKLM にも `quit` を送る（D.8）。m3 F.5 の A12 どおり、更新を始めた GUI はパイプの最後のメッセージで自分から終了する |
| v0.1.0 | 公開済み。アップデーターがない | 最初の更新対応版（v0.2.0）は利用者が手で入れる（B.8） |

### 0.2 設計の原則

1. **境界は helper。** GUI は「何を表示するか」を決めるだけ。helper は GUI から届いたものを何も信用せず、署名、失効、巻き戻し、版、アーキテクチャ、SHA-256、大きさを自分で確かめる。
2. **分からないときは入れない（fail closed）。** 例外は `expires` だけで、これは表示のための助言にする（B.4、C.6）。
3. **helper はネットワークに触れない。** ダウンロードは非昇格の GUI が行う。helper の実行ファイルにネットワークのコードを入れない（A.9、G.6）。
4. **helper は利用者の場所のファイルを開かない。** インストーラーのバイト列はパイプで受け取る（D.3。計画 4.2 の手順 2 のとおり）。
5. **UAC は利用者がボタンを押したときだけ。** 自動の確認とダウンロードは UAC を出さない（m3 0.2 の 6）。
6. **どこで止まっても状態が分かる。** 更新の段階は HKLM に 1 回の書き込みで記録し、次に起動した GUI が結果か中断を表示する（D.6、D.13）。
7. **キーボードの設定に触れない。** 更新はジャーナルを読むだけ（open な操作があれば始めない）で、書かない。サイレントの上書きインストールは、アンインストーラーも `--uninstall-restore` も実行しない（D.9.4）。
8. **テストはネットワークに出ない。** 本番以外の URL と鍵を使う経路は、リリース ビルドには**コンパイルされない**（A.10、F.6）。

### 0.3 用語

| 用語 | 意味 |
|---|---|
| 更新情報（マニフェスト） | `latest.json`（A.2） |
| 署名ファイル | `latest.json.minisig`（A.4） |
| H1 | 利用者が［今すぐ更新］を押し、UAC で起動した `%ProgramFiles%\SHIN DATA CENTER\MKLM\mklm-helper.exe`。更新情報の検証と、インストーラーの受け取り（staging）を行う |
| H2 | H1 が `Updates\<run-id>\` にコピーして起動した helper（`--run-update <run-id>`）。インストーラーを実行する |
| 実行 ID（run-id） | `<version>-<16 桁の小文字の 16 進>`（例: `0.2.1-3f9a0c2b7d1e4a65`）。更新 1 回のフォルダー名であり、記録の鍵 |
| インストール先 | `%ProgramFiles%\SHIN DATA CENTER\MKLM`（NSIS が固定する `$INSTDIR`） |
| 鮮度 | 更新情報の `expires` を過ぎたかどうか（`Freshness`） |
| 機械の記録 | HKLM の `Update` キー（D.6）。helper だけが書く |
| 利用者の記録 | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\state.json`（C.4）。GUI と CLI が書く |

---

## A. リリースの成果物と取得

### A.1 リリースに載るファイル

| ファイル | 作る人 | 用途 |
|---|---|---|
| `MKLM-Setup-<v>-x64.exe`、`MKLM-Setup-<v>-arm64.exe` | CI（release.yml） | インストーラー |
| `SHA256SUMS` | CI | 人が照合する。`xtask sign-release` の入力 |
| `latest.json` | メンテナー（`cargo xtask sign-release`） | 更新情報 |
| `latest.json.minisig` | メンテナー（同） | 更新情報の署名 |

- ファイル名は固定（release.yml の冒頭のコメントどおり）。
- `latest.json` と `.minisig` は**下書きのうちに**上げてから公開する（B.4、B.5）。

### A.2 `latest.json`（スキーマ 1）

| フィールド | 型 | 決まり |
|---|---|---|
| `schema` | 整数 | `1` だけ |
| `product` | 文字列 | `"MKLM"` だけ（用途の分離） |
| `channel` | 文字列 | `"stable"` だけ（β チャネルは v1.x。計画 3 章） |
| `version` | 文字列 | `X.Y.Z`。プレリリースとビルド情報は不可。各数は 0〜65535（VERSIONINFO に入るため）。先頭の `v` は不可 |
| `issued_at` | 整数 | 署名した時刻（Unix 秒、UTC） |
| `expires` | 整数 | 有効期限（Unix 秒、UTC）。`issued_at < expires`、差は `MAX_VALIDITY_SECS`（800 日）以下 |
| `key_id` | 文字列 | 署名した鍵の ID（16 桁の大文字 16 進。B.2）。署名ファイルの鍵 ID と一致すること |
| `revoked_keys` | 文字列の配列 | 失効させる鍵の ID（空でもよい）。規則は B.2 |
| `min_from_version` | 文字列（省略可） | これより古い版からは自動で更新させない（手で入れてもらう）。段階を踏む必要がある変更のため。形は `version` と同じ |
| `assets` | 配列 | `x64` と `arm64` をちょうど 1 つずつ |
| `assets[].arch` | 文字列 | `"x64"` か `"arm64"` |
| `assets[].name` | 文字列 | `MKLM-Setup-<version>-<arch>.exe` と完全に一致 |
| `assets[].size` | 整数 | バイト数。1 以上 `MAX_INSTALLER_LEN`（64 MiB）以下 |
| `assets[].sha256` | 文字列 | 64 桁の小文字の 16 進 |

例（値は説明用）:

```json
{
  "schema": 1,
  "product": "MKLM",
  "channel": "stable",
  "version": "0.2.1",
  "issued_at": 1792022400,
  "expires": 1826582400,
  "key_id": "8F1A2B3C4D5E6F70",
  "revoked_keys": [],
  "assets": [
    {
      "arch": "x64",
      "name": "MKLM-Setup-0.2.1-x64.exe",
      "size": 6291456,
      "sha256": "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b"
    },
    {
      "arch": "arm64",
      "name": "MKLM-Setup-0.2.1-arm64.exe",
      "size": 6029312,
      "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    }
  ]
}
```

（1792022400 は 2026-10-15 00:00 UTC、1826582400 はその 400 日後の 2027-11-19。）

### A.3 形式の決まり

- UTF-8、BOM なし。JSON を厳密に解析する: 未知のフィールド、同じフィールドの重複、末尾の余計なデータ、型の違いはすべて拒否（`ManifestMalformed`）。
- 時刻は Unix 秒の整数にする。RFC 3339 の文字列にしないのは、日付の解析のコードを持たないため（検証の正しさを優先）。画面では GUI が現地時刻に直す。
- `xtask` は正準形（2 スペースの字下げ、A.2 の表の順、LF、末尾に改行 1 つ）で書く。クライアントは正準形を求めない。署名は受け取ったバイト列そのものに対して確かめる。
- 互換のない変更は `schema` を上げ、**別の名前**（`latest-v2.json`）で並べて公開する。古いクライアントは `latest.json`（スキーマ 1）を読み続ける。未知のフィールドを拒否するので、フィールドを足すだけの変更でもスキーマを上げる。

### A.4 `latest.json.minisig`

- minisign の署名ファイル（4 行: untrusted comment、署名、trusted comment、全体の署名）。**prehashed（アルゴリズム `ED`、BLAKE2b-512）だけ**を受け付ける（`minisign_verify::PublicKey::verify(…, allow_legacy = false)`）。
- trusted comment（署名の対象に含まれる）: `mklm-latest-json v1 version=<version> issued_at=<unix 秒>`。クライアントは、先頭が `mklm-latest-json v1` で、その直後が行末か空白であることを求める（`WrongTrustedComment`）。これは用途の分離のため: 同じ鍵で別のファイルに付けた署名を、`latest.json` の署名として通させない。残りの部分は、人が `minisign -V` で読むためのもので、判断には使わない。
- untrusted comment は読まない。
- 署名ファイルは UTF-8 で 4 KiB 以下（`MAX_SIGNATURE_LEN`）。

### A.5 URL

| 取得するもの | URL |
|---|---|
| 更新情報 | `https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/latest/download/latest.json` |
| 署名 | `…/releases/download/<tag>/latest.json.minisig`（`<tag>` は更新情報の 1 回目のリダイレクトから取る。A.6）。取れなければ `…/releases/latest/download/latest.json.minisig` |
| インストーラー | `…/releases/download/v<version>/MKLM-Setup-<version>-<arch>.exe`（検証済みの更新情報の版から作る。C.8） |
| リリース ページ（画面のリンク） | `…/releases/tag/v<version>` |

- `releases/latest/download` は REST API のレート制限を消費しない（計画 4.2）。
- 実測（2026-09-29、v0.1.0 の `SHA256SUMS`）: `…/releases/latest/download/SHA256SUMS` → 302 → `…/releases/download/v0.1.0/SHA256SUMS` → 302 → `https://release-assets.githubusercontent.com/github-production-release-asset/…?…&se=…&sig=…`（1 時間ほどで切れる署名付きの URL）。
- 「最新」は「下書きでもプレリリースでもない、最も新しいリリース」。リリースの `make_latest` で変えられる（GitHub REST のドキュメント）。
- 更新情報には URL を持たせない。インストーラーの URL は、固定のリポジトリの URL と、検証した `version` と `name` から作る（C.8）。署名の鍵が漏れても、ダウンロード先を任意のホストに変えられない。

### A.6 URL の規則とリダイレクト

- **本番の規則**（`UrlPolicy::production()`）
  - https だけ。ポートは 443（省略可）だけ。userinfo、IP アドレスのホスト、ASCII 以外のホスト、フラグメント（`#`）、空白と制御文字は拒否。
  - 最初の要求のホストは `github.com`。リダイレクト先のホストは、`github.com` か、`.githubusercontent.com` で終わるもの。
  - 完全一致の一覧にしない理由: GitHub はリリースのファイルの配布元を `objects.githubusercontent.com` から `release-assets.githubusercontent.com` に変えたことがある。一覧から外れると更新が止まり、その修正を更新で届けられない。ファイルの完全性は署名と SHA-256 が守るので、ホストの制限は「知らない場所に行かない」ための多重の備えの 1 つと位置づける。
- **リダイレクト**
  - 1 回の取得につき最大 5 回（`TooManyRedirects`）。
  - WinHTTP の自動のリダイレクトは切る（`WINHTTP_OPTION_REDIRECT_POLICY` を `NEVER`）。`mklm_update::fetch` が 1 回ずつ判断する。
  - `Location` は絶対 URL か、同じホストの絶対パス（`/` で始まる）。解決した URL を同じ規則で確かめてから接続する。規則に合わなければ**接続しない**（`RedirectNotAllowed`）。
  - 301、302、303、307、308 だけをリダイレクトとして扱う。`Location` がなければ `MissingLocation`。
- **tag の取り出し**: 更新情報の 1 回目のリダイレクトの `Location` が `https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/download/v<X.Y.Z>/latest.json`（`X.Y.Z` は A.2 の `version` の規則）なら、tag は `vX.Y.Z`。署名はその tag の URL から取る。更新情報と署名の取得の間に新しいリリースが公開されても、同じリリースの 2 つがそろうようにするため。tag が分かったときは、更新情報の `version` が tag と一致することも確かめる（`TagMismatch`）。形が違えば（GitHub の仕様の変更）tag なしで `releases/latest/download` から署名を取る。2 つがずれて検証に失敗した場合は、次の確認でやり直す。

### A.7 要求の中身

- GET だけ。送るヘッダーは `User-Agent` と `Accept`（更新情報と署名は `*/*`、インストーラーは `application/octet-stream`）だけ。
- `User-Agent`: `MKLM/<version> (Windows; <x64|arm64>; +https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager)`。利用者や PC を識別する情報は入れない。
- Cookie と認証情報（GitHub のトークンを含む）は送らない（`WINHTTP_DISABLE_COOKIES`）。サーバーの認証（401）は失敗として扱う。
- `Accept-Encoding` を送らない。`Content-Encoding` の付いた応答は拒否（`UnexpectedEncoding`）。
- TLS は 1.2 と 1.3 だけ（`WINHTTP_OPTION_SECURE_PROTOCOLS`）。証明書の検証は Windows の既定（Schannel と Windows の証明書ストア）。証明書のピン留めはしない（GitHub の証明書の更新で止まるため）。
- プロキシ: `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`（Windows 8.1 以降。システムと利用者ごとのプロキシ設定、IE の設定、PAC を使い、複数のプロキシの切り替えと認証も扱う。Microsoft Learn の `WinHttpOpen`）。それでも 407 なら `ProxyAuthRequired`。
- 同期モードの WinHTTP を専用のスレッドで使う（`WINHTTP_FLAG_SECURE_DEFAULTS` は非同期モードを強制するので使わず、TLS の版は上のオプションで決める）。取り消しは、読み取りの合間に見るフラグと、受信の期限による（A.8）。

### A.8 大きさの上限と期限

| 取得するもの | 上限 | 期限 |
|---|---|---|
| `latest.json` | 64 KiB（`MAX_MANIFEST_LEN`）。`Content-Length` が上限を超えれば本文を読まずに拒否。本文を上限 + 1 バイトまで読んで超えれば拒否（`TooLarge`） | 名前解決 15 秒、接続 15 秒、送信 30 秒、受信 30 秒（1 回の読み取りの無通信）。署名と合わせた全体で 60 秒（`DeadlineExceeded`） |
| `latest.json.minisig` | 4 KiB（`MAX_SIGNATURE_LEN`） | 同じ 60 秒の中 |
| インストーラー | 検証済みの更新情報の `size` ちょうど（64 MiB 以下）。`Content-Length` が違えば読まずに拒否。短くても長くても `SizeMismatch`。読みながら SHA-256 を計算し、違えば `HashMismatch` | 名前解決と接続は同じ。受信 60 秒。全体 30 分 |

- 取り消し（利用者の［キャンセル］、GUI の終了）は、64 KiB の読み取りの合間にフラグで確かめる。最悪の遅れは受信の期限（30 秒 / 60 秒）。
- 自動の再試行はしない。失敗は次の定時の確認（E.1）か、利用者の［もう一度確認］でやり直す。

### A.9 HTTP クライアントの選択: WinHTTP（`windows` クレート経由）

| 観点 | WinHTTP（`windows` 0.62.2 の `Win32_Networking_WinHttp`） | ureq 3 + rustls（ring） | ureq / reqwest + aws-lc-rs | ureq / reqwest + native-tls（schannel） |
|---|---|---|---|---|
| TLS と証明書 | Schannel、Windows の証明書ストア（企業の CA も含む） | 自前の TLS。ストアを使うには rustls-platform-verifier が要る | 同左 | Schannel、Windows のストア |
| プロキシ | システムと利用者ごとの設定、PAC、WPAD、プロキシ認証（`AUTOMATIC_PROXY`） | 環境変数（`HTTPS_PROXY`）だけ。PAC はない | 同左（reqwest はレジストリの静的な設定を読むが PAC はない — 未確認） | ureq は環境変数だけ |
| aarch64-pc-windows-msvc のビルド | 追加の道具は要らない（Rust のバインディングとシステムの DLL） | ring は ARM64 の Windows で **clang が必須**（ring の BUILDING.md、issue #2117） | C コンパイラが要る。x64 の Windows では NASM も（事前ビルドのオブジェクトか `AWS_LC_SYS_NO_ASM` で回避可） | schannel は Rust だけ。reqwest は tokio と hyper を連れてくる |
| 依存の重さ | 新しいクレートは 0（`windows` の feature だけ） | 十数個（rustls、ring、webpki など） | aws-lc-sys（C のソース） | reqwest なら 100 前後 |
| ライセンス | OS の部品 | ISC / MIT / Apache-2.0（ring は独自の条項を含む） | Apache-2.0 / ISC / OpenSSL 系 | MIT / Apache-2.0 |
| unsafe の置き場 | `mklm-win`（このプロジェクトの規則どおり、unsafe はここだけ） | なし（クレートの中） | 同左 | 同左 |
| テスト | `http://127.0.0.1` のループバックで確かめられる（A.10） | 同左 | 同左 | 同左 |

**決定: WinHTTP。** 企業のネットワーク（PAC、認証付きのプロキシ、TLS 検査用の社内 CA）でそのまま動き、ARM64 のクロスビルド（CI は windows-latest の x64 で ARM64 もビルドする）に clang、CMake、NASM が要らず、新しい依存がない。代わりに FFI のコードを自分で書くが、使う関数は `WinHttpOpen`、`WinHttpSetOption`、`WinHttpSetTimeouts`、`WinHttpConnect`、`WinHttpOpenRequest`、`WinHttpSendRequest`、`WinHttpReceiveResponse`、`WinHttpQueryHeaders`、`WinHttpReadData`、`WinHttpCloseHandle` の 10 個に限られる。

**置き場所**

| 層 | 場所 | 中身 |
|---|---|---|
| FFI | `mklm_win::net`（feature `net`） | 1 回の GET と、本文の逐次の読み取り。リダイレクトは追わない。unsafe はここだけ |
| 規則 | `mklm_update::fetch` | リダイレクト、URL の規則、上限、期限、SHA-256。OS に依存しない純粋なコード（`Transport` トレイトの上で動く） |
| つなぎ | `mklm_update::winhttp`（feature `winhttp`、Windows だけ） | `Transport` を `mklm_win::net` で実装する |
| 利用 | `mklm-client`（`mklm-update` を feature `winhttp` 付きで使う）→ GUI と CLI | — |
| 使わない | `mklm-helper`（`mklm-update` を feature なしで使う） | 検証のコードだけが入る |

ワークスペースをまとめてビルドすると feature が統合され、helper の中の `mklm-update` にも `winhttp` が付く（m3 A.1 の `gui` と同じ事情）。helper はそのコードを呼ばないので、最終的な実行ファイルには残らない。念のため release.yml で、`mklm-helper.exe` が `WINHTTP.dll` をインポートしていないことを確かめる（G.6）。

### A.10 テストとデバッグでの URL と鍵の差し替え

- 本番の URL は `Endpoints::production()` が返す固定の文字列だけ。**実行時に URL を変える手段（環境変数、引数、設定ファイル）は、リリース ビルドには存在しない。**
- ループバックの規則 `UrlPolicy::loopback()`、`Endpoints::loopback(base)`（`http://127.0.0.1:<port>` だけ）、`mklm_win::net` の平文 HTTP（接続先が `127.0.0.1` のときだけ）、`WinHttpTransport::new_without_proxy` は、すべて `#[cfg(debug_assertions)]`。
- デバッグ ビルドの GUI と CLI だけが `--update-endpoint=http://127.0.0.1:<port>` を解釈する（F.6 のリハーサル用）。リリース ビルドではこの引数を解釈するコードがなく、ほかの未知の引数と同じくログに書いて無視する。
- 鍵も同じ: デバッグ ビルドだけ、ビルド時の環境変数 `MKLM_UPDATE_DEV_PUBKEY`（`option_env!`）の公開鍵を、埋め込みの鍵に加えて信頼する（F.6）。
- `cfg(test)` ではなく `debug_assertions` を根拠にする理由: `cfg(test)` はテストされているクレートの中でしか立たず、`mklm-update` のテストが使う `mklm-win` の中では立たない。`debug_assertions` はテストのプロファイルでも立ち、クレートをまたいで同じ値になる。リリース プロファイルでは Cargo の既定で立たない。
- 守り: ルートの `Cargo.toml` の `[profile.release]` に `debug-assertions = false` を明示する（WP-0）。release.yml は、ビルドした 3 つの exe の VERSIONINFO に `VS_FF_DEBUG` が立っていないこと（`(Get-Item …).VersionInfo.IsDebug` が false）を確かめる。各 exe の `build.rs` は debug プロファイルでだけこの印を立てる（`apps/mklm-helper/build.rs` の `MKLM_FILEFLAGS`）。
- 前例: helper にはすでに、デバッグ ビルドだけの `MKLM_DEBUG_PAUSE`（m2 H.2 の R5）がある。

---

## B. 署名（メンテナー、オフライン）

### B.1 クレート

| クレート | 版（crates.io、2026-09-29） | ライセンス | 使う場所 | 備考 |
|---|---|---|---|---|
| `minisign-verify` | 0.3.0（2026-09-25） | MIT | `mklm-update`（GUI、CLI、helper） | 依存 0。`=0.3.0` で固定する（検証の中心なので、上げるときは意図して上げる） |
| `minisign` | 0.10.0（2026-09-25） | MIT | `xtask` と `mklm-update` の dev-dependency | 鍵の生成（scrypt でパスワード暗号化）と署名。製品には入らない |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | `mklm-update`、`xtask` | 純粋な Rust |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | `mklm-update`、`xtask` | 依存 0 |

- `minisign-verify` 0.3.0 の API（docs.rs で確認）: `PublicKey::from_base64(&str)`、`Signature::decode(&str)`、`PublicKey::verify(&[u8], &Signature, allow_legacy: bool) -> Result<(), Error>`（鍵 ID が違えば `Error::UnexpectedKeyId`、legacy を許さない設定で legacy なら `Error::UnexpectedAlgorithm`）、`Signature::trusted_comment() -> &str`。**鍵 ID を返す公開の関数はない。** そこで鍵 ID は、`mklm-update` が公開鍵と署名の base64 を自分で解いて読む（C.2）。
- 両クレートとも 4 日前に版が上がったばかりで、0.2.x / 0.9.x からの変更点は確かめられていない（L 章）。WP-0 がドキュメントでシグネチャを確かめる。
- MIT のクレートの著作権表示は、計画 2.4 の `THIRD-PARTY-LICENSES`（cargo-about）に入れる。

### B.2 鍵の形と失効の規則

- 鍵は 2 本: 通常用（`KeyRole::Primary`）とバックアップ用（`KeyRole::Backup`）。どちらも minisign の Ed25519 で、秘密鍵はパスワード付き（scrypt）。
- **鍵 ID**: 8 バイト。表記は 16 桁の大文字の 16 進で、8 バイトを little-endian の `u64` として読んだもの（minisign のコマンドが表示する形に合わせる。未確認のため、WP-U がテストで `minisign` クレートの出力と照合する）。
- **埋め込み**（`crates/mklm-update/src/keys.rs`）: `EMBEDDED_KEYS`（ID、役割、base64 の公開鍵）と `REVOKED_KEY_IDS`（その版の時点で失効している ID）。
  - 鍵ができるまでは `EMBEDDED_KEYS` は空。空のビルドでは更新が使えない（`UpdateRefusal::NotConfigured`。GUI はその旨を表示する）。
  - release.yml は `cargo xtask check-keys` で、空のまま（または壊れた鍵のまま）のリリースを止める（G.6）。
- **失効の規則**（`verify_manifest` と `xtask` が同じ規則を使う）
  1. 署名した鍵が失効していれば拒否（`RevokedKey`）。失効の出どころは、埋め込みの `REVOKED_KEY_IDS`、機械の記録、利用者の記録（C.4）のどれでもよい。
  2. 検証に通った更新情報の `revoked_keys` は、記録に足す。取り消せない。
  3. 更新情報は、自分に署名した鍵を失効させられない（`IllegalRevocation`）。
  4. 通常用の鍵で署名した更新情報は、バックアップ用の鍵を失効させられない（`IllegalRevocation`。更新情報ごと拒否）。バックアップ用の鍵で署名した更新情報は、通常用の鍵を失効させられる。
     - 理由: 通常用の鍵はリリースのたびに使うので、漏れる機会が多い。漏れた通常用の鍵で「バックアップ用を失効」と書いた更新情報を作れば、復旧の道を断てる。これを許さない。
     - 裏返しとして、バックアップ用の鍵が漏れると、通常用を失効させて自動更新を止める攻撃ができる。バックアップ用の鍵はほとんど使わず、別の場所に保管するので、この危険の方が小さいと判断した（B.6、B.7）。
  5. 埋め込みにない ID（将来の鍵など）の失効も記録する。
- **巻き戻しの記録は鍵ごと**: 「その鍵で署名された更新情報の `issued_at` の最大値」を鍵ごとに持つ（C.4）。鍵をまたいで 1 つの最大値にしない理由: 漏れた鍵で遠い未来の `issued_at` を付けた更新情報を 1 度でも受け取ると、その後の正しい更新情報（バックアップ用の鍵による失効の告知も）がすべて「古い」として拒否されてしまうため。

### B.3 `xtask`（署名の道具）

- 場所: `xtask/`（ワークスペースのメンバー、`publish = false`、バイナリ名 `xtask`）。`.cargo/config.toml` の別名 `xtask = "run --package xtask --locked --"` で `cargo xtask <コマンド>` と打つ（計画 2.4 の `xtask/`）。
- インストーラーには入らない（`build-installer.ps1` がビルドするのは `mklm`、`mklm-cli`、`mklm-helper` だけ）。`cargo test --workspace` で、署名と検証の往復がテストされる（F.1）。
- 依存: `mklm-update`（検証は製品と同じコード）、`minisign`、`sha2`、`semver`、`serde_json`、`clap`、`anyhow`。

| コマンド | 動作 |
|---|---|
| `keygen --role primary\|backup --out <dir>` | パスワードを 2 回尋ねる（画面に出さない）。`<dir>\mklm-<role>.key`（minisign と互換の、暗号化された秘密鍵）と `<dir>\mklm-<role>.pub` を書き、鍵 ID と、`keys.rs` に貼る 1 行を表示する（例: `TrustedKey { id: "8F1A2B3C4D5E6F70", role: KeyRole::Primary, public_key: "RWQ…" },`）。`<dir>` がリポジトリの作業ツリーの中（上の階層に `.git` がある）なら拒否する。既存のファイルは上書きしない |
| `sign-release --tag vX.Y.Z --dist <dir> --key <秘密鍵> [--expires-days N] [--revoke <KEYID>]... [--min-from-version X.Y.Z] [--out <dir>]` | 下の手順 |
| `verify --dir <dir> [--installed X.Y.Z] [--arch x64\|arm64]` | `<dir>` の `latest.json` と `.minisig` を、埋め込みの鍵と空の記録で検証し、結果（版、鍵、有効期限、アセット）を表示する。公開後の確認にも使う |
| `check-keys` | 埋め込みの鍵が「通常用 1 本とバックアップ用 1 本」で、どちらも解析でき、表記の ID が公開鍵の ID と一致し、互いに違い、`REVOKED_KEY_IDS` に入っていないこと。release.yml が呼ぶ |
| `dev-keygen --out <dir>`、`sign-release --dev …`、`serve-releases --dir <dir> [--port N]` | デバッグ ビルドのリハーサル専用（F.6） |

**`sign-release` の手順**

1. `--tag` を `v` と A.2 の規則の版に分ける（プレリリースとビルド情報は拒否）。
2. `<dist>` に `SHA256SUMS`、`MKLM-Setup-<v>-x64.exe`、`MKLM-Setup-<v>-arm64.exe` がそろっていること。
3. 2 つのインストーラーの SHA-256 と大きさを計算し、`SHA256SUMS` の同じ名前の行と一致すること。`SHA256SUMS` に余分な行や重複があれば拒否する。
4. 秘密鍵の ID が `EMBEDDED_KEYS` にあること（**そのリリースのバイナリが信頼しない鍵では署名させない**）。`--revoke` の組み合わせが B.2 の規則に合うこと。
5. 更新情報を正準形で作る。`issued_at` は今、`expires` は今 + `--expires-days`（既定 400 日、上限 800 日）。
6. パスワードを尋ね、A.4 の trusted comment を付けて署名する。
7. **自己検証**: `mklm_update::verify_manifest` を、埋め込みの鍵、空の記録、`installed = 0.0.0`、x64 と arm64 の両方で呼ぶ。通らなければ何も書かない。
8. `latest.json` と `latest.json.minisig` を `--out`（既定は `<dist>`）に書き、要約（版、有効期限の日付、鍵 ID と役割、2 つのアセットの大きさと SHA-256）を表示する。

- 公式の `minisign` コマンドでも同じものが作れる（`minisign -S -s mklm-primary.key -m latest.json -t "mklm-latest-json v1 version=0.2.1 issued_at=…"`）。`xtask` が使えないときの予備として、B.5 の注に書く。
- 有効期限を延ばす、署名し直す: 下書きのうちなら `sign-release` をもう一度実行して上げ直す。公開した後は新しいリリースが要る（B.4）。

### B.4 `expires` と GitHub の immutable releases

**GitHub のドキュメントで確かめたこと**（2026-09-29）

- 公開した時点で、Git のタグとリリースのファイルが固定される。ファイルの追加、置き換え、削除はできない。
- 題とリリースノートは公開後も編集できる。
- 下書きのうちは自由に編集できる（「すべてのファイルを下書きに付けてから公開する」ことが推奨されている）。
- リリースは削除できるが、同じタグ名は二度と使えない。
- 公開すると、タグ、コミット、ファイルを含む release attestation が自動で作られる。
- リポジトリか組織の設定で有効にする。

**帰結と決定**

1. `latest.json` と `.minisig` は**下書きのうちに**上げる。公開した後では直せない。
2. `releases/latest/download` は最新のリリースのファイルしか配らない。同じリリースの `latest.json` を署名し直して `expires` を延ばすことはできないので、延ばすには新しいリリースが要る。
3. したがって、有効期限の既定は **400 日**（13 か月あまり）にする。年に 1 回以上リリースすれば切れない。依存クレートの更新やセキュリティ修正を兼ねた保守リリースを、年に 1 回の目安にする（B.5 の最後）。
4. **`expires` は「凍結の検知」のための助言で、インストールの可否には使わない**（C.6）。
   - 期限切れの更新情報が差す版でも、署名が正しく、入っている版より新しいなら、それを入れて状態が悪くなることはない。
   - 期限を門にすると、メンテナーが 1 年あまり動けないだけで、正しい最新版すら自動で入らなくなる。
   - 古い更新情報の再送（巻き戻し）は `issued_at` の記録で、ダウングレードは版の比較で防ぐ（C.4、C.5）。凍結（新しい版を隠され、古いものを見せ続けられる）は `expires` で**気付かせる**。
5. **期限が切れたときに利用者が見るもの**: 設定の「更新」欄と更新のページに、情報として「更新情報の有効期限（2027/11/19）を過ぎています。新しい版が長く公開されていないか、古い情報が届いています。GitHub のリリース ページで確かめてください。［リリース ページを開く］」と出す。メイン画面のバナーとトレイには出さない（毎日の騒音にしない）。CLI の `update --check` は警告の行を 1 行出す。キーボードの機能には何も影響しない。
6. 間違った `latest.json` を公開してしまった場合は直せない。次の版を出すか、1 つ前のリリースに「最新」の印を戻す（immutable なリリースでも「最新」の印を変えられるかは未確認）。クライアントの側では、ハッシュが違えばダウンロードで止まり、形式が違えば「更新を確認できません」になるだけで、害はない。
7. 公開の前の確認は道具で行う（B.3 の手順 7、B.5）。

### B.5 メンテナーのリリース手順（チェックリスト）

この節は `docs/maintainer/release-signing.ja.md`（WP-U）にそのまま写す。

**準備（初回だけ）**

- [ ] `cargo xtask keygen --role primary --out E:\mklm-keys`（E: はオフラインで保管する USB メモリ。B.6）
- [ ] 別の媒体に `cargo xtask keygen --role backup --out F:\mklm-keys-backup`
- [ ] 表示された 2 行を `crates/mklm-update/src/keys.rs` の `EMBEDDED_KEYS` に貼り、`cargo xtask check-keys` → コミット（公開鍵だけ。秘密鍵は決してリポジトリに入れない）
- [ ] 公開鍵を `docs/install-guide.ja.md` の「ファイルが正しいか確かめる」に載せる
- [ ] GitHub アカウントにハードウェア キーの 2 段階認証、`main` とタグ `v*` の保護（計画 4.3）
- [ ] リポジトリの設定で immutable releases を有効にする（J 章の質問 5）

**毎回（gh CLI を使う場合）**

1. `Cargo.toml` の `[workspace.package] version` を上げてコミットし、`main` に入れる。
2. `git tag vX.Y.Z` → `git push origin vX.Y.Z`。
3. Actions の「Release」が緑になり、下書きのリリースに 2 つのインストーラーと `SHA256SUMS` が付くのを待つ。
4. タグの版を取り出す: `git switch --detach vX.Y.Z`（`xtask` をそのタグのソースでビルドするため。埋め込みの鍵がリリースのバイナリと同じになる）。
5. `gh release download vX.Y.Z --pattern "MKLM-Setup-*" --pattern SHA256SUMS --dir dist-vX.Y.Z`
6. （任意、release.yml に来歴の証明を足した後）`gh attestation verify dist-vX.Y.Z\MKLM-Setup-X.Y.Z-x64.exe --repo SHIN-DATA-CENTER/multi-keyboard-layout-manager`（arm64 も）
7. 秘密鍵の USB メモリをつなぐ。
8. `cargo xtask sign-release --tag vX.Y.Z --dist dist-vX.Y.Z --key E:\mklm-keys\mklm-primary.key`。パスワードを入れ、表示された要約（版、有効期限、鍵、2 つのハッシュ）を見る。
9. USB メモリを外す。
10. `gh release upload vX.Y.Z dist-vX.Y.Z\latest.json dist-vX.Y.Z\latest.json.minisig`
11. 下書きのファイルが 5 つ（インストーラー 2、`SHA256SUMS`、`latest.json`、`latest.json.minisig`）であることを確かめる: `gh release view vX.Y.Z`
12. 公開する: `gh release edit vX.Y.Z --draft=false --latest`
13. 公開後の確認: `curl.exe -L -o check\latest.json https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/latest/download/latest.json`、`.minisig` も同様に取り、`cargo xtask verify --dir check`。
14. `git switch main`。

**毎回（gh CLI を使わない場合）**

- 手順 5: ブラウザーで GitHub の「Releases」→ 下書きの「Edit」を開き、3 つのファイルをダウンロードして `dist-vX.Y.Z` に置く（下書きはプッシュ権限のある人にだけ見える）。
- 手順 10〜12: 同じ「Edit」の画面で 2 つのファイルをドラッグして付け、「Set as the latest release」にチェックを入れて「Publish release」。
- 手順 13: ブラウザーで上の URL を開いて保存し、`cargo xtask verify`。

**注**

- `gh` が下書きをタグ名で扱えるか（`download`、`upload`、`edit`）は未確認。できなければブラウザーの手順で行う。
- `xtask` が使えないとき: 公式の `minisign` コマンド（B.3 の末尾）で署名し、`latest.json` は前回のものを手で直して作る。そのあと必ず `cargo xtask verify` を通す。
- 年に 1 回は保守リリースを出す（B.4 の 3）。`xtask sign-release` が表示する有効期限の日付の 2 か月前をカレンダーに入れる。

### B.6 鍵の保管

- 通常用の秘密鍵: 暗号化した USB メモリ（BitLocker To Go）に置き、署名するときだけつなぐ。パスワードはパスワード マネージャーに置く。
- バックアップ用の秘密鍵: 通常用とは別の媒体で、別の場所に置く（例: 自宅の金庫と別の建物）。日常では使わない。
- 秘密鍵をクラウドの同期フォルダー、GitHub、CI、開発機のディスクに置かない。`keygen` は作業ツリーの中への書き込みを拒否する。
- 署名する PC は普段の開発機でよい（「保管はオフライン、使う瞬間だけつなぐ」を前提にする）。専用のオフラインの PC を用意するかはメンテナーの判断（J 章の質問 6）。
- 年に 1 回、バックアップ用の鍵の媒体が読めることと、パスワードを覚えていることを確かめる（`cargo xtask sign-release --dev` ではなく、`minisign -S` で適当なファイルに署名してみる）。

### B.7 鍵が漏れたとき、なくしたとき

| 事態 | 手順 | 利用者の側 |
|---|---|---|
| 通常用の鍵が漏れた（疑いを含む） | (1) オフラインで新しい通常用の鍵 P2 を作る。(2) `EMBEDDED_KEYS` を「P2（通常用）、今のバックアップ用 B1」にし、`REVOKED_KEY_IDS` に旧 P1 を足す。(3) 新しい版を出す。(4) その `latest.json` を**バックアップ用の鍵**で `--revoke <P1 の ID>` を付けて署名する。(5) 公開し、README とリリースノートで知らせる。(6) 後の版で新しいバックアップ用 B2 に替えるかを決める（B1 は 1 度使ったので） | 自動更新を使っている PC は、次の確認で P1 の失効を記録し、以後 P1 の署名を受け付けない。新しい版を入れれば P1 はバイナリの中でも失効する。失効の告知が届く前に、攻撃者が P1 で署名した更新情報と GitHub への書き込みの両方を手に入れていれば、その間は危険（署名だけでは GitHub のファイルを置き換えられない） |
| バックアップ用の鍵が漏れた | (1) 新しいバックアップ用 B2 を作る。(2) `EMBEDDED_KEYS` を「P1、B2」にし、`REVOKED_KEY_IDS` に B1 を足す。(3) 新しい版を通常用の鍵で署名して出す（B.2 の 4 により、通常用の鍵で B1 を失効させる更新情報は作れない） | 新しい版を入れるまで、古い版は B1 を信頼し続ける。攻撃者は B1 で通常用の鍵を失効させ、自動更新を止められる。その場合、利用者は手で新しい版を入れる（README で案内） |
| 両方が漏れた、または両方をなくした | 新しい鍵の組で新しい版を出し、README、リリースノート、Issue で「手で入れ直してください」と案内する | 自動更新は、手で新しい版を入れるまで止まる（危険な更新を受け付けることはない。受け付けない側に倒れる） |
| 通常用の鍵をなくした（漏れてはいない） | バックアップ用の鍵で新しい通常用の鍵を入れた版を署名して出す（失効は不要だが、念のため旧 ID を失効させてもよい） | 影響なし |
| パスワードを忘れた | 「なくした」と同じ | 同上 |

- この表は `docs/maintainer/release-signing.ja.md` にも載せる（WP-U）。

### B.8 v0.1.0 からの移行

- v0.1.0 にはアップデーターがないので、v0.2.0（最初の更新対応版）は利用者が手で入れる。v0.2.0 のリリースノートと README に「この版から自動更新に対応しました。今回だけ手でインストーラーを実行してください」と書く。
- v0.1.0 → v0.2.0 の上書きは M5a の上書きと同じ経路（M5a の実機テストその 2）。設定とジャーナルは引き継がれる。
- v0.2.0 の最初の自動更新（v0.2.0 → v0.2.1）が、実機での最初の本番の試験になる（F.7）。その前にデバッグ ビルドのリハーサル（F.6）を済ませる。

---

## C. クライアントの検証（`crates/mklm-update`）

### C.1 クレートの構成

`#![forbid(unsafe_code)]`。OS に依存しない（`winhttp` モジュールを除く）。

| モジュール | 役割 | 担当 |
|---|---|---|
| `lib.rs` | 定数（H.1）、`installer_name`、`release_page_url`、`user_agent` | WP-U |
| `keys` | 鍵 ID、埋め込みの鍵、`TrustAnchors` | WP-U |
| `manifest` | `Manifest`、`ManifestAsset`、`Arch`、`Sha256Digest`、厳密な解析と正準形 | WP-U |
| `verify` | `verify_manifest`（C.3） | WP-U |
| `state` | `TrustState`（失効と巻き戻しの記録） | WP-U |
| `version` | 版の解析と比較 | WP-U |
| `refusal` | `UpdateRefusal`（拒否の理由のすべて） | WP-U |
| `url` | 厳密な URL、`UrlPolicy`、`Endpoints` | WP-U |
| `fetch` | `Transport`、`fetch_manifest`、`download_asset` | WP-U |
| `stage` | helper がインストーラーを受け取るときの状態機械（`Stager`） | WP-U |
| `run` | 更新 1 回の記録（`RunRecord`）と結果（`UpdateResult`）、NSIS の終了コードの分類、結果の判定 | WP-H |
| `gate` | ジャーナルによる更新の可否 | WP-H |
| `winhttp`（feature `winhttp`、Windows） | `Transport` の WinHTTP 実装 | WP-U |
| `base64`（非公開） | 厳密な base64（RFC 4648、パディング必須、正準形でなければ拒否）。鍵 ID を読むため | WP-U |

依存: `mklm-core`（`Journal`、`BootId`、`Timestamp`、`ProcessIdentity`、`Liveness`）、`serde`、`serde_json`、`thiserror`、`minisign-verify`、`sha2`、`semver`。feature `winhttp` のときだけ `mklm-win`（feature `net`）。

### C.2 信頼の起点（埋め込みの鍵）

- `TrustAnchors::embedded()`
  - `EMBEDDED_KEYS` が空なら `KeyError::NotConfigured`（デバッグ ビルドで `MKLM_UPDATE_DEV_PUBKEY` があれば、その鍵だけで作る）。
  - 各鍵を `minisign_verify::PublicKey::from_base64` で読み、base64 を自分でも解いて鍵 ID（2〜9 バイト目）を取り出し、`TrustedKey::id` の表記と一致することを確かめる（`IdMismatch`）。ID の重複は `DuplicateId`。
  - `REVOKED_KEY_IDS` を持つ。
- `TrustAnchors::from_keys(&[(KeyRole, &str)], &[&str])`: テストと `xtask` 用。同じ検査をする。
- 役割の数（通常用 1、バックアップ用 1）は `xtask check-keys` が確かめる。`embedded()` は数を問わない（デバッグの鍵を足せるように）。

### C.3 検証の手順（`verify_manifest`。GUI、CLI、helper で同じ関数）

入力は `VerifyInput`（H.1）: 更新情報と署名のバイト列、信頼の起点、記録（`TrustState`）、入っている版、アーキテクチャ、今の時刻、tag（取得したときだけ）、目的（`Check` か `Install`）。**どの段階で失敗しても、それより後は何も解析しない。**

1. 大きさ: 更新情報が `MAX_MANIFEST_LEN` 以下（`ManifestTooLarge`）、署名が `MAX_SIGNATURE_LEN` 以下（`SignatureTooLarge`）。
2. 署名ファイルが UTF-8 で、`Signature::decode` で読めること（`SignatureMalformed`）。
3. trusted comment が A.4 の形であること（`WrongTrustedComment`）。
4. 署名の 2 行目の base64 から鍵 ID を読み、信頼の起点にその ID の鍵があること（`UnknownKey { key_id }`）。
5. その鍵が失効していないこと: 埋め込みの `REVOKED_KEY_IDS`、`state.revoked`（`RevokedKey`）。
6. `PublicKey::verify(manifest, &signature, false)` が成功すること（`BadSignature`）。**この時点までは、更新情報の中身を一切読まない。**
7. 更新情報を厳密に解析する（A.3。`ManifestMalformed`）。
8. `schema == 1`（`UnsupportedSchema`）、`product == "MKLM"`（`WrongProduct`）、`channel == "stable"`（`WrongChannel`）。
9. `key_id` が手順 4 の鍵 ID と一致（`KeyIdMismatch`）。
10. `revoked_keys` の各 ID が正しい形で、B.2 の規則 3、4 に反しないこと（`IllegalRevocation`、形の誤りは `ManifestMalformed`）。
11. `version` と `min_from_version` が A.2 の規則に合うこと（`BadVersion`）。tag があれば `version` と一致（`TagMismatch`）。
12. `issued_at < expires` かつ差が `MAX_VALIDITY_SECS` 以下（`BadTimestamps`）。
13. 巻き戻し: `state.max_issued_at[署名した鍵]` があり、`issued_at` がそれより小さければ拒否（`Rollback`）。同じ値は許す（同じ更新情報を取り直した場合）。
14. アセット: `x64` と `arm64` がちょうど 1 つずつで、`name` が `installer_name(version, arch)` と一致し、`size` が 1〜`MAX_INSTALLER_LEN`、`sha256` が 64 桁の小文字の 16 進（`AssetMalformed`）。自分のアーキテクチャのものを選ぶ（ないことは手順の上で起こらないが、`NoAssetForArch` を残す）。
15. 鮮度: `now_unix > expires` なら `Freshness::Expired`、そうでなければ `Fresh`（拒否ではない。C.6）。
16. 申し出の種類（`OfferKind`）: `min_from_version` があり、入っている版がそれより古ければ `ManualRequired`。そうでなく `is_newer(version, installed)` なら `Newer`。それ以外は `UpToDate`。
17. 目的が `Install` のときは、`OfferKind::Newer` 以外を拒否する（`UpToDate` → `NotNewer`、`ManualRequired` → `ManualUpdateRequired`）。

成功すると `VerifiedManifest`（H.1）を返す。記録の更新は呼び出し元が `TrustState::recorded` で行う（C.4）。

### C.4 失効と巻き戻しの記録

| 記録 | 場所 | 書く人 | 読む人 | 信頼 |
|---|---|---|---|---|
| 機械の記録 | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` の `Trust`（D.6） | helper（H1）だけ。検証に通った更新情報で、ステージングを始める前 | helper（H1、H2）、GUI、CLI（Users は読める） | helper が使うのはこれだけ |
| 利用者の記録 | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\state.json`（`ClientState` の中の `trust`） | GUI と CLI。確認のたびに | GUI と CLI | 表示のためだけ |

- **helper は利用者の記録を読まない**（HKCU と利用者のフォルダーを読まない原則。計画 2.2）。インストールの判断は、機械の記録と埋め込みの鍵だけで行う。
- GUI と CLI は、機械の記録と利用者の記録を `TrustState::merged`（鍵ごとの最大値、失効の和集合）で合わせてから検証し、成功したら `TrustState::recorded` の結果を利用者の記録に書く。機械の記録は H1 が次に更新するときに進む。
- 機械の記録が壊れていた（JSON として読めない）場合: helper は空の記録として扱い、ログに残す。書けるのは管理者だけで（D.6）、管理者は信頼の外にいないので、ここで更新を止め続けるより、失効は埋め込みの一覧と次の更新情報に任せる方がよいと判断した。
- 記録は JSON で、`schema` は 1。未知のフィールドは無視する（自分のデータで、将来の版が足しても害がない）。

### C.5 版の規則

- 更新情報の版: `X.Y.Z` だけ（A.2）。プレリリースの版は、今のチャネル（stable）には載せない。`xtask` も拒否する。
- 入っている版: 実行ファイルの `CARGO_PKG_VERSION`（GUI、CLI、helper は同じワークスペースの版）。開発用のプレリリースの版（例 `0.2.0-dev.1`）は受け付け、semver の優先順位で比べる（`0.2.0` は `0.2.0-dev.1` より新しい）。ビルド情報（`+…`）は拒否する。
- `is_newer(offered, installed)`: semver の優先順位で `offered > installed`。同じ版は「新しくない」。
- **ダウングレードはしない**: helper（H1 も H2 も）は、自分がコンパイルされた版（= インストールされている版）より新しくない更新情報を拒否する（`NotNewer`）。
- **スキップ**: 利用者が［この版をスキップ］を押した版（`settings.update.skipped_version`）は、バナーとダウンロードの対象にしない。それより新しい版が出れば、また知らせる。スキップしていても、更新のページからは入れられる。

### C.6 `expires` の扱い

- 検証は通し、`Freshness::Expired` を返すだけ（B.4 の 4）。
- GUI と CLI は E.6 の文で知らせる。helper は拒否しない（ログに残すだけ）。
- PC の時計が大きくずれていると、期限切れの表示が誤ることがある。インストールの判断には影響しない。

### C.7 アーキテクチャ

- 選ぶアセットは、**動いている MKLM のビルドのアーキテクチャ**（`Arch::of_this_build()`、`cfg!(target_arch)`）にする。GUI と helper は同じインストールなので一致する。
- 理由: ARM64 版は実機で試していない（m2 I.16。ARM64 の PC がない）。ARM64 の PC で x64 版を使っている利用者を、更新のついでに未検証の ARM64 版に切り替えない（J 章の質問 1）。
- ネイティブのアーキテクチャ（`IsWow64Process2` の `pNativeMachine`。x64 のエミュレーションで動く x64 版でも ARM64 を返すと広く報告されている — 未確認）は、GUI の情報表示（「この PC では ARM64 版も使えます」）にだけ使う（`mklm_win::os::native_machine`）。
- helper は、自分のビルドのアーキテクチャのアセットだけを受け付ける。
- ARM64 のインストーラーは x64 の PC で `.onInit` が拒否する（D.9 の終了コード 21）ので、誤って選ばれても入らない。

### C.8 アセットの選択と URL

- インストーラーの URL は `Endpoints::asset_url(version, name)` が、`…/releases/download/v<version>/<name>` として作る。`latest/download` を使わないのは、確認とダウンロードの間に新しいリリースが公開されても、検証した版のファイルを取るため。
- ダウンロードでは、大きさと SHA-256 を検証済みの更新情報と照らす（A.8）。一致したものだけを、利用者のキャッシュに正式な名前で置く（E.1）。

---

## D. インストールの流れ

### D.1 全体

```
GUI（非昇格）                 H1（昇格、$INSTDIR\mklm-helper.exe）     H2（昇格、Updates\<run-id>\mklm-helper.exe）   NSIS
 確認・ダウンロード・検証（自動）
 ［今すぐ更新］→ UAC の事前説明 → UAC
 パイプを作る、helper を起動 ──UAC──▶ 起動、ハンドシェイク（m2 E.3）
 StageUpdate{manifest, signature} ──▶ 検証（C.3、Install）
                                    ロック、ジャーナル、記録（Trust）
                                    Updates\<run-id>\ を作る、Run=staging
                 ◀── Update::SendInstaller{name,size,sha256}
 InstallerChunk × N ─────────────▶ 書きながら SHA-256、大きさ
                 ◀── Update::Received{bytes}（1 MiB ごと）
                                    一致 → 自分をコピー、Run=staged
                                    CreateProcess ─────────────────────▶ 起動（--run-update <run-id>）
                                                                        自分の場所、フォルダー、記録を確かめ、
                                                                        署名、版、SHA-256 を検証し直す
                                                                        Run=ready
                                    Run=ready を見る（最大 20 秒）
                 ◀── Update::HandedOff{run_id, to_version}
 RunOnce（--after-update）を登録                                          
 終了（1 秒の案内の後）             ロックを放して終了                   H1 の終了を待つ、ロック、ジャーナル
                                                                        インストール先の版が変わっていないこと
                                                                        ほかの MKLM に quit、終了を待つ
                                                                        Run=installing
                                                                        "<dir>\MKLM-Setup-….exe" /S ───────▶ 上書き
                                                                        終了を待つ（最大 15 分）◀─────────── 終了コード
                                                                        ファイルの版をそろって確かめる
                                                                        LastResult、Run=done、ロックを放す
 ◀────────────────── Explorer 経由で "$INSTDIR\mklm.exe" --after-update を非昇格で起動
 結果を表示                                                              後片付けをして終了
```

### D.2 GUI の前提条件（UAC の前）

［今すぐ更新］は、次をすべて満たすときだけ押せる。満たさないときはボタンを無効にし、理由を文で出す（E.6）。

1. 更新が使える（`Availability::Available`: 埋め込みの鍵があり、MKLM が `%ProgramFiles%\SHIN DATA CENTER\MKLM` から動いている）。
2. ダウンロード済みで、キャッシュの更新情報を検証し直して通る（`reverify_cached`）。インストーラーの SHA-256 は送りながらもう一度計算する（D.3）。
3. helper のセッションが動いていない（m3 A.4 の「セッションは同時に 1 つ」）。
4. ジャーナルに open な操作がない（`gate::blocker` と `startup::summarize` の `blocks_writes`。helper の判断が本物で、これは UAC を無駄に出さないための下見）。
5. 別の更新が進んでいない（`RunView::InProgress` でない）。

### D.3 パイプの要求（`PROTOCOL_VERSION` 3）

**新しいメッセージ**（型は H.2）

| 向き | メッセージ | 中身 |
|---|---|---|
| 呼び出し元 → helper | `CallerMessage::StageUpdate(StageUpdateRequest)` | `manifest`（`latest.json` の UTF-8 の文字列、そのまま）、`signature`（`.minisig` の文字列、そのまま） |
| 呼び出し元 → helper | `CallerMessage::InstallerChunk(InstallerChunk)` | `offset`（何バイト目からか）、`hex`（小文字の 16 進。1〜64 KiB 分） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::SendInstaller { name, size, sha256, chunk_len })` | 検証に通った。これだけのバイト列を送れ |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::Received { bytes })` | 受け取った量（1 MiB ごと） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::HandedOff { run_id, to_version })` | H2 に引き継いだ。呼び出し元は終了すること。helper はこの後すぐ終了する |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::Refused(UpdateRefusal))` | 断った。何も変えていない。セッションは続く（呼び出し元が `Bye`） |

- `Request` の列挙には足さない。`Request` はエンジンに写す要求の一覧（m2 S5）で、更新はエンジンを使わないため。既存の網羅的な `match`（中継の `plans_first` など）に影響しない。
- 要求の実行中は、helper の書き込み専用のスレッドがこれまでどおり 10 秒ごとに `Event::Heartbeat` を送る（m2 S7）。呼び出し元は更新のセッションで Heartbeat 以外の `Event` を受け取らない（来たらログに書いて無視）。
- `StageUpdate` は 1 つのセッションで何度でも送れる（断られた後の再試行）。`InstallerChunk` は `SendInstaller` の後でだけ受け付け、それ以外で届けばプロトコルの誤り（helper は `HelperMessage::Error` を返して切断）。
- 大きさ: 64 KiB の 16 進は 128 KiB で、フレームの上限 256 KiB に余裕を持って収まる。更新情報（64 KiB 以下）は JSON の文字列にすると引用符などのエスケープで最悪 2 倍（制御文字は更新情報の中にないので 6 倍にはならない）で、これも収まる。
- 送る側は、キャッシュのファイルを 64 KiB ずつ読み、送りながら SHA-256 を計算する。終わりで更新情報の値と違えば `Bye` を送って止める（`StageEnd::SourceChanged`。もう一度ダウンロードする）。

**バイト列をパイプで送る理由**（依頼の「ダウンロードしたファイルのパスを渡す」からの変更。K.2）

昇格した helper が、利用者が書けるパス（`%LOCALAPPDATA%` の下）を開くと、次の危険がある。

- 利用者が作ったシンボリック リンク（開発者モードでは標準ユーザーでも作れる）や途中のジャンクションで、`\\server\share\…`（UNC）へ誘導される。Windows の既定ではローカルからリモートへのシンボリック リンクをたどるので、昇格した helper が管理者の資格情報で外部の SMB サーバーに認証してしまう（NTLM の中継）。
- `\\.\pipe\…` などのデバイスへ誘導され、helper が利用者のパイプ サーバーに接続させられる。
- 検証と実行の間に中身を差し替えられる（TOCTOU）。

各階層をリパースポイントなしで開いて確かめれば防げるが、コードが増え、見落としが安全に直結する。パイプで受け取れば、helper は**利用者の場所を一切開かず**、自分で作った保護されたファイルに書くだけになる。計画 4.2 の手順 2（「helper は GUI から、latest.json、.minisig、インストーラーの中身（バイト列）だけを受け取る」）とも一致し、計画 2.2 の「helper は HKCU と `%APPDATA%` を読まない」も保てる。代償は 64 KiB ずつ 100 回前後のフレーム（数 MB。1〜2 秒）だけ。

### D.4 H1: 検証と受け取り

H1 は、既存のパイプのセッション（m2 E.1〜E.3）の中で `CallerMessage::StageUpdate` を受けたときに、`apps/mklm-helper/src/update.rs` の処理を行う。

1. 自分の実行ファイルのフォルダーが `mklm_win::os::fixed_install_dir()`（`%ProgramFiles%\SHIN DATA CENTER\MKLM`）であること（`NotInstalledCopy`）。開発用のビルド（`target\…`）は更新しない。
2. 機械の記録を読む（`update_store::read_update_store`。壊れていれば空。C.4）。
3. `verify_manifest`（目的 `Install`、入っている版 = 自分の `CARGO_PKG_VERSION`、アーキテクチャ = 自分のビルド、信頼の起点 = `TrustAnchors::embedded()`）。失敗は `Refused`。
4. `ensure_protected_dir(Base)` → `FileLock::acquire(10 秒)`（`Busy`）。
5. ジャーナルを読み（`read_journal_store` → `Journal::parse`）、`gate::check_journal`: 読めない項目があれば `JournalUnreadable`、書き込み中の項目があれば `RecoveryNeeded`、ほかの open な項目があれば `OperationOpen { waiting_for_reboot }`。
6. `Run` の記録を見る: 終わっていない記録があり、その持ち主（`stager` か `runner`）が生きていれば `UpdateInProgress`。死んでいれば（起動 ID が違うか、プロセスがない）中断として `LastResult` に `Interrupted` を書き（`run::interrupted_result`）、`Run` を消す。
7. 古い実行のフォルダーを片付ける（D.11）。
8. 機械の記録を進める: `TrustState::recorded(verified)` を `Trust` に書く（`RegFlushKey`）。
9. `ensure_protected_dir(Updates)` → `RunDir::create(<run-id>)`（`run-id` は版と `BCryptGenRandom` の 8 バイト。フォルダーは `PRIVATE_DIR_SDDL` を明示して作る。すでにあれば失敗）。
10. `Run` を書く: `phase = staging`、`caller`（パイプのサーバーの PID から、`proc_identity::process_identity` で作成時刻を付けたもの）、`caller_session`（`GetNamedPipeServerSessionId`）、`stager`（自分）、起動 ID、時刻。
11. `latest.json` と `latest.json.minisig` を、受け取ったバイト列のまま `RunDir::write_new` で書く（`CREATE_NEW`、`FlushFileBuffers`）。
12. `RunDir::create_exclusive(installer_name)`（共有なし）を開き、`mklm_ipc::staging::receive_installer` で受け取る: `SendInstaller` を送り、各 `InstallerChunk` について `offset` が受け取った量と一致すること、16 進が厳密に読めること、長さが 1〜64 KiB であること、合計が `size` を超えないことを確かめ（`Stager::accept`）、書き、SHA-256 を進める。1 つのフレームの待ちは 30 秒（`MESSAGE_TIMEOUT`）、全体は 10 分。
13. 合計が `size` になったら `StagedFile::commit`（`FlushFileBuffers` して閉じる）→ `Stager::finish`（SHA-256 の照合。`InstallerHashMismatch`）。
14. 自分の実行ファイル（`$INSTDIR\mklm-helper.exe`。インストール先なので管理者しか書けない）を `RunDir::copy_in` で `mklm-helper.exe` としてコピーし、コピーの SHA-256 が元と一致することを確かめる。
15. `Run.phase = staged`。
16. `elevation::spawn_from_elevated(<run dir>\mklm-helper.exe, "--run-update <run-id>")`（`CreateProcessW`。UAC は出ない。作業フォルダーは System32）。
17. `Run.phase` が `ready` になるのを最大 20 秒待つ（100 ms ごとに読み直す）。H2 が先に終了した、または期限が切れたら: H2 がまだ動いていれば終了させ（プロセスのハンドルがある）、フォルダーを消し、`Run` を消して、`Refused(HandOffFailed { detail })`（H2 の終了コードを含む）。
18. `Update::HandedOff { run_id, to_version }` を送る。
19. ロックを放し（`FileLock` を落とす）、パイプを閉じて、終了コード 0 で終わる。

- 12 と 13 の途中で呼び出し元が去った（`Bye`、パイプの切断）、または `Refused` になった場合: ファイルとフォルダーを消し、`Run` を消す。`LastResult` は書かない（利用者は GUI でその場で結果を見ているか、取り消した）。
- H1 がロックを持つのは 4 から 19 まで（ふつう数秒）。その間、ほかの書き手は `Busy` になる。

### D.5 `Updates` フォルダー

```
%ProgramData%\SHIN DATA CENTER\MKLM\Updates\          PRIVATE_DIR_SDDL（SYSTEM と Administrators だけ）
  0.2.1-3f9a0c2b7d1e4a65\                             実行 ID。PRIVATE_DIR_SDDL を明示して作る
    latest.json                                        受け取ったバイト列のまま
    latest.json.minisig                                同上
    MKLM-Setup-0.2.1-x64.exe                           SHA-256 を確かめたインストーラー
    mklm-helper.exe                                    H1 のコピー（= H2）
```

- 使う前に毎回、`SHIN DATA CENTER` から `Updates` までの各階層と実行のフォルダーについて、所有者、保護された DACL、ほかの SID に書き込み系の権利がないこと、リパースポイントでないことを確かめ、ハンドルで固定する（m2 D.9、`protected_dir`）。先回りして作られた階層は隔離して作り直す（m2 S1）。
- Users には読ませない。読めると、利用者がファイルを共有なしで開いたままにして、H2 の検証や後片付けを止められるため（m2 I.18 と同じ種類の妨害）。そのため、GUI が読む結果はレジストリに置く（D.6）。
- ファイルは `CREATE_NEW` で作り、書く間は共有なし、H2 が検証して実行する間は `FILE_SHARE_READ` だけ（書き込みと削除の共有なし）で開いておく（`RunDir::open_locked`）。

### D.6 HKLM の `Update` キー

```
HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update      JOURNAL_KEY_SDDL（SYSTEM と Administrators がフル、Users が読み取り）
  Trust        REG_SZ  TrustState の JSON（C.4）
  Run          REG_SZ  RunRecord の JSON（進行中の更新。終われば消す）
  LastResult   REG_SZ  UpdateResult の JSON（最後に終わった更新の結果）
```

- 作るのと書くのは helper だけ（`mklm_win::update_store`）。ジャーナルと同じ方法（`open_or_create_product_subkey`、`SHIN DATA CENTER` から `Update` までの各キーの所有者と DACL の確認、書いたら `RegFlushKey`）。値の名前は `UPDATE_VALUE_NAMES` の 3 つだけ（`regwrite` の S9 と同じ関所）。
- 読むのは誰でもよい（`read_update_store`。非昇格の GUI と CLI も）。
- **計画と依頼の `Updates\last-result.json` をここに移した理由**（K.3）
  1. `Updates` は SYSTEM と Administrators だけのフォルダー（m2 G.1）で、非昇格の GUI は読めない。
  2. Users が読めるファイルにすると、読み手が開いたままにするだけで書き換えを止められる（m2 I.18）。レジストリの値の書き込みは 1 回の `RegSetValueExW` で原子的で、読み手に止められない。
  3. 標準ユーザーは `HKLM\SOFTWARE` の下にキーを作れないので、先回りの心配がない（m2 C.1）。
  4. ジャーナルの仕組み（DACL の確認、フラッシュ）をそのまま使える。
- アンインストールしても残す（ジャーナルと同じ。M5a のアンインストーラーは `HKLM\SOFTWARE\SHIN DATA CENTER` に触れない）。
- JSON の形は H.5。

### D.7 H2: `--run-update <run-id>`

**コマンドライン**（`mklm_ipc::RunUpdateArgs`。m2 E.4 と同じく、`GetCommandLineW` の生の文字列から `command_line_tail` でプログラム名と空白 1 つを除き、残りを手書きの厳密なパーサーで確かめる）

```
mklm-helper.exe --run-update <run-id>
```

`RUN_UPDATE_PATTERN`（先頭から末尾まで一致すること）:

```
^--run-update [0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}-[0-9a-f]{16}$
```

加えて、各数は「`0` か、`0` で始まらない」かつ 65535 以下（PID の範囲の確認と同じく、正規表現の外の規則）。テストは m2 と同じ差分テスト（1 文字の挿入、置換、削除で、パーサーとパターンが同じ結論になること）。

**手順**

1. `restrict_dll_search()`（失敗は致命的）、`SetProcessShutdownParameters(0x100, SHUTDOWN_NORETRY)`、作業フォルダーを System32 に、コマンドラインの検証（`exit 2`）、昇格の確認（`exit 4`）、OS の版（`exit 5`）。
2. 自分の実行ファイルのパス（NT 形式）が `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>\mklm-helper.exe` であること。`verify_protected_dir(Updates)` と `RunDir::open(<run-id>)` で各階層を確かめて固定する。違えば `exit 7`（記録は書かない）。
3. `Run` を読む: あって、`run_id` が一致し、`phase == staged` で、`runner` がまだないこと。違えば `exit 7`（記録は書かない。誰が起動したか分からないため）。
4. **検証し直す**: `latest.json` と `.minisig` を `open_locked` で読み（大きさの上限つき）、機械の記録と埋め込みの鍵で `verify_manifest`（目的 `Install`、入っている版 = 自分の `CARGO_PKG_VERSION`、アーキテクチャ = 自分のビルド）。版が `Run.to_version` と一致すること。
5. インストーラーを `open_locked`（`FILE_SHARE_READ` だけ、通常のファイルでリパースポイントでないこと）で開き、大きさと SHA-256 を確かめる。**このハンドルはインストーラーのプロセスを作るまで閉じない**（書き込み、名前の変更、削除を止めておく。`CreateProcessW` は読み取りと実行の共有でイメージを開くので、このハンドルと両立する — 未確認、F.6 で確かめる）。
6. `Run.phase = ready`、`runner = 自分`。（H1 はこれを見て GUI に引き継ぎを知らせ、終了する。）
7. ここから後の失敗はすべて、`LastResult` を書き、`Run` を消し、GUI を起動し直し（D.10）、後片付けをして `exit 7` で終わる。
8. H1（`Run.stager`）の終了を最大 30 秒待つ（`proc_identity::wait_for_exit`）。
9. `FileLock::acquire(60 秒)`（`NotInstalled(Refused(Busy))`）。
10. ジャーナルを読み直し、`gate::check_journal`（`NotInstalled(Refused(…))`）。
11. インストール先の版が変わっていないこと: `$INSTDIR\mklm-helper.exe` の VERSIONINFO のビルド ID が、自分のビルド ID と同じ（`InstalledVersionChanged`）。引き継ぎの間に誰かが手で別の版を入れた場合に、古い helper が新しいものを上書きして戻すのを防ぐ。
12. `Run.phase = waiting`。ほかの MKLM を終わらせる（D.8）。
13. `Run.phase = installing`（書いてフラッシュしてから起動する）。
14. `CreateProcessW("<run dir>\MKLM-Setup-<v>-<arch>.exe", "\"<path>\" /S")`。失敗は `NotInstalled(InstallerNotStarted { code })`（225 / 226 はウイルス対策ソフトによる停止）。成功したら `Run.installer` を書き、手順 5 のハンドルを閉じる。
15. 終了を最大 15 分待つ。期限を過ぎたら `Failed(InstallerTimedOut)`。**インストーラーは止めない**（ファイルの置き換えの途中で止めると、必ず半端になるため）。この場合は GUI を起動し直さない（インストーラーがまだ `mklm.exe` を置き換えているかもしれないため）。
16. `Run.phase = finishing`。インストール先の 3 つの exe のビルド ID を読み（`InstallState`）、`run::decide_outcome` で結果を決める（D.13）。
17. `LastResult` を書き、`Run` を消し、ロックを放す。
18. GUI を起動し直す（D.10。15 の期限切れを除く）。
19. 後片付け（D.11）。結果が `Installed` なら `exit 0`、それ以外は `exit 7`。

- H2 は `%ProgramData%\SHIN DATA CENTER\MKLM\logs\update.log`（`DataDir::Logs`、SYSTEM と Administrators だけ）に英語で段階と結果を追記する（1 MB で 1 世代を回す）。H1 も同じファイルに書く。キーの内容は書かない（そもそも扱わない）。

### D.8 ほかの MKLM の終了を待つ

インストーラーは、`$INSTDIR` の `mklm-helper.exe`、`mklm-cli.exe`、`mklm.exe` のどれかが動いていると拒否する（0.1）。H2 は先に、次の順で終わらせる。

1. **更新を始めた GUI**（`Run.caller`）は、`HandedOff` を受けて自分から終了する（m3 F.5 の A12）。H2 は最大 30 秒待つ。
2. **すべてのセッションの GUI**: `mklm_win::instance::quit_all_instances($INSTDIR\mklm.exe の NT パス, 5 秒)`
   - `\\.\pipe\` を列挙し、`SHINDATACENTER.MKLM.Instance.` で始まるパイプに接続する（多重起動のパイプの DACL は Administrators に許している。m3 F.1）。
   - サーバーの PID（`GetNamedPipeServerProcessId`）の実行ファイル（`process_image_nt_path`。`OpenProcess` を使わないので、ほかの利用者のプロセスでも読める。m2 S3）が `$INSTDIR\mklm.exe` であるときだけ `quit\n` を送る。違えば何も送らない。
   - 返事が `busy`（そのセッションで、helper との作業、たとえばカウントダウンが進行中）なら、更新をやめる（`NotInstalled(InstanceBusy)`）。キーボードの操作の途中で MKLM を終わらせない。
   - `ok` なら終わるのを待つ。ほかの利用者の GUI は、その利用者が次にサインインしたとき、Run キーの自動起動で新しい版として戻り、そのとき結果を表示する（E.5）。H2 はほかの利用者のセッションに GUI を起動しない（その利用者のトークンを持たないため）。
3. **すべてのプロセス**: 実行ファイルが `$INSTDIR` の `mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe` のどれかであるプロセスを `proc_identity::processes_with_images` で探し、なくなるまで待つ。GUI と CLI は 30 秒、helper は最大 75 秒（何もしていない helper は 60 秒で終わる。m2 E.7）。残れば `NotInstalled(ProgramsStillRunning { programs })`。
4. インストーラーの `CloseMklm` が、念のためにもう一度確かめる（0.1）。ここで拒否された場合は D.9 の終了コードで分かる。

### D.9 NSIS

#### D.9.1 終了コード

`mklm.nsi` のすべての拒否と失敗で、`Quit` の**直前**に `SetErrorLevel` を置く（NSIS のフォーラムの経験則: 直前でないと NSIS 自身の値で上書きされることがある）。`.onInit` の `Abort` も `SetErrorLevel` と `Quit` に置き換えて、形をそろえる。

| コード | `!define` | 場面 | 何か置き換えたか |
|---|---|---|---|
| 0 | — | 成功 | はい |
| 1 | —（NSIS の既定） | 利用者が取り消した（`/S` では起きない） | いいえ |
| 2 | —（NSIS の既定） | スクリプトによる中止（`File` の書き込みの失敗など、下の表にないもの） | 分からない（D.13 で調べる） |
| 20 | `MKLM_EXIT_OS_TOO_OLD` | Windows 11 24H2（build 26100）未満 | いいえ |
| 21 | `MKLM_EXIT_WRONG_ARCH` | ARM64 のインストーラーを ARM64 以外で、または 32 ビットの Windows | いいえ |
| 22 | `MKLM_EXIT_HELPER_RUNNING` | `$INSTDIR\mklm-helper.exe` が動いている | いいえ |
| 23 | `MKLM_EXIT_CLI_RUNNING` | `$INSTDIR\mklm-cli.exe` が動いている | いいえ |
| 24 | `MKLM_EXIT_GUI_RUNNING` | `--quit` の後も `mklm.exe` が残った（`/S` では「キャンセル」が既定） | いいえ |
| 25 | `MKLM_EXIT_BAD_INSTALL_DIR` | アンインストーラーが想定の場所にない（アンインストーラーの終了コードは呼び出し元に届かない。NSIS の付録 D） | いいえ |
| 3010 | — | アンインストールの復元で PC の再起動が必要（既存） | — |

- 同じ値を `mklm_update::run::nsis_exit` の定数に持つ。WP-H のテスト（`crates/mklm-update/tests/nsis_exit_codes.rs`）が `mklm.nsi` を読んで、`!define MKLM_EXIT_*` の値が定数と一致することを確かめる。
- `Quit` を `SetErrorLevel` なしで呼んだときの終了コードは NSIS のドキュメントに書かれていない（未確認。2 だと思われる）。この設計はそれに頼らない。

#### D.9.2 CI での確認（WP-H）

- 静的な検査（`installer/check-nsi.ps1`、ci.yml）: すべての `MessageBox` に `/SD` があること（サイレントで止まらないため）。すべての `Quit` と `Abort` の直前が `SetErrorLevel` であること（`.onInit` のページ用の `Abort` は使っていない）。`uninstall.exe` と `--uninstall-restore` を実行する行が `un.` の関数とアンインストールのセクションにしかないこと。
- 煙の試験（release.yml、x64 のランナー。ランナーは使い捨ての VM で、管理者として動く）: できたインストーラーで次を確かめる。(1) `/S` で新規インストール → 0、ファイルがそろう。(2) もう一度 `/S` → 0（上書き）。(3) PowerShell で `$INSTDIR\mklm-cli.exe` を共有なしで開いたまま `/S` → 23。(4) ARM64 のインストーラーを `/S` → 21。(5) `uninstall.exe /S` を実行し、終わるのを待ってファイルが消えたことを確かめる。失敗すれば下書きのリリースを作らない。

#### D.9.3 H2 からの実行

- `"<run dir>\MKLM-Setup-<v>-<arch>.exe" /S`。`/D=` は渡さない（`.onInit` がインストール先を固定する）。
- H2 は昇格しているので、インストーラーの `RequestExecutionLevel admin` による UAC は出ない。
- ダウンロードではなく H1 が書いたファイルなので、Mark of the Web（`Zone.Identifier`）がなく、SmartScreen の確認は出ない。スマート アプリ コントロールが有効な PC では、署名のないインストーラーは止められる（MKLM 自体がそこでは使えない。計画 5 章）。
- NSIS は利用者（H2 のアカウント）の `%TEMP%` にプラグインを展開する。NSIS 3.11 で直った CVE-2025-43715 の対策として、3.11 以上を使う（build-installer.ps1 が確かめている）。

#### D.9.4 サイレントの上書きがキーボードの設定に触れないこと

`mklm.nsi` のインストールの経路（`.onInit`、`Section "MKLM"`、`CloseMklm`）が実行するのは、`"$INSTDIR\mklm.exe" --quit`（多重起動のパイプに `quit` を送るだけ。M3 の GUI は `--quit` で窓を開かず、HKLM にも書かない）だけ。アンインストーラー（`uninstall.exe`）と `mklm-helper.exe --uninstall-restore` を実行するのはアンインストールのセクションだけで、上書きでは実行されない（NSIS は古い版のアンインストーラーを呼ばず、`WriteUninstaller` で書き直すだけ）。インストーラーは `HKLM\SOFTWARE\SHIN DATA CENTER` とキーボードの値に何も書かない。D.9.2 の静的な検査がこれを固定する。`/S` では完了ページがないので、`LaunchUnelevated`（完了ページの「MKLM を起動する」）も動かない（GUI の起動し直しは H2 が行う。D.10）。

### D.10 GUI を非昇格で起動し直す

- H2 は `mklm_win::shell_launch::launch_via_shell("$INSTDIR\mklm.exe", "--after-update", "$INSTDIR", 10 秒)` を呼ぶ。中身は計画 4.2 の手順 8 の方法: `CoCreateInstance(CLSID_ShellWindows)` → `IShellWindows::FindWindowSW(SWC_DESKTOP, SWFO_NEEDDISPATCH)` → `IServiceProvider::QueryService(SID_STopLevelBrowser)` → `IShellBrowser::QueryActiveShellView` → `IShellView::GetItemObject(SVGIO_BACKGROUND)` → `IShellFolderViewDual::get_Application` → `IShellDispatch2::ShellExecute`。デスクトップのシェル（Explorer）が起動するので、GUI はそのセッションのサインインしている利用者として、**非昇格で**動く。
- COM の呼び出しは別のスレッドで行い、10 秒で見切る（Explorer が固まっていても H2 が止まらないように）。
- **H2 自身のトークンでは決して起動しない**（`CreateProcessW` や `explorer.exe <path>` にも頼らない。後者は H2 のアカウントで Explorer を起動しうるため）。
- **別の管理者の資格情報で昇格した場合**（標準ユーザー A が UAC で管理者 B の資格情報を入れた。m2 S3）: H1 と H2 は B として動く。B のプロセスから A の Explorer の `ShellWindows` に届くかは未確認。届けば A として起動し、届かなければ失敗する。どちらでも B として GUI を起動することはない。失敗しても、次の 3 つで GUI は戻る。
  1. 更新を始めた GUI は、終了する前に自分の HKCU の RunOnce に `SHINDATACENTER.MKLM.AfterUpdate = "<INSTDIR>\mklm.exe" --after-update` を登録する（計画 4.2 の手順 8 の予備。昇格している GUI は登録しない。m3 F.2）。次のサインインで結果が前面に出る。
  2. Run キーの自動起動（既定でオン）。
  3. 利用者がスタート メニューから開く。GUI は引き継ぎの前に「開かない場合はスタート メニューから開いてください」と伝えている（E.4）。
- 起動し直した GUI は、RunOnce の値を消す（`unregister_after_update`。同じ利用者なので消せる）。
- 起動し直したかどうかは `LastResult.gui_relaunch_attempted`（試みたか）に残す。GUI が `LastResult` を読むより先に起動しないよう、`LastResult` を書いてから起動する（D.7 の 17、18）。

### D.11 後片付け

- H2 は最後に、自分の実行のフォルダーのインストーラーと 2 つの更新情報を消す。自分の実行ファイルは動いているので消せない。`MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` で、自分の実行ファイルとフォルダーを次の再起動で消すよう予約する。
- H1（D.4 の 7）と H2 は、ほかの古い実行のフォルダー（今の `Run` のもの以外で、中の実行ファイルが動いていないもの）を消す。消すときはリパースポイントをたどらず（`FILE_FLAG_OPEN_REPARSE_POINT`）、通常のファイルだけを消す（`remove_run_dir`）。
- 利用者のキャッシュ（`%LOCALAPPDATA%\…\update\`）のインストーラーは、GUI が更新の後の起動で消す（`UpdateCache::prune`）。
- アンインストールは `Updates` を消さない（最大でも helper のコピー 1 つが再起動まで残るだけで、`Updates` は管理者しか書けない。ジャーナルと同じく残す）。

### D.12 段階ごとの失敗、期限切れ、再起動、サインアウト

「記録」は `Run` と `LastResult`（D.6）。「中断」は、`Run` が残っていて、持ち主のプロセスがいないか起動 ID が変わった状態（`run::classify_run`）。

| 段階（`Run.phase`） | 動いているもの | 失敗・期限切れ | 再起動・電源断・サインアウト | 何が変わったか | 次の GUI の表示 |
|---|---|---|---|---|---|
| 確認とダウンロード（記録なし） | GUI | ページに理由（E.6）。次の定時の確認でやり直す | 途中の `.part` を次の起動で消す | 何も | 何もなし |
| UAC の待ち（記録なし） | GUI | UAC を断れば「取り消しました」 | — | 何も | — |
| H1 の検証（記録なし） | GUI、H1 | `Refused`。GUI はそのまま | — | 何も（機械の記録は 8 で進むことがある） | — |
| `staging` | GUI、H1 | 呼び出し元が去った、ハッシュ違い、期限切れ → フォルダーと `Run` を消す | `Run` が残る → 中断 | 何も | 「前回の更新の準備は中断されました。何も変更されていません」（情報）。次の H1 が片付ける |
| `staged` | H1、H2 の起動中 | H2 が 20 秒で `ready` にならない → H1 が H2 を止め、フォルダーと `Run` を消し、`Refused(HandOffFailed)`。GUI はそのまま | 中断 | 何も | 同上 |
| `ready` / `waiting` | H2（GUI は終了中か終了済み） | ロック、ジャーナル、版の変化、ほかの MKLM → `LastResult = NotInstalled`、GUI を起動し直す | 中断。サインアウトでは H2 もセッションと一緒に終わる | 何も | `NotInstalled` の理由と次の手順（E.6）。中断なら「更新は中断されました。何も変更されていません」 |
| `installing` | H2、インストーラー | 終了コード → D.13 の判定。15 分を過ぎたら `Failed(InstallerTimedOut)`（止めない） | 中断。NSIS はファイルを 1 つずつ置き換えるので、半端になりうる | 分からない → ファイルの版で調べる（D.13） | D.13 の表 |
| `finishing` | H2 | — | 中断（ほぼ終わっている） | 置き換え済みのことが多い | D.13 の表 |
| 記録の後（`Run` なし、`LastResult` あり） | H2（起動し直しと後片付け） | 起動し直しの失敗は RunOnce と Run キーが補う（D.10） | 影響なし（片付けは次回） | — | `LastResult` を 1 回表示 |

- サインアウトの間にインストーラーが止められることを防ぐため、`installing` の間だけセッションの終了を止める（`ShutdownBlockReasonCreate`）案は採らない。インストールはふつう数秒で、止められた場合は D.13 で検出して案内できる（I.7）。

### D.13 途中で止まったインストールの検出と、次の GUI の表示

**インストールの状態**（`run::InstallState`）: インストール先の `mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe` の VERSIONINFO のビルド ID（`elevation::file_build_id`。非昇格でも読める）。3 つがあってすべて同じなら、その版（ビルド ID の `+` の前）が「そろった版」（`consistent_version`）。1 つでも欠けるか違えば「そろっていない」。

**結果の判定**（`run::decide_outcome`。H2 が使う。中断の場合は GUI と次の H1 が `interrupted_result` で同じ規則を使う）

| 条件 | 結果 |
|---|---|
| 15 分を過ぎた | `Failed(InstallerTimedOut)` |
| そろった版 = 新しい版 | `Installed`（終了コードが 0 でなくても。コードは `installer_exit` に残す） |
| そろった版 = 元の版、コードが 20〜25 | `NotInstalled(InstallerRefused { exit })` |
| そろった版 = 元の版、そのほかのコード | `NotInstalled(InstallerExit { code })` |
| そろった版がそれ以外 | `Failed(UnexpectedVersion { found })` |
| そろっていない | `Failed(Inconsistent)` |

**GUI の起動時**（どの起動方法でも。`--tray`、`--after-update`、RunOnce、手で）

1. `update_store` を読み、`classify_run` で `Run` を分類する。
   - `InProgress`（`ready` 以降で持ち主が生きている）: 更新の途中なので、**窓を出さず、インスタンスにもならずに、すぐ終了する**（ログに残す）。窓を出すと `mklm.exe` がロックされ、インストーラーを止めてしまうため。数十秒後に H2 が GUI を起動し直す。`staging` / `staged` の間（まだ GUI が動いているはずの段階）は、ふつうに起動する。
   - `Interrupted`: 「中断」の結果を表示する（下の表）。
2. `LastResult` の `run_id` が `settings.update.result_seen` と違えば、結果を 1 回表示し、`result_seen` を書く。ほかの利用者の GUI にも 1 回だけ出る（その利用者に「MKLM が更新された」ことが伝わる）。
3. インストールの状態がそろっていなければ、`LastResult` によらず「MKLM のファイルの版がそろっていません」を出す（m3 の helper のビルド ID の確認より先に、原因を示すため）。

| 中断した段階 | インストールの状態 | 表示 |
|---|---|---|
| `staging`〜`waiting` | そろった版 = 元の版 | 「更新は中断されました。何も変更されていません（MKLM は 0.2.0 のままです）。」［今すぐ更新］ |
| `installing` / `finishing` | そろった版 = 新しい版 | 「MKLM は 0.2.1 に更新されました（終わる直前に PC が再起動したか、サインアウトしました）。」 |
| `installing` / `finishing` | そろった版 = 元の版 | 「更新は中断されました。MKLM は 0.2.0 のままです。」［今すぐ更新］ |
| どれでも | そろっていない | 「更新が途中で止まったため、MKLM のファイルの版がそろっていません。キーボードの設定はそのままです。GitHub のリリース ページから MKLM-Setup-0.2.1-x64.exe をダウンロードして実行してください。」［リリース ページを開く］ |

- そろっていない状態では helper を起動できない（ビルド ID が合わない。m2 E.3）ので、自動更新でも直せない。インストーラーを手で実行すれば直る（NSIS の上書きはファイルを全部書き直す）。`docs/recovery.md` に「更新が途中で止まったとき」を足す（WP-H）。
- `Run` の中断の記録を `LastResult` に移して消すのは、次の H1（D.4 の 6）。GUI は HKLM に書けないので、表示だけを `Run` から作る。

### D.14 CLI（`mklm-cli update`）

| コマンド | 動作 | 終了コード |
|---|---|---|
| `mklm-cli update --check [--json]` | GUI と同じ確認（`mklm_client::update::check`）。利用者の記録を更新する。ダウンロードもインストールもしない。「MKLM 0.2.1 is available (installed 0.2.0). Open MKLM to install it.」または「MKLM is up to date (0.2.0).」。期限切れなら警告を 1 行 | 0: 確認できた（最新か、更新あり）。1: 確認できなかった。2: 使い方の誤り |
| `mklm-cli update --status [--json]` | 機械の記録（`LastResult`、進行中の `Run`、インストールの状態）を表示する | 0 / 1 / 2 |

- **CLI はインストールしない**（J 章の質問 4）。理由: `mklm-cli.exe` 自身が `$INSTDIR` にあり、インストールの前に終わらなければならない。結果を表示できるのは次に起動した GUI か `update --status` になり、CLI の利点（その場で結果と終了コードが分かる）がない。GUI の引き継ぎと起動し直しをもう 1 組作る価値は小さい。スクリプトや管理ツールからは、インストーラーを直接 `/S` で実行すればよい（終了コードは D.9.1。`docs/install-guide.ja.md` に書く）。
- 出力は英語（M1、M2 と同じ）。`--json` の形は `{"installed":"0.2.0","status":"update-available"|"up-to-date"|"manual-required","offered":"0.2.1","freshness":"fresh"|"expired","expires":1826582400,"release_page":"https://…"}`（`--check`）。

### D.15 helper の終了コード（m2 E.8 への追加）

| コード | 意味 | 使う場面 |
|---|---|---|
| 0 | 正常（パイプのセッション）。`--run-update` では「インストールした」 | 既存 + H2 |
| 1 | その他の失敗 | 既存 |
| 2 | コマンドラインが固定の形でない | 既存 + H2 |
| 3 | 接続かハンドシェイクの失敗 | 既存 |
| 4 | 昇格していない | 既存 + H2 |
| 5 | 対応していない OS | 既存 + H2 |
| 6 | （`--uninstall-restore`）ロック中 | 既存 |
| 7 | （`--run-update`）インストールしなかった、または失敗した。理由は `LastResult`。`ready` の前なら記録なし | 新規 |
| 3010 | （`--uninstall-restore`）再起動が必要 | 既存 |

H1 は、H2 が `ready` の前に終了したとき、その終了コードを `HandOffFailed` の `detail` に入れる。

---

## E. GUI

### E.1 設定、確認の時刻、ワーカー

**設定**（`settings.toml` の `[update]`。すべて `#[serde(default)]`）

| 項目 | 既定 | 意味 |
|---|---|---|
| `auto_check` | `true` | 「更新を自動で確認する」。オフなら、自動ではネットワークに一切出ない（［今すぐ確認］は使える） |
| `skipped_version` | なし | ［この版をスキップ］の版 |
| `result_seen` | なし | 表示済みの `LastResult` の `run_id` |

**利用者のキャッシュ**（`%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\`。ローミングしない場所）

| ファイル | 中身 |
|---|---|
| `state.json` | `ClientState`: 利用者の記録（`TrustState`）、最後の確認の時刻、最後に成功した確認の時刻 |
| `latest.json`、`latest.json.minisig` | 最後に検証に通った更新情報（受け取ったバイト列のまま） |
| `MKLM-Setup-<v>-<arch>.exe` | 検証済みのインストーラー（1 つだけ残す） |
| `MKLM-Setup-<v>-<arch>.exe.part` | ダウンロードの途中（起動時に消す。続きからの再開はしない） |

- 書くときは一時ファイルに書いてから名前を変える（m3 F.4 と同じ）。

**確認の時刻**

- 起動の後: 前回の確認から 24 時間以上たっていれば、60 秒 + 0〜120 秒の乱数の後。そうでなければ前回 + 24 時間 + 0〜60 分の乱数。サインイン直後の負荷を避け、利用者の間で時刻を散らすため。
- その後: 24 時間 + 0〜60 分の乱数ごと。
- スリープからの復帰（`ShellEvent::Resumed`）で予定を過ぎていれば、30 秒後。
- ［今すぐ確認］は予定によらずすぐ。
- 乱数は `mklm_win::session::random_bytes`。時刻の管理は UI スレッドの `slint::Timer`。

**スレッド**（m3 A.4 に足す）

| スレッド | 数 | やること | UI への戻り方 |
|---|---|---|---|
| 更新のワーカー（`mklm-update`） | 1（常駐） | `UpdateTask::{Check { manual }, Download, Cancel}`: `mklm_client::update::{check, download}`、キャッシュの読み書き、機械の記録の読み取り | `invoke_from_event_loop` で `AppMsg::Update*` |
| セッション ワーカー（既存） | 同時に 1 | 更新のセッション: `launch::start`（UAC）→ `update::stage` | 既存の形。セッション ID 付き |

- 確認とダウンロードは I/O ワーカーに載せない（遅いネットワークで一覧の読み直しを待たせないため）。

**流れ**

- 確認 → `Available` なら、スキップしていなければ自動でダウンロード → `Ready`（ボタンが押せる）。
- 新しい版が出れば、古いダウンロードを捨てて取り直す。
- 従量制の接続でもダウンロードする（数 MB のため。J 章の質問 7）。

### E.2 状態と、更新のページ

新しいページ `update`（m3 B.0 のページに 1 つ足す。左のナビゲーションには足さず、メイン画面のバナー、トレイのメニュー、設定のページの「更新」欄から開く）。

| 状態 | 表示 | ボタン |
|---|---|---|
| 使えない（`NotConfigured`、`NotInstalledCopy`） | 理由（E.6） | ［今すぐ確認］（`NotInstalledCopy` のときだけ。確認だけ） |
| 確認していない / 確認中 | 「確認しています…」 | ［キャンセル］ |
| 最新 | 「MKLM は最新です（0.2.0）。最後の確認: 2026/10/16 09:12」。期限切れならその文（B.4 の 5） | ［今すぐ確認］ |
| 更新あり・ダウンロード中 | 新しい版、公開日、［リリースノートを開く］、進捗（MB と %） | ［キャンセル］ |
| 更新あり・準備完了 | 下の図 | ［この版をスキップ］［後で］［今すぐ更新（次に Windows の確認が出ます）］ |
| 手で更新が必要（`ManualRequired`） | 「この版からは自動で更新できません…」 | ［リリース ページを開く］ |
| 更新中（セッション） | 「管理者の確認を待っています…」→「更新を準備しています…（送信 45 %）」 | ［キャンセル］（全部送り終えるまで） |
| 失敗 | 理由と次の手順（E.6） | 状況に応じて［もう一度確認］［もう一度ダウンロード］［リリース ページを開く］［詳細をコピー］ |

```
┌ MKLM の更新 ────────────────────────────────────────────────────────┐
│ 新しい版があります: 0.2.1（今の版: 0.2.0）                              │
│ 公開: 2026/10/15    ［リリースノートを開く］                            │
│ ダウンロード: 完了（6.0 MB。内容を確かめました）                        │
│                                                                         │
│ ［今すぐ更新］を押すと、Windows が管理者の許可を求めます。発行元は      │
│ 「不明」、プログラム名は mklm-helper.exe と表示されます。許可すると     │
│ MKLM はいったん終了し、更新が終わると自動で開きます。キーボードの       │
│ 設定は変わりません。                                                    │
│      ［この版をスキップ］［後で］［今すぐ更新（次に Windows の確認が出ます）］│
└─────────────────────────────────────────────────────────────────────────┘
```

- ［後で］: バナーを次の確認（24 時間後）か次の起動まで隠す。
- ［この版をスキップ］: `skipped_version` に書き、バナーを消す。
- 設定のページに「更新」欄: 「☑ 更新を自動で確認する（1 日に 1 回、GitHub に問い合わせます）」、最後の確認の時刻と結果、［今すぐ確認］、［更新のページを開く］。
- 初回のウィザードの「ようこそ」に 1 行: 「MKLM は 1 日に 1 回、GitHub で新しい版を確かめます（設定でオフにできます）。」

### E.3 バナーとトレイ

- メイン画面のバナー（情報）: 「MKLM の新しい版（0.2.1）を使えます。［詳細…］」。準備完了（ダウンロード済み）のときだけ出す。スキップした版には出さない。
- トレイ: ツールチップに「MKLM — 新しい版（0.2.1）があります」。メニューに「MKLM を更新…」（準備完了のときだけ有効。更新のページを開く）。
- Windows の通知（トースト）は使わない（m3 B.16 の方針）。
- 期限切れはバナーとトレイに出さない（B.4 の 5）。

### E.4 UAC の事前説明と引き継ぎ

- ［今すぐ更新］で `settings.change.uac_notice_seen` が false なら、m3 B.5 の UAC の説明の画面（`UacNoticeScreen`）を先に出す。true なら、ページの説明の文（E.2 の図）とボタンの文で足りる。昇格した GUI では UAC が出ないので、どちらも出さず、ボタンは「今すぐ更新」。
- 利用者がボタンを押したときだけ UAC を出す。自動の確認とダウンロードは UAC を出さない。
- セッションの段階は m3 B.18 と同じ（`Launching` → `Running`）。UAC を断れば「取り消しました（何も変更していません）」。
- `HandedOff` を受けたら:
  1. RunOnce（`--after-update`）を登録する（昇格した GUI は登録しない。D.10）。
  2. オーバーレイ「MKLM を更新しています。まもなく MKLM が終了し、更新が終わると自動で開きます。開かない場合は、スタート メニューから MKLM を開いてください。」を 1 秒出す（読み上げは assertive）。
  3. 通常の終了の経路で終了する（トレイを消す、I/O の保存を最大 1 秒待つ。m3 F.5）。セッションは終わっているので、`quit` が待たされることはない。

### E.5 更新の後の表示

- `--after-update`（H2 か RunOnce から）: 窓を表示して前面に出し、`LastResult`（未表示なら）か中断の結果を、結果のオーバーレイで出す（m3 B.17 の形）。`--after-update` がなくても、未表示の結果があれば 1 回出す（D.13）。
- 結果の文は E.6。`Installed` では「MKLM を 0.2.1 に更新しました。」と［リリースノートを開く］。
- ほかの利用者の GUI（その利用者の次のサインインで起動）も同じ結果を 1 回出す。
- 更新の後の最初の起動で、利用者のキャッシュのインストーラーを消す。

### E.6 文言（エラーと次の手順）

Rust が組み立てる文は `i18n.rs` に、型ごとの網羅的な `match` で日英を持つ（m3 D.4）。`UpdateRefusal`、`FetchError`、`TransportError`、`CheckError`、`DownloadError`、`StageEnd`、`UpdateOutcome`、`NotInstalledReason`、`FailedReason`、`RunPhase`、`InstallerExit`、`Availability`、`Freshness` のすべての列挙子に文を持つ。英語の診断は「技術的な詳細」と［詳細をコピー］に入れる。主なもの（日本語 UI）:

| 状況 | 文 | 次の操作 |
|---|---|---|
| 名前解決、接続、期限切れ | 「更新を確認できませんでした。インターネットにつながっているか確かめてください。会社や学校のネットワークでは、プロキシの設定が必要なことがあります。」 | ［もう一度確認］ |
| `ProxyAuthRequired` | 「プロキシの認証が必要なため、更新を確認できませんでした。Windows のプロキシの設定を確かめてください。」 | ［もう一度確認］ |
| `Tls` | 「GitHub と安全に接続できませんでした。PC の日付と時刻が正しいか確かめてください。」 | ［もう一度確認］ |
| `NotFound` | 「更新情報が見つかりませんでした。しばらくしてからもう一度確かめてください。」 | ［もう一度確認］ |
| `RateLimited`、そのほかの HTTP の状態 | 「GitHub が一時的に応答しませんでした。しばらくしてからもう一度確かめてください。」 | ［もう一度確認］ |
| 署名の関係（`SignatureMalformed`、`WrongTrustedComment`、`UnknownKey`、`BadSignature`、`RevokedKey`、`KeyIdMismatch`、`IllegalRevocation`） | 「更新情報の署名を確かめられませんでした。安全のため、この更新は使いません。GitHub のリリース ページで最新の案内を確かめてください。」 | ［リリース ページを開く］ |
| 形式の関係（`ManifestMalformed`、`UnsupportedSchema`、`WrongProduct`、`WrongChannel`、`BadVersion`、`TagMismatch`、`BadTimestamps`、`AssetMalformed`、`NoAssetForArch`、大きさの超過） | 「更新情報の形式が、この版の MKLM と合いません。GitHub のリリース ページから新しい版を入れてください。」 | ［リリース ページを開く］ |
| `Rollback` | 手動の確認のときだけ「以前より古い更新情報が届いたため、無視しました。」。自動の確認では表示しない | — |
| `ManualRequired` | 「この版からは自動で更新できません。リリース ページからインストーラーをダウンロードして実行してください。」 | ［リリース ページを開く］ |
| `NotConfigured` | 「この MKLM には更新を確かめるための鍵が入っていないため、自動更新は使えません。」 | — |
| `NotInstalledCopy` | 「この MKLM は <パス> から動いているため、自動更新は使えません（インストールした MKLM だけが更新できます）。」 | ［今すぐ確認］（確認だけ） |
| ダウンロードの `SizeMismatch`、`HashMismatch`、`SourceChanged` | 「ダウンロードしたファイルが更新情報と一致しませんでした。もう一度ダウンロードします。」 | ［もう一度ダウンロード］ |
| キャッシュに書けない | 「ダウンロードしたファイルを保存できませんでした。ディスクの空きを確かめてください。」 | ［もう一度ダウンロード］ |
| `OperationOpen { waiting_for_reboot: false }`、`RecoveryNeeded` | 「確認待ちの変更があるため、今は更新できません。［確認…］で変更を決めてから更新してください。」 | ［確認…］ |
| `OperationOpen { waiting_for_reboot: true }` | 「PC の再起動を待っている変更があるため、今は更新できません。PC を再起動し（シャットダウンではなく再起動）、確認を終えてから更新してください。」 | ［再起動…］ |
| `Busy`、`UpdateInProgress` | 「別の MKLM が処理中です。しばらくしてからもう一度試してください。」 | — |
| UAC を断った | 「取り消しました（何も変更していません）。」 | — |
| `HandOffFailed`、`Storage`、`Internal`、`JournalUnreadable`、`CallerLeft`、`Lost`、`Unresponsive`、`Protocol` | 「更新の準備に失敗しました。何も変更していません。PC を再起動してからもう一度試してください。直らない場合は［詳細をコピー］を押して、その内容を添えて報告してください。」 | ［詳細をコピー］ |
| `Installed` | 「MKLM を 0.2.1 に更新しました。」 | ［リリースノートを開く］ |
| `NotInstalled(InstanceBusy)` | 「ほかのユーザーの MKLM がキーボードの変更の途中だったため、更新しませんでした（MKLM は 0.2.0 のままです）。その変更が終わってから、もう一度［今すぐ更新］を押してください。」 | ［今すぐ更新］ |
| `NotInstalled(ProgramsStillRunning)`、`InstallerRefused(HelperRunning / CliRunning / GuiRunning)`、`CallerDidNotExit` | 「ほかの MKLM（mklm-cli など）が動いていたため、更新しませんでした（MKLM は 0.2.0 のままです）。それを終了してから、もう一度［今すぐ更新］を押してください。」 | ［今すぐ更新］ |
| `NotInstalled(InstallerNotStarted 225 / 226)` | 「インストーラーがウイルス対策ソフトに止められました。Windows セキュリティ → ウイルスと脅威の防止 → 保護の履歴 で確かめてください。MKLM は 0.2.0 のままです。」 | ［詳細をコピー］ |
| `NotInstalled(InstalledVersionChanged)` | 「更新の準備の間に、別の方法で MKLM がインストールされました。今の版で問題なければ、何もする必要はありません。」 | — |
| そのほかの `NotInstalled` | 「更新できませんでした。MKLM は 0.2.0 のままで、キーボードの設定も変わっていません。」＋理由 | ［今すぐ更新］［詳細をコピー］ |
| `Failed(Inconsistent / UnexpectedVersion / InstallerTimedOut)` | D.13 の「そろっていない」の文。期限切れには「インストーラーが 15 分たっても終わりませんでした。」を前に付ける | ［リリース ページを開く］［詳細をコピー］ |
| 中断 | D.13 の表 | — |

- `NotInstalled` と `Failed` の文には、キーボードの設定が変わっていないことを必ず添える（更新はキーボードに触れない。0.2 の 7）。
- 表記の決まりは m3 D.5 に従う（ボタンは［］、Windows の画面の語は「」、2 文以上は「。」で終える、「確定」を使わない）。

### E.7 多言語とアクセシビリティ

- `.slint` の固定の文言は `@tr` と `translations/ja/LC_MESSAGES/mklm.po`。`tests/translations.rs` が漏れを検出する（m3 D.2）。Rust の文は `i18n.rs`（E.6）。利用者向けの文言を `i18n.rs` と `.po` の外に置かない（m3 I 章のレビュー規則）。
- 日本語の出力のラテン文字の許可リスト（`vm::unexpected_latin`、m3 H.1）に `GitHub`、`x64`、`ARM64`、`MB` を足す。
- 読み上げ: ページの見出しにフォーカス（m3 E.1）。進捗は `accessible-label`「ダウンロード: 45 %」、25 % ごとに polite で読み上げる。結果と失敗は assertive。バナーの［詳細…］の読み上げ名は「MKLM の更新の詳細を開く」。ボタンはすべて読み上げ名を持つ（「0.2.1 をスキップ」「今すぐ 0.2.1 に更新」）。
- 状態を色だけで示さない。キーボードだけで操作できる。ボタンの行は本文のスクロールの外（m3 B.0、E.4）。
- リンク（リリースノート、リリース ページ）は `mklm_win::ui::open_release_page(version)` で開く。固定の接頭辞と厳密な版から URL を作り、`ShellExecuteW` に渡す（m3 I 章の「`ShellExecuteW` に渡してよいもの」に足す）。

### E.8 開発用のビルド

- `$INSTDIR` の外（`target\…`）で動く GUI は `NotInstalledCopy`: 確認だけできて、インストールのボタンは出ない。
- デバッグ ビルドの `--update-endpoint=http://127.0.0.1:<port>` は、そのプロセスの間だけ有効（A.10、F.6）。

---

## F. テスト

### F.1 単体テスト（`mklm-update`、ネットワークなし、昇格なし）

- 鍵はテストの中で `minisign`（dev-dependency）で作る使い捨ての鍵。`TrustAnchors::from_keys` で渡す。テスト用の秘密鍵をリポジトリに置かない。

| 対象 | テスト |
|---|---|
| 正常 | 通常用の鍵で署名した更新情報が通る。バックアップ用の鍵でも通る。`VerifiedManifest` の各値 |
| 改ざんした更新情報 | 1 バイトでも変えれば `BadSignature`（各位置で。空白、改行、末尾の追加も） |
| 改ざんした署名 | 署名の行、trusted comment、全体の署名の行のどれを変えても `BadSignature` か `SignatureMalformed`。trusted comment の先頭が違えば `WrongTrustedComment`。legacy の署名（`Ed`）は拒否 |
| 違う鍵 | 埋め込みにない鍵 → `UnknownKey`。ID だけ同じで違う鍵（base64 の手直し）→ `BadSignature` |
| 失効 | 埋め込みの `REVOKED_KEY_IDS`、機械の記録、利用者の記録のそれぞれで `RevokedKey`。自分を失効させる → `IllegalRevocation`。通常用がバックアップ用を失効させる → `IllegalRevocation`。バックアップ用が通常用を失効させる → 通り、`recorded` の後は通常用の更新情報が `RevokedKey` |
| 巻き戻し | `issued_at` が記録より小さい → `Rollback`。同じ → 通る。鍵ごとの記録（ほかの鍵の大きな値に影響されない） |
| 期限 | 期限切れ → `Check` でも `Install` でも通り、`Freshness::Expired`。`issued_at >= expires`、差が 800 日を超える → `BadTimestamps` |
| アーキテクチャ | アセットが 1 つしかない、同じ arch が 2 つ、名前が違う（版、arch、大文字小文字、パス区切り）→ `AssetMalformed`。自分の arch を選ぶ |
| ハッシュ | `sha256` が 63 桁、大文字、16 進でない → `AssetMalformed`。`Stager` で中身が違う → `InstallerHashMismatch` |
| 大きさ | 更新情報 64 KiB + 1、署名 4 KiB + 1、アセットの `size` が 0 と 64 MiB + 1 → それぞれの拒否 |
| スキーマ | `schema` 2、`product` 違い、`channel` 違い、未知のフィールド、フィールドの重複、BOM、末尾のデータ、UTF-8 でない、型違い（文字列の数） |
| 版 | `v0.2.1`、`0.2`、`0.2.1-beta`、`0.2.1+x`、`00.2.1`、`65536.0.0` → `BadVersion`。同じ版 → `NotNewer`（`Install`）/ `UpToDate`（`Check`）。古い版 → 同じ。`min_from_version` → `ManualRequired` / `ManualUpdateRequired`。入っている版のプレリリースとの比較 |
| tag | tag と版の不一致 → `TagMismatch` |
| 記録 | `merged`（最大値、和集合）、`recorded`、JSON の往復、壊れた JSON |
| URL | https だけ、ホストの規則、userinfo、ポート、IP、非 ASCII、相対の `Location` の解決、`Endpoints::production()` の URL の文字列、tag の取り出し（形が違えば `None`） |
| `Stager` | 順番の違う `offset`、空のチャンク、64 KiB + 1 のチャンク、合計の超過、足りないまま `finish` |
| 定数 | `installer_name`、`release_page_url`、`user_agent` の文字列 |
| base64 | RFC 4648 の例、パディングなし、余分なパディング、正準形でない最後の文字 |

`xtask` の単体テスト: 使い捨ての鍵で、偽の `dist`（`SHA256SUMS` と小さな偽のインストーラー 2 つ）に `sign-release` をかけ、`mklm-update` で検証できること。プレリリースの tag、`SHA256SUMS` の食い違い、arch の欠け、埋め込みにない鍵（テスト用の差し替えで）、B.2 に反する `--revoke`、作業ツリーの中への `keygen` をそれぞれ拒否すること。

### F.2 helper の側のテスト（昇格なし）

- `mklm_ipc`: 新しいメッセージの serde の往復と JSON の形の固定（`PROTOCOL_VERSION` 3）。64 KiB のチャンクのフレームが `MAX_FRAME_LEN` に収まること。16 進の厳密さ（大文字、奇数の長さ）。`RunUpdateArgs` の差分テスト（D.7）。`is_uninstall_restore` と `HelperArgs::parse` が `--run-update …` を受け付けないこと、その逆。
- `mklm_ipc::staging::receive_installer`（偽のリンクと偽の書き込み先）: 正常、順番違い、途中の `Bye`、途中の切断、期限切れ、大きさの超過、ハッシュ違い、`InstallerChunk` の代わりに `Request` が来る。`Stager` は使い捨ての鍵で作った `VerifiedManifest` から作る。
- `mklm_update::run`（WP-H）: `classify_installer_exit` の表、`decide_outcome` の表（D.13）、`classify_run`（すべての段階 × 起動 ID の変化 × 持ち主の生死）、`interrupted_result`、`RunId` の文法、JSON の往復と形の固定（H.5）。
- `mklm_update::gate::check_journal`: `mklm_core::fixtures` のジャーナルで、読めない項目、書き込み中、確認待ち、再起動待ち、衝突、閉じたものだけ。
- `crates/mklm-update/tests/nsis_exit_codes.rs`: `mklm.nsi` の定義と定数の一致（D.9.1）。
- `mklm-win`: `update_store` の名前の関所（キーを開く前に拒否することだけ。書き込みは実機）、`update_dir` の名前の検査。

### F.3 ループバックの HTTP テスト（`mklm-update`、feature `winhttp`、Windows）

- `cargo test -p mklm-update --features winhttp`（ci.yml に足す）。`cargo test --workspace` でも feature の統合で走る。`#[cfg(debug_assertions)]` なので `--release` のテストでは走らない。
- テストの中の小さなサーバー（`std::net::TcpListener` を `127.0.0.1:0` に。全インターフェイスに開かないので、Windows ファイアウォールの確認は出ない）が、台本どおりの応答（状態、ヘッダー、本文の分割、遅延）を返す。プロキシを通さないセッション（`new_without_proxy`）を使う。

| テスト | 期待 |
|---|---|
| 200 の小さな本文 | そのまま読める |
| `latest/download` → `download/v0.2.1/latest.json` → 別のポートの 200 | 本文と tag `v0.2.1` |
| リダイレクト 6 回 | `TooManyRedirects` |
| `https://` やほかのホスト（`localhost`、`127.0.0.2`）へのリダイレクト | `RedirectNotAllowed`、**接続が発生しない**（サーバーが接続を数える） |
| `Content-Length` が上限を超える | 本文を読まずに `TooLarge` |
| チャンク転送で上限を超える | 上限 + 1 バイトで `TooLarge` |
| インストーラーが短い / 長い / 中身が違う | `SizeMismatch` / `HashMismatch` |
| 応答しない（受信の期限を 1 秒にしたテスト用の `Limits`） | `Transport(Timeout)` |
| 全体の期限 | `DeadlineExceeded` |
| 404 / 429 / 500 | `NotFound` / `RateLimited` / `HttpStatus` |
| `Content-Encoding: gzip` | `UnexpectedEncoding` |
| 取り消しのフラグ | `Cancelled` |

### F.4 GUI と CLI

- `state::tests`: 確認の予定（前回の時刻、乱数の範囲、復帰）、自動ダウンロードの条件（スキップ、`auto_check` のオフ）、ボタンの可否（D.2 の条件）、セッションは同時に 1 つ（更新とキーボードの変更の排他）、`HandedOff` → RunOnce → 終了、起動時の `InProgress` → すぐ終了、`LastResult` の 1 回だけの表示。
- `vm::update` のスナップショット（日英、m3 H.3）: E.2 の各状態、E.6 のすべての列挙子。
- `mklm-client::update`: 偽の `Transport` で `check` と `download`（キャッシュ、記録の更新、スキップ）。偽の `Link` で `stage`（正常、`Refused`、途中の取り消し、元のファイルの変化、helper の喪失、`SendInstaller` の中身が申し出と違う）。
- 結合テスト（統合の後に通る。WP-C が書く）: `crates/mklm-client/tests/update_staging.rs` で、`mklm_client::update::stage` と `mklm_ipc::staging::receive_installer` をメモリ上の双方向のリンクでつなぐ。
- CLI: `update --check` と `--status` の出力と終了コード（偽の `Transport` と偽の記録で）。

### F.5 インストーラーの CI の試験

D.9.2 の静的な検査と煙の試験。

### F.6 全体の経路をどう確かめるか（ローカルのリハーサル、デバッグ ビルド）

昇格とインストールを伴う部分（H1 のロックとフォルダー、H2、NSIS、起動し直し）は単体テストにできない。v0.2.0 を出す前に、デバッグ ビルドで一度通しで確かめる（ユーザーの同意を得て、開発機で）。

1. `cargo xtask dev-keygen --out %TEMP%\mklm-dev` → 表示された公開鍵を、ビルドの環境変数 `MKLM_UPDATE_DEV_PUBKEY` に入れる（デバッグ ビルドだけがこの鍵を読む。A.10）。
2. `Cargo.toml` の版を一時的に 0.2.0 にして `installer\build-installer.ps1 -Profile dev`（デバッグの 3 つの exe でインストーラーを作る。WP-H）→ 手でインストールする。
3. 版を 0.2.1 にして同じくビルドし、`cargo xtask sign-release --dev --tag v0.2.1 --dist dist-dev --key %TEMP%\mklm-dev\mklm-dev.key`。
4. `cargo xtask serve-releases --dir dist-dev --port 8421`（`127.0.0.1` だけで待ち受け、`/releases/latest/download/…` → `/releases/download/v0.2.1/…` のリダイレクトを GitHub と同じ形で返す）。
5. インストールしたデバッグの GUI を `--update-endpoint=http://127.0.0.1:8421` で起動 → 確認 → ダウンロード → ［今すぐ更新］→ UAC → 更新 → 起動し直し → 結果。
6. 変えた版は元に戻す（コミットしない）。

- 確かめること: D.7 の 5（読み取りの共有だけのハンドルを開いたまま `CreateProcessW` できる）、D.10 の起動し直し、D.8 の `quit`、`update.log`、`LastResult`、中断（H2 を `taskkill` で止める。ユーザーの同意の上で）。
- リリース ビルドにはこの経路がない（A.10）。本番の鍵とサーバーでの確かめは F.7。

### F.7 実機のテスト計画（v0.2.0 → v0.2.1。後でユーザーの同意を得て行う）

準備: M0 の安全手順（`reg export`、PIN、スクリーン キーボード、BitLocker の回復キー）。`mklm-cli status --json --all > before.json`。v0.2.0 を手で入れる（B.8）。v0.2.1 を B.5 の手順で公開する。★は 1 項目ずつ同意を得てから。

- [ ] T-UPD-1: v0.2.0 を起動して 3 分待つ → バナー「新しい版（0.2.1）」、更新のページが準備完了。`%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\` にインストーラーと `state.json`
- [ ] T-UPD-2: 設定で自動の確認をオフ → 次の起動で通信しない（`mklm.log`）。［今すぐ確認］で確認できる
- [ ] T-UPD-3 ★: ［今すぐ更新］→ UAC の説明（初回）→ UAC（プログラム名 mklm-helper.exe、発行元「不明」を記録）→「はい」→ MKLM が消え、数十秒で 0.2.1 が開き「0.2.1 に更新しました」。起動した GUI が昇格していない。`HKLM\…\MKLM\Update` の `LastResult`、`Updates` の片付け、`update.log`。キーボードの値が変わっていない（`status --json` の比較）
- [ ] T-UPD-4 ★: UAC で「いいえ」→「取り消しました」。何も変わらない
- [ ] T-UPD-5 ★: Keychron の変更を「PC の再起動で切り替える」で保存（再起動待ち）した状態で［今すぐ更新］→ ボタンが押せず理由が出る（再起動待ちを元に戻して終える）
- [ ] T-UPD-6 ★: `mklm-cli` の長いコマンド（カウントダウン中の `set`）の最中に［今すぐ更新］→ UAC の前に止まるか、H2 が `ProgramsStillRunning` / `InstanceBusy` で止め、0.2.0 のまま結果が出る
- [ ] T-UPD-7 ★（任意）: 別のアカウントでサインインして MKLM を動かしたまま（ユーザーの切り替え）、元のアカウントで更新 → 別のアカウントの MKLM が終わり、更新される。別のアカウントに戻ると（次のサインインで）結果が出る
- [ ] T-UPD-8 ★（任意）: 標準ユーザーのアカウントで、管理者の資格情報を入れて更新 → 更新される。GUI が起動し直すか（D.10 の未確認の点）、次のサインインで結果が出るかを記録
- [ ] T-UPD-9: ネットワークを切って［今すぐ確認］→ 名前解決か接続の文。戻して再試行
- [ ] T-UPD-10: ダウンロード中に［キャンセル］→ 止まり、`.part` が次の起動で消える
- [ ] T-UPD-11: ［この版をスキップ］→ バナーが消える。更新のページからは入れられる
- [ ] T-UPD-12 ★（任意、危険が小さくない）: H2 の待ちの間に `taskkill /F` で H2 を止める（昇格したターミナル）→ 次の起動で「更新は中断されました。何も変更されていません」
- [ ] T-UPD-13: `mklm-cli update --check` と `--status`（`--json` も）
- [ ] T-UPD-14: ナレーターで更新のページ（見出し、進捗、結果の読み上げ）。表示スケール 200 %
- [ ] T-UPD-15: Windows Defender が `Updates` のインストーラーをどう扱うか（隔離されなければ記録だけ）

後始末: `status --json --all` を取り直して比べる。

---

## G. 作業の分担

### G.1 順序

1. **WP-0**（1 人）: 骨組み。コンパイルが通り、既存のテストがすべて通る状態で渡す（G.2）。
2. **WP-U、WP-H、WP-C**（並行）: ファイルの持ち主は重ならない（G.3〜G.5）。互いの実装を待たずに、H 章の約束に対して書く。
3. **統合**: 3 つを合わせ、F.4 の結合テストを含めて `cargo test --workspace`、`clippy -D warnings`（x64 と ARM64）、`fmt` を通す。
4. **レビュー**（G.6 の確認事項）→ F.6 のリハーサル → 鍵の生成（メンテナー）→ v0.2.0 → F.7。

### G.2 WP-0: 骨組み

**やること**

- ルートの `Cargo.toml`
  - `members` に `"crates/mklm-update"` と `"xtask"`。
  - `[workspace.dependencies]` に:
    ```toml
    mklm-update = { path = "crates/mklm-update" }
    # Update manifest signatures (design m5b B.1): exact pin, bumped deliberately.
    minisign-verify = "=0.3.0"
    # Maintainer signing tool (xtask) and test keys only; never linked into the shipped executables.
    minisign = "0.10.0"
    sha2 = "0.11.0"
    semver = "1.0.28"
    ```
  - `[profile.release]` に `debug-assertions = false`（A.10）。
- `.cargo/config.toml`（新規）:
  ```toml
  [alias]
  xtask = "run --package xtask --locked --"
  ```
- `apps/build_id.rs`: `HASHED_CRATES` に `"crates/mklm-update"` を足す（配列の長さ 5）。ipc のメッセージが `mklm-update` の型を含むため（m2 A.5 の「版を上げ忘れた変更の検出」）。
- `crates/mklm-update/Cargo.toml`（新規）:
  ```toml
  [package]
  name = "mklm-update"
  description = "Signed update manifests of MKLM: verification, anti-rollback state, fetch policy (design m5b)"
  version.workspace = true
  edition.workspace = true
  rust-version.workspace = true
  license.workspace = true
  authors.workspace = true
  repository.workspace = true

  [dependencies]
  mklm-core.workspace = true
  serde.workspace = true
  serde_json.workspace = true
  thiserror.workspace = true
  minisign-verify.workspace = true
  sha2.workspace = true
  semver.workspace = true

  [target.'cfg(windows)'.dependencies]
  mklm-win = { workspace = true, optional = true, features = ["net"] }

  [features]
  # The WinHTTP transport (mklm_update::winhttp). The GUI and the CLI get it through mklm-client;
  # the helper never enables it (design m5b A.9).
  winhttp = ["dep:mklm-win"]

  [dev-dependencies]
  minisign.workspace = true
  mklm-core = { workspace = true, features = ["test-fixtures"] }

  [lints]
  workspace = true
  ```
- `crates/mklm-ipc/Cargo.toml`: `mklm-update.workspace = true`。
- `crates/mklm-win/Cargo.toml`:
  - `[features]` に `net = ["windows/Win32_Networking_WinHttp"]`。
  - `windows` の features に、見込みとして `"Win32_System_Ole"`、`"Win32_System_Variant"`（`shell_launch` の `VARIANT` / `BSTR`）を足す（ほかに要るものは WP-H が足す）。
- `crates/mklm-client/Cargo.toml`: `mklm-update = { workspace = true, features = ["winhttp"] }`。
- `apps/mklm-helper/Cargo.toml`: `mklm-update.workspace = true`（feature なし）。
- `apps/mklm/Cargo.toml`、`apps/mklm-cli/Cargo.toml`: `mklm-update.workspace = true`。
- `xtask/Cargo.toml`（新規）: `publish = false`、依存 `mklm-update`、`minisign`、`sha2`、`semver`、`serde_json`、`clap`、`anyhow`。`src/main.rs` は B.3 のサブコマンドを clap で定義し、本体は「not implemented yet」のエラーで終了コード 1。
- H 章のすべての公開の項目を、該当するファイルに置く。
  - **型、列挙、定数、トレイトは完全に書く**（serde の属性を含む）。
  - **小さな文法と表の関数は WP-0 が完全に実装し、テストを付ける**: `KeyId::parse` と表示、`Arch::as_str` / `of_this_build`、`installer_name`、`release_page_url`、`user_agent`、`RunId::new` / `parse`、`RunUpdateArgs::parse` / `to_parameters` / `run_update_args`、`encode_hex` / `decode_hex`、`InstallerChunk::new` / `decode`、`classify_installer_exit`、`nsis_exit` の定数、`UPDATE_VALUE_NAMES`、`Endpoints::production` の URL。3 つの WP がこれらの結果を前提にするため。
  - **それ以外の関数の本体**は、テストが通る経路に `todo!()` を置かない。`Result` を返すものは `Err`（`UpdateRefusal::Internal { detail: "not implemented (m5b skeleton)" }`、`mklm_win::Error::Win32 { function: "<名前> (m5b skeleton)", code: 50 }`（`ERROR_NOT_SUPPORTED`）など）、そのほかは害のない既定値を返す。モジュールの先頭に `#![allow(unused_variables, dead_code)] // Skeleton (M5b)` を付ける（m2 0.4 と同じ）。
  - `EMBEDDED_KEYS` と `REVOKED_KEY_IDS` は空。
- 既存のコードをコンパイルさせる変更（網羅的な `match`）:
  - `apps/mklm-helper/src/session.rs`: `serve` のループに `CallerMessage::StageUpdate` → `crate::update::stage(…)`（骨組みは `Update::Refused(Internal)` を送って続ける）、`CallerMessage::InstallerChunk` → プロトコルの誤り。`Inbox::poll` で両方を「去った」に。`session()` の先頭の分岐に `mklm_ipc::run_update_args` → `crate::run_update::run(args)`（骨組みは終了コード 7）。`apps/mklm-helper/src/update.rs` と `run_update.rs` を作る。
  - `crates/mklm-client/src/session.rs`: 中継の `match` に `HelperMessage::Update(_)` を足し、`Hello` と同じく「予期しないフレーム」で失う扱いにする。
  - `apps/mklm-cli/src/write/relay.rs` のテストの偽物など、`HelperMessage` / `CallerMessage` を網羅するもの。
  - `crates/mklm-ipc/tests/messages.rs`: 列挙子を網羅するテストに新しいものを足す。
  - `crates/mklm-client/src/lib.rs`: `pub mod update;`（中は H.4 の骨組み）。
  - `crates/mklm-win/src/lib.rs`: `pub mod update_store; pub mod update_dir; pub mod shell_launch; pub mod user_dirs; #[cfg(feature = "net")] pub mod net;`。`ui/mod.rs` に `pub mod open_url;`（または関数）。
- `cargo build --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`（x64 と `--target aarch64-pc-windows-msvc`）、`cargo fmt --check` が通ること。`Cargo.lock` をコミットする（以後、WP は依存を足さない。足す必要があれば統合のときにまとめる）。

**WP-0 が完了したら、G.3〜G.5 の持ち主に渡す。** `Cargo.toml` と `Cargo.lock` は WP-0 の後、統合まで誰も変えない（例外: WP-H は `crates/mklm-win/Cargo.toml` の `windows` の features だけ足してよい。features は `Cargo.lock` を変えない）。

### G.3 WP-U: `mklm-update`、署名の道具、メンテナーの文書

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-update/src/{lib,keys,manifest,verify,state,version,refusal,url,fetch,stage,winhttp,base64}.rs`、`crates/mklm-update/tests/*`（`nsis_exit_codes.rs` を除く） | C 章、A.6〜A.10 |
| `crates/mklm-win/src/net.rs` | WinHTTP の FFI（A.7、A.9）。unsafe にはすべて `// SAFETY:` |
| `xtask/**` | B.3、F.6 の `dev-keygen`、`sign-release --dev`、`serve-releases` |
| `docs/maintainer/release-signing.ja.md`（新規） | B.5〜B.7 |
| `docs/install-guide.ja.md`、`README.md` | 自動更新の説明、公開鍵と `minisign` による手での検証、インストーラーの `/S` と終了コード（D.9.1） |

完了の条件: F.1、F.3 が通る。`xtask` の往復のテスト。

### G.4 WP-H: パイプ、Windows の部品、helper、NSIS、CI

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-update/src/{run,gate}.rs`、`crates/mklm-update/tests/nsis_exit_codes.rs` | D.7、D.13 の判定、ジャーナルの門 |
| `crates/mklm-ipc/src/{lib,message,update,staging,args}.rs`、`crates/mklm-ipc/tests/*` | D.3、D.7 のコマンドライン、F.2 |
| `crates/mklm-win/src/{update_store,update_dir,shell_launch,proc_identity,instance,os}.rs`、`protected_dir.rs` と `journal_store.rs`（`RunDir` と `UpdateStore` に要る内部の関数を公開する変更だけ）、`crates/mklm-win/Cargo.toml` の `windows` の features | D.5〜D.8、D.10、D.11、C.7 |
| `apps/mklm-helper/src/{main,session,update,run_update}.rs` | D.4、D.7、D.15 |
| `installer/nsis/mklm.nsi`、`installer/build-installer.ps1`（`-Profile dev`）、`installer/check-nsi.ps1`（新規） | D.9、F.6 |
| `.github/workflows/{ci,release}.yml` | G.6 |
| `docs/recovery.md` | 「更新が途中で止まったとき」（D.13） |

完了の条件: F.2、F.5 が通る。F.6 のリハーサルの手順が動く（実施は同意の後）。

### G.5 WP-C: クライアント、GUI、CLI、多言語

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-client/src/update/**`、`crates/mklm-client/src/lib.rs`、`crates/mklm-client/tests/update_*.rs` | H.4、D.2、D.3 の送る側、C.4 の利用者の記録 |
| `crates/mklm-win/src/{user_dirs.rs,ui/open_url.rs}`、`crates/mklm-win/src/session.rs`（`register_after_update` / `unregister_after_update` だけ） | E.1、E.7、D.10 |
| `apps/mklm/**`（`state/update.rs`、`vm/update.rs`、`i18n.rs`、`ui/screens/update.slint`、`app.slint`、`translations/`、`settings.rs`、`args.rs`（`--after-update`、デバッグの `--update-endpoint`）、`worker.rs`、`app.rs`、`tray.rs`、`tests/*`） | E 章 |
| `apps/mklm-cli/src/{update.rs,main.rs}` | D.14 |

完了の条件: F.4 が通る（結合テストは統合の後）。

### G.6 CI と、レビューで確かめること

**ci.yml に足すもの**（WP-H）

- `cargo test -p mklm-update --features winhttp --locked`（F.3）。
- `cargo tree -p mklm-helper -e features --locked` の出力に `mklm-win feature "net"` と `mklm-update feature "winhttp"` がないこと。
- `installer/check-nsi.ps1`（D.9.2）。

**release.yml に足すもの**（WP-H）

- ビルドの前に `cargo xtask check-keys`（鍵が空のままのリリースを止める。B.2）。
- ビルドの後: 3 つの exe の `VersionInfo.IsDebug` が false（A.10）。`dumpbin /imports mklm-helper.exe` に `WINHTTP.dll` がない（A.9）。
- D.9.2 の煙の試験（x64）。
- （任意、計画 4.3〜4.4）`actions/attest-build-provenance` でインストーラーの来歴を証明する（`id-token: write`、`attestations: write` の権限。アクションは SHA で固定）。B.5 の手順 6 で使う。

**レビューで確かめること**（m2 K、m3 I の規則への追加）

- HKLM に `KEY_SET_VALUE` を使うのは `regwrite`、`journal_store`、`machine_settings`、`update_store` だけ。`update_store` は `UPDATE_VALUE_NAMES` の 3 つだけを書く。
- HKCU に書くのは `session` だけ（RunOnce の `AfterUpdate` が増えた）。helper は HKCU を読み書きしない。
- `ShellExecuteW` に渡してよいものに、`open_release_page` が作るリリース ページの URL が増えた。
- helper は利用者の場所のファイルを開かない（インストーラーはパイプで受け取る）。helper の依存にネットワークのコードがない（上の CI）。
- helper の固定のコマンドラインは 3 つ（パイプのセッション、`--uninstall-restore`、`--run-update`）で、どれも手書きの厳密なパーサー。
- 本番以外の URL、平文 HTTP、デバッグの鍵が `#[cfg(debug_assertions)]` の外にない。
- `verify_manifest` は署名を確かめる前に更新情報の中身を解析しない（C.3 の 6）。
- H2 はインストーラーのハンドルを `CreateProcessW` まで閉じない。H2 は自分のトークンで GUI を起動しない。
- 利用者がボタンを押していない UAC がない。
- `mklm.nsi` のすべての `Quit` / `Abort` の直前に `SetErrorLevel`、すべての `MessageBox` に `/SD`。
- 利用者向けの文言が `i18n.rs` と `.po` の外にない。

### G.7 リスク

1. **GitHub の配布の仕組みの変更**: リダイレクトの形や配布元のドメインが変わると、確認か署名の取得が止まる（A.6 で `.githubusercontent.com` の接尾辞と tag なしの予備を用意した）。止まった場合、更新で直せないので、手で入れてもらう。
2. **新しい版のクレート**: `minisign-verify` 0.3.0 と `minisign` 0.10.0 は出たばかりで、変更点を確かめていない。WP-0 がシグネチャを確かめ、問題があれば 0.2.5 / 0.9.1 に戻す（`xtask` の往復のテストが両者の互換を確かめる）。
3. **最初の更新対応版の不具合**: v0.2.0 のアップデーターに不具合があると、v0.2.0 の利用者は手で直す必要がある。F.6 のリハーサルで減らす。
4. **起動し直しの失敗**: 別の管理者で昇格した場合など（D.10）。RunOnce、Run キー、案内の文で補うが、「MKLM が消えた」と感じる利用者がいうる。
5. **途中で止まったインストール**: 電源断やサインアウト（D.12）。検出と案内はあるが、直すにはインストーラーを手で実行する必要がある。
6. **ウイルス対策ソフト**: 署名のないインストーラーを `ProgramData` から実行するので、止められたり遅くなったりしうる（`InstallerNotStarted`、期限 15 分）。
7. **有効期限の失効**: メンテナーが 13 か月リリースしないと、全員に期限切れの情報が出る（B.4）。
8. **鍵をなくす**: 両方をなくすと自動更新が止まり、手で入れ直してもらうしかない（B.7）。
9. **feature の統合**: ワークスペースのビルドで helper にも WinHTTP のコードがコンパイルされる。リンカーが落とす前提で、release.yml のインポートの検査で守る（A.9）。
10. **ほかの利用者の MKLM を終わらせる**: 何もしていない MKLM は、その利用者の同意なしに終わる（次のサインインで戻る。D.8、J 章の質問 2）。
11. **ロックの受け渡しの隙間**: H1 がロックを放してから H2 が取るまでの間に、ほかの書き手が操作を始めうる。H2 がジャーナルを確かめ直して止めるので安全だが、更新はやり直しになる（D.7 の 10）。
12. **ジョブ オブジェクト**: H1 が kill-on-close のジョブに入っていると、H1 の終了で H2 も終わる。H1 は UAC（AppInfo）が作るので呼び出し元のジョブには入らないはずだが、確かめていない。H2 は `ready` の前に H1 に見られているので、止められても `Interrupted` として検出される。
13. **更新とアンインストールの同時実行**: アンインストーラー（`--uninstall-restore` はロックで待つが、ファイルの削除は止まらない）と重なると、結果が壊れうる。起こりにくいので、検出（D.13）に任せる。
14. **x64 版を ARM64 の PC で使っている利用者**: ARM64 版に切り替わらない（C.7。J 章の質問 1）。
15. **リハーサルと本番の違い**: F.6 はデバッグ ビルドと平文の HTTP で、TLS、GitHub、リリース ビルドの最適化は F.7 でしか確かめられない。

---

## H. API の約束

WP の境界を越えるすべての公開の項目。ここにない公開の項目は、その WP の中の都合で決めてよい。`use` と `derive` は、ここに書いたものを必ず持つ（書いていない `derive` を足すのはよい）。

### H.1 `mklm-update`

```rust
// crates/mklm-update/src/lib.rs
#![forbid(unsafe_code)]

pub mod fetch;
pub mod gate;
pub mod keys;
pub mod manifest;
pub mod refusal;
pub mod run;
pub mod stage;
pub mod state;
pub mod url;
pub mod verify;
pub mod version;
#[cfg(all(windows, feature = "winhttp"))]
pub mod winhttp;
mod base64;

pub use keys::{KeyError, KeyId, KeyRole, TrustAnchors, TrustedKey};
pub use manifest::{Arch, Manifest, ManifestAsset, Sha256Digest, Sha256Stream};
pub use refusal::UpdateRefusal;
pub use semver::Version;
pub use state::{StateError, TrustState};
pub use verify::{Freshness, OfferKind, Purpose, SelectedAsset, VerifiedManifest, VerifyInput, verify_manifest};

pub const PRODUCT: &str = "MKLM";
pub const CHANNEL: &str = "stable";
pub const MANIFEST_SCHEMA: u32 = 1;
pub const MANIFEST_NAME: &str = "latest.json";
pub const SIGNATURE_NAME: &str = "latest.json.minisig";
pub const TRUSTED_COMMENT_PREFIX: &str = "mklm-latest-json v1";
pub const REPO_URL: &str = "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager";
pub const MAX_MANIFEST_LEN: usize = 64 * 1024;
pub const MAX_SIGNATURE_LEN: usize = 4 * 1024;
pub const MAX_INSTALLER_LEN: u64 = 64 * 1024 * 1024;
pub const MAX_VALIDITY_SECS: u64 = 800 * 86_400;
pub const DEFAULT_VALIDITY_DAYS: u64 = 400;
/// The GUI's start argument after an update (H2 and the RunOnce value pass it).
pub const GUI_AFTER_UPDATE_ARG: &str = "--after-update";

/// `MKLM-Setup-<version>-<arch>.exe`.
pub fn installer_name(version: &Version, arch: Arch) -> String;
/// `<REPO_URL>/releases/tag/v<version>`.
pub fn release_page_url(version: &Version) -> String;
/// `MKLM/<version> (Windows; <arch>; +<REPO_URL>)`.
pub fn user_agent(version: &str, arch: Arch) -> String;
```

```rust
// crates/mklm-update/src/keys.rs
/// A minisign key ID. Text form: 16 upper-case hex digits of the 8 bytes read as a little-endian
/// u64 (design m5b B.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    /// Exactly 16 upper-case hex digits.
    pub fn parse(text: &str) -> Result<KeyId, KeyError>;
    pub fn to_text(self) -> String;
}
impl std::fmt::Display for KeyId { /* to_text */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRole {
    Primary,
    Backup,
}

/// One embedded public key (what `xtask keygen` prints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    pub id: &'static str,
    pub role: KeyRole,
    /// The base64 line of the minisign public key file.
    pub public_key: &'static str,
}

/// Empty until the maintainer generates the keys (design m5b B.5).
pub const EMBEDDED_KEYS: &[TrustedKey] = &[];
/// Key IDs revoked as of this build.
pub const REVOKED_KEY_IDS: &[&str] = &[];

/// The parsed keys a manifest may be signed with. `Debug` prints IDs and roles only.
pub struct TrustAnchors { /* private */ }
impl std::fmt::Debug for TrustAnchors { /* ids and roles */ }

impl TrustAnchors {
    /// `EMBEDDED_KEYS` and `REVOKED_KEY_IDS` (debug builds: plus `MKLM_UPDATE_DEV_PUBKEY` as a
    /// primary key). `NotConfigured` when there is no key at all.
    pub fn embedded() -> Result<TrustAnchors, KeyError>;
    /// For tests and xtask: the same checks as `embedded`.
    pub fn from_keys(keys: &[(KeyRole, &str)], revoked: &[&str]) -> Result<TrustAnchors, KeyError>;
    pub fn ids(&self) -> Vec<(KeyId, KeyRole)>;
    pub fn role_of(&self, id: KeyId) -> Option<KeyRole>;
    pub fn revoked_by_build(&self, id: KeyId) -> bool;
}

/// The key ID inside a minisign public key (base64 line), strict base64.
pub fn public_key_id(public_key_base64: &str) -> Result<KeyId, KeyError>;
/// The key ID inside a minisign signature file (its second line), strict base64.
pub fn signature_key_id(signature_text: &str) -> Result<KeyId, KeyError>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("this build has no update keys")]
    NotConfigured,
    #[error("embedded key {index} is not a minisign public key")]
    BadPublicKey { index: usize },
    #[error("{text:?} is not a key ID")]
    BadKeyId { text: String },
    #[error("embedded key {index}: its ID does not match the public key")]
    IdMismatch { index: usize },
    #[error("two embedded keys have the same ID")]
    DuplicateId,
    #[error("the embedded keys must be exactly one primary and one backup key")]
    Roles,
    #[error("the signature file is malformed")]
    BadSignatureText,
}
```

```rust
// crates/mklm-update/src/manifest.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X64,   // "x64"
    Arm64, // "arm64"
}
impl Arch {
    pub fn as_str(self) -> &'static str;
    /// `cfg!(target_arch = "aarch64")` → `Arm64`, else `X64`.
    pub fn of_this_build() -> Arch;
}

/// latest.json, schema 1, as on the wire (design m5b A.2). Checked by `verify_manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub channel: String,
    pub version: String,
    pub issued_at: u64,
    pub expires: u64,
    pub key_id: String,
    pub revoked_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_from_version: Option<String>,
    pub assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAsset {
    pub arch: Arch,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    /// Strict: UTF-8 without BOM, no unknown or duplicate fields, nothing after the object.
    /// `ManifestTooLarge` / `ManifestMalformed`.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, UpdateRefusal>;
    /// The canonical text xtask writes (design m5b A.3).
    pub fn to_canonical_json(&self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest(pub [u8; 32]);
impl Sha256Digest {
    /// Exactly 64 lower-case hex digits.
    pub fn parse_hex(text: &str) -> Option<Sha256Digest>;
    pub fn to_hex(&self) -> String;
    pub fn of(bytes: &[u8]) -> Sha256Digest;
}

/// Incremental SHA-256 (sha2).
#[derive(Debug, Clone, Default)]
pub struct Sha256Stream { /* private */ }
impl Sha256Stream {
    pub fn new() -> Sha256Stream;
    pub fn update(&mut self, bytes: &[u8]);
    pub fn finish(self) -> Sha256Digest;
}
```

```rust
// crates/mklm-update/src/verify.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// GUI / CLI: an older or equal version is `OfferKind::UpToDate`, not an error.
    Check,
    /// Helper: anything but `OfferKind::Newer` is refused.
    Install,
}

#[derive(Debug, Clone, Copy)]
pub struct VerifyInput<'a> {
    pub manifest: &'a [u8],
    pub signature: &'a [u8],
    pub anchors: &'a TrustAnchors,
    pub state: &'a TrustState,
    pub installed: &'a Version,
    pub arch: Arch,
    pub now_unix: u64,
    /// The tag from the fetch's first redirect (design m5b A.6); `None` in the helper.
    pub tag: Option<&'a str>,
    pub purpose: Purpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAsset {
    pub arch: Arch,
    pub name: String,
    pub size: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferKind {
    Newer,
    UpToDate,
    /// `min_from_version` is newer than the installed version: update by hand.
    ManualRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub version: Version,
    pub issued_at: u64,
    pub expires: u64,
    pub signer: KeyId,
    pub signer_role: KeyRole,
    pub revoked: std::collections::BTreeSet<KeyId>,
    pub min_from_version: Option<Version>,
    pub asset: SelectedAsset,
    pub freshness: Freshness,
    pub offer: OfferKind,
}

/// Design m5b C.3, in that order. Never parses the manifest before the signature is verified.
pub fn verify_manifest(input: &VerifyInput<'_>) -> Result<VerifiedManifest, UpdateRefusal>;
```

```rust
// crates/mklm-update/src/state.rs
/// Revocations and the highest `issued_at` per signing key (design m5b C.4). JSON; unknown
/// fields are ignored. `Default` has `schema = 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    pub schema: u32,
    /// Key ID text → highest `issued_at` seen from that key.
    #[serde(default)]
    pub max_issued_at: std::collections::BTreeMap<String, u64>,
    /// Key ID texts.
    #[serde(default)]
    pub revoked: std::collections::BTreeSet<String>,
}
impl Default for TrustState { /* schema 1, empty */ }

impl TrustState {
    pub fn parse(json: &str) -> Result<TrustState, StateError>;
    pub fn to_json(&self) -> String;
    /// Per key the higher value; the union of the revocations.
    pub fn merged(&self, other: &TrustState) -> TrustState;
    /// After a successful verification: the signer's maximum and the manifest's revocations.
    pub fn recorded(&self, verified: &VerifiedManifest) -> TrustState;
    pub fn is_revoked(&self, id: KeyId) -> bool;
    pub fn max_issued_at(&self, id: KeyId) -> Option<u64>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("malformed update record: {0}")]
    Malformed(String),
    #[error("update record schema {0} is not supported")]
    Schema(u32),
}
```

```rust
// crates/mklm-update/src/version.rs
/// Largest value of one version part (VERSIONINFO fields are 16-bit).
pub const MAX_VERSION_PART: u64 = 65_535;
/// `X.Y.Z` only (design m5b A.2). `BadVersion`.
pub fn parse_release_version(text: &str) -> Result<Version, UpdateRefusal>;
/// `CARGO_PKG_VERSION`: a pre-release is allowed, build metadata is not. `BadVersion`.
pub fn parse_installed_version(text: &str) -> Result<Version, UpdateRefusal>;
/// SemVer precedence: `offered > installed`.
pub fn is_newer(offered: &Version, installed: &Version) -> bool;
```

```rust
// crates/mklm-update/src/refusal.rs
/// Why an update was not verified, staged or started. Nothing was changed when one is returned.
/// Serialized inside the pipe's `UpdateMessage::Refused` and the registry's `LastResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum UpdateRefusal {
    // Set-up
    #[error("this build has no update keys")]
    NotConfigured,
    #[error("MKLM does not run from its installation folder")]
    NotInstalledCopy,
    // Sizes
    #[error("the manifest is too large ({len} bytes)")]
    ManifestTooLarge { len: u64 },
    #[error("the signature is too large ({len} bytes)")]
    SignatureTooLarge { len: u64 },
    // Signature
    #[error("the signature file is malformed")]
    SignatureMalformed,
    #[error("the signature's trusted comment is not for an MKLM manifest")]
    WrongTrustedComment,
    #[error("signed with an unknown key {key_id}")]
    UnknownKey { key_id: String },
    #[error("the signature does not verify")]
    BadSignature,
    #[error("signed with the revoked key {key_id}")]
    RevokedKey { key_id: String },
    // Content
    #[error("malformed manifest: {detail}")]
    ManifestMalformed { detail: String },
    #[error("manifest schema {schema} is not supported")]
    UnsupportedSchema { schema: u32 },
    #[error("the manifest is for another product")]
    WrongProduct,
    #[error("the manifest is for another channel")]
    WrongChannel,
    #[error("the manifest names key {manifest} but was signed with {signature}")]
    KeyIdMismatch { manifest: String, signature: String },
    #[error("the manifest may not revoke key {key_id}")]
    IllegalRevocation { key_id: String },
    #[error("{text:?} is not a release version")]
    BadVersion { text: String },
    #[error("the manifest of tag {tag} is for version {version}")]
    TagMismatch { tag: String, version: String },
    #[error("issued_at and expires are inconsistent")]
    BadTimestamps,
    #[error("the manifest was issued at {issued_at}, before {seen}")]
    Rollback { issued_at: u64, seen: u64 },
    #[error("no installer for {arch:?}")]
    NoAssetForArch { arch: Arch },
    #[error("malformed asset: {detail}")]
    AssetMalformed { detail: String },
    // Offer
    #[error("{offered} is not newer than {installed}")]
    NotNewer { offered: String, installed: String },
    #[error("{installed} must be updated by hand (the update requires at least {min_from})")]
    ManualUpdateRequired { min_from: String, installed: String },
    // Helper (staging and run)
    #[error("another MKLM holds the write lock")]
    Busy,
    #[error("another update is in progress")]
    UpdateInProgress,
    #[error("an operation is waiting for the user")]
    OperationOpen { waiting_for_reboot: bool },
    #[error("an interrupted operation needs recovery")]
    RecoveryNeeded,
    #[error("the journal cannot be read")]
    JournalUnreadable,
    #[error("received {received} of {expected} installer bytes")]
    InstallerSizeMismatch { expected: u64, received: u64 },
    #[error("the installer's SHA-256 does not match the manifest")]
    InstallerHashMismatch,
    #[error("a malformed installer chunk")]
    ChunkMalformed,
    #[error("installer chunk at {found}, expected {expected}")]
    ChunkOutOfOrder { expected: u64, found: u64 },
    #[error("the caller left")]
    CallerLeft,
    #[error("the update runner did not start: {detail}")]
    HandOffFailed { detail: String },
    #[error("storage: {detail}")]
    Storage { detail: String },
    #[error("internal: {detail}")]
    Internal { detail: String },
}
```

```rust
// crates/mklm-update/src/url.rs
/// A URL that passed a `UrlPolicy` (design m5b A.6). Only https (debug builds: http to 127.0.0.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url { /* private */ }
impl Url {
    pub fn parse(text: &str, policy: &UrlPolicy) -> Result<Url, UrlError>;
    /// A `Location` header: absolute, or an absolute path on the same host.
    pub fn resolve(&self, location: &str, policy: &UrlPolicy) -> Result<Url, UrlError>;
    pub fn as_str(&self) -> &str;
    pub fn is_https(&self) -> bool;
    pub fn host(&self) -> &str;
    pub fn port(&self) -> u16;
    pub fn path_and_query(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPolicy { /* private */ }
impl UrlPolicy {
    /// https, port 443, first host github.com, redirects to github.com or *.githubusercontent.com.
    pub fn production() -> UrlPolicy;
    /// http://127.0.0.1:<any port> only.
    #[cfg(debug_assertions)]
    pub fn loopback() -> UrlPolicy;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints { /* private */ }
impl Endpoints {
    pub fn production() -> Endpoints;
    /// `base` = `http://127.0.0.1:<port>` standing in for `REPO_URL` (same paths below it).
    #[cfg(debug_assertions)]
    pub fn loopback(base: &str) -> Result<Endpoints, UrlError>;
    pub fn policy(&self) -> &UrlPolicy;
    /// `<base>/releases/latest/download/latest.json`
    pub fn manifest_url(&self) -> Url;
    /// `<base>/releases/download/<tag>/latest.json.minisig`, or the latest/download one.
    pub fn signature_url(&self, tag: Option<&str>) -> Url;
    /// `<base>/releases/download/v<version>/<name>`
    pub fn asset_url(&self, version: &Version, name: &str) -> Url;
    /// `v<X.Y.Z>` when `location` is `<base>/releases/download/v<X.Y.Z>/latest.json`.
    pub fn tag_from_location(&self, location: &Url) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{url:?} is not allowed: {reason}")]
pub struct UrlError {
    pub url: String,
    pub reason: &'static str,
}
```

```rust
// crates/mklm-update/src/fetch.rs
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub resolve: Duration,
    pub connect: Duration,
    pub send: Duration,
    pub receive: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeouts: Timeouts,
    /// Whole fetch, redirects included.
    pub total: Duration,
    pub max_redirects: u8,
}
impl Limits {
    /// 15 s / 15 s / 30 s / 30 s, total 60 s, 5 redirects (design m5b A.8).
    pub fn manifest() -> Limits;
    /// 15 s / 15 s / 30 s / 60 s, total 30 min, 5 redirects.
    pub fn installer() -> Limits;
}

/// One GET without following redirects, cookies or credentials.
pub trait Transport {
    fn get(&mut self, url: &Url, accept: &str, timeouts: &Timeouts)
        -> Result<Box<dyn Response + '_>, TransportError>;
}

pub trait Response {
    fn status(&self) -> u16;
    /// Case-insensitive name; `None` when absent.
    fn header(&self, name: &str) -> Option<String>;
    /// 0 at the end of the body.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("timed out")]
    Timeout,
    #[error("the host name could not be resolved")]
    NameNotResolved,
    #[error("could not connect")]
    CannotConnect,
    #[error("the secure connection failed")]
    Tls,
    #[error("the proxy requires authentication")]
    ProxyAuthRequired,
    #[error("cancelled")]
    Cancelled,
    #[error("network error {code}")]
    Other { code: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    Transport(TransportError),
    #[error("not found (404)")]
    NotFound,
    #[error("rate limited ({status})")]
    RateLimited { status: u16 },
    #[error("HTTP status {status}")]
    HttpStatus { status: u16 },
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("redirect to {location:?} is not allowed")]
    RedirectNotAllowed { location: String },
    #[error("a redirect without Location")]
    MissingLocation,
    #[error("an encoded (compressed) response")]
    UnexpectedEncoding,
    #[error("larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("received {received} bytes, expected {expected}")]
    SizeMismatch { expected: u64, received: u64 },
    #[error("SHA-256 mismatch")]
    HashMismatch,
    #[error("the fetch took too long")]
    DeadlineExceeded,
    #[error("cancelled")]
    Cancelled,
    #[error("writing the download failed: {detail}")]
    Sink { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedManifest {
    pub manifest: Vec<u8>,
    pub signature: Vec<u8>,
    pub tag: Option<String>,
}

/// latest.json, then its signature (from the same tag when known). Design m5b A.5–A.8.
pub fn fetch_manifest(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<FetchedManifest, FetchError>;

/// Streams the installer into `sink`, checking the exact size and SHA-256; `progress(bytes)`.
pub fn download_asset(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    version: &Version,
    asset: &SelectedAsset,
    limits: &Limits,
    sink: &mut dyn std::io::Write,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), FetchError>;
```

```rust
// crates/mklm-update/src/stage.rs
/// Raw bytes per `InstallerChunk` (hex doubles it; fits `MAX_FRAME_LEN`).
pub const CHUNK_LEN: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlan {
    pub run_id: RunId,
    pub from_version: Version,
    pub to_version: Version,
    pub asset: SelectedAsset,
}
impl StagePlan {
    pub fn new(verified: &VerifiedManifest, from_version: &Version, run_id: RunId) -> StagePlan;
}

/// H1's receiving state machine (design m5b D.4 steps 12–13). Pure.
#[derive(Debug)]
pub struct Stager { /* private */ }
impl Stager {
    pub fn new(plan: StagePlan) -> Stager;
    pub fn plan(&self) -> &StagePlan;
    pub fn received(&self) -> u64;
    /// `offset` must equal `received()`; 1..=CHUNK_LEN bytes; the total may not exceed the size.
    /// Returns the new total. `ChunkOutOfOrder` / `ChunkMalformed` / `InstallerSizeMismatch`.
    pub fn accept(&mut self, offset: u64, data: &[u8]) -> Result<u64, UpdateRefusal>;
    pub fn is_complete(&self) -> bool;
    /// `InstallerSizeMismatch` / `InstallerHashMismatch`.
    pub fn finish(self) -> Result<Sha256Digest, UpdateRefusal>;
}
```

```rust
// crates/mklm-update/src/run.rs  (WP-H implements; WP-0 writes the types and the grammar)
use mklm_core::{BootId, Liveness, ProcessIdentity, Timestamp};

/// `<major>.<minor>.<patch>-<16 lower-case hex digits>`, each number 0..=65535 without a
/// leading zero. Folder name and record key of one update.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RunId(String);
impl RunId {
    pub fn new(version: &Version, random: [u8; 8]) -> RunId;
    pub fn parse(text: &str) -> Result<RunId, UpdateRefusal>;
    pub fn as_str(&self) -> &str;
    pub fn version(&self) -> Version;
}
impl TryFrom<String> for RunId { /* parse */ }
impl From<RunId> for String { /* the text */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunPhase {
    Staging,
    Staged,
    Ready,
    Waiting,
    Installing,
    Finishing,
    Done,
}

/// `HKLM\…\MKLM\Update\Run` (design m5b D.6, D.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub schema: u32, // 1
    pub run_id: RunId,
    pub from_version: String,
    pub to_version: String,
    pub arch: Arch,
    pub phase: RunPhase,
    pub boot_id: BootId,
    pub started_at: Timestamp,
    pub phase_at: Timestamp,
    /// The GUI that asked (the pipe server of H1).
    pub caller: Option<ProcessIdentity>,
    pub caller_session: Option<u32>,
    /// H1.
    pub stager: ProcessIdentity,
    /// H2, from `ready` on.
    pub runner: Option<ProcessIdentity>,
    /// The NSIS process, from `installing` on.
    pub installer: Option<ProcessIdentity>,
}
impl RunRecord {
    pub fn to_json(&self) -> String;
    pub fn from_json(text: &str) -> Result<RunRecord, StateError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgramKind {
    Gui,
    Cli,
    Helper,
}

/// NSIS exit codes (design m5b D.9.1); mirrored by `!define MKLM_EXIT_*` in mklm.nsi.
pub mod nsis_exit {
    pub const OS_TOO_OLD: u32 = 20;
    pub const WRONG_ARCH: u32 = 21;
    pub const HELPER_RUNNING: u32 = 22;
    pub const CLI_RUNNING: u32 = 23;
    pub const GUI_RUNNING: u32 = 24;
    pub const BAD_INSTALL_DIR: u32 = 25;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallerExit {
    Success,       // 0
    UserCancelled, // 1
    ScriptAborted, // 2
    OsTooOld,      // 20
    WrongArch,     // 21
    HelperRunning, // 22
    CliRunning,    // 23
    GuiRunning,    // 24
    BadInstallDir, // 25
    Other(u32),
}
impl InstallerExit {
    /// 20..=25: refused before any file was replaced.
    pub fn is_refusal(self) -> bool;
}
pub fn classify_installer_exit(code: u32) -> InstallerExit;

/// Build IDs (VERSIONINFO `MKLMBuildId`) of the three installed executables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallState {
    pub gui: Option<String>,
    pub cli: Option<String>,
    pub helper: Option<String>,
}
impl InstallState {
    /// The version part of the build ID when all three exist and are equal.
    pub fn consistent_version(&self) -> Option<Version>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotInstalledReason {
    Refused(UpdateRefusal),
    CallerDidNotExit,
    InstanceBusy,
    ProgramsStillRunning { programs: Vec<ProgramKind> },
    InstalledVersionChanged { found: Option<String> },
    InstallerNotStarted { code: u32 },
    InstallerRefused { exit: InstallerExit },
    InstallerExit { code: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum FailedReason {
    InstallerTimedOut,
    Inconsistent,
    UnexpectedVersion { found: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum UpdateOutcome {
    Installed,
    NotInstalled(NotInstalledReason),
    Failed(FailedReason),
    Interrupted { phase: RunPhase },
}

/// `HKLM\…\MKLM\Update\LastResult` (design m5b D.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateResult {
    pub schema: u32, // 1
    pub run_id: RunId,
    pub from_version: String,
    pub to_version: String,
    pub arch: Arch,
    pub finished_at: Timestamp,
    pub outcome: UpdateOutcome,
    pub installer_exit: Option<u32>,
    /// `InstallState::consistent_version` afterwards; `None` when inconsistent or unknown.
    pub installed_version: Option<String>,
    pub gui_relaunch_attempted: bool,
}
impl UpdateResult {
    pub fn to_json(&self) -> String;
    pub fn from_json(text: &str) -> Result<UpdateResult, StateError>;
}

/// The table of design m5b D.13.
pub fn decide_outcome(
    from: &Version,
    to: &Version,
    exit: Option<u32>,
    timed_out: bool,
    after: &InstallState,
) -> UpdateOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunView {
    Idle,
    InProgress { phase: RunPhase, to_version: String },
    Interrupted(RunRecord),
}

/// `Run` as the GUI, the CLI and H1 see it: the owner of the phase (`stager` up to `staged`,
/// `runner` from `ready`) alive in this boot → `InProgress`; otherwise (boot changed, owner dead
/// or unknown) → `Interrupted`. `Done` or no record → `Idle`.
pub fn classify_run(
    run: Option<&RunRecord>,
    current_boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> RunView;

/// The `LastResult` for an interrupted record (`UpdateOutcome::Interrupted`, the installed
/// version from `after`).
pub fn interrupted_result(record: &RunRecord, now: Timestamp, after: &InstallState) -> UpdateResult;

/// Waits and deadlines of the update run (design m5b D.4, D.7, D.8).
pub mod timing {
    use std::time::Duration;
    pub const LOCK_WAIT_STAGER: Duration = Duration::from_secs(10);
    pub const CHUNK_WAIT: Duration = Duration::from_secs(30);
    pub const STAGE_TOTAL: Duration = Duration::from_secs(10 * 60);
    pub const READY_WAIT: Duration = Duration::from_secs(20);
    pub const STAGER_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const LOCK_WAIT_RUNNER: Duration = Duration::from_secs(60);
    pub const CALLER_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const INSTANCE_QUIT_WAIT: Duration = Duration::from_secs(5);
    pub const PROGRAMS_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const HELPERS_EXIT_WAIT: Duration = Duration::from_secs(75);
    pub const INSTALLER_WAIT: Duration = Duration::from_secs(15 * 60);
    pub const RELAUNCH_WAIT: Duration = Duration::from_secs(10);
    /// Caller: after the last chunk, the longest wait for `HandedOff`.
    pub const HANDOFF_WAIT: Duration = Duration::from_secs(90);
}
```

```rust
// crates/mklm-update/src/gate.rs  (WP-H)
/// Unreadable entries → `JournalUnreadable`; an in-flight entry → `RecoveryNeeded`; any other
/// open entry → `OperationOpen { waiting_for_reboot }` (true when one is `PendingReboot`).
pub fn check_journal(journal: &mklm_core::Journal) -> Result<(), UpdateRefusal>;
```

```rust
// crates/mklm-update/src/winhttp.rs  (feature "winhttp", Windows; WP-U)
#[derive(Debug)]
pub struct WinHttpTransport { /* mklm_win::net::HttpSession */ }
impl WinHttpTransport {
    /// Automatic proxy (design m5b A.7).
    pub fn new(user_agent: &str) -> Result<WinHttpTransport, TransportError>;
    /// No proxy, for the loopback tests and the debug rehearsal.
    #[cfg(debug_assertions)]
    pub fn new_without_proxy(user_agent: &str) -> Result<WinHttpTransport, TransportError>;
}
impl Transport for WinHttpTransport { /* … */ }
```

### H.2 `mklm-ipc`

```rust
// crates/mklm-ipc/src/lib.rs
/// 3 (M5b, design m5b D.3): `CallerMessage::{StageUpdate, InstallerChunk}`,
/// `HelperMessage::Update`.
pub const PROTOCOL_VERSION: u32 = 3;
pub mod staging;
pub mod update;
pub use update::*;
```

```rust
// crates/mklm-ipc/src/message.rs (additions)
pub enum CallerMessage {
    Welcome(Welcome),
    Request(Request),
    Decision(Decision),
    Bye,
    StageUpdate(StageUpdateRequest),
    InstallerChunk(InstallerChunk),
}

pub enum HelperMessage {
    Hello(Hello),
    Event(Event),
    Result(OperationResult),
    Error(ErrorInfo),
    Update(UpdateMessage),
}
// (derives and #[serde(tag = "type", content = "data", rename_all = "kebab-case",
//  deny_unknown_fields)] unchanged: "stage-update", "installer-chunk", "update")
```

```rust
// crates/mklm-ipc/src/update.rs
pub use mklm_update::stage::CHUNK_LEN;
pub use mklm_update::UpdateRefusal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageUpdateRequest {
    /// latest.json exactly as downloaded.
    pub manifest: String,
    /// latest.json.minisig exactly as downloaded.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallerChunk {
    pub offset: u64,
    /// Lower-case hex of 1..=CHUNK_LEN bytes.
    pub hex: String,
}
impl InstallerChunk {
    pub fn new(offset: u64, bytes: &[u8]) -> InstallerChunk;
    /// `ChunkMalformed` for upper case, odd length, other characters, 0 or > CHUNK_LEN bytes.
    pub fn decode(&self) -> Result<Vec<u8>, UpdateRefusal>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UpdateMessage {
    SendInstaller { name: String, size: u64, sha256: String, chunk_len: u32 },
    Received { bytes: u64 },
    HandedOff { run_id: String, to_version: String },
    Refused(UpdateRefusal),
}

/// Lower-case hex.
pub fn encode_hex(bytes: &[u8]) -> String;
/// Lower-case hex only, even length; `None` otherwise.
pub fn decode_hex(text: &str) -> Option<Vec<u8>>;
```

```rust
// crates/mklm-ipc/src/args.rs (additions)
pub const RUN_UPDATE_FLAG: &str = "--run-update";
/// Anchored. Plus: each number is `0` or has no leading zero, and is at most 65535.
pub const RUN_UPDATE_PATTERN: &str =
    r"^--run-update [0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}-[0-9a-f]{16}$";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunUpdateArgs {
    pub run_id: mklm_update::run::RunId,
}
impl RunUpdateArgs {
    pub fn parse(tail: &str) -> Result<RunUpdateArgs, ArgsError>;
    /// `--run-update <run-id>`: what `parse` accepts.
    pub fn to_parameters(&self) -> String;
}
/// A raw `GetCommandLineW` string → the arguments, when the tail is exactly the fixed form.
pub fn run_update_args(command_line: &str) -> Option<RunUpdateArgs>;
```

```rust
// crates/mklm-ipc/src/staging.rs  (WP-H)
use std::time::Duration;
use mklm_update::stage::Stager;
use mklm_update::Sha256Digest;

pub trait StageLink {
    fn send(&mut self, message: HelperMessage) -> Result<(), String>;
    fn recv(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError>;
}

pub trait StageSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagingEnd {
    /// Every byte arrived and matched; the digest of what was written.
    Complete(Sha256Digest),
    /// Already sent to the caller as `UpdateMessage::Refused`.
    Refused(UpdateRefusal),
    /// `Bye`, a closed pipe or a protocol violation.
    CallerLeft,
}

/// Sends `SendInstaller`, then reads `InstallerChunk`s into `sink` through `stager`, sending
/// `Received` every `progress_every` bytes (design m5b D.4 steps 12–13).
pub fn receive_installer(
    link: &mut dyn StageLink,
    sink: &mut dyn StageSink,
    stager: Stager,
    chunk_wait: Duration,
    total: Duration,
    progress_every: u64,
) -> StagingEnd;
```

### H.3 `mklm-win`

```rust
// crates/mklm-win/src/net.rs  (feature "net"; WP-U)
#[derive(Debug)]
pub struct HttpSession { /* WinHttpOpen handle */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpTimeouts {
    pub resolve_ms: i32,
    pub connect_ms: i32,
    pub send_ms: i32,
    pub receive_ms: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct HttpGet<'a> {
    /// false only in debug builds and only to 127.0.0.1 (refused otherwise).
    pub secure: bool,
    pub host: &'a str,
    pub port: u16,
    pub path_and_query: &'a str,
    pub accept: &'a str,
    pub timeouts: HttpTimeouts,
}

#[derive(Debug)]
pub struct HttpResponse<'s> { /* request handle, borrows the session */ }

impl HttpSession {
    /// `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, TLS 1.2/1.3, no cookies, no automatic redirects,
    /// no decompression (design m5b A.7).
    pub fn open(user_agent: &str) -> Result<HttpSession, NetError>;
    #[cfg(debug_assertions)]
    pub fn open_direct(user_agent: &str) -> Result<HttpSession, NetError>;
    pub fn get(&self, request: &HttpGet<'_>) -> Result<HttpResponse<'_>, NetError>;
}

impl HttpResponse<'_> {
    pub fn status(&self) -> u16;
    pub fn header(&self, name: &str) -> Option<String>;
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetErrorKind {
    Timeout,
    NameNotResolved,
    CannotConnect,
    Tls,
    ProxyAuth,
    Cancelled,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{function} failed with WinHTTP error {code}")]
pub struct NetError {
    pub function: &'static str,
    pub code: u32,
    pub kind: NetErrorKind,
}
```

```rust
// crates/mklm-win/src/update_store.rs  (WP-H)
pub const UPDATE_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Update";
pub const TRUST_VALUE: &str = "Trust";
pub const RUN_VALUE: &str = "Run";
pub const LAST_RESULT_VALUE: &str = "LastResult";
pub const UPDATE_VALUE_NAMES: [&str; 3] = [TRUST_VALUE, RUN_VALUE, LAST_RESULT_VALUE];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawUpdateStore {
    pub trust: Option<String>,
    pub run: Option<String>,
    pub last_result: Option<String>,
}

/// Unelevated read; a missing key is the default.
pub fn read_update_store() -> Result<RawUpdateStore, Error>;

/// Helper only: the key with `JOURNAL_KEY_SDDL`, owner and DACL of every level verified.
#[derive(Debug)]
pub struct UpdateStore { /* key handle */ }
impl UpdateStore {
    pub fn open_or_create() -> Result<UpdateStore, Error>;
    pub fn read(&self) -> Result<RawUpdateStore, Error>;
    /// REG_SZ, then `RegFlushKey`. `name` must be in `UPDATE_VALUE_NAMES` (`ValueNotAllowed`).
    pub fn write(&self, name: &str, json: &str) -> Result<(), Error>;
    /// Absent is success; flushed.
    pub fn delete(&self, name: &str) -> Result<(), Error>;
}
```

```rust
// crates/mklm-win/src/update_dir.rs  (WP-H)
use crate::protected_dir::ProtectedDir;

/// One `Updates\<run-id>` folder, validated and pinned.
#[derive(Debug)]
pub struct RunDir { /* path, pins */ }
impl RunDir {
    /// `CreateDirectoryW` with `PRIVATE_DIR_SDDL`; fails if it exists.
    pub fn create(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error>;
    /// Validates owner, DACL and no reparse point, and pins it.
    pub fn open(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error>;
    pub fn path(&self) -> &std::path::Path;
    /// `CREATE_NEW`, write, `FlushFileBuffers`.
    pub fn write_new(&self, file: &str, bytes: &[u8]) -> Result<(), Error>;
    /// `CREATE_NEW`, no sharing, for streaming.
    pub fn create_exclusive(&self, file: &str) -> Result<StagedFile, Error>;
    /// A regular file (not a reparse point) of at most `max_len` bytes, opened with
    /// `FILE_SHARE_READ` only.
    pub fn open_locked(&self, file: &str, max_len: u64) -> Result<LockedFile, Error>;
    /// Copies `source` in as `file` (`CREATE_NEW`, flushed).
    pub fn copy_in(&self, source: &std::path::Path, file: &str) -> Result<(), Error>;
    pub fn remove_file(&self, file: &str) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct StagedFile { /* handle; deleted on drop unless committed */ }
impl std::io::Write for StagedFile { /* … */ }
impl StagedFile {
    /// `FlushFileBuffers` and close.
    pub fn commit(self) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct LockedFile { /* handle */ }
impl std::io::Read for LockedFile { /* … */ }
impl LockedFile {
    pub fn len(&self) -> u64;
    pub fn path(&self) -> &std::path::Path;
}

pub fn list_run_dirs(updates: &ProtectedDir) -> Result<Vec<String>, Error>;
/// Regular files only, never following reparse points.
pub fn remove_run_dir(updates: &ProtectedDir, name: &str) -> Result<(), Error>;
/// `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)`.
pub fn remove_at_reboot(path: &std::path::Path) -> Result<(), Error>;
```

```rust
// crates/mklm-win/src/os.rs (additions, WP-H)
/// Relative to FOLDERID_ProgramFiles; NSIS's fixed `$INSTDIR`.
pub const INSTALL_SUBDIR: &str = r"SHIN DATA CENTER\MKLM";
/// `SHGetKnownFolderPath(FOLDERID_ProgramFiles)` + `INSTALL_SUBDIR`.
pub fn fixed_install_dir() -> Result<std::path::PathBuf, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMachine {
    X64,
    Arm64,
    Other(u16),
}
/// `IsWow64Process2(GetCurrentProcess())`'s native machine.
pub fn native_machine() -> Result<NativeMachine, Error>;
```

```rust
// crates/mklm-win/src/proc_identity.rs (additions, WP-H)
/// PID → identity with its creation time (SystemProcessInformation); `None` if gone.
pub fn process_identity(pid: u32) -> Result<Option<ProcessIdentity>, Error>;
/// NT path of a file (`GetFinalPathNameByHandleW(VOLUME_NAME_NT)`), for comparing with
/// `process_image_nt_path`.
pub fn file_nt_path(path: &std::path::Path) -> Result<String, Error>;
/// Every process whose image NT path equals one of `nt_paths` (case-insensitive), with the
/// index of the path.
pub fn processes_with_images(nt_paths: &[String]) -> Result<Vec<(ProcessIdentity, usize)>, Error>;
/// Polls `process_liveness` every 250 ms; true when all are gone within `timeout`.
pub fn wait_for_exit(processes: &[ProcessIdentity], timeout: std::time::Duration) -> Result<bool, Error>;
```

```rust
// crates/mklm-win/src/instance.rs (additions, WP-H)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAnswer {
    Ok,
    Busy,
    /// The pipe's server is not `gui_nt_path`: nothing was sent.
    NotOurs,
    NoAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceQuit {
    pub pipe: String,
    pub server_pid: Option<u32>,
    pub answer: QuitAnswer,
}

/// Every `SHINDATACENTER.MKLM.Instance.*` pipe whose server runs `gui_nt_path` gets `quit`
/// (design m5b D.8).
pub fn quit_all_instances(gui_nt_path: &str, timeout: std::time::Duration)
    -> Result<Vec<InstanceQuit>, Error>;
```

```rust
// crates/mklm-win/src/shell_launch.rs  (WP-H)
/// Starts `exe arguments` through the desktop shell of this session (IShellWindows →
/// IShellDispatch2::ShellExecute): as the signed-in user, unelevated. Gives up after `timeout`.
/// Never falls back to this process's own token.
pub fn launch_via_shell(
    exe: &std::path::Path,
    arguments: &str,
    directory: &std::path::Path,
    timeout: std::time::Duration,
) -> Result<(), Error>;
```

```rust
// crates/mklm-win/src/session.rs (additions, WP-C)
pub const AFTER_UPDATE_VALUE: &str = "SHINDATACENTER.MKLM.AfterUpdate";
/// HKCU RunOnce `SHINDATACENTER.MKLM.AfterUpdate` = `command_line`.
pub fn register_after_update(command_line: &str) -> Result<(), Error>;
pub fn unregister_after_update() -> Result<(), Error>;

// crates/mklm-win/src/user_dirs.rs  (WP-C)
/// `SHGetKnownFolderPath(FOLDERID_LocalAppData)`.
pub fn local_app_data_dir() -> Result<std::path::PathBuf, Error>;
/// `<LocalAppData>\SHIN DATA CENTER\MKLM\update`, created if missing.
pub fn update_cache_dir() -> Result<std::path::PathBuf, Error>;

// crates/mklm-win/src/ui/open_url.rs  (feature "gui", WP-C)
/// Opens `<repository>/releases/tag/v<version>`; `version` must be `X.Y.Z` digits.
pub fn open_release_page(version: &str) -> Result<(), Error>;
```

既存のものを使う: `elevation::{file_build_id, spawn_from_elevated, is_elevated, system_directory, process_command_line}`、`protected_dir::{ensure_protected_dir, verify_protected_dir, DataDir::{Base, Updates, Logs}, FileLock, PRIVATE_DIR_SDDL}`、`journal_store::{read_journal_store, JOURNAL_KEY_SDDL}`、`proc_identity::{process_liveness, process_image_nt_path, current_process_identity}`、`session::{boot_id, random_bytes, new_uuid}`。

### H.4 `mklm-client`

```rust
// crates/mklm-client/src/update/mod.rs
pub mod cache;
pub mod check;
pub mod download;
pub mod stage;
#[cfg(windows)]
pub mod env;
#[cfg(windows)]
pub mod status;
```

```rust
// update/env.rs (Windows)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    NotConfigured,
    NotInstalledCopy { exe_dir: PathBuf, install_dir: PathBuf },
    Unknown { detail: String },
}

#[derive(Debug)]
pub struct UpdateEnv {
    pub installed: Version,
    pub arch: Arch,
    pub native: Option<NativeMachine>,
    pub availability: Availability,
    pub anchors: Option<TrustAnchors>,
    pub endpoints: Endpoints,
    pub install_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// `app_version` = the front end's `CARGO_PKG_VERSION`; `endpoints` = `Endpoints::production()`
/// (debug builds: maybe the `--update-endpoint` override).
pub fn environment(app_version: &str, endpoints: Endpoints) -> UpdateEnv;
```

```rust
// update/cache.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientState {
    pub schema: u32, // 1
    #[serde(default)]
    pub trust: TrustState,
    #[serde(default)]
    pub last_check: Option<u64>,
    #[serde(default)]
    pub last_success: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UpdateCache { /* dir */ }
impl UpdateCache {
    pub fn new(dir: PathBuf) -> UpdateCache;
    /// Missing or unreadable → default.
    pub fn load_state(&self) -> ClientState;
    /// Temporary file, then rename.
    pub fn save_state(&self, state: &ClientState) -> std::io::Result<()>;
    pub fn store_manifest(&self, manifest: &[u8], signature: &[u8]) -> std::io::Result<()>;
    pub fn load_manifest(&self) -> Option<(Vec<u8>, Vec<u8>)>;
    pub fn installer_path(&self, name: &str) -> PathBuf;
    pub fn partial_path(&self, name: &str) -> PathBuf;
    /// Deletes every installer and `.part` except `keep_installer`.
    pub fn prune(&self, keep_installer: Option<&str>) -> std::io::Result<()>;
}
```

```rust
// update/check.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub verified: VerifiedManifest,
    pub manifest: Vec<u8>,
    pub signature: Vec<u8>,
    pub skipped: bool,
    /// The cached installer whose size and SHA-256 match, if any.
    pub downloaded: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate(VerifiedManifest),
    Available(Offer),
    ManualRequired(VerifiedManifest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    Unavailable(Availability),
    Fetch(FetchError),
    Refused(UpdateRefusal),
    Cache(String),
}

/// Fetch, verify (`Purpose::Check`, machine ∪ user state), record the user state, cache the
/// manifest when an update is available. Never downloads the installer.
pub fn check(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
    cancel: &AtomicBool,
) -> Result<CheckOutcome, CheckError>;

/// The cached manifest verified again (before "update now", after a restart).
pub fn reverify_cached(
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
) -> Result<CheckOutcome, CheckError>;
```

```rust
// update/download.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Fetch(FetchError),
    Cache(String),
}

/// Into `<name>.part`, verified, then renamed to `<name>`; `progress(received, total)`.
pub fn download(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    offer: &Offer,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, DownloadError>;
```

```rust
// update/stage.rs
pub trait StageFrontend {
    fn sent(&mut self, bytes: u64, total: u64);
    fn received(&mut self, bytes: u64);
    /// Honoured until the last chunk is sent.
    fn cancel_requested(&mut self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageEnd {
    /// The caller must quit now (design m5b E.4).
    HandedOff { run_id: String, to_version: String },
    Refused(UpdateRefusal),
    Cancelled,
    /// The cached installer changed while it was sent (SHA-256 differs).
    SourceChanged,
    Lost(String),
    Unresponsive,
    Protocol(String),
}

/// Over a connected helper link: `StageUpdate`, then the installer in `CHUNK_LEN` pieces
/// (design m5b D.3). Sends `Bye` itself except after `HandedOff`.
pub fn stage(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    frontend: &mut dyn StageFrontend,
) -> StageEnd;
```

```rust
// update/status.rs (Windows)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub machine_trust: TrustState,
    pub run: RunView,
    pub last_result: Option<UpdateResult>,
    pub install: InstallState,
    /// English diagnostics of what could not be read (the fields then hold defaults).
    pub warnings: Vec<String>,
}

/// Unelevated: `read_update_store`, `classify_run` (boot ID, process liveness), the build IDs of
/// the three executables in `install_dir`.
pub fn read_status(install_dir: &Path) -> UpdateStatus;
```

### H.5 コマンドライン、ファイル、レジストリ、JSON

| もの | 形 |
|---|---|
| H2 のコマンドライン | `"<ProgramData>\SHIN DATA CENTER\MKLM\Updates\<run-id>\mklm-helper.exe" --run-update <run-id>`（`RUN_UPDATE_PATTERN`） |
| GUI の引数 | `--after-update`（新しい起動の形。窓を出して更新の結果を前面に。`--post-reboot` と同じ優先順位の扱い）。デバッグ ビルドだけ `--update-endpoint=http://127.0.0.1:<port>` |
| RunOnce | `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce\SHINDATACENTER.MKLM.AfterUpdate` = `"<INSTDIR>\mklm.exe" --after-update` |
| 機械のフォルダー | `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>\{latest.json, latest.json.minisig, MKLM-Setup-<v>-<arch>.exe, mklm-helper.exe}`、`…\MKLM\logs\update.log` |
| 利用者のフォルダー | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\{state.json, latest.json, latest.json.minisig, MKLM-Setup-<v>-<arch>.exe[.part]}` |
| レジストリ | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` の `Trust`、`Run`、`LastResult`（REG_SZ、JSON） |
| 設定 | `settings.toml` の `[update]`: `auto_check`、`skipped_version`、`result_seen` |

JSON の例（形を固定するテストの期待値に使う）:

```json
// Trust
{"schema":1,"max_issued_at":{"8F1A2B3C4D5E6F70":1792022400},"revoked":[]}
```

```json
// Run
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","phase":"installing","boot_id":"0b6d3c2a-9e1f-4d5a-8c7b-6a5f4e3d2c1b","started_at":1792022460000,"phase_at":1792022485000,"caller":{"pid":8532,"creation_time":134041234567890123},"caller_session":1,"stager":{"pid":9120,"creation_time":134041234600000000},"runner":{"pid":9344,"creation_time":134041234700000000},"installer":{"pid":9512,"creation_time":134041234800000000}}
```

```json
// LastResult: installed
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"installed"},"installer_exit":0,"installed_version":"0.2.1","gui_relaunch_attempted":true}
```

```json
// LastResult: not installed, another user's MKLM was busy
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"instance-busy"}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// LastResult: refused on re-verification (H2)
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"refused","code":"operation-open","waiting_for_reboot":true}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// pipe frames (PROTOCOL_VERSION 3)
{"v":3,"seq":2,"body":{"type":"stage-update","data":{"manifest":"{\n  \"schema\": 1, …","signature":"untrusted comment: …"}}}
{"v":3,"seq":2,"body":{"type":"update","data":{"kind":"send-installer","data":{"name":"MKLM-Setup-0.2.1-x64.exe","size":6291456,"sha256":"3a7b…","chunk_len":65536}}}}
{"v":3,"seq":3,"body":{"type":"installer-chunk","data":{"offset":0,"hex":"4d5a9000…"}}}
{"v":3,"seq":9,"body":{"type":"update","data":{"kind":"handed-off","data":{"run_id":"0.2.1-3f9a0c2b7d1e4a65","to_version":"0.2.1"}}}}
{"v":3,"seq":3,"body":{"type":"update","data":{"kind":"refused","data":{"code":"rollback","issued_at":1792022400,"seen":1792108800}}}}
```

---

## I. 未解決の問題

1. **`CreateProcessW` と読み取り共有のハンドル**（D.7 の 5）: `FILE_SHARE_READ` だけで開いたままのインストーラーを `CreateProcessW` で起動できるはず（ローダーは読み取りと実行の共有で開き、実行は共有の検査では読み取りとして扱われる）だが、実機で確かめていない。できなければ、ハンドルを閉じてから起動し、起動したプロセスのイメージのパスとファイルの ID（`GetFileInformationByHandleEx(FileIdInfo)`）が検証したものと一致することを確かめる方式に変える。F.6 で確かめる。
2. **別の管理者で昇格したときの起動し直し**（D.10）: 届くかどうか未確認。届かなければ RunOnce と Run キーに任せる。F.7 の T-UPD-8。
3. **NSIS の終了コード**（D.9.1）: `SetErrorLevel` の直後の `Quit` でその値が返ることは、D.9.2 の煙の試験で確かめる。`.onInit` の `Quit` でも同じかは未確認。
4. **`gh` と下書き**（B.5）: `gh release download / upload / edit` がタグ名で下書きを扱えるか未確認。
5. **immutable なリリースの「最新」の印**（B.4 の 6）: 公開後に別のリリースへ「最新」を移せるか未確認。
6. **ARM64 のネイティブの判定**（C.7）: x64 のエミュレーションの中で `IsWow64Process2` が ARM64 を返すことは広く報告されているが、実機で確かめていない（ARM64 の PC がない）。今の設計では表示にしか使わない。
7. **サインアウトとインストール**（D.12）: インストールの数秒の間のサインアウトは止めない。止める仕組み（`ShutdownBlockReasonCreate`）は、必要なら後で足す。
8. **`ProgramData` の先回り**（m2 I.18）: `Updates` も先回りして作られうる。隔離の仕組み（m2 S1）がそのまま効くが、隔離できない場合は更新も `Storage` で止まる。
9. **従量制の接続**（E.1）: ダウンロードを自動で行う。問題になれば、`NetworkInformation` の接続の費用を見て止める。

---

## J. ユーザーに決めてもらうこと

| # | 質問 | 選択肢 | おすすめ |
|---|---|---|---|
| 1 | ARM64 の PC で x64 版を使っている利用者を、自動更新で ARM64 版に切り替えるか | (a) 切り替えない（今のアーキテクチャのまま） (b) 自動で切り替える (c) 画面で尋ねる | (a)。ARM64 版は実機で試していない（M6 で試した後に見直す） |
| 2 | 更新のとき、ほかの利用者（ユーザーの切り替え）の MKLM が何もせず動いていたら | (a) 終わらせて更新する（その人の次のサインインで戻る） (b) 更新をやめて「ほかの人に終了してもらって」と出す | (a)。キーボードの操作の途中なら、どちらでも更新をやめる |
| 3 | 更新情報の有効期限 | (a) 400 日（年 1 回のリリースで切れない） (b) 180 日 (c) 2 年 | (a)。加えて、年 1 回の保守リリースを目安にする |
| 4 | CLI からインストールできるようにするか | (a) 確認と状態の表示だけ (b) インストールもできる | (a)。スクリプトはインストーラーを直接 `/S` で実行できる |
| 5 | GitHub の immutable releases をいつ有効にするか | (a) v0.2.0 から (b) 手順に慣れてから | (a)。この設計は公開後にファイルを変えないので、影響は「間違いを直せない」ことだけ（B.4 の 6） |
| 6 | 署名をする PC | (a) 普段の開発機（鍵は USB に保管し、署名のときだけつなぐ） (b) 専用のオフラインの PC | (a)。パート タイムの 1 人のメンテナーには (b) は重い |
| 7 | 従量制の接続でも自動でダウンロードするか | (a) する（数 MB） (b) しない | (a) |

最初の更新対応版（v0.2.0）の前に、メンテナーが鍵を作る必要がある（B.5 の準備）。

---

## K. 計画・依頼からの変更点

| # | 計画・依頼 | この設計 | 理由 |
|---|---|---|---|
| 1 | MSI と msiexec（計画 4.1〜4.2） | NSIS の `/S` と、決めた終了コード（D.9） | 2026-09-28 のユーザーの決定 |
| 2 | 依頼: 新しい要求は「ダウンロードしたファイルのパス」を運ぶ | インストーラーのバイト列をパイプで 64 KiB ずつ運ぶ（D.3） | 昇格した helper が利用者の書ける場所を開くと、シンボリック リンクによる UNC への誘導（NTLM の中継）、デバイスへの誘導、差し替えの危険がある。計画 4.2 の手順 2（バイト列だけを受け取る）と計画 2.2（HKCU と `%APPDATA%` を読まない）にも合う |
| 3 | 結果は `Updates\last-result.json`（計画 4.2 の 9、依頼） | HKLM の `Update\LastResult`（と `Run`、`Trust`）（D.6） | `Updates` は非昇格の GUI が読めない。読めるファイルにすると読み手が書き換えを止められる。レジストリの値は原子的に書け、先回りされない |
| 4 | インストール済みの版は `MsiGetProductInfo`（計画 4.2 の 3） | helper 自身の版（= インストールした版）と、インストール先の exe のビルド ID（D.7 の 11、D.13） | NSIS に対応するものがない。ビルド ID はすでに埋め込まれている |
| 5 | アセットに `culture` と `url`、UpgradeCode の照合（計画 4.2） | `culture` はない（インストーラーは日英を含む）。`url` は持たず、版と名前から作る（A.5、C.8）。UpgradeCode の代わりに `product` と名前 | NSIS は 1 つのインストーラーで両言語。URL を署名の中に持たせないと、鍵が漏れてもダウンロード先を変えられない |
| 6 | `expires` を過ぎたら「更新を確認できません」（計画 4.2） | 情報として知らせるだけで、インストールは止めない（B.4、C.6） | immutable releases では有効期限を延ばすのに新しいリリースが要る。期限を門にすると、メンテナーが動けないだけで正しい更新も止まる |
| 7 | GitHub の asset `digest` を API で照合（計画 4.2） | しない | REST API を使わない方針（レート制限）。SHA-256 は署名した更新情報にある |
| 8 | `xtask sign-release` が `gh attestation verify` を行う（計画 4.3） | チェックリストの任意の手順にする。来歴の証明を release.yml に足すのは任意（G.6） | 今の release.yml に `actions/attest` がない。署名の正しさは来歴の証明に頼らない |
| 9 | helper は HKLM に記録したインストール先から起動する（計画 2.2、m2 E.1） | 固定のインストール先（`%ProgramFiles%\SHIN DATA CENTER\MKLM`）で動いていることを確かめる（D.4 の 1） | NSIS はインストール先を固定し、HKLM の製品のキーに書かない（M5a） |
| 10 | 更新はロックを持ったまま msiexec を実行する（m2 D.9） | H1 と H2 がそれぞれロックを取り、H2 はジャーナルを確かめ直す（D.4、D.7） | ロックはプロセスをまたいで渡せない（`LockFileEx` の鍵はハンドルとプロセスに属する） |
| 11 | 起動し直しの予備は HKCU の RunOnce（計画 4.2 の 8） | 同じ。RunOnce は GUI が引き継ぎの前に自分で登録する（D.10） | helper は HKCU に書かない |
| 12 | 確認は起動時と 24 時間ごと（計画 4.2） | 起動の 60〜180 秒後（前回から 24 時間以上なら）と、24 時間 + 0〜60 分ごと（E.1） | サインイン直後の負荷を避け、利用者の間で時刻を散らす |

---

## L. 確認できていない事実

- `minisign-verify` 0.3.0 と `minisign` 0.10.0（どちらも 2026-09-25 公開）の、前の版からの変更点。`minisign` クレートの `KeyPair::generate_unencrypted_keypair` の有無と、`sign` が prehashed の署名を作るか。docs.rs で確かめたのは B.1 に挙げた `minisign-verify` の関数だけ。
- minisign の鍵 ID の表記（little-endian の u64 の 16 進）が、公式のコマンドの表示と同じか。
- `sha2` 0.11.0 と `minisign-verify` 0.3.0 の最低の Rust の版（ツールチェーンは 1.98.1 なので問題ないはず）。
- `windows` 0.62.2 の feature 名: `Win32_Networking_WinHttp`（`windows::Win32::Networking::WinHttp` のモジュールはドキュメントで確かめた）、`Win32_System_Ole`、`Win32_System_Variant`、`IShellDispatch2` などの置き場所。
- `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY` がループバックの要求をプロキシに送らないか（テストはプロキシなしのセッションを使うので影響しない）。同期モードの要求を別のスレッドから `WinHttpCloseHandle` で取り消せるか（この設計は頼らない）。
- NSIS の `Quit` の既定の終了コード（`SetErrorLevel` なし）と、`.onInit` での `SetErrorLevel` + `Quit` の振る舞い。`File` の書き込みに失敗したときのサイレントの動作と終了コード。
- `gh release` の各コマンドが、タグ名で下書きのリリースを扱えるか。
- immutable なリリースでも「最新」の印を別のリリースに移せるか。
- 昇格したプロセスから `IShellWindows` / `IShellDispatch2` で起動し直す方法が、Windows 11 25H2 で同じ利用者のときに動くこと（M5a では NSIS の `explorer.exe <path>` の方法を確かめた）と、別の管理者のときの振る舞い。
- `FILE_SHARE_READ` だけのハンドルを開いたまま、そのファイルを `CreateProcessW` で起動できること（I.1）。
- `IsWow64Process2` がエミュレーション中の x64 のプロセスで `IMAGE_FILE_MACHINE_ARM64` を返すこと。
- UAC で起動した helper（H1）がジョブ オブジェクトに入っていないこと（G.7 の 12）。
- reqwest が Windows のレジストリのプロキシ設定を読むが PAC を扱わないこと（A.9 の比較の 1 項目。決定には影響しない）。
- GitHub のリリースのファイルの配布元が、今後も `*.githubusercontent.com` であること（2026-09-29 の実測は `release-assets.githubusercontent.com`）。
