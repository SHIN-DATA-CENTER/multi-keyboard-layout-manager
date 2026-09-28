# M5b 設計: 自動更新（mklm-update / helper の更新要求 / NSIS / リリースの署名）

| 項目 | 内容 |
|---|---|
| 対象 | マイルストーン M5 の後半（M5b）: アップデーターとリリースの署名（計画 4.2〜4.4、6 章の M5） |
| 根拠 | 承認済みプラン 2.1〜2.2、4.2〜4.4、5 章（4.x は MSI 向けに書かれている。インストーラーは 2026-09-28 のユーザーの決定で NSIS）。M2 設計（`docs/design/m2-engine.md`）の D.9、E、G.1、I.18。M3 設計（`docs/design/m3-gui.md`）の A.4、B.5、B.17、D、E、F。M5a の実機テスト（`docs/research/m5-install-tests.md`）。main の 37989f8 のコード |
| ユーザーの決定（M5b の依頼） | 完全な自動更新（ダウンロード、署名の検証、サイレント インストール）。minisign で署名した `latest.json`。**秘密鍵はメンテナーがオフラインで保管し、GitHub の secrets には置かない**。公開鍵を 2 本（通常用とバックアップ用）埋め込み、鍵 ID、失効（`revoked_keys`）、`issued_at`（巻き戻しの防止）、`expires`（凍結の検知）を持つ。確認とダウンロードは自動、インストールは利用者がボタンを押したときだけ。UAC の事前説明あり（未署名のため）。当面バイナリは署名しない。インストーラーは NSIS 3.12（WiX は使わない）。「まず使えるもの」を優先するが、更新の経路は安全に直結するので、検証の正しさは譲らない |
| 状態 | 設計。**レビュー第 1 回（47 件）と第 2 回（20 件: 第 1 回の対応の検証 17 件、新しい攻撃の検討 3 件）を反映した版**（対応は M 章の 2 つの表）。**2026-09-29 のユーザーの決定（J 章の J-1〜J-9）を反映した**（有効期限の既定 180 日、署名は普段のアカウント、など）。骨組み（G.2 の WP-0）は次の段階 |
| 読み手 | M5b を分担して実装する人（G 章、H 章）とレビューする人。指摘に使えるよう、すべての節に番号を付けた |

識別子、コード、コマンドは英語のまま書く。「計画」は承認済みプラン、「m2 D.9」「m3 F.5」は M2 / M3 設計の節を指す。H1 と H2 は D 章で定義する helper の 2 つのプロセスを指す。「未確認」と書いたものは L 章にまとめた。レビューの指摘は「（SECURITY-1）」のように ID で引く。

---

## 0. 前提と方針

### 0.1 main（37989f8）にすでにあるもの

| もの | 内容 | M5b への影響 |
|---|---|---|
| NSIS のインストーラー（`installer/nsis/mklm.nsi`） | `%ProgramFiles%\SHIN DATA CENTER\MKLM` に固定。`/S` でサイレント。実行中の検査はパスで行う: `$INSTDIR\mklm-helper.exe` と `$INSTDIR\mklm-cli.exe` を追記モードで開けなければ、MessageBox（`/SD` の既定）を出して `Quit`。動いている GUI には `"$INSTDIR\mklm.exe" --quit` で終了を頼む。3 つの exe を `File` でその場で上書きする | 終了コードを決めていない。D.9.1 で全部の拒否に `SetErrorLevel` を付ける。`File` のその場の上書きは、失敗すると古い exe を切り詰めたまま残しうるので、`.new` に書いてから名前の変更で入れ替える方式に変える（D.9.2）。helper が `$INSTDIR` で動いている間はインストーラーが拒否するので、更新を実行する helper は別の場所の、別の名前のコピー（`mklm-update-runner.exe`）でなければならない（D.4、D.7） |
| `installer/build-installer.ps1` | リリースの 3 つの exe をビルドし、`dist\MKLM-Setup-<v>-<arch>.exe` と、`dist` のすべてのインストーラーの `SHA256SUMS` を作る | デバッグのリハーサル用の `-Profile dev`（`dist-dev\<版>\` に書く）を足す（F.6） |
| `.github/workflows/release.yml` | タグ `vX.Y.Z` → x64 と ARM64 のインストーラー（ARM64 は windows-latest でのクロスコンパイル）と `SHA256SUMS` を載せた**下書き**のリリース。NSIS は `choco install nsis` | 来歴の証明（必須）、NSIS の固定（ハッシュを確かめた公式の zip）、鍵と設定の検査、煙の試験などを足す（G.6）。下書きの題を「未署名、公開しないこと」にする（B.5） |
| helper（`apps/mklm-helper`） | `requireAdministrator`。固定のコマンドラインは 2 つ（パイプのセッションと `--uninstall-restore`）。HKCU と `%APPDATA%` を読まない。ネットワークのクレートを持たない | 3 つ目の固定のコマンドライン `--run-update <run-id>`（D.7）と、パイプの新しい要求（D.3）。通常のセッションでも、呼び出し元が知っている新しい失効と巻き戻しの記録を受け取る（`RecordTrust`、C.4） |
| パイプの約束（`mklm-ipc`） | `PROTOCOL_VERSION` 2。フレームの上限 `MAX_FRAME_LEN` 256 KiB。呼び出し元と helper のビルド ID の完全一致 | 版を 3 に上げる。ビルド ID の対象に `mklm-update` を足す（G.2） |
| 保護されたフォルダー（`mklm_win::protected_dir`） | `DataDir::Updates`（`…\MKLM\Updates`、`PRIVATE_DIR_SDDL`: SYSTEM と Administrators だけ）は定義済みで未使用。所有者、DACL、リパースポイントの検証と、先回りして作られた階層の隔離（m2 D.9、S1） | 実行ごとのサブフォルダーを、明示的な DACL で作る（D.5） |
| 書き込みのロック | `%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock` を `LockFileEx`。ジャーナルに open な操作がある間は更新しない（m2 D.9） | H1 と H2 がそれぞれロックを取り、ジャーナルを確かめる（D.4、D.7） |
| 多重起動の防止（m3 F.1） | `activate` と `quit` だけの小さなパイプ。`quit` はセッション中に `busy` を返すが、その前に取り消しと「後で終了」を立てる（`state.rs` の `quit_requested`）。再接続待ちでは終了の確認を出す | 更新には `quit` を使わない。副作用のない 3 つ目のコマンド `quit-if-idle` を足し、H2 はそれだけを使う（D.8。RELIABILITY-1、OPS-UX-TEST-4）。m3 F.1、F.5、A12 を合わせて改めた |
| v0.1.0 | 公開済み。アップデーターがない | 最初の更新対応版（v0.2.0）は利用者が手で入れる（B.8） |

### 0.2 設計の原則

1. **境界は helper。** GUI は「何を表示するか」を決めるだけ。helper は GUI から届いたものを何も信用せず、署名、失効、巻き戻し、版、アーキテクチャ、SHA-256、大きさを自分で確かめる。
2. **分からないときは入れない（fail closed）。** 例外は `expires` だけで、これは表示のための助言にする（B.4、C.6）。
3. **helper はネットワークに触れない。** ダウンロードは非昇格の GUI が行う。helper の実行ファイルにネットワークのコードを入れない（A.9、G.6）。
4. **helper は利用者の場所のファイルを開かず、利用者の環境変数を子に渡さない。** インストーラーのバイト列はパイプで受け取る（D.3）。H2 とインストーラーは、helper が作った最小の環境ブロックで起動する（D.9.4。SECURITY-6）。
5. **UAC は利用者がボタンを押したときだけ。** 自動の確認とダウンロードは UAC を出さない（m3 0.2 の 6）。
6. **どこで止まっても状態が分かる。** 更新の段階とインストーラーのプロセスを HKLM に記録し、次に起動した GUI が結果か中断を表示する（D.6、D.13）。
7. **キーボードの設定に触れない。** 更新はジャーナルを読むだけ（open な操作があれば始めない）で、書かない。サイレントの上書きインストールは、アンインストーラーも `--uninstall-restore` も実行しない（D.9.5）。
8. **テストの経路はリリース ビルドにコンパイルされない。** 本番以外の URL と鍵を使う経路は `cfg(all(debug_assertions, mklm_update_dev))` の中だけにあり、release.yml がそれを確かめる（A.10。SECURITY-9、OPS-UX-TEST-17）。
9. **秘密鍵に触れるのは、固定した公式の `minisign` だけ。** リポジトリのコードは秘密鍵とパスワードを扱わない。署名の前に、署名するものの出どころ（タグのコミット、CI の来歴の証明、SHA-256）を道具で確かめ、署名した後は、今までに出したどの版がその署名を受け付けるかを確かめてから公開する（B.3、B.5。SECURITY-1、SECURITY-2、OPS-UX-TEST-1）。

### 0.3 用語

| 用語 | 意味 |
|---|---|
| 更新情報（マニフェスト） | `latest.json`（A.2） |
| 主署名、副署名 | `latest.json.minisig`（必須）と `latest.json.alt.minisig`（鍵の移行の間だけ。A.4、B.7） |
| 信頼の起点ファイル | `crates/mklm-update/trust/anchors.txt`。そのビルドが信頼する公開鍵と、失効させた鍵 ID の一覧（B.2）。`xtask` は過去のタグのこのファイルを `git show` で読む |
| 移行の窓 | `TRANSITION_WINDOW_DAYS`（400 日）。ある版が「最新」でなくなってから（= 次の安定版が公開されてから）400 日の間は、その版の利用者が新しい署名を受け付けられるようにしておく。対象の版の正確な決め方は B.3 の「窓の中の版」（FIX-VERIFICATION-2。レビュー前は各版の公開日から数えていたので、長く最新だった版の利用者を取り残しえた） |
| 信頼できる時刻 | `https://api.github.com` の応答の `Date`。`issued_at` はこれで決める（B.3） |
| H1 | 利用者が［今すぐ更新］を押し、UAC で起動した `%ProgramFiles%\SHIN DATA CENTER\MKLM\mklm-helper.exe`。更新情報の検証と、インストーラーの受け取り（staging）を行う |
| H2 | H1 が `Updates\<run-id>\mklm-update-runner.exe` にコピーして起動した helper（`--run-update <run-id>`）。インストーラーを実行する。名前を変えるのは、プロセスの名前で MKLM を探す道具やインストーラーの変更に巻き込まれないため（RELIABILITY-12） |
| 実行 ID（run-id） | `<version>-<16 桁の小文字の 16 進>`（例: `0.2.1-3f9a0c2b7d1e4a65`）。更新 1 回のフォルダー名であり、記録の鍵 |
| インストール先 | `%ProgramFiles%\SHIN DATA CENTER\MKLM`（NSIS が固定する `$INSTDIR`） |
| 鮮度 | 更新情報の `expires` を過ぎたかどうか（`Freshness`） |
| 機械の記録 | HKLM の `Update` キー（D.6）。helper だけが書く |
| 利用者の記録 | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\state.json`（C.4）。GUI と CLI が書く |
| 開発用の cfg | `--cfg mklm_update_dev`。F.3 と F.6 のコマンドだけが `RUSTFLAGS` で渡す（A.10） |

---

## A. リリースの成果物と取得

### A.1 リリースに載るファイル

| ファイル | 作る人 | 用途 |
|---|---|---|
| `MKLM-Setup-<v>-x64.exe`、`MKLM-Setup-<v>-arm64.exe` | CI（release.yml） | インストーラー。CI が来歴の証明（build provenance）を付ける（G.6） |
| `SHA256SUMS` | CI | 人が照合する。`xtask prepare-release` の入力の 1 つ |
| `latest.json` | メンテナー（`cargo xtask prepare-release`） | 更新情報 |
| `latest.json.minisig` | メンテナー（公式の `minisign`、オフライン） | 更新情報の主署名 |
| `latest.json.alt.minisig` | メンテナー（同）。鍵の移行の間だけ | 更新情報の副署名（B.7） |

- ファイル名は固定（release.yml の冒頭のコメントどおり）。
- `latest.json` と署名は**下書きのうちに**上げてから公開する。上げて公開するのは `cargo xtask publish` で、ファイルの組がそろっていなければ公開しない（B.3、B.5。OPS-UX-TEST-2）。

### A.2 `latest.json`（スキーマ 1）

| フィールド | 型 | 決まり |
|---|---|---|
| `schema` | 整数 | `1` だけ |
| `product` | 文字列 | `"MKLM"` だけ（用途の分離） |
| `channel` | 文字列 | `"stable"` だけ（β チャネルは v1.x。計画 3 章） |
| `version` | 文字列 | `X.Y.Z`。プレリリースとビルド情報は不可。各数は 0〜65535（VERSIONINFO に入るため）。先頭の `v` は不可 |
| `issued_at` | 整数 | 署名した時刻（Unix 秒、UTC）。`xtask` が GitHub の時刻から決める（B.3） |
| `expires` | 整数 | 有効期限（Unix 秒、UTC）。`issued_at < expires`、差は `MAX_VALIDITY_SECS`（800 日）以下。`xtask` の既定は `issued_at` + 180 日（`DEFAULT_VALIDITY_DAYS`。J-3 の決定） |
| `key_ids` | 文字列の配列 | この更新情報に署名した鍵の ID（16 桁の大文字 16 進。B.2）。1 つか 2 つで、重複なし。主署名の鍵が先。検証に使った署名の鍵 ID がこの中にあること（`SignerNotListed`）。レビュー前の `key_id`（1 つ）から変えた（SECURITY-4） |
| `revoked_keys` | 文字列の配列 | 失効させる鍵 ID（空でもよい）。規則は B.2。以前の更新情報の失効もすべて引き継ぐ（`xtask` が確かめる。B.3） |
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
  "expires": 1807574400,
  "key_ids": ["8F1A2B3C4D5E6F70"],
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

（1792022400 は 2026-10-15 00:00 UTC、1807574400 はその 180 日後（既定の有効期限。J-3）の 2027-04-13。）

### A.3 形式の決まり

- UTF-8、BOM なし。JSON を厳密に解析する: 未知のフィールド、同じフィールドの重複、末尾の余計なデータ、型の違いはすべて拒否（`ManifestMalformed`）。
- 時刻は Unix 秒の整数にする。RFC 3339 の文字列にしないのは、日付の解析のコードを持たないため（検証の正しさを優先）。画面では GUI が現地時刻に直す。
- `xtask` は正準形（2 スペースの字下げ、A.2 の表の順、LF、末尾に改行 1 つ）で書く。クライアントは正準形を求めない。署名は受け取ったバイト列そのものに対して確かめる。
- 互換のない変更は `schema` を上げ、**別の名前**（`latest-v2.json`）で並べて公開する。古いクライアントは `latest.json`（スキーマ 1）を読み続ける。未知のフィールドを拒否するので、フィールドを足すだけの変更でもスキーマを上げる。

### A.4 署名ファイル

- minisign の署名ファイル（4 行: untrusted comment、署名、trusted comment、全体の署名）。**prehashed（アルゴリズム `ED`、BLAKE2b-512）だけ**を受け付ける（`minisign_verify::PublicKey::verify(…, allow_legacy = false)`）。公式の `minisign` 0.12 は prehashed が既定（公式のドキュメントで確認。`-l` を付けたときだけ legacy）。
- 主署名 `latest.json.minisig` は必須。副署名 `latest.json.alt.minisig` は、鍵の移行の間だけ置く（B.7）。どちらも同じ `latest.json` のバイト列に対する署名で、形も同じ。
- trusted comment（署名の対象に含まれる）は用途ごとに接頭辞を分ける。クライアントは、先頭が決まった接頭辞で、その直後が行末か空白であることを求める（`WrongTrustedComment`）。残りの部分は、人が `minisign -V` で読むためのもので、判断には使わない。

| 用途 | 接頭辞 | 受け付けるもの |
|---|---|---|
| 本番の更新情報 | `mklm-latest-json v1`（例: `mklm-latest-json v1 version=0.2.1 issued_at=1792022400`） | すべてのビルドの、信頼の起点ファイルの鍵 |
| リハーサルの更新情報 | `mklm-dev-latest-json v1` | 開発用の cfg のビルドの、開発用の鍵だけ（A.10）。本番の鍵がこの接頭辞で署名したものも、開発用の鍵が本番の接頭辞で署名したものも拒否する（OPS-UX-TEST-7） |
| 鍵の点検 | `mklm-key-drill v1` | `xtask key-drill check` だけ。更新情報としては必ず `WrongTrustedComment` になる（B.6。OPS-UX-TEST-13） |

- 同じ鍵で別のファイルに付けた署名を、`latest.json` の署名として通させないための分離である。
- untrusted comment は読まない。
- 署名ファイルは UTF-8 で 4 KiB 以下（`MAX_SIGNATURE_LEN`）。

### A.5 URL

| 取得するもの | URL |
|---|---|
| 更新情報 | `https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/latest/download/latest.json` |
| 主署名 | `…/releases/download/<tag>/latest.json.minisig`（`<tag>` は更新情報の 1 回目のリダイレクトから取る。A.6）。tag が取れなければ `…/releases/latest/download/latest.json.minisig` |
| 副署名 | 主署名が `UnknownKey` か `RevokedKey` で失敗したときだけ、同じ規則で `latest.json.alt.minisig` を取る。404 なら副署名はない（主署名の拒否の理由をそのまま使う） |
| インストーラー | `…/releases/download/v<version>/MKLM-Setup-<version>-<arch>.exe`（検証済みの更新情報の版から作る。C.8） |
| リリース ページ（画面のリンク） | `…/releases/tag/v<version>` |

- `releases/latest/download` は REST API のレート制限を消費しない（計画 4.2）。
- 実測（2026-09-29、v0.1.0 の `SHA256SUMS`）: `…/releases/latest/download/SHA256SUMS` → 302 → `…/releases/download/v0.1.0/SHA256SUMS` → 302 → `https://release-assets.githubusercontent.com/github-production-release-asset/…?…&se=…&sig=…`（1 時間ほどで切れる署名付きの URL）。
- 「最新」は「下書きでもプレリリースでもない、最も新しいリリース」。リリースの `make_latest` で変えられる（GitHub REST のドキュメント）。immutable なリリースでも、公開後に「最新」と「プレリリース」の印は変えられる（GitHub のドキュメントで確認、2026-09-29。I.5 は解決）。
- 更新情報には URL を持たせない。インストーラーの URL は、固定のリポジトリの URL と、検証した `version` と `name` から作る（C.8）。署名の鍵が漏れても、ダウンロード先を任意のホストに変えられない。
- 副署名を常に取らないのは、ふだんは存在せず、毎日の確認で 404 を 1 つ増やすだけになるため。取る条件を鍵の選択の失敗だけにしたのは、ほかの失敗（改ざん、形式の誤り）で別の署名を探しに行く理由がないため（SECURITY-4、OPS-UX-TEST-1）。

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
- **tag の取り出し**（`Endpoints::tag_from_location(location, asset_name)`。どのファイル名にも使える。OPS-UX-TEST-8）: `…/releases/latest/download/<asset_name>` の 1 回目のリダイレクトの `Location` が `https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/download/v<X.Y.Z>/<asset_name>`（`X.Y.Z` は A.2 の `version` の規則）なら、tag は `vX.Y.Z`。
  - 更新情報のときは、署名をその tag の URL から取る。更新情報と署名の取得の間に新しいリリースが公開されても、同じリリースの 2 つがそろうようにするため。更新情報の `version` が tag と一致することも確かめる（`TagMismatch`）。
  - 形が違えば（GitHub の仕様の変更）tag なしで `releases/latest/download` から署名を取る。2 つがずれて検証に失敗した場合は、次の確認でやり直す。
  - `xtask fetch-smoke` は同じ関数で `SHA256SUMS` の tag を取り出し、v0.2.0 を出す前に本番の経路を確かめる（F.8）。

### A.7 要求の中身

- GET だけ。送るヘッダーは `User-Agent` と `Accept`（更新情報と署名は `*/*`、インストーラーは `application/octet-stream`）だけ。
- `User-Agent`: `MKLM/<version> (Windows; <x64|arm64>; +https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager)`。利用者や PC を識別する情報は入れない。
- Cookie と認証情報（GitHub のトークンを含む）は送らない（`WINHTTP_DISABLE_COOKIES`）。
- **Windows の資格情報を自動で送らない**（SECURITY-13）: `WINHTTP_OPTION_AUTOLOGON_POLICY` を `WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH` にし、`WinHttpSetCredentials` を決して呼ばない。サーバーの認証（401）は `HttpStatus`、プロキシの認証（407）は `ProxyAuthRequired` として失敗にする。WPAD や PAC でプロキシを指定できる同じネットワークの攻撃者に、利用者の NetNTLM の応答を渡さないため。代わりに、Windows 統合認証を求めるプロキシの内側では自動更新が使えない（E.6 の文でリリース ページに案内する。J-8 の決定 (a)）。
- `Accept-Encoding` を送らない。`Content-Encoding` の付いた応答は拒否（`UnexpectedEncoding`）。
- TLS は 1.2 と 1.3 だけ（`WINHTTP_OPTION_SECURE_PROTOCOLS`）。証明書の検証は Windows の既定（Schannel と Windows の証明書ストア）。証明書のピン留めはしない（GitHub の証明書の更新で止まるため）。
- プロキシ: `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`（Windows 8.1 以降。システムと利用者ごとのプロキシ設定、IE の設定、PAC を使い、複数のプロキシの切り替えを扱う。Microsoft Learn の `WinHttpOpen`）。認証の要らないプロキシはそのまま使える。
- 同期モードの WinHTTP を専用のスレッドで使う（`WINHTTP_FLAG_SECURE_DEFAULTS` は非同期モードを強制するので使わず、TLS の版は上のオプションで決める）。取り消しは、読み取りの合間に見るフラグと、受信の期限による（A.8）。

### A.8 大きさの上限と期限

| 取得するもの | 上限 | 期限 |
|---|---|---|
| `latest.json` | 64 KiB（`MAX_MANIFEST_LEN`）。`Content-Length` が上限を超えれば本文を読まずに拒否。本文を上限 + 1 バイトまで読んで超えれば拒否（`TooLarge`） | 名前解決 15 秒、接続 15 秒、送信 30 秒、受信 30 秒（1 回の読み取りの無通信）。主署名と合わせた全体で 60 秒（`DeadlineExceeded`） |
| `latest.json.minisig`、`latest.json.alt.minisig` | 4 KiB（`MAX_SIGNATURE_LEN`） | 主署名は上の 60 秒の中。副署名は別に 60 秒 |
| インストーラー | 検証済みの更新情報の `size` ちょうど（64 MiB 以下）。`Content-Length` が違えば読まずに拒否。短くても長くても `SizeMismatch`。読みながら SHA-256 を計算し、違えば `HashMismatch` | 名前解決と接続は同じ。受信 60 秒。全体 30 分 |

- この表の「期限」は取得にかける時間の上限で、更新情報の有効期限 `expires`（既定は発行から 180 日。J-3 の決定。A.2、B.4、C.6）とは別のもの。
- 取り消し（利用者の［キャンセル］、GUI の終了）は、64 KiB の読み取りの合間にフラグで確かめる。最悪の遅れは受信の期限（30 秒 / 60 秒）。
- 自動の再試行はしない。失敗は次の定時の確認（E.1）か、利用者の［もう一度確認］でやり直す。

### A.9 HTTP クライアントの選択: WinHTTP（`windows` クレート経由）

| 観点 | WinHTTP（`windows` 0.62.2 の `Win32_Networking_WinHttp`） | ureq 3 + rustls（ring） | ureq / reqwest + aws-lc-rs | ureq / reqwest + native-tls（schannel） |
|---|---|---|---|---|
| TLS と証明書 | Schannel、Windows の証明書ストア（企業の CA も含む） | 自前の TLS。ストアを使うには rustls-platform-verifier が要る | 同左 | Schannel、Windows のストア |
| プロキシ | システムと利用者ごとの設定、PAC、WPAD（`AUTOMATIC_PROXY`） | 環境変数（`HTTPS_PROXY`）だけ。PAC はない | 同左（reqwest はレジストリの静的な設定を読むが PAC はない — 未確認） | ureq は環境変数だけ |
| aarch64-pc-windows-msvc のビルド | 追加の道具は要らない（Rust のバインディングとシステムの DLL） | ring は ARM64 の Windows で **clang が必須**（ring の BUILDING.md、issue #2117） | C コンパイラが要る。x64 の Windows では NASM も（事前ビルドのオブジェクトか `AWS_LC_SYS_NO_ASM` で回避可） | schannel は Rust だけ。reqwest は tokio と hyper を連れてくる |
| 依存の重さ | 新しいクレートは 0（`windows` の feature だけ） | 十数個（rustls、ring、webpki など） | aws-lc-sys（C のソース） | reqwest なら 100 前後 |
| ライセンス | OS の部品 | ISC / MIT / Apache-2.0（ring は独自の条項を含む） | Apache-2.0 / ISC / OpenSSL 系 | MIT / Apache-2.0 |
| unsafe の置き場 | `mklm-win`（このプロジェクトの規則どおり、unsafe はここだけ） | なし（クレートの中） | 同左 | 同左 |
| テスト | `http://127.0.0.1` のループバックで確かめられる（A.10） | 同左 | 同左 | 同左 |

**決定: WinHTTP。** 企業のネットワーク（PAC、TLS 検査用の社内 CA）でそのまま動き、ARM64 のクロスビルド（CI は windows-latest の x64 で ARM64 もビルドする）に clang、CMake、NASM が要らず、新しい依存がない。代わりに FFI のコードを自分で書くが、使う関数は `WinHttpOpen`、`WinHttpSetOption`、`WinHttpSetTimeouts`、`WinHttpConnect`、`WinHttpOpenRequest`、`WinHttpSendRequest`、`WinHttpReceiveResponse`、`WinHttpQueryHeaders`、`WinHttpReadData`、`WinHttpCloseHandle` の 10 個に限られる（`WinHttpSetCredentials` は使わない。A.7）。

**置き場所**

| 層 | 場所 | 中身 |
|---|---|---|
| FFI | `mklm_win::net`（feature `net`） | 1 回の GET と、本文の逐次の読み取り。リダイレクトは追わない。unsafe はここだけ |
| 規則 | `mklm_update::fetch` | リダイレクト、URL の規則、上限、期限、SHA-256。OS に依存しない純粋なコード（`Transport` トレイトの上で動く） |
| つなぎ | `mklm_update::winhttp`（feature `winhttp`、Windows だけ） | `Transport` を `mklm_win::net` で実装する |
| 利用 | `mklm-client`（`mklm-update` を feature `winhttp` 付きで使う）→ GUI と CLI。`xtask`（`verify --remote`、`fetch-smoke`、`prepare-release` の公開中の更新情報の取得） | — |
| 使わない | `mklm-helper`（`mklm-update` を feature なしで使う） | 検証のコードだけが入る |

ワークスペースをまとめてビルドすると feature が統合され、helper の中の `mklm-update` にも `winhttp` が付く（m3 A.1 の `gui` と同じ事情）。helper はそのコードを呼ばないので、最終的な実行ファイルには残らない。念のため release.yml で、`mklm-helper.exe` が `WINHTTP.dll` をインポートしていないことを確かめる（G.6）。

### A.10 テストとデバッグでの URL と鍵の差し替え（SECURITY-9、OPS-UX-TEST-7、OPS-UX-TEST-17）

- 本番の URL は `Endpoints::production()` が返す固定の文字列だけ。**実行時に URL を変える手段（環境変数、引数、設定ファイル）は、リリース ビルドには存在しない。**
- 本番以外の経路は、すべて `#[cfg(all(debug_assertions, mklm_update_dev))]` の中に置く。
  - ループバックの規則 `UrlPolicy::loopback()`、`Endpoints::loopback(base)`（`http://127.0.0.1:<port>` だけ）、`mklm_win::net` の平文 HTTP（接続先が `127.0.0.1` のとき、または名前付きのプロキシのセッションで `.invalid` の名前のときだけ）、`HttpSession::open_direct`、`HttpSession::open_named_proxy` と `AutologonLevel`（F.3 のプロキシの認証の試験）、`WinHttpTransport::new_without_proxy`。
  - GUI と CLI の `--update-endpoint=http://127.0.0.1:<port>`（F.6）。リリース ビルドにはこの引数を解釈するコードがなく、ほかの未知の引数と同じくログに書いて無視する。
  - 開発用の鍵: ビルド時の環境変数 `MKLM_UPDATE_DEV_PUBKEY`（`option_env!`）の公開鍵を、`TrustAnchors::for_this_build()` が「開発用の鍵」として足す。開発用の鍵は `mklm-dev-latest-json v1` の署名だけを通し、失効も巻き戻しの記録も動かさない（A.4、B.2）。
  - 目印: `mklm_update::dev` に `#[used] static DEV_MARKER: [u8; 26] = *b"MKLM-UPDATE-DEV-OVERRIDES!";`。release.yml は 6 つの exe（x64 と ARM64 の 3 つずつ）のバイト列にこの文字列がないことを確かめる。実行しないので、x64 のランナーで ARM64 の exe も調べられる。
  - **目印が必ずリンクされるようにする**（FIX-VERIFICATION-3）: rlib の非公開のモジュールの `#[used]` の静的な値が、リンカーの `/OPT:REF` を越えて exe に残るとは言い切れない。そこで、3 つの exe がすべて通る開発用の経路 `TrustAnchors::for_this_build()` の開発用の分岐（`dev::extra_key()`）が、`std::hint::black_box(&DEV_MARKER)` で目印を実際に参照する。GUI と CLI は `mklm_client::update::env::environment` から、helper は H1 と H2 と `RecordTrust` の処理からこの関数を呼ぶ。
  - **陽性の対照**（FIX-VERIFICATION-3）: 目印の検査は「ないこと」を確かめるだけなので、目印が開発用のビルドにも残らない状態では、何も守らないまま常に通る。そこで、開発用の cfg で作った 3 つの exe に目印が**あること**を、次の 2 か所で確かめ、なければ失敗させる。(1) ci.yml の F.3 のステップ（同じ `RUSTFLAGS` と `CARGO_TARGET_DIR` で `cargo build -p mklm -p mklm-cli -p mklm-helper --locked` もして、3 つの exe を調べる）。(2) `build-installer.ps1 -Profile dev`（リハーサルのインストーラーに入れる 3 つの exe）。検査の関数（`installer/find-marker.ps1`。バイト列の中の完全一致）は release.yml の「ないこと」の検査と同じものを使う。
- `mklm_update_dev` は Cargo の feature ではなく cfg にする。feature はワークスペースのビルドで統合されるので、どこかのクレートが有効にするとリリースにも入りうる。cfg は `RUSTFLAGS` で明示したときだけ立つ。立てるのは F.3 の ci.yml の 1 ステップ（別の `CARGO_TARGET_DIR`）と、F.6 のリハーサルのビルド（`build-installer.ps1 -Profile dev`）だけ。
  - ルートの `Cargo.toml` の `[workspace.lints.rust]` に `unexpected_cfgs = { level = "warn", check-cfg = ['cfg(mklm_update_dev)'] }` を足す（綴りの誤りを `clippy -D warnings` で検出する）。
- `debug_assertions` も併せて求める理由: 誤ってリリースのプロファイルで cfg を立てても、`[profile.release] debug-assertions = false`（WP-0 が明示する）なら入らない。二重の条件にする。
- **守り**（release.yml。G.6）
  1. ビルドの前（`installer/check-build-env.ps1`。FIX-VERIFICATION-3 で広げた）:
     - 次の環境変数が 1 つでもあれば失敗: `RUSTFLAGS`、`CARGO_ENCODED_RUSTFLAGS`、`CARGO_BUILD_RUSTFLAGS`、`MKLM_UPDATE_DEV_PUBKEY`、`RUSTC`、`RUSTC_WRAPPER`、`RUSTC_WORKSPACE_WRAPPER`、`CARGO_BUILD_RUSTC`、`CARGO_BUILD_RUSTC_WRAPPER`、`CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`。名前が `CARGO_PROFILE_` で始まるもの、`CARGO_TARGET_` で始まり `_RUSTFLAGS` で終わるもの（`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS` など）も失敗。ラッパーや別の `rustc` は、フラグを足す別の入口になるため。
     - リポジトリの `.cargo/config.toml` に `[alias]` 以外の表（`[build]`、`[target…]`、`[profile…]`）か `rustflags` があれば失敗。
     - Cargo が読むほかの設定ファイルがあれば失敗: `%CARGO_HOME%\config.toml` と `%CARGO_HOME%\config`（`CARGO_HOME` がなければ `%USERPROFILE%\.cargo`）、作業ツリーの**親のフォルダー**のそれぞれの `.cargo\config.toml` と `.cargo\config`（Cargo は今のフォルダーから上へすべてを読み、合わせる）。
     - ワークフローのコマンドに `--config` を書かない（レビューで確かめる。G.6）。
  2. ビルドの後: 3 つの exe の `VersionInfo.IsDebug` が false。6 つの exe に目印の文字列がない。この「ないこと」の検査が意味を持つのは、上の陽性の対照（ci.yml と `-Profile dev` で「あること」）が通っているときだけ。
- 各 exe の `build.rs` は、`VS_FF_DEBUG` を `PROFILE` ではなく `CARGO_CFG_DEBUG_ASSERTIONS` の有無から決める（Cargo のドキュメントは `PROFILE` を使わないよう勧めている。`CARGO_CFG_<cfg>` は「ビルドするパッケージの cfg」を表す。デバッグの表明の設定がそのまま反映されるかは未確認で、WP-0 が `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` のビルドで確かめる）。守りは 3 つを重ねる: ビルドの前の環境の検査、`IsDebug`、陽性の対照つきの目印の検査。どれか 1 つが漏れても、ほかが止める。
- `xtask` は開発用の鍵を読まない: 本番のコマンドはすべて `TrustAnchors::release()`（信頼の起点ファイルだけ。ビルドのプロファイルによらない）を使い、`cfg!(mklm_update_dev)` のビルドなら何もせずに失敗する。リハーサルのコマンド（`--dev`）は、開発用の公開鍵をファイルで明示的に受け取る（B.3）。
- テストは `TrustAnchors::from_keys` だけを使う（開発者の環境変数でテストの結果が変わらないように）。
- 前例: helper にはすでに、デバッグ ビルドだけの `MKLM_DEBUG_PAUSE`（m2 H.2 の R5）がある。

---

## B. 署名（メンテナー、オフライン）

### B.1 クレートと道具

| もの | 版（2026-09-29） | ライセンス | 使う場所 | 備考 |
|---|---|---|---|---|
| `minisign-verify`（クレート） | 0.3.0（2026-09-25） | MIT | `mklm-update`（GUI、CLI、helper、`xtask`） | 依存 0。`=0.3.0` で固定する（検証の中心なので、上げるときは意図して上げる）。v0.2.0 の前に、レビューする人がソース全体を読み、0.2.x からの差分を確かめて G.6 の記録に残す |
| `minisign`（クレート） | 0.10.0（2026-09-25） | MIT | `mklm-update` と `xtask` の **dev-dependency だけ**（テストで使い捨ての鍵を作り、署名する） | 本物の鍵には決して触れない。`xtask` の通常の依存には入れない（SECURITY-2） |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | `mklm-update`、`xtask` | 純粋な Rust |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | `mklm-update`、`xtask` | 依存 0 |
| 公式の `minisign`（コマンド） | 0.12（`minisign-0.12-win64.zip`） | ISC | メンテナーの鍵の生成、署名、鍵の点検（B.5、B.6）。F.6 のリハーサルの署名も同じもの | リポジトリにもビルドにも入らない。作者の公開鍵 `RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3`（minisign の README）で zip の署名を確かめ、SHA-256 を記録して、鍵と同じ USB メモリに置く（`xtask verify-signer`。B.3）。ライセンスへの同意の操作はない |

- `minisign-verify` 0.3.0 の API（docs.rs で確認）: `PublicKey::from_base64(&str)`、`Signature::decode(&str)`、`PublicKey::verify(&[u8], &Signature, allow_legacy: bool) -> Result<(), Error>`（鍵 ID が違えば `Error::UnexpectedKeyId`、legacy を許さない設定で legacy なら `Error::UnexpectedAlgorithm`）、`Signature::trusted_comment() -> &str`。**鍵 ID を返す公開の関数はない。** そこで鍵 ID は、`mklm-update` が公開鍵と署名の base64 を自分で解いて読む（C.2）。
- `minisign-verify` と `minisign` のクレートは 4 日前に版が上がったばかりで、前の版からの変更点は確かめられていない（L 章）。WP-0 がドキュメントでシグネチャを確かめる。
- 秘密鍵を扱うのを公式のコマンドだけにした理由（SECURITY-2）: 署名の瞬間に `cargo xtask …` を実行すると、そのタグのソースと crates.io の依存（proc-macro、ビルド スクリプトを含む）を、秘密鍵のつながった PC でコンパイルして実行することになり、パスワードもそのプロセスに打ち込む。`xtask` を「公開のデータだけを扱う道具」にし、秘密鍵には、固定したハッシュの単一の実行ファイルだけが触れるようにする。
- MIT と ISC の著作権表示は、計画 2.4 の `THIRD-PARTY-LICENSES`（cargo-about）に入れる（公式の `minisign` は配布物に入らないので対象外）。

### B.2 鍵の形、信頼の起点ファイル、失効と巻き戻しの規則

**鍵**

- 鍵は 2 本: 通常用（`KeyRole::Primary`）とバックアップ用（`KeyRole::Backup`）。どちらも minisign の Ed25519 で、秘密鍵はパスワード付き（scrypt）。作るのは公式の `minisign -G`（B.5）。
- **鍵 ID**: 8 バイト。表記は 16 桁の大文字の 16 進で、8 バイトを little-endian の `u64` として読んだもの（minisign のコマンドが表示する形に合わせる。未確認のため、WP-U がテストで `minisign` クレートの出力と、B.5 の準備で公式のコマンドの表示と照合する）。
- **指紋**（`KeyFingerprint`）: 公開鍵の base64 を解いた 42 バイト（アルゴリズム 2、鍵 ID 8、Ed25519 の公開鍵 32）の SHA-256。記録した失効がどの鍵のものかを、ID だけでなくこれで区別する（SECURITY-3）。

**信頼の起点ファイル**（`crates/mklm-update/trust/anchors.txt`。OPS-UX-TEST-1）

```
# MKLM update trust anchors (design m5b B.2). "<role> <KEYID> <base64 public key>" or "revoked <KEYID>".
primary 8F1A2B3C4D5E6F70 RWQ…
backup  0123456789ABCDEF RWQ…
revoked 1111222233334444
```

- 1 行 1 項目。`#` で始まる行と空行は無視。項目の間は空白 1 つ以上。それ以外の形は拒否（`KeyError::BadAnchorsLine { line }`）。
- 製品は `include_str!` で埋め込み（`ANCHORS_TEXT`）、起動のたびに厳密に解析する（`TrustAnchors::release()`）。`xtask` は同じ解析器で、過去のタグのファイルを `git show <tag>:crates/mklm-update/trust/anchors.txt` で読み、そのタグのバイナリが何を信頼するかを知る（B.3 の取り残しの検査）。
- 鍵ができるまでは、コメントだけ。鍵のないビルドでは更新が使えない（`UpdateRefusal::NotConfigured`。GUI はその旨を表示する）。
- release.yml は `cargo xtask check-keys` で、鍵のない、または壊れた、または役割の数が違う（通常用 1、バックアップ用 1 でない）ファイルのリリースを止める（G.6）。

**失効の規則**（`verify_manifest` と `xtask` が同じ規則を使う。SECURITY-3）

1. 署名した鍵が失効していれば拒否（`RevokedKey`）。失効の出どころは次のどれか。
   - そのビルドの信頼の起点ファイルの `revoked` の行（ID で判断する。ビルドを作った人が決めたことなので）。
   - 機械の記録と利用者の記録（C.4）。記録の失効は **(鍵 ID、指紋) の組**で、署名した鍵の ID と指紋の両方が一致したときだけ効く。
2. 検証に通った更新情報の `revoked_keys` の各 ID は、そのビルドの信頼の起点で次のように扱う。
   | ID が指す鍵 | 署名した鍵が通常用 | 署名した鍵がバックアップ用 |
   |---|---|---|
   | 署名した鍵そのもの | `IllegalRevocation`（更新情報ごと拒否） | 同左 |
   | 埋め込みのバックアップ用の鍵 | **無視する（記録しない。更新情報は受け付ける）** | （署名した鍵そのもの。上の行） |
   | 埋め込みの、ほかの通常用の鍵 | 記録する | 記録する |
   | すでにビルドで失効している ID | 何もしない | 何もしない |
   | 埋め込みにない ID | **無視する（記録しない）** | 同左 |
   - レビュー第 1 回の後は「通常用がバックアップ用を失効 → `IllegalRevocation`」だったが、無視に改めた（FIX-VERIFICATION-1）。`prepare-release` は失効を引き継ぐ（B.3 の手順 9）ので、B1 を `revoked` にした版の後の更新情報はすべて `revoked_keys` に B1 を持つ。拒否にすると、B1 を埋め込んだ古い版がそれ以降の通常用の鍵の署名をすべて拒み、バックアップ用の鍵を替えただけで全員が取り残された（B.7 の 3、4 行目）。無視しても守りは弱まらない: 漏れた通常用の鍵を持つ攻撃者は、失効の行を省けば同じ更新情報を作れるので、拒否は攻撃を何も止めていなかった。記録しないので、規則 4 はそのまま成り立つ。
   - `IllegalRevocation` が残るのは「署名した鍵そのもの」を失効させる更新情報だけ（作る側の誤り。`prepare-release` の手順 10 が署名の前に見つける）。
3. 記録した失効は取り消せない。
4. バックアップ用の鍵は、更新情報では失効させられない（通常用の鍵の署名では無視され、バックアップ用の鍵の署名では自分自身の失効になる）。失効させるのは新しい版の信頼の起点ファイル（`revoked` の行）だけ。
   - 理由: 通常用の鍵はリリースのたびに使うので、漏れる機会が多い。漏れた通常用の鍵で「バックアップ用を失効」と書いた更新情報を作れば、復旧の道を断てる。これを許さない。
   - 裏返しとして、**バックアップ用の鍵が漏れることは、通常用の鍵が漏れることと同じかそれ以上に重い**（SECURITY-4 の (d)）。攻撃者は GitHub への書き込みと合わせて、悪意のある更新情報に署名でき、古いクライアントに通常用の鍵を失効させることもできる。バックアップ用の鍵はほとんど使わず、別の場所に保管する（B.6）。起きたときの手順は B.7。
5. （レビュー前の規則 5「埋め込みにない ID の失効も記録する」は**やめた**。）漏れた通常用の鍵で、まだ公開前の後継の鍵（P2、B2）の ID を失効させた更新情報を作られると、後継の鍵を入れた版を入れた後、その鍵の署名がすべて `RevokedKey` になり、手で入れ直す以外に抜けられなくなるため。新しい版が加える鍵は、その版の信頼の起点ファイルが守る。

**巻き戻しの記録**（SECURITY-5、SECURITY-12、RELIABILITY-6、OPS-UX-TEST-18）

- 記録は鍵ごとに持つ: 「その鍵で署名された更新情報の `issued_at` の最大値」。ただし**記録する値は `min(issued_at, 記録する時の PC の時刻)`** にする。
  - 理由: 署名した PC の時計が未来にずれていた、または漏れた鍵で遠い未来の日付を付けられた更新情報を 1 度受け取っても、その値で以後の正しい更新情報を拒み続けないため。記録は受け取った時点の時刻を超えないので、その後に正しく作られた更新情報（`issued_at` は記録した時刻より後）はすべて通る。
  - レビューの提案（`now + 2 日`）より厳しくした理由: 2 日の余裕があると、誤った日付の更新情報を見てから 2 日以内に次の正しい版が出た場合、その版を永久に拒むクライアントが出る（毎日確認するクライアントのほとんどが該当する）。
  - 代わりに、PC の時計が遅れているクライアントでは、記録が実際より小さくなり、巻き戻しの防止が時計の遅れの分だけ弱まる。TLS の検証も時計に頼るので、大きく遅れた PC はそもそも取得できない。
- **判断は鍵をまたいだ最大値で行う**: 更新情報の `issued_at` が、次の鍵を除いたすべての記録の最大値（`TrustState::rollback_threshold`）より小さければ拒否（`Rollback`）。同じ値は許す（同じ更新情報を取り直した場合）。
  - 除く鍵: ビルドで失効している鍵、記録で失効している鍵（ID と指紋）、**この更新情報が正しく失効させる鍵**。
  - 最後の除外により、漏れた鍵で先に受け取らされた更新情報があっても、その鍵を失効させるバックアップ用の鍵の更新情報は、必ず巻き戻しの検査を通る。
  - レビュー前は鍵ごとに比べていた。鍵をまたいだ最大値にすると、helper（H1）は「この PC がこれまでに検証したどの更新情報より古いもの」を入れなくなる（SECURITY-5 の (a) の、古いが正しく署名された版を管理者に入れさせる攻撃への備え）。上の記録の上限があるので、鍵をまたいでも、未来の日付で記録が汚れる心配はない。
- `Check`（GUI と CLI）と `Install`（helper）で同じ規則を使う。

### B.3 `xtask`（公開のデータだけを扱う道具）

- 場所: `xtask/`（ワークスペースのメンバー、`publish = false`、バイナリ名 `xtask`）。`.cargo/config.toml` の別名 `xtask = "run --package xtask --locked --"` で `cargo xtask <コマンド>` と打つ（計画 2.4 の `xtask/`）。dev プロファイルでビルドされるが、本番のコマンドは `TrustAnchors::release()` しか使わないので、プロファイルで結果は変わらない（A.10）。
- インストーラーには入らない（`build-installer.ps1` がビルドするのは `mklm`、`mklm-cli`、`mklm-helper` だけ）。
- 依存: `mklm-update`（feature `winhttp`。検証と取得は製品と同じコード）、`sha2`、`semver`、`serde_json`、`clap`、`anyhow`。GitHub の操作は `gh` コマンドを子プロセスで呼ぶ（`ReleaseHost` トレイトの後ろに置き、テストでは偽物に替える）。
- **秘密鍵のファイルを開くコードを持たない。** パスワードも尋ねない。署名は公式の `minisign` がオフラインで行う（B.5）。
- 本番のコマンドは、`cfg!(mklm_update_dev)` のビルドなら「開発用の cfg でビルドされた xtask では実行できません」と出して終了コード 1。

| コマンド | 動作 | ネットワーク |
|---|---|---|
| `check-keys` | 信頼の起点ファイルが解析でき、通常用 1 本とバックアップ用 1 本で、表記の ID が公開鍵の ID と一致し、互いに違い、`revoked` に入っておらず、**過去のタグの同じ ID が別の公開鍵を指していない**こと（鍵 ID の使い回しの検出）、**過去のタグで `revoked` にした ID を埋め込んでいない**こと。release.yml が呼ぶ。過去のタグは `git tag -l "v*"` と `git show <tag>:<ANCHORS_REPO_PATH>` で読む。**読めないときは飛ばさずに失敗する**: リポジトリが浅い（`git rev-parse --is-shallow-repository` が true）、`v*` のタグが 1 つもない、またはファイルを持つ最初のタグより後のタグにファイルがない（FIX-VERIFICATION-15）。ファイルを持つ最初のタグより前のタグ（v0.1.0 など）は、更新に対応していない版として飛ばす | なし |
| `pubkey-line --pub <file> --role primary\|backup` | 公式の `minisign -G` が書いた `.pub` を読み、信頼の起点ファイルに貼る 1 行（`primary <ID> <base64>`）と指紋を表示する。公開鍵だけを扱う | なし |
| `verify-signer --zip <minisign の zip> --sig <zip.minisig>` | 公式の `minisign` の配布物を作者の公開鍵（B.1。定数）で確かめ、zip と中の `minisign.exe` の SHA-256 を表示する（メンテナーが記録する） | なし |
| `prepare-release …` | 下の手順。署名する前のすべての確認と、`latest.json` と trusted comment の作成 | あり |
| `publish --tag vX.Y.Z --dir <dir>` | 下の手順。署名の検証、アップロード、公開、公開後の確認 | あり |
| `verify --remote [--installers] [--min-days-left N] [--newest-published]`、`verify --dir <dir>` | 本番の取得の経路（`WinHttpTransport`、本番の URL の規則、tag の取り出し、副署名の規則）で公開中の更新情報を取り、信頼の起点ファイルの鍵と空の記録で検証する。`--installers` なら 2 つのインストーラーもダウンロードして大きさと SHA-256 を確かめる（SECURITY-1 の (6)）。期限までの日数が N 未満、`issued_at` が信頼できる時刻より 1 日以上先なら失敗。`--newest-published` は、配られている版が公開済みの最も新しい安定版より古ければ失敗（G.6 の見張り。RED-TEAM-1）。`--dir` は手元のファイルを検証する | `--remote` はあり |
| `fetch-smoke` | 本番の経路で `releases/latest/download/SHA256SUMS` を取り、リダイレクトの各段が本番の規則を通ること、tag が取り出せること、TLS、本文が `SHA256SUMS` として読めることを確かめる（v0.1.0 にもある。OPS-UX-TEST-8） | あり |
| `key-drill start --role backup --out <dir>`、`key-drill check --dir <dir> --role backup` | 鍵の点検（B.6）。`start` は乱数の nonce のファイル（`<out>\nonce.bin`、32 バイト）と、公式の `minisign` のコマンド（`<out>\SIGN-OFFLINE.txt`。署名は `<out>\nonce.bin.minisig`、trusted comment は固定の `mklm-key-drill v1`。署名が新しい乱数のファイルにかかるので、nonce を trusted comment に写す必要はない）を書く。`check` は、その署名が「窓の中の版」（下の定義。今の時刻で数える）のすべてのタグの信頼の起点ファイルのバックアップ用の鍵で、`start` が作った `nonce.bin` の署名として通ることと、trusted comment が `mklm-key-drill v1` であることを確かめ、鍵 ID を表示する。何も書かない（OPS-UX-TEST-13） | `check` はリリースの公開日のため `gh release list`、ファイルのため `git` |
| `prepare-release --dev …`、`serve-releases --dir <dir> [--port N]` | デバッグ ビルドのリハーサル専用（F.6）。文法と出力は下の「`prepare-release --dev` の文法」 | なし |

**`prepare-release --tag vX.Y.Z --commit <40 桁の SHA> --main-key-id <ID> [--alt-key-id <ID>] [--expires-days N] [--revoke <ID>]... [--min-from-version X.Y.Z] [--allow-strand <tag>,...] [--published-misdated] --out <dir>` の手順**（SECURITY-1、SECURITY-4、SECURITY-12、RELIABILITY-6、OPS-UX-TEST-1）

1. `--tag` を `v` と A.2 の規則の版に分ける（プレリリースとビルド情報は拒否）。
2. **手元の作業ツリー**: `HEAD` と `vX.Y.Z^{commit}` が `--commit` と一致し、変更のないこと。`--commit` はメンテナーがレビューしたコミット（B.5 の手順 4）。
3. **GitHub のタグ**: `gh api repos/{owner}/{repo}/git/ref/tags/vX.Y.Z`（注釈付きのタグなら `git/tags/<sha>` をたどる）のコミットが `--commit` と一致すること。
4. **下書き**: `gh release view vX.Y.Z --json isDraft,isPrerelease,assets,name` で、下書きで、プレリリースでなく、ファイルがちょうど `MKLM-Setup-<v>-x64.exe`、`MKLM-Setup-<v>-arm64.exe`、`SHA256SUMS` の 3 つであること。3 つを `<out>\assets\` にダウンロードする。
5. **ハッシュ**: 2 つのインストーラーの SHA-256 と大きさが、`SHA256SUMS` の同じ名前の行と、GitHub のアセットの `digest`（REST API のアセットの `digest` フィールド。`sha256:<hex>`）の両方と一致すること。`SHA256SUMS` に余分な行や重複があれば拒否。
6. **来歴の証明**: 各インストーラーについて `gh attestation verify <file> --repo SHIN-DATA-CENTER/multi-keyboard-layout-manager --signer-workflow SHIN-DATA-CENTER/multi-keyboard-layout-manager/.github/workflows/release.yml --source-digest <commit> --source-ref refs/tags/vX.Y.Z --deny-self-hosted-runners` が成功すること（フラグは gh の manual で確認）。証明がなければ署名しない（release.yml が必ず付ける。G.6）。来歴の証明は「どこで、どのコミットから作られたか」を示すだけで、ソースが無害であることは示さない。後者はメンテナーの差分のレビュー（B.5 の手順 4）が担う。
7. **信頼できる時刻**: `gh api -i /` の応答の `Date` を読む。手元の時計との差が 5 分を超えれば拒否（時計を直してからやり直す）。`issued_at` はこの `Date`（秒）。`expires` は `issued_at` + `--expires-days`（既定 180 日（`DEFAULT_VALIDITY_DAYS`。J-3 の決定）、上限 800 日）。
8. **公開中の更新情報**: 本番の経路で `latest.json` と署名を取り、**直前のリリースのタグ**の信頼の起点ファイルで検証する（鍵を移行した後でも、公開中のものは古い鍵で署名されているため）。
   - `issued_at` が公開中のもの以下なら拒否。公開中のものが信頼できる時刻より未来の日付なら、`--published-misdated` を付けたときだけ続ける（B.7 の「日付の誤った更新情報」。クライアントは記録を受け取った時刻で抑えているので、正しい日付の次の版を受け付ける）。
   - 公開中の更新情報がない（404）のは、信頼の起点ファイルを持つ過去のタグがないとき（最初の更新対応版 v0.2.0）だけ許す。
9. **失効の引き継ぎ**: `revoked_keys` = このタグの信頼の起点ファイルの `revoked` ∪ 公開中の更新情報の `revoked_keys` ∪ `--revoke`。`--revoke` が B.2 の規則に反すれば拒否（下の 10 の各版の信頼の起点で判断する）。バックアップ用の鍵の ID が入っても、それを埋め込んだ版は通常用の鍵の署名の中では無視するので（B.2 の規則 2）、古い版を拒ませることはない（FIX-VERIFICATION-1）。
10. **取り残しの検査**: 対象の版 = 下の「窓の中の版」（信頼の起点ファイルのないタグは除く）と、このタグ自身。各版の信頼の起点ファイルで、C.3 の手順と同じ判断（`xtask::strand::accepts`。`verify_manifest` の鍵の選択と失効の規則だけを、署名の代わりに鍵 ID で行う純粋な関数）を行い、主署名の鍵（`--main-key-id`）か副署名の鍵（`--alt-key-id`）の少なくとも一方で「知っていて、失効しておらず、`revoked_keys` が `IllegalRevocation` にならない」ことを確かめ、表にして表示する（「v0.2.0: 主署名 ×（未知の鍵）、副署名 ○」）。受け付けない版があれば拒否。`--allow-strand` に挙げた版だけは、取り残すことを承知したものとして続ける。
11. `latest.json` を正準形で `<out>` に書く（`key_ids` = 主署名の鍵、副署名の鍵）。trusted comment（`mklm-latest-json v1 version=<v> issued_at=<t>`）を `<out>\trusted-comment.txt` に書く。
12. `<out>\SIGN-OFFLINE.txt` に、オフラインで実行する公式の `minisign` のコマンドを、そのまま貼れる形で書く:
    ```
    E:\tools\minisign.exe -S -s E:\mklm-keys\mklm-primary.key -m <out>\latest.json -x <out>\latest.json.minisig -t "<trusted comment>"
    ```
    （副署名があれば、同じ形で `-x <out>\latest.json.alt.minisig` の行も。）
13. `<out>\prepare.json` に、タグ、コミット、2 つのアセットの SHA-256 と `digest`、`issued_at`、鍵 ID、窓の中の版の一覧、取り残しの検査の結果を書く（`publish` が照らし直す）。
14. 要約を表示する: 版、**`issued_at` と有効期限の UTC の日付**（大きく）、主署名と副署名の鍵と役割、失効、2 つのアセットの大きさと SHA-256、取り残しの表（窓の中の各版の後継の公開日と、窓が明ける日を含む）。

**窓の中の版**（`xtask::releases::window_targets(releases, now)`。`prepare-release`、`publish`（`prepare.json` に残した一覧）、`key-drill check` が同じ関数を使う。FIX-VERIFICATION-2）

- 材料: `gh release list --exclude-drafts --json tagName,publishedAt,isPrerelease`（件数の上限を十分に大きく）のうち、タグが安定版の形（`vX.Y.Z`）のもの。**公開後にプレリリースの印を付けたもの（B.5 の「リリースの事故」）も含める**（その間に入れた利用者がいうる）。`-` を含むタグ（本来のプレリリース）は除く。`now` は手順 7 の信頼できる時刻（`key-drill check` では同じく `gh api -i /` の `Date`）。
- 公開日の順に並べ、各版 R の「後継の公開日」を、R の次に公開された版の `publishedAt` とする。
- 窓の中の版 = 後継がまだない版（= 今の最新。直前のリリースはここに入る）と、後継の公開日から `TRANSITION_WINDOW_DAYS`（400 日）たっていない版。
- 理由: 利用者が新しい鍵を覚えるのは、その版が最新である間に確認したときではなく、**後継が出た後に**確認したときである。レビュー前の「各版の公開日から 400 日」では、長く最新だった版（たとえば 1 年）が、後継（鍵の移行の版）の公開から 2 か月で窓から外れ、その 2 か月に確認しなかった利用者が取り残された。
- 例（`xtask` の単体テスト。F.1）: v0.4.0 を 2027-01-10 に公開。v0.5.0（鍵の移行: 主 B1、副 P2）を 2028-01-10 に公開。2028-03-10 に v0.6.0 を主 P2 だけで準備する。v0.4.0 の後継の公開日は 2028-01-10 で、60 日しかたっていないので窓の中。v0.4.0 は P2 を知らないので、手順 10 は拒否する（副署名に B1 を付ければ通る）。

**`prepare-release --dev` の文法**（F.6 のリハーサル専用。FIX-VERIFICATION-17）

```
cargo xtask prepare-release --dev --tag vX.Y.Z --dist <dir> --dev-pub <file.pub> --minisign <minisign.exe>
                            [--only-arch x64|arm64] [--expires-days N] [--issued-at <Unix 秒>] --out <dir>
```

- `--dist`: `build-installer.ps1 -Profile dev` が書いたフォルダー（`dist-dev\<版>\`）。`MKLM-Setup-<v>-x64.exe` と `MKLM-Setup-<v>-arm64.exe`（`--only-arch` のときはその 1 つ）と `SHA256SUMS` があり、`SHA256SUMS` の行が各ファイルと一致すること。
- `--dev-pub`: 公式の `minisign -G` が書いた開発用の鍵の `.pub`。その鍵 ID が `key_ids` になる。**鍵 ID が `TrustAnchors::release()`（本番の信頼の起点ファイル）にあれば拒否**（本番の鍵を開発用として使わない）。
- `--minisign`: `SIGN-OFFLINE.txt` に書く `minisign.exe` のパス（F.6 の準備で、確かめた zip から取り出したもの）。`xtask` はこのファイルを実行しない。
- `--only-arch`: もう一方のアーキテクチャのアセットを「大きさ 1、SHA-256 は 64 個の 0」で埋める（OPS-UX-TEST-9）。
- `issued_at`: 手元の時計（GitHub には問い合わせない）。`--issued-at` は巻き戻しのリハーサル用。`expires` は `issued_at` + `--expires-days`（既定 180。本番と同じ `DEFAULT_VALIDITY_DAYS`）。
- `--out`: `--dist` と同じでもよい（F.6 はそうする）。書くもの: `latest.json`（正準形、`revoked_keys` は空）、`trusted-comment.txt`（`mklm-dev-latest-json v1 version=<v> issued_at=<t>`）、`SIGN-OFFLINE.txt`（下の 1 行）。`prepare.json` は書かない（`publish` は `--dev` の成果物を扱わない）。
  ```
  <--minisign> -S -s <--dev-pub と同じフォルダーの、同じ名前の .key> -m <out>\latest.json -x <out>\latest.json.minisig -t "<trusted comment>"
  ```
  （`xtask` は `.key` のパスを文字列として書くだけで、開かない。B.3 の「秘密鍵のファイルを開くコードを持たない」は変わらない。）
- 拒否するもの: `--dev` のない `--dist`、`--dev-pub`、`--minisign`、`--only-arch`、`--issued-at`（本番の文法と混ぜない）。プレリリースのタグ。ファイルの欠け、`SHA256SUMS` の食い違い。
- `serve-releases --dir <dist-dev のルート> [--port N]`: `127.0.0.1` だけで待ち受け、`<ルート>\<版>\` をタグ `v<版>` として、`/releases/latest/download/<名前>` → 302 → `/releases/download/v<最新>/<名前>` を GitHub と同じ形で返す。「最新」は `latest.json` と `latest.json.minisig` がそろった、最も新しい版のフォルダー。

**`publish --tag vX.Y.Z --dir <dir>` の手順**（OPS-UX-TEST-2）

1. プレリリースのタグは拒否する（B.5 のプレリリースの手順を使う。OPS-UX-TEST-14）。`prepare.json` を読み、タグのコミット、下書きの状態、2 つのインストーラーの `digest` が prepare の時から変わっていないこと（`gh api`）。
2. `latest.json` と署名を、`prepare.json` の対象の版すべての信頼の起点ファイルで、C.3 の手順そのもの（`Purpose::Check`、入っている版 = その版、空の記録。主署名 → 必要なら副署名）で検証する。trusted comment が `trusted-comment.txt` と完全に一致すること。どれかの版で通らなければ公開しない（`--allow-strand` の版を除く）。
3. `gh release upload vX.Y.Z latest.json latest.json.minisig [latest.json.alt.minisig]`。
4. 下書きのファイルが、ちょうど予定の 5 つ（副署名があれば 6 つ）で、上げた 2〜3 つの `digest` が手元のファイルと一致すること。
5. 公開: `gh release edit vX.Y.Z --draft=false --latest --title "MKLM vX.Y.Z"`（下書きの題の「未署名」を外す）。
6. 公開後の確認: `verify --remote --installers` と同じ処理（CDN の反映を待って、最大 5 分、30 秒ごとに再試行）。失敗すれば、B.5 の「リリースの事故」の手順を表示して終了コード 1。

- 有効期限を延ばす、署名し直す: 下書きのうちなら `prepare-release` からやり直す。公開した後は新しいリリースが要る（B.4）。
- 公式の `minisign` だけでも署名そのものは作れるが、`prepare-release` と `publish` の確認を飛ばさない。`xtask` がビルドできない場合は、リリースを延期する（確認の道具を失ったまま署名しない）。

### B.4 `expires` と GitHub の immutable releases

**GitHub のドキュメントで確かめたこと**（2026-09-29）

- 公開した時点で、Git のタグとリリースのファイルが固定される。ファイルの追加、置き換え、削除はできない。
- 題とリリースノート、**「プレリリース」と「最新」の印は公開後も変えられる**。
- 下書きのうちは自由に編集できる（「すべてのファイルを下書きに付けてから公開する」ことが推奨されている）。
- リリースは削除できるが、同じタグ名は二度と使えない。
- 公開すると、タグ、コミット、ファイルを含む release attestation が自動で作られる。
- リポジトリか組織の設定で有効にする。

**帰結と決定**

1. `latest.json` と署名は**下書きのうちに**上げる。公開した後では直せない。上げ忘れたまま公開しないよう、公開は `xtask publish` で行う（B.3）。下書きの題は「MKLM vX.Y.Z — UNSIGNED, DO NOT PUBLISH」にし、ブラウザーで誤って公開しにくくする（release.yml。OPS-UX-TEST-2）。
2. `releases/latest/download` は最新のリリースのファイルしか配らない。同じリリースの `latest.json` を署名し直して `expires` を延ばすことはできないので、延ばすには新しいリリースが要る。
3. したがって、有効期限の既定は **180 日**（約 6 か月）にする（**J-3 の決定 (b)**、2026-09-29。`DEFAULT_VALIDITY_DAYS`（H.1）、`prepare-release` の既定（B.3 の手順 7）、A.2 の例）。期限は凍結を知らせる唯一の信号なので（下の 4）、短い方を選んだ。少なくとも 5 か月に 1 回（年に 2〜3 回）、依存クレートの更新やセキュリティ修正を兼ねた保守リリースを出せば切れない。見張り（`update-canary.yml`）は期限の 60 日前（発行から 120 日後）から失敗して知らせる（G.6）。レビュー第 2 回の前の既定は 400 日（年に 1 回のリリース）だった。
4. **`expires` は「凍結の検知」のための助言で、インストールの可否には使わない**（C.6）。
   - 期限切れの更新情報が差す版でも、署名が正しく、入っている版より新しいなら、それを入れて状態が悪くなることはない。
   - 期限を門にすると、メンテナーが半年（180 日）動けないだけで、正しい最新版すら自動で入らなくなる。
   - 古い更新情報の再送（巻き戻し）は `issued_at` の記録で、ダウングレードは版の比較で防ぐ（C.4、C.5）。
   - **凍結**（新しい版を隠され、古いが正しく署名された更新情報を見せ続けられる）への備えを、レビュー第 2 回で正直に書き直した（RED-TEAM-1）。現実的な凍結は、GitHub のリポジトリに書ける攻撃者が新しいリリースの「最新」の印を外す（または削除する）か、TLS を検査する社内のプロキシを握った攻撃者が、利用者がすでに持っている版の更新情報を返し続けることで起きる。このとき、確認は毎回**成功**し（`last_success` が進む）、`issued_at` は記録と同じ（巻き戻しにならない）で、画面は「最新です」のままになる。見分ける手がかりは次のとおりで、**30 日の確認の失敗のバナーは凍結には効かない**（それが知らせるのは、つながらない、形式が読めない、など失敗が続くときだけ）。
     1. `expires`: 凍結を知らせる**唯一の自動の信号**。隠された版に気付くまで、最長で `expires - issued_at`（既定 180 日。J-3 の決定）かかる。
     2. 巻き戻しの警告（E.3）: 隠された新しい更新情報を、この PC のどれかの GUI か CLI が一度でも受け取っていた場合だけ（`RecordTrust` で機械の記録にも伝わる）。
     3. 人の目: 更新のページの「最新です」の行に、受け取った更新情報の公開日を出す（「MKLM は最新です（0.2.0。2026/10/15 の更新情報）」。E.2）。`update --check --json` も `issued_at` を出す（D.14）。
     4. メンテナーの見張り: `update-canary.yml` が、配られている版がリポジトリの最も新しい安定版のリリースより古ければ失敗する（G.6）。リポジトリを握った攻撃者はワークフローも止められるので、主に事故の検出になる。
   - 期限とは別の「更新情報が古い」の信号（たとえば 90 日）は採らなかった: 期限をもう 1 つ短く持つのと同じで、メンテナーがその間隔でリリースしなければ全員に誤報が出る。凍結を早く知らせるために、期限そのものを短くした（J-3 の決定 (b)。180 日）。期限は門ではない（この項の初め）ので、短くしたときの代償は、メンテナーがリリースしない間に出る情報の表示だけである。
5. **期限が切れたときに利用者が見るもの**: 設定の「更新」欄と更新のページに、情報として「更新情報の有効期限（2027/04/13）を過ぎています。新しい版が長く公開されていないか、古い情報が届いています。GitHub のリリース ページで確かめてください。［リリース ページを開く］」と出す。加えて、メイン画面に情報のバナーを 30 日に 1 回だけ出す（E.3。レビュー前は出さなかった。SECURITY-11）。CLI の `update --check` は警告の行を 1 行出す。キーボードの機能には何も影響しない。
6. 間違った `latest.json` を公開してしまった、または `latest.json` なしで公開してしまった場合は、そのリリースに「プレリリース」の印を付けて「最新」から外す（公開後も変えられる）。直前のリリースが「最新」に戻る。そのあと次の版（X.Y.(Z+1)）を出す（B.5 の「リリースの事故」）。クライアントの側では、ハッシュが違えばダウンロードで止まり、形式が違えば「更新を確認できません」になるだけで、害はない。
7. 公開の前の確認は道具で行う（B.3 の `prepare-release` と `publish`）。公開の後は、`publish` の確認と、CI の見張り（`update-canary.yml`。G.6。OPS-UX-TEST-3）が確かめる。

### B.5 メンテナーのリリース手順（チェックリスト）

この節は `docs/maintainer/release-signing.ja.md`（WP-U）にそのまま写す。

**準備（初回だけ）**

最初の項目（公式の `minisign` の用意と確認）は、F.6 のリハーサルの**前に**行う（リハーサルも同じ道具で署名するため）。2 つ目以降（本物の鍵の生成など）は、F.6 の後片付けが済んでから行う（G.1 の順序。FIX-VERIFICATION-17）。

- [ ] 公式の `minisign` を用意する: `https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-win64.zip` とその `.minisig` をダウンロード → `cargo xtask verify-signer --zip … --sig …` → 表示された SHA-256 を `release-signing.ja.md` に記録 → 確かめた zip を残しておく（F.6 はここから取り出した `minisign.exe` を使う）。鍵の媒体への `minisign.exe` の配置は、下の鍵の生成のときに行う。
- [ ] （ここから F.6 の後）確かめた zip の中の `minisign.exe` を、通常用の鍵の USB メモリ（`E:\tools\`）とバックアップ用の媒体の両方に置く。
- [ ] 鍵は普段の開発機の、普段のアカウントで作り、署名する（**J-6 の決定 (a)**、2026-09-29。署名専用のアカウントは作らない）。その前に B.6 の「署名に使う PC の条件」を確かめる: F.6 の後片付けが済み、デバッグ ビルドの MKLM と開発用の鍵（`%USERPROFILE%\mklm-dev-keys\`）が残っていない。
- [ ] ネットワークを切る → エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる → 通常用の USB メモリ（BitLocker To Go）をつなぐ → `Get-FileHash E:\tools\minisign.exe` が記録した値と一致することを確かめる → `E:\tools\minisign.exe -G -p E:\mklm-keys\mklm-primary.pub -s E:\mklm-keys\mklm-primary.key`（パスワードを 2 回）→ 外す。
- [ ] 同じく（ネットワークを切ったまま）、別の媒体でバックアップ用: `F:\tools\minisign.exe -G -p F:\mklm-keys-backup\mklm-backup.pub -s F:\mklm-keys-backup\mklm-backup.key` → 外す → ネットワークを戻す。2 つのパスワードは別々の保管場所に置く（B.6）。
- [ ] 2 つの `.pub`（公開鍵。秘密ではない）を作業用の PC に写し、`cargo xtask pubkey-line --pub … --role primary`、`--role backup` → 表示された 2 行を `crates/mklm-update/trust/anchors.txt` に貼る → `cargo xtask check-keys` → コミット（公開鍵だけ。秘密鍵は決してリポジトリに入れない）。`minisign -G` が表示した鍵 ID と、`pubkey-line` の ID が一致することを目で確かめる（L 章の ID の表記の確認を兼ねる）。
- [ ] 公開鍵を `docs/install-guide.ja.md` の「ファイルが正しいか確かめる」に載せる。
- [ ] GitHub アカウントにハードウェア キーの 2 段階認証。`main` とタグ `v*` の保護（計画 4.3）。
- [ ] リポジトリの設定で immutable releases を有効にする（v0.2.0 から。レビューで「最新」の印を公開後に変えられることが確かめられたので、J 章の質問から外した）。
- [ ] バックアップ用の鍵の点検を 1 回行う（B.6 の手順）。

**毎回（安定版。gh CLI を使う）**

1. `Cargo.toml` の `[workspace.package] version` を上げてコミットし、`main` に入れる。
2. `git tag vX.Y.Z` → `git push origin vX.Y.Z`。
3. Actions の「Release」が緑になり、題が「MKLM vX.Y.Z — UNSIGNED, DO NOT PUBLISH」の下書きに、2 つのインストーラーと `SHA256SUMS` が付くのを待つ（来歴の証明も CI が付ける）。
4. **レビュー**: `git fetch --tags` → `git switch --detach vX.Y.Z` → `git rev-parse HEAD` をメモする（これが `--commit`）。`git diff <直前のタグ>..vX.Y.Z` を読む。特に `.github/`、`installer/`、各 `build.rs`、`Cargo.lock`、`rust-toolchain.toml`、`xtask/`、`crates/mklm-update/trust/`。意図しない変更があれば止める。
5. （任意）同じコミットを手元でビルドし、インストーラーのハッシュを比べる（再現可能なビルドかは確かめていない。L 章。違っても直ちに異常とは言えないので、比べた結果を記録するだけ）。
6. `cargo xtask prepare-release --tag vX.Y.Z --commit <SHA> --main-key-id <通常用の ID> --out release-work\vX.Y.Z`。表示された UTC の日付と、取り残しの表と、2 つのインストーラーの SHA-256 を見る。ここで `xtask` がコンパイルされる（**鍵はまだつながない**）。
7. **ネットワークを切る**（Wi-Fi をオフ、ケーブルを抜く）。エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる。鍵をつないでいる間は、`minisign.exe` 以外を起動しない（J-6 の決定 (a) の注意。B.6）。
8. 通常用の USB メモリをつなぐ。`Get-FileHash E:\tools\minisign.exe` が記録した SHA-256 と一致することを確かめる。（任意）`Get-Content …\latest.json` で版と 2 つの SHA-256 を表示し、手順 6 で見た値と一致することを確かめる。
9. 署名のコマンドを実行し、`minisign` にパスワードを入れる。`SIGN-OFFLINE.txt` の行を貼ってよいが、貼る前に、それが 1 行（副署名があれば 2 行）だけで、`release-signing.ja.md` に載せた次の固定の形と、版と `issued_at` の値のほかは同じであることを目で確かめる（紛れ込んだ別のコマンド、たとえば `&` の後ろや 3 行目がないこと）:
   ```
   E:\tools\minisign.exe -S -s E:\mklm-keys\mklm-primary.key -m release-work\vX.Y.Z\latest.json -x release-work\vX.Y.Z\latest.json.minisig -t "mklm-latest-json v1 version=X.Y.Z issued_at=<数字>"
   ```
   （副署名があるときは、鍵と `-x` の名前を変えた 2 行目。）値を写し間違えても、`publish` が trusted comment の不一致で公開を止める（B.3）。秘密鍵に触れるのは公式の `minisign` だけ（0.2 の 9。J-9 の決定 (a)）。
10. USB メモリを外す。**ネットワークを戻す。**
11. `cargo xtask publish --tag vX.Y.Z --dir <手順 6 の --out>`。署名の検証、アップロード、ファイルの組の確認、公開、公開後の確認（インストーラーのダウンロードを含む）が終わるのを待つ。
12. Actions の「Update canary」（公開で動く）が緑であることを確かめる。
13. `git switch main`。手順 6 の `--out` のフォルダーは消してよい（公開のデータだけ）。

**毎回（gh CLI が使えない場合）**

- 自動更新の経路に関わるので、`gh` なしでは公開しない。`gh` を入れて（`winget install GitHub.cli`）上の手順で行う。`gh` が下書きをタグ名で扱えることは、レビューが cli/cli のソース（`FetchRelease` が下書きをタグ名で探す）で確かめた（I.4 は解決）。

**プレリリース（例: `v0.3.0-rc.1`。OPS-UX-TEST-14）**

- タグを push すると、release.yml は `--prerelease` 付きの下書き（題「MKLM v0.3.0-rc.1 (pre-release, no auto-update)」）を作る。
- 署名しない。`latest.json` を付けない。`xtask publish` は使わない（拒否する）。
- 公開は `gh release edit v0.3.0-rc.1 --draft=false --prerelease`。**`--latest` は決して付けない**（プレリリースは「最新」にならないが、念のため）。

**リリースの事故（runbook。OPS-UX-TEST-2）**

| 事態 | すぐにすること | その後 |
|---|---|---|
| `latest.json` か署名なしで、または間違った `latest.json` で公開してしまった | `gh release edit vX.Y.Z --prerelease`（「最新」から外れ、直前のリリースが「最新」に戻る）→ `cargo xtask verify --remote` で直前の版が配られていることを確かめる → リリースノートの先頭に「この版は使わないでください」 | X.Y.(Z+1) を通常の手順で出す（タグは再利用できない） |
| 署名したインストーラーが意図しないものだった（後で分かった） | 上と同じ。加えて、鍵の漏れの疑いがあれば B.7 | 原因を調べ、修正版を出す。Issue で知らせる |
| 見張り（`update-canary.yml`）が失敗した | Actions のログで理由を見る（更新情報がない、署名が通らない、tag が合わない、期限まで 60 日未満、インストーラーのハッシュ違い） | 期限なら保守リリースを出す。GitHub の配布の変更なら、クライアントの修正版を出し、手で入れてもらう案内（G.7 の 1） |

**注**

- 少なくとも 5 か月に 1 回は保守リリースを出す（有効期限の既定は 180 日。B.4 の 3、J-3 の決定）。見張りは期限の 60 日前から失敗して知らせるが、公開のリポジトリの定期のワークフローは 60 日間活動がないと GitHub が止めるので、`prepare-release` が表示する有効期限の日付の 2 か月前をカレンダーにも入れる。

### B.6 鍵の保管と点検

- 通常用の秘密鍵: 暗号化した USB メモリ（BitLocker To Go）に、公式の `minisign.exe` と一緒に置き、署名するときだけつなぐ。パスワードはパスワード マネージャー A に置く。
- バックアップ用の秘密鍵: 通常用とは別の媒体で、別の場所に置く（例: 自宅の金庫と別の建物）。`minisign.exe` の写しも同じ媒体に置く。日常では使わない。**パスワードは、通常用のパスワードとは別の保管場所に置き、バックアップ用の媒体と同じ場所にも置かない**（例: 紙に書いて封をし、媒体とは別の金庫。パスワード マネージャー A をなくすと 2 本とも使えなくなる、を避ける。OPS-UX-TEST-13）。
- 秘密鍵をクラウドの同期フォルダー、GitHub、CI、開発機のディスクに置かない。`xtask` は秘密鍵を開かない。
- **署名に使う PC の条件**（**J-6 の決定 (a)**、2026-09-29: 普段の開発機の、普段のアカウントで署名する。SECURITY-2、SECURITY-9）
  - 署名の間はネットワークを切る。鍵をつないでいる間は、`minisign.exe` 以外を実行しない（エディター、ブラウザー、`cargo` を動かしているターミナルは先に閉じる）。
  - F.6 のデバッグ ビルドの MKLM が入っていないこと、開発用の鍵（`%USERPROFILE%\mklm-dev-keys\`）が残っていないこと（F.6 の後片付け）。
  - 秘密鍵に触れるのは、鍵の媒体に置いた、ハッシュを確かめた公式の `minisign` だけ（J-9 の決定 (a)）。`xtask` もほかの道具も鍵のファイルを開かない。
  - Windows と Defender が最新であること。
- **残る危険**（FIX-VERIFICATION-16。**ユーザーが 2026-09-29 に受け入れた**。J-6、G.7 の 21）: `xtask` は鍵に触れないが、`cargo xtask prepare-release`（B.5 の手順 6）は、タグの `xtask` と、`Cargo.lock` のすべてのビルド スクリプトと proc-macro を、メンテナーの権限でコンパイルして実行する。ふだんの開発でも毎日同じことが起きる。`Cargo.lock` の差分のレビューは版を見るだけで、コードは読まない。侵された依存のクレートが利用者の権限で常駐するプログラムを仕込めば、数分後につないだ USB メモリの `E:\mklm-keys\*.key` を写し、`minisign` に打ち込むパスワードを記録し、手順 10 でネットワークが戻った後に送り出せる。そうなれば、オフラインの鍵はオンラインの鍵と同じだけ危うい。ネットワークを切るだけでは防げない。署名は普段のアカウントで行うので、この危険は上の注意では消えない。将来、危険を下げたくなったときの安い順の対策（今は採らない）:
  1. 署名専用の Windows のアカウント（J-6 の選択肢 (c)）: 標準ユーザーで、cargo も git も一度も実行しない。開発用のアカウントが書いたファイルを実行しない、貼らない。署名の前に開発用のアカウントから**サインアウト**する（ユーザーの切り替えでは、そのアカウントのプロセスが動き続け、BitLocker To Go で開いた USB メモリ（exFAT / FAT にはアクセス権がない）を読める）。利用者の権限の常駐プログラムを締め出せるが、管理者の権限まで奪われていれば防げない。
  2. 署名専用の PC（J-6 の (b)）、または毎回きれいな状態から起動する仮想マシンやライブ USB で署名する。
  3. `prepare-release` を別の PC で実行し、公開のデータ（`latest.json`、`trusted-comment.txt`、`SIGN-OFFLINE.txt`）だけを署名する PC に運ぶ。
  - 同じ理由で、署名するもの（`latest.json` の中身）の正しさは、それを作った開発用のアカウントを信頼している。B.5 の手順 8 の任意の照合（手順 6 で表示した値との比較）は、受け渡しの間の差し替えを見つけるだけで、`xtask` そのものが侵された場合は、来歴の証明と公開後の `verify --remote --installers` と見張り（G.6）が後から見つける（G.7 の 19）。
- **点検（年に 1 回、バックアップ用。通常用の鍵を新しくした後にも）**
  1. `cargo xtask key-drill start --role backup --out drill-2027`（ネットワークはつないだまま、鍵はつながない）。
  2. ネットワークを切る → エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる → バックアップ用の媒体をつなぐ → `minisign.exe` の SHA-256 を確かめる → 手順 1 の `SIGN-OFFLINE.txt` のコマンド（`release-signing.ja.md` の固定の形 `F:\tools\minisign.exe -S -s F:\mklm-keys-backup\mklm-backup.key -m <out>\nonce.bin -x <out>\nonce.bin.minisig -t "mklm-key-drill v1"` と同じであることを目で確かめてから。`<out>` は手順 1 のフォルダー。B.5 の手順 9 と同じ理由）を実行してパスワードを入れる → 外す → ネットワークを戻す。
  3. `cargo xtask key-drill check --dir <手順 1 の --out> --role backup` → 「窓の中の版（B.3）がすべて信頼するバックアップ用の鍵（ID …）で署名されています」。
  4. `release-signing.ja.md` の点検の記録に、日付、鍵 ID、結果を 1 行足す。
  - これで確かめられること: 媒体が読める、パスワードを覚えている、**その鍵が出荷したビルドに埋め込んだバックアップ用の鍵そのものである**（古い鍵のファイルを取り違えていない）。レビュー前の「適当なファイルに署名する」では、最後の点を確かめられなかった。

### B.7 鍵が漏れたとき、なくしたとき（SECURITY-4、OPS-UX-TEST-1）

前提: 利用者の版は、その版の信頼の起点ファイルの鍵しか知らない。古い版が新しい版の更新情報を受け付けるには、主署名か副署名のどちらかが、古い版の知っている失効していない鍵のものでなければならない。`prepare-release` の取り残しの検査（B.3 の手順 10）が、これを版ごとに確かめる。以下、P1 / B1 を今の通常用 / バックアップ用、P2 / B2 を新しい鍵とする。

| 事態 | 次の版 N の信頼の起点ファイル | N の `latest.json` の署名 | N+1 以降の署名 | 取り残される利用者 | 利用者の側の危険 |
|---|---|---|---|---|---|
| 通常用 P1 が漏れた（疑いを含む） | primary P2、backup B1、revoked P1 | 主: B1、副: P2。`revoked_keys` に P1（B1 が P1 を失効させる。B.2 の規則 2） | 主: P2、副: B1。移行の窓の間（N より前の最後の版が最新でなくなった日、つまり N の公開の日から 400 日。B.3 の「窓の中の版」）、取り残しの検査が副署名を求めなくなるまで続ける。`revoked_keys` は P1 を引き継ぐ | 移行の窓の間に 1 度も確認しなかった PC（400 日以上オフライン）: 主署名は未知の鍵、副署名はなし → 「署名を確かめられません」→ 手で入れ直す | 失効の告知（N か、それ以降の更新情報）を受け取る前に、攻撃者が P1 の署名と GitHub への書き込みの両方を手に入れていれば、その PC には悪意のある更新を入れられる。受け取った後は P1 の署名を拒む |
| 通常用 P1 をなくした、パスワードを忘れた（漏れてはいない） | primary P2、backup B1、revoked P1（念のため失効させる。害はない） | 主: B1、副: P2 | 上と同じ | 上と同じ | なし |
| バックアップ用 B1 が漏れた | primary P1、backup B2、revoked B1 | 主: P1（古い版は P1 を知っている）。`revoked_keys` には B1 が入る（B.3 の手順 9）が、B1 を埋め込んだ古い版は、通常用の P1 の署名の中のそれを無視して N を受け付ける（B.2 の規則 2。記録もしないので規則 4 のとおり）。B1 を拒むのは N を入れた版から | 主: P1。`revoked_keys` は B1 を引き継ぐ（古い版は無視し、N 以降の版はビルドで失効済み） | なし（ただし下の攻撃を受けた PC は手で入れ直す） | **通常用の漏れと同じかそれ以上**: 攻撃者は GitHub への書き込みと合わせて、古い版に B1 で署名した悪意のある更新を入れられ、P1 を失効させて正しい更新を止めることもできる。N を入れた PC だけが B1 を拒む。README、リリースノート、Issue で早く N を入れるよう知らせる |
| バックアップ用 B1 をなくした | primary P1、backup B2、revoked B1（念のため） | 主: P1（`revoked_keys` の B1 は、上の行と同じく古い版では無視される） | 主: P1 | なし | なし |
| 両方が漏れた、または両方をなくした | 新しい P2、B2 | 主: P2（古い版は検証できない） | 主: P2 | **すべての利用者**が手で入れ直す | 漏れた場合は、手で入れ直すまで、攻撃者（鍵と GitHub への書き込みの両方）は古い版に悪意のある更新を入れられる。README、リリースノート、Issue で「手で入れ直してください」 |
| 日付の誤った（未来の）更新情報を公開した | 変えない | — | 次の版を正しい日付でふつうに出す（`prepare-release --published-misdated`） | なし（クライアントは記録を受け取った時刻で抑えている。B.2） | なし。期限切れの表示が一時的にずれるだけ |
| `latest.json` なし、または間違ったもので公開した | 変えない | — | B.5 の「リリースの事故」 | 事故の間に確認した PC は、次の確認で戻る | なし（形式か署名で止まる） |

- 通常用の鍵を替えたら、移行の窓の間はバックアップ用の鍵をリリースのたびに使う（副署名）。バックアップ用の鍵が表に出る回数が増えるので、移行の窓が明けたら、バックアップ用の鍵も新しくする（P2 が主署名で、信頼の起点ファイルを「P2、B2、revoked P1、revoked B1」にした版を出す）ことを勧める。P2 を知る版は取り残されない: 「P2、B1、revoked P1」の版は、P2 の署名の中の B1 の失効を無視し（B.2 の規則 2）、P1 の失効はビルドで失効済みとして何もしない。「P2、B2、…」の版は B1 をビルドで失効済みとして扱う。F.1 と `xtask` のテストが、この回転の前後の版で次の版を検証する（FIX-VERIFICATION-1）。
- レビュー第 1 回の後の版は、通常用の鍵がバックアップ用の鍵を失効させる更新情報を拒否（`IllegalRevocation`）していたため、上の 3、4 行目と回転の後の更新情報を、B1 を埋め込んだすべての版が拒み、この表の「取り残される利用者: なし」と矛盾していた（FIX-VERIFICATION-1）。
- レビュー前のこの表は、通常用の鍵をなくした場合の利用者の側を「影響なし」としていたが誤りだった（新しい通常用の鍵だけで署名すると、古い版はすべて取り残される）。
- この表は `docs/maintainer/release-signing.ja.md` にも載せる（WP-U）。

### B.8 v0.1.0 からの移行

- v0.1.0 にはアップデーターがないので、v0.2.0（最初の更新対応版）は利用者が手で入れる。v0.2.0 のリリースノートと README に「この版から自動更新に対応しました。今回だけ手でインストーラーを実行してください」と書く。
- v0.1.0 → v0.2.0 の上書きは M5a の上書きと同じ経路（M5a の実機テストその 2）。設定とジャーナルは引き継がれる。release.yml の煙の試験が、直前のリリース（v0.1.0）からの上書きを毎回確かめる（D.9.3。OPS-UX-TEST-19）。
- v0.2.0 の本番の取得の経路（TLS、プロキシ、GitHub のリダイレクト、tag の取り出し）は、v0.2.1 が出るまで利用者の PC で使われない。そこで v0.2.0 のタグの前に、`cargo xtask fetch-smoke` を開発機で（プロキシの内側があればそこでも）実行し、CI（release.yml と見張り）でも実行する（F.8。OPS-UX-TEST-8）。ここに不具合があると、v0.2.0 の利用者は更新で直せない。
- v0.2.0 を公開したら、v0.2.0 を入れた PC で `mklm-cli update --check --json` が `"status":"up-to-date"`、`"freshness":"fresh"` を返すことを確かめる（F.8）。
- v0.2.0 の最初の自動更新（v0.2.0 → v0.2.1）が、実機での最初の本番の試験になる（F.7）。その前にデバッグ ビルドのリハーサル（F.6）を済ませる。

---

## C. クライアントの検証（`crates/mklm-update`）

### C.1 クレートの構成

`#![forbid(unsafe_code)]`。OS に依存しない（`winhttp` モジュールを除く）。

| モジュール | 役割 | 担当 |
|---|---|---|
| `lib.rs` | 定数（H.1）、`installer_name`、`release_page_url`、`user_agent` | WP-U |
| `keys` | 鍵 ID、指紋、信頼の起点ファイルの解析、`TrustAnchors` | WP-U |
| `manifest` | `Manifest`、`ManifestAsset`、`Arch`、`Sha256Digest`、厳密な解析と正準形 | WP-U |
| `verify` | `verify_manifest`（C.3）、`tries_alternate` | WP-U |
| `state` | `TrustState`（失効と巻き戻しの記録） | WP-U |
| `version` | 版の解析と比較 | WP-U |
| `refusal` | `UpdateRefusal`（拒否の理由のすべて） | WP-U |
| `url` | 厳密な URL、`UrlPolicy`、`Endpoints` | WP-U |
| `fetch` | `Transport`、`fetch_manifest`、`fetch_alt_signature`、`fetch_latest_file`、`download_asset` | WP-U |
| `stage` | helper がインストーラーを受け取るときの状態機械（`Stager`） | WP-U |
| `run` | 更新 1 回の記録（`RunRecord`）と結果（`UpdateResult`）、NSIS の終了コードの分類、結果の判定、時間の定数 | WP-H |
| `run_flow` | H2 の手順（D.7）を `RunnerEnv` の上で行う純粋な駆動部（`run_update`）。OS の操作はすべて環境のトレイトの向こう（OPS-UX-TEST-10） | WP-H |
| `gate` | ジャーナルによる更新の可否 | WP-H |
| `winhttp`（feature `winhttp`、Windows） | `Transport` の WinHTTP 実装 | WP-U |
| `base64`（非公開） | 厳密な base64（RFC 4648、パディング必須、正準形でなければ拒否）。鍵 ID と指紋を読むため | WP-U |
| `dev`（非公開、`cfg(all(debug_assertions, mklm_update_dev))`） | 開発用の鍵の読み込み、目印の静的な値（A.10） | WP-U |

依存: `mklm-core`（`Journal`、`BootId`、`Timestamp`、`ProcessIdentity`、`Liveness`）、`serde`、`serde_json`、`thiserror`、`minisign-verify`、`sha2`、`semver`。feature `winhttp` のときだけ `mklm-win`（feature `net`）。

H1 の手順の駆動部（`stage_update`）は、パイプのメッセージを使うので `mklm_ipc::staging` に置く（`mklm-ipc` が `mklm-update` に依存する向きのため。H.2）。

### C.2 信頼の起点

- `TrustAnchors::release()`: 埋め込みの `ANCHORS_TEXT`（B.2 の信頼の起点ファイル）だけから作る。ビルドのプロファイルや環境変数によらない。鍵が 1 本もなければ `KeyError::NotConfigured`。
  - 各鍵を `minisign_verify::PublicKey::from_base64` で読み、base64 を自分でも解いて鍵 ID（2〜9 バイト目）を取り出し、行の ID と一致することを確かめる（`IdMismatch`）。ID の重複は `DuplicateId`。指紋を計算して持つ。
  - `revoked` の行の ID を持つ。
- `TrustAnchors::for_this_build()`: 製品（GUI、CLI、helper）が使う。`release()` と同じ。開発用の cfg のビルドだけ、`MKLM_UPDATE_DEV_PUBKEY` を開発用の鍵として足す（A.10。鍵が `release()` になくても、開発用の鍵があれば作れる）。
- `TrustAnchors::from_file(&AnchorsFile)`: `xtask` が過去のタグのファイルを読むとき。同じ検査をする。
- `TrustAnchors::from_keys(&[(KeyRole, &str)], &[&str])`: テスト用。同じ検査をする。
- 役割の数（通常用 1、バックアップ用 1）は `TrustAnchors::check_release_roles` で確かめ、`xtask check-keys` が呼ぶ。`for_this_build()` は数を問わない（開発用の鍵を足せるように）。

### C.3 検証の手順（`verify_manifest`。GUI、CLI、helper、`xtask` で同じ関数）

入力は `VerifyInput`（H.1）: 更新情報と**1 つの**署名のバイト列、信頼の起点、記録（`TrustState`）、入っている版、アーキテクチャ、今の時刻、tag（取得したときだけ）、目的（`Check` か `Install`）。**どの段階で失敗しても、それより後は何も解析しない。**

1. 大きさ: 更新情報が `MAX_MANIFEST_LEN` 以下（`ManifestTooLarge`）、署名が `MAX_SIGNATURE_LEN` 以下（`SignatureTooLarge`）。
2. 署名ファイルが UTF-8 で、`Signature::decode` で読めること（`SignatureMalformed`）。
3. 署名の 2 行目の base64 から鍵 ID を読み、信頼の起点にその ID の鍵があること（`UnknownKey { key_id }`）。
4. その鍵が失効していないこと: ビルドの失効（ID）、記録の失効（ID と指紋の組）（`RevokedKey`）。
5. trusted comment の接頭辞が、その鍵の種類に合うこと（本番の鍵なら `mklm-latest-json v1`、開発用の鍵なら `mklm-dev-latest-json v1`。A.4）（`WrongTrustedComment`）。
6. `PublicKey::verify(manifest, &signature, false)` が成功すること（`BadSignature`）。**この時点までは、更新情報の中身を一切読まない。**
7. 更新情報を厳密に解析する（A.3。`ManifestMalformed`）。
8. `schema == 1`（`UnsupportedSchema`）、`product == "MKLM"`（`WrongProduct`）、`channel == "stable"`（`WrongChannel`）。
9. `key_ids` が 1〜2 個の、重複のない正しい形の ID で（`ManifestMalformed`）、手順 3 の鍵 ID を含むこと（`SignerNotListed`）。知らない ID が含まれていてもよい（移行の間、新しい鍵の ID が並ぶ）。
10. `revoked_keys` の各 ID が正しい形で（`ManifestMalformed`）、B.2 の失効の規則 2 の表に反しないこと（`IllegalRevocation` は署名した鍵そのものの失効だけ）。記録する失効（埋め込みの、署名した鍵以外の通常用の鍵）を `VerifiedManifest::revoked` に集める。埋め込みにない ID と、通常用の鍵の署名の中の埋め込みのバックアップ用の鍵の ID は無視する（FIX-VERIFICATION-1）。開発用の鍵が署名した更新情報の失効は、すべて無視する。
11. `version` と `min_from_version` が A.2 の規則に合うこと（`BadVersion`）。tag があれば `version` と一致（`TagMismatch`）。
12. `issued_at < expires` かつ差が `MAX_VALIDITY_SECS` 以下（`BadTimestamps`）。
13. 巻き戻し: `issued_at` が `state.rollback_threshold(anchors, 手順 10 の失効)` より小さければ拒否（`Rollback { issued_at, seen }`）。B.2 の「巻き戻しの記録」。
14. アセット: `x64` と `arm64` がちょうど 1 つずつで、`name` が `installer_name(version, arch)` と一致し、`size` が 1〜`MAX_INSTALLER_LEN`、`sha256` が 64 桁の小文字の 16 進（`AssetMalformed`）。自分のアーキテクチャのものを選ぶ（ないことは手順の上で起こらないが、`NoAssetForArch` を残す）。
15. 鮮度: `now_unix > expires` なら `Freshness::Expired`、そうでなければ `Fresh`（拒否ではない。C.6）。
16. 申し出の種類（`OfferKind`）: `min_from_version` があり、入っている版がそれより古ければ `ManualRequired`。そうでなく `is_newer(version, installed)` なら `Newer`。それ以外は `UpToDate`。
17. 目的が `Install` のときは、`OfferKind::Newer` 以外を拒否する（`UpToDate` → `NotNewer`、`ManualRequired` → `ManualUpdateRequired`）。

成功すると `VerifiedManifest`（H.1）を返す。記録の更新は呼び出し元が `TrustState::recorded(verified, now_unix)` で行う（C.4）。

**主署名と副署名**（A.5）: 呼び出し元（`mklm_client::update::check`、`xtask`）は、主署名で `verify_manifest` を呼び、失敗が `tries_alternate(&error)`（`UnknownKey` か `RevokedKey`）のときだけ副署名を取って、もう一度 `verify_manifest` を呼ぶ。副署名がない（404）か、副署名でも失敗したときは、主署名の拒否の理由を返す（ログには両方を書く）。どちらの署名で通ったか（`SignatureSlot`）をキャッシュに残し、helper にはその署名だけを送る（D.3）。

### C.4 失効と巻き戻しの記録

| 記録 | 場所 | 書く人 | 読む人 | 信頼 |
|---|---|---|---|---|
| 機械の記録 | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` の `Trust`（D.6） | helper だけ: H1（ステージングを始める前）と、どのセッションでも呼び出し元が送った `RecordTrust`（下） | helper（H1、H2）、GUI、CLI（Users は読める） | helper が使うのはこれだけ |
| 利用者の記録 | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\state.json`（`ClientState` の中の `trust`） | GUI と CLI。確認のたびに | GUI と CLI | 表示のためと、`RecordTrust` の材料 |

- **helper は利用者の記録を読まない**（HKCU と利用者のフォルダーを読まない原則。計画 2.2）。インストールの判断は、機械の記録と埋め込みの鍵だけで行う。
- GUI と CLI は、機械の記録と利用者の記録を `TrustState::merged`（鍵ごとの最大値、失効の和集合）で合わせてから検証し、成功したら `TrustState::recorded` の結果を利用者の記録に書く。
- **記録の値**: `recorded` は、署名した鍵の最大値を `max(今の値, min(issued_at, now_unix))` にし、記録する失効を (鍵 ID、指紋) の組で足す（B.2）。開発用の鍵の更新情報は、何も記録しない。
- **機械の記録を進める経路**（SECURITY-5）
  1. H1 が、ステージングを始める前に、送られた更新情報で進める（D.4）。
  2. **`RecordTrust`**: GUI と CLI は、helper のセッション（キーボードの変更を含む、すべてのセッション）の `Welcome` の直後に、利用者のキャッシュの「最後に検証に通った更新情報と、その署名」を送る。送るのは、利用者の記録が機械の記録より進んでいるとき（`TrustState::is_ahead_of`: より大きい最大値か、機械の記録にない失効を持つ）だけ。
     - helper（`mklm_ipc::staging::record_trust`。H.2）は、書き込みのロックを最大 2 秒待って取り（取れなければ `TrustNotRecorded(Busy)` で何もしない）、機械の記録を読み直し、`apply_trust_report`（`verify_manifest` の `Purpose::Check`、入っている版 = 自分の版、その機械の記録、`for_this_build()`）で検証して合わせ、変わったときだけ書いてから、ロックを放す。返事は `TrustRecorded { changed }` か `TrustNotRecorded(理由)`。
     - どちらの返事でも、セッションはそのまま続く（キーボードの変更を止めない）。署名されたデータなので、送り手を信用する必要はない。
     - 限界: 機械の記録が進むのは、利用者が helper を起動したときだけ（UAC なしに HKLM に書く方法はない。0.2 の 5）。進んでいない機械で、悪意のある GUI が古い正しい更新情報を H1 に送る攻撃（SECURITY-5 の (a)）は、それまでに誰かのセッションで新しい記録が届いていれば防げ、届いていなければ防げない（G.7）。
- 機械の記録が壊れていた（JSON として読めない）場合: helper は空の記録として扱い、ログに残す。書けるのは管理者だけで（D.6）、管理者は信頼の外にいないので、ここで更新を止め続けるより、失効はビルドの一覧と次の更新情報に任せる方がよいと判断した。
- 記録は JSON で、`schema` は 1。未知のフィールドは無視する（自分のデータで、将来の版が足しても害がない）。失効は `{"key_id": …, "fingerprint": …}` の配列（H.5）。

### C.5 版の規則

- 更新情報の版: `X.Y.Z` だけ（A.2）。プレリリースの版は、今のチャネル（stable）には載せない。`xtask` も拒否する。
- 入っている版: 実行ファイルの `CARGO_PKG_VERSION`（GUI、CLI、helper は同じワークスペースの版）。開発用のプレリリースの版（例 `0.2.0-dev.1`）は受け付け、semver の優先順位で比べる（`0.2.0` は `0.2.0-dev.1` より新しい）。ビルド情報（`+…`）は拒否する。
- `is_newer(offered, installed)`: semver の優先順位で `offered > installed`。同じ版は「新しくない」。
- **ダウングレードはしない**: helper（H1 も H2 も）は、自分がコンパイルされた版（= インストールされている版）より新しくない更新情報を拒否する（`NotNewer`）。加えて B.2 の巻き戻しの記録で、この機械が知っている最新の更新情報より古いものを拒否する。
- **スキップ**: 利用者が［この版をスキップ］を押した版（`settings.update.skipped_version`）は、バナーと自動のダウンロードの対象にしない。それより新しい版が出れば、また知らせる。スキップしていても、更新のページには「スキップした版（未ダウンロード）」として出し、［ダウンロード］でダウンロードしてから入れられる（E.2。OPS-UX-TEST-16）。

### C.6 `expires` の扱い

- 検証は通し、`Freshness::Expired` を返すだけ（B.4 の 4）。
- 期限の既定は発行から 180 日（`DEFAULT_VALIDITY_DAYS`。J-3 の決定）。メンテナーが 180 日リリースしなければ、すべての利用者の確認が `Expired` になる（G.7 の 7）。
- 正しく署名された古い更新情報を見せ続けられる凍結を、自動で知らせるのはこの期限だけである（B.4 の 4。RED-TEAM-1）。凍結に気付くまでの最長の時間も 180 日になる。
- GUI と CLI は E.3 と E.6 の文で知らせる。helper は拒否しない（ログに残すだけ）。
- PC の時計が大きくずれていると、期限切れの表示が誤ることがある。インストールの判断には影響しない。

### C.7 アーキテクチャ

- 選ぶアセットは、**動いている MKLM のビルドのアーキテクチャ**（`Arch::of_this_build()`、`cfg!(target_arch)`）にする。GUI と helper は同じインストールなので一致する。
- 理由: ARM64 版は実機で試していない（m2 I.16。ARM64 の PC がない）。ARM64 の PC で x64 版を使っている利用者を、更新のついでに未検証の ARM64 版に切り替えない（J-1 の決定 (a)）。
- ネイティブのアーキテクチャ（`IsWow64Process2` の `pNativeMachine`。x64 のエミュレーションで動く x64 版でも ARM64 を返すと広く報告されている — 未確認）は、GUI の情報表示（「この PC では ARM64 版も使えます」）にだけ使う（`mklm_win::os::native_machine`）。
- helper は、自分のビルドのアーキテクチャのアセットだけを受け付ける。
- ARM64 のインストーラーは x64 の PC で `.onInit` が拒否する（D.9.1 の終了コード 21）ので、誤って選ばれても入らない。

### C.8 アセットの選択と URL

- インストーラーの URL は `Endpoints::asset_url(version, name)` が、`…/releases/download/v<version>/<name>` として作る。`latest/download` を使わないのは、確認とダウンロードの間に新しいリリースが公開されても、検証した版のファイルを取るため。
- ダウンロードでは、大きさと SHA-256 を検証済みの更新情報と照らす（A.8）。一致したものだけを、利用者のキャッシュに正式な名前で置く（E.1）。

---

## D. インストールの流れ

### D.1 全体

```
GUI（非昇格）                 H1（昇格、$INSTDIR\mklm-helper.exe）     H2（昇格、Updates\<run-id>\mklm-update-runner.exe）  NSIS
 確認・ダウンロード・検証（自動）
 ［今すぐ更新］→ UAC の事前説明 → UAC
 パイプを作る、helper を起動 ──UAC──▶ 起動、ハンドシェイク（m2 E.3）
 （必要なときだけ RecordTrust）──▶ 検証して機械の記録を進める（C.4）
 StageUpdate{manifest, signature} ──▶ 検証（C.3、Install）、空き容量
                                    ロック、ジャーナル、記録（Trust）
                                    Updates\<run-id>\ を作る、Run=staging
                 ◀── Update::SendInstaller{name,size,sha256}
 InstallerChunk × N ─────────────▶ 書きながら SHA-256、大きさ
                 ◀── Update::Received{bytes}（1 MiB ごと）
                                    一致 → 自分を runner としてコピー、Run=staged
                 ◀── Update::StartingRunner
                                    最小の環境で CreateProcess ────────▶ 起動（--run-update <run-id>）
                 ◀── Heartbeat（10 秒ごと）                            自分の場所、フォルダー、記録を確かめ、
                                                                       インストーラーを開いて固定、Run=ready
                                    Run=ready を見る（最大 120 秒）
                 ◀── Update::HandedOff{run_id, to_version}
 RunOnce（--after-update）を登録
 案内（最大 15 秒）の後に終了         ロックを放して終了                  H1 の終了を待つ
                                                                       署名、版、SHA-256 を検証し直す
                                                                       ロック、ジャーナル、インストール先の版、空き容量
                                                                       ほかの MKLM に quit-if-idle、終了を待つ
                                                                       ファイルが使われていないこと
                                                                       NSIS を一時停止で作る
                                                                       Run=installing（installer 付き）→ 再開 ──▶ .new に展開、
                                                                       セッションの終了を止める                  名前の変更で入れ替え
                                                                       終了を待つ（15 分、最大 60 分）◀───────── 終了コード
                                                                       ファイルの版をそろって確かめる
                                                                       LastResult、Run を消す、ロックを放す、後片付け
 ◀────────────────── Explorer 経由で "$INSTDIR\mklm.exe" --after-update を非昇格で起動（最後）
 結果を表示                                                            終了
```

### D.2 GUI の前提条件（UAC の前）

［今すぐ更新］は、次をすべて満たすときだけ押せる。満たさないときはボタンを無効にし、理由を文で出す（E.6）。

1. 更新が使える（`Availability::Available`: 信頼の起点に鍵があり、MKLM が `%ProgramFiles%\SHIN DATA CENTER\MKLM` から動いている）。
2. ダウンロード済みで、キャッシュの更新情報を検証し直して通る（`reverify_cached`）。インストーラーの SHA-256 は送りながらもう一度計算する（D.3）。
3. helper のセッションが動いていない（m3 A.4 の「セッションは同時に 1 つ」）。
4. ジャーナルに open な操作がない（`gate::blocker` と `startup::summarize` の `blocks_writes`。helper の判断が本物で、これは UAC を無駄に出さないための下見）。
5. 別の更新が進んでいない（`RunView::InProgress` でない）。

### D.3 パイプの要求（`PROTOCOL_VERSION` 3）

**新しいメッセージ**（型は H.2）

| 向き | メッセージ | 中身 |
|---|---|---|
| 呼び出し元 → helper | `CallerMessage::RecordTrust(TrustReport)` | `manifest`、`signature`（利用者のキャッシュの、最後に検証に通った更新情報と、通った方の署名）。C.4 |
| 呼び出し元 → helper | `CallerMessage::StageUpdate(StageUpdateRequest)` | `manifest`（`latest.json` の UTF-8 の文字列、そのまま）、`signature`（検証に通った方の署名ファイルの文字列、そのまま） |
| 呼び出し元 → helper | `CallerMessage::InstallerChunk(InstallerChunk)` | `offset`（何バイト目からか）、`hex`（小文字の 16 進。1〜64 KiB 分） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::TrustRecorded { changed })` | `RecordTrust` を記録した（`changed` は値が変わったか） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::TrustNotRecorded(UpdateRefusal))` | 記録しなかった（検証に通らない、ロックが取れない、など）。セッションは続く |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::SendInstaller { name, size, sha256, chunk_len })` | 検証に通った。これだけのバイト列を送れ |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::Received { bytes })` | 受け取った量（1 MiB ごと） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::StartingRunner)` | 受け取りを終え、H2 を起動した。H2 の準備を待っている（最大 120 秒）。GUI は「Windows がファイルを確認しています…」と出す（RELIABILITY-5） |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::HandedOff { run_id, to_version })` | H2 に引き継いだ。呼び出し元は終了すること。helper はこの後すぐ終了する |
| helper → 呼び出し元 | `HelperMessage::Update(UpdateMessage::Refused(UpdateRefusal))` | 断った。何も変えていない。セッションは続く（呼び出し元が `Bye`） |

- `Request` の列挙には足さない。`Request` はエンジンに写す要求の一覧（m2 S5）で、更新はエンジンを使わないため。既存の網羅的な `match`（中継の `plans_first` など）に影響しない。
- `RecordTrust` は `Welcome` の直後の 1 つ目のメッセージとしてだけ受け付ける（それ以外で届けばプロトコルの誤り。helper の `serve` は `mklm_ipc::staging::CallerOrder` でこれを確かめる。H.2）。送るかどうかは呼び出し元が決め、送らないのがふつう。
- **Heartbeat**: helper の書き込み専用のスレッドは、`busy` のフラグが立っている間だけ 10 秒ごとに `Event::Heartbeat` を送る（m2 S7。`session.rs` の `write_loop`）。今の `busy` は `CallerMessage::Request` の処理の間だけ立つので、`StageUpdate` の処理の間も立てる（RELIABILITY-5）。H1 が H2 の準備を待つ最大 120 秒の間も、呼び出し元の受信の期限が切れない。呼び出し元は更新のセッションで Heartbeat 以外の `Event` を受け取らない（来たらログに書いて無視）。
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

H1 は、既存のパイプのセッション（m2 E.1〜E.3）の中で `CallerMessage::StageUpdate` を受けたときに、`mklm_ipc::staging::stage_update`（純粋な駆動部。H.2）を、`apps/mklm-helper/src/update.rs` の `StagerEnv` の実装（mklm-win を使う）で動かす。

1. 自分の実行ファイルのフォルダーが `mklm_win::os::fixed_install_dir()`（`%ProgramFiles%\SHIN DATA CENTER\MKLM`）であること（`NotInstalledCopy`）。開発用のビルド（`target\…`）は更新しない。
2. 機械の記録を読む（`update_store::read_update_store`。壊れていれば空。C.4）。
3. `verify_manifest`（目的 `Install`、入っている版 = 自分の `CARGO_PKG_VERSION`、アーキテクチャ = 自分のビルド、信頼の起点 = `TrustAnchors::for_this_build()`、記録 = 機械の記録。鍵をまたいだ巻き戻しの判断。B.2）。失敗は `Refused`。
4. 空き容量: `%ProgramData%` のボリュームに、インストーラーの大きさ + 16 MiB 以上（`DiskFull { needed, available }`。RELIABILITY-2）。
5. `ensure_protected_dir(Base)` → `FileLock::acquire(10 秒)`（`Busy`）。
6. ジャーナルを読み（`read_journal_store` → `Journal::parse`）、`gate::check_journal`: 読めない項目があれば `JournalUnreadable`、書き込み中の項目があれば `RecoveryNeeded`、ほかの open な項目があれば `OperationOpen { waiting_for_reboot }`。
7. `Run` の記録を見る（`classify_run`）: `InProgress`（`stager`、`runner`、**`installer`** のどれかが生きている。RELIABILITY-4）なら `UpdateInProgress`。死んでいれば（起動 ID が違うか、プロセスがない）中断として `LastResult` に `interrupted_result` を書き、`Run` を消す。
8. 古い実行のフォルダーを片付ける（D.11 の `sweep_stale_run_dirs`）。
9. 機械の記録を進める: 読み直した `Trust` に `TrustState::recorded(verified, now)` を合わせて書く（`RegFlushKey`）。
10. `ensure_protected_dir(Updates)` → `RunDir::create(<run-id>)`（`run-id` は版と `BCryptGenRandom` の 8 バイト。フォルダーは `PRIVATE_DIR_SDDL` を明示して作る。すでにあれば失敗）。
11. `Run` を書く: `phase = staging`、`caller`（パイプのサーバーの PID から、`proc_identity::process_identity` で作成時刻を付けたもの）、`caller_session`（`GetNamedPipeServerSessionId`）、`stager`（自分）、起動 ID、時刻。
12. `latest.json` と、受け取った署名を `latest.json.minisig` の名前で、受け取ったバイト列のまま `RunDir::write_new` で書く（`CREATE_NEW`、`FlushFileBuffers`）。H2 はこの 1 つの署名だけで検証し直す。
13. `RunDir::create_exclusive(installer_name)`（共有なし）を開き、`mklm_ipc::staging::receive_installer` で受け取る: `SendInstaller` を送り、各 `InstallerChunk` について `offset` が受け取った量と一致すること、16 進が厳密に読めること、長さが 1〜64 KiB であること、合計が `size` を超えないことを確かめ（`Stager::accept`）、書き、SHA-256 を進める。1 つのフレームの待ちは 30 秒（`CHUNK_WAIT`）、全体は 10 分。
14. 合計が `size` になったら `StagedFile::commit`（`FlushFileBuffers` して閉じる）→ `Stager::finish`（SHA-256 の照合。`InstallerHashMismatch`）。
15. 自分の実行ファイル（`$INSTDIR\mklm-helper.exe`。インストール先なので管理者しか書けない）を `RunDir::copy_in` で **`mklm-update-runner.exe`** としてコピーし、コピーの SHA-256 が元と一致することを確かめる（RELIABILITY-12）。
16. `RunDir::create_private_subdir("tmp")`（`PRIVATE_DIR_SDDL`。H2 と NSIS の `TEMP`。D.9.4）。
17. `Run.phase = staged`。
18. `Update::StartingRunner` を送る。`elevation::spawn_clean(<run dir>\mklm-update-runner.exe, "--run-update <run-id>", runner_environment(<run dir>\tmp), suspended = false)`（`CreateProcessW`。UAC は出ない。作業フォルダーは System32。**環境ブロックは最小のものを明示し、H1 の環境を引き継がない**。D.9.4。SECURITY-6）。
19. `Run.phase` が `ready` になるのを最大 `READY_WAIT`（120 秒）待つ（250 ms ごとに読み直す。その間も Heartbeat が流れる）。H2 が先に終了した、または期限が切れたら: H2 がまだ動いていれば `TerminateProcess` し、そのプロセスのハンドルの終了を最大 5 秒待ってから（ハンドルが閉じる前に消そうとしない。RELIABILITY-5）、フォルダーを消し、`Run` を消して、`Refused(HandOffFailed { detail })`（H2 の終了コードを含む）。
20. `Update::HandedOff { run_id, to_version }` を送る。
21. ロックを放し（`FileLock` を落とす）、パイプを閉じて、終了コード 0 で終わる。

- 13 と 14 の途中で呼び出し元が去った（`Bye`、パイプの切断）、または `Refused` になった場合: ファイルとフォルダーを消し、`Run` を消す。`LastResult` は書かない（利用者は GUI でその場で結果を見ているか、取り消した）。
- H1 がロックを持つのは 5 から 21 まで（ふつう数秒。H2 の準備にウイルス対策ソフトが時間をかけると、最大 2 分あまり）。その間、ほかの書き手は `Busy` になる。
- H2 の準備を速くした（RELIABILITY-5）: レビュー前は、H2 が署名の検証とインストーラーのハッシュを終えてから `ready` にしていた。新しい実行ファイルの初回の起動（Defender のクラウドの確認で 10 秒ほど止まることがある）と 6 MB のハッシュが 20 秒の期限に重なり、遅い PC では毎回「PC を再起動して」になっていた。今は、H2 は自分の場所と記録を確かめてインストーラーを開くだけで `ready` にし、重い検証は H1 が去った後に行う（D.7 の手順 8）。

### D.5 `Updates` フォルダー

```
%ProgramData%\SHIN DATA CENTER\MKLM\Updates\          PRIVATE_DIR_SDDL（SYSTEM と Administrators だけ）
  0.2.1-3f9a0c2b7d1e4a65\                             実行 ID。PRIVATE_DIR_SDDL を明示して作る
    latest.json                                        受け取ったバイト列のまま
    latest.json.minisig                                検証に通った方の署名（主署名か副署名）
    MKLM-Setup-0.2.1-x64.exe                           SHA-256 を確かめたインストーラー
    mklm-update-runner.exe                             H1 のコピー（= H2）
    tmp\                                               H2 と NSIS の TEMP（$PLUGINSDIR はここにできる）。PRIVATE_DIR_SDDL
```

- 使う前に毎回、`SHIN DATA CENTER` から `Updates` までの各階層と実行のフォルダー（と `tmp`）について、所有者、保護された DACL、ほかの SID に書き込み系の権利がないこと、リパースポイントでないことを確かめ、ハンドルで固定する（m2 D.9、`protected_dir`）。先回りして作られた階層は隔離して作り直す（m2 S1）。
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
- 書く helper: H1（`Trust`、`Run`、`LastResult`）、H2（`Run`、`LastResult`）、通常のパイプのセッション（`RecordTrust` の `Trust`。ロックを取った後の片付けで、持ち主の死んだ `Run` を `LastResult` に移す。D.11、D.13。RELIABILITY-11）。
- 読むのは誰でもよい（`read_update_store`。非昇格の GUI と CLI も）。
- **計画と依頼の `Updates\last-result.json` をここに移した理由**（K.3）
  1. `Updates` は SYSTEM と Administrators だけのフォルダー（m2 G.1）で、非昇格の GUI は読めない。
  2. Users が読めるファイルにすると、読み手が開いたままにするだけで書き換えを止められる（m2 I.18）。レジストリの値の書き込みは 1 回の `RegSetValueExW` で原子的で、読み手に止められない。
  3. 標準ユーザーは `HKLM\SOFTWARE` の下にキーを作れないので、先回りの心配がない（m2 C.1）。
  4. ジャーナルの仕組み（DACL の確認、フラッシュ）をそのまま使える。
- アンインストールしても残す（ジャーナルと同じ。M5a のアンインストーラーは `HKLM\SOFTWARE\SHIN DATA CENTER` に触れない）。古い `LastResult` が再インストールの後に誤って出ないよう、GUI は日付と今のインストールとの一致で表示を決める（D.13。OPS-UX-TEST-6）。
- JSON の形は H.5。

### D.7 H2: `--run-update <run-id>`

**コマンドライン**（`mklm_ipc::RunUpdateArgs`。m2 E.4 と同じく、`GetCommandLineW` の生の文字列から `command_line_tail` でプログラム名と空白 1 つを除き、残りを手書きの厳密なパーサーで確かめる）

```
mklm-update-runner.exe --run-update <run-id>
```

`RUN_UPDATE_PATTERN`（先頭から末尾まで一致すること）:

```
^--run-update [0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}-[0-9a-f]{16}$
```

加えて、各数は「`0` か、`0` で始まらない」かつ 65535 以下（PID の範囲の確認と同じく、正規表現の外の規則）。テストは m2 と同じ差分テスト（1 文字の挿入、置換、削除で、パーサーとパターンが同じ結論になること）。

**手順**（`mklm_update::run_flow::run_update` が `RunnerEnv` の上で行う。環境の実装は `apps/mklm-helper/src/run_update.rs`。OPS-UX-TEST-10）

1. 準備: `restrict_dll_search()`（失敗は致命的）。`CoInitializeEx(COINIT_MULTITHREADED)` と、**ほかのどの COM の呼び出しより前に** `CoInitializeSecurity`（認証の水準は既定、なりすましの水準は `RPC_C_IMP_LEVEL_IDENTIFY`、`EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA`。SECURITY-8）。`session_end::shut_down_first()`（`SetProcessShutdownParameters(0x3FF, SHUTDOWN_NORETRY)`。RELIABILITY-3）。作業フォルダーを System32 に。コマンドラインの検証（`exit 2`）、昇格の確認（`exit 4`）、OS の版（`exit 5`）。
2. 自分の実行ファイルのパス（NT 形式）が `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>\mklm-update-runner.exe` であること。`verify_protected_dir(Updates)` と `RunDir::open(<run-id>)` で各階層を確かめて固定する。違えば `exit 7`（記録は書かない）。
3. `Run` を読む: あって、`run_id` が一致し、`phase == staged` で、`runner` がまだないこと。違えば `exit 7`（記録は書かない。誰が起動したか分からないため）。
4. インストーラーを `open_locked`（`FILE_SHARE_READ` だけ、通常のファイルでリパースポイントでないこと）で開く。**このハンドルはインストーラーのプロセスを作るまで閉じない**（書き込み、名前の変更、削除を止めておく。`CreateProcessW` は読み取りと実行の共有でイメージを開くので、このハンドルと両立する — 未確認、F.6 で確かめる）。ハッシュはまだ計算しない。
5. セッションの終了の窓を作る（`SessionEndWindow::spawn_with_answer`。下の「サインアウトとシャットダウン」）。
6. `Run.phase = ready`、`runner = 自分`。（H1 はこれを見て GUI に引き継ぎを知らせ、終了する。）
7. ここから後の失敗はすべて、`LastResult` を書き、`Run` を消し、ロックを持っていれば放し、後片付けをし、GUI を起動し直して（D.10）、`exit 7` で終わる。
8. **検証し直す**: H1（`Run.stager`）の終了を最大 30 秒待つ（`proc_identity::wait_for_exit`）。`latest.json` と `.minisig` を `open_locked` で読み（大きさの上限つき）、機械の記録と `for_this_build()` で `verify_manifest`（目的 `Install`、入っている版 = 自分の `CARGO_PKG_VERSION`、アーキテクチャ = 自分のビルド）。版が `Run.to_version` と一致すること。手順 4 のハンドルからインストーラーを読み、大きさと SHA-256 を確かめる（`NotInstalled(Refused(…))`）。
9. `FileLock::acquire(60 秒)`（`NotInstalled(Refused(Busy))`）。
10. ジャーナルを読み直し、`gate::check_journal`（`NotInstalled(Refused(…))`）。
11. インストール先の版が変わっていないこと: `$INSTDIR\mklm-helper.exe` の VERSIONINFO のビルド ID が、自分のビルド ID と同じ（`InstalledVersionChanged`）。引き継ぎの間に誰かが手で別の版を入れた場合に、古い helper が新しいものを上書きして戻すのを防ぐ。
12. 空き容量: `%ProgramFiles%` のボリュームに、インストーラーの大きさの 4 倍 + 64 MiB 以上（`NotInstalled(DiskFull)`）。展開後の 3 つの exe の大きさは H2 には分からないので、LZMA で圧縮したインストーラーの大きさから多めに見積もる。`.new` と元の exe が一時的に並ぶ（D.9.2）ことも含めた値。
13. `Run.phase = waiting`。ほかの MKLM を終わらせる（D.8 の 1〜3）。
14. ファイルが使われていないこと（D.8 の 4。`NotInstalled(FilesInUse)`）。
15. セッションの終了が始まっていないこと（始まっていれば `NotInstalled(SessionEnding)`。下）。
16. **インストーラーを一時停止で作る**（RELIABILITY-4）: `elevation::spawn_clean("<run dir>\MKLM-Setup-<v>-<arch>.exe", "/S", runner_environment(<run dir>\tmp), suspended = true)`。失敗は `NotInstalled(InstallerNotStarted { code })`（225 / 226 はウイルス対策ソフトによる停止）。
    - 作れたら、`Run` を `phase = installing`、`installer = そのプロセスの識別`（PID と作成時刻）にして 1 回で書き、フラッシュする。**書けなければ、まだ一度も動いていないインストーラーを `TerminateProcess` し、`NotInstalled(Refused(Storage))`。**
    - セッションの終了を止める理由を出す（`ShutdownBlockReasonCreate("MKLM を更新しています。数秒お待ちください")`）。
    - 手順 4 のハンドルを閉じ、`ResumeThread`。
    - この順序で、「インストーラーが動いているのに、記録がそれを知らない」時間をなくす（H2 が落ちても、`Run.installer` が生きている間は誰も更新が終わったとみなさない）。
17. 終了を最大 `INSTALLER_WAIT`（15 分）待つ。
    - 期限を過ぎたら: `LastResult = Failed(InstallerTimedOut)` を書く（`Run` は `installing` のまま残す。**インストーラーは止めない**。ファイルの置き換えの途中で止めると、必ず半端になるため）。さらに最大 `INSTALLER_WAIT_MAX`（合わせて 60 分）待ち続ける。その間に終われば手順 18 へ進み、`LastResult` を本当の結果で書き直す。
    - 60 分でも終わらなければ: 止める理由を消し、ロックを放し、`Run`（`installing`、`installer` 付き）を残したまま `exit 7`。GUI は起動し直さない（インストーラーがまだ `mklm.exe` を置き換えているかもしれないため）。`installer` が生きている間は、誰が見ても `InProgress`。死んだ後に見た人が、ビルド ID で結果を決める（`interrupted_result`。D.13）。
18. 止める理由を消す（`ShutdownBlockReasonDestroy`）。`Run.phase = finishing`。インストール先の 3 つの exe のビルド ID を読み（`InstallState::from_build_ids(update_dir::read_build_ids(install_dir))`。環境のトレイトでは `RunnerEnv::read_install_state`）、`run::decide_outcome` で結果を決める（D.13）。
19. `LastResult` を書き（`gui_relaunch_attempted` は、次の手順で起動を試みるなら true）、`Run` を消し、ロックを放す。
20. 後片付け（D.11）: 自分の実行のフォルダーのインストーラー、2 つの更新情報、`tmp` を消す。
21. **最後に** GUI を起動し直す（D.10。17 の 60 分の期限切れを除く）。
22. 結果が `Installed` なら `exit 0`、それ以外は `exit 7`。

**サインアウトとシャットダウン**（RELIABILITY-3）

- H2 は `SetProcessShutdownParameters(0x3FF, SHUTDOWN_NORETRY)` で、セッションの終了を最初に知らされる（レビュー前の 0x100 は最後だった。NSIS は既定の 0x280 なので、H2 が気付く前にインストーラーが終わらされていた）。
- 見えないトップレベルの窓（`SessionEndWindow::spawn_with_answer`）が `WM_QUERYENDSESSION` に答える。答えと段階の移り変わりは 1 つのロックで順序を決める（手順 16 でインストーラーを作る判断と、セッションの終了の判断が入れ違わないように）。
  - `ready` / `waiting`（インストーラーを作る前）: 「終了が始まった」の印を立てて `TRUE`（止めない）。本体は手順 15 でそれを見て、インストーラーを作らずに `NotInstalled(SessionEnding)` を書き、ロックを放して終わる。書く前に H2 が終わらされても、`Run` が残るだけで、次の GUI が「中断されました。何も変更されていません」を出す。
  - `installing`: `FALSE`（止める）。理由の文は手順 16 で出したもの。Windows は「このアプリがシャットダウンを妨げています」の画面にこの文を出す。利用者が「強制的に…」を選べば止められないが、そのときは D.13 の検出が最後の砦になる。2 段階の置き換え（D.9.2）で、半端になりうる時間は 6 回の名前の変更の間に縮んだ。
  - `finishing` 以降: `TRUE`。
- `ShutdownBlockReasonCreate` は、窓を作ったスレッドから呼ぶ必要がある（Microsoft Learn）。本体からは窓のスレッドに独自のメッセージを送って作り、消す（`SessionEndWindow::set_block_reason`）。

- H2 は `%ProgramData%\SHIN DATA CENTER\MKLM\logs\update.log`（`DataDir::Logs`、SYSTEM と Administrators だけ）に英語で段階と結果、各段階の所要時間（H2 の起動から `ready` までの時間を含む。RELIABILITY-5）を追記する（1 MB で 1 世代を回す）。H1 も同じファイルに書く。キーの内容は書かない（そもそも扱わない）。

### D.8 ほかの MKLM の終了を待つ

インストーラーは、`$INSTDIR` の `mklm-helper.exe`、`mklm-cli.exe`、`mklm.exe` のどれかが動いていると拒否する（0.1）。H2 は先に、次の順で終わらせる。

1. **更新を始めた GUI**（`Run.caller`）は、`HandedOff` を受けて自分から終了する（m3 F.5 の A12）。H2 は最大 30 秒待つ（`CallerDidNotExit`）。H1 が `HandedOff` を送る前に落ちた場合も、GUI は `Run` の記録から引き継ぎを知って終了する（E.4。RELIABILITY-5）。
2. **すべてのセッションの GUI**: `mklm_win::instance::quit_idle_instances($INSTDIR\mklm.exe の NT パス, 1 本 5 秒, 全体 20 秒, 最大 16 本)`（SECURITY-7、RELIABILITY-1、OPS-UX-TEST-4。パイプの名前の決め方を FIX-VERIFICATION-5 で改めた）
   - まず、実行ファイルが `$INSTDIR\mklm.exe` であるプロセスを `proc_identity::processes_with_images` で探す（PID、作成時刻、セッション ID）。GUI が 1 つもなければ何もしない。
   - **パイプの名前は列挙せず、計算する**: `proc_identity::process_users()`（`WTSEnumerateProcessesW`。各プロセスの `SessionId` と、プライマリ トークンの利用者の SID `pUserSid`。Microsoft Learn の `WTS_PROCESS_INFOW`）から、上の各 GUI の PID の行を取り、セッション ID が `processes_with_images` の値と一致することを確かめ、`instance::instance_pipe_path(session, sid)`（既存。GUI が自分のパイプを作るのと同じ関数）で名前を作る。`pUserSid` がない、または 2 つのセッション ID が合わない GUI は、ログに書いて飛ばす（そのセッションの GUI が残れば、3 のプロセスの待ちが決める）。
   - 作った名前を、念のため `instance::parse_instance_pipe_name` の厳密な正規表現（`^SHINDATACENTER\.MKLM\.Instance\.([0-9]{1,10})\.(S-1-5-21(-[0-9]{1,10}){4}|S-1-12-1(-[0-9]{1,10}){4})$`。ローカルのアカウントとドメインは `S-1-5-21-…`、Microsoft Entra ID のアカウントは `S-1-12-1-…`）にも通す。合わない名前は**開かない**。`\\.\pipe\` の列挙はしないので、ほかの利用者が作った偽の名前のパイプで予算を使わされることはない（レビュー第 1 回の後の版は列挙していたので、正しい形の偽の名前を 16 本以上作られると、本物の GUI のパイプまで届かなかった）。
   - 本物の GUI のパイプは `FILE_FLAG_FIRST_PIPE_INSTANCE` と最大 1 インスタンスで作られている（`instance.rs`）ので、GUI が動いている間、同じ名前のインスタンスをほかの誰かが足すことはできない。GUI より先に同じ名前を作られた場合は、GUI 自身が多重起動の仕組みを作れない（m3 の範囲）。その名前のサーバーは下の確認で `NotOurs` になり、GUI は 3 の待ちで `ProgramsStillRunning` になる（G.7 の 16）。
   - 候補は最大 16 本（GUI のプロセスの数。利用者とセッションごとに 1 つなので、ふつうは数本）。超えた分はログに書いて送らない。
   - 各候補を `pipe::open_client`（`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`、重なった I/O）で開く。1 本の期限は 5 秒、全体の期限は 20 秒（`INSTANCES_TOTAL`）。
   - サーバーの確認: `GetNamedPipeServerProcessId` がその GUI の PID であること。`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` で**ハンドルを取り、送り終えるまで持つ**（PID の再利用を防ぐ）。そのハンドルで実行ファイルの NT パスが `$INSTDIR\mklm.exe` であり、`GetNamedPipeServerSessionId` がパイプの名前のセッションと一致することを確かめる。ほかの利用者のプロセスを開けない場合（未確認。I.12）は、作成時刻付きの識別（`process_identity`）を送る前と返事の後で比べ、`processes_with_images` で見た GUI と同じプロセスであることを確かめる。合わなければ何も送らない（`NotOurs`）。
   - 送るのは **`quit-if-idle\n`**（13 バイト。`MAX_INSTANCE_MESSAGE` の 16 以内）だけ。`quit` は使わない。
     - `ok`: その GUI は何もしていない状態で、終了する。H2 は 3 で終わりを待つ。
     - `busy`: その GUI は helper のセッション中か、利用者への問いかけ（終了の確認、再接続待ち、回復の確認）を出している。**GUI は何も変えていない**（取り消しも、後で終了する印も立てない）。H2 は更新をやめる（`NotInstalled(InstanceBusy { sessions })`。`busy` を返した GUI のセッション ID を残し、管理者が誰の MKLM かを調べられるようにする。RED-TEAM-3）。その利用者が自分の GUI に手を加えれば、いつでも `busy` を返させられる（G.7 の 16）。
     - 返事がない（期限切れ、パイプのスレッドが止まっている）: 「終了しなかった」とみなし、3 に任せる。
   - GUI の側の `quit-if-idle` の決まりは E.4.1。GUI は終了する前に、自分の HKCU の RunOnce に `--after-update` を登録する（昇格していなければ。RELIABILITY-7）。
   - **ほかの利用者の GUI について正直に書くと**（RELIABILITY-7、OPS-UX-TEST-21）: ユーザーの切り替えで別の利用者の画面に戻るのはサインインではなく再接続なので、Run キーも RunOnce も動かない。終わらせた GUI は、**その利用者が MKLM を開き直すか、サインアウトしてサインインし直すまで戻らない**（その間、その利用者のトレイのアイコンと通知が消える）。次のサインインでは、RunOnce で結果と「別のユーザーの更新のために MKLM が終了していました」が出る（E.5）。H2 はほかの利用者のセッションに GUI を起動しない（その利用者のトークンを持たない。タスク スケジューラーで起動し直す案は採らなかった。I.11、J-2 の決定 (a)）。
3. **すべてのプロセス**: 実行ファイルが `$INSTDIR` の `mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe` のどれかであるプロセスを `proc_identity::processes_with_images` で探し、なくなるまで待つ。GUI と CLI は 30 秒、helper は最大 75 秒（何もしていない helper は 60 秒で終わる。m2 E.7）。残れば `NotInstalled(ProgramsStillRunning { programs, holders })`（`holders` は残ったプロセスの PID、セッション ID、実行ファイルの名前。RED-TEAM-3）。
4. **ファイルが使われていないこと**（SECURITY-10）: 3 つの exe のそれぞれを `DELETE` の権利と、読み取り、書き込み、削除のすべての共有で開けること（`update_dir::files_in_use`）。ほかのプロセスが削除の共有なしで開いていれば開けない（NSIS の名前の変更も同じ理由で失敗する）。ウイルス対策ソフトの一時的な読み取りを見込み、最大 10 秒、250 ms ごとに試す。開けなければ、開いているプロセスを Restart Manager（`RmStartSession`、`RmRegisterResources`、`RmGetList`。`update_dir::file_holders`）で調べ、`NotInstalled(FilesInUse { programs, holders })` にする（調べられなければ `holders` は空。未確認。I.16）。
   - 標準ユーザーは Program Files のファイルを読めるので、**ほかの利用者は、ファイルを削除の共有なしで開いたままにするだけで、すべての更新（と手でのインストール。NSIS の終了コード 26）を止め続けられる**。そのプロセスが動いている限り、何度試しても入らず、自動で抜ける方法はない（RED-TEAM-3。レビュー第 1 回の後の版は「遅らせる」と書いていたが、実際には期限のない妨害である）。半端に入ることはない（D.9.2）。
   - 管理者ができること: 結果の「技術的な詳細」と `update --status` に出る `holders`（PID、セッション ID、実行ファイルの名前）で相手を見つけ、タスク マネージャーの「ユーザー」タブ（セッションと利用者の対応）でそのプロセスを止めてから、もう一度［今すぐ更新］を押す。PC を再起動した直後、ほかの利用者がサインインする前に更新する方法もある（`docs/recovery.md` に書く。WP-H）。`holders` には利用者の SID を入れない（`LastResult` は Users が読めるため。セッション ID と名前は標準ユーザーもほかの手段で見られる）。
5. インストーラーの `CloseMklm` が、念のためにもう一度確かめる（0.1）。ここで拒否された場合は D.9.1 の終了コードで分かる。

### D.9 NSIS

#### D.9.1 終了コード

`mklm.nsi` のすべての拒否と失敗で、`Quit` の**直前**に `SetErrorLevel` を置く。`.onInit` の `Abort` も `SetErrorLevel` と `Quit` に置き換えて、形をそろえる。

- NSIS のソース（`Main.c`）で確かめたこと（RELIABILITY-10）: 終了のときに `if (g_exec_flags.errlvl != -1) ret = errlvl;`。つまり `SetErrorLevel` の値は、どこで設定しても最後まで残り、`.onInit` の `Quit` でも効く（I.3 は煙の試験での確認を残して、ほぼ解決）。「直前に置く」は必須ではないが、**失敗でない経路にまぎれ込んだ `SetErrorLevel` は、成功のときの終了コードまで変えてしまう**ので、check-nsi.ps1 が「`SetErrorLevel` は `Quit` の直前にしか現れない（アンインストーラーの 3010 を除く）」を確かめる（D.9.3）。

| コード | `!define` | 場面 | 何か置き換えたか |
|---|---|---|---|
| 0 | — | 成功 | はい |
| 1 | —（NSIS の既定） | 利用者が取り消した（`/S` では起きない） | いいえ |
| 2 | —（NSIS の既定） | スクリプトによる中止（下の表にないもの。たとえば文書のファイルの書き込みの失敗） | いいえ（2 段階の置き換えでは、exe を置き換える前にしか起きない。D.9.2。決めるのは D.13 のビルド ID） |
| 20 | `MKLM_EXIT_OS_TOO_OLD` | Windows 11 24H2（build 26100）未満 | いいえ |
| 21 | `MKLM_EXIT_WRONG_ARCH` | ARM64 のインストーラーを ARM64 以外で、または 32 ビットの Windows | いいえ |
| 22 | `MKLM_EXIT_HELPER_RUNNING` | `$INSTDIR\mklm-helper.exe` が動いている（書き込みの共有なしで開かれている） | いいえ |
| 23 | `MKLM_EXIT_CLI_RUNNING` | `$INSTDIR\mklm-cli.exe` が同上 | いいえ |
| 24 | `MKLM_EXIT_GUI_RUNNING` | `--quit` の後も `mklm.exe` が同上（`/S` では「キャンセル」が既定） | いいえ |
| 26 | `MKLM_EXIT_FILES_IN_USE` | exe の名前の変更に失敗した（ほかのプロセスが削除の共有なしで開いている）。入れ替えた分を元に戻し、MessageBox（`$(FILES_IN_USE)`、`/SD IDOK`）を出してから終わる（D.9.2。SECURITY-10） | いいえ |
| 27 | `MKLM_EXIT_FILE_WRITE` | 新しい exe（`.new`）を書けなかった（ディスクの空き、ウイルス対策ソフト）。`.new` を消し、MessageBox（`$(FILE_WRITE_FAILED)`、`/SD IDOK`）を出してから終わる（RELIABILITY-2） | いいえ |
| 3010 | — | （アンインストーラーだけ）アンインストールの復元で PC の再起動が必要（既存） | — |

- 対話のインストール（利用者が手で実行、D.13 の［インストーラーを実行］）で黙って消えないよう、20〜24 と同じく 26 と 27 にも理由の MessageBox を出す（FIX-VERIFICATION-10）。`/SD IDOK` なので `/S` では止まらない。順序は MessageBox → `SetErrorLevel` → `Quit` で、D.9.3 の規則 1〜3 を満たす。文（`LangString`、日英）:
  - `FILES_IN_USE`: 「別のプログラムが MKLM のファイルを開いているため、ファイルを置き換えられませんでした。何も変更していません。しばらくしてから、もう一度実行してください。」
  - `FILE_WRITE_FAILED`: 「新しいファイルを書き込めませんでした。ディスクの空きとウイルス対策ソフトを確かめてください。何も変更していません。」
- 今の `mklm.nsi`（37989f8）の拒否（`CloseMklm` の `Quit`、`.onInit` の `Abort`、`un.onInit` の `Quit`）には `SetErrorLevel` がなく、終了コードは 3010 のほかに決めていない。上の表の 20〜27 は**この設計で決める**もの（FIX-VERIFICATION-10。レビュー第 1 回の後の L 章は「25 は `un.onInit` だけ」を今のコードの事実として書いていたが、誤りだった）。
- 25（`MKLM_EXIT_BAD_INSTALL_DIR`）はインストーラーの表から外した（RELIABILITY-10）。この設計では、これを出すのはアンインストーラーの `un.onInit` だけにする。インストーラーの `.onInit` はインストール先を固定するので出ない。アンインストーラーは自分を `%TEMP%` に写して起動し直す（`Main.c` の `~nsu%X.tmp`）ので、その終了コードは `_?=` を付けて直接実行したときにしか呼び出し元に届かない。`un.onInit` には `SetErrorLevel 25` を付けるが、`mklm_update::run::nsis_exit` には持たない（`InstallerExit::Other(25)`）。
- 同じ値を `mklm_update::run::nsis_exit` の定数に持つ。WP-H のテスト（`crates/mklm-update/tests/nsis_exit_codes.rs`）が `mklm.nsi` を読んで、`!define MKLM_EXIT_*` の値が定数と一致することを確かめる（25 はアンインストーラー専用の印を付けて除く）。
- `InstallerExit::leaves_old_files()`: 20〜24、26、27（何も置き換えていないことが NSIS の側で保証されるもの）。

#### D.9.2 2 段階の置き換え（RELIABILITY-2、SECURITY-10）

今の `mklm.nsi` は、3 つの exe を `File` でその場で上書きする。`File` は `CREATE_ALWAYS` で開くので、書き込みの途中で失敗すると古い exe は切り詰められ、新しい中身も入っていない。しかも既定の `AllowSkipFiles on` では、サイレントの既定の答えが「無視」（エラーの印を立てて続ける）と報告されており（NSIS の `exec.c` はメッセージ ボックスの種類と既定の答えをコンパイル時に決める。煙の試験で確かめる）、今のスクリプトは `${Errors}` を見ないので、終了コード 0 のまま半端になる。そこで次の形に変える。

```nsis
Section "MKLM" SecMain
  SectionIn RO
  Call CloseMklm
  SetShellVarContext all
  SetOutPath "$INSTDIR"

  ; 1. Documents first: a failure here aborts (exit 2) before any executable is touched.
  AllowSkipFiles off
  File /oname=LICENSE.txt "${ROOT}\LICENSE"
  File /oname=recovery.md "${ROOT}\docs\recovery.md"
  File /oname=install-guide.md "${ROOT}\docs\install-guide.ja.md"

  ; 2. The new executables next to the old ones. Nothing is replaced yet.
  !insertmacro DELETE_LEFTOVERS          ; *.new and *.old of an earlier attempt
  AllowSkipFiles on                      ; silent default: set the error flag and go on
  ClearErrors
  File "/oname=$INSTDIR\mklm-helper.exe.new" "${SRCDIR}\mklm-helper.exe"
  File "/oname=$INSTDIR\mklm-cli.exe.new" "${SRCDIR}\mklm-cli.exe"
  File "/oname=$INSTDIR\mklm.exe.new" "${SRCDIR}\mklm.exe"
  AllowSkipFiles off
  ${If} ${Errors}
    !insertmacro DELETE_NEW
    MessageBox MB_OK|MB_ICONSTOP "$(FILE_WRITE_FAILED)" /SD IDOK
    SetErrorLevel ${MKLM_EXIT_FILE_WRITE}
    Quit
  ${EndIf}

  ; 3. Swap by renames (same volume; a running image may be renamed, but CloseMklm has already
  ;    made sure nothing runs). On any failure the swapped ones are put back, then exit 26.
  Call SwapExecutables

  ; 4. The old copies. No /REBOOTOK: a leftover is removed by the next install (DELETE_LEFTOVERS).
  Delete "$INSTDIR\mklm-helper.exe.old"
  Delete "$INSTDIR\mklm-cli.exe.old"
  Delete "$INSTDIR\mklm.exe.old"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; shortcut and ARP values as before
SectionEnd
```

- `SwapExecutables`: `mklm-helper.exe`、`mklm-cli.exe`、`mklm.exe` の順に、(a) 元があれば `Rename <名前> <名前>.old`、(b) `Rename <名前>.new <名前>`。どこかで失敗したら、それまでに入れ替えた分を逆の順に戻し（新しい方を消し、`.old` を元の名前に戻す）、残りの `.new` を消して `MessageBox MB_OK|MB_ICONSTOP "$(FILES_IN_USE)" /SD IDOK` → `SetErrorLevel ${MKLM_EXIT_FILES_IN_USE}` → `Quit`。
- 名前の変更は、ほかのプロセスがそのファイルを削除の共有なしで開いていると失敗する。H2 は NSIS の前に同じ条件を確かめている（D.8 の 4）ので、ここで失敗するのは、その後に開かれた場合だけ。
- `AllowSkipFiles on` は `.new` の展開の間だけにする: ここでは「失敗したら印を立てて続ける」の方が、後で `${Errors}` を見て 27 で終われるので都合がよい（置き換える前なので、飛ばしても害がない）。もし煙の試験で、サイレントの既定の答えが「中止」だと分かっても、終了コードが 2 になるだけで、何も置き換えていないことは変わらない。
- 半端になりうるのは、(b) の 3 回の名前の変更と、(a) の 3 回の間だけ（ミリ秒の単位）。そこで電源が落ちても、`.old` と `.new` が残り、D.13 の検出と、次のインストールの `DELETE_LEFTOVERS` で直る。
- 置き換えの失敗の半端（古い exe が切り詰められ、GUI も helper も起動できない）がなくなるので、レビュー前の「インストーラーを手で実行する」以外に直す手段がない状態は、ほぼ電源断のときだけになる。

#### D.9.3 CI での確認（WP-H。RELIABILITY-10、OPS-UX-TEST-19）

**静的な検査**（`installer/check-nsi.ps1`。ci.yml）

1. すべての `MessageBox` に `/SD` があること（サイレントで止まらないため）。
2. すべての `Quit` の直前が `SetErrorLevel` であること。`.onInit` に `Abort` がないこと。
3. `SetErrorLevel` は `Quit` の直前にしか現れないこと（アンインストーラーの 3010 を除く）。
4. `uninstall.exe` と `--uninstall-restore` を実行する行が、`un.` の関数とアンインストールのセクションにしかないこと。
5. 3 つの exe を書く `File` は、`/oname=…\<名前>.new` の形だけであること。`AllowSkipFiles on` は `.new` の展開の直前にだけ現れ、直後に `off` に戻ること。インストールのセクションに `/REBOOTOK` がないこと。
6. **実行中の MKLM は `$INSTDIR` のパスでだけ見つける**: プロセスの名前で探す命令やプラグイン（`FindProcDLL`、`nsProcess`、`Processes::`、`KillProc`、`tasklist`、`taskkill`）がないこと（RELIABILITY-12）。

**煙の試験**（`installer/smoke-test.ps1`。ランナーは `windows-2025` に固定する。`.onInit` と `--uninstall-restore` は build 26100 未満を拒否するので、`windows-latest` の中身が変わってもリリースが理由なく落ちないように。`windows-2025` が build 26100 であることは未確認。使い捨ての VM で、管理者として動く）

1. **直前のリリースからの上書き**: 公開中の最新のリリースの x64 のインストーラーを `gh release download` で取り、`/S` → 0。`HKLM\SOFTWARE\SHIN DATA CENTER` を書き出しておく。新しいインストーラーを `/S` → 0。3 つの exe のビルド ID がそろって新しい版であること。`HKLM\SOFTWARE\SHIN DATA CENTER` が変わっていないこと。
2. もう一度 `/S` → 0（同じ版の上書き）。この 1 回は D.9.4 の最小の環境ブロックで実行する。
   開いたままにするハンドルの権利と共有は、NSIS の `IsLocked` と合わせて正確に決める（FIX-VERIFICATION-10）。`IsLocked` の `FileOpen … a` は NSIS の `myOpenFile` を通り、`CreateFile` を**書き込みを含む権利と、共有 `FILE_SHARE_READ` だけ**で開く（共有は NSIS の `util.c` の `myOpenFile` で確認。`a` が読み書きか書き込みだけかはコンパイラーの対応表で、未確認だが、どちらでも下の結論は同じ）。したがって、先に開いておくハンドルの権利が読み取りだけなら `IsLocked` は通り、書き込みを含めば通らない。
3. PowerShell で `[IO.File]::Open("$INSTDIR\mklm-cli.exe", 'Open', 'Read', 'None')` のまま `/S` → 23（共有なしなので `IsLocked` が開けない）。3 つのビルド ID が変わっていないこと。
4. `[IO.File]::Open("$INSTDIR\mklm.exe", 'Open', 'Read', 'ReadWrite')` のまま `/S` → 26。権利は読み取りだけ（`IsLocked` の書き込みの開き方と両立するので「動いていない」とみなされ、置き換えまで進む）、共有は読み取りと書き込みで削除なし（名前の変更が失敗する）。3 つのビルド ID が変わっておらず、`.new` と `.old` が残っていないこと（2 段階の置き換えの戻し）。権利を `ReadWrite` にすると `IsLocked` が「動いている」と判断して 24 になり、戻しの経路を試さなくなるので、そうしない。
5. `[IO.File]::Open("$INSTDIR\mklm.exe", 'Open', 'Read', 'Read')` のまま `/S` → 24（共有が読み取りだけなので `IsLocked` の書き込みの開き方が失敗し、「動いている」とみなす。`--quit` の後も同じ。今の決まりの記録）。
6. ARM64 のインストーラーを `/S` → 21。
7. アンインストール: `Start-Process -Wait "$INSTDIR\uninstall.exe" -ArgumentList '/S', "_?=$INSTDIR"`（`_?=` を付けると、自分を写して起動し直さず、終わるまで待て、終了コードも届く）→ 0。3 つの exe が消えていること（`_?=` のときは `uninstall.exe` 自身とフォルダーが残るので、試験が消す）。

- 実行する場所: release.yml（ビルドの後、下書きを作る前。失敗すれば下書きを作らない）と、新しいワークフロー `installer.yml`（`pull_request` と `main` への push で、`installer/**`、`apps/**`、`Cargo.lock`、そのワークフロー自身が変わったとき。タグを打つ前に NSIS の後退に気付くため）。
- 27（書き込みの失敗）は CI で再現しにくいので、静的な検査（5）とスクリプトの読み合わせで確かめる。

#### D.9.4 H2 からの実行（SECURITY-6）

- `"<run dir>\MKLM-Setup-<v>-<arch>.exe" /S`。`/D=` は渡さない（`.onInit` がインストール先を固定する）。
- H2 は昇格しているので、インストーラーの `RequestExecutionLevel admin` による UAC は出ない。
- **環境ブロック**: H1 が H2 を起動するときも、H2 が NSIS を起動するときも、`CREATE_UNICODE_ENVIRONMENT` で次だけを含むブロックを明示して渡す（`elevation::runner_environment`。親の環境は何も引き継がない）。作業フォルダーは System32。
  | 変数 | 値 |
  |---|---|
  | `SystemRoot`、`windir` | `GetSystemWindowsDirectoryW` |
  | `SystemDrive` | その先頭の `X:` |
  | `ComSpec` | `<System32>\cmd.exe` |
  | `PATH` | `<System32>;<Windows>;<System32>\Wbem` |
  | `ProgramData`、`ProgramFiles`、`ProgramW6432` | 既知のフォルダー（`FOLDERID_ProgramData`、`FOLDERID_ProgramFiles`、`FOLDERID_ProgramFilesX64`） |
  | `TEMP`、`TMP` | `<run dir>\tmp`（`PRIVATE_DIR_SDDL`。D.5） |
  - 理由: UAC で起動した H1 の環境には、非昇格の利用者が書き換えられる `HKCU\Environment` の値（`TEMP`、`TMP`、`PATH`、`__COMPAT_LAYER` など）が入る。NSIS は `$TEMP` を `TMP` / `TEMP` から決め、そこに `$PLUGINSDIR` を作って `System.dll` などのプラグインを読み込む（`.onInit` の `x64.nsh` の判定も System プラグインを使う）。NSIS 3.11 の修正（CVE-2025-43715）は `$PLUGINSDIR` 自体の作り方を直したが、その親のフォルダーを利用者が持っていれば、削除の権利で差し替える競争の余地が残る（悪用できるかは未確認）。環境を最小にすれば、`$PLUGINSDIR` は保護された実行のフォルダーの中にでき、`CloseMklm` が昇格したまま起動する `mklm.exe --quit` も利用者の `PATH` や互換モードの影響を受けない。
  - ブロックの組み立て（名前の大文字小文字を区別しない順に並べ、`名前=値\0` を続けて最後に `\0`）は純粋な関数 `elevation::environment_block` にし、単体テストを付ける（F.2）。
  - NSIS やプラグインが、上にない環境変数を必要とするかは未確認（L 章）。F.6 のリハーサルと煙の試験で確かめる（煙の試験の 2 は、`System.Diagnostics.ProcessStartInfo` の `EnvironmentVariables` を空にしてから同じ変数だけを入れ、`UseShellExecute = false` で実行する）。
- ダウンロードではなく H1 が書いたファイルなので、Mark of the Web（`Zone.Identifier`）がなく、SmartScreen の確認は出ない。スマート アプリ コントロールが有効な PC では、署名のないインストーラーは止められる（MKLM 自体がそこでは使えない。計画 5 章）。
- 対話のインストール（利用者が手で実行）は利用者の `%TEMP%` を使う。そのために NSIS 3.11 以上を使う（build-installer.ps1 が確かめている）。

#### D.9.5 サイレントの上書きがキーボードの設定に触れないこと

`mklm.nsi` のインストールの経路（`.onInit`、`Section "MKLM"`、`CloseMklm`、`SwapExecutables`）が実行するのは、`"$INSTDIR\mklm.exe" --quit`（多重起動のパイプに `quit` を送るだけ。M3 の GUI は `--quit` で窓を開かず、HKLM にも書かない）だけ。アンインストーラー（`uninstall.exe`）と `mklm-helper.exe --uninstall-restore` を実行するのはアンインストールのセクションだけで、上書きでは実行されない（NSIS は古い版のアンインストーラーを呼ばず、`WriteUninstaller` で書き直すだけ）。インストーラーは `HKLM\SOFTWARE\SHIN DATA CENTER` とキーボードの値に何も書かない。D.9.3 の静的な検査と煙の試験の 1 がこれを固定する。`/S` では完了ページがないので、`LaunchUnelevated`（完了ページの「MKLM を起動する」）も動かない（GUI の起動し直しは H2 が行う。D.10）。

### D.10 GUI を非昇格で起動し直す

- H2 は最後の手順（ロックを放し、ハンドルを閉じ、`LastResult` を書いた後）で `mklm_win::shell_launch::launch_via_shell("$INSTDIR\mklm.exe", "--after-update", "$INSTDIR", 10 秒)` を呼ぶ。中身は計画 4.2 の手順 8 の方法: `CoCreateInstance(CLSID_ShellWindows)` → `IShellWindows::FindWindowSW(SWC_DESKTOP, SWFO_NEEDDISPATCH)` → `IServiceProvider::QueryService(SID_STopLevelBrowser)` → `IShellBrowser::QueryActiveShellView` → `IShellView::GetItemObject(SVGIO_BACKGROUND)` → `IShellFolderViewDual::get_Application` → `IShellDispatch2::ShellExecute`。デスクトップのシェル（Explorer）が起動するので、GUI はそのセッションのサインインしている利用者として、**非昇格で**動く。
- COM の呼び出しは別のスレッドで行い、10 秒で見切る（Explorer が固まっていても H2 が止まらないように）。
- **COM のセキュリティ**（SECURITY-8）: H2 は Explorer（中程度の整合性レベル）からインターフェイスのポインターを受け取る。同じ利用者のマルウェアが Explorer に入り込んでいると、独自のマーシャリングの OBJREF を返して、昇格した H2 の中で任意のクラスを復元させうる。そこで H2 は、ほかのどの COM の呼び出しより前に `CoInitializeSecurity(…, RPC_C_IMP_LEVEL_IDENTIFY, …, EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA, …)` を呼ぶ（D.7 の手順 1）。
- **H2 自身のトークンでは決して起動しない**（`CreateProcessW` や `explorer.exe <path>` にも頼らない。後者は H2 のアカウントで Explorer を起動しうるため）。
- **別の管理者の資格情報で昇格した場合**（標準ユーザー A が UAC で管理者 B の資格情報を入れた。m2 S3）: H1 と H2 は B として動く。この PC のレジストリ（読み取りだけで確認、2026-09-29）では、`HKCR\AppID\{9BA05972-F6A8-11CF-A442-00A0C90A8F39}`（ShellWindows）の `RunAs` が `Interactive User`、`LocalServer32` が `rundll32.exe shell32.dll,SHCreateLocalServerRunDll {9BA05972-…}` だった。つまり ShellWindows は H2 のトークンではなく、**そのセッションの対話中の利用者（A）として**動き、B として GUI が起動することはない（RELIABILITY-7、SECURITY-8）。残る未確認は、B の H2 から A のサーバーへの COM の呼び出しが、アクセスの検査で通るかどうか（I.2、T-UPD-8）。通らなければ失敗し、次の 3 つで GUI は戻る。
  1. 更新を始めた GUI は、終了する前に自分の HKCU の RunOnce に `SHINDATACENTER.MKLM.AfterUpdate = "<INSTDIR>\mklm.exe" --after-update` を登録する（計画 4.2 の手順 8 の予備。昇格している GUI は登録しない。m3 F.2）。次のサインインで結果が前面に出る。
  2. Run キーの自動起動（既定でオン。次のサインインで）。
  3. 利用者がスタート メニューから開く。GUI は引き継ぎの前に「2 分たっても開かない場合は、スタート メニューから開いてください」と伝えている（E.4）。
- 起動し直した GUI、または手で開いた GUI は、表示していない結果がなければ RunOnce の値を消す（`unregister_after_update`。同じ利用者なので消せる。D.13。OPS-UX-TEST-15）。
- 起動し直したかどうかは `LastResult.gui_relaunch_attempted`（試みたか）に残す。GUI が `LastResult` を読むより先に起動しないよう、`LastResult` を書いてから起動する（D.7 の 19、21）。

### D.11 後片付け（RELIABILITY-9）

- H2 は最後に、自分の実行のフォルダーのインストーラー、2 つの更新情報、`tmp` を消す。自分の実行ファイルは動いているので消せないが、**`MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` は使わない**（レビュー前はこれで次の再起動での削除を予約していた）。`PendingFileRenameOperations` に書くと、Windows Update、Intune / SCCM や多くのインストーラーが「再起動が必要」と判断し、高速スタートアップのシャットダウンではその予約が処理されないので、長く残る。得られるのは数 MB のフォルダーの削除だけで、それは次の昇格した MKLM が行える。
- **掃除**（`update_dir::sweep_stale_run_dirs(updates, keep)`）: `Updates` の実行のフォルダーのうち、`Run` の記録が指すもの（`keep`）以外で、中の `mklm-update-runner.exe` のイメージが動いていないものを消す。消すときはリパースポイントをたどらず（`FILE_FLAG_OPEN_REPARSE_POINT`）、通常のファイルだけを消す（`remove_run_dir`）。呼ぶのは次の 3 か所。
  1. H1（D.4 の手順 8）。
  2. H2 の最後（自分以外のフォルダー）。
  3. helper の通常のパイプのセッションで、ロックを取った直後（安い処理。失敗はログだけで、要求は続ける）。同じ場所で、持ち主が死んだ `Run` を `LastResult` に移す（D.13。RELIABILITY-11）。
- 利用者のキャッシュ（`%LOCALAPPDATA%\…\update\`）のインストーラーは、GUI が次の条件をすべて満たすときだけ消す（`UpdateCache::prune`。RELIABILITY-2）: `LastResult` が `Installed` で、その `to_version` が動いている版と同じで、インストールの状態がそろっている。それ以外（失敗、そろっていない、中断）のときは残す。そろっていない状態で［インストーラーを実行］に使うため（D.13）。
- アンインストールは `Updates` を消さない（最大でも runner のコピー 1 つが残るだけで、`Updates` は管理者しか書けない。ジャーナルと同じく残す）。

### D.12 段階ごとの失敗、期限切れ、再起動、サインアウト

「記録」は `Run` と `LastResult`（D.6）。「中断」は、`Run` が残っていて、その段階の持ち主のプロセスがいないか起動 ID が変わった状態（`run::classify_run`。`installing` と `finishing` では、`runner` か `installer` のどちらかが生きていれば中断ではない）。

| 段階（`Run.phase`） | 動いているもの | 失敗・期限切れ | 再起動・電源断・サインアウト | 何が変わったか | 次の GUI の表示 |
|---|---|---|---|---|---|
| 確認とダウンロード（記録なし） | GUI | ページに理由（E.6）。次の定時の確認でやり直す | 途中の `.part` を次の起動で消す | 何も | 何もなし |
| UAC の待ち（記録なし） | GUI | UAC を断れば「取り消しました」 | — | 何も | — |
| H1 の検証（記録なし） | GUI、H1 | `Refused`。GUI はそのまま | — | 何も（機械の記録は D.4 の 9 で進むことがある） | — |
| `staging` | GUI、H1 | 呼び出し元が去った、ハッシュ違い、期限切れ → フォルダーと `Run` を消す | `Run` が残る → 中断 | 何も | 「前回の更新の準備は中断されました。何も変更されていません」（情報、1 回だけ）。次の H1 か通常のセッションが片付ける |
| `staged` | H1、H2 の起動中 | H2 が 120 秒で `ready` にならない → H1 が H2 を止め、終わりを待ってからフォルダーと `Run` を消し、`Refused(HandOffFailed)`。GUI はそのまま | 中断 | 何も | 同上 |
| `ready` / `waiting` | H2（GUI は終了中か終了済み） | 検証し直し、ロック、ジャーナル、版の変化、空き容量、ほかの MKLM、ファイルの使用、セッションの終了 → `LastResult = NotInstalled`、GUI を起動し直す | サインアウト・シャットダウン: H2 は止めずに `NotInstalled(SessionEnding)` を書いて終わる（書く前に終わらされれば中断）。電源断: 中断 | 何も | `NotInstalled` の理由と次の手順（E.6）。中断なら「更新は中断されました。何も変更されていません」 |
| `installing` | H2、インストーラー（`Run.installer`） | 終了コード → D.13 の判定。15 分を過ぎたら `LastResult = Failed(InstallerTimedOut)` を書き、最大 60 分まで待ち続ける（止めない）。60 分で `Run` を残して H2 は終わる | サインアウト・シャットダウン: H2 が止める（理由の文を出す）。利用者が強制すれば中断。電源断: 中断（半端になりうるのは名前の変更の数ミリ秒） | インストーラーが生きている間は `InProgress`。死んだら、ファイルの版で調べる（D.13） | D.13 の表 |
| `finishing` | H2 | — | 中断（ほぼ終わっている） | 置き換え済みのことが多い | D.13 の表 |
| 記録の後（`Run` なし、`LastResult` あり） | H2（後片付けと起動し直し） | 起動し直しの失敗は RunOnce と Run キーが補う（D.10） | 影響なし（片付けは次の昇格した MKLM） | — | `LastResult` を 1 回表示（D.13 の条件） |

- レビュー前は、`installing` の間もセッションの終了を止めなかった（I.7）。H2 が見えないまま最大 3 分半あまり待つことがあり、利用者が「終わった」と思ってサインアウトしうるので、止めることにした（D.7。RELIABILITY-3）。GUI の引き継ぎの案内も「MKLM が開き直すまで、サインアウトや再起動は待ってください」と伝える（E.4）。

### D.13 途中で止まったインストールの検出と、次の GUI の表示

**インストールの状態**（`run::InstallState`。読むのは `mklm_win::update_dir::read_build_ids(install_dir)` と、その結果を変換する `InstallState::from_build_ids` の 1 組で、H2（`RunnerEnv::read_install_state` の実装）、H1（`StagerEnv::read_install_state`）、GUI と CLI の `read_status` が同じものを使う。OPS-UX-TEST-12。レビュー第 1 回の後の版がここで書いていた `update_dir::read_install_state` という関数はない。`mklm-win` は `mklm-update` の型を返せないため。FIX-VERIFICATION-11）: インストール先の `mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe` の VERSIONINFO のビルド ID（`elevation::file_build_id`。非昇格でも読める）。3 つがあってすべて同じなら、その版（ビルド ID の `+` の前）が「そろった版」（`consistent_version`）。1 つでも欠けるか違えば「そろっていない」。`.new` と `.old` は見ない。

**結果の判定**（`run::decide_outcome`。H2 が使う。中断の場合は `interrupted_result` が同じ規則を使う）

| 条件 | 結果 |
|---|---|
| インストーラーが 15 分で終わらなかった（その後 60 分までに終わらず、H2 が見届けられなかった） | `Failed(InstallerTimedOut)` |
| そろった版 = 新しい版 | `Installed`（終了コードが 0 でなくても。コードは `installer_exit` に残す） |
| そろった版 = 元の版、`InstallerExit::leaves_old_files()`（20〜24、26、27） | `NotInstalled(InstallerRefused { exit })` |
| そろった版 = 元の版、そのほかのコード | `NotInstalled(InstallerExit { code })` |
| そろった版がそれ以外 | `Failed(UnexpectedVersion { found })` |
| そろっていない | `Failed(Inconsistent)` |

**GUI の起動時**（どの起動方法でも。`--tray`、`--after-update`、RunOnce、手で。OPS-UX-TEST-6、RELIABILITY-11、OPS-UX-TEST-15）

1. `update_store` を読み、`classify_run` で `Run` を分類する。
   - `InProgress` で段階が `ready` 以降（`runner` か `installer` が生きている）: 更新の途中なので、**窓を出さず、インスタンスにもならずに、すぐ終了する**（ログに残す。RunOnce の値は消さない）。窓を出すと `mklm.exe` がロックされ、インストーラーを止めてしまうため。`staging` / `staged` の間（まだ GUI が動いているはずの段階）は、ふつうに起動する。
     - この GUI のセッションが `Run.caller_session` と同じなら、数十秒後に H2 が GUI を起動し直す。
     - **違う（または `caller_session` がない）なら**（別の利用者が、更新の間にサインインした、または MKLM を開いた。FIX-VERIFICATION-9）: H2 はこのセッションに GUI を起動し直さない（D.10 はその利用者のトークンを持たない）。そこで終了する前に、昇格していなければ自分の HKCU の RunOnce に `--after-update` を登録し、`settings.update.closed_by_update` に今の時刻を書く（保存を待ってから終了する）。**このセッションの MKLM は、その利用者が MKLM を開き直すか、次にサインインするまで動かない**（トレイのアイコンも通知もない）。次の起動で、結果と「別のユーザーの更新のために、MKLM はいったん終了していました。」が出る（3 と E.5。起動を見送った場合も同じ文を使う）。
   - `Interrupted`: 下の「表示の条件」を満たせば、中断の結果を表示する。表示した `Run.run_id` を `settings.update.result_seen` に書く（利用者ごとに 1 回だけ。レビュー前は起動のたびに出ていた）。
2. `LastResult` は、次をすべて満たすときだけ 1 回表示し、`result_seen` に `run_id` を書く。満たさなければ、表示せずに `result_seen` に書く。
   - `run_id` が `result_seen` と違う。
   - `finished_at` が 14 日以内（`RESULT_SHOW_DAYS`）。
   - 今の状態と合う: `Installed`（と、新しい版が入った中断）は、動いている版が `to_version`。`NotInstalled`、`Failed`、新しい版が入っていない中断は、動いている版が `from_version` で、`Failed(Inconsistent)` なら今のインストールの状態もそろっていない。
3. 文の選び方:
   - 更新を始めたのがこの利用者（`settings.update.started_run == run_id`。引き継ぎのときに GUI が書く）なら、E.6 の文。
   - ほかの利用者の更新なら、中立の文: 「MKLM は 0.2.1 に更新されました（別のユーザーが更新しました）。」。失敗や中断は、ほかの利用者には出さない（その利用者の操作ではなく、次の手順もないため）。ただし「そろっていない」は 4 で全員に出る。
   - この利用者の GUI が `quit-if-idle` で終わっていた、または更新の途中のため起動を見送った（1。どちらも `settings.update.closed_by_update` の時刻の後に終わった更新）なら、「別のユーザーの更新のために、MKLM はいったん終了していました。」を添える（RELIABILITY-7）。
4. **今の**インストールの状態がそろっていなければ、`LastResult` によらず「MKLM のファイルの版がそろっていません」を出す（m3 の helper のビルド ID の確認より先に、原因を示すため）。
   - 利用者のキャッシュに、キャッシュの更新情報の版のインストーラーがあれば、［インストーラーを実行］を出す: 押すと、キャッシュの更新情報を検証し直し、インストーラーの大きさと SHA-256 を照らしてから、`ShellExecuteExW`（`runas`）で**対話の**インストーラーを起動する（利用者がボタンを押したときの UAC。NSIS の画面が出る。RELIABILITY-2）。
   - **この照合は、壊れたファイル（ダウンロードの途中の失敗、ディスクの誤り）を見つけるためだけのもので、安全の保証ではない**（RED-TEAM-2）。キャッシュはサインインしている利用者が書ける場所で、照合は同じ利用者の非昇格の GUI が行い、昇格したインストーラーは照合の後に同じパスを開き直す。その間に利用者（またはその利用者の権限のマルウェア）はファイルを差し替えられる。GUI そのものも同じ利用者のプロセスなので、ハンドルを開いたままにしても、その利用者に対しては何も守れない。したがってこの経路は、利用者が自分でダウンロードしたインストーラーを手で実行するのと同じ信頼しか持たない。
     - 画面と読み上げの文で「検証済み」「確かめた」と言わない（ボタンは「インストーラーを実行（管理者の許可が要ります）」だけ）。
     - 標準ユーザーの画面で別の管理者が資格情報を入れる場合（UAC の資格情報の画面）、その管理者から見ると、この経路の UAC は署名のない任意のプログラムと見分けがつかない（場所は利用者のフォルダーになる）。`docs/recovery.md` と `install-guide.ja.md` に「ほかの利用者の PC で管理者として承認するときは、リリース ページから自分でダウンロードし、`SHA256SUMS` と署名で確かめたインストーラーを使う」と書く（WP-H、WP-U）。標準ユーザーはこの経路がなくても、同じ見た目の UAC を管理者に出せるので、MKLM が新しい昇格の道を開くわけではない。
   - ［リリース ページを開く］も出す。
5. 表示していない結果がなければ、`unregister_after_update` を呼ぶ（手で開いた場合も、次のサインインで意味のない窓が出ないように）。`--after-update` で起動していて表示するものがなければ、`--tray` と同じに振る舞う。

| 中断した段階 | インストールの状態 | 表示（この利用者の更新のとき） |
|---|---|---|
| `staging`〜`waiting` | そろった版 = 元の版 | 「更新は中断されました。何も変更されていません（MKLM は 0.2.0 のままです）。」［今すぐ更新］ |
| `installing` / `finishing` | そろった版 = 新しい版 | 「MKLM は 0.2.1 に更新されました（終わる直前に PC が再起動したか、サインアウトしました）。」 |
| `installing` / `finishing` | そろった版 = 元の版 | 「更新は中断されました。MKLM は 0.2.0 のままです。」［今すぐ更新］ |
| どれでも | そろっていない | 「更新が途中で止まったため、MKLM のファイルの版がそろっていません。キーボードの設定はそのままです。［インストーラーを実行］を押すか、GitHub のリリース ページから MKLM-Setup-0.2.1-x64.exe をダウンロードして実行してください。」［インストーラーを実行］［リリース ページを開く］ |

- そろっていない状態では helper を起動できない（ビルド ID が合わない。m2 E.3）ので、自動更新でも直せない。インストーラーを実行すれば直る（NSIS の上書きはファイルを全部書き直す）。`mklm.exe` 自体が起動できない場合に備え、`docs/recovery.md` に「更新の後に MKLM が起動しないとき」を足す: `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\` にあるインストーラーか、リリース ページのインストーラーを実行する（WP-H）。
- `Run` の中断の記録を `LastResult` に移して消すのは、次の H1（D.4 の 7）か、helper の通常のセッション（D.11）。GUI は HKLM に書けないので、表示だけを `Run` から作る。

### D.14 CLI（`mklm-cli update`）

| コマンド | 動作 | 終了コード |
|---|---|---|
| `mklm-cli update --check [--json]` | GUI と同じ確認（`mklm_client::update::check`。副署名の規則を含む）。利用者の記録を更新する。ダウンロードもインストールもしない。「MKLM 0.2.1 is available (installed 0.2.0). Open MKLM to install it.」または「MKLM is up to date (0.2.0).」。期限切れなら警告を 1 行（終了コードは変えない）。巻き戻しを無視したときも警告を 1 行 | 0: 最新。20: 更新あり。21: 手で更新が必要。22: このビルドでは更新を使えない（`NotConfigured`）。**6: 更新の途中（しばらくしてから再試行）**。1: 確認できなかった。2: 使い方の誤り |
| `mklm-cli update --status [--json]` | 機械の記録（`LastResult`、進行中の `Run`、インストールの状態）と、利用者の記録（最後の確認、最後に成功した確認、最後の失敗の種類、無視した巻き戻し）を表示する | 0 / 1 / 2 |

- 終了コードを分けた（OPS-UX-TEST-20）: レビュー前は「最新」「更新あり」「手で更新が必要」がどれも 0 で、スクリプトは `--json` を解析するしかなかった。レビューの提案の 10〜12 ではなく 20〜22 にしたのは、書き込みのコマンドの 10（`AWAITING_CONFIRM`）と同じ数が別の意味になるのを避けるため。`NotInstalledCopy`（開発用のビルド）は確認だけできるので、結果に応じて 0 / 20 / 21。
- **更新中の早い終了**（RELIABILITY-2 の 4。対象を FIX-VERIFICATION-14 で絞った）: 書き込みのコマンド（`set`、`migrate`、`revert`、`undo`、`resolve`、`restore`、`recover`、`keep`、`reboot`、`post-reboot`）と `update --check` は、起動の直後に `classify_run` を見て、`ready` 以降の `InProgress` なら「MKLM is being updated. Try again in a minute.」を出して終了コード 6（`BLOCKED`。書き込みのコマンドの既存の意味「ほかのものが道をふさいでいる」と同じ）で終わる。GUI と同じく、更新の途中で `mklm-cli.exe` を長く動かし続けないため（`update --check` はネットワークの待ちで 1 分を超えうる。H2 は CLI の終了を 30 秒しか待たない）。
  - 読み取りだけのコマンド（`list`、`status`、`global status`、`journal`、`update --status`）は早い終了の対象にしない。1 秒ほどで終わり、H2 の待ち（D.8 の 3）の間に終わるので、更新を止めない。`main.rs` の約束（読み取りのコマンドは 0 / 1 / 2）はそのまま保つ。監視のスクリプトが更新の間に見慣れない 6 を受け取ることはない。
  - `update --check` の 6 は、上の表と `docs/install-guide.ja.md` の終了コードの表に載せる（WP-U）。
- **CLI はインストールしない**（J-4 の決定 (a): `update --check` と `--status` だけ）。理由: `mklm-cli.exe` 自身が `$INSTDIR` にあり、インストールの前に終わらなければならない。結果を表示できるのは次に起動した GUI か `update --status` になり、CLI の利点（その場で結果と終了コードが分かる）がない。GUI の引き継ぎと起動し直しをもう 1 組作る価値は小さい。スクリプトや管理ツールからは、インストーラーを直接 `/S` で実行すればよい（終了コードは D.9.1。`docs/install-guide.ja.md` に、この表と並べて書く）。
- 出力は英語（M1、M2 と同じ）。`--json` の形は `{"installed":"0.2.0","status":"update-available"|"up-to-date"|"manual-required","offered":"0.2.1","freshness":"fresh"|"expired","issued_at":1792022400,"expires":1807574400,"release_page":"https://…","rollback_ignored":null|{"issued_at":…,"seen":…}}`（`--check`）。

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
| 7 | （`--run-update`）インストールしなかった、または失敗した。理由は `LastResult`。`ready` の前なら記録なし。インストーラーが 60 分で終わらなかったときも 7（`Run` が残る） | 新規 |
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
| `result_seen` | なし | 表示済み（または表示しないと決めた）結果の `run_id`。`LastResult` と中断の両方（D.13） |
| `started_run` | なし | この利用者の GUI が引き継いだ更新の `run_id`（`HandedOff` のときに書く）。結果の文を選ぶため（D.13） |
| `closed_by_update` | なし | この GUI が `quit-if-idle` で終わった、または別のセッションの更新の途中のため起動を見送った（D.13 の 1）時刻（Unix 秒）。次の起動で「別のユーザーの更新のために終了していました」を添えるため（E.5） |
| `stale_notice_at` | なし | 「長く確認できていない」「期限切れ」のバナーを最後に出した時刻（30 日に 1 回にするため。E.3） |
| `rollback_notice_for` | なし | 巻き戻しの警告を出した `issued_at`（同じものに 2 度出さないため） |

**利用者のキャッシュ**（`%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\`。ローミングしない場所）

| ファイル | 中身 |
|---|---|
| `state.json` | `ClientState`: 利用者の記録（`TrustState`）、最後の確認の時刻、最後に成功した確認の時刻、最後の失敗（種類、ID、最初に起きた時刻）、最後に無視した巻き戻し、キャッシュの署名が主署名か副署名か |
| `latest.json`、`latest.json.minisig` | 最後に検証に通った更新情報と、通った方の署名（受け取ったバイト列のまま） |
| `MKLM-Setup-<v>-<arch>.exe` | 検証済みのインストーラー（1 つだけ残す。消す条件は D.11） |
| `MKLM-Setup-<v>-<arch>.exe.part` | ダウンロードの途中（起動時に消す。続きからの再開はしない） |

- 書くときは一時ファイルに書いてから名前を変える（m3 F.4 と同じ）。

**確認の時刻**

- 起動の後: 前回の確認から 24 時間以上たっていれば、60 秒 + 0〜120 秒の乱数の後。そうでなければ前回 + 24 時間 + 0〜60 分の乱数。サインイン直後の負荷を避け、利用者の間で時刻を散らすため。
- その後: 24 時間 + 0〜60 分の乱数ごと。
- スリープからの復帰（`ShellEvent::Resumed`）で予定を過ぎていれば、30 秒後。
- ［今すぐ確認］は予定によらずすぐ。
- **時計の守り**（RELIABILITY-8）: `last_check` か `last_success` が今より 1 時間以上先なら、「一度も確認していない」とみなして起動の後の遅れの後に確認し、値を書き直す。時計が一度未来に進んでいた PC で、自動の確認が二度と動かなくなるのを防ぐ。
- 乱数は `mklm_win::session::random_bytes`。時刻の管理は UI スレッドの `slint::Timer`。

**スレッド**（m3 A.4 に足す）

| スレッド | 数 | やること | UI への戻り方 |
|---|---|---|---|
| 更新のワーカー（`mklm-update`） | 1（常駐） | `UpdateTask::{Check { manual }, Download, Cancel}`: `mklm_client::update::{check, download}`、キャッシュの読み書き、機械の記録の読み取り | `invoke_from_event_loop` で `AppMsg::Update*` |
| セッション ワーカー（既存） | 同時に 1 | 更新のセッション: `launch::start`（UAC）→ `update::stage` | 既存の形。セッション ID 付き |

- 確認とダウンロードは I/O ワーカーに載せない（遅いネットワークで一覧の読み直しを待たせないため）。
- helper のセッション（キーボードの変更を含む）を始めるとき、`mklm-client` のセッションの開始が `update::pending_trust_report` を呼び、必要なら `RecordTrust` を送る（C.4）。GUI の状態には影響しない（返事はログに書くだけ）。

**流れ**

- 確認 → `Available` で、スキップしていなければ自動でダウンロード → `Ready`（ボタンが押せる）。スキップした版は、更新のページの［ダウンロード］でだけダウンロードする。
- 新しい版が出れば、古いダウンロードを捨てて取り直す（D.11 の条件で消せるものだけ）。
- 従量制の接続でもダウンロードする（数 MB のため。J-7 の決定 (a)）。

### E.2 状態と、更新のページ

新しいページ `update`（m3 B.0 のページに 1 つ足す。左のナビゲーションには足さず、メイン画面のバナー、トレイのメニュー、設定のページの「更新」欄から開く）。

| 状態 | 表示 | ボタン |
|---|---|---|
| 使えない（`NotConfigured`、`NotInstalledCopy`、`Unknown`） | 理由（E.6） | ［今すぐ確認］（`NotInstalledCopy` のときだけ。確認だけ） |
| 確認していない / 確認中 | 「確認しています…」 | ［キャンセル］ |
| 最新 | 「MKLM は最新です（0.2.0。2026/10/15 の更新情報）。最後の確認: 2026/10/16 09:12」。更新情報の日付は `issued_at` を現地の日付にしたもの（凍結に人が気付く手がかり。B.4 の 4。RED-TEAM-1） | ［今すぐ確認］ |
| 更新あり・スキップした版（未ダウンロード） | 「0.2.1 はスキップしました。」、新しい版、公開日、［リリースノートを開く］ | ［ダウンロード］［今すぐ確認］（OPS-UX-TEST-16） |
| 更新あり・ダウンロード中 | 新しい版、公開日、［リリースノートを開く］、進捗（MB と %） | ［キャンセル］ |
| 更新あり・準備完了 | 下の図 | ［この版をスキップ］［後で］［今すぐ更新（次に Windows の確認が出ます）］ |
| 手で更新が必要（`ManualRequired`） | 「この版からは自動で更新できません…」 | ［リリース ページを開く］ |
| 更新中（セッション） | 「管理者の確認を待っています…」→「更新を準備しています…（送信 45 %）」→「Windows がファイルを確認しています…（最大 2 分ほど）」（`StartingRunner` の後。RELIABILITY-5） | ［キャンセル］（全部送り終えるまで） |
| 失敗 | 理由と次の手順（E.6） | 状況に応じて［もう一度確認］［もう一度ダウンロード］［リリース ページを開く］［詳細をコピー］ |
| ファイルの版がそろっていない（D.13） | D.13 の文 | ［インストーラーを実行］（キャッシュにあるとき）［リリース ページを開く］ |

どの状態でも、当てはまれば本文の下に情報の行を足す: 期限切れ（B.4 の 5）、30 日以上確認できていない（E.3）、最後の確認で古い更新情報を無視した（E.3）。

```
┌ MKLM の更新 ────────────────────────────────────────────────────────┐
│ 新しい版があります: 0.2.1（今の版: 0.2.0）                              │
│ 公開: 2026/10/15    ［リリースノートを開く］                            │
│ ダウンロード: 完了（6.0 MB。内容を確かめました）                        │
│                                                                         │
│ ［今すぐ更新］を押すと、Windows が管理者の許可を求めます。発行元は      │
│ 「不明」と表示されます。許可する前に「詳細を表示」を押し、プログラムの  │
│ 場所が C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe であるこ │
│ とを確かめてください。違う場所なら「いいえ」を押してください。MKLM が   │
│ 許可を求めるのは、あなたが［今すぐ更新］を押した直後だけです。          │
│ 許可すると MKLM はいったん終了し、更新が終わると自動で開きます。        │
│ キーボードの設定は変わりません。                                        │
│      ［この版をスキップ］［後で］［今すぐ更新（次に Windows の確認が出ます）］│
└─────────────────────────────────────────────────────────────────────────┘
```

- 場所の文のパスは、固定の文字列ではなく `fixed_install_dir()` の実際のパス（`%ProgramFiles%` が C: 以外でも正しく出す）。未署名のため、UAC の画面は「発行元: 不明」になり、マルウェアも同じ見た目の画面を出せる。場所を確かめる手順を教え、「MKLM が許可を求めるのはボタンの直後だけ」と伝えることで、便乗の UAC を見分けられるようにする（SECURITY-14）。`docs/install-guide.ja.md` にも同じことを書く。
- ［後で］: バナーを次の確認（24 時間後）か次の起動まで隠す。
- ［この版をスキップ］: `skipped_version` に書き、バナーを消す。
- 設定のページに「更新」欄: 「☑ 更新を自動で確認する（1 日に 1 回、GitHub に問い合わせます）」、最後の確認の時刻と結果、最後に成功した確認の時刻（30 日以上前なら「30 日以上、更新を確認できていません。［詳細］」と最後の失敗の種類。OPS-UX-TEST-5）、［今すぐ確認］、［更新のページを開く］。
- 初回のウィザードの「ようこそ」に 1 行: 「MKLM は 1 日に 1 回、GitHub で新しい版を確かめます（設定でオフにできます）。」

### E.3 バナーとトレイ

- **更新あり**（情報）: 「MKLM の新しい版（0.2.1）を使えます。［詳細…］」。準備完了（ダウンロード済み）のときだけ出す。スキップした版には出さない。
- **長く確認できていない、または期限切れ**（情報。SECURITY-11、OPS-UX-TEST-5）: `auto_check` がオンで、更新が使える（`Availability::Available`）か確認だけできる（`NotInstalledCopy`）とき、最後に成功した確認が 30 日以上前（成功がなければ、記録した最初の失敗から 30 日以上）か、キャッシュの更新情報が期限切れ（既定では発行から 180 日。J-3 の決定）のとき。`stale_notice_at` から 30 日以上たっていれば 1 回出し、`stale_notice_at` を書く。
  - 最後の失敗の種類は `classify`（H.4）が決め、`ClientState.last_failure` に残す。取り消し（`Cancelled`）と `NotConfigured` は失敗として記録しない（30 日の数え始めにならない。FIX-VERIFICATION-12）。
  - 最後の失敗が一時的なもの: 「30 日以上、MKLM の更新を確認できていません。インターネットやプロキシの設定を確かめてください。［詳細…］」
  - 最後の失敗が構造的なもの（E.6 の「構造」。`Rollback` を含む）か、期限切れ: 「MKLM の自動更新が使えなくなっている可能性があります。GitHub のリリース ページで新しい版を確かめてください。［リリース ページを開く］［詳細…］」
  - この知らせは、確認が**失敗し続ける**ときのもので、正しく署名された古い更新情報を見せ続けられる凍結（確認は成功する）には効かない。凍結を知らせるのは期限切れの方だけ（B.4 の 4。RED-TEAM-1）。
- **古い更新情報を無視した**（警告。SECURITY-11、RELIABILITY-6）: 自動の確認でも、`Rollback` が起きたら、その `issued_at` について 1 回だけ出す（`rollback_notice_for`）: 「以前に確かめたものより古い更新情報が届いたため、使いませんでした。［詳細…］」。改ざんか、記録の破損か、新しい版の取り下げのしるしなので、黙って捨てない。更新のページと `update --status` にも「最後の確認: 古い更新情報を無視しました（受け取ったもの 2026/10/20、記録 2026/11/02）」と出す。ログには両方の値を書く。
- トレイ: ツールチップに「MKLM — 新しい版（0.2.1）があります」。メニューに「MKLM を更新…」（準備完了のときだけ有効。更新のページを開く）。長く確認できていないことと巻き戻しは、トレイには出さない（バナーと設定で足りる）。
- Windows の通知（トースト）は使わない（m3 B.16 の方針）。
- どのバナーも、インストールやキーボードの機能を止めない。

### E.4 UAC の事前説明と引き継ぎ

- ［今すぐ更新］で `settings.change.uac_notice_seen` が false なら、m3 B.5 の UAC の説明の画面（`Page::UacNotice`）を先に出す。そのとき、どこから来たか（`UacNoticeOrigin::Update`）を覚えておき、［続ける］で更新のセッションを始め、［キャンセル］で更新のページに戻る（今の `UacGo` と `CancelChange` は変更のページに戻るので、分ける。H.4。OPS-UX-TEST-12）。true なら、ページの説明の文（E.2 の図）とボタンの文で足りる。昇格した GUI では UAC が出ないので、どちらも出さず、ボタンは「今すぐ更新」。説明の画面にも、E.2 と同じ「詳細を表示」で場所を確かめる文を足す（SECURITY-14）。
- 利用者がボタンを押したときだけ UAC を出す。自動の確認とダウンロードは UAC を出さない。
- セッションの段階は m3 B.18 と同じ考え方で、更新用の値を持つ（`SessionPhase::Launching` → `SessionPhase::Updating { id, stage }`。H.4）。UAC を断れば「取り消しました（何も変更していません）」。
- 最後のチャンクを送った後に helper を失った（`Lost`、`Unresponsive`）場合: HKLM の `Run` を 1 秒ごとに最大 30 秒読み、`Run.caller` がこのプロセス（PID と作成時刻）で、`phase` が `ready` か `waiting`、`runner` が生きていれば、`HandedOff` を受けたものとして扱う（H1 が `HandedOff` を送る前に落ちても、GUI が残って H2 の `CallerDidNotExit` にならないように。RELIABILITY-5）。
- `HandedOff` を受けたら（またはそう扱ったら）:
  1. `settings.update.started_run` に `run_id` を書く（保存は下の 3 の終了の経路で待つ）。
  2. RunOnce（`--after-update`）を登録する（昇格した GUI は登録しない。D.10）。
  3. 引き継ぎのオーバーレイを出す（OPS-UX-TEST-15。長さを FIX-VERIFICATION-13 で決め直した）。［OK］はいつでも押せ、押せばすぐ閉じて 4 へ進む。押されなければ `HANDOFF_OVERLAY_MAX`（15 秒）で自分で閉じて 4 へ進む。H2 は呼び出し元の終了を `CALLER_EXIT_WAIT`（30 秒。D.8 の 1）までしか待たないので、利用者が席を外していても、GUI は必ずその前に終わる（15 秒 + 終了の経路の最大 2 秒 < 30 秒。F.4 のテストで定数の関係を固定する）。15 秒は、下の文を assertive で読み上げ終える長さの見込み（約 110 文字）。同じ内容（MKLM がいったん終了し、自動で開き直す）は UAC の前の更新のページ（E.2）にもあるので、読み終わる前に閉じても、知るべきことを失わない（WCAG 2.2.1 の「本質的な情報は時間の制限の前にも示す」）:
     「MKLM を更新しています。MKLM はまもなく終了し、更新が終わると 1 分ほどで自動で開きます。開き直すまで、サインアウトや再起動はしないでください。2 分たっても開かない場合は、スタート メニューから MKLM を開いてください。」
  4. 通常の終了の経路で終了する（トレイを消す、I/O の保存を最大 1 秒待つ。m3 F.5）。

#### E.4.1 更新のセッションの間の、終了、閉じる、セッションの終わり（OPS-UX-TEST-12）

m3 F.5 の表に、更新のセッションの列を足したもの。`SessionPhase::Updating` の `stage`（`UpdateStage`）で分ける。「UAC 待ち（更新）」は、`SessionPhase::Launching` で `AppState::session_purpose == Some(SessionPurpose::Update)` の状態（H.4。FIX-VERIFICATION-11）。`Notice::Connected` を受けたとき、`session_purpose` が `Update` なら `Updating { stage: Sending }` へ、`Change` なら今までどおり `Running` へ移る。

| 場面 | UAC 待ち（`Launching`、更新） | 送信中（`Updating { Sending }`） | H2 の準備待ち（`Updating { StartingRunner }`、最後のチャンクの後） | 引き継ぎ済み（`Updating { HandedOff }`、オーバーレイ） |
|---|---|---|---|---|
| ウィンドウの × | 閉じない（m3 と同じ） | 閉じない（ページに［キャンセル］がある） | 閉じない（「Windows がファイルを確認しています…」） | オーバーレイを閉じて終了（引き継ぎの終了の経路） |
| トレイの「終了」、`quit` | すぐに終了（取り消しも立てる。m3 と同じ）。`quit` には `ok` | 送信をやめて `Bye`（helper はフォルダーと `Run` を消す。何も変わらない）→ 終了。`quit` には `ok` | すぐには終わらない: 「後で終了」の印を立て、`HandedOff`（か `Run` による判断）を受けたら引き継ぎの終了、`Refused` なら終了。`quit` には `busy` | 引き継ぎの終了の経路ですぐに終了。`quit` には `ok` |
| `quit-if-idle` | `busy`（何も変えない） | `busy`（何も変えない） | `busy`（何も変えない） | `busy`（何も変えない。この GUI はすでに自分で終了する途中で、H2 はその終了を D.8 の 1 で別に待つ。`ok` にすると下の決まりの「何もしていない」の経路に入り、`closed_by_update` と RunOnce を書いてしまう。FIX-VERIFICATION-8） |
| `activate` | 窓を出す | 窓を出す | 窓を出す | 窓を出す |
| サインアウト、シャットダウン（`WM_QUERYENDSESSION`） | 取り消しを立てる（m3 と同じ） | 取り消しを立てる。helper は切断を見て片付ける | 何もしない（H2 が `ready` なら H2 が自分で扱う。D.7） | RunOnce は登録済み。何もしない |

**`quit-if-idle` の決まり**（すべての場面。m3 F.1 を改めた。RELIABILITY-1、OPS-UX-TEST-4）

- UI スレッドで判断し（`state::quit_if_idle`）、判断と動作を同じところで行う。
- 次をすべて満たすときだけ「何もしていない」: `SessionPhase::Idle`、`quit_pending` が立っていない、オーバーレイが問いかけ（`QuitConfirm`、`Reconnect`、`RecoveryConfirm`、`Countdown`、`Progress`）でない。
  - 満たせば: 昇格していなければ HKCU の RunOnce（`--after-update`）を登録し、`settings.update.closed_by_update` に今の時刻を書き、`Effect::Quit`。返事は `ok`。
  - 満たさなければ: **何も変えずに** `busy`（取り消し、「後で終了」、確認の画面のどれも立てない）。
- パイプのスレッドがコマンドを受け取ってから 4 秒（`UI_REPLY_WAIT`）を過ぎて UI スレッドに届いた `quit-if-idle` は、何もしない（返事が届かず、H2 はもう「終了しなかった」と判断して先に進んでいるため）。

### E.5 更新の後の表示

- `--after-update`（H2 か RunOnce から）: 窓を表示して前面に出し、表示する結果があれば、結果のオーバーレイで出す（m3 B.17 の形）。`--after-update` がなくても、表示する結果があれば 1 回出す。何を出すかは D.13 の条件（14 日以内、今の状態と合う、この利用者の更新か）。表示するものがなければ `--tray` と同じ。
- 結果の文は E.6。`Installed` では「MKLM を 0.2.1 に更新しました。」と［リリースノートを開く］。
- ほかの利用者の GUI（その利用者が MKLM を開き直すか、次にサインインしたときに起動）は、中立の文「MKLM は 0.2.1 に更新されました（別のユーザーが更新しました）。」を 1 回出す。その GUI が `quit-if-idle` で終わっていれば「別のユーザーの更新のために、MKLM はいったん終了していました。」を添える（RELIABILITY-7）。
- 更新の後、D.11 の条件を満たせば、利用者のキャッシュのインストーラーを消す。

### E.6 文言（エラーと次の手順。OPS-UX-TEST-16）

Rust が組み立てる文は `i18n.rs` に、型ごとの網羅的な `match` で日英を持つ（m3 D.4）。下の表は、`UpdateRefusal`、`FetchError`、`TransportError`、`CheckError`、`DownloadError`、`StageEnd`、`UpdateOutcome`、`NotInstalledReason`、`FailedReason`、`InstallerExit`、`Availability`、`Freshness`、`RunPhase`、`ProgramKind` の**すべての列挙子**を、文の ID と次の操作に写す（レビュー前は主なものだけで、`RedirectNotAllowed` などが抜けていた）。英語の診断は「技術的な詳細」と［詳細をコピー］に入れる。

「種類」は、30 日の知らせ（E.3）の文と、案内の向き先を決める。**一時**: 時間をおけば直りうる（もう一度試す）。**構造**: この版の MKLM では直らない見込み、または改ざんのしるし（リリース ページへ案内する）。**—**: どちらでもない（状態の説明）。種類は `ErrorClass::{Transient, Structural}` の 2 つだけで、表の「—」は失敗の記録の対象外を表す（FIX-VERIFICATION-12）。

**確認の失敗の分類**（`mklm_client::update::classify::classify(&CheckError, not_found_days) -> Option<(ErrorClass, &'static str)>`。F.4 のテストがすべての列挙子を確かめる）

- `None`（`last_failure` に書かない。`last_check` も進めない。30 日の数え始めにならない）: `CheckError::Fetch(FetchError::Cancelled)`、`CheckError::Fetch(FetchError::Transport(TransportError::Cancelled))`、`CheckError::Unavailable(Availability::NotConfigured)`、`CheckError::Unavailable(Availability::NotInstalledCopy { .. })`（確認だけは行うので、ふつうは起きない）。
- `Some((class, id))`: 下の表の種類と文の ID。`Rollback` は `Structural`（`upd-rollback`）とし、E.3 の警告のバナーも別に出す。
- `check` が返さないもの（`Purpose::Check` では起きない `NotNewer`、`ManualUpdateRequired`、helper だけの拒否（`Busy` 以下））は、万一来れば `Some((Transient, "upd-prepare-failed"))` にし、ログに不具合として書く。

| 列挙子 | 種類 | 文の ID | 文（日本語 UI） | 次の操作 |
|---|---|---|---|---|
| `TransportError::{Timeout, NameNotResolved, CannotConnect, Other}`、`FetchError::DeadlineExceeded` | 一時 | `upd-net` | 「更新を確認できませんでした。インターネットにつながっているか確かめてください。会社や学校のネットワークでは、プロキシの設定が必要なことがあります。」（ダウンロードのときは `upd-net-dl`:「確認」を「ダウンロード」に替えた文） | ［もう一度確認］／［もう一度ダウンロード］ |
| `TransportError::ProxyAuthRequired` | 構造 | `upd-proxy-auth` | 「このネットワークのプロキシは認証を求めるため、MKLM は自動で更新を確認できません（安全のため、Windows の資格情報を自動では送りません）。GitHub のリリース ページで新しい版を確かめてください。」 | ［リリース ページを開く］ |
| `TransportError::Tls` | 一時 | `upd-tls` | 「GitHub と安全に接続できませんでした。PC の日付と時刻が正しいか確かめてください。」 | ［もう一度確認］ |
| `TransportError::Cancelled`、`FetchError::Cancelled`、`StageEnd::Cancelled`、UAC を断った | — | `upd-cancelled` | 「取り消しました（何も変更していません）。」 | — |
| `FetchError::NotFound`（7 日未満） | 一時 | `upd-not-found` | 「更新情報が見つかりませんでした。しばらくしてからもう一度確かめてください。」 | ［もう一度確認］ |
| `FetchError::NotFound`（最初の失敗から 7 日以上） | 構造 | `upd-not-found-long` | 「更新情報が 7 日以上見つかりません。自動更新が使えなくなっている可能性があります。GitHub のリリース ページで新しい版を確かめてください。」 | ［リリース ページを開く］ |
| `FetchError::{RateLimited, HttpStatus}` | 一時 | `upd-gh-temp` | 「GitHub が一時的に応答しませんでした。しばらくしてからもう一度確かめてください。」 | ［もう一度確認］ |
| `FetchError::{TooManyRedirects, RedirectNotAllowed, MissingLocation, UnexpectedEncoding}` | 構造 | `upd-gh-changed` | 「GitHub の配布の仕組みが変わったため、この版の MKLM は自動で更新できない可能性があります。GitHub のリリース ページから新しい版を入れてください。」 | ［リリース ページを開く］ |
| `FetchError::TooLarge`（更新情報か署名） | 構造 | `upd-format` | 下の「形式」と同じ | ［リリース ページを開く］ |
| `FetchError::{SizeMismatch, HashMismatch, TooLarge（インストーラー）}`、`StageEnd::SourceChanged` | 一時 | `upd-download-mismatch` | 「ダウンロードしたファイルが更新情報と一致しませんでした。もう一度ダウンロードしてください。」 | ［もう一度ダウンロード］ |
| `FetchError::Sink`、`CheckError::Cache`、`DownloadError::Cache` | 一時 | `upd-cache` | 「ダウンロードしたファイルを保存できませんでした。ディスクの空きを確かめてください。」 | ［もう一度ダウンロード］ |
| `CheckError::Fetch`、`DownloadError::Fetch` | — | （中の `FetchError` の行） | — | — |
| `CheckError::Refused`、`StageEnd::Refused`、`NotInstalledReason::Refused` | — | （中の `UpdateRefusal` の行） | — | — |
| `CheckError::Unavailable` | — | （中の `Availability` の行） | — | — |
| `UpdateRefusal::{SignatureTooLarge, SignatureMalformed, WrongTrustedComment, UnknownKey, BadSignature, RevokedKey, SignerNotListed, IllegalRevocation}` | 構造 | `upd-sig` | 「更新情報の署名を確かめられませんでした。安全のため、この更新は使いません。GitHub のリリース ページで最新の案内を確かめてください。」 | ［リリース ページを開く］ |
| `UpdateRefusal::{ManifestTooLarge, ManifestMalformed, UnsupportedSchema, WrongProduct, WrongChannel, BadVersion, BadTimestamps, AssetMalformed, NoAssetForArch}` | 構造 | `upd-format` | 「更新情報の形式が、この版の MKLM と合いません。GitHub のリリース ページから新しい版を入れてください。」 | ［リリース ページを開く］ |
| `UpdateRefusal::TagMismatch` | 一時 | `upd-race` | 「更新情報を取得している間に、新しい版が公開されたようです。しばらくしてからもう一度確かめてください。」 | ［もう一度確認］ |
| `UpdateRefusal::Rollback` | 構造（加えて警告のバナー。E.3） | `upd-rollback` | 「以前に確かめたものより古い更新情報が届いたため、使いませんでした。新しい版が取り下げられたか、途中で古い情報に差し替えられた可能性があります。GitHub のリリース ページで最新の版を確かめてください。」（自動の確認でも出す。E.3） | ［リリース ページを開く］ |
| `UpdateRefusal::NotNewer` | — | `upd-not-newer` | 「この更新は、今の MKLM より新しくありません（すでに入っているようです）。」 | ［今すぐ確認］ |
| `UpdateRefusal::ManualUpdateRequired`、`OfferKind::ManualRequired` | — | `upd-manual` | 「この版からは自動で更新できません。リリース ページからインストーラーをダウンロードして実行してください。」 | ［リリース ページを開く］ |
| `UpdateRefusal::NotConfigured`、`Availability::NotConfigured` | — | `upd-not-configured` | 「この MKLM には更新を確かめるための鍵が入っていないため、自動更新は使えません。」 | — |
| `UpdateRefusal::NotInstalledCopy`、`Availability::NotInstalledCopy` | — | `upd-not-installed-copy` | 「この MKLM は <パス> から動いているため、自動更新は使えません（インストールした MKLM だけが更新できます）。」 | ［今すぐ確認］（確認だけ） |
| `Availability::Unknown` | 一時 | `upd-env-unknown` | 「更新を使えるか確かめられませんでした。MKLM を開き直してください。」 | ［詳細をコピー］ |
| `Availability::Available` | — | — | （文なし） | — |
| `UpdateRefusal::{OperationOpen { waiting_for_reboot: false }, RecoveryNeeded}` | — | `upd-op-open` | 「確認待ちの変更があるため、今は更新できません。［確認…］で変更を決めてから更新してください。」 | ［確認…］ |
| `UpdateRefusal::OperationOpen { waiting_for_reboot: true }` | — | `upd-op-reboot` | 「PC の再起動を待っている変更があるため、今は更新できません。PC を再起動し（シャットダウンではなく再起動）、確認を終えてから更新してください。」 | ［再起動…］ |
| `UpdateRefusal::{Busy, UpdateInProgress}` | 一時 | `upd-busy` | 「別の MKLM が処理中です。しばらくしてからもう一度試してください。」 | ［今すぐ更新］ |
| `UpdateRefusal::DiskFull`、`NotInstalledReason::DiskFull` | — | `upd-disk-full` | 「ディスクの空きが足りないため、更新できませんでした（あと約 <n> MB 必要です）。MKLM は 0.2.0 のままで、キーボードの設定も変わっていません。」 | ［今すぐ更新］ |
| `UpdateRefusal::{InstallerSizeMismatch, InstallerHashMismatch, ChunkMalformed, ChunkOutOfOrder}` | 一時 | `upd-stage-mismatch` | 「更新の準備の途中で、受け渡したファイルが一致しませんでした。何も変更していません。もう一度ダウンロードしてから試してください。」 | ［もう一度ダウンロード］ |
| `UpdateRefusal::{HandOffFailed, Storage, Internal, JournalUnreadable, CallerLeft}`、`StageEnd::{Lost, Unresponsive, Protocol}` | 一時 | `upd-prepare-failed` | 「更新の準備に失敗しました。何も変更していません。PC を再起動してからもう一度試してください。直らない場合は［詳細をコピー］を押して、その内容を添えて報告してください。」 | ［詳細をコピー］ |
| `StageEnd::HandedOff` | — | `upd-handoff` | E.4 のオーバーレイの文 | ［OK］ |
| `UpdateOutcome::Installed`（この利用者の更新） | — | `upd-installed` | 「MKLM を 0.2.1 に更新しました。」 | ［リリースノートを開く］ |
| `UpdateOutcome::Installed`（ほかの利用者の更新） | — | `upd-installed-other` | 「MKLM は 0.2.1 に更新されました（別のユーザーが更新しました）。」 | ［リリースノートを開く］ |
| `NotInstalledReason::InstanceBusy` | 一時 | `upd-instance-busy` | 「ほかのユーザーの MKLM が、キーボードの変更の途中か確認を待っていたため、更新しませんでした（MKLM は 0.2.0 のままです）。その変更が終わってから、もう一度［今すぐ更新］を押してください。」（技術的な詳細に、`busy` を返した MKLM のセッション ID） | ［今すぐ更新］［詳細をコピー］ |
| `NotInstalledReason::{ProgramsStillRunning, CallerDidNotExit}`、`InstallerExit::{HelperRunning, CliRunning, GuiRunning}`（`InstallerRefused` の中） | 一時 | `upd-programs-running` | 「ほかの MKLM（<プログラム>）が動いていたため、更新しませんでした（MKLM は 0.2.0 のままです）。それを終了してから、もう一度［今すぐ更新］を押してください。」（技術的な詳細に `holders`。ほかのユーザーのセッションの `mklm-cli` のこともある） | ［今すぐ更新］［詳細をコピー］ |
| `NotInstalledReason::FilesInUse`、`InstallerExit::FilesInUse` | 一時 | `upd-files-in-use` | 「別のプログラムが MKLM のファイルを開いていたため、更新しませんでした（MKLM は 0.2.0 のままです）。ほかのユーザーのプログラムや、ウイルス対策ソフトのことがあります。しばらくしてから、もう一度［今すぐ更新］を押してください。何度試しても同じなら、［詳細をコピー］の内容（ファイルを開いているプログラムとセッションの番号）を管理者に伝えてください。」（技術的な詳細に `holders` の PID、セッション ID、実行ファイルの名前。RED-TEAM-3） | ［今すぐ更新］［詳細をコピー］ |
| `NotInstalledReason::SessionEnding` | — | `upd-session-ending` | 「サインアウトかシャットダウンが始まったため、更新しませんでした（MKLM は 0.2.0 のままです）。」 | ［今すぐ更新］ |
| `NotInstalledReason::InstallerNotStarted { code: 225 / 226 }` | — | `upd-av-blocked` | 「インストーラーがウイルス対策ソフトに止められました。Windows セキュリティ → ウイルスと脅威の防止 → 保護の履歴 で確かめてください。MKLM は 0.2.0 のままです。」 | ［詳細をコピー］ |
| `NotInstalledReason::InstallerNotStarted`（ほかのコード） | 一時 | `upd-installer-not-started` | 「インストーラーを起動できませんでした。MKLM は 0.2.0 のままで、キーボードの設定も変わっていません。」 | ［今すぐ更新］［詳細をコピー］ |
| `NotInstalledReason::InstalledVersionChanged` | — | `upd-version-changed` | 「更新の準備の間に、別の方法で MKLM がインストールされました。今の版で問題なければ、何もする必要はありません。」 | — |
| `InstallerExit::{OsTooOld, WrongArch}`（`InstallerRefused` の中） | 構造 | `upd-installer-env` | 「インストーラーが、この PC では動かないと判断しました（<理由>）。MKLM は 0.2.0 のままです。」 | ［リリース ページを開く］ |
| `InstallerExit::FileWrite`（`InstallerRefused` の中） | — | `upd-file-write` | 「インストーラーがファイルを書けませんでした。ディスクの空きとウイルス対策ソフトを確かめてください。MKLM は 0.2.0 のままです。」 | ［今すぐ更新］ |
| `NotInstalledReason::InstallerExit { code }`、`InstallerExit::{Success, UserCancelled, ScriptAborted, Other}` | — | `upd-not-installed` | 「更新できませんでした。MKLM は 0.2.0 のままで、キーボードの設定も変わっていません。」＋理由（技術的な詳細に「インストーラーの終了コード n」） | ［今すぐ更新］［詳細をコピー］ |
| `FailedReason::{Inconsistent, UnexpectedVersion}` | — | `upd-inconsistent` | D.13 の「そろっていない」の文（今の状態がそろっていないときだけ。そろっていれば今の版の文） | ［インストーラーを実行］［リリース ページを開く］［詳細をコピー］ |
| `FailedReason::InstallerTimedOut` | — | `upd-timeout` | 「インストーラーが 60 分たっても終わりませんでした。」の後に、今のインストールの状態に合う文（D.13） | 同上 |
| `UpdateOutcome::Interrupted { phase }` | — | `upd-interrupted-*` | D.13 の表 | D.13 の表 |
| `Freshness::Expired` | 構造 | `upd-expired` | B.4 の 5 の文 | ［リリース ページを開く］ |
| `Freshness::Fresh` | — | — | （文なし） | — |
| 30 日以上確認できていない | 最後の失敗に従う | `upd-stale`、`upd-stale-structural` | E.3 の文 | E.3 |
| `RunPhase::{Staging, Staged, Ready, Waiting, Installing, Finishing, Done}` | — | `upd-phase-*` | 技術的な詳細の段階の名前だけ（受け取り中、受け取り済み、準備完了、ほかの MKLM の終了待ち、インストール中、確認中、完了）。利用者の文には段階の名前を出さない | — |
| `ProgramKind::{Gui, Cli, Helper}` | — | `upd-program-*` | 一覧の名前（「MKLM」「mklm-cli」「mklm-helper」） | — |

- `NotInstalled` と `Failed` の文には、キーボードの設定が変わっていないことを必ず添える（更新はキーボードに触れない。0.2 の 7）。
- 表記の決まりは m3 D.5 に従う（ボタンは［］、Windows の画面の語は「」、2 文以上は「。」で終える、「確定」を使わない）。
- 「構造」の失敗は、30 日の知らせを待たず、その場の文でもリリース ページへ案内する。

### E.7 多言語とアクセシビリティ

- `.slint` の固定の文言は `@tr` と `translations/ja/LC_MESSAGES/mklm.po`。`tests/translations.rs` が漏れを検出する（m3 D.2）。Rust の文は `i18n.rs`（E.6）。利用者向けの文言を `i18n.rs` と `.po` の外に置かない（m3 I 章のレビュー規則）。
- 日本語の出力のラテン文字の検査（`vm::unexpected_latin`、m3 H.1）: `LATIN_ALLOWED` に `GitHub`、`x64`、`ARM64`、`MB` を足す。`FILE_NAMES_ALLOWED` に `mklm-cli` と `mklm-helper` を足す（`upd-programs-running` の文と `ProgramKind` の名前のため。今は `mklm-helper.exe` だけで、`.exe` のない形は単語に分けられて検出される）。インストーラーの名前（`MKLM-Setup-0.2.1-x64.exe`）とインストール先のパスは、テストで `names` の引数に渡す（OPS-UX-TEST-16）。
- 読み上げ: ページの見出しにフォーカス（m3 E.1）。進捗は `accessible-label`「ダウンロード: 45 %」、25 % ごとに polite で読み上げる。結果、失敗、引き継ぎは assertive（引き継ぎのオーバーレイは［OK］か 15 秒で閉じる。同じ内容は UAC の前の更新のページにもある。E.4。WCAG 2.2.1）。バナーの［詳細…］の読み上げ名は「MKLM の更新の詳細を開く」。ボタンはすべて読み上げ名を持つ（「0.2.1 をスキップ」「今すぐ 0.2.1 に更新」「0.2.1 をダウンロード」「インストーラーを実行（管理者の許可が要ります）」）。
- 状態を色だけで示さない。キーボードだけで操作できる。ボタンの行は本文のスクロールの外（m3 B.0、E.4）。
- リンク（リリースノート、リリース ページ）は `mklm_win::ui::open_release_page(version)` で開く。固定の接頭辞と厳密な版から URL を作り、`ShellExecuteW` に渡す。［インストーラーを実行］は `mklm_win::ui::run_installer_interactive(path)`（`ShellExecuteExW` の `runas`。利用者のキャッシュのインストーラーだけ。大きさと SHA-256 の照合は壊れたファイルを避けるためで、安全の保証ではない。D.13。RED-TEAM-2）。どちらも m3 I 章の「`ShellExecuteW` に渡してよいもの」に足す。

### E.8 開発用のビルド

- `$INSTDIR` の外（`target\…`）で動く GUI は `NotInstalledCopy`: 確認だけできて、インストールのボタンは出ない。
- 開発用の cfg のデバッグ ビルドの `--update-endpoint=http://127.0.0.1:<port>` は、そのプロセスの間だけ有効（A.10、F.6）。MKLM がすでに動いていると、2 つ目の起動は `activate` を送って終わるので、この引数は届かない（F.6 の手順で先に終了させる）。

---

## F. テスト

### F.1 単体テスト（`mklm-update`、ネットワークなし、昇格なし）

- 鍵はテストの中で `minisign`（dev-dependency）で作る使い捨ての鍵。`TrustAnchors::from_keys` で渡す。テスト用の秘密鍵をリポジトリに置かない。`TrustAnchors::for_this_build()` はテストで使わない（開発者の環境変数で結果が変わらないように。OPS-UX-TEST-7）。

| 対象 | テスト |
|---|---|
| 正常 | 通常用の鍵で署名した更新情報が通る。バックアップ用の鍵でも通る。`VerifiedManifest` の各値（指紋、`key_ids` を含む） |
| 改ざんした更新情報 | 1 バイトでも変えれば `BadSignature`（各位置で。空白、改行、末尾の追加も） |
| 改ざんした署名 | 署名の行、trusted comment、全体の署名の行のどれを変えても `BadSignature` か `SignatureMalformed`。trusted comment の先頭が違えば `WrongTrustedComment`。legacy の署名（`Ed`）は拒否 |
| 用途の分離 | 本番の鍵で `mklm-dev-latest-json v1` → `WrongTrustedComment`。本番の鍵で `mklm-key-drill v1` → `WrongTrustedComment`。開発用の cfg のテスト（F.3 と同じ条件）だけ: 開発用の鍵で本番の接頭辞 → `WrongTrustedComment`、開発用の接頭辞 → 通るが、記録（`recorded`）は何も変えない |
| 違う鍵 | 信頼の起点にない鍵 → `UnknownKey`。ID だけ同じで違う鍵（base64 の手直し）→ `BadSignature` |
| `key_ids` | 空、3 つ、重複、形の誤り → `ManifestMalformed`。署名の鍵を含まない → `SignerNotListed`。知らない ID を含む（移行の間）→ 通る |
| 失効（SECURITY-3） | ビルドの `revoked` → `RevokedKey`。記録の失効（ID と指紋が一致）→ `RevokedKey`。**記録の失効が同じ ID で別の指紋 → 効かない**（後のビルドが同じ ID の別の鍵を信頼する場合）。自分を失効 → `IllegalRevocation`。**通常用がバックアップ用を失効 → 通り、何も記録しない**（`VerifiedManifest::revoked` にも入らず、`recorded` の後もバックアップ用の鍵の更新情報が通る。B.2 の規則 2。FIX-VERIFICATION-1）。バックアップ用が通常用を失効 → 通り、`recorded` の後は通常用の更新情報が `RevokedKey`。**通常用が埋め込みにない ID を失効 → 無視され、記録に入らない**。ビルドで失効済みの ID → 何も起きない |
| 巻き戻し（B.2） | `issued_at` がしきい値より小さい → `Rollback`。同じ → 通る。しきい値は鍵をまたいだ最大値（鍵 A の大きな値で鍵 B の古い更新情報が拒否される）。失効した鍵の値は除かれる。**その更新情報自身が失効させる鍵の値も除かれる**（漏れた P1 の未来の日付の後でも、B1 の P1 失効の更新情報が通る）。`recorded` は `min(issued_at, now)` を記録する（未来の日付の更新情報を受け取った後、正しい日付の次の更新情報が通る） |
| 鍵の移行（SECURITY-4、OPS-UX-TEST-1） | 古い版の信頼の起点（P1、B1）と新しい版（P2、B1、revoked P1）で: (a) 移行の版 N（主 B1、副 P2、`revoked_keys` [P1]）を古い版が主署名で通し、P1 の失効を記録する。(b) **N を見逃した古い版**が N+1（主 P2、副 B1）を受け取る: 主署名は `UnknownKey` → `tries_alternate` が true → 副署名で通り、P1 の失効を記録する。(c) 新しい版は N+1 を主署名で通す。(d) P1 の失効を記録した古い版は、P1 で署名した偽の更新情報（主）を `RevokedKey` とし、副署名がなければ拒否のまま。(e) `BadSignature` などでは `tries_alternate` が false |
| バックアップ用の鍵の回転（FIX-VERIFICATION-1） | 信頼の起点が (P1、B1)、(P1、B2、revoked B1)、(P2、B1、revoked P1)、(P2、B2、revoked P1、revoked B1) の 4 つの版で: (a) B1 が漏れた後の版 N（主 P1、`revoked_keys` [B1]）を、(P1、B1) の版と (P1、B2、revoked B1) の版の両方が通す。前者は B1 を記録しない（後で B1 の更新情報もまだ通る）。後者は何もしない（ビルドで失効済み）。(b) 移行の窓の後の回転の版（主 P2、`revoked_keys` [P1、B1]）を、(P2、B1、revoked P1) と (P2、B2、revoked P1、revoked B1) の両方が通す。(c) (P1、B1) の版は (b) を `UnknownKey` で拒む（窓の外として意図どおり） |
| `apply_trust_report`（SECURITY-5。FIX-VERIFICATION-6） | (a) 機械の記録より新しい更新情報 → `Ok(Some(merged))`: 署名した鍵の最大値が `min(issued_at, now)`、失効が足される。(b) 機械の記録と同じ更新情報をもう一度 → `Ok(None)`。(c) 1 バイト変えた署名 → `Err(BadSignature)`、機械の記録は変わらない。(d) 機械の記録より古い更新情報 → `Err(Rollback)`。(e) 開発用の鍵の更新情報（開発用の cfg のテスト）→ `Ok(None)`。(f) 入っている版より古い版の更新情報でも（`Purpose::Check` なので）記録は進む |
| 期限 | 期限切れ → `Check` でも `Install` でも通り、`Freshness::Expired`。`issued_at >= expires`、差が 800 日を超える → `BadTimestamps` |
| アーキテクチャ | アセットが 1 つしかない、同じ arch が 2 つ、名前が違う（版、arch、大文字小文字、パス区切り）→ `AssetMalformed`。自分の arch を選ぶ |
| ハッシュ | `sha256` が 63 桁、大文字、16 進でない → `AssetMalformed`。`Stager` で中身が違う → `InstallerHashMismatch` |
| 大きさ | 更新情報 64 KiB + 1、署名 4 KiB + 1、アセットの `size` が 0 と 64 MiB + 1 → それぞれの拒否 |
| スキーマ | `schema` 2、`product` 違い、`channel` 違い、未知のフィールド、フィールドの重複、BOM、末尾のデータ、UTF-8 でない、型違い（文字列の数） |
| 版 | `v0.2.1`、`0.2`、`0.2.1-beta`、`0.2.1+x`、`00.2.1`、`65536.0.0` → `BadVersion`。同じ版 → `NotNewer`（`Install`）/ `UpToDate`（`Check`）。古い版 → 同じ。`min_from_version` → `ManualRequired` / `ManualUpdateRequired`。入っている版のプレリリースとの比較 |
| tag | tag と版の不一致 → `TagMismatch` |
| 記録 | `merged`（最大値、和集合）、`recorded`、`is_ahead_of`、`rollback_threshold`、JSON の往復、壊れた JSON |
| 信頼の起点ファイル | 正しいファイル、コメントだけ（`NotConfigured`）、形の誤りの行（行番号付きの `BadAnchorsLine`）、ID と公開鍵の不一致、重複、`check_release_roles`（通常用 2 本、バックアップ用なし、失効した鍵を埋め込み） |
| URL | https だけ、ホストの規則、userinfo、ポート、IP、非 ASCII、相対の `Location` の解決、`Endpoints::production()` の URL の文字列、`tag_from_location`（`latest.json` と `SHA256SUMS` の両方。形が違えば `None`）、副署名の URL |
| `Stager` | 順番の違う `offset`、空のチャンク、64 KiB + 1 のチャンク、合計の超過、足りないまま `finish` |
| 定数 | `installer_name`、`release_page_url`、`user_agent`、`RUNNER_EXE_NAME` の文字列。`DEFAULT_VALIDITY_DAYS` が 180（J-3 の決定）で、`MAX_VALIDITY_SECS` 以下 |
| base64 | RFC 4648 の例、パディングなし、余分なパディング、正準形でない最後の文字 |

**`xtask` の単体テスト**（`ReleaseHost` の偽物と、使い捨ての鍵で署名した偽のリリース。ネットワークなし）

- `prepare-release`: 正常（`latest.json`、trusted comment、`SIGN-OFFLINE.txt`、`prepare.json`）。`--expires-days` なしでは `expires` = `issued_at` + 180 日（J-3 の決定）、`--expires-days 800` まで通り、801 は拒否。拒否するもの: プレリリースの tag、手元の HEAD とタグとコミットの不一致、GitHub のタグのコミットの不一致、下書きでない、余分なアセット、`SHA256SUMS` の食い違い、`digest` の食い違い、来歴の証明の失敗、手元の時計と `Date` の差が 5 分を超える、`issued_at` が公開中のもの以下（未来の日付の公開中のものは `--published-misdated` のときだけ通る）、公開中の失効を引き継いでいない（`--revoke` の欠け）、B.2 に反する `--revoke`、取り残しの検査で受け付けない版がある（`--allow-strand` で通る）。
- `publish`: 通る組、プレリリースの tag の拒否、`prepare.json` の後に下書きの `digest` が変わった、署名が対象の版で通らない、trusted comment の不一致、アップロード後のファイルの組の不一致。
- `prepare-release` の取り残しの検査（FIX-VERIFICATION-1、FIX-VERIFICATION-2）: F.1 の「バックアップ用の鍵の回転」の 4 つの版を窓の中に置き、(a) と (b) の更新情報が拒否されないこと（`--allow-strand` なしで通る）。`window_targets` の B.3 の例の時系列（v0.4.0 が v0.5.0 の公開から 60 日で窓の中 → v0.6.0 の主 P2 だけの準備は拒否、副 B1 を付ければ通る）。後継の公開から 399 日と 401 日の境目。公開後にプレリリースの印を付けた安定版の形のタグは対象に入り、`-` のタグは入らない。
- `check-keys`: 過去のタグで同じ ID が別の公開鍵を指す → 拒否。過去のタグで `revoked` にした ID を埋め込む → 拒否。浅いリポジトリ、`v*` のタグがない、ファイルを持つ最初のタグより後のタグでファイルが読めない → 飛ばさずに失敗（FIX-VERIFICATION-15）。
- `prepare-release --dev`（FIX-VERIFICATION-17）: B.3 の文法どおりの出力（`latest.json`（既定の `expires` は `issued_at` + 180 日）、`trusted-comment.txt` の開発用の接頭辞、`SIGN-OFFLINE.txt` の 1 行）。`--dev-pub` の鍵 ID が本番の信頼の起点にある → 拒否。`--dev` なしの `--dist` などの拒否。`--only-arch` の埋め物。
- `key-drill check`: 正しいバックアップ用の鍵 → 通る。古い（埋め込みにない）バックアップ用の鍵 → 拒否。通常用の鍵 → 拒否。別の `nonce.bin`（前回の点検のもの）の署名 → 拒否。trusted comment が `mklm-key-drill v1` でない → 拒否。対象の版は `window_targets` と同じ。
- `pubkey-line`: 公式の `minisign -G` の `.pub` の形（テストのデータに、形を写したもの）から正しい行と ID。
- 本番のコマンドが、開発用の cfg のビルドで失敗すること（開発用の cfg のテストで）。

### F.2 helper の側のテスト（昇格なし）

- `mklm_ipc`: 新しいメッセージ（`RecordTrust`、`StartingRunner`、`TrustRecorded`、`TrustNotRecorded` を含む）の serde の往復と JSON の形の固定（`PROTOCOL_VERSION` 3）。64 KiB のチャンクのフレームが `MAX_FRAME_LEN` に収まること。16 進の厳密さ（大文字、奇数の長さ）。`RunUpdateArgs` の差分テスト（D.7）。`is_uninstall_restore` と `HelperArgs::parse` が `--run-update …` を受け付けないこと、その逆。
- `mklm_ipc::staging::receive_installer`（偽のリンクと偽の書き込み先）: 正常、順番違い、途中の `Bye`、途中の切断、期限切れ、大きさの超過、ハッシュ違い、`InstallerChunk` の代わりに `Request` が来る。`Stager` は使い捨ての鍵で作った `VerifiedManifest` から作る。
- **H1 の駆動部**（`mklm_ipc::staging::stage_update`、偽の `StagerEnv` と偽のリンク。OPS-UX-TEST-10）: D.4 の各手順で失敗を 1 つずつ注入し、終わりの記録（`Run`、`Trust`、`LastResult`、フォルダー）、送ったメッセージ、ロックの解放、H2 を止めたか、終了コードを表で確かめる。特に: 呼び出し元が 13 の途中で去る → フォルダーと `Run` が消え、`LastResult` を書かない。H2 が `ready` の前に終わる、120 秒の期限 → H2 を止めて終わりを待ってから消す（順序）、`Refused(HandOffFailed)`。`Run` に生きた `installer` → `UpdateInProgress`。空き容量の不足 → `DiskFull`。
- **`RecordTrust` の処理**（`mklm_ipc::staging::record_trust` と `CallerOrder`、偽の `StagerEnv`。SECURITY-5。FIX-VERIFICATION-6）:
  - 新しい更新情報 → `TrustRecorded { changed: true }`、偽の環境の `Trust` が `apply_trust_report` の結果になり、ロックを取って放した。同じものをもう一度 → `TrustRecorded { changed: false }`、書かない。
  - 署名の誤り → `TrustNotRecorded(BadSignature)`、書かない。ロックが `TRUST_LOCK_WAIT`（2 秒）で取れない → `TrustNotRecorded(Busy)`、書かない。
  - どの返事の後も、`CallerOrder` は続く `Request` を受け付ける（セッションは続く）。
  - `CallerOrder`: `Welcome` の直後の `RecordTrust` → 受け付ける。`Request` の後、2 つ目の `RecordTrust`、`StageUpdate` の後の `RecordTrust` → プロトコルの誤り（helper は `HelperMessage::Error` を返して切断）。
  - **記録の後の巻き戻し**: 偽の環境で `record_trust`（更新情報 M2）の後、同じセッションで `stage_update`（M2 より `issued_at` の古い、正しく署名された M1）→ `Refused(Rollback)`、`Run` もフォルダーも作らない。
- 結合テスト（`crates/mklm-client/tests/update_staging.rs` に足す。WP-C が書き、統合の後に通る。F.4）: 利用者の記録が進んでいるとき、`mklm-client` のセッションの開始が `RecordTrust` を送り、偽の helper の側で `record_trust` が機械の記録を進める。進んでいなければ送らない。
- **H2 の駆動部**（`mklm_update::run_flow::run_update`、偽の `RunnerEnv`）: D.12 の表の各行について、D.7 の各手順で失敗を注入し、終わりの `Run` と `LastResult`、起動し直したか、終了コードを確かめる。固定する順序: `Run = installing`（`installer` 付き）を書いてから `ResumeThread`。その書き込みの失敗 → 一時停止のままのインストーラーを止め、`NotInstalled(Refused(Storage))`。`LastResult` を書いて `Run` を消し、ロックを放してから後片付け、最後に起動し直す。15 分の期限 → `Failed(InstallerTimedOut)` を書き、`Run` を残して待ち続け、60 分以内に終われば本当の結果で書き直す。60 分 → `Run` を残し、起動し直さず、7。`ready` / `waiting` でのセッションの終了 → インストーラーを作らず `NotInstalled(SessionEnding)`。`installing` でのセッションの終了 → 止める答え。
- `mklm_update::run`（WP-H）: `classify_installer_exit` の表（25 は `Other`）、`leaves_old_files`、`decide_outcome` の表（D.13。26 と 27 を含む）、`classify_run`（すべての段階 × 起動 ID の変化 × `stager`、`runner`、`installer` の生死。`installing` で runner が死に installer が生きている → `InProgress`）、`interrupted_result`、`RunId` の文法、JSON の往復と形の固定（H.5）。
- `mklm_update::gate::check_journal`: `mklm_core::fixtures` のジャーナルで、読めない項目、書き込み中、確認待ち、再起動待ち、衝突、閉じたものだけ。
- `crates/mklm-update/tests/nsis_exit_codes.rs`: `mklm.nsi` の定義と定数の一致（D.9.1）。
- `mklm-win`:
  - `update_store` の名前の関所（キーを開く前に拒否することだけ。書き込みは実機）、`update_dir` の名前の検査。
  - `elevation::environment_block`: 並び順（大文字小文字を区別しない）、`名前=値\0` の連結と最後の `\0`、空の値、`=` を含む名前の拒否。`runner_environment` が決めた変数だけを持ち、`__COMPAT_LAYER` や利用者の `PATH` を持たないこと（SECURITY-6）。
  - `instance::parse_instance_pipe_name`: 正しい名前（`S-1-5-21-…`、`S-1-12-1-…`）、`/`、`..`、`\`、NUL、ASCII 以外、桁の多すぎる数、余分な部分 → `None`。`pipe::check_pipe_path` が `/`、`.`、`..` だけの名前、ASCII 以外、制御文字を拒否すること（SECURITY-7）。
  - `instance::instance_pipe_candidates`（純粋。FIX-VERIFICATION-5）: GUI のプロセスと `process_users` の結果から、`instance_pipe_path` の名前を作る。SID がない、セッション ID が食い違う、SID が正規表現に合わない（`S-1-5-18` など）GUI は飛ばす。16 本を超えた分は捨てる。同じ入力から同じ順序。
  - `update_dir` の `FileHolder` の変換（Restart Manager の結果の PID、セッション ID、名前。RED-TEAM-3）: 名前の長さの上限、制御文字の除去。
  - `InstanceCommand::QuitIfIdle` の wire（13 バイト、`MAX_INSTANCE_MESSAGE` 以内）と `parse`。
- `installer/check-nsi.ps1` の規則（D.9.3）を、わざと壊したスクリプトの断片で確かめる Pester のテスト（WP-H。Windows PowerShell 5.1 の同梱の Pester 3 で動く形）。

### F.3 ループバックの HTTP テスト（`mklm-update`、feature `winhttp`、Windows、開発用の cfg）

- `$env:RUSTFLAGS = "--cfg mklm_update_dev"; $env:CARGO_TARGET_DIR = "target\dev-update"; cargo test -p mklm-update --features winhttp --locked; cargo test -p mklm-win --features net --test net_proxy --locked`（ci.yml に足す 1 ステップ。別の `CARGO_TARGET_DIR` で、ふだんのビルドのキャッシュを壊さない）。ループバックの経路は `cfg(all(debug_assertions, mklm_update_dev))` なので、この形でだけ走る（A.10）。
- 同じステップの続きで `cargo build -p mklm -p mklm-cli -p mklm-helper --locked` を行い、3 つの exe に目印の文字列が**ある**ことを `installer/find-marker.ps1` で確かめる。なければ失敗（A.10 の陽性の対照。FIX-VERIFICATION-3）。
- テストの中の小さなサーバー（`std::net::TcpListener` を `127.0.0.1:0` に。全インターフェイスに開かないので、Windows ファイアウォールの確認は出ない）が、台本どおりの応答（状態、ヘッダー、本文の分割、遅延）を返す。プロキシを通さないセッション（`new_without_proxy`）を使う。

| テスト | 期待 |
|---|---|
| 200 の小さな本文 | そのまま読める |
| `latest/download` → `download/v0.2.1/latest.json` → 別のポートの 200 | 本文と tag `v0.2.1` |
| `latest/download/SHA256SUMS` → `download/v0.1.0/SHA256SUMS` | tag `v0.1.0`（`fetch_latest_file`） |
| 副署名が 404 | `fetch_alt_signature` が `None` |
| リダイレクト 6 回 | `TooManyRedirects` |
| `https://` やほかのホスト（`localhost`、`127.0.0.2`）へのリダイレクト | `RedirectNotAllowed`、**接続が発生しない**（サーバーが接続を数える） |
| `Content-Length` が上限を超える | 本文を読まずに `TooLarge` |
| チャンク転送で上限を超える | 上限 + 1 バイトで `TooLarge` |
| インストーラーが短い / 長い / 中身が違う | `SizeMismatch` / `HashMismatch` |
| 応答しない（受信の期限を 1 秒にしたテスト用の `Limits`） | `Transport(Timeout)` |
| 全体の期限 | `DeadlineExceeded` |
| 404 / 429 / 500 | `NotFound` / `RateLimited` / `HttpStatus` |
| **401 に `WWW-Authenticate: NTLM` と `Negotiate`** | `HttpStatus { 401 }`。サーバーが受け取ったどの要求にも `Authorization` がない（SECURITY-13） |
| **407 に `Proxy-Authenticate: NTLM`**（プロキシなしのセッション） | `Transport(ProxyAuthRequired)`（状態の番号の写し方だけの試験。WinHTTP はこのサーバーをプロキシとして扱わないので、プロキシの認証の経路は通らない） |
| `Content-Encoding: gzip` | `UnexpectedEncoding` |
| 取り消しのフラグ | `Cancelled` |

**プロキシの認証の試験**（`crates/mklm-win/tests/net_proxy.rs`、feature `net`、開発用の cfg。SECURITY-13。FIX-VERIFICATION-4）

上の 407 の行は、WinHTTP がループバックのサーバーをプロキシとして扱わないので、自動ログオンの方針がプロキシの認証に効くかを何も示さない。そこで、名前付きのプロキシのセッションで確かめる。

- `HttpSession::open_named_proxy(user_agent, "127.0.0.1:<port>", AutologonLevel)`（開発用の cfg だけ。H.3）: `WINHTTP_ACCESS_TYPE_NAMED_PROXY`、バイパスの一覧なし、ほかのオプションは `open` と同じ。
- 要求先はループバックでない名前 `https://update.invalid/latest.json`（`.invalid` は名前解決されない予約の名前。WinHTTP の暗黙のループバックのバイパスも効かない）。WinHTTP はプロキシに `CONNECT update.invalid:443` を送るので、外には何も出ない。
- テストのサーバー（プロキシ役）は、すべての `CONNECT` に `407`、`Proxy-Authenticate: NTLM` と `Proxy-Authenticate: Negotiate`、`Content-Length: 0` を返し、接続を保つ。受け取ったすべての要求の見出しを記録する。**NTLM のチャレンジ（Type 2）は決して返さない**ので、どの場合も資格情報から計算した応答（Type 3）は作られない。
- 期待（`AutologonLevel::High`。製品と同じ）: 結果は `NetErrorKind::ProxyAuth`（か状態 407）で、3 秒のあいだ、どの要求にも `Proxy-Authorization` がない。
- **対照**（`AutologonLevel::Low`）: 同じ台本で、`Proxy-Authorization: NTLM …` か `Negotiate …` が少なくとも 1 回届くこと。届かなければ、この環境では HIGH の試験が何も示さないので、テストを失敗させる（L 章に結果を書く）。
- 平文の `http://update.invalid/` の `GET` をプロキシに送る形も同じ期待で試す（`HttpGet.secure = false` は、開発用の cfg で名前付きのプロキシのセッションに限り、`.invalid` の名前にも許す）。
- この試験が通るまで、L 章の「HIGH がプロキシへの既定の資格情報の送信も止める」は未確認のままにする。`AUTOMATIC_PROXY`（WPAD と PAC）の経路そのものは、この試験でも通らない（自動ログオンの方針がセッションの設定で、プロキシの見つけ方によらず効くという前提は未確認）。

### F.4 GUI と CLI

- `state::tests`:
  - 確認の予定（前回の時刻、乱数の範囲、復帰、**未来の `last_check` と `last_success`**。RELIABILITY-8）。
  - 自動ダウンロードの条件（スキップ、`auto_check` のオフ）。スキップした版の［ダウンロード］。
  - ボタンの可否（D.2 の条件）。セッションは同時に 1 つ（更新とキーボードの変更の排他）。
  - **`quit-if-idle`**（RELIABILITY-1、OPS-UX-TEST-4）: `Idle` → `Effect::Quit`、RunOnce の登録、`closed_by_update`、返事 `ok`。`Launching`、`Running`（カウントダウン、書き込み中、再接続待ち）、`Updating` の各段階（**`HandedOff` を含む**。FIX-VERIFICATION-8）、`quit_pending`、`QuitConfirm` / `RecoveryConfirm` のオーバーレイ → 返事 `busy` で、**`AppState` がまったく変わらず、効果が空**（`closed_by_update` も RunOnce も書かない）。4 秒を過ぎて届いたもの → 何もしない。
  - E.4.1 の表の各升（×、終了、`quit`、`quit-if-idle`、`activate`、セッションの終了 × 更新のセッションの 4 段階）。
  - `session_purpose`（FIX-VERIFICATION-11）: 更新のボタン → `Launching` と `Some(Update)`。変更の適用 → `Launching` と `Some(Change)`。`Notice::Connected` で、`Update` なら `Updating { Sending }`、`Change` なら `Running`。セッションが終われば `None`。
  - `HandedOff` → `started_run`、RunOnce、オーバーレイ、終了。［OK］で即座に終了の経路へ。［OK］なしでは `HANDOFF_OVERLAY_MAX`（15 秒）で終了の経路へ（FIX-VERIFICATION-13）。定数の関係 `HANDOFF_OVERLAY_MAX + 2 秒 < CALLER_EXIT_WAIT` を固定するテスト。最後のチャンクの後の `Lost` で、`Run.caller` が自分で `ready` なら引き継ぎとして扱う。
  - 起動時の `InProgress`（`ready` 以降）→ すぐ終了。セッションが `Run.caller_session` と同じ → RunOnce を消さず、何も書かない。違う、または `caller_session` がない → 昇格していなければ RunOnce を登録し、`closed_by_update` を書いてから終了（FIX-VERIFICATION-9）。
  - `classify`（FIX-VERIFICATION-12）: `CheckError` のすべての列挙子について、E.6 の表と同じ種類と文の ID。`Cancelled`（2 つ）と `NotConfigured` → `None` で、`last_failure` と `last_check` が変わらない。`Rollback` → `Structural`。30 日の間 `Rollback` だけが続いた後のバナーは構造の文（`upd-stale-structural`）と巻き戻しの警告。
  - 結果の表示の条件（D.13）: 14 日を過ぎた `LastResult`、今の版と合わない `LastResult`、ほかの利用者の更新（中立の文）、失敗はほかの利用者に出さない、中断は利用者ごとに 1 回だけ、表示するものがなければ `unregister_after_update`。
  - バナー: 30 日以上確認できていない（一時 / 構造）、期限切れ（発行から 180 日を過ぎた更新情報。J-3）、30 日に 1 回、巻き戻しの警告（同じ `issued_at` に 1 回）。
  - UAC の説明の画面の行き先（`UacNoticeOrigin::Update` で［キャンセル］→ 更新のページ）。
- `vm::update` のスナップショット（日英、m3 H.3）: E.2 の各状態、E.6 の表のすべての行。ラテン文字の検査（E.7）。
- `mklm-client::update`: 偽の `Transport` で `check`（主署名、副署名の取得の条件、キャッシュ、記録の更新、スキップ、巻き戻しの記録、最後の失敗の種類と最初の時刻）と `download`。偽の `Link` で `stage`（正常、`Refused`、途中の取り消し、元のファイルの変化、helper の喪失、`SendInstaller` の中身が申し出と違う、`StartingRunner` の後の Heartbeat の間の待ち）。`pending_trust_report`（利用者の記録が進んでいるときだけ）。
- 結合テスト（統合の後に通る。WP-C が書く）: `crates/mklm-client/tests/update_staging.rs` で、`mklm_client::update::stage` と `mklm_ipc::staging::stage_update`（偽の `StagerEnv`）をメモリ上の双方向のリンクでつなぐ。
- CLI: `update --check` と `--status` の出力と終了コード（0 / 20 / 21 / 22 / 6 / 1 / 2。偽の `Transport` と偽の記録で）。更新中の早い終了: 書き込みのコマンドと `update --check` → 6。`list`、`status`、`global status`、`journal`、`update --status` → 早い終了をせず、ふだんどおり（FIX-VERIFICATION-14）。`--json` に `issued_at` があること（RED-TEAM-1）。

### F.5 インストーラーの CI の試験

D.9.3 の静的な検査と煙の試験。ci.yml（静的な検査）、`installer.yml`（煙の試験。PR で、`installer/**`、`apps/**`、`Cargo.lock` が変わったとき）、release.yml（煙の試験。下書きの前）。

### F.6 全体の経路をどう確かめるか（ローカルのリハーサル、デバッグ ビルド。OPS-UX-TEST-9）

昇格とインストールを伴う部分（H1 のロックとフォルダー、H2、環境ブロック、NSIS の 2 段階の置き換え、起動し直し）は単体テストにできない。v0.2.0 を出す前に、デバッグ ビルドで一度通しで確かめる（ユーザーの同意を得て、開発機で。キーボードの設定には触れない）。

**準備**

- 公式の `minisign.exe` を使う。リハーサルも本番と同じ道具で署名する。**B.5 の準備の最初の項目（zip のダウンロードと `cargo xtask verify-signer`）を、このリハーサルの前に済ませておく**（G.1。FIX-VERIFICATION-17）。確かめた zip から `minisign.exe` を `%USERPROFILE%\mklm-dev-keys\minisign.exe` に取り出し、`Get-FileHash` が記録した値と一致することを確かめる。本物の鍵の USB メモリはリハーサルの間つながない。
- 開発用の鍵は `%TEMP%` ではなく `%USERPROFILE%\mklm-dev-keys\` に置き、最後に消す（SECURITY-9）。
- 作業は使い捨てのブランチで行う: 統合したコミットから `git switch -c rehearsal/m5b`。版の変更は `Cargo.lock` も変えるので（`--locked` のビルドが失敗するため）、このブランチにだけコミットする。

**手順**

1. 開発用の鍵: `%USERPROFILE%\mklm-dev-keys\minisign.exe -G -W -p %USERPROFILE%\mklm-dev-keys\mklm-dev.pub -s %USERPROFILE%\mklm-dev-keys\mklm-dev.key`（`-W` はパスワードなし）。`.pub` の 2 行目（base64）を、**その PowerShell の中だけで** `$env:MKLM_UPDATE_DEV_PUBKEY` に入れる（`setx` は使わない。OPS-UX-TEST-7）。
2. MKLM を全部終わらせる: `& "C:\Program Files\SHIN DATA CENTER\MKLM\mklm.exe" --quit`、`target\` の下で動いている `mklm.exe` にも `--quit`。タスク マネージャーで `mklm*.exe` がないことを確かめる（E.8。2 つ目の起動は `activate` を送って終わり、`--update-endpoint` が届かないため。`target\` の GUI が同じセッションの多重起動のパイプを持っていると、H2 が `NotOurs` から `ProgramsStillRunning` になるため）。
3. 0.2.0: ブランチで `[workspace.package] version = "0.2.0"` → `cargo update -w --offline`（ワークスペースのメンバーの版だけを `Cargo.lock` に反映）→ コミット → `.\installer\build-installer.ps1 -Arch x64 -Profile dev`（`RUSTFLAGS=--cfg mklm_update_dev` を自分の子の `cargo` にだけ渡し、dev プロファイルで 3 つの exe をビルドし、3 つに目印があることを確かめ（なければ失敗。A.10 の陽性の対照）、`dist-dev\0.2.0\MKLM-Setup-0.2.0-x64.exe` と、そのフォルダーだけの `SHA256SUMS` を書く）→ そのインストーラーを手で実行して入れる。入れた GUI が起動すると、HKCU の Run の値はインストールした場所を指すように直る（m3 F.3 の `needs_repair`）。
4. 0.2.1: 同じく版を 0.2.1 にして `cargo update -w --offline` → コミット → `build-installer.ps1 -Arch x64 -Profile dev` → `dist-dev\0.2.1\`。
5. `cargo xtask prepare-release --dev --tag v0.2.1 --dist dist-dev\0.2.1 --dev-pub %USERPROFILE%\mklm-dev-keys\mklm-dev.pub --minisign %USERPROFILE%\mklm-dev-keys\minisign.exe --only-arch x64 --out dist-dev\0.2.1`（文法は B.3 の「`prepare-release --dev` の文法」。ARM64 のアセットは埋め物。ARM64 のクロスビルドの道具は要らない）。この `xtask` は `RUSTFLAGS` のない別の PowerShell でビルドされてもよい（`--dev` のコマンドは cfg を問わない）。
6. `dist-dev\0.2.1\SIGN-OFFLINE.txt` の 1 行（`%USERPROFILE%\mklm-dev-keys\minisign.exe -S -s %USERPROFILE%\mklm-dev-keys\mklm-dev.key -m dist-dev\0.2.1\latest.json -x dist-dev\0.2.1\latest.json.minisig -t "mklm-dev-latest-json v1 version=0.2.1 issued_at=…"`）を実行する。
7. `cargo xtask serve-releases --dir dist-dev --port 8421`（`127.0.0.1` だけで待ち受け、`/releases/latest/download/…` → `/releases/download/v0.2.1/…` のリダイレクトを GitHub と同じ形で返す。「最新」は `latest.json` のある最も新しい版のフォルダー）。
8. 手順 2 と同じく MKLM を全部終わらせてから、インストールしたデバッグの GUI を `"C:\Program Files\SHIN DATA CENTER\MKLM\mklm.exe" --update-endpoint=http://127.0.0.1:8421` で起動 → 確認 → ダウンロード → ［今すぐ更新］→ UAC（「詳細を表示」でプログラムの場所を確かめる）→ 更新 → 起動し直し → 結果。
9. 変えた版はブランチごと捨てる（`main` にはコミットしない）。

**確かめること**

- D.7 の 4 と 16: 読み取りの共有だけのハンドルを開いたまま、一時停止でインストーラーを作れること。`Run` に `installer` が入ってから動き出すこと（`update.log` の順序）。
- D.9.4: 最小の環境ブロックで NSIS が正常に終わること。`$PLUGINSDIR` が `<run dir>\tmp\ns*.tmp` にできたこと（任意: Process Monitor）。
- D.9.2: `.new` と `.old` が残っていないこと。
- D.10 の起動し直し、D.8 の `quit-if-idle`（任意で 2 つ目のアカウント）、`update.log`、`LastResult`。
- **H2 の起動から `ready` までの時間**を `update.log` で記録する。Windows セキュリティの「クラウド提供の保護」をオンにした状態で行う（RELIABILITY-5）。
- 中断（`waiting` の間に H2 を `taskkill` で止める。ユーザーの同意の上で）→ 次の起動で「更新は中断されました」が 1 回だけ出る。
- 任意: `installing` の間にサインアウトを選ぶ → 「MKLM を更新しています」の理由の文で止められる。

**後片付け**（SECURITY-9、OPS-UX-TEST-9）

1. デバッグの MKLM をアンインストールする（「設定」→「アプリ」）。この PC で MKLM を使うなら、公開中のリリース ビルドを入れ直す。
2. 任意（昇格した PowerShell、ユーザーの同意の上で）: リハーサルの `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` の `Run` と `LastResult`（開発用の鍵は `Trust` を変えないので、`Trust` は残してよい）と、`%ProgramData%\SHIN DATA CENTER\MKLM\Updates\` の残り（D.11 の掃除でも消える）。
3. `%USERPROFILE%\mklm-dev-keys\`（取り出した `minisign.exe` を含む）と `dist-dev\` を消す。`MKLM_UPDATE_DEV_PUBKEY` を入れた PowerShell を閉じる。ブランチ `rehearsal/m5b` を消す。
4. この PC で本物の署名をするなら、その前に 1〜3 が済んでいることを確かめる（B.6）。

- リリース ビルドにはこの経路がない（A.10）。本番の鍵とサーバーでの確かめは F.7 と F.8。

### F.7 実機のテスト計画（v0.2.0 → v0.2.1。後でユーザーの同意を得て行う。OPS-UX-TEST-11）

新しい開発機のキーボードはまだ調べていない（Keychron があるかも分からない）。更新の試験はキーボードに書かない試験だけで行い、キーボードに書く試験は、キーボードを M0 の安全手順で調べた後に回す。

準備（全体）: `mklm-cli status --json --all > before.json`。v0.2.0 を手で入れる（B.8）。v0.2.1 を B.5 の手順で公開する。★は 1 項目ずつ同意を得てから。

**グループ A（キーボードに書かない。すぐに行える）**

- [ ] T-UPD-1: v0.2.0 を起動して 3 分待つ → バナー「新しい版（0.2.1）」、更新のページが準備完了。`%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\` にインストーラーと `state.json`
- [ ] T-UPD-2: 設定で自動の確認をオフ → 次の起動で通信しない（`mklm.log`）。［今すぐ確認］で確認できる
- [ ] T-UPD-3 ★: ［今すぐ更新］→ UAC の説明（初回）→ UAC（「詳細を表示」でプログラムの場所が `C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe`、発行元「不明」を記録）→「はい」→ 引き継ぎの案内が出る（［OK］を押さなければ 15 秒で閉じる）→ MKLM が消え、数十秒で 0.2.1 が開き「0.2.1 に更新しました」。起動した GUI が昇格していない。`HKLM\…\MKLM\Update` の `LastResult`、`Updates` の片付け（runner のフォルダーは次の昇格した MKLM まで残る）、`update.log`、`PendingFileRenameOperations` に何も足されていないこと。キーボードの値が変わっていない（`status --json` の比較）
- [ ] T-UPD-4 ★: UAC で「いいえ」→「取り消しました」。何も変わらない
- [ ] T-UPD-5A ★（`Busy`、キーボードに書かない代わり）: 昇格した PowerShell で `$f=[IO.File]::Open("$env:ProgramData\SHIN DATA CENTER\MKLM\mklm.lock",'Open','ReadWrite','ReadWrite'); $f.Lock(0,1)`（`FileLock` の共有は読み取りと書き込みで、この開き方と両立する。`LockFileEx` の排他のロックが byte 0 にかかる）→ ［今すぐ更新］→ UAC の後に「別の MKLM が処理中です」→ `$f.Unlock(0,1); $f.Close()`
- [ ] T-UPD-6A ★（`ProgramsStillRunning`、キーボードに書かない代わり）: 別のターミナルで `mklm-cli set <キーボード> --layout <配列>` を実行し、`Continue? [y/N]` で止めておく（答えるまで何も書かず、UAC も出ない。`commands.rs` の `confirm`）→ ［今すぐ更新］→ 30 秒ほどで「ほかの MKLM（mklm-cli）が動いていたため、更新しませんでした」、0.2.0 のまま → CLI に `n` と答える
- [ ] T-UPD-7 ★（任意）: 別のアカウントでサインインして MKLM を動かしたまま（ユーザーの切り替え）、元のアカウントで更新 → 別のアカウントの MKLM が終わり（`quit-if-idle` の `ok`）、更新される。別のアカウントに**切り替えて戻っても MKLM は戻らない**（再接続はサインインではない）ことを確かめ、そのアカウントでサインアウトしてサインインし直すと、RunOnce で「MKLM は 0.2.1 に更新されました（別のユーザーが更新しました）」と「いったん終了していました」が出る
- [ ] T-UPD-8 ★（任意）: 標準ユーザーのアカウントで、管理者の資格情報を入れて更新 → 更新される。GUI が起動し直すか（D.10 の未確認の点: COM のアクセスの検査）、次のサインインで結果が出るかを記録。起動し直した GUI が、標準ユーザーとして動いていること
- [ ] T-UPD-9: ネットワークを切って［今すぐ確認］→ 名前解決か接続の文。戻して再試行
- [ ] T-UPD-10: ダウンロード中に［キャンセル］→ 止まり、`.part` が次の起動で消える
- [ ] T-UPD-11: ［この版をスキップ］→ バナーが消える。更新のページに「スキップした版」と［ダウンロード］
- [ ] T-UPD-12 ★（任意、危険が小さくない）: H2 の待ちの間に `taskkill /F` で H2（`mklm-update-runner.exe`）を止める（昇格したターミナル）→ 次の起動で「更新は中断されました。何も変更されていません」が 1 回だけ出る
- [ ] T-UPD-13: `mklm-cli update --check` と `--status`（`--json` も）。終了コード（0 / 20）
- [ ] T-UPD-14: ナレーターで更新のページ（見出し、進捗、結果、引き継ぎの案内の読み上げが切れないこと）。表示スケール 200 %
- [ ] T-UPD-15: Windows Defender が `Updates` のインストーラーと `mklm-update-runner.exe` をどう扱うか（隔離されなければ記録だけ。H2 の起動から `ready` までの時間）
- [ ] T-UPD-16 ★（任意）: 更新の `installing` の間にサインアウトを選ぶ → 「MKLM を更新しています。数秒お待ちください」が出て止められる。「キャンセル」で戻れば更新が終わる

**グループ B（キーボードに書く。新しい開発機のキーボードを M0 の安全手順で調べた後）**

準備: M0 の安全手順（`reg export`、PIN、スクリーン キーボード、BitLocker の回復キー）。

- [ ] T-UPD-5B ★: 調べたキーボードの変更を「PC の再起動で切り替える」で保存（再起動待ち）した状態で［今すぐ更新］→ ボタンが押せず理由が出る（再起動待ちを元に戻して終える）
- [ ] T-UPD-6B ★: `mklm-cli` の長いコマンド（カウントダウン中の `set`）の最中に［今すぐ更新］→ UAC の前に止まるか、H2 が `ProgramsStillRunning` / `InstanceBusy` で止め、0.2.0 のまま結果が出る

後始末: `status --json --all` を取り直して `before.json` と比べる。

- グループ B の代わりとして、`OperationOpen` と `RecoveryNeeded` は F.2 の `gate` のテストと F.4 の D.2 の状態のテストが確かめる。

### F.8 本番の取得の経路の確認（OPS-UX-TEST-8、OPS-UX-TEST-3）

| いつ | 何を | どこで |
|---|---|---|
| v0.2.0 のタグの前 | `cargo xtask fetch-smoke`（本番の経路で v0.1.0 の `SHA256SUMS`。リダイレクトの各段、tag の取り出し、TLS、本文） | 開発機。プロキシの内側の PC があればそこでも |
| 毎回のリリース（下書きの前） | `cargo xtask fetch-smoke` | release.yml の x64 のジョブ |
| 公開の直後 | `xtask publish` の公開後の確認（`verify --remote --installers` と同じ） | メンテナーの PC |
| 公開の直後と毎週 | `cargo xtask verify --remote --installers --min-days-left 60 --newest-published` と `cargo xtask fetch-smoke` | `update-canary.yml`（`release: published`、毎週、手動） |
| v0.2.0 の公開の後 | v0.2.0 を入れた PC で `mklm-cli update --check --json` → `"status":"up-to-date"`、`"freshness":"fresh"`、終了コード 0 | 開発機 |

---

## G. 作業の分担

### G.1 順序

1. **WP-0**（1 人）: 骨組み。コンパイルが通り、既存のテストがすべて通る状態で渡す（G.2）。
2. **WP-U、WP-H、WP-C**（並行）: ファイルの持ち主は重ならない（G.3〜G.5）。互いの実装を待たずに、H 章の約束に対して書く。
3. **統合**: 3 つを合わせ、F.4 の結合テストを含めて `cargo test --workspace`、F.3 の開発用の cfg のテスト、`clippy -D warnings`（x64 と ARM64）、`fmt` を通す。
4. **レビュー**（G.6 の確認事項。`minisign-verify` のソースの読み合わせを含む）→ 公式の `minisign` の用意と `xtask verify-signer`（メンテナー。B.5 の準備の最初の項目。F.6 がこれを使う。FIX-VERIFICATION-17）→ F.6 のリハーサル（と後片付け）→ 鍵の生成（メンテナー。B.5 の準備の残り。J-6 の決定 (a) により普段の開発機の普段のアカウントで、ネットワークを切り、F.6 のデバッグ ビルドと開発用の鍵が残っていないことを確かめてから、公式の `minisign` だけで行う。署名専用のアカウントは作らない）→ `fetch-smoke`（F.8）→ v0.2.0 → F.7 のグループ A → （キーボードを調べた後）グループ B。

### G.2 WP-0: 骨組み

**やること**

- ルートの `Cargo.toml`
  - `members` に `"crates/mklm-update"` と `"xtask"`。
  - `[workspace.dependencies]` に:
    ```toml
    mklm-update = { path = "crates/mklm-update" }
    # Update manifest signatures (design m5b B.1): exact pin, bumped deliberately.
    minisign-verify = "=0.3.0"
    # Throwaway test keys only (dev-dependency of mklm-update and xtask). Real keys are handled by the
    # official minisign binary alone (design m5b B.1, B.5).
    minisign = "0.10.0"
    sha2 = "0.11.0"
    semver = "1.0.28"
    ```
  - `[profile.release]` に `debug-assertions = false`（A.10）。
  - `[workspace.lints.rust]` に `unexpected_cfgs = { level = "warn", check-cfg = ['cfg(mklm_update_dev)'] }`（A.10）。
- `.cargo/config.toml`（新規。`[alias]` だけ。release.yml がほかの表を禁じる。A.10）:
  ```toml
  [alias]
  xtask = "run --package xtask --locked --"
  ```
- `apps/build_id.rs`: `HASHED_CRATES` に `"crates/mklm-update"` を足す（配列の長さ 5）。ipc のメッセージが `mklm-update` の型を含むため（m2 A.5 の「版を上げ忘れた変更の検出」）。加えて、各クレートの `Cargo.toml` と `src/` の外にあってビルドに埋め込まれるファイルの一覧 `HASHED_FILES: [&str; 1] = ["crates/mklm-update/trust/anchors.txt"]` を足し、ハッシュと `watched_paths` の両方に入れる（FIX-VERIFICATION-15。信頼の起点ファイルは `include_str!` で埋め込まれるので、鍵だけが違う 2 つのビルドが同じビルド ID にならないように）。`build_id.rs` は 3 つの `build.rs` が `#[path]` で読むだけでテストがないので、WP-0 が一度「`anchors.txt` の行を 1 つ変えると、3 つの exe の `MKLMBuildId` が変わる」ことを手で確かめ、結果を L 章に書く。
- `apps/{mklm,mklm-cli,mklm-helper}/build.rs`: `VS_FF_DEBUG` を `PROFILE` ではなく `CARGO_CFG_DEBUG_ASSERTIONS` の有無から決める（A.10）。`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` のリリース ビルドで `IsDebug` が true になるかを確かめ、結果を L 章に書き足す。
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
  # The WinHTTP transport (mklm_update::winhttp). The GUI, the CLI and xtask get it; the helper never
  # enables it (design m5b A.9).
  winhttp = ["dep:mklm-win"]

  [dev-dependencies]
  minisign.workspace = true
  mklm-core = { workspace = true, features = ["test-fixtures"] }

  [lints]
  workspace = true
  ```
- `crates/mklm-update/trust/anchors.txt`（新規）: B.2 の 1 行目のコメントだけ（鍵なし）。
- `crates/mklm-ipc/Cargo.toml`: `mklm-update.workspace = true`。
- `crates/mklm-win/Cargo.toml`:
  - `[features]` に `net = ["windows/Win32_Networking_WinHttp"]`。
  - `windows` の features に、見込みとして `"Win32_System_Ole"`、`"Win32_System_Variant"`（`shell_launch` の `VARIANT` / `BSTR`）、`"Win32_System_Com"`（`CoInitializeSecurity`）を足す（ほかに要るものは各 WP が足す。G.2 の最後）。
- `crates/mklm-client/Cargo.toml`: `mklm-update = { workspace = true, features = ["winhttp"] }`。
- `apps/mklm-helper/Cargo.toml`: `mklm-update.workspace = true`（feature なし）。
- `apps/mklm/Cargo.toml`、`apps/mklm-cli/Cargo.toml`: `mklm-update.workspace = true`。
- `xtask/Cargo.toml`（新規）: `publish = false`、依存 `mklm-update`（feature `winhttp`）、`sha2`、`semver`、`serde_json`、`clap`、`anyhow`。dev-dependency に `minisign`。**通常の依存に `minisign` を入れない**（B.1）。`src/main.rs` は B.3 のサブコマンドを clap で定義し、本体は「not implemented yet」のエラーで終了コード 1。
- H 章のすべての公開の項目を、該当するファイルに置く。
  - **型、列挙、定数、トレイトは完全に書く**（serde の属性を含む）。
  - **小さな文法と表の関数は WP-0 が完全に実装し、テストを付ける**: `KeyId::parse` と表示、`parse_anchors`（行の文法だけ。鍵の検査は WP-U）、`Arch::as_str` / `of_this_build`、`installer_name`、`release_page_url`、`user_agent`、`SignatureSlot::file_name`、`RunId::new` / `parse`、`RunUpdateArgs::parse` / `to_parameters` / `run_update_args`、`encode_hex` / `decode_hex`、`InstallerChunk::new` / `decode`、`classify_installer_exit`、`InstallerExit::leaves_old_files`、`nsis_exit` の定数、`UPDATE_VALUE_NAMES`、`Endpoints::production` の URL、`InstanceCommand::QuitIfIdle` の wire と `parse`、`instance::parse_instance_pipe_name`、`elevation::environment_block`。3 つの WP がこれらの結果を前提にするため。
  - **それ以外の関数の本体**は、テストが通る経路に `todo!()` を置かない。`Result` を返すものは `Err`（`UpdateRefusal::Internal { detail: "not implemented (m5b skeleton)" }`、`mklm_win::Error::Win32 { function: "<名前> (m5b skeleton)", code: 50 }`（`ERROR_NOT_SUPPORTED`）など）、そのほかは害のない既定値を返す。モジュールの先頭に `#![allow(unused_variables, dead_code)] // Skeleton (M5b)` を付ける（m2 0.4 と同じ）。
- 既存のコードをコンパイルさせる変更（網羅的な `match`）:
  - `apps/mklm-helper/src/session.rs`: `serve` のループに `CallerMessage::RecordTrust` → 骨組みは `TrustNotRecorded(Internal)` を送って続ける、`CallerMessage::StageUpdate` → `crate::update::stage(…)`（骨組みは `Update::Refused(Internal)` を送って続ける）、`CallerMessage::InstallerChunk` → プロトコルの誤り。`Inbox::poll` で 3 つを「去った」に。`session()` の先頭の分岐に `mklm_ipc::run_update_args` → `crate::run_update::run(args)`（骨組みは終了コード 7）。`apps/mklm-helper/src/update.rs` と `run_update.rs` を作る。
  - `crates/mklm-client/src/session.rs`: 中継の `match` に `HelperMessage::Update(_)` を足し、`Hello` と同じく「予期しないフレーム」で失う扱いにする（`RecordTrust` の返事は WP-C が扱う）。
  - `apps/mklm/src/app.rs` の `instance_command`: `InstanceCommand::QuitIfIdle` → 骨組みは何もせず `InstanceReply::Busy`。
  - `apps/mklm-cli/src/write/relay.rs` のテストの偽物など、`HelperMessage` / `CallerMessage` を網羅するもの。
  - `crates/mklm-ipc/tests/messages.rs`: 列挙子を網羅するテストに新しいものを足す。
  - `crates/mklm-client/src/lib.rs`: `pub mod update;`（中は H.4 の骨組み）。
  - `crates/mklm-win/src/lib.rs`: `pub mod update_store; pub mod update_dir; pub mod shell_launch; pub mod user_dirs; #[cfg(feature = "net")] pub mod net;`。`ui/mod.rs` に `pub mod open_url;`。
- `cargo build --workspace`、`cargo test --workspace`、F.3 の開発用の cfg のコマンド（骨組みではテストは空でよい）、`cargo clippy --workspace --all-targets -- -D warnings`（x64 と `--target aarch64-pc-windows-msvc`）、`cargo fmt --check` が通ること。`Cargo.lock` をコミットする（以後、WP は依存を足さない。足す必要があれば統合のときにまとめる）。

**WP-0 が完了したら、G.3〜G.5 の持ち主に渡す。** `Cargo.toml` と `Cargo.lock` は WP-0 の後、統合まで誰も変えない。例外: **各 WP は、自分のモジュールに要る `windows` の features を `crates/mklm-win/Cargo.toml` に足してよい**（WP-U の `net.rs`、WP-C の `user_dirs.rs` と `ui/open_url.rs`、WP-H のそのほか。features は `Cargo.lock` を変えない。統合のときに行をまとめる。OPS-UX-TEST-12）。

### G.3 WP-U: `mklm-update`、署名の道具、メンテナーの文書

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-update/src/{lib,keys,manifest,verify,state,version,refusal,url,fetch,stage,winhttp,base64,dev}.rs`、`crates/mklm-update/tests/*`（`nsis_exit_codes.rs` を除く）、`crates/mklm-update/trust/anchors.txt` の形（鍵はメンテナーが入れる） | C 章、A.4〜A.10、B.2 |
| `crates/mklm-win/src/net.rs`、`crates/mklm-win/tests/net_proxy.rs`（新規） | WinHTTP の FFI（A.7、A.9。自動ログオンの方針）。unsafe にはすべて `// SAFETY:`。プロキシの認証の試験（F.3。FIX-VERIFICATION-4） |
| `xtask/**` | B.3 のすべてのコマンド、F.6 の `prepare-release --dev` と `serve-releases`、`ReleaseHost`（`gh` の呼び出し） |
| `docs/maintainer/release-signing.ja.md`（新規） | B.5〜B.7、公式の `minisign` の SHA-256 の記録、点検の記録の欄 |
| `docs/install-guide.ja.md`、`README.md` | 自動更新の説明、UAC の「詳細を表示」でプログラムの場所を確かめる手順（SECURITY-14）、公開鍵と `minisign` による手での検証、インストーラーの `/S` と終了コード（D.9.1）、`mklm-cli update --check` の終了コード（6 を含む。D.14）、ほかの利用者の PC で管理者として承認するときの注意（D.13。RED-TEAM-2） |

完了の条件: F.1、F.3 が通る。`xtask` のテスト。

### G.4 WP-H: パイプ、Windows の部品、helper、NSIS、CI

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-update/src/{run,run_flow,gate}.rs`、`crates/mklm-update/tests/nsis_exit_codes.rs` | D.7（駆動部と `RunnerEnv`）、D.13 の判定、ジャーナルの門 |
| `crates/mklm-ipc/src/{lib,message,update,staging,args}.rs`、`crates/mklm-ipc/tests/*` | D.3、D.4 の駆動部（`stage_update` と `StagerEnv`）、D.7 のコマンドライン、F.2 |
| `crates/mklm-win/src/{update_store,update_dir,shell_launch,proc_identity,instance,pipe,os,elevation,session_end}.rs`（`pipe` は `check_pipe_path` だけ。`elevation` は環境ブロックと `spawn_clean` だけ。`session_end` は `shut_down_first`、`spawn_with_answer`、`set_block_reason` だけ）、`protected_dir.rs` と `journal_store.rs`（`RunDir` と `UpdateStore` に要る内部の関数を公開する変更だけ） | D.5〜D.11、C.7 |
| `apps/mklm-helper/src/{main,session,update,run_update}.rs` | D.4、D.7、D.15、`RecordTrust`、通常のセッションの片付け（D.11）、`StageUpdate` の間の `busy` |
| `installer/nsis/mklm.nsi`、`installer/build-installer.ps1`（`-Profile dev`、目印の陽性の対照）、`installer/check-nsi.ps1`（新規）、`installer/smoke-test.ps1`（新規）、`installer/check-build-env.ps1`（新規。A.10 のビルドの前の検査）、`installer/find-marker.ps1`（新規。目印の「ある / ない」の検査）、`installer/tests/*.Tests.ps1`（新規） | D.9、F.5、F.6、A.10 |
| `.github/workflows/{ci,release,installer,update-canary}.yml` | G.6 |
| `docs/recovery.md` | 「更新が途中で止まったとき」「更新の後に MKLM が起動しないとき」（D.13）。「更新がいつも『ファイルを開いていた』『ほかの MKLM が動いていた』で止まるとき」（`holders` の読み方、タスク マネージャーで止める、再起動の直後に更新する。D.8。RED-TEAM-3）。「ほかの利用者の PC で管理者として承認するとき」（D.13。RED-TEAM-2） |

完了の条件: F.2、F.5 が通る。F.6 のリハーサルの手順が動く（実施は同意の後）。

### G.5 WP-C: クライアント、GUI、CLI、多言語

| 持つファイル | 内容 |
|---|---|
| `crates/mklm-client/src/update/**`、`crates/mklm-client/src/lib.rs`、`crates/mklm-client/src/session.rs`（セッションの開始の `RecordTrust` と `Update` の返事の扱いだけ）、`crates/mklm-client/tests/update_*.rs` | H.4、D.2、D.3 の送る側、C.4 の利用者の記録と `RecordTrust` |
| `crates/mklm-win/src/{user_dirs.rs,ui/open_url.rs}`、`crates/mklm-win/src/session.rs`（`register_after_update` / `unregister_after_update` だけ） | E.1、E.7、D.10 |
| `apps/mklm/**`（`state.rs` の `SessionPhase::Updating`、`SessionPurpose` と `AppState::session_purpose`、`UacNoticeOrigin`、`quit_if_idle`、`state/update.rs`、`vm/update.rs`、`vm/mod.rs` の許可リスト、`i18n.rs`、`ui/screens/update.slint`、`app.slint`、`translations/`、`settings.rs`、`args.rs`（`--after-update`、開発用の `--update-endpoint`）、`worker.rs`、`app.rs`（`instance_command` の `QuitIfIdle`）、`single_instance.rs`（遅れて届いた `quit-if-idle` の扱い）、`tray.rs`、`tests/*`） | E 章 |
| `apps/mklm-cli/src/{update.rs,main.rs}` | D.14（終了コード、更新中の早い終了） |

完了の条件: F.4 が通る（結合テストは統合の後）。

### G.6 CI と、レビューで確かめること

**ci.yml に足すもの**（WP-H）

- F.3: `RUSTFLAGS=--cfg mklm_update_dev`、`CARGO_TARGET_DIR=target\dev-update` で `cargo test -p mklm-update --features winhttp --locked` と `cargo test -p mklm-win --features net --test net_proxy --locked`、続けて同じ設定で 3 つの exe をビルドし、目印が**ある**ことを確かめる（A.10 の陽性の対照。FIX-VERIFICATION-3）。
- `cargo tree -p mklm-helper -e features --locked` の出力に `mklm-win feature "net"` と `mklm-update feature "winhttp"` がないこと。
- `installer/check-nsi.ps1` と、その Pester のテスト（D.9.3、F.2）。

**installer.yml（新規。WP-H）**

- `pull_request` と `main` への push で、`installer/**`、`apps/**`、`Cargo.lock`、このファイルが変わったとき。`runs-on: windows-2025`。
- 固定した NSIS の zip（下）→ x64 のインストーラーをビルド → `installer/smoke-test.ps1`（D.9.3 の煙の試験）。

**release.yml に足すもの**（WP-H）

- 権限: `build` のジョブに `id-token: write` と `attestations: write`（来歴の証明）、`contents: read`。`draft-release` は `contents: write` のまま。
- `build` のジョブの `actions/checkout` に `fetch-depth: 0` を付ける（すべての履歴とタグ。`check-keys` が過去のタグの信頼の起点ファイルを `git show` で読むため。今は既定の 1 で、タグを取らない。FIX-VERIFICATION-15）。`persist-credentials: false` はそのまま。
- ビルドの前:
  - A.10 の守り（`installer/check-build-env.ps1`: `RUSTFLAGS`、`CARGO_TARGET_*_RUSTFLAGS`、`RUSTC_WRAPPER` などの環境変数、リポジトリと親のフォルダーと `CARGO_HOME` の Cargo の設定ファイル）。ワークフローのどのコマンドにも `--config` を書かない。
  - `cargo xtask check-keys`（鍵がない、壊れた、役割の数が違う、ID の使い回し、失効させた ID の再利用のリリースを止める。過去のタグを読めなければ失敗。B.2、B.3）。
  - **NSIS の固定**（SECURITY-1）: `choco install nsis` をやめ、公式の `nsis-3.12.zip`（SourceForge）をダウンロードし、ワークフローに書いた SHA-256 と一致しなければ失敗。展開した `makensis.exe` を `build-installer.ps1 -Makensis` に渡す。SHA-256 の値は、WP-H が一度ダウンロードし、SourceForge が表示するハッシュと照らしてから書く（L 章）。
- ビルドの後:
  - 3 つの exe の `VersionInfo.IsDebug` が false。6 つの exe（2 つのジョブ）に目印の文字列がない（A.10）。
  - `dumpbin /imports mklm-helper.exe` に `WINHTTP.dll` がない（A.9）。`dumpbin` は PATH にないので、`vswhere.exe -latest -products * -find "VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe"` で探す（ARM64 の exe も x64 の `dumpbin` で読める。OPS-UX-TEST-19）。
  - **`actions/attest-build-provenance`（SHA で固定）で 2 つのインストーラーの来歴を証明する。必須**（失敗すればジョブが失敗し、下書きを作らない。`prepare-release` が `gh attestation verify` で確かめる。SECURITY-1）。
  - x64 のジョブで `cargo xtask fetch-smoke`（F.8）。
- 煙の試験のジョブ（`needs: build`、`runs-on: windows-2025`）: 2 つのインストーラーを受け取り、`smoke-test.ps1`（直前のリリースからの上書きを含む。D.9.3）。
- `draft-release`（`needs: [build, smoke]`）:
  - タグに `-` を含む（プレリリース）なら `gh release create … --prerelease --title "MKLM $TAG (pre-release, no auto-update)"`。
  - そうでなければ `--title "MKLM $TAG — UNSIGNED, DO NOT PUBLISH"`。ノートの先頭に「署名と公開は docs/maintainer/release-signing.ja.md の手順（`cargo xtask prepare-release` と `publish`）で行う。このページの Publish ボタンは使わない」。
- アクションはすべて SHA で固定（今と同じ）。

**update-canary.yml（新規。WP-H。OPS-UX-TEST-3）**

- きっかけ: `release: [published]`、毎週（例: 月曜 03:17 UTC）、`workflow_dispatch`。
- 権限: `contents: read` だけ。自分で登録した secrets を使わない（`gh` には自動の `GITHUB_TOKEN` を `GH_TOKEN` として渡す）。`runs-on: windows-latest`。
- `cargo xtask verify --remote --installers --min-days-left 60 --newest-published` と `cargo xtask fetch-smoke`。更新情報がない、署名が通らない、tag が合わない、期限まで 60 日未満、インストーラーの大きさかハッシュが違えば失敗し、GitHub がメンテナーにメールで知らせる。
- `--newest-published`（RED-TEAM-1）: 配られている `latest.json` の版が、`gh release list --exclude-drafts` の安定版の形のタグ（公開後にプレリリースの印を付けたものを含む）の最も新しい版より古ければ失敗する。「最新」の印を外された（事故か、リポジトリを握った攻撃者による凍結）ことを、メンテナーが知るため。事故の手順（B.5）で印を外した後は、次の版を出すまで失敗し続ける（それでよい）。
- 注: 公開のリポジトリの定期のワークフローは、60 日間リポジトリに活動がないと GitHub が止める（GitHub のドキュメント。今回は確かめ直していない）。カレンダーの予定（B.5 の注）を残す。

**レビューで確かめること**（m2 K、m3 I の規則への追加）

- HKLM に `KEY_SET_VALUE` を使うのは `regwrite`、`journal_store`、`machine_settings`、`update_store` だけ。`update_store` は `UPDATE_VALUE_NAMES` の 3 つだけを書く。
- HKCU に書くのは `session` だけ（RunOnce の `AfterUpdate` が増えた）。helper は HKCU を読み書きしない。
- `ShellExecuteW` / `ShellExecuteExW` に渡してよいものに、`open_release_page` が作るリリース ページの URL と、`run_installer_interactive` の検証し直したキャッシュのインストーラー（利用者のボタンのときだけ）が増えた。
- helper は利用者の場所のファイルを開かない（インストーラーはパイプで受け取る）。helper の依存にネットワークのコードがない（上の CI）。
- helper の固定のコマンドラインは 3 つ（パイプのセッション、`--uninstall-restore`、`--run-update`）で、どれも手書きの厳密なパーサー。
- **H1 が H2 を、H2 が NSIS を起動するときは、`runner_environment` の最小の環境ブロックを明示し、親の環境を引き継がない**。作業フォルダーは System32（SECURITY-6）。
- **H2 は、ほかのどの COM の呼び出しより前に `CoInitializeSecurity`（`RPC_C_IMP_LEVEL_IDENTIFY`、`EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA`）を呼ぶ**（SECURITY-8）。
- **H2 は、多重起動のパイプを列挙せず、動いている `$INSTDIR\mklm.exe` のプロセスのセッションと SID から名前を計算し、厳密な検査にも通してから開く**（FIX-VERIFICATION-5）。`quit` を送らず、`quit-if-idle` だけを送る（SECURITY-7、RELIABILITY-1）。
- 通常用の鍵の署名の中の、埋め込みのバックアップ用の鍵の失効は無視され、記録されない（B.2 の規則 2。F.1 の「バックアップ用の鍵の回転」。FIX-VERIFICATION-1）。
- `LastResult` の `holders` に利用者の SID を入れない（RED-TEAM-3）。［インストーラーを実行］の周りの文に「検証済み」がない（RED-TEAM-2）。
- **WinHTTP は `WINHTTP_OPTION_AUTOLOGON_POLICY` を HIGH にし、`WinHttpSetCredentials` を呼ばない**（SECURITY-13）。
- 本番以外の URL、平文 HTTP、開発用の鍵が `#[cfg(all(debug_assertions, mklm_update_dev))]` の外にない（A.10）。開発用の分岐が `black_box(&DEV_MARKER)` で目印を参照している。ワークフローに `--config` と、フラグを足す環境変数がない。
- **`xtask` に秘密鍵のファイルを開くコードがなく、パスワードを尋ねない。`xtask` の通常の依存に `minisign` のクレートがない**（B.1、B.3）。
- `verify_manifest` は署名を確かめる前に更新情報の中身を解析しない（C.3 の 6）。失効の規則（B.2 の表）と、巻き戻しのしきい値（記録は `min(issued_at, now)`、判断は鍵をまたいだ最大値）が F.1 のテストと合っている。
- H2 はインストーラーのハンドルを `CreateProcessW` まで閉じない。インストーラーを一時停止で作り、`Run.installer` を書いてから再開する。H2 は自分のトークンで GUI を起動せず、起動し直しは最後の手順。
- 更新のコードのどこにも `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` がない（D.11）。
- 利用者がボタンを押していない UAC がない。
- `mklm.nsi`: すべての `Quit` の直前に `SetErrorLevel`、`SetErrorLevel` は `Quit` の直前だけ（3010 を除く）、すべての `MessageBox` に `/SD`、3 つの exe は `.new` から名前の変更で入れ替える、実行中の MKLM を `$INSTDIR` のパスでだけ見つける（プロセスの名前を使わない。RELIABILITY-12）。
- GUI の `quit-if-idle` は、`busy` を返すとき状態を何も変えない（F.4 のテスト）。
- 利用者向けの文言が `i18n.rs` と `.po` の外にない。E.6 の表のすべての列挙子に文がある。
- **`minisign-verify` 0.3.0 のソース全体を読み、結果（読んだ版、気になった点）を `docs/maintainer/release-signing.ja.md` に記録する**（v0.2.0 の前に 1 回。版を上げるたびに差分。SECURITY-2）。

### G.7 リスク

1. **GitHub の配布の仕組みの変更**: リダイレクトの形や配布元のドメインが変わると、確認か署名の取得が止まる（A.6 で `.githubusercontent.com` の接尾辞と tag なしの予備を用意した）。止まった場合、更新で直せないので、手で入れてもらう。利用者には、構造の失敗の文と 30 日の知らせで伝わり（E.3、E.6）、メンテナーには見張りが知らせる（G.6）。
2. **新しい版のクレート**: `minisign-verify` 0.3.0 と `minisign` 0.10.0 は出たばかりで、変更点を確かめていない。WP-0 がシグネチャを確かめ、レビューで `minisign-verify` のソースを読む。問題があれば 0.2.5 / 0.9.1 に戻す（`minisign` のクレートはテストにしか使わない）。
3. **最初の更新対応版の不具合**: v0.2.0 のアップデーターに不具合があると、v0.2.0 の利用者は手で直す必要がある。F.6 のリハーサル、本番の経路の確認（F.8）で減らす。
4. **起動し直しの失敗**: 別の管理者で昇格した場合など（D.10）。RunOnce、Run キー、案内の文で補うが、「MKLM が消えた」と感じる利用者がいうる。
5. **途中で止まったインストール**: 電源断や、利用者が強制したシャットダウン（D.12）。2 段階の置き換えで半端の時間は短くなり、検出と［インストーラーを実行］で直せる。
6. **ウイルス対策ソフト**: 署名のないインストーラーと runner を `ProgramData` から実行するので、止められたり遅くなったりしうる（`InstallerNotStarted`、準備の期限 120 秒、インストールの期限 15 分〜60 分）。
7. **有効期限の失効**: 有効期限の既定は 180 日（J-3 の決定）なので、メンテナーが 180 日（約 6 か月）リリースしないと、全員に期限切れの情報とバナーが出る（B.4）。見張りが 60 日前（発行から 120 日後）から知らせる。インストールは止まらない（C.6）。
8. **鍵をなくす**: 両方をなくすと自動更新が止まり、手で入れ直してもらうしかない（B.7）。**バックアップ用の鍵の漏れは、通常用の漏れと同じかそれ以上に重い**（B.2 の規則 4）。通常用を替えた後の移行の窓では、バックアップ用の鍵をリリースのたびに使う。
9. **feature の統合**: ワークスペースのビルドで helper にも WinHTTP のコードがコンパイルされる。リンカーが落とす前提で、release.yml のインポートの検査で守る（A.9）。
10. **ほかの利用者の MKLM を終わらせる**: 何もしていない MKLM は、その利用者の同意なしに終わり、**その利用者が開き直すかサインインし直すまで戻らない**（ユーザーの切り替えで戻るのは再接続で、Run キーは動かない。D.8、J-2 の決定 (a)）。更新の途中（`ready` から `finishing`）にサインインした、または MKLM を開いたほかの利用者の MKLM も、起動を見送り、次に開くかサインインするまで動かない（D.13 の 1。FIX-VERIFICATION-9）。
11. **ロックの受け渡しの隙間**: H1 がロックを放してから H2 が取るまでの間に、ほかの書き手が操作を始めうる。H2 がジャーナルを確かめ直して止めるので安全だが、更新はやり直しになる（D.7 の 10）。
12. **ジョブ オブジェクト**: H1 が kill-on-close のジョブに入っていると、H1 の終了で H2 も終わる。H1 は UAC（AppInfo）が作るので呼び出し元のジョブには入らないはずだが、確かめていない。H2 は `ready` の前に H1 に見られているので、止められても `Interrupted` として検出される。
13. **更新とアンインストールの同時実行**: アンインストーラー（`--uninstall-restore` はロックで待つが、ファイルの削除は止まらない）と重なると、結果が壊れうる。起こりにくいので、検出（D.13）に任せる。
14. **x64 版を ARM64 の PC で使っている利用者**: ARM64 版に切り替わらない（C.7。J-1 の決定 (a)）。
15. **リハーサルと本番の違い**: F.6 はデバッグ ビルドと平文の HTTP で、TLS、GitHub、リリース ビルドの最適化は F.7 と F.8 でしか確かめられない。本番の取得の経路は `fetch-smoke` で v0.2.0 の前に確かめる。
16. **ほかの利用者による妨害**（SECURITY-10。FIX-VERIFICATION-7、RED-TEAM-3 で書き直した）: この PC の**どの利用者も、期限なしに、すべての更新（セキュリティの修正を含む）と手でのインストールを止め続けられる**。半端には入らない（D.9.2）が、防ぎようがなく、自動で抜ける方法もない。方法は少なくとも次の 4 つ。
    - `$INSTDIR` の exe を削除の共有なしで開いたままにする（D.8 の 4 の `FilesInUse`、NSIS の 26）。
    - 自分の MKLM を何かの途中（キーボードの変更の確認待ちなど）にしておく、または自分の MKLM に手を加えて `quit-if-idle` にいつも `busy` を返させる（`InstanceBusy`）。
    - `mklm-cli`（`set` の `Continue? [y/N]` など）を入力待ちのまま置く（`ProgramsStillRunning`。F.7 の T-UPD-6A と同じ形）。
    - GUI より先に、その利用者の多重起動のパイプの名前を作っておく（その GUI は `NotOurs` になり、3 の待ちで `ProgramsStillRunning`。D.8 の 2）。
    - 管理者への手がかりとして、結果と `update --status` に相手のプロセス（PID、セッション ID、名前）を残す（D.8、E.6）。管理者はそのプロセスを止めるか、PC を再起動してほかの利用者がサインインする前に更新する（`docs/recovery.md`）。J-2 のどの選択肢でも、この妨害は変わらない（決定は (a)）。
17. **認証の要るプロキシ**（SECURITY-13）: Windows の資格情報を自動で送らないので、統合認証のプロキシの内側では自動更新が使えない（J-8 の決定 (a)）。
18. **機械の記録の遅れ**（SECURITY-5）: 機械の記録が進むのは誰かが helper を起動したときだけなので、悪意のある利用者が管理者に古い正しい版を入れさせる攻撃は、記録が進んでいない PC では防げない（C.4）。
19. **来歴の証明とレビューの限界**（SECURITY-1）: 来歴の証明は「どのワークフローが、どのコミットから作ったか」を示すだけ。依存のクレートや GitHub のランナーそのものが侵されていれば防げない。差分のレビューと、任意の手元のビルドとの比べ合わせで減らす。
20. **公式の `minisign` の信頼**: 秘密鍵に触れる唯一の道具なので、初回に作者の署名で確かめ、ハッシュを固定し、鍵と同じ媒体に置く（B.5）。作者の鍵そのものが侵されれば防げない。
21. **署名する PC で動く開発のコード**（FIX-VERIFICATION-16）: `prepare-release` とふだんの開発は、依存のクレートのビルド スクリプトと proc-macro をメンテナーの権限で実行する。侵された依存が常駐すれば、数分後につなぐ鍵のファイルとパスワードを盗める（B.6 の「残る危険」）。**J-6 の決定 (a)**（2026-09-29）で、署名は普段の開発機の普段のアカウントで行い、署名専用のアカウントは作らない。ネットワークを切る、開発用の鍵とデバッグ ビルドを残さない、鍵に触れるのは公式の `minisign` だけ、という注意（B.6）は、侵された依存の常駐を締め出さない。**ユーザーはこの残る危険（侵されたビルドの依存が鍵とパスワードを盗みうること）を受け入れた**。起きた場合は、来歴の証明、公開後の検査、見張りで後から見つけ（19 と同じ）、B.7 の手順で鍵を替える。危険を下げたくなったときの対策は B.6 の「残る危険」の 1〜3。
22. **凍結**（RED-TEAM-1）: GitHub のリポジトリに書ける攻撃者（「最新」の印を外す、リリースを消す）か、TLS を検査するプロキシを握った攻撃者は、正しく署名された古い更新情報を見せ続けて、新しい版（セキュリティの修正を含む）を隠せる。確認は成功し続け、画面は「最新です」のまま。自動で知らせるのは `expires` だけで、最長で有効期限の長さ（既定 180 日。J-3 の決定）かかる（B.4 の 4）。時刻の新しさだけを頻繁に保証する別の鍵（TUF の timestamp の役割）は、オンラインの鍵が要るので、今の決定（秘密鍵はオフラインだけ）の外にある。

---

## H. API の約束

WP の境界を越えるすべての公開の項目。ここにない公開の項目は、その WP の中の都合で決めてよい。`use` と `derive` は、ここに書いたものを必ず持つ（書いていない `derive` を足すのはよい）。レビュー第 1 回で変わった項目には、コメントに指摘の ID を付けた。

依存の向き: `mklm-ipc` → `mklm-update` →（feature `winhttp` のときだけ）`mklm-win`。`mklm-win` は `mklm-update` に依存できない（循環になる）ので、`mklm-win` の関数は自分の型か標準の型を返し、`mklm-update` の側で変換する（例: `update_dir::read_build_ids` → `InstallState::from_build_ids`）。

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
pub mod run_flow;
pub mod stage;
pub mod state;
pub mod url;
pub mod verify;
pub mod version;
#[cfg(all(windows, feature = "winhttp"))]
pub mod winhttp;
mod base64;
#[cfg(all(debug_assertions, mklm_update_dev))]
mod dev; // the development key and DEV_MARKER (design m5b A.10)

pub use keys::{AnchorEntry, AnchorsFile, KeyError, KeyFingerprint, KeyId, KeyRole, TrustAnchors};
pub use manifest::{Arch, Manifest, ManifestAsset, Sha256Digest, Sha256Stream};
pub use refusal::UpdateRefusal;
pub use semver::Version;
pub use state::{RevokedKey, StateError, TrustState};
pub use verify::{
    Freshness, OfferKind, Purpose, SelectedAsset, SignatureSlot, VerifiedManifest, VerifyInput,
    apply_trust_report, tries_alternate, verify_manifest,
};

pub const PRODUCT: &str = "MKLM";
pub const CHANNEL: &str = "stable";
pub const MANIFEST_SCHEMA: u32 = 1;
pub const MANIFEST_NAME: &str = "latest.json";
pub const SIGNATURE_NAME: &str = "latest.json.minisig";
/// The alternate signature, present during a key transition (SECURITY-4, OPS-UX-TEST-1).
pub const ALT_SIGNATURE_NAME: &str = "latest.json.alt.minisig";
pub const TRUSTED_COMMENT_PREFIX: &str = "mklm-latest-json v1";
/// Rehearsal manifests; accepted only from the development key in development builds.
pub const DEV_TRUSTED_COMMENT_PREFIX: &str = "mklm-dev-latest-json v1";
/// `xtask key-drill`; never accepted as a manifest (OPS-UX-TEST-13).
pub const KEY_DRILL_COMMENT_PREFIX: &str = "mklm-key-drill v1";
pub const REPO_URL: &str = "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager";
pub const MAX_MANIFEST_LEN: usize = 64 * 1024;
pub const MAX_SIGNATURE_LEN: usize = 4 * 1024;
pub const MAX_INSTALLER_LEN: u64 = 64 * 1024 * 1024;
pub const MAX_VALIDITY_SECS: u64 = 800 * 86_400;
/// xtask's default `--expires-days` (user decision J-3, 2026-09-29: 180 days; the canary warns 60
/// days before).
pub const DEFAULT_VALIDITY_DAYS: u64 = 180;
/// A release stays a strand-check target until this many days after its successor was published
/// (xtask `window_targets`, design m5b B.3; FIX-VERIFICATION-2).
pub const TRANSITION_WINDOW_DAYS: u64 = 400;
/// `key_ids` holds 1 or 2 IDs.
pub const MAX_KEY_IDS: usize = 2;
/// The GUI's start argument after an update (H2 and the RunOnce value pass it).
pub const GUI_AFTER_UPDATE_ARG: &str = "--after-update";
/// H2's file name in its run folder (RELIABILITY-12).
pub const RUNNER_EXE_NAME: &str = "mklm-update-runner.exe";

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

/// SHA-256 of the decoded public key (42 bytes: algorithm, key ID, Ed25519 key). Scopes recorded
/// revocations (SECURITY-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyFingerprint(pub [u8; 32]);
impl KeyFingerprint {
    /// 64 lower-case hex digits.
    pub fn to_hex(&self) -> String;
    pub fn parse_hex(text: &str) -> Option<KeyFingerprint>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRole {
    Primary,
    Backup,
}

/// One `primary` / `backup` line of the trust anchors file (design m5b B.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorEntry {
    pub role: KeyRole,
    pub id: KeyId,
    /// The base64 line of the minisign public key file.
    pub public_key: String,
}

/// The parsed trust anchors file (OPS-UX-TEST-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnchorsFile {
    pub keys: Vec<AnchorEntry>,
    pub revoked: Vec<KeyId>,
}

/// Path of the file in the repository, for `git show <tag>:<path>` (xtask).
pub const ANCHORS_REPO_PATH: &str = "crates/mklm-update/trust/anchors.txt";
/// The file of this build.
pub const ANCHORS_TEXT: &str = include_str!("../trust/anchors.txt");

/// Strict: comments (`#`) and blank lines skipped; `<role> <KEYID> <base64>` or `revoked <KEYID>`;
/// anything else is `BadAnchorsLine { line }` (1-based). Does not decode the keys.
pub fn parse_anchors(text: &str) -> Result<AnchorsFile, KeyError>;

/// The parsed keys a manifest may be signed with. `Debug` prints IDs and roles only.
pub struct TrustAnchors { /* private */ }
impl std::fmt::Debug for TrustAnchors { /* ids and roles */ }

impl TrustAnchors {
    /// `ANCHORS_TEXT` only, in every build profile (xtask's production commands, tests of the
    /// committed file). `NotConfigured` when the file has no key (OPS-UX-TEST-7).
    pub fn release() -> Result<TrustAnchors, KeyError>;
    /// What the GUI, the CLI and the helper verify with: `release()`; in development builds
    /// (`cfg(all(debug_assertions, mklm_update_dev))`) plus `MKLM_UPDATE_DEV_PUBKEY` as the
    /// development key (design m5b A.10).
    pub fn for_this_build() -> Result<TrustAnchors, KeyError>;
    /// A file read from another tag (xtask); the same checks.
    pub fn from_file(file: &AnchorsFile) -> Result<TrustAnchors, KeyError>;
    /// Tests: the same checks.
    pub fn from_keys(keys: &[(KeyRole, &str)], revoked: &[&str]) -> Result<TrustAnchors, KeyError>;
    pub fn ids(&self) -> Vec<(KeyId, KeyRole)>;
    pub fn role_of(&self, id: KeyId) -> Option<KeyRole>;
    pub fn fingerprint_of(&self, id: KeyId) -> Option<KeyFingerprint>;
    /// A `revoked` line of the file.
    pub fn revoked_by_build(&self, id: KeyId) -> bool;
    pub fn revoked_ids(&self) -> Vec<KeyId>;
    /// Always false outside development builds.
    pub fn is_dev_key(&self, id: KeyId) -> bool;
    /// `xtask check-keys`: exactly one primary and one backup key, none revoked. `Roles`.
    pub fn check_release_roles(&self) -> Result<(), KeyError>;
}

/// The key ID inside a minisign public key (base64 line), strict base64.
pub fn public_key_id(public_key_base64: &str) -> Result<KeyId, KeyError>;
pub fn public_key_fingerprint(public_key_base64: &str) -> Result<KeyFingerprint, KeyError>;
/// The key ID inside a minisign signature file (its second line), strict base64.
pub fn signature_key_id(signature_text: &str) -> Result<KeyId, KeyError>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("this build has no update keys")]
    NotConfigured,
    #[error("line {line} of the trust anchors file is malformed")]
    BadAnchorsLine { line: usize },
    #[error("key {index} is not a minisign public key")]
    BadPublicKey { index: usize },
    #[error("{text:?} is not a key ID")]
    BadKeyId { text: String },
    #[error("key {index}: its ID does not match the public key")]
    IdMismatch { index: usize },
    #[error("two keys have the same ID")]
    DuplicateId,
    #[error("the keys must be exactly one primary and one backup key, none of them revoked")]
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
    /// 1..=MAX_KEY_IDS distinct key IDs; the main signature's key first (SECURITY-4).
    pub key_ids: Vec<String>,
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
    /// GUI / CLI / RecordTrust: an older or equal version is `OfferKind::UpToDate`, not an error.
    Check,
    /// Helper: anything but `OfferKind::Newer` is refused.
    Install,
}

/// Which signature file verified (design m5b A.4, A.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureSlot {
    Main,
    Alt,
}
impl SignatureSlot {
    /// `SIGNATURE_NAME` / `ALT_SIGNATURE_NAME`.
    pub fn file_name(self) -> &'static str;
}

#[derive(Debug, Clone, Copy)]
pub struct VerifyInput<'a> {
    pub manifest: &'a [u8],
    /// One signature file (main or alternate).
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
    pub signer_fingerprint: KeyFingerprint,
    /// Signed by the development key (development builds only): records nothing.
    pub signer_is_dev: bool,
    pub key_ids: Vec<KeyId>,
    /// The revocations this build records: embedded primary keys other than the signer, with
    /// their fingerprints (design m5b B.2 rule 2). Unknown IDs, and the embedded backup key in a
    /// primary-signed manifest (ignored, FIX-VERIFICATION-1), are not here.
    pub revoked: std::collections::BTreeMap<KeyId, KeyFingerprint>,
    pub min_from_version: Option<Version>,
    pub asset: SelectedAsset,
    pub freshness: Freshness,
    pub offer: OfferKind,
}

/// Design m5b C.3, in that order. Never parses the manifest before the signature is verified.
pub fn verify_manifest(input: &VerifyInput<'_>) -> Result<VerifiedManifest, UpdateRefusal>;

/// True for `UnknownKey` and `RevokedKey` only: the caller may then fetch and try the alternate
/// signature (design m5b A.5, C.3).
pub fn tries_alternate(error: &UpdateRefusal) -> bool;

/// The helper's handling of `CallerMessage::RecordTrust` (design m5b C.4): verifies with
/// `Purpose::Check` against `machine`, and returns the merged record when it changes anything
/// (`Ok(None)`: verified, nothing new).
pub fn apply_trust_report(
    manifest: &[u8],
    signature: &[u8],
    anchors: &TrustAnchors,
    installed: &Version,
    arch: Arch,
    machine: &TrustState,
    now_unix: u64,
) -> Result<Option<TrustState>, UpdateRefusal>;
```

```rust
// crates/mklm-update/src/state.rs
/// One recorded revocation, scoped to the key it was aimed at (SECURITY-3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RevokedKey {
    pub key_id: String,
    /// `KeyFingerprint::to_hex`.
    pub fingerprint: String,
}

/// Revocations and the highest recorded `issued_at` per signing key (design m5b B.2, C.4). JSON;
/// unknown fields are ignored. `Default` has `schema = 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    pub schema: u32,
    /// Key ID text → highest `min(issued_at, time of recording)` seen from that key.
    #[serde(default)]
    pub max_issued_at: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub revoked: std::collections::BTreeSet<RevokedKey>,
}
impl Default for TrustState { /* schema 1, empty */ }

impl TrustState {
    pub fn parse(json: &str) -> Result<TrustState, StateError>;
    pub fn to_json(&self) -> String;
    /// Per key the higher value; the union of the revocations.
    pub fn merged(&self, other: &TrustState) -> TrustState;
    /// After a successful verification: the signer's maximum becomes
    /// `max(old, min(verified.issued_at, now_unix))`, and `verified.revoked` is added. A
    /// development-key manifest changes nothing (SECURITY-12, RELIABILITY-6).
    pub fn recorded(&self, verified: &VerifiedManifest, now_unix: u64) -> TrustState;
    /// A recorded revocation with this ID and fingerprint.
    pub fn is_revoked(&self, id: KeyId, fingerprint: KeyFingerprint) -> bool;
    pub fn max_issued_at(&self, id: KeyId) -> Option<u64>;
    /// The anti-rollback threshold (design m5b B.2): the highest recorded value over all keys
    /// except those revoked by the build, by this record (ID and, for keys `anchors` knows, the
    /// fingerprint; unknown keys by ID), or in `revoking` (the manifest being verified).
    pub fn rollback_threshold(
        &self,
        anchors: &TrustAnchors,
        revoking: &std::collections::BTreeSet<KeyId>,
    ) -> Option<u64>;
    /// A higher maximum for some key, or a revocation `other` lacks: the client then sends
    /// `RecordTrust` (design m5b C.4).
    pub fn is_ahead_of(&self, other: &TrustState) -> bool;
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
/// Serialized inside the pipe's `UpdateMessage::{Refused, TrustNotRecorded}` and the registry's
/// `LastResult`.
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
    /// Replaces the earlier `KeyIdMismatch` (key_id → key_ids, SECURITY-4).
    #[error("the manifest does not list its signing key {key_id}")]
    SignerNotListed { key_id: String },
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
    #[error("not enough disk space: {needed} bytes needed, {available} available")]
    DiskFull { needed: u64, available: u64 },
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
/// A URL that passed a `UrlPolicy` (design m5b A.6). Only https (development builds: http to
/// 127.0.0.1).
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
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn loopback() -> UrlPolicy;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints { /* private */ }
impl Endpoints {
    pub fn production() -> Endpoints;
    /// `base` = `http://127.0.0.1:<port>` standing in for `REPO_URL` (same paths below it).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn loopback(base: &str) -> Result<Endpoints, UrlError>;
    pub fn policy(&self) -> &UrlPolicy;
    /// `<base>/releases/latest/download/latest.json`
    pub fn manifest_url(&self) -> Url;
    /// `<base>/releases/download/<tag>/<slot file>`, or the latest/download one without a tag.
    pub fn signature_url(&self, tag: Option<&str>, slot: SignatureSlot) -> Url;
    /// `<base>/releases/latest/download/<name>` (fetch-smoke: `SHA256SUMS`).
    pub fn latest_file_url(&self, name: &str) -> Url;
    /// `<base>/releases/download/v<version>/<name>`
    pub fn asset_url(&self, version: &Version, name: &str) -> Url;
    /// `v<X.Y.Z>` when `location` is `<base>/releases/download/v<X.Y.Z>/<asset_name>`
    /// (OPS-UX-TEST-8: any asset name).
    pub fn tag_from_location(&self, location: &Url, asset_name: &str) -> Option<String>;
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
    /// 15 s / 15 s / 30 s / 30 s, total 60 s, 5 redirects (design m5b A.8). Also the alternate
    /// signature and fetch-smoke.
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
    /// Includes 401 (server authentication is never answered, SECURITY-13).
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
    /// The main signature.
    pub signature: Vec<u8>,
    pub tag: Option<String>,
}

/// latest.json, then its main signature (from the same tag when known). Design m5b A.5–A.8.
pub fn fetch_manifest(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<FetchedManifest, FetchError>;

/// The alternate signature of the same tag; `Ok(None)` on 404 (design m5b A.5).
pub fn fetch_alt_signature(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    tag: Option<&str>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Option<Vec<u8>>, FetchError>;

/// `releases/latest/download/<name>` of at most `max_len` bytes, and the tag of its first redirect
/// (xtask fetch-smoke, OPS-UX-TEST-8).
pub fn fetch_latest_file(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    name: &str,
    max_len: u64,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Option<String>), FetchError>;

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

/// H1's receiving state machine (design m5b D.4 steps 13–14). Pure.
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
    /// The NSIS process, written together with `phase = installing` while it is still suspended
    /// (RELIABILITY-4).
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

/// NSIS exit codes of the installer (design m5b D.9.1); mirrored by `!define MKLM_EXIT_*` in
/// mklm.nsi. 25 (`MKLM_EXIT_BAD_INSTALL_DIR`) is the uninstaller's only and is not here.
pub mod nsis_exit {
    pub const OS_TOO_OLD: u32 = 20;
    pub const WRONG_ARCH: u32 = 21;
    pub const HELPER_RUNNING: u32 = 22;
    pub const CLI_RUNNING: u32 = 23;
    pub const GUI_RUNNING: u32 = 24;
    pub const FILES_IN_USE: u32 = 26;
    pub const FILE_WRITE: u32 = 27;
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
    FilesInUse,    // 26
    FileWrite,     // 27
    Other(u32),    // 25 included
}
impl InstallerExit {
    /// 20..=24, 26, 27: NSIS guarantees nothing was replaced (renamed from `is_refusal`).
    pub fn leaves_old_files(self) -> bool;
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
    /// From `mklm_win::update_dir::read_build_ids` (order: gui, cli, helper). The one conversion
    /// H2 and `read_status` share (OPS-UX-TEST-12).
    pub fn from_build_ids(ids: [Option<String>; 3]) -> InstallState;
    /// The version part of the build ID when all three exist and are equal.
    pub fn consistent_version(&self) -> Option<Version>;
}

/// A process that kept the update from running (RED-TEAM-3): for the administrator, in the
/// technical details and `update --status`. No user SID (`LastResult` is readable by Users).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHolder {
    pub pid: u32,
    pub session_id: u32,
    /// Executable file name (Restart Manager's `strAppName` or the image's file name), at most
    /// 260 characters, control characters removed.
    pub name: String,
}

/// A running installed MKLM program (design m5b D.8 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProgram {
    pub identity: ProcessIdentity,
    pub kind: ProgramKind,
    pub session_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotInstalledReason {
    Refused(UpdateRefusal),
    CallerDidNotExit,
    /// The sessions whose MKLM answered `busy` (RED-TEAM-3).
    InstanceBusy {
        #[serde(default)]
        sessions: Vec<u32>,
    },
    ProgramsStillRunning {
        programs: Vec<ProgramKind>,
        #[serde(default)]
        holders: Vec<FileHolder>,
    },
    /// Opened by another process without delete sharing (SECURITY-10); `holders` from the
    /// Restart Manager, empty when it could not tell (RED-TEAM-3).
    FilesInUse {
        programs: Vec<ProgramKind>,
        #[serde(default)]
        holders: Vec<FileHolder>,
    },
    DiskFull { needed: u64, available: u64 },
    /// The session began to end before the installer was started (RELIABILITY-3).
    SessionEnding,
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

/// `Run` as the GUI, the CLI and the helper see it: `InProgress` when the owner of the phase is
/// alive in this boot — `stager` up to `staged`, `runner` for `ready` and `waiting`, `runner` OR
/// `installer` for `installing` and `finishing` (RELIABILITY-4); otherwise (boot changed, owners
/// dead or unknown) `Interrupted`. `Done` or no record → `Idle`.
pub fn classify_run(
    run: Option<&RunRecord>,
    current_boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> RunView;

/// The `LastResult` for an interrupted record (`UpdateOutcome::Interrupted`, the installed
/// version from `after`).
pub fn interrupted_result(record: &RunRecord, now: Timestamp, after: &InstallState) -> UpdateResult;

/// What another MKLM's single-instance pipe answered (mapped from `mklm_win::instance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAnswer {
    Quit,
    Busy,
    NotOurs,
    NoAnswer,
}

/// Free space the update needs (design m5b D.4 step 4, D.7 step 12).
pub mod space {
    pub const STAGING_MARGIN: u64 = 16 * 1024 * 1024;
    pub const INSTALL_FACTOR: u64 = 4;
    pub const INSTALL_MARGIN: u64 = 64 * 1024 * 1024;
}

/// Waits and deadlines of the update run (design m5b D.4, D.7, D.8, E.1–E.4).
pub mod timing {
    use std::time::Duration;
    pub const LOCK_WAIT_STAGER: Duration = Duration::from_secs(10);
    pub const CHUNK_WAIT: Duration = Duration::from_secs(30);
    pub const STAGE_TOTAL: Duration = Duration::from_secs(10 * 60);
    /// H1: H2 up to `ready` (RELIABILITY-5: was 20 s).
    pub const READY_WAIT: Duration = Duration::from_secs(120);
    /// H1: after TerminateProcess of H2, before deleting its folder.
    pub const RUNNER_KILL_WAIT: Duration = Duration::from_secs(5);
    pub const STAGER_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const LOCK_WAIT_RUNNER: Duration = Duration::from_secs(60);
    pub const CALLER_EXIT_WAIT: Duration = Duration::from_secs(30);
    /// One instance pipe (connect, send, reply).
    pub const INSTANCE_QUIT_WAIT: Duration = Duration::from_secs(5);
    /// All instance pipes together (SECURITY-7).
    pub const INSTANCES_TOTAL: Duration = Duration::from_secs(20);
    pub const MAX_INSTANCE_PIPES: usize = 16;
    pub const PROGRAMS_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const HELPERS_EXIT_WAIT: Duration = Duration::from_secs(75);
    /// D.8 step 4.
    pub const FILES_IN_USE_RETRY: Duration = Duration::from_secs(10);
    pub const INSTALLER_WAIT: Duration = Duration::from_secs(15 * 60);
    /// Total wait before H2 leaves `Run` behind (RELIABILITY-4).
    pub const INSTALLER_WAIT_MAX: Duration = Duration::from_secs(60 * 60);
    pub const RELAUNCH_WAIT: Duration = Duration::from_secs(10);
    /// The helper's `RecordTrust` lock wait.
    pub const TRUST_LOCK_WAIT: Duration = Duration::from_secs(2);
    /// Caller: after the last chunk, the longest wait for `HandedOff` (RELIABILITY-5: was 90 s).
    pub const HANDOFF_WAIT: Duration = Duration::from_secs(150);
    /// Caller: reading `Run` after losing the helper past the last chunk.
    pub const CALLER_RUN_POLL: Duration = Duration::from_secs(30);
    /// GUI: the hand-off overlay closes by itself after this long; [OK] closes it at once
    /// (OPS-UX-TEST-15; FIX-VERIFICATION-13 replaced the 5 s minimum). Must stay well below
    /// `CALLER_EXIT_WAIT` (tested: MAX + 2 s < CALLER_EXIT_WAIT).
    pub const HANDOFF_OVERLAY_MAX: Duration = Duration::from_secs(15);
    /// GUI: results older than this are marked seen silently (OPS-UX-TEST-6).
    pub const RESULT_SHOW_DAYS: u64 = 14;
    /// GUI: the "no successful check" / "expired" banner (SECURITY-11, OPS-UX-TEST-5).
    pub const STALE_NOTICE_DAYS: u64 = 30;
    /// GUI: NotFound becomes structural after this long.
    pub const NOT_FOUND_STRUCTURAL_DAYS: u64 = 7;
}
```

```rust
// crates/mklm-update/src/run_flow.rs  (WP-H; OPS-UX-TEST-10)
use std::time::Duration;
use mklm_core::{BootId, Journal, ProcessIdentity, Timestamp};

/// Everything H2 does to the machine (design m5b D.7). The helper implements it over mklm-win
/// (`apps/mklm-helper/src/run_update.rs`); tests use fakes. Errors are English diagnostics.
pub trait RunnerEnv {
    // Who and where
    fn version(&self) -> &Version;
    fn arch(&self) -> Arch;
    fn build_id(&self) -> &str;
    fn anchors(&self) -> &TrustAnchors;
    fn me(&self) -> ProcessIdentity;
    fn boot_id(&self) -> BootId;
    fn now(&self) -> Timestamp;
    fn now_unix(&self) -> u64;
    /// Step 2: this image is `Updates\<run_id>\mklm-update-runner.exe`, every level verified and pinned.
    fn is_runner_of(&mut self, run_id: &RunId) -> Result<bool, String>;
    // Records (HKLM Update)
    fn read_run(&mut self) -> Result<Option<RunRecord>, String>;
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String>;
    fn delete_run(&mut self) -> Result<(), String>;
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String>;
    fn read_trust(&mut self) -> TrustState;
    // The run folder
    /// Step 4: `open_locked` (FILE_SHARE_READ only), kept until the installer is created.
    fn lock_installer(&mut self, name: &str, max_len: u64) -> Result<(), String>;
    fn read_staged_manifest(&mut self) -> Result<(Vec<u8>, Vec<u8>), String>;
    /// Through the locked handle.
    fn hash_locked_installer(&mut self) -> Result<(u64, Sha256Digest), String>;
    fn cleanup_own_run_dir(&mut self);
    fn sweep_other_run_dirs(&mut self);
    // Waits, lock, journal
    fn wait_for_exit(&mut self, process: &ProcessIdentity, timeout: Duration) -> bool;
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal>;
    fn release_lock(&mut self);
    fn read_journal(&mut self) -> Result<Journal, String>;
    // The installed copy
    fn installed_helper_build_id(&mut self) -> Result<Option<String>, String>;
    fn free_space_program_files(&mut self) -> Result<u64, String>;
    fn read_install_state(&mut self) -> InstallState;
    // Other MKLM programs (D.8)
    /// Each GUI's answer with the session of its pipe (FIX-VERIFICATION-5: names computed from
    /// the GUI processes, never enumerated).
    fn quit_idle_instances(&mut self) -> Vec<(InstanceAnswer, u32)>;
    fn running_programs(&mut self) -> Vec<RunningProgram>;
    /// Retried for `FILES_IN_USE_RETRY`; the programs whose file stays in use, and who holds them
    /// (Restart Manager; empty when unknown. RED-TEAM-3).
    fn files_in_use(&mut self) -> (Vec<ProgramKind>, Vec<FileHolder>);
    // Session end (RELIABILITY-3)
    /// Atomically: if the session end has not begun, answer later WM_QUERYENDSESSION with "block"
    /// and return true; else false.
    fn claim_installing(&mut self) -> bool;
    fn set_block_reason(&mut self, on: bool);
    fn release_installing(&mut self);
    // The installer (RELIABILITY-4, SECURITY-6)
    /// Suspended, clean environment, current directory System32. `Err(Win32 code)`.
    fn spawn_installer_suspended(&mut self) -> Result<ProcessIdentity, u32>;
    fn terminate_suspended_installer(&mut self);
    /// Closes the step-4 handle, then `ResumeThread`.
    fn resume_installer(&mut self) -> Result<(), String>;
    /// The exit code, or `None` when still running after `timeout`.
    fn wait_installer(&mut self, timeout: Duration) -> Option<u32>;
    // The end
    fn relaunch_gui(&mut self) -> Result<(), String>;
    fn log(&mut self, line: &str);
}

/// Design m5b D.7 steps 2–22 (step 1 is the caller's). Returns the process exit code (0 / 7).
pub fn run_update(env: &mut dyn RunnerEnv, run_id: &RunId) -> u32;
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
    /// Automatic proxy, autologon HIGH, no credentials (design m5b A.7).
    pub fn new(user_agent: &str) -> Result<WinHttpTransport, TransportError>;
    /// No proxy, for the loopback tests and the rehearsal.
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn new_without_proxy(user_agent: &str) -> Result<WinHttpTransport, TransportError>;
}
impl Transport for WinHttpTransport { /* … */ }
```

### H.2 `mklm-ipc`

```rust
// crates/mklm-ipc/src/lib.rs
/// 3 (M5b, design m5b D.3): `CallerMessage::{RecordTrust, StageUpdate, InstallerChunk}`,
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
    /// Only as the first message after `Welcome` (design m5b C.4, D.3; SECURITY-5).
    RecordTrust(TrustReport),
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
//  deny_unknown_fields)] unchanged: "record-trust", "stage-update", "installer-chunk", "update")
```

```rust
// crates/mklm-ipc/src/update.rs
pub use mklm_update::stage::CHUNK_LEN;
pub use mklm_update::UpdateRefusal;

/// The client's newest verified manifest and the signature that verified it (design m5b C.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustReport {
    pub manifest: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageUpdateRequest {
    /// latest.json exactly as downloaded.
    pub manifest: String,
    /// The signature file (main or alternate) that verified, exactly as downloaded.
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
    /// Answer to `RecordTrust`; the session goes on either way.
    TrustRecorded { changed: bool },
    TrustNotRecorded(UpdateRefusal),
    SendInstaller { name: String, size: u64, sha256: String, chunk_len: u32 },
    Received { bytes: u64 },
    /// H2 was started; waiting for it to be ready (up to `READY_WAIT`; RELIABILITY-5).
    StartingRunner,
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
use mklm_core::{BootId, Journal, Liveness, ProcessIdentity, Timestamp};
use mklm_update::run::{InstallState, RunId, RunRecord, UpdateResult};
use mklm_update::stage::Stager;
use mklm_update::{Arch, Sha256Digest, TrustAnchors, TrustState, Version};

pub trait StageLink {
    fn send(&mut self, message: HelperMessage) -> Result<(), String>;
    fn recv(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError>;
}

pub trait StageSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// The installer file being received: `commit` flushes and closes it; dropped uncommitted, it is
/// deleted.
pub trait StagedInstaller: StageSink {
    fn commit(self: Box<Self>) -> Result<(), String>;
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
/// `Received` every `progress_every` bytes (design m5b D.4 steps 13–14).
pub fn receive_installer(
    link: &mut dyn StageLink,
    sink: &mut dyn StageSink,
    stager: Stager,
    chunk_wait: Duration,
    total: Duration,
    progress_every: u64,
) -> StagingEnd;

/// Everything H1 does to the machine (design m5b D.4). The helper implements it over mklm-win
/// (`apps/mklm-helper/src/update.rs`); tests use fakes (OPS-UX-TEST-10).
pub trait StagerEnv {
    fn version(&self) -> &Version;
    fn arch(&self) -> Arch;
    fn anchors(&self) -> &TrustAnchors;
    fn me(&self) -> ProcessIdentity;
    fn boot_id(&self) -> BootId;
    fn now(&self) -> Timestamp;
    fn now_unix(&self) -> u64;
    /// Step 1.
    fn runs_from_install_dir(&mut self) -> bool;
    /// The pipe server with its creation time, and its session.
    fn caller(&mut self) -> (Option<ProcessIdentity>, Option<u32>);
    fn free_space_program_data(&mut self) -> Result<u64, String>;
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal>;
    fn release_lock(&mut self);
    fn read_journal(&mut self) -> Result<Journal, String>;
    fn liveness(&self, process: &ProcessIdentity) -> Liveness;
    fn read_install_state(&mut self) -> InstallState;
    /// A corrupted value reads as empty (logged).
    fn read_trust(&mut self) -> TrustState;
    fn write_trust(&mut self, trust: &TrustState) -> Result<(), String>;
    fn read_run(&mut self) -> Result<Option<RunRecord>, String>;
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String>;
    fn delete_run(&mut self) -> Result<(), String>;
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String>;
    fn sweep_run_dirs(&mut self, keep: Option<&RunId>);
    fn random_suffix(&mut self) -> [u8; 8];
    fn create_run_dir(&mut self, run_id: &RunId) -> Result<(), String>;
    fn remove_run_dir(&mut self, run_id: &RunId);
    fn write_run_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
    fn create_installer(&mut self, name: &str) -> Result<Box<dyn StagedInstaller>, String>;
    /// `$INSTDIR\mklm-helper.exe` → `<run dir>\RUNNER_EXE_NAME`, SHA-256 compared.
    fn copy_self_as_runner(&mut self) -> Result<(), String>;
    /// `<run dir>\tmp` with `PRIVATE_DIR_SDDL`.
    fn create_tmp_dir(&mut self) -> Result<(), String>;
    /// Clean environment, current directory System32 (SECURITY-6).
    fn spawn_runner(&mut self, run_id: &RunId) -> Result<ProcessIdentity, String>;
    /// `Some(exit code)` once the runner has exited.
    fn runner_exit_code(&mut self) -> Option<u32>;
    /// `TerminateProcess`, then waits for its handle up to `wait` (RELIABILITY-5).
    fn stop_runner(&mut self, wait: Duration);
    fn sleep(&mut self, duration: Duration);
    fn log(&mut self, line: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageFlowEnd {
    /// `HandedOff` was sent; the helper exits 0.
    HandedOff { run_id: RunId },
    /// Already sent as `UpdateMessage::Refused`; the session goes on.
    Refused(UpdateRefusal),
    CallerLeft,
}

/// Design m5b D.4 steps 1–20 for one `StageUpdate`. The helper sets its heartbeat flag around
/// the call (D.3).
pub fn stage_update(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    request: &StageUpdateRequest,
) -> StageFlowEnd;

/// The helper's handling of `CallerMessage::RecordTrust` (design m5b C.4; SECURITY-5,
/// FIX-VERIFICATION-6): `acquire_lock(TRUST_LOCK_WAIT)` (else `TrustNotRecorded(Busy)`), reads
/// the machine record again, `apply_trust_report` against it with `env.anchors()`,
/// `env.version()`, `env.arch()`, `env.now_unix()`, writes it when changed, releases the lock.
/// Returns the reply to send; the session goes on whatever it is.
pub fn record_trust(env: &mut dyn StagerEnv, report: &TrustReport) -> UpdateMessage;

/// The order rule of the caller's messages the helper session enforces (design m5b D.3):
/// `RecordTrust` only as the first message after `Welcome`. The helper's `serve` loop feeds every
/// message after `Welcome` through it; `Err` is a protocol violation (`HelperMessage::Error`,
/// then disconnect).
#[derive(Debug, Default)]
pub struct CallerOrder { /* private */ }
impl CallerOrder {
    pub fn new() -> CallerOrder;
    pub fn check(&mut self, message: &CallerMessage) -> Result<(), &'static str>;
}
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
    /// false only in development builds: to 127.0.0.1, or to a `.invalid` host through an
    /// `open_named_proxy` session (F.3); refused otherwise.
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
    /// no decompression, `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH; never `WinHttpSetCredentials`
    /// (design m5b A.7; SECURITY-13).
    pub fn open(user_agent: &str) -> Result<HttpSession, NetError>;
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_direct(user_agent: &str) -> Result<HttpSession, NetError>;
    /// Tests only (F.3 proxy authentication; FIX-VERIFICATION-4): `WINHTTP_ACCESS_TYPE_NAMED_PROXY`
    /// with `proxy` = `127.0.0.1:<port>` (anything else refused), no bypass list, the other
    /// options as `open`, and the given autologon level (`Low` only for the control case).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_named_proxy(
        user_agent: &str,
        proxy: &str,
        autologon: AutologonLevel,
    ) -> Result<HttpSession, NetError>;
    pub fn get(&self, request: &HttpGet<'_>) -> Result<HttpResponse<'_>, NetError>;
}

#[cfg(all(debug_assertions, mklm_update_dev))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutologonLevel {
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH` (what `open` always uses).
    High,
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW`: the F.3 control case only.
    Low,
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
    /// A subfolder (`tmp`) with `PRIVATE_DIR_SDDL`, validated and pinned like the run folder.
    pub fn create_private_subdir(&self, name: &str) -> Result<std::path::PathBuf, Error>;
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
    /// Back to offset 0 (to hash, then keep the handle).
    pub fn rewind(&mut self) -> Result<(), Error>;
}

pub fn list_run_dirs(updates: &ProtectedDir) -> Result<Vec<String>, Error>;
/// Regular files only, never following reparse points.
pub fn remove_run_dir(updates: &ProtectedDir, name: &str) -> Result<(), Error>;
/// Every run folder except `keep` whose `mklm-update-runner.exe` is not running; the names
/// removed (design m5b D.11; replaces `remove_at_reboot`, RELIABILITY-9).
pub fn sweep_stale_run_dirs(updates: &ProtectedDir, keep: Option<&str>) -> Result<Vec<String>, Error>;
/// Build IDs of `mklm.exe`, `mklm-cli.exe`, `mklm-helper.exe` in `install_dir` (in that order;
/// `None` when missing or unreadable). Callers convert with `InstallState::from_build_ids`.
pub fn read_build_ids(install_dir: &std::path::Path) -> [Option<String>; 3];
/// Indices into `names` of the files in `install_dir` that cannot be opened with `DELETE` and
/// full sharing (another handle lacks FILE_SHARE_DELETE). Missing files are not in use.
pub fn files_in_use(install_dir: &std::path::Path, names: &[&str]) -> Result<Vec<usize>, Error>;
/// Who holds `paths` open (Restart Manager: `RmStartSession`, `RmRegisterResources`, `RmGetList`,
/// `RmEndSession`): (PID, session ID, application name) each (RED-TEAM-3). Best effort; the
/// callers treat an error as "unknown". Whether `RmGetList` reports plain open handles (not only
/// loaded images) is unverified (design m5b I.16).
pub fn file_holders(paths: &[std::path::PathBuf]) -> Result<Vec<(u32, u32, String)>, Error>;
/// `GetDiskFreeSpaceExW` for the caller: free bytes on the volume of `path`.
pub fn free_space(path: &std::path::Path) -> Result<u64, Error>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProcess {
    pub identity: ProcessIdentity,
    /// Index into the `nt_paths` argument.
    pub path_index: usize,
    pub session_id: u32,
}
/// Every process whose image NT path equals one of `nt_paths` (case-insensitive).
pub fn processes_with_images(nt_paths: &[String]) -> Result<Vec<ImageProcess>, Error>;

/// One row of `WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessUser {
    pub session_id: u32,
    /// `pUserSid` as `S-1-…` text (`ConvertSidToStringSidW`); `None` when absent.
    pub user_sid: Option<String>,
}
/// PID → session and user SID of every process (design m5b D.8 step 2; FIX-VERIFICATION-5).
/// Whether `pUserSid` is filled for other users' processes when called from an elevated,
/// non-SYSTEM process is unverified (design m5b I.12).
pub fn process_users() -> Result<std::collections::HashMap<u32, ProcessUser>, Error>;
/// Polls `process_liveness` every 250 ms; true when all are gone within `timeout`.
pub fn wait_for_exit(processes: &[ProcessIdentity], timeout: std::time::Duration) -> Result<bool, Error>;
```

```rust
// crates/mklm-win/src/instance.rs (additions, WP-H; the GUI side is WP-C)
pub enum InstanceCommand {
    Activate,
    Quit,
    /// `quit-if-idle\n` (13 bytes): quit only when nothing is going on, else answer `busy` and
    /// change nothing (design m5b D.8, E.4.1; RELIABILITY-1, OPS-UX-TEST-4). The updater's only
    /// command.
    QuitIfIdle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAnswer {
    Ok,
    Busy,
    /// The pipe's server is not `gui_nt_path` in the pipe's session: nothing was sent.
    NotOurs,
    NoAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceQuit {
    pub pipe: String,
    /// The session in the pipe's name.
    pub session_id: u32,
    pub server_pid: Option<u32>,
    pub answer: QuitAnswer,
}

/// `SHINDATACENTER.MKLM.Instance.<session>.<SID>` → (session, SID) when the whole name matches
/// `^SHINDATACENTER\.MKLM\.Instance\.([0-9]{1,10})\.(S-1-5-21(-[0-9]{1,10}){4}|S-1-12-1(-[0-9]{1,10}){4})$`
/// (SECURITY-7). Anything else: `None`, and the pipe is never opened.
pub fn parse_instance_pipe_name(name: &str) -> Option<(u32, String)>;

/// Pure (FIX-VERIFICATION-5): for each GUI process (from `processes_with_images`) whose
/// `process_users` row has a SID and the same session, `instance_pipe_path(session, sid)` —
/// kept only if its name also passes `parse_instance_pipe_name`; at most `max_pipes`, in input
/// order. Returns (pipe path, the GUI process).
pub fn instance_pipe_candidates(
    guis: &[crate::proc_identity::ImageProcess],
    users: &std::collections::HashMap<u32, crate::proc_identity::ProcessUser>,
    max_pipes: usize,
) -> Vec<(String, crate::proc_identity::ImageProcess)>;

/// Design m5b D.8 step 2: finds the processes running `gui_nt_path`, computes their pipe names
/// with `instance_pipe_candidates` (never enumerates `\\.\pipe\`), opens each with
/// `pipe::open_client` (SQOS identification, overlapped I/O), `per_pipe` each and `total` for all;
/// the server must be that GUI's PID, pinned by a handle (or compared by identity before and
/// after), running `gui_nt_path` in the pipe's session; sends `quit-if-idle` only.
pub fn quit_idle_instances(
    gui_nt_path: &str,
    per_pipe: std::time::Duration,
    total: std::time::Duration,
    max_pipes: usize,
) -> Result<Vec<InstanceQuit>, Error>;
```

`pipe::check_pipe_path`（非公開）は、`\`、NUL に加えて `/`、名前全体が `.` か `..`、ASCII 以外、制御文字を拒否する（SECURITY-7）。

```rust
// crates/mklm-win/src/elevation.rs (additions, WP-H; SECURITY-6)
/// Name/value pairs of an explicit environment block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanEnvironment {
    pub vars: Vec<(String, String)>,
}

/// SystemRoot, windir, SystemDrive, ComSpec, PATH (System32; Windows; System32\Wbem),
/// ProgramData, ProgramFiles, ProgramW6432 from the system, and TEMP = TMP = `temp_dir`
/// (design m5b D.9.4). Nothing from this process's environment.
pub fn runner_environment(temp_dir: &std::path::Path) -> Result<CleanEnvironment, Error>;

/// `CREATE_UNICODE_ENVIRONMENT` block: sorted case-insensitively by name, `name=value\0` each,
/// then `\0`. Names containing `=` or NUL, and values containing NUL, are refused. Pure.
pub fn environment_block(vars: &[(String, String)]) -> Result<Vec<u16>, Error>;

/// A process started by `spawn_clean`.
#[derive(Debug)]
pub struct SpawnedProcess {
    pub process: ElevatedProcess,
    /// The primary thread, kept only when started suspended.
    pub thread: Option<OwnedHandle>,
}
impl SpawnedProcess {
    pub fn identity(&self) -> Result<ProcessIdentity, Error>;
    pub fn resume(&mut self) -> Result<(), Error>;
    pub fn terminate(&self, exit_code: u32) -> Result<(), Error>;
}

/// `CreateProcessW` of the absolute `exe` (regular file, not a reparse point) with `parameters`,
/// `env` as the whole environment, current directory System32, `CREATE_NO_WINDOW`, optionally
/// `CREATE_SUSPENDED`; nothing inherited. No UAC (the caller is elevated).
pub fn spawn_clean(
    exe: &std::path::Path,
    parameters: &str,
    env: &CleanEnvironment,
    suspended: bool,
) -> Result<SpawnedProcess, Error>;
```

```rust
// crates/mklm-win/src/session_end.rs (additions, WP-H; RELIABILITY-3)
/// H2: the highest application level, so that it hears of the session end first.
pub const RUNNER_SHUTDOWN_LEVEL: u32 = 0x3FF;
/// `SetProcessShutdownParameters(RUNNER_SHUTDOWN_LEVEL, SHUTDOWN_NORETRY)`.
pub fn shut_down_first() -> Result<(), Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryAnswer {
    Allow,
    Block,
}

impl SessionEndWindow {
    /// Like `spawn`, but the handler's answer to `QueryEndSession` is returned to Windows.
    pub fn spawn_with_answer(
        handler: impl Fn(SessionEndEvent) -> QueryAnswer + Send + 'static,
    ) -> Result<SessionEndWindow, Error>;
    /// `ShutdownBlockReasonCreate` / `Destroy` on the window's own thread (posted to it).
    pub fn set_block_reason(&self, reason: Option<&str>) -> Result<(), Error>;
}
```

```rust
// crates/mklm-win/src/shell_launch.rs  (WP-H)
/// H2's COM set-up, before any other COM call: `CoInitializeEx(COINIT_MULTITHREADED)` and
/// `CoInitializeSecurity` with `RPC_C_IMP_LEVEL_IDENTIFY` and
/// `EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA` (SECURITY-8). Uninitializes on drop.
#[derive(Debug)]
pub struct RunnerCom { /* private */ }
pub fn init_com_for_runner() -> Result<RunnerCom, Error>;

/// Starts `exe arguments` through the desktop shell of this session (IShellWindows →
/// IShellDispatch2::ShellExecute): as the session's interactive user, unelevated. Gives up after
/// `timeout`. Never falls back to this process's own token.
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
/// `ShellExecuteExW` with `runas` of an installer in the user's update cache, on the user's button
/// press only (design m5b D.13; RELIABILITY-2). The caller's size and SHA-256 check only catches a
/// corrupted file: the signed-in user can swap the file afterwards, so this path is no more
/// trusted than running a downloaded installer by hand (RED-TEAM-2). `Error::Cancelled` when UAC
/// is declined.
pub fn run_installer_interactive(installer: &std::path::Path) -> Result<(), Error>;
```

既存のものを使う: `elevation::{file_build_id, spawn_from_elevated, is_elevated, system_directory, process_command_line}`、`protected_dir::{ensure_protected_dir, verify_protected_dir, DataDir::{Base, Updates, Logs}, FileLock, PRIVATE_DIR_SDDL}`、`journal_store::{read_journal_store, JOURNAL_KEY_SDDL}`、`proc_identity::{process_liveness, process_image_nt_path, current_process_identity}`、`session::{boot_id, random_bytes, new_uuid}`、`session_end::{SessionEndWindow, SessionEndEvent}`、`pipe::open_client`。

レビュー前にあった `update_dir::remove_at_reboot` は削除した（RELIABILITY-9）。

### H.4 `mklm-client`

```rust
// crates/mklm-client/src/update/mod.rs
pub mod cache;
pub mod check;
pub mod classify;
pub mod download;
pub mod stage;
pub mod trust_report;
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
    /// `TrustAnchors::for_this_build()`.
    pub anchors: Option<TrustAnchors>,
    pub endpoints: Endpoints,
    pub install_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// `app_version` = the front end's `CARGO_PKG_VERSION`; `endpoints` = `Endpoints::production()`
/// (development builds: maybe the `--update-endpoint` override).
pub fn environment(app_version: &str, endpoints: Endpoints) -> UpdateEnv;
```

```rust
// update/cache.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    Transient,
    Structural,
}

/// The last failed check (design m5b E.3; OPS-UX-TEST-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub class: ErrorClass,
    /// The message ID of design m5b E.6 (e.g. "upd-gh-changed").
    pub message_id: String,
    /// When this run of failures began (Unix seconds).
    pub first_at: u64,
    pub at: u64,
}

/// The last ignored older manifest (design m5b E.3; SECURITY-11, RELIABILITY-6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackNote {
    pub issued_at: u64,
    pub seen: u64,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientState {
    pub schema: u32, // 1
    #[serde(default)]
    pub trust: TrustState,
    #[serde(default)]
    pub last_check: Option<u64>,
    #[serde(default)]
    pub last_success: Option<u64>,
    #[serde(default)]
    pub last_failure: Option<CheckFailure>,
    #[serde(default)]
    pub last_rollback: Option<RollbackNote>,
    /// Which signature the cached `latest.json.minisig` is.
    #[serde(default)]
    pub cached_signature: Option<SignatureSlot>,
}

#[derive(Debug, Clone)]
pub struct UpdateCache { /* dir */ }
impl UpdateCache {
    pub fn new(dir: PathBuf) -> UpdateCache;
    /// Missing or unreadable → default.
    pub fn load_state(&self) -> ClientState;
    /// Temporary file, then rename.
    pub fn save_state(&self, state: &ClientState) -> std::io::Result<()>;
    /// The manifest and the signature that verified it.
    pub fn store_manifest(&self, manifest: &[u8], signature: &[u8]) -> std::io::Result<()>;
    pub fn load_manifest(&self) -> Option<(Vec<u8>, Vec<u8>)>;
    pub fn installer_path(&self, name: &str) -> PathBuf;
    pub fn partial_path(&self, name: &str) -> PathBuf;
    /// Deletes every installer and `.part` except `keep_installer`. The GUI calls it only under
    /// the conditions of design m5b D.11.
    pub fn prune(&self, keep_installer: Option<&str>) -> std::io::Result<()>;
}
```

```rust
// update/classify.rs
/// Design m5b E.6: the class and message ID of a check failure. `not_found_days` = days since the
/// run of failures began (NotFound turns structural after `NOT_FOUND_STRUCTURAL_DAYS`). `None`
/// for what is not a failure (both `Cancelled`s, `Unavailable(NotConfigured)`,
/// `Unavailable(NotInstalledCopy)`): not recorded, `last_check` unchanged. `Rollback` is
/// `Structural` (FIX-VERIFICATION-12).
pub fn classify(error: &CheckError, not_found_days: u64) -> Option<(ErrorClass, &'static str)>;
```

```rust
// update/check.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub verified: VerifiedManifest,
    pub manifest: Vec<u8>,
    /// The signature that verified (main or alternate).
    pub signature: Vec<u8>,
    pub slot: SignatureSlot,
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

/// Fetch, verify (`Purpose::Check`, machine ∪ user state; the alternate signature when
/// `tries_alternate`), record the user state (including `last_failure` and `last_rollback`),
/// cache the manifest. Never downloads the installer.
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
    /// `UpdateMessage::StartingRunner` arrived.
    fn starting_runner(&mut self);
    /// Honoured until the last chunk is sent.
    fn cancel_requested(&mut self) -> bool;
}

/// Asks the machine record whether this process was handed off although `HandedOff` was lost
/// (design m5b E.4; RELIABILITY-5): `Run.caller` is this process and `phase` is ready or waiting
/// with a live runner. `status::RunRecordProbe` reads HKLM.
pub trait HandOffProbe {
    fn handed_off(&mut self) -> Option<(String, String)>; // (run_id, to_version)
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
/// (design m5b D.3), then waits up to `HANDOFF_WAIT`; after the last chunk a lost helper is
/// checked against `probe` for up to `CALLER_RUN_POLL`. Sends `Bye` itself except after
/// `HandedOff`.
pub fn stage(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    frontend: &mut dyn StageFrontend,
    probe: &mut dyn HandOffProbe,
) -> StageEnd;
```

```rust
// update/trust_report.rs
/// The `RecordTrust` to send after `Welcome`, if any (design m5b C.4): the cached verified
/// manifest and its signature when the user record `is_ahead_of` the machine record.
pub fn pending_trust_report(cache: &UpdateCache, machine: &TrustState) -> Option<TrustReport>;
```

```rust
// update/status.rs (Windows)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub machine_trust: TrustState,
    pub run: RunView,
    pub last_result: Option<UpdateResult>,
    /// `InstallState::from_build_ids(update_dir::read_build_ids(install_dir))`.
    pub install: InstallState,
    pub client: ClientState,
    /// English diagnostics of what could not be read (the fields then hold defaults).
    pub warnings: Vec<String>,
}

/// Unelevated: `read_update_store`, `classify_run` (boot ID, process liveness), the build IDs of
/// the three executables in `install_dir`, the user cache.
pub fn read_status(install_dir: &Path, cache: &UpdateCache) -> UpdateStatus;

/// `HandOffProbe` over `read_update_store` for this process (`current_process_identity`).
#[derive(Debug)]
pub struct RunRecordProbe { /* private */ }
impl RunRecordProbe {
    pub fn new() -> RunRecordProbe;
}
impl HandOffProbe for RunRecordProbe { /* … */ }
```

**GUI の中の名前**（WP-C の中のことだが、E.4.1 の表と F.4 のテストのために固定する。OPS-UX-TEST-12）

| 名前 | 場所 | 中身 |
|---|---|---|
| `SessionPhase::Updating { id: SessionId, stage: UpdateStage }` | `apps/mklm/src/state.rs` | 更新のセッション（接続の後）。UAC の間は今の `SessionPhase::Launching { id }` を使い、下の `session_purpose` で区別する |
| `SessionPurpose::{Change, Update}`、`AppState::session_purpose: Option<SessionPurpose>` | 同上 | `Launching` にするのと同時に書き、セッションが終わったら `None`。`Notice::Connected` で `Update` なら `Updating { stage: Sending }`、`Change` なら `Running` へ（E.4.1。FIX-VERIFICATION-11）。名前を `SessionKind` にしないのは、`mklm_client::session::SessionKind { Request, Recovery }`（`Notice::Connected` の中身）とぶつかるため |
| `UpdateStage::{Sending { sent: u64, total: u64 }, StartingRunner, HandedOff}` | 同上 | E.4.1 の列 |
| `UacNoticeOrigin::{Change, Update}`（`AppState::uac_origin`） | 同上 | UAC の説明の画面の［続ける］と［キャンセル］の行き先（E.4） |
| `OverlayKind::UpdateHandOff` | 同上 | 引き継ぎのオーバーレイ（［OK］か `HANDOFF_OVERLAY_MAX` の 15 秒で閉じる。E.4） |
| `state::quit_if_idle(&mut AppState, received: Instant) -> (Vec<Effect>, InstanceReply)` | 同上 | E.4.1 の `quit-if-idle` の決まり |
| `Settings.update.{auto_check, skipped_version, result_seen, started_run, closed_by_update, stale_notice_at, rollback_notice_for}` | `apps/mklm/src/settings.rs` | E.1 |

### H.5 コマンドライン、ファイル、レジストリ、JSON

| もの | 形 |
|---|---|
| H2 のコマンドライン | `"<ProgramData>\SHIN DATA CENTER\MKLM\Updates\<run-id>\mklm-update-runner.exe" --run-update <run-id>`（`RUN_UPDATE_PATTERN`）。環境は `runner_environment` だけ、作業フォルダーは System32 |
| NSIS のコマンドライン | `"<run dir>\MKLM-Setup-<v>-<arch>.exe" /S`（同じ環境、一時停止で作ってから再開） |
| GUI の引数 | `--after-update`（新しい起動の形。表示する結果があれば窓を出して前面に。なければ `--tray` と同じ。`--post-reboot` と同じ優先順位の扱い）。開発用の cfg のデバッグ ビルドだけ `--update-endpoint=http://127.0.0.1:<port>` |
| 多重起動のコマンド | `activate\n`、`quit\n`（インストーラーとアンインストーラーの `--quit` だけ）、`quit-if-idle\n`（H2 だけ）。返事は `ok\n` / `busy\n` |
| RunOnce | `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce\SHINDATACENTER.MKLM.AfterUpdate` = `"<INSTDIR>\mklm.exe" --after-update`（引き継いだ GUI と、`quit-if-idle` で終わる GUI が登録する） |
| 機械のフォルダー | `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>\{latest.json, latest.json.minisig, MKLM-Setup-<v>-<arch>.exe, mklm-update-runner.exe, tmp\}`、`…\MKLM\logs\update.log` |
| インストール先の一時ファイル | `$INSTDIR\{mklm,mklm-cli,mklm-helper}.exe.new` と `.old`（2 段階の置き換えの間だけ。D.9.2） |
| 利用者のフォルダー | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\{state.json, latest.json, latest.json.minisig, MKLM-Setup-<v>-<arch>.exe[.part]}` |
| レジストリ | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` の `Trust`、`Run`、`LastResult`（REG_SZ、JSON） |
| 設定 | `settings.toml` の `[update]`: `auto_check`、`skipped_version`、`result_seen`、`started_run`、`closed_by_update`、`stale_notice_at`、`rollback_notice_for` |
| 信頼の起点ファイル | `crates/mklm-update/trust/anchors.txt`（B.2） |
| リリースのファイル | `MKLM-Setup-<v>-{x64,arm64}.exe`、`SHA256SUMS`、`latest.json`、`latest.json.minisig`、（移行の間）`latest.json.alt.minisig` |

JSON の例（形を固定するテストの期待値に使う）:

```json
// Trust
{"schema":1,"max_issued_at":{"8F1A2B3C4D5E6F70":1792022400},"revoked":[{"key_id":"1111222233334444","fingerprint":"5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592"}]}
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
// LastResult: not installed, another user's MKLM (session 2) was busy
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"instance-busy","sessions":[2]}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// LastResult: not installed, H2 found mklm.exe held open (RED-TEAM-3)
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"files-in-use","programs":["gui"],"holders":[{"pid":7120,"session_id":2,"name":"powershell.exe"}]}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// LastResult: not installed, a file was held open (the installer's swap failed)
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"installer-refused","exit":"files-in-use"}},"installer_exit":26,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// LastResult: refused on re-verification (H2)
{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"refused","code":"operation-open","waiting_for_reboot":true}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}
```

```json
// pipe frames (PROTOCOL_VERSION 3)
{"v":3,"seq":2,"body":{"type":"record-trust","data":{"manifest":"{\n  \"schema\": 1, …","signature":"untrusted comment: …"}}}
{"v":3,"seq":2,"body":{"type":"update","data":{"kind":"trust-recorded","data":{"changed":true}}}}
{"v":3,"seq":3,"body":{"type":"stage-update","data":{"manifest":"{\n  \"schema\": 1, …","signature":"untrusted comment: …"}}}
{"v":3,"seq":3,"body":{"type":"update","data":{"kind":"send-installer","data":{"name":"MKLM-Setup-0.2.1-x64.exe","size":6291456,"sha256":"3a7b…","chunk_len":65536}}}}
{"v":3,"seq":4,"body":{"type":"installer-chunk","data":{"offset":0,"hex":"4d5a9000…"}}}
{"v":3,"seq":9,"body":{"type":"update","data":{"kind":"starting-runner"}}}
{"v":3,"seq":10,"body":{"type":"update","data":{"kind":"handed-off","data":{"run_id":"0.2.1-3f9a0c2b7d1e4a65","to_version":"0.2.1"}}}}
{"v":3,"seq":3,"body":{"type":"update","data":{"kind":"refused","data":{"code":"rollback","issued_at":1792022400,"seen":1792108800}}}}
```

`InstallerExit::Other(u32)` は serde で `{"other":25}` になる（`rename_all = "kebab-case"` の外部タグ）。形を固定するテストに含める。

---

## I. 未解決の問題

1. **`CreateProcessW` と読み取り共有のハンドル**（D.7 の 4、16）: `FILE_SHARE_READ` だけで開いたままのインストーラーを `CreateProcessW`（一時停止）で起動できるはず（ローダーは読み取りと実行の共有で開き、実行は共有の検査では読み取りとして扱われる）だが、実機で確かめていない。できなければ、ハンドルを閉じてから起動し、起動したプロセスのイメージのパスとファイルの ID（`GetFileInformationByHandleEx(FileIdInfo)`）が検証したものと一致することを確かめる方式に変える。F.6 で確かめる。
2. **別の管理者で昇格したときの起動し直し**（D.10）: ShellWindows は `RunAs = Interactive User` なので、起動するのはそのセッションの利用者で、別の管理者として起動することはない（この PC のレジストリで確認）。残る未確認は、B の H2 から A の ShellWindows への呼び出しが COM のアクセスの検査を通るか。通らなければ RunOnce と Run キーに任せる。F.7 の T-UPD-8。
3. **NSIS の終了コードと `File`**（D.9.1、D.9.2）: `SetErrorLevel` の値が `.onInit` の `Quit` を含めて最後まで効くことは、レビューが `Main.c` で確かめた。残るのは、`AllowSkipFiles on` の `File` の書き込みの失敗のサイレントの既定の答え（「無視」か「中止」か。どちらでも何も置き換えない）と、名前の変更の失敗の扱い。煙の試験（D.9.3 の 4）で確かめる。
4. （解決）`gh` と下書き: `gh` は下書きをタグ名で扱える（レビューが cli/cli の `FetchRelease` で確認）。
5. （解決）immutable なリリースの「最新」と「プレリリース」の印は、公開後も変えられる（GitHub のドキュメント、2026-09-29）。
6. **ARM64 のネイティブの判定**（C.7）: x64 のエミュレーションの中で `IsWow64Process2` が ARM64 を返すことは広く報告されているが、実機で確かめていない（ARM64 の PC がない）。今の設計では表示にしか使わない。
7. **サインアウトとインストール**（D.7、D.12）: `installing` の間はセッションの終了を止めることにした。0x3FF の H2 に `WM_QUERYENDSESSION` が NSIS より先に届くこと、Windows が理由の文をどう見せるかは、F.6 と T-UPD-16 で確かめる。
8. **`ProgramData` の先回り**（m2 I.18）: `Updates` も先回りして作られうる。隔離の仕組み（m2 S1）がそのまま効くが、隔離できない場合は更新も `Storage` で止まる。
9. **従量制の接続**（E.1）: ダウンロードを自動で行う（J-7 の決定 (a)）。問題になれば、`NetworkInformation` の接続の費用を見て止める。
10. （解決。WP-0、2026-09-29）**`CARGO_CFG_DEBUG_ASSERTIONS`**（A.10）: ビルド スクリプトに、対象のデバッグの表明の設定が渡る。`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` のリリース ビルドで 3 つの exe の `IsDebug` が true になった（L 章）。
11. **ほかの利用者のセッションでの起動し直し**（D.8）: `quit-if-idle` で終わらせたほかの利用者の GUI は、その利用者が開き直すかサインインし直すまで戻らない。タスク スケジューラーの 1 回だけのタスク（その利用者の SID、`TASK_LOGON_INTERACTIVE_TOKEN`、`RunLevel` LUA）で戻す案は、管理者がパスワードなしで登録できるか未確認で、コードも増えるので採らなかった（J-2 の決定は (a)。(c) は選ばれなかった）。
12. **ほかの利用者の GUI のプロセスを開けるか、SID が取れるか**（D.8）: 昇格した H2 が、ほかの利用者の `mklm.exe` を `PROCESS_QUERY_LIMITED_INFORMATION` で開けるかは確かめていない。開けなければ、作成時刻付きの識別を前後で比べる方式に落ちる。また、パイプの名前を計算するのに使う `WTSEnumerateProcessesW` の `pUserSid` が、SYSTEM でない昇格したプロセスから見て、ほかの利用者のプロセスについても入っているかを、この PC で確かめていない（Microsoft Learn は「プロセスのプライマリ トークンの利用者の SID」とだけ書く。FIX-VERIFICATION-5）。入っていなければ、その GUI は飛ばされ、D.8 の 3 の待ちで `ProgramsStillRunning` になる（安全側）。F.6 の任意の 2 つ目のアカウントか T-UPD-7 で確かめる。
13. **再現可能なビルド**（B.5 の 5）: 同じコミットを手元でビルドして CI と同じハッシュになるかは確かめていない（PDB のパス、時刻、NSIS の圧縮など）。比べるのは任意で、違っても直ちに異常とはしない。
14. **ウイルス対策ソフトの遅れ**（D.4、RELIABILITY-5）: H2 の起動から `ready` までの時間。F.6 と T-UPD-15 で測り、120 秒で足りなければ見直す。
15. **最小の環境ブロック**（D.9.4）: NSIS、プラグイン、`mklm.exe --quit` が、ほかの環境変数を必要としないか。F.6 と煙の試験で確かめる。
16. **Restart Manager でファイルを開いているプロセスが分かるか**（D.8 の 4。RED-TEAM-3）: `RmGetList` は、読み込まれたモジュールだけでなく、ふつうに開いたファイルのハンドルの持ち主も返すと理解しているが、確かめていない。煙の試験の 4（PowerShell でファイルを開いたまま）と同じ状態で `file_holders` が `powershell.exe` を返すかを、F.6 で確かめる。返さなければ `holders` は空のままで、理由の文は変わらない。
17. **凍結への備え**（B.4 の 4。RED-TEAM-1）: 自動の信号は `expires` だけ。期限は 180 日に短くした（J-3 の決定 (b)）。将来、オンラインの鍵で新しさだけを保証する仕組み（TUF の timestamp）を足すかは、ユーザーの判断に残る。

---

## J. ユーザーに決めてもらうこと

2026-09-29 に、ユーザーがすべての質問に答えた（右端の列）。設計の本文は決定に合わせてある（J-3 の有効期限 180 日: A.2、A.8、B.3、B.4、B.5、C.6、E.3、F.1、G.7 の 7 と 22、H.1。J-6 の署名の PC: B.5、B.6、G.1、G.7 の 21、L 章）。本文では「J-n の決定」と引く。

| # | 質問 | 選択肢 | おすすめ | 決定（2026-09-29） |
|---|---|---|---|---|
| 1 | ARM64 の PC で x64 版を使っている利用者を、自動更新で ARM64 版に切り替えるか | (a) 切り替えない（今のアーキテクチャのまま） (b) 自動で切り替える (c) 画面で尋ねる | (a)。ARM64 版は実機で試していない（M6 で試した後に見直す） | 決定（2026-09-29）: **(a)** 自動で ARM64 版に切り替えない（C.7、G.7 の 14） |
| 2 | 更新のとき、ほかの利用者（ユーザーの切り替え）の MKLM が何もせず動いていたら | (a) 終わらせて更新する。その利用者の MKLM は、その利用者が開き直すか、サインアウトしてサインインし直すまで戻らない（ユーザーの切り替えで戻るのは再接続で、自動起動は動かない）。次のサインインで結果と「いったん終了していました」が出る (b) 更新をやめて「別のユーザーの MKLM が開いています。そのユーザーに終了してもらってから更新してください」と出す (c) (a) に加えて、タスク スケジューラーでその利用者のセッションに MKLM を起動し直す（コードが増え、動くか未確認） | (a)。自動起動が既定でオンなので、共有の PC ではほかの利用者の MKLM がほぼいつも動いており、(b) では更新がほとんどできない。キーボードの操作の途中や確認待ちなら、どれでも更新をやめる。**注**（FIX-VERIFICATION-7）: どの選択肢でも、この PC のほかの利用者は更新を期限なしに止め続けられる。MKLM のファイルを開いたままにする、自分の MKLM を何かの途中にしておく（または手を加えていつも `busy` を返させる）、`mklm-cli` を入力待ちのまま置く、などで（G.7 の 16）。半端には入らず、管理者には相手のプロセスが示される | 決定（2026-09-29）: **(a)** 何もしていないほかの利用者の MKLM を `quit-if-idle` で終わらせて更新する（D.8、E.4.1、G.7 の 10）。タスク スケジューラーでの起動し直し（(c)）は作らない（I.11） |
| 3 | 更新情報の有効期限（凍結に気付くまでの最長の時間でもある） | (a) 400 日（年 1 回のリリースで切れない） (b) 180 日（5 か月ごとのリリースが要る。見張りは期限の 60 日前に知らせる） (c) 2 年 | **(b) に変えた**（RED-TEAM-1）。レビュー第 1 回の後の版は「30 日以上確認できないときのバナーと巻き戻しの警告も凍結に気付かせる」として (a) を勧めたが、誤りだった: 正しく署名された古い更新情報を見せ続けられると、確認は成功し続け、どちらも出ない。凍結を自動で知らせるのは期限だけで、期限の長さがそのまま、隠されたセキュリティの修正に気付くまでの最長の時間になる（B.4 の 4）。期限はインストールの門ではないので、メンテナーがリリースできなかったときの代償は、利用者に出る「期限切れ」の情報の表示（とバナー 30 日に 1 回）だけ。年に 2 回以上リリースできないなら (a) | 決定（2026-09-29）: **(b)** 180 日。`DEFAULT_VALIDITY_DAYS` = 180（H.1）、`prepare-release` の既定も 180 日（B.3）。見張り（`update-canary.yml`）は期限の 60 日前に知らせる（`--min-days-left 60`。G.6）。保守リリースは少なくとも 5 か月に 1 回（B.4 の 3、B.5 の注） |
| 4 | CLI からインストールできるようにするか | (a) 確認と状態の表示だけ (b) インストールもできる | (a)。スクリプトはインストーラーを直接 `/S` で実行できる | 決定（2026-09-29）: **(a)** CLI は `update --check` と `update --status` だけ（D.14） |
| 6 | 署名をする PC | (a) 普段の開発機の、普段のアカウント。署名の間はネットワークを切る、F.6 のデバッグ ビルドと開発用の鍵が残っていない、署名は鍵の USB の公式の `minisign` だけで行う（B.6）。**残る危険**: `prepare-release` とふだんの開発は、依存のクレートのビルド スクリプトと proc-macro を同じアカウントで実行する。侵された依存が常駐すれば、数分後につなぐ鍵のファイルと、打ち込むパスワードを盗み、ネットワークが戻った後に送り出せる（FIX-VERIFICATION-16） (b) 専用のオフラインの PC（または毎回きれいな状態から起動する仮想マシンやライブ USB） (c) 普段の開発機に、署名専用の標準ユーザーのアカウントを 1 つ作る。そのアカウントでは cargo も git も実行しない。署名の前に開発用のアカウントから**サインアウト**してから、そのアカウントで署名する（B.5 の手順 7〜10） | **(c)**。アカウントを 1 つ作り、署名のたびにサインアウトとサインインを 1 回ずつ足すだけで、(a) の残る危険のうち、利用者の権限で常駐するものを締め出せる。管理者の権限まで奪われた場合は (b) でなければ防げない | 決定（2026-09-29）: **(a)** 普段の開発機の、普段のアカウントで署名する。署名専用のアカウントは作らない。守る注意: 署名の間はネットワークを切る、F.6 のデバッグ ビルドと開発用の鍵を残さない、鍵に触れるのは公式の `minisign` だけ（B.5、B.6）。**ユーザーは残る危険（侵されたビルドの依存が鍵とパスワードを盗みうること）を受け入れた**（G.7 の 21） |
| 7 | 従量制の接続でも自動でダウンロードするか | (a) する（数 MB） (b) しない | (a) | 決定（2026-09-29）: **(a)** 従量制の接続でもダウンロードする（E.1、I.9） |
| 8 | 認証の要るプロキシ（Windows の統合認証）への対応 | (a) Windows の資格情報を自動で送らない。そのプロキシの内側では自動更新が使えず、リリース ページへ案内する (b) プロキシにだけ既定の資格情報を送る（同じネットワークの攻撃者が WPAD でプロキシを名乗ると、利用者の NetNTLM の応答が渡る） | (a)。署名で更新の中身は守られても、資格情報の漏れは守れない（SECURITY-13） | 決定（2026-09-29）: **(a)** プロキシにも Windows の資格情報を送らない（A.7、G.7 の 17） |
| 9 | 秘密鍵を扱う道具 | (a) 公式の `minisign` 0.12 の Windows 版（作者の鍵で確かめ、ハッシュを固定し、鍵と同じ媒体に置く。ISC ライセンスで、同意の操作はない） (b) minisign をソースから自分でビルドする（Zig か CMake と libsodium が要る） (c) 一度だけ、読み合わせたコミットから `xtask` の署名の道具をビルドし、鍵と一緒に保管する | (a)。手間が最も少なく、作者の署名で出どころを確かめられる | 決定（2026-09-29）: **(a)** 秘密鍵に触れるのは公式の `minisign` 0.12 のバイナリだけ（0.2 の 9、B.1、B.5） |

- レビュー前の質問 5（immutable releases をいつ有効にするか）は、公開後も「最新」と「プレリリース」の印を変えられると確かめられ、事故の手順（B.5）もできたので、(a) v0.2.0 から、に決めて質問から外した。2026-09-29 にユーザーも確かめた: **immutable releases は v0.2.0 から有効にする**（B.4、B.5 の準備）。

最初の更新対応版（v0.2.0）の前に、メンテナーが公式の `minisign` を用意し、鍵を作る必要がある（B.5 の準備。J-6 の決定 (a) により、普段のアカウントで行う）。

---

## K. 計画・依頼からの変更点

| # | 計画・依頼 | この設計 | 理由 |
|---|---|---|---|
| 1 | MSI と msiexec（計画 4.1〜4.2） | NSIS の `/S` と、決めた終了コード（D.9）。exe は `.new` から名前の変更で入れ替える | 2026-09-28 のユーザーの決定。その場の上書きは失敗すると切り詰められた exe を残すため（RELIABILITY-2） |
| 2 | 依頼: 新しい要求は「ダウンロードしたファイルのパス」を運ぶ | インストーラーのバイト列をパイプで 64 KiB ずつ運ぶ（D.3） | 昇格した helper が利用者の書ける場所を開くと、シンボリック リンクによる UNC への誘導（NTLM の中継）、デバイスへの誘導、差し替えの危険がある。計画 4.2 の手順 2（バイト列だけを受け取る）と計画 2.2（HKCU と `%APPDATA%` を読まない）にも合う |
| 3 | 結果は `Updates\last-result.json`（計画 4.2 の 9、依頼） | HKLM の `Update\LastResult`（と `Run`、`Trust`）（D.6） | `Updates` は非昇格の GUI が読めない。読めるファイルにすると読み手が書き換えを止められる。レジストリの値は原子的に書け、先回りされない |
| 4 | インストール済みの版は `MsiGetProductInfo`（計画 4.2 の 3） | helper 自身の版（= インストールした版）と、インストール先の exe のビルド ID（D.7 の 11、D.13） | NSIS に対応するものがない。ビルド ID はすでに埋め込まれている |
| 5 | アセットに `culture` と `url`、UpgradeCode の照合（計画 4.2） | `culture` はない（インストーラーは日英を含む）。`url` は持たず、版と名前から作る（A.5、C.8）。UpgradeCode の代わりに `product` と名前 | NSIS は 1 つのインストーラーで両言語。URL を署名の中に持たせないと、鍵が漏れてもダウンロード先を変えられない |
| 6 | `expires` を過ぎたら「更新を確認できません」（計画 4.2） | 情報として知らせるが、インストールは止めない（B.4、C.6）。メイン画面のバナーにも 30 日に 1 回出す（E.3。レビュー前は出さなかった。SECURITY-11） | immutable releases では有効期限を延ばすのに新しいリリースが要る。期限を門にすると、メンテナーが動けないだけで正しい更新も止まる |
| 7 | GitHub の asset `digest` を API で照合（計画 4.2） | クライアントは照合しない。**メンテナーの道具**（`prepare-release`、`publish`）が照合する | クライアントは REST API を使わない方針（レート制限）。SHA-256 は署名した更新情報にある。署名の前後の照合には `digest` が役に立つ（OPS-UX-TEST-2） |
| 8 | `xtask sign-release` が `gh attestation verify` を行う（計画 4.3） | **必須**: release.yml が来歴の証明を必ず付け、`prepare-release` がワークフロー、コミット、タグ、GitHub ホストのランナーを指定して確かめる（B.3） | レビュー前は任意だった。オフラインの鍵が、CI や GitHub の乗っ取りで差し替えられたファイルに署名しないため（SECURITY-1） |
| 9 | helper は HKLM に記録したインストール先から起動する（計画 2.2、m2 E.1） | 固定のインストール先（`%ProgramFiles%\SHIN DATA CENTER\MKLM`）で動いていることを確かめる（D.4 の 1） | NSIS はインストール先を固定し、HKLM の製品のキーに書かない（M5a） |
| 10 | 更新はロックを持ったまま msiexec を実行する（m2 D.9） | H1 と H2 がそれぞれロックを取り、H2 はジャーナルを確かめ直す（D.4、D.7） | ロックはプロセスをまたいで渡せない（`LockFileEx` の鍵はハンドルとプロセスに属する） |
| 11 | 起動し直しの予備は HKCU の RunOnce（計画 4.2 の 8） | 同じ。RunOnce は GUI が引き継ぎの前に自分で登録する。`quit-if-idle` で終わるほかの利用者の GUI も登録する（D.8、D.10） | helper は HKCU に書かない |
| 12 | 確認は起動時と 24 時間ごと（計画 4.2） | 起動の 60〜180 秒後（前回から 24 時間以上なら）と、24 時間 + 0〜60 分ごと（E.1）。未来の時刻の記録は無視する | サインイン直後の負荷を避け、利用者の間で時刻を散らす。時計の誤りで確認が止まらないように（RELIABILITY-8） |
| 13 | 計画 4.3: `xtask sign-release` が鍵で署名する | 秘密鍵に触れるのは、固定した公式の `minisign` だけ。`xtask` は署名の前の確認（`prepare-release`）と後の確認・公開（`publish`）を行う（B.3、B.5） | 署名の瞬間に、タグのソースと crates.io の依存を、鍵のつながった PC でコンパイルして実行しないため（SECURITY-2） |
| 14 | 依頼: 更新情報の `key_id`（1 つ）と署名ファイル 1 つ | `key_ids`（1〜2）と、移行の間だけの副署名（A.2、A.4、B.7） | 鍵を替えた後も、古い版が新しい版の更新情報を受け付けられるように（SECURITY-4、OPS-UX-TEST-1） |
| 15 | 依頼: `revoked_keys` で鍵を失効させる | 失効させられるのは、検証する版の信頼の起点にある通常用の鍵だけ。知らない ID は無視し、記録は指紋で範囲を決める。バックアップ用の鍵は新しい版の信頼の起点でだけ失効させ、通常用の鍵の署名の中のバックアップ用の鍵の失効は無視する（拒否しない。FIX-VERIFICATION-1）（B.2） | 漏れた鍵で後継の鍵を先回りして失効させる攻撃を防ぐ（SECURITY-3） |
| 16 | 依頼: `issued_at` で巻き戻しを防ぐ | 記録は `min(issued_at, 受け取った時刻)` を鍵ごとに持ち、判断は鍵をまたいだ最大値（失効した鍵を除く）で行う（B.2）。`issued_at` は GitHub の時刻から決める（B.3） | 署名した PC の時計の誤りで以後の更新が止まらないように。helper がこの PC の知る最新より古い更新情報を入れないように（SECURITY-5、SECURITY-12、RELIABILITY-6） |
| 17 | m3 F.5 の A12: 多重起動のパイプの `quit` は更新に使わない | 副作用のない `quit-if-idle` を足し、H2 はそれだけを使う（D.8、E.4.1。m3 F.1、F.5、A12 を合わせて改めた） | `quit` は `busy` を返す前に取り消しと「後で終了」を立て、確認の画面を出すため（RELIABILITY-1、OPS-UX-TEST-4） |
| 18 | 計画 4.2: 「N 日間、更新情報がない」ときも「更新を確認できません」 | 30 日以上確認に成功していなければ、30 日に 1 回のバナーと設定の行で知らせる（E.3） | レビュー前はこの規則を黙って落としていた（OPS-UX-TEST-5） |
| 19 | 計画 4.2 の 8: 起動し直しは Explorer 経由 | 同じ。加えて、H2 は COM のセキュリティを最初に固定し、起動し直しを最後の手順にする（D.10） | 昇格した H2 が中程度の整合性レベルの Explorer からオブジェクトを受け取るため（SECURITY-8） |

---

## L. 確認できていない事実

**確かめたこと**（2026-09-29。レビューの指摘を含む）

- GitHub の immutable releases: 公開後も題、リリースノート、「プレリリース」と「最新」の印を変えられる。ファイルとタグは固定（GitHub のドキュメント）。
- `gh attestation verify` のフラグ `--repo`、`--signer-workflow`（形は `[host/]<owner>/<repo>/<path>/<to>/<workflow>`）、`--source-digest`、`--source-ref`、`--deny-self-hosted-runners`（gh の manual）。
- GitHub REST のリリースのアセットに `digest` フィールドがある（REST のドキュメントのスキーマ。値の形 `sha256:<hex>` はレビューの記述）。
- 公式の `minisign` 0.12: `-S` は既定で prehashed、`-l` で legacy、`-t` で trusted comment、`-x` で署名ファイル、`-W` でパスワードなしの鍵。Windows 版は `minisign-0.12-win64.zip`。作者の公開鍵 `RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3`（README）。
- Cargo のドキュメント: ビルド スクリプトで `PROFILE` を使うのは勧められない。`CARGO_CFG_<cfg>` はビルドするパッケージの cfg。
- この PC のレジストリ（読み取りだけ）: ShellWindows の AppID の `RunAs` は `Interactive User`、`LocalServer32` は `rundll32.exe shell32.dll,SHCreateLocalServerRunDll {9BA05972-…}`。
- コード（main の 37989f8）: `state.rs` の `quit_requested` は、`Running` で `quit_pending` と `CancelSession` を立て、再接続待ちでは終了の確認を出す。`app.rs` の `instance_command` は返事によらず `QuitRequested` を送る。helper の `write_loop` は `busy` の間だけ Heartbeat を送り、`busy` は `Request` の間だけ立つ。`FileLock` の共有は読み取りと書き込み。各 `build.rs` は `PROFILE` で `VS_FF_DEBUG` を決めている。`mklm.nsi` には `SetErrorLevel` が 3010 の 1 つしかなく、`.onInit` の拒否は `Abort`、`CloseMklm` と `un.onInit` の拒否は `SetErrorLevel` のない `Quit`（「25 は `un.onInit` だけ」はこの設計の決定で、コードの事実ではない。レビュー第 1 回の後の版は事実として書いていた。FIX-VERIFICATION-10）。`mklm-win::instance` の多重起動のパイプは `FILE_FLAG_FIRST_PIPE_INSTANCE` と最大 1 インスタンスで作られ、名前は `instance_pipe_path(session, sid)`。`mklm_client::session::SessionKind` は `{Request, Recovery}`（H.4 の `SessionPurpose` の名前の理由）。`apps/build_id.rs` は各クレートの `Cargo.toml` と `src/` だけをハッシュし、テストを持たない。release.yml の `actions/checkout` は `fetch-depth` を指定していない（既定の 1。タグなし）。
- NSIS の `Main.c`: `errlvl` が設定されていれば、それが終了コード（レビューが確認）。
- NSIS の `util.c` の `myOpenFile`: `CreateFile` の共有は `FILE_SHARE_READ` だけ（2026-09-29 に kichik/nsis の master のソースで確認）。`FileOpen` の `a` が書き込みだけか読み書きかは確かめていない（D.9.3 の結論には影響しない）。
- Microsoft Learn の `WTS_PROCESS_INFOW`: `SessionId`、`ProcessId`、`pProcessName`、`pUserSid`（「プロセスのプライマリ トークンの利用者の SID」）を持つ（2026-09-29）。
- Microsoft Learn の `WinHttpOpen`: `WINHTTP_ACCESS_TYPE_NAMED_PROXY` はバイパスの一覧に当たらない名前をプロキシに送る。`<local>` はピリオドのない名前だけをバイパスする（2026-09-29）。

**WP-0 で確かめたこと**（2026-09-29、この開発機。Rust 1.98.1、VS Build Tools 2022。G.2 の手で確かめる項目を含む）

- **ビルド ID と信頼の起点ファイル**（G.2。FIX-VERIFICATION-15）: `apps/build_id.rs` が `HASHED_CRATES` の 5 つと `HASHED_FILES`（`anchors.txt`）をハッシュするようにした後、`cargo build -p mklm -p mklm-cli -p mklm-helper`（dev）で 3 つの exe の VERSIONINFO の `MKLMBuildId` はそろって `0.1.0+806e3257…ad9a`。`anchors.txt` にコメントの行を 1 つ足して同じビルドをすると、3 つとも `0.1.0+162df532…66ba` に変わり（`watched_paths` によりビルド スクリプトが走り直した）、元に戻すと 3 つとも元の値に戻った。
- **`VS_FF_DEBUG` と `CARGO_CFG_DEBUG_ASSERTIONS`**（A.10。I.10 は解決）: 3 つの `build.rs` を `CARGO_CFG_DEBUG_ASSERTIONS` の有無で決めるようにした。`cargo build --release --workspace` の 3 つの exe は `VersionInfo.IsDebug` が false。`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`（別の `CARGO_TARGET_DIR`）の `cargo build --release -p mklm -p mklm-cli -p mklm-helper` では、生成した `*-version.rch` が `MKLM_FILEFLAGS 0x1L` で、3 つとも `IsDebug` が true（`PROFILE` は `release` のまま）。つまり、ビルド スクリプトには対象のパッケージのデバッグの表明の設定が渡る。dev のビルドも 3 つとも true。
- **目印（DEV_MARKER）**（A.10）: `RUSTFLAGS=--cfg mklm_update_dev`、`CARGO_TARGET_DIR=target\dev-update` の `cargo build -p mklm -p mklm-cli -p mklm-helper`（dev）で、3 つの exe すべてに `MKLM-UPDATE-DEV-OVERRIDES!` があった（陽性の対照が通る）。骨組みでは GUI と CLI はまだ `for_this_build` を呼ばない（WP-C の `env::environment` から呼ぶ）が、dev のビルドでは `#[used]` の静的な値だけで残った。helper は骨組みの `update` と `run_update` から `for_this_build` を呼ぶ。`--release` の 3 つの exe と、開発用の cfg なしの dev の `mklm.exe` には目印がない。
- **helper とネットワーク**（A.9）: `cargo tree -p mklm-helper -e features` に `mklm-win feature "net"` も `mklm-update feature "winhttp"` も `Win32_Networking_WinHttp` もない（対照: `mklm-cli` の木には `net` と `winhttp` がある）。`dumpbin /imports`（`vswhere` で見つけた MSVC 14.44.35207 の `Hostx64\x64\dumpbin.exe`）で、リリースの x64 の `mklm-helper.exe` は `WINHTTP.dll` をインポートしていない。
- **クレートの版と API**（B.1、G.7 の 2）: `minisign-verify` =0.3.0、`minisign` 0.10.0、`sha2` 0.11.0、`semver` 1.0.28 は crates.io にあり、解決してコンパイルできる（`sha2` 0.11.0 の `rust-version` は 1.85、`minisign-verify` 0.3.0 は宣言なしで依存 0）。`minisign-verify` 0.3.0 のソース（`lib.rs`）で、`PublicKey::from_base64`、`Signature::decode`、`PublicKey::verify(&[u8], &Signature, allow_legacy)`（鍵 ID が違えば `Error::UnexpectedKeyId`、legacy を許さず legacy なら `Error::UnexpectedAlgorithm`、署名の誤りは `Error::InvalidSignature`）、`Signature::trusted_comment()` が B.1 のとおりであることを確かめた（`Signature` は `Debug` を実装しない）。`minisign` 0.10.0 の `sign` は常に prehashed（`ED`）の署名を作り、`minisign-verify` が `allow_legacy = false` で通す。`minisign` 0.10.0 の依存は `getrandom` 0.4、`scrypt` 0.11、`rpassword` 7.5.4、`ct-codecs`（dev-dependency だけ）。これらを `crates/mklm-update/tests/crate_apis.rs`（使い捨ての鍵をテストの中で作る）に固定した。
- **鍵 ID の表記**（B.2）: `minisign` 0.10.0 の公開鍵ファイルの 1 行目は `untrusted comment: minisign public key: <ID>` で、`<ID>` は鍵 ID の 8 バイトを little-endian の u64 とした 16 桁の大文字の 16 進（`{:016X}`）。`KeyId::to_text` と一致する（同じテスト）。公開鍵の base64 を解いた 42 バイトの先頭 2 バイトは `Ed`、続く 8 バイトが鍵 ID。
- **`windows` 0.62.2 の feature 名**: `Win32_Networking_WinHttp`、`Win32_System_Ole`、`Win32_System_Variant`、`Win32_System_RemoteDesktop`、`Win32_System_RestartManager` はクレートの `Cargo.toml` にある（`Win32_System_Com` は既存）。WP-0 は `net`（`Win32_Networking_WinHttp`）、`Win32_System_Ole`、`Win32_System_Variant` を足した。

**確かめていないこと**

- `minisign-verify` 0.3.0 と `minisign` 0.10.0（どちらも 2026-09-25 公開）の、前の版からの変更点（`minisign-verify` のソース全体の読み合わせは G.6 のレビュー）。`minisign` クレートの署名と鍵が、公式の `minisign` 0.12 の鍵ファイルと互換か（prehashed であることは WP-0 で確かめた）。
- 公式の `minisign` のコマンドの鍵 ID の表示が、`minisign` クレートと同じ形か（B.5 の準備で目で確かめる。クレートの形は WP-0 で確かめた）。
- 公式の `minisign` の Windows 版の配布物が、作者の鍵で署名された `.minisig` を伴うか、legacy の署名か（`verify-signer` は legacy も受け付ける）。ライセンスが ISC であること。
- `IShellWindows` / `IShellDispatch2` などの `windows` 0.62.2 での置き場所（feature 名は WP-0 で確かめた）。
- `WINHTTP_OPTION_AUTOLOGON_POLICY` の HIGH が、プロキシへの既定の資格情報の送信も止めること。レビュー第 1 回の後の版は「F.3 の 407 のテストで確かめる」と書いたが、そのテストはプロキシなしのセッションで、プロキシの認証の経路を通らなかった（FIX-VERIFICATION-4）。F.3 の「プロキシの認証の試験」（名前付きのプロキシ、LOW の対照つき）が通るまで未確認。その試験が通っても、`AUTOMATIC_PROXY`（WPAD と PAC）で見つけたプロキシに同じ方針が効くことは前提のまま（未確認）。`WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY` がループバックの要求をプロキシに送らないか（ループバックのテストはプロキシなしのセッションを使うので影響しない）。
- `WTSEnumerateProcessesW` の `pUserSid` が、SYSTEM でない昇格したプロセスから、ほかの利用者のプロセスについても得られること（I.12。FIX-VERIFICATION-5）。
- Restart Manager の `RmGetList` が、ふつうに開いたファイルのハンドルの持ち主を返すこと（I.16。RED-TEAM-3）。
- `std::hint::black_box` で参照した `#[used]` の静的な値が、`/OPT:REF` のリンクの後も 3 つの exe に残ること（A.10。dev のビルドで残ることは WP-0 で確かめた。リリースには開発用のモジュールがない。ci.yml と `-Profile dev` の陽性の対照が、残らなければ失敗して知らせる）。
- GitHub の下書きのページがアセットの SHA-256 を表示するか（B.5 の手順 8 の任意の照合の手段にならないか。今の手順は手順 6 の表示と比べるだけ）。
- （確かめる必要がなくなったもの）署名専用のアカウントにかかわる 2 つ（`C:\Users\Public` の受け渡しのフォルダーの権限、サインアウトした開発用のアカウントのプロセスが残らないこと）は、J-6 の決定 (a)（普段のアカウントで署名する）で不要になった。
- 引き継ぎの案内の文（約 110 文字）を、ナレーターが 15 秒で読み終えること（E.4。T-UPD-14 で確かめる）。
- NSIS: `AllowSkipFiles on` の `File` の失敗のサイレントの既定の答え、`Rename` の失敗が `${Errors}` を立てること、`windows-2025` のランナーが build 26100 であること。NSIS の zip（`nsis-3.12.zip`）の SHA-256（WP-H が SourceForge の表示と照らして記録する）。
- 昇格したプロセスから `IShellWindows` / `IShellDispatch2` で起動し直す方法が、Windows 11 25H2 で同じ利用者のときに動くこと（M5a では NSIS の `explorer.exe <path>` の方法を確かめた）と、別の管理者のときの COM のアクセスの検査。`EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA` の下で ShellWindows の呼び出しが通ること。
- `FILE_SHARE_READ` だけのハンドルを開いたまま、そのファイルを `CreateProcessW`（一時停止）で起動できること（I.1）。
- `ShutdownBlockReasonCreate` を窓のスレッドから呼ぶ必要があること（Microsoft Learn の記述と理解しているが、読み直していない）。0x3FF の H2 に `WM_QUERYENDSESSION` が先に届くこと。
- 昇格した H2 が、ほかの利用者の `mklm.exe` を `PROCESS_QUERY_LIMITED_INFORMATION` で開けること（I.12）。
- NSIS とプラグインが、最小の環境ブロックの外の環境変数を必要としないこと（I.15）。
- `IsWow64Process2` がエミュレーション中の x64 のプロセスで `IMAGE_FILE_MACHINE_ARM64` を返すこと。
- UAC で起動した helper（H1）がジョブ オブジェクトに入っていないこと（G.7 の 12）。
- reqwest が Windows のレジストリのプロキシ設定を読むが PAC を扱わないこと（A.9 の比較の 1 項目。決定には影響しない）。
- GitHub のリリースのファイルの配布元が、今後も `*.githubusercontent.com` であること（2026-09-29 の実測は `release-assets.githubusercontent.com`）。
- 公開のリポジトリの定期のワークフローが、60 日間の活動のなさで止まること（GitHub のドキュメントの記憶。確かめ直していない）。
- 同じコミットの手元のビルドが CI と同じハッシュになるか（I.13）。

---

## M. レビュー対応

対応: **採用**（指摘のとおりに変えた）、**一部採用**（目的は受け入れ、方法の一部を変えた、または一部を採らなかった）、**不採用**（採らなかった。理由を書く）。

### M.1 第 1 回（47 件）

第 1 回は不採用の指摘はない。第 2 回の検証で誤りが分かった行には「第 2 回で訂正」を書き足した。

| ID | 重さ | 対応 | 何を変えたか / 採らなかった部分とその理由 |
|---|---|---|---|
| SECURITY-1 | critical | 一部採用 | (1) release.yml の来歴の証明を必須にした（SHA で固定、G.6）。(2) `prepare-release --commit`: 手元の HEAD とタグ、GitHub のタグのコミット、`gh attestation verify`（`--signer-workflow`、`--source-digest`、`--source-ref`、`--deny-self-hosted-runners`）、`SHA256SUMS`、GitHub の `digest` を確かめ、合わなければ作らない（B.3）。(3) チェックリストに差分のレビューを足した（B.5 の 4）。(5) NSIS はハッシュを固定した公式の zip（G.6）。(6) `verify --remote --installers` が公開後にインストーラーも確かめる（`publish` と見張り）。**採らなかった部分**: (4) の手元の再ビルドとの照合は「推奨の必須の手順」にせず任意にした（B.5 の 5）。再現可能なビルドかどうかを確かめておらず（I.13）、一致しないことが異常を意味しないうちは、手順の門にできないため |
| SECURITY-2 | major | 一部採用 | 秘密鍵に触れるのを、作者の鍵で確かめてハッシュを固定した公式の `minisign` 0.12 だけにした（鍵の生成、署名、点検。B.1、B.5、B.6）。`xtask` は秘密鍵のファイルを開くコードもパスワードの入力も持たない。順序: 確認と `latest.json` の作成（オンライン）→ ネットワークを切る → 鍵をつなぐ → 署名 → 外す → つなぐ → 公開。F.6 のデバッグ ビルドと開発用の鍵を残した PC では署名しない（B.6、F.6 の後片付け）。`minisign` のクレートはテストの dev-dependency だけにした。**採らなかった部分**: cargo-deny / cargo-vet の導入。`minisign` のクレートは本物の鍵に触れなくなったので対象から外れ、`minisign-verify` は依存 0 の小さなクレートなので、v0.2.0 の前のソース全体の読み合わせと記録で代える（G.6）。1 人のメンテナーに cargo-vet の運用は重い |
| SECURITY-3 | major | 採用 | 規則 5 をやめた。失効させられるのは検証する版の信頼の起点にある通常用の鍵だけで、知らない ID は無視する。バックアップ用の鍵は更新情報では失効させられない。記録の失効は (鍵 ID、指紋) の組にした（B.2、C.3 の 10、H.1 の `RevokedKey`）。F.1 にテストを足した |
| SECURITY-4 | major | 採用 | 副署名 `latest.json.alt.minisig` と `key_ids`（1〜2）。主署名が `UnknownKey` / `RevokedKey` のときだけ副署名を取る（A.4、A.5、C.3）。`prepare-release` は、このタグの失効、公開中の更新情報の失効、`--revoke` の和を `revoked_keys` に求める（B.3 の 9）。B.7 を書き直した（N と N+1 以降の鍵、バックアップ用の鍵の漏れは通常用の漏れと同じかそれ以上、手で入れ直す場合）。回転の版を見逃した利用者の F.1 のテスト |
| SECURITY-5 | major | 採用 | `CallerMessage::RecordTrust`: GUI と CLI は、どの helper のセッションでも、利用者の記録が機械の記録より進んでいれば、検証済みの更新情報を送り、helper が検証して機械の記録を進める（C.4、D.3）。巻き戻しの判断を鍵をまたいだ最大値（失効した鍵を除く）にした（B.2）。届く範囲の限界を G.7 の 18 に書いた。F.2 と F.1 にテスト（**第 2 回で訂正**: 実際には `RecordTrust` の serde の形しか試していなかった。`apply_trust_report`、helper の処理（`record_trust`、`CallerOrder`）、記録の後の `stage_update` の `Rollback` のテストを足した。FIX-VERIFICATION-6） |
| SECURITY-6 | major | 採用 | H1 → H2、H2 → NSIS を `CREATE_UNICODE_ENVIRONMENT` の最小の環境ブロック（`runner_environment`）で起動し、`TEMP` / `TMP` は保護された `<run dir>\tmp`、作業フォルダーは System32（D.4 の 18、D.7 の 16、D.9.4）。ブロックの組み立ては純粋な関数とテスト（F.2）、レビューの確認事項（G.6） |
| SECURITY-7 | major | 採用（第 1 回は一部採用、第 2 回で残りも採用） | パイプの名前を厳密な正規表現に通してから開く（`parse_instance_pipe_name`）。対象は `$INSTDIR\mklm.exe` が動いているセッションだけ、最大 16 本、1 本 5 秒、全体 20 秒、SQOS の識別、サーバーのプロセスをハンドルで固定（開けなければ作成時刻付きの識別を前後で比べる）。`check_pipe_path` も `/`、`.`、`..`、ASCII 以外を拒否（D.8、H.3）。第 1 回では、列挙をやめて名前を計算する方式を採らなかった（理由として「ほかのセッションの利用者の SID には `WTSQueryUserToken`、ほかの利用者のトークンを開くこと、`LookupAccountName` のどれかが要る」と書いた）。**第 2 回で訂正**: その理由は誤りだった。`WTSEnumerateProcessesW` の `WTS_PROCESS_INFOW.pUserSid` で得られる（Microsoft Learn）。列挙をやめ、動いている `$INSTDIR\mklm.exe` のプロセスから名前を計算する方式に改めた（D.8 の 2、H.3。FIX-VERIFICATION-5）。これで偽の名前のパイプで予算を使わせる遅延もなくなった |
| SECURITY-8 | minor | 採用 | H2 はほかのどの COM の呼び出しより前に `CoInitializeSecurity`（`RPC_C_IMP_LEVEL_IDENTIFY`、`EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA`）を呼ぶ（D.7 の 1、D.10、H.3 の `init_com_for_runner`）。起動し直しは最後の手順（ロックを放し、ハンドルを閉じ、`LastResult` を書いた後）。レジストリの事実を D.10、I.2、L 章に書き、T-UPD-8 を残した |
| SECURITY-9 | minor | 採用 | 開発用の経路をすべて `cfg(all(debug_assertions, mklm_update_dev))` にした。cfg は F.3 と F.6 のコマンドだけが渡す。`build.rs` は `CARGO_CFG_DEBUG_ASSERTIONS` で `VS_FF_DEBUG` を決める（Cargo が渡すかは WP-0 が確かめる）。release.yml は `RUSTFLAGS` などの環境変数と `.cargo/config.toml` を検査し、6 つの exe の目印を探す（A.10、G.6）。F.6: 開発用の鍵は `%TEMP%` の外に置いて消す、デバッグ ビルドをアンインストールしてから署名する |
| SECURITY-10 | minor | 採用 | NSIS の 2 段階の置き換え（`.new` に展開、名前の変更で入れ替え、失敗したら戻す）と、終了コード 26（`FILES_IN_USE`）と 27（`FILE_WRITE`）（D.9.1、D.9.2）。H2 は NSIS の前に同じ条件を確かめ、`NotInstalled(FilesInUse)`（D.8 の 4）。ほかの利用者が更新を遅らせられることを G.7 の 16 と E.6 の文に書いた。**第 2 回で訂正**: 「J 章の質問 2 の説明に書いた」は誤りで、書いていなかった。また「遅らせる」ではなく期限のない妨害で、方法もファイルを開くことだけではない（`busy` を返す GUI、入力待ちの CLI、先回りのパイプの名前）。J 章の質問 2 の注、G.7 の 16、D.8 の 4 を書き直し、管理者に相手のプロセスを示す `holders` を足した（FIX-VERIFICATION-7、RED-TEAM-3） |
| SECURITY-11 | minor | 一部採用 | 30 日以上確認に成功していない、または期限切れのとき、メイン画面のバナーを 30 日に 1 回（E.3）。自動の確認の `Rollback` も警告として見せる（E.3、E.6）。**採らなかった部分**: 期限を 180 日に短くすることは、メンテナーのリリースの頻度（利用者の負担）に関わるので、設計では決めず J 章の質問 3 で尋ねる（おすすめは 400 日のまま。気付かせる仕組みを 2 つ足したため）。**その後**: 第 2 回（RED-TEAM-1）でおすすめを 180 日に変え、2026-09-29 のユーザーの決定（J-3）で 180 日になった |
| SECURITY-12 | minor | 一部採用 | `prepare-release` は `issued_at` を GitHub の `Date` から決め、手元の時計との差が 5 分を超えるか、公開中の `issued_at` 以下なら拒否し、UTC の日付を大きく表示する（B.3 の 7、8）。クライアントの記録は **`min(issued_at, 受け取った時刻)`** にした。**変えた部分**: 指摘の `now + 2 日` ではなく `now` にした。2 日の余裕があると、誤った日付の更新情報を見てから 2 日以内に出た次の正しい版を、毎日確認するクライアントの多くが永久に拒むため（B.2） |
| SECURITY-13 | minor | 採用 | `WINHTTP_OPTION_AUTOLOGON_POLICY` を HIGH、`WinHttpSetCredentials` を呼ばない、401 と 407 は失敗（A.7、H.3）。F.3 に 401 / 407（NTLM、Negotiate）で `Authorization` を送らないテスト（**第 2 回で訂正**: 407 のテストはプロキシなしのセッションで、プロキシの認証の経路を通っていなかった。名前付きのプロキシと LOW の対照の試験を足し、L 章を未確認に戻した。FIX-VERIFICATION-4）。G.6 の確認事項。統合認証のプロキシの内側で自動更新が使えなくなるので、J 章の質問 8 で確かめる |
| SECURITY-14 | minor | 採用 | 更新のページと UAC の説明の画面に「詳細を表示」でプログラムの場所を確かめる手順と、「MKLM が許可を求めるのは［今すぐ更新］を押した直後だけ」を足した（E.2、E.4）。`install-guide.ja.md` にも書く（G.3） |
| RELIABILITY-1 | major | 採用 | 副作用のない `quit-if-idle` を足し、H2 はそれだけを使う（D.8、E.4.1、H.3）。`busy` のときは何も変えない。4 秒を過ぎて届いたものは何もしない。m3 F.1、F.5、A12 を改めた。`state.rs` のテスト（F.4） |
| RELIABILITY-2 | major | 採用 | 2 段階の置き換え（D.9.2）。`.new` の展開の間だけ `AllowSkipFiles on` にして `${Errors}` を見て 27、それ以外は `off`（指摘の「常に `off`」との違いは、置き換える前の失敗を 2 ではなく 27 で報告するため）。終了コード 26 / 27 と 2 の行の訂正（D.9.1）。空き容量の検査（D.4 の 4、D.7 の 12）。CLI の更新中の早い終了（D.14）。キャッシュのインストーラーは成功してそろったときだけ消す（D.11）。そろっていないときの［インストーラーを実行］（D.13）。`docs/recovery.md`（G.4）。煙の試験の 4（D.9.3） |
| RELIABILITY-3 | major | 採用 | H2 は `SetProcessShutdownParameters(0x3FF)` と見えない窓。`installing` の間は `ShutdownBlockReasonCreate` と拒否、`ready` / `waiting` では NSIS を起動せずに `NotInstalled(SessionEnding)`（D.7、H.3）。引き継ぎの案内に「開き直すまでサインアウトや再起動はしない」（E.4）。I.7 と D.12 を改めた |
| RELIABILITY-4 | major | 採用 | NSIS を一時停止で作り、`Run`（`installing` と `installer`）を書いてフラッシュしてから再開。書けなければ止める（D.7 の 16）。`classify_run` は `installing` / `finishing` で runner か installer が生きていれば `InProgress`（H.1）。H1 は installer が生きていれば `UpdateInProgress`（D.4 の 7）。15 分では `Run` を消さず、`Failed(InstallerTimedOut)` を書いて 60 分まで待つ（D.7 の 17）。F.2 のテスト |
| RELIABILITY-5 | major | 採用 | H2 は場所と記録とインストーラーのハンドルだけで `ready` にし、検証し直しとハッシュは H1 が去った後（D.4 の注、D.7 の 8）。`READY_WAIT` 120 秒、`StartingRunner` のメッセージと Heartbeat（`StageUpdate` の間も `busy`）、GUI の `HANDOFF_WAIT` 150 秒（D.3、H.1）。止めた H2 の終わりを待ってから消す（D.4 の 19）。GUI は helper を失ったら `Run.caller` で引き継ぎを判断（E.4、H.4 の `HandOffProbe`）。F.6 で Defender のクラウドの保護をオンにして時間を記録 |
| RELIABILITY-6 | major | 一部採用 | 信頼できる時刻、公開中の `issued_at` 以下の拒否、UTC の表示（B.3）。`verify` は 1 日以上先の `issued_at` で失敗（B.3）。クライアントは `Rollback` の両方の値をログに書き、更新のページと `update --status` に出す（E.3、D.14）。**変えた部分**: B.7 の「日付の誤った更新情報」の行は、バックアップ用の鍵で次の版に署名して主の鍵を替える手順ではなく、「次の版をふつうに出す」にした。記録を受け取った時刻で抑える（SECURITY-12 の対応）ので、正しい日付の次の版はそのまま通り、鍵を替える必要がないため |
| RELIABILITY-7 | minor | 一部採用 | D.8、E.5、J 章の質問 2、T-UPD-7、G.7 の 10 の誤り（「次のサインインで戻る」）を直した。`quit-if-idle` で終わる GUI は自分の RunOnce を登録し、次のサインインで結果と「いったん終了していました」を出す（E.4.1、E.5、D.13）。I.2 と L 章にレジストリの事実。**採らなかった部分**: タスク スケジューラーでほかの利用者のセッションに GUI を起動し直す案。管理者がパスワードなしで対話のトークンのタスクを登録できるか未確認で、昇格したプロセスがほかの利用者として何かを起動する仕組みを増やすことになる。J 章の質問 2 の (c) として利用者に選んでもらう（I.11） |
| RELIABILITY-8 | minor | 採用 | `last_check` か `last_success` が 1 時間以上先なら、確認していないものとみなす（E.1）。F.4 のテスト |
| RELIABILITY-9 | minor | 採用 | `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` をやめ、`sweep_stale_run_dirs` を H1、H2、helper の通常のセッションで呼ぶ（D.11、H.3）。`remove_at_reboot` を削除。インストーラーの `.old` も `/REBOOTOK` なしで消す（D.9.2）。T-UPD-3 で `PendingFileRenameOperations` を確かめる |
| RELIABILITY-10 | minor | 採用 | 25 をインストーラーの表から外した（D.9.1、H.1）。煙の試験のアンインストールを `_?=` と `Start-Process -Wait` に、ランナーを `windows-2025` に固定、ファイルを開いたままの試験（D.9.3）。成功の経路にまぎれた `SetErrorLevel` の危険と check-nsi.ps1 の規則（D.9.1、D.9.3）。I.3 を改めた |
| RELIABILITY-11 | minor | 採用 | 中断の表示も `result_seen` で利用者ごとに 1 回だけ（D.13）。helper の通常のセッションが、ロックを取った後に死んだ `Run` を `LastResult` に移す（D.6、D.11） |
| RELIABILITY-12 | minor | 採用 | H2 のコピーを `mklm-update-runner.exe` にした（0.3、D.4 の 15、D.7、H.1、H.5）。check-nsi.ps1 とレビューの確認事項に「実行中の MKLM は `$INSTDIR` のパスでだけ見つける」（D.9.3、G.6） |
| OPS-UX-TEST-1 | critical | 採用 | 信頼の起点を `crates/mklm-update/trust/anchors.txt` に移し、`prepare-release` が過去のタグのファイルを `git show` で読んで、直前の版と 400 日以内の版がその署名を受け付けるかを表にし、受け付けない版があれば拒否（`--allow-strand` で例外）（B.2、B.3 の 10）。`publish` も実際の署名で同じ検査。副署名（SECURITY-4 と共通）。B.7 の「影響なし」を直し、取り残される利用者の列を足した |
| OPS-UX-TEST-2 | major | 採用 | `cargo xtask publish`（ファイルの組、署名の検証、`digest`、`--latest` での公開、公開後の確認）で公開する（B.3、B.5）。下書きの題を「UNSIGNED, DO NOT PUBLISH」に（G.6）。リリースの事故の runbook（B.5）。I.4 と I.5 を閉じた |
| OPS-UX-TEST-3 | major | 採用 | `update-canary.yml`（公開、毎週、手動。secrets なし。本番の経路の `verify --remote --installers --min-days-left 60` と `fetch-smoke`）。60 日の停止の注とカレンダーの予定（B.5、G.6） |
| OPS-UX-TEST-4 | major | 採用 | RELIABILITY-1 と同じ対応（`quit-if-idle`、13 バイト、H.3 の `InstanceCommand::QuitIfIdle`、m3 F.1 / F.5、F.4 のテスト）。J 章の質問 2 と G.7 の 10 を直した |
| OPS-UX-TEST-5 | major | 採用 | 30 日以上確認に成功していなければ、30 日に 1 回のバナーと設定の行。一時的な失敗と構造的な失敗で文を分け、構造的なものはリリース ページへ（E.2、E.3、E.6）。`ClientState.last_failure`（H.4）。K の 18。F.4 のテスト |
| OPS-UX-TEST-6 | major | 採用 | `LastResult` は 14 日以内で、今の版とインストールの状態に合うときだけ表示し、そうでなければ黙って表示済みにする。「そろっていない」の文は今の状態がそろっていないときだけ。ほかの利用者には中立の文（`started_run` で判断）（D.13、E.5、E.6） |
| OPS-UX-TEST-7 | major | 採用 | `TrustAnchors::release()`（信頼の起点ファイルだけ）を `xtask` の本番のコマンドが使い、開発用の cfg の `xtask` では本番のコマンドが失敗する。開発用の署名は別の接頭辞 `mklm-dev-latest-json v1`。テストは `from_keys` だけ（A.4、A.10、B.3、C.2、F.1） |
| OPS-UX-TEST-8 | major | 採用 | `xtask fetch-smoke`（本番の `WinHttpTransport` と URL の規則で `releases/latest/download/SHA256SUMS`）、`tag_from_location` をどのファイル名にも使えるように（A.6、B.3、H.1）。v0.2.0 の前に開発機で、毎回 release.yml で、毎週見張りで。v0.2.0 の公開後の `update --check --json` の確認（B.8、F.8） |
| OPS-UX-TEST-9 | major | 採用 | F.6 を書き直した: 使い捨てのブランチで版を変えて `cargo update -w --offline` してコミット、`dist-dev\<版>\` と版ごとの `SHA256SUMS`、`prepare-release --dev --only-arch x64`（`--dev` なしでは使えない）、MKLM を全部終わらせる手順、後片付け |
| OPS-UX-TEST-10 | major | 一部採用 | H1 と H2 の手順を、環境のトレイト（`StagerEnv`、`RunnerEnv`）の上の純粋な駆動部（`stage_update`、`run_update`）にし、D.12 の各行で失敗を注入する表のテストを F.2 に足した（H.1、H.2）。**変えた部分**: 置き場所。H2 の駆動部は指摘どおり `mklm_update::run_flow` だが、H1 の駆動部はパイプのメッセージを使うので `mklm_ipc::staging` に置いた（`mklm-ipc` が `mklm-update` に依存するため、逆向きには置けない）。helper のクレートに置かないのは、`requireAdministrator` のマニフェストを持つ bin のクレートで、今は単体テストがないため |
| OPS-UX-TEST-11 | major | 採用 | F.7 をグループ A（キーボードに書かない。`Busy` は昇格した PowerShell でロック、`ProgramsStillRunning` は `mklm-cli` の `Continue? [y/N]` で待たせる）とグループ B（キーボードを調べた後）に分けた。`FileLock` の共有がその開き方と両立することはコードで確かめた |
| OPS-UX-TEST-12 | major | 採用 | `update_dir::read_build_ids` と `InstallState::from_build_ids` の 1 組を H2 と `read_status` が共有（D.13、H.1、H.3、H.4）。`InstanceCommand::QuitIfIdle`（H.3）。E.4.1 の表。GUI の中の名前（`SessionPhase::Updating`、`UacNoticeOrigin` など）を H.4 に固定。各 WP が自分のモジュールの `windows` の features を足してよい（G.2） |
| OPS-UX-TEST-13 | major | 一部採用 | `key-drill start` / `check`: 公式の `minisign` で nonce に署名し（trusted comment `mklm-key-drill v1`）、`xtask` が最新と 400 日以内のタグのバックアップ用の鍵で確かめる。パスワードの保管を分け、点検の記録を残す（B.6）。**変えた部分**: 指摘の `xtask` が鍵を復号して署名する形は、SECURITY-2 の方針（秘密鍵に触れるのは公式の `minisign` だけ）に反するので、署名は `minisign`、確認は `xtask` に分けた |
| OPS-UX-TEST-14 | minor | 採用 | release.yml はタグに `-` があれば `--prerelease` の下書き。プレリリースの手順（署名しない、`--latest` を付けない）。`xtask publish` はプレリリースのタグを拒否（B.3、B.5、G.6） |
| OPS-UX-TEST-15 | minor | 採用 | 引き継ぎのオーバーレイを 5 秒以上（**第 2 回で変更**: 最長 15 秒で自分で閉じ、［OK］でいつでも閉じる。FIX-VERIFICATION-13）、［OK］付き、文を「1 分ほどで自動で開きます。2 分たっても開かない場合は…」に（E.4）。表示する結果がなければ `unregister_after_update`、`--after-update` は `--tray` と同じに（D.13、E.5） |
| OPS-UX-TEST-16 | minor | 採用 | E.6 をすべての列挙子の表（文の ID、一時 / 構造、次の操作）にした。「スキップした版（未ダウンロード）」と［ダウンロード］（C.5、E.2）。`FILE_NAMES_ALLOWED` に `mklm-cli` と `mklm-helper`、インストーラーの名前は `names` で渡す（E.7） |
| OPS-UX-TEST-17 | minor | 採用 | SECURITY-9 と合わせて対応（目印の静的な値と release.yml の検査、`CARGO_CFG_DEBUG_ASSERTIONS`）（A.10、G.6） |
| OPS-UX-TEST-18 | minor | 一部採用 | 鍵をつないでいる間にコンパイルしない点は、SECURITY-2 の対応（署名は公式の `minisign` だけ、`prepare-release` は鍵をつなぐ前）で満たした。時刻の検査は採用したが、差の上限は 1 時間ではなく 5 分にした（RELIABILITY-6 の値。Windows の時刻の同期があれば 5 分で足り、厳しい方が誤りを早く見つける） |
| OPS-UX-TEST-19 | minor | 採用 | `installer.yml`（PR で `installer/**`、`apps/**`、`Cargo.lock` が変わったとき煙の試験）、直前のリリースからの上書きの試験、`dumpbin` を `vswhere` で探す（D.9.3、G.6） |
| OPS-UX-TEST-20 | minor | 一部採用 | `update --check` の終了コードを分けた: 0 最新、20 更新あり、21 手で更新が必要、22 このビルドでは使えない、1 確認できなかった、2 使い方の誤り（D.14）。`install-guide.ja.md` に書く。**変えた部分**: 指摘の 10〜12 ではなく 20〜22 にした。書き込みのコマンドの 10（`AWAITING_CONFIRM`）と同じ数が別の意味になるのを避けるため |
| OPS-UX-TEST-21 | minor | 採用 | RELIABILITY-7 と同じ訂正（D.8、J 章の質問 2、G.7 の 10）。(b) の選択肢の文に「別のユーザー」を入れ、後でその利用者に「いったん終了していました」を出す |

### M.2 第 2 回（20 件: 第 1 回の対応の検証 17 件、新しい攻撃の検討 3 件）

不採用の指摘はない。

| ID | 重さ | 対応 | 何を変えたか / 採らなかった部分とその理由 |
|---|---|---|---|
| FIX-VERIFICATION-1 | major | 採用（案 (a)） | 通常用の鍵の署名の中の、埋め込みのバックアップ用の鍵の失効は、`IllegalRevocation` ではなく**無視**（記録しない）にした（B.2 の規則 2 の表と注、規則 4、C.3 の 10、H.1 の `VerifiedManifest::revoked`、K の 15）。攻撃者は失効の行を省けば同じものを作れるので、拒否は何も守っていなかった。B.3 の手順 9 と 10、B.7 の 3、4 行目と注（回転の後の「P2、B2、revoked P1、revoked B1」）を合わせ、F.1 に「バックアップ用の鍵の回転」、`xtask` のテストに取り残しの検査の例を足した。`check-keys` に「過去に失効させた ID を埋め込まない」を足した。案 (b)（手順 9 でバックアップ用の ID を入れない）を採らなかったのは、`revoked_keys` を版ごとに変える規則が複雑になり、引き継ぎの検査（公開中の失効をすべて含む）とぶつかるため |
| FIX-VERIFICATION-2 | major | 採用 | 取り残しの検査の対象を「窓の中の版」: 後継の公開から 400 日たっていない版と、後継のない版（直前の版）と、このタグ自身、にした（B.3。`xtask::releases::window_targets`）。公開後にプレリリースの印を付けた安定版も含める。0.3 の「移行の窓」、B.7 の 1 行目、`key-drill check`（B.3、B.6。`gh release list` を使うのでネットワークが要る）、H.1 の定数の説明を合わせた。指摘の時系列を `xtask` のテストにした（F.1） |
| FIX-VERIFICATION-3 | minor | 採用 | 開発用の分岐が `std::hint::black_box(&DEV_MARKER)` で目印を参照する。陽性の対照: ci.yml の F.3 のステップと `build-installer.ps1 -Profile dev` が、開発用の cfg の 3 つの exe に目印が**ある**ことを確かめ、なければ失敗（A.10、F.3、F.6、G.6）。ビルドの前の検査を `CARGO_TARGET_*_RUSTFLAGS`、`RUSTC`、`RUSTC_WRAPPER`、`RUSTC_WORKSPACE_WRAPPER`、`CARGO_BUILD_RUSTC*`、`CARGO_HOME` と親のフォルダーの Cargo の設定ファイル、ワークフローの `--config` に広げた（`installer/check-build-env.ps1`。G.4） |
| FIX-VERIFICATION-4 | minor | 採用 | `HttpSession::open_named_proxy`（開発用の cfg、`AutologonLevel`）を足し、`https://update.invalid/` への `CONNECT` にプロキシ役のサーバーが 407（NTLM、Negotiate）を返す試験を `crates/mklm-win/tests/net_proxy.rs` に置いた。HIGH では `Proxy-Authorization` が来ないこと、LOW の対照では来ること（来なければ失敗）。チャレンジは返さないので、資格情報からの応答は作られない（F.3、H.3、G.3）。L 章を未確認に戻し、`AUTOMATIC_PROXY` の経路は前提のままと書いた。F.3 の表の 407 の行は状態の番号の写し方の試験と書き直した |
| FIX-VERIFICATION-5 | minor | 採用 | パイプの列挙をやめ、`processes_with_images` の GUI と `WTSEnumerateProcessesW`（`process_users`）の SID とセッションから `instance_pipe_path` で名前を計算する。正規表現は二重の確かめとして残す（D.8 の 2、H.3 の `instance_pipe_candidates`、`quit_idle_instances` の引数から `sessions` を外した）。多重起動のパイプは最大 1 インスタンスなので、動いている GUI の名前に偽物を足すことはできない。M.1 の SECURITY-7 の行を訂正し、`pUserSid` の未確認を I.12 と L 章に書いた。F.2 に `instance_pipe_candidates` のテスト |
| FIX-VERIFICATION-6 | minor | 採用 | F.1 に `apply_trust_report`（新しい、同じ、署名の誤り、古い、開発用の鍵、古い版）。helper の処理を純粋な `mklm_ipc::staging::record_trust` と順序の規則 `CallerOrder` にして（H.2）、F.2 に表のテスト（ロックの期限切れで `Busy`、どの返事の後もセッションが続く、2 つ目や遅れた `RecordTrust` はプロトコルの誤り）と、「`RecordTrust`（M2）の後の `StageUpdate`（古い M1）が `Refused(Rollback)`」を足した。結合テストを F.2 に書き、M.1 の SECURITY-5 の行を訂正した |
| FIX-VERIFICATION-7 | minor | 採用 | J 章の質問 2 に注（どの選択肢でも、ほかの利用者はファイルを開いたままにする、`busy` の GUI（手を加えたものを含む）、入力待ちの CLI などで、更新を止め続けられる）。G.7 の 16 を 4 つの方法に書き直した。M.1 の SECURITY-10 の行を訂正した |
| FIX-VERIFICATION-8 | minor | 採用 | E.4.1 の `HandedOff` の `quit-if-idle` を `busy`（何も変えない）にした。H2 は呼び出し元の終了を D.8 の 1 で別に待つ。F.4 のテストに `HandedOff` を明記した |
| FIX-VERIFICATION-9 | minor | 採用 | 起動時に `Run` が `ready` 以降で、セッションが `Run.caller_session` と違う（またはない）GUI は、昇格していなければ自分の RunOnce（`--after-update`）を登録し、`closed_by_update` を書いてから終了する。その利用者の MKLM は、開き直すか次にサインインするまで動かないことを D.13 の 1、E.1、G.7 の 10 に書いた。F.4 のテスト |
| FIX-VERIFICATION-10 | minor | 採用 | (a) 煙の試験の 3〜5 で `[IO.File]::Open` の権利と共有を正確に決めた（4 は `'Read','ReadWrite'`、5 は `'Read','Read'`）。NSIS の `myOpenFile` の共有が `FILE_SHARE_READ` だけであることは、こちらでも kichik/nsis のソースで確かめた。(b) 26 と 27 の前に `/SD IDOK` の MessageBox（`FILES_IN_USE`、`FILE_WRITE_FAILED`）を出す（D.9.1、D.9.2）。(c) L 章の「25 は `un.onInit` だけ」をこの設計の決定として書き直し、37989f8 の `mklm.nsi` の実際（`SetErrorLevel` は 3010 だけ）を書いた |
| FIX-VERIFICATION-11 | minor | 採用 | D.7 の 18 と D.13 の `update_dir::read_install_state` を `read_build_ids` と `InstallState::from_build_ids` に直した（環境のトレイトのメソッドの名前はそのまま）。H.4 に `SessionPurpose::{Change, Update}` と `AppState::session_purpose` を足し、E.4.1 と F.4 で使った。`SessionKind` の名前は `mklm_client::session::SessionKind` とぶつかるので避けた |
| FIX-VERIFICATION-12 | minor | 採用（案: 種類を増やさない） | `classify` を `Option<(ErrorClass, &str)>` にし、すべての `CheckError` の写し方を E.6 に書いた: 2 つの `Cancelled` と `NotConfigured`、`NotInstalledCopy` は記録しない（`last_check` も進めない）。`Rollback` は `Structural`（と別の警告のバナー）。`check` が返さないものは `Transient` と不具合のログ。E.6 の「種類」の列の説明、E.3、H.4、F.4 を合わせた。`Warning` の種類を足さなかったのは、30 日のバナーの文を選ぶ目的には、構造の文（リリース ページへ）で足りるため |
| FIX-VERIFICATION-13 | minor | 採用 | `HANDOFF_OVERLAY_MIN`（5 秒）を `HANDOFF_OVERLAY_MAX`（15 秒）に替えた。［OK］でいつでも閉じ、押されなければ 15 秒で閉じる。`CALLER_EXIT_WAIT`（30 秒）との関係を F.4 のテストで固定する。15 秒にしたのは、指摘の 10 秒では日本語の案内（約 110 文字）の読み上げが終わらない見込みのため。同じ内容は UAC の前のページにもある（E.4、E.7、H.1、H.4、D.1） |
| FIX-VERIFICATION-14 | minor | 採用（両方の案を組み合わせた） | 早い終了（6）の対象を、書き込みのコマンドと `update --check` に絞った。読み取りのコマンド（`list`、`status`、`global status`、`journal`、`update --status`）は 1 秒ほどで終わり更新を止めないので、`main.rs` の 0 / 1 / 2 の約束を保つ。`update --check` はネットワークで長く動きうるので 6 を残し、D.14 の表と `install-guide.ja.md` に載せる（G.3）。F.4 のテスト |
| FIX-VERIFICATION-15 | minor | 採用 | (a) `apps/build_id.rs` に `HASHED_FILES`（`crates/mklm-update/trust/anchors.txt`）を足し、ハッシュと `watched_paths` に入れる（G.2）。`build_id.rs` にテストがないので、WP-0 が手で一度確かめる。(b) release.yml の `build` のジョブの checkout に `fetch-depth: 0`。`check-keys` は浅いリポジトリ、タグなし、読めないタグで、飛ばさずに失敗する（B.3、G.6、F.1） |
| FIX-VERIFICATION-16 | minor | 採用 | B.6 に「残る危険」（開発のコードが鍵とパスワードを盗みうる）と、安い順の対策（署名専用のアカウント、専用の PC かきれいな起動、`prepare-release` を別の PC で）を書いた。J 章の質問 6 に (c) 署名専用の Windows のアカウントを足し、おすすめにした。B.5 の準備と手順 6〜11、B.6 の点検を (c) の流れにした（受け渡しは `C:\Users\Public\mklm-sign\`）。加えて、書き直しの中で見つけた穴を塞いだ: (c) で開発用のアカウントが書いた `SIGN-OFFLINE.txt` を貼ると、侵されていれば鍵をつないだ署名のセッションで任意のコマンドを実行させられるので、(c) では固定の形のコマンドを打ち、値だけを写す（B.5 の手順 9、B.6 の点検）。そのため鍵の点検の trusted comment を固定の `mklm-key-drill v1` にした（署名が新しい乱数のファイルにかかるので、nonce を写す必要はない）。G.7 に 21 を足した。**その後**: 2026-09-29 のユーザーの決定（J-6）は (a)（普段のアカウントで署名）で、B.5、B.6、G.1 から (c) の流れを外し、G.7 の 21 に受け入れた残る危険を書いた |
| FIX-VERIFICATION-17 | minor | 採用 | G.1 で「公式の `minisign` の用意と `verify-signer`」を F.6 の前に、鍵の生成を後に分けた（B.5 の準備にも書いた）。B.3 に `prepare-release --dev` の文法（`--dist`、`--dev-pub`、`--minisign`、`--only-arch`、`--issued-at`、`--out`、`SIGN-OFFLINE.txt` の 1 行、拒否するもの）と `serve-releases` を書き、F.6 の手順 1、5、6 と後片付けを合わせた。F.1 にテスト |
| RED-TEAM-1 | major | 一部採用 | B.4 の 4 を書き直した: 正しく署名された古い更新情報を見せ続ける凍結では、確認が成功し続け、30 日のバナーも巻き戻しの警告も出ない。自動の信号は `expires` だけ（C.6、E.3、G.7 の 22、I.17）。人の手がかりとして、「最新です」の行と `update --check --json` に更新情報の日付（`issued_at`）を出す（E.2、D.14）。見張りに `verify --newest-published`（配られている版が公開済みの最も新しい安定版より古ければ失敗）を足した（B.3、G.6、F.8）。J 章の質問 3 のおすすめを 180 日に変えた（期限は門ではないので、短くする代償は情報の表示だけ）。**採らなかった部分**: 期限とは別の「更新情報の古さ」の警告（たとえば 90 日）。期限をもう 1 つ短く持つのと同じで、メンテナーがその間隔でリリースしなければ全員に誤報が出るので、期限そのものを短くする方を選んだ。オンラインの鍵による新しさの保証（TUF の timestamp）は、秘密鍵をオフラインだけに置く決定の外にあるので、I.17 に残した |
| RED-TEAM-2 | minor | 採用（主張を取り下げた） | D.13 の［インストーラーを実行］の照合を「壊れたファイルを見つけるためだけで、安全の保証ではない」と書き直した。閉じる案（helper が検証して実行する、管理者だけの場所に写す）は、ファイルの版がそろっていないときは helper を起動できず（ビルド ID の不一致）、GUI 自身がその利用者のプロセスなので、どれもその利用者に対しては何も守れないため採らなかった。画面と読み上げの文で「検証済み」と言わない。ほかの利用者の PC で管理者として承認するときの注意を `recovery.md` と `install-guide.ja.md` に書く（D.13、E.7、H.3、G.3、G.4）。標準ユーザーはこの経路がなくても同じ見た目の UAC を出せるので、新しい昇格の道ではない |
| RED-TEAM-3 | minor | 採用 | G.7 の 16 と D.8 の 4 を「期限のない妨害で、自動で抜ける方法はない」に書き直した。`FilesInUse` と `ProgramsStillRunning` に相手のプロセス（`FileHolder`: PID、セッション ID、名前。Restart Manager と `processes_with_images` から。利用者の SID は入れない）、`InstanceBusy` にセッション ID を足し、技術的な詳細と `update --status` に出す（H.1、H.3 の `file_holders`、E.6、H.5）。管理者の手順（タスク マネージャーで止める、再起動の直後に更新する）を `recovery.md` に書く（G.4）。Restart Manager が開いたハンドルを返すかは未確認（I.16） |

**m3 への反映**: `docs/design/m3-gui.md` の F.1（コマンドの一覧に `quit-if-idle`）、F.5（M5 の更新の段落）、A12（レビュー対応の表）に、この設計の D.8 と E.4.1 への参照を足した。
