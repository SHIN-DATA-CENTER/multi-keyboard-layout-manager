# リリースの署名と公開（メンテナー向け）

MKLM の自動更新は、メンテナーがオフラインで署名した `latest.json`（更新情報）を、利用者の PC の MKLM が検証して使う。この文書は、署名の準備、毎回のリリースの手順、鍵の保管と点検、事故と鍵の漏れへの対応をまとめる。設計の根拠は `docs/design/m5b-updater.md` の B 章（とくに B.3〜B.7）。

- **秘密鍵に触れるのは、ハッシュを確かめた公式の `minisign` 0.12 だけ**（J-9 の決定 (a)）。`cargo xtask` は公開のデータだけを扱い、秘密鍵のファイルを開かず、パスワードも尋ねない。
- **署名は普段の開発機の、普段のアカウントで行う**（J-6 の決定 (a)、2026-09-29）。署名の間はネットワークを切り、鍵の媒体の `minisign.exe` 以外を動かさない。残る危険（下の「鍵の保管と点検」）は、ユーザーが受け入れた。
- 有効期限の既定は **180 日**（J-3 の決定 (b)）。**少なくとも 5 か月に 1 回**は保守リリースを出す。見張り（`update-canary.yml`）は期限の 60 日前から失敗して知らせる。
- immutable releases は **v0.2.0 から**有効にする。公開した後は、ファイルの追加も置き換えもできない。`latest.json` と署名は必ず**下書きのうちに**上げる（`cargo xtask publish` が行う）。

---

## 1. 道具

| 道具 | 使う場面 |
|---|---|
| 公式の `minisign` 0.12（`minisign-0.12-win64.zip`） | 鍵の生成、署名、鍵の点検の署名。リリースのリハーサル（設計 F.6）の署名も同じもの |
| `gh`（GitHub CLI） | `cargo xtask` が子プロセスで呼ぶ（タグ、下書き、来歴の証明、アップロード、公開）。`gh` なしでは公開しない（`winget install GitHub.cli`） |
| `cargo xtask` | 下の表 |

| コマンド | 何をするか | ネットワーク |
|---|---|---|
| `cargo xtask verify-signer --zip <zip> --sig <zip.minisig>` | 公式の `minisign` の zip を作者の公開鍵 `RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3` で確かめ、zip と中の `minisign.exe` の SHA-256 を表示する（`minisign.exe` は Windows の `tar.exe` で一時フォルダーに取り出して計算し、すぐ消す） | なし |
| `cargo xtask pubkey-line --pub <file.pub> --role primary\|backup` | `minisign -G` の `.pub` から、信頼の起点ファイルに貼る 1 行と、鍵 ID、指紋（公開鍵の SHA-256）を表示する | なし |
| `cargo xtask check-keys` | 信頼の起点ファイル（`crates/mklm-update/trust/anchors.txt`）の検査。通常用 1 本とバックアップ用 1 本、失効させた鍵を埋め込んでいない、過去のタグの同じ ID が別の鍵を指していない、過去に失効させた ID を埋め込み直していない。過去のタグを読めなければ（浅いリポジトリ、`v*` のタグがない、途中のタグにファイルがない）失敗する。release.yml が呼ぶ | なし（git） |
| `cargo xtask prepare-release …` | 署名の前のすべての確認と、`latest.json`、`trusted-comment.txt`、`SIGN-OFFLINE.txt`、`prepare.json` の作成（下の 3 章） | あり |
| `cargo xtask publish --tag vX.Y.Z --dir <dir>` | 署名の検証、アップロード、ファイルの組の確認、公開、公開後の確認 | あり |
| `cargo xtask verify --remote [--installers] [--min-days-left N] [--newest-published]` | 公開中の `latest.json` を本番の経路で取り、この木の信頼の起点ファイルで検証する。見張りが使う | あり |
| `cargo xtask verify --dir <dir>` | 手元の `latest.json` と署名を検証する | なし |
| `cargo xtask fetch-smoke` | 本番の経路で `releases/latest/download/SHA256SUMS` を取り、リダイレクトの各段、tag の取り出し、TLS、本文を確かめる | あり |
| `cargo xtask key-drill start --role backup --out <dir>`、`key-drill check --dir <dir> --role backup` | 鍵の点検（6 章） | `check` はあり |
| `cargo xtask prepare-release --dev …`、`cargo xtask serve-releases --dir <dir> [--port N]` | デバッグ ビルドのリハーサル専用（設計 F.6）。本番の鍵は使えない | なし |

- 本番のコマンドは、`--cfg mklm_update_dev` でビルドされた xtask では何もせずに失敗する（設計 A.10）。
- `prepare-release` と `publish` の確認を飛ばして、公式の `minisign` だけで署名して公開しない。`xtask` がビルドできない場合は、リリースを延期する。

---

## 2. 準備（初回だけ）

最初の項目（公式の `minisign` の用意と確認）は、設計 F.6 のリハーサルの**前に**行う（リハーサルも同じ道具で署名するため）。2 つ目以降（本物の鍵の生成など）は、F.6 の後片付けが済んでから行う（設計 G.1 の順序）。

- [ ] 公式の `minisign` を用意する: `https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-win64.zip` とその `.minisig` をダウンロード → `cargo xtask verify-signer --zip … --sig …` → 表示された SHA-256 を下の「記録」の表に書く → 確かめた zip を残しておく（F.6 はここから取り出した `minisign.exe` を使う）。
- [ ] （ここから F.6 の後）確かめた zip の中の `minisign.exe` を、通常用の鍵の USB メモリ（`E:\tools\`）とバックアップ用の媒体（`F:\tools\`）の両方に置く。
- [ ] 鍵は普段の開発機の、普段のアカウントで作り、署名する（J-6 の決定 (a)。署名専用のアカウントは作らない）。その前に 5 章の「署名に使う PC の条件」を確かめる: F.6 の後片付けが済み、デバッグ ビルドの MKLM と開発用の鍵（`%USERPROFILE%\mklm-dev-keys\`）が残っていない。
- [ ] ネットワークを切る → エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる → 通常用の USB メモリ（BitLocker To Go）をつなぐ → `Get-FileHash E:\tools\minisign.exe` が記録した値と一致することを確かめる → `E:\tools\minisign.exe -G -p E:\mklm-keys\mklm-primary.pub -s E:\mklm-keys\mklm-primary.key`（パスワードを 2 回）→ 外す。
- [ ] 同じく（ネットワークを切ったまま）、別の媒体でバックアップ用: `F:\tools\minisign.exe -G -p F:\mklm-keys-backup\mklm-backup.pub -s F:\mklm-keys-backup\mklm-backup.key` → 外す → ネットワークを戻す。2 つのパスワードは別々の保管場所に置く（5 章）。
- [ ] 2 つの `.pub`（公開鍵。秘密ではない）を作業用のフォルダーに写し、`cargo xtask pubkey-line --pub … --role primary`、`--role backup` → 表示された 2 行を `crates/mklm-update/trust/anchors.txt` に貼る → `cargo xtask check-keys` → コミット（公開鍵だけ。秘密鍵は決してリポジトリに入れない）。`minisign -G` が表示した鍵 ID と、`pubkey-line` の ID が一致することを目で確かめる（`minisign` は先頭の 0 を省いて表示することがある。`pubkey-line` は `.pub` の 1 行目の ID と違えば止まる）。
- [ ] 公開鍵を `docs/install-guide.ja.md` の「更新情報を手で確かめる」に載せる。
- [ ] GitHub アカウントにハードウェア キーの 2 段階認証。`main` とタグ `v*` の保護。
- [ ] リポジトリの設定で immutable releases を有効にする（v0.2.0 から）。
- [ ] バックアップ用の鍵の点検を 1 回行う（6 章）。

---

## 3. 毎回のリリース（安定版）

1. `Cargo.toml` の `[workspace.package] version` を上げてコミットし、`main` に入れる。
2. `git tag vX.Y.Z` → `git push origin vX.Y.Z`。
3. Actions の「Release」が緑になり、題が「MKLM vX.Y.Z — UNSIGNED, DO NOT PUBLISH」の下書きに、2 つのインストーラーと `SHA256SUMS` が付くのを待つ（来歴の証明も CI が付ける）。
4. **レビュー**: `git fetch --tags` → `git switch --detach vX.Y.Z` → `git rev-parse HEAD` をメモする（これが `--commit`）。`git diff <直前のタグ>..vX.Y.Z` を読む。特に `.github/`、`installer/`、各 `build.rs`、`Cargo.lock`、`rust-toolchain.toml`、`xtask/`、`crates/mklm-update/trust/`。意図しない変更があれば止める。
5. （任意）同じコミットを手元でビルドし、インストーラーのハッシュを比べる（再現可能なビルドかは確かめていない。違っても直ちに異常とは言えないので、比べた結果を記録するだけ）。
6. `cargo xtask prepare-release --tag vX.Y.Z --commit <SHA> --main-key-id <通常用の ID> --out release-work\vX.Y.Z`。表示された UTC の日付（発行と有効期限）と、取り残しの表と、2 つのインストーラーの SHA-256 を見る。ここで `xtask` がコンパイルされる（**鍵はまだつながない**）。
7. **ネットワークを切る**（Wi-Fi をオフ、ケーブルを抜く）。エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる。鍵をつないでいる間は、`minisign.exe` 以外を起動しない。
8. 通常用の USB メモリをつなぐ。`Get-FileHash E:\tools\minisign.exe` が記録した SHA-256 と一致することを確かめる。（任意）`Get-Content release-work\vX.Y.Z\latest.json` で版と 2 つの SHA-256 を表示し、手順 6 で見た値と一致することを確かめる。
9. 署名のコマンドを実行し、`minisign` にパスワードを入れる。`SIGN-OFFLINE.txt` の行を貼ってよいが、貼る前に、それが 1 行（副署名があれば 2 行）だけで、下の「固定の形」と、版と `issued_at` の値のほかは同じであることを目で確かめる（紛れ込んだ別のコマンド、たとえば `&` の後ろや 3 行目がないこと）。値を写し間違えても、`publish` が trusted comment の不一致で公開を止める。
10. USB メモリを外す。**ネットワークを戻す。**
11. `cargo xtask publish --tag vX.Y.Z --dir release-work\vX.Y.Z`。署名の検証（窓の中のすべての版の信頼の起点ファイルで）、アップロード、ファイルの組の確認、公開、公開後の確認（インストーラーのダウンロードを含む。CDN の反映を最大 5 分、30 秒ごとに待つ）が終わるのを待つ。
12. Actions の「Update canary」（公開で動く）が緑であることを確かめる。
13. `git switch main`。手順 6 の `--out` のフォルダーは消してよい（公開のデータだけ）。
14. 有効期限の日付の 2 か月前をカレンダーに入れる（公開のリポジトリの定期のワークフローは、60 日間活動がないと GitHub が止めるため、見張りだけに頼らない）。

### 固定の形（手順 9 で見比べる）

通常用の鍵（主署名）:

```
E:\tools\minisign.exe -S -s E:\mklm-keys\mklm-primary.key -m release-work\vX.Y.Z\latest.json -x release-work\vX.Y.Z\latest.json.minisig -t "mklm-latest-json v1 version=X.Y.Z issued_at=<数字>"
```

バックアップ用の鍵で署名するとき（鍵の移行の間の主署名か副署名）は、1 つ目の 2 つのパスを `F:\tools\minisign.exe` と `F:\mklm-keys-backup\mklm-backup.key` に替えた形。副署名は `-x release-work\vX.Y.Z\latest.json.alt.minisig`。`-t` の中身は `trusted-comment.txt` と同じ。

鍵の点検（6 章）:

```
F:\tools\minisign.exe -S -s F:\mklm-keys-backup\mklm-backup.key -m <out>\nonce.bin -x <out>\nonce.bin.minisig -t "mklm-key-drill v1"
```

### `prepare-release` が確かめること（設計 B.3）

1. タグが `vX.Y.Z`（プレリリースは拒否）。
2. 手元の `HEAD` とタグ `vX.Y.Z` が `--commit` と一致し、追跡しているファイルに変更がない。
3. GitHub のタグ（注釈付きならたどる）のコミットが `--commit`。
4. 下書きで、プレリリースでなく、ファイルがちょうど 2 つのインストーラーと `SHA256SUMS`。3 つを `<out>\assets\` にダウンロードする。
5. 2 つのインストーラーの SHA-256 と大きさが、`SHA256SUMS`（余分な行や重複は拒否）と GitHub のアセットの `digest` の両方と一致する。
6. 来歴の証明: `gh attestation verify <file> --repo SHIN-DATA-CENTER/multi-keyboard-layout-manager --signer-workflow SHIN-DATA-CENTER/multi-keyboard-layout-manager/.github/workflows/release.yml --source-digest <commit> --source-ref refs/tags/vX.Y.Z --deny-self-hosted-runners`。
7. 信頼できる時刻: `gh api -i /` の `Date`。手元の時計との差が 5 分を超えれば拒否。`issued_at` はこの時刻、`expires` は `issued_at` + `--expires-days`（既定 180、上限 800）。
8. 公開中の `latest.json` を本番の経路で取り、それを出した版のタグの信頼の起点ファイルで検証する。`issued_at` が公開中のもの以下なら拒否（公開中のものが未来の日付の事故なら `--published-misdated`）。公開中のものがない（404）のは、信頼の起点ファイルを持つ過去のタグがないときだけ。
9. 失効の引き継ぎ: `revoked_keys` = このタグの信頼の起点ファイルの `revoked` ∪ 公開中の `revoked_keys` ∪ `--revoke`。
10. 取り残しの検査: 窓の中の版（「最新」と、後継の公開から 400 日たっていない版）とこのタグ自身の信頼の起点ファイルで、主署名の鍵か副署名の鍵の少なくとも一方が「知っていて、失効しておらず、署名した鍵そのものを失効させていない」こと。表にして表示する。受け付けない版があれば拒否（`--allow-strand <tag>,...` で承知の上で続ける）。
11.〜14. `latest.json`（正準形）、`trusted-comment.txt`、`SIGN-OFFLINE.txt`、`prepare.json` を書き、要約を表示する。

### `publish` が確かめること

1. プレリリースのタグは拒否。`prepare.json` のタグ、コミット、下書きの状態、2 つのインストーラーの `digest` が、準備の時から変わっていない。
2. 署名の trusted comment が `trusted-comment.txt` と完全に一致する。`prepare.json` の窓の中の版（`--allow-strand` の版を除く）とこの版のすべての信頼の起点ファイルで、設計 C.3 の手順（主署名 → 必要なら副署名）で通る。どれかで通らなければ何も上げない。
3. `gh release upload`（`latest.json`、`latest.json.minisig`、あれば `latest.json.alt.minisig`）。
4. 下書きのファイルがちょうど 5 つ（副署名があれば 6 つ）で、上げたファイルの `digest` が手元のファイルと一致する。
5. `gh release edit vX.Y.Z --draft=false --latest --title "MKLM vX.Y.Z"`。
6. 公開後の確認（`verify --remote --installers` と同じ処理）。失敗すれば、4 章の「リリースの事故」の手順を表示して終了コード 1。

---

## 4. プレリリースとリリースの事故

**プレリリース**（例: `v0.3.0-rc.1`）

- タグを push すると、release.yml は `--prerelease` 付きの下書き（題「MKLM v0.3.0-rc.1 (pre-release, no auto-update)」）を作る。
- 署名しない。`latest.json` を付けない。`xtask publish` は使わない（拒否する）。
- 公開は `gh release edit v0.3.0-rc.1 --draft=false --prerelease`。**`--latest` は決して付けない**。

**リリースの事故（runbook）**

| 事態 | すぐにすること | その後 |
|---|---|---|
| `latest.json` か署名なしで、または間違った `latest.json` で公開してしまった | `gh release edit vX.Y.Z --prerelease`（「最新」から外れ、直前のリリースが「最新」に戻る）→ `cargo xtask verify --remote` で直前の版が配られていることを確かめる → リリースノートの先頭に「この版は使わないでください」 | X.Y.(Z+1) を通常の手順で出す（タグは再利用できない） |
| 署名したインストーラーが意図しないものだった（後で分かった） | 上と同じ。加えて、鍵の漏れの疑いがあれば 7 章 | 原因を調べ、修正版を出す。Issue で知らせる |
| 見張り（`update-canary.yml`）が失敗した | Actions のログで理由を見る（更新情報がない、署名が通らない、tag が合わない、期限まで 60 日未満、インストーラーのハッシュ違い、配られている版が公開済みの最新の安定版より古い） | 期限なら保守リリースを出す。GitHub の配布の変更なら、クライアントの修正版を出し、手で入れてもらう案内。「最新」の印が外れていれば、印を戻すか次の版を出す |

- クライアントの側では、ハッシュが違えばダウンロードで止まり、形式が違えば「更新を確認できません」になるだけで、害はない。
- 事故の手順で「最新」の印を外した後は、次の版を出すまで見張りが失敗し続ける（それでよい）。

---

## 5. 鍵の保管と、署名に使う PC

- **通常用の秘密鍵**: 暗号化した USB メモリ（BitLocker To Go）に、公式の `minisign.exe` と一緒に置き、署名するときだけつなぐ。パスワードはパスワード マネージャー A に置く。
- **バックアップ用の秘密鍵**: 通常用とは別の媒体で、別の場所に置く（例: 自宅の金庫と別の建物）。`minisign.exe` の写しも同じ媒体に置く。日常では使わない。**パスワードは、通常用のパスワードとは別の保管場所に置き、バックアップ用の媒体と同じ場所にも置かない**（例: 紙に書いて封をし、媒体とは別の金庫）。
- 秘密鍵をクラウドの同期フォルダー、GitHub、CI、開発機のディスクに置かない。`xtask` は秘密鍵を開かない。
- **バックアップ用の鍵が漏れることは、通常用の鍵が漏れることと同じかそれ以上に重い**（攻撃者は GitHub への書き込みと合わせて、古い版に悪意のある更新を入れられ、通常用の鍵を失効させることもできる）。

**署名に使う PC の条件**（J-6 の決定 (a): 普段の開発機の、普段のアカウントで署名する）

- 署名の間はネットワークを切る。鍵をつないでいる間は、`minisign.exe` 以外を実行しない（エディター、ブラウザー、`cargo` を動かしているターミナルは先に閉じる）。
- F.6 のデバッグ ビルドの MKLM が入っていないこと、開発用の鍵（`%USERPROFILE%\mklm-dev-keys\`）が残っていないこと。
- 秘密鍵に触れるのは、鍵の媒体に置いた、ハッシュを確かめた公式の `minisign` だけ。
- Windows と Defender が最新であること。

**残る危険**（2026-09-29 にユーザーが受け入れた。設計 B.6、G.7 の 21）: `xtask` は鍵に触れないが、`cargo xtask prepare-release` は、タグの `xtask` と `Cargo.lock` のすべてのビルド スクリプトと proc-macro を、メンテナーの権限でコンパイルして実行する。ふだんの開発でも毎日同じことが起きる。侵された依存のクレートが利用者の権限で常駐するプログラムを仕込めば、数分後につないだ USB メモリの鍵のファイルを写し、`minisign` に打ち込むパスワードを記録し、ネットワークが戻った後に送り出せる。ネットワークを切るだけでは防げない。起きた場合は、来歴の証明、公開後の検査、見張りで後から見つけ、7 章の手順で鍵を替える。危険を下げたくなったときの安い順の対策（今は採らない）:

1. 署名専用の Windows のアカウント（標準ユーザーで、cargo も git も一度も実行しない。署名の前に開発用のアカウントから**サインアウト**する）。
2. 署名専用の PC、または毎回きれいな状態から起動する仮想マシンやライブ USB で署名する。
3. `prepare-release` を別の PC で実行し、公開のデータ（`latest.json`、`trusted-comment.txt`、`SIGN-OFFLINE.txt`）だけを署名する PC に運ぶ。

---

## 6. 鍵の点検（年に 1 回、バックアップ用。通常用の鍵を新しくした後にも）

1. `cargo xtask key-drill start --role backup --out drill-2027`（ネットワークはつないだまま、鍵はつながない）。32 バイトの乱数の `nonce.bin` と、`SIGN-OFFLINE.txt` ができる（同じフォルダーで 2 回目は拒否する。点検ごとに新しいフォルダーを使う）。
2. ネットワークを切る → エディター、ブラウザー、`cargo` を動かしているターミナルを閉じる → バックアップ用の媒体をつなぐ → `minisign.exe` の SHA-256 を確かめる → 手順 1 の `SIGN-OFFLINE.txt` のコマンド（3 章の「固定の形」と同じであることを目で確かめてから）を実行してパスワードを入れる → 外す → ネットワークを戻す。
3. `cargo xtask key-drill check --dir drill-2027 --role backup` → 「… signed by the backup key <ID> that every release in the window embeds …」。窓の中のどれかの版が別のバックアップ用の鍵を埋め込んでいれば（古い鍵のファイルを取り違えた）、署名が前回の点検の `nonce.bin` のものなら、trusted comment が `mklm-key-drill v1` でなければ、失敗する。何も書かない。
4. 下の「点検の記録」に、日付、鍵 ID、結果を 1 行足す。

これで確かめられること: 媒体が読める、パスワードを覚えている、**その鍵が出荷したビルドに埋め込んだバックアップ用の鍵そのものである**。

---

## 7. 鍵が漏れたとき、なくしたとき

前提: 利用者の版は、その版の信頼の起点ファイルの鍵しか知らない。古い版が新しい版の更新情報を受け付けるには、主署名か副署名のどちらかが、古い版の知っている失効していない鍵のものでなければならない。`prepare-release` の取り残しの検査が、これを版ごとに確かめる。以下、P1 / B1 を今の通常用 / バックアップ用、P2 / B2 を新しい鍵とする。

| 事態 | 次の版 N の信頼の起点ファイル | N の `latest.json` の署名 | N+1 以降の署名 | 取り残される利用者 | 利用者の側の危険 |
|---|---|---|---|---|---|
| 通常用 P1 が漏れた（疑いを含む） | primary P2、backup B1、revoked P1 | 主: B1、副: P2。`revoked_keys` に P1（B1 が P1 を失効させる） | 主: P2、副: B1。移行の窓の間（N の公開の日から 400 日）、取り残しの検査が副署名を求めなくなるまで続ける。`revoked_keys` は P1 を引き継ぐ | 移行の窓の間に 1 度も確認しなかった PC（400 日以上オフライン）: 手で入れ直す | 失効の告知を受け取る前に、攻撃者が P1 の署名と GitHub への書き込みの両方を手に入れていれば、その PC には悪意のある更新を入れられる。受け取った後は P1 の署名を拒む |
| 通常用 P1 をなくした、パスワードを忘れた（漏れてはいない） | primary P2、backup B1、revoked P1（念のため） | 主: B1、副: P2 | 上と同じ | 上と同じ | なし |
| バックアップ用 B1 が漏れた | primary P1、backup B2、revoked B1 | 主: P1。`revoked_keys` には B1 が入るが、B1 を埋め込んだ古い版は、通常用の P1 の署名の中のそれを無視して N を受け付ける。B1 を拒むのは N を入れた版から | 主: P1。`revoked_keys` は B1 を引き継ぐ | なし（ただし下の攻撃を受けた PC は手で入れ直す） | **通常用の漏れと同じかそれ以上**: 攻撃者は GitHub への書き込みと合わせて、古い版に B1 で署名した悪意のある更新を入れられ、P1 を失効させて正しい更新を止めることもできる。N を入れた PC だけが B1 を拒む。README、リリースノート、Issue で早く N を入れるよう知らせる |
| バックアップ用 B1 をなくした | primary P1、backup B2、revoked B1（念のため） | 主: P1 | 主: P1 | なし | なし |
| 両方が漏れた、または両方をなくした | 新しい P2、B2 | 主: P2（古い版は検証できない） | 主: P2 | **すべての利用者**が手で入れ直す | 漏れた場合は、手で入れ直すまで、攻撃者（鍵と GitHub への書き込みの両方）は古い版に悪意のある更新を入れられる。README、リリースノート、Issue で「手で入れ直してください」 |
| 日付の誤った（未来の）更新情報を公開した | 変えない | — | 次の版を正しい日付でふつうに出す（`prepare-release --published-misdated`） | なし（クライアントは記録を受け取った時刻で抑えている） | なし。期限切れの表示が一時的にずれるだけ |
| `latest.json` なし、または間違ったもので公開した | 変えない | — | 4 章の「リリースの事故」 | 事故の間に確認した PC は、次の確認で戻る | なし（形式か署名で止まる） |

- 通常用の鍵を替えたら、移行の窓の間はバックアップ用の鍵をリリースのたびに使う（副署名）。窓が明けたら、バックアップ用の鍵も新しくする（P2 が主署名で、信頼の起点ファイルを「P2、B2、revoked P1、revoked B1」にした版を出す）ことを勧める。
- 失効の規則（設計 B.2）: 署名した鍵そのものを失効させる更新情報は拒否される。通常用の鍵の署名の中の、埋め込みのバックアップ用の鍵の失効と、埋め込みにない ID の失効は、無視される（記録されない）。バックアップ用の鍵を失効させるのは、新しい版の信頼の起点ファイル（`revoked` の行）だけ。

---

## 8. 記録

### 公式の `minisign` の SHA-256

`cargo xtask verify-signer` が表示した値を書く。署名の前（3 章の手順 8、6 章の手順 2）に `Get-FileHash` で照らす。

| 日付 | ファイル | SHA-256 | 確かめた人 |
|---|---|---|---|
| （未記入） | `minisign-0.12-win64.zip` | （`verify-signer` の出力） | |
| （未記入） | `minisign.exe`（zip の中） | （`verify-signer` の出力） | |

### 埋め込んだ鍵

| 役割 | 鍵 ID | 指紋（`pubkey-line` の出力） | 最初に入れた版 | 失効させた版 |
|---|---|---|---|---|
| primary | （未作成） | | | |
| backup | （未作成） | | | |

### 鍵の点検の記録

| 日付（UTC） | 役割 | 鍵 ID | 結果 | 備考 |
|---|---|---|---|---|
| （未実施） | backup | | | |

### `minisign-verify` のソースの読み合わせ（設計 G.6。v0.2.0 の前に 1 回、版を上げるたびに差分）

| 日付 | 版 | 読んだ範囲 | 気になった点 | 読んだ人 |
|---|---|---|---|---|
| 2026-09-29 | 0.3.0（crates.io の `.crate` の SHA-256 `871285dc…a960ce` = `Cargo.lock` の checksum） | パッケージのすべてのファイル: `Cargo.toml`、`src/lib.rs`、`src/base64.rs`、`src/crypto/{mod,ed25519,curve25519,sha512,blake2b,cryptoutil}.rs` | MKLM の使い方では問題になる点なし。注意点は下のメモ（署名ファイルの読み方が緩い、公開鍵の点の検査が弱い、定数は数値として照らしていない） | Claude Opus 5.5（AI。M5b の実装のセキュリティ レビュー、SECURITY-3） |

2026-09-29 の読み合わせのメモ（上の行の詳細。版を上げるときは、この版からの差分を読む）:

- **形**: 依存なし、ビルド スクリプトなし（`build = false`）、proc-macro なし、`unsafe` なし（`base64.rs` は `#![forbid(unsafe_code)]`、ほかのファイルにも `unsafe` と `extern` がない）。ネットワーク、プロセス、環境変数を使わない。ファイルを読むのは `PublicKey::from_file` と `Signature::from_file` だけで、MKLM は呼ばない（`from_base64` と `decode` だけ）。`Cargo.toml` の `[profile.release]`（`panic = "abort"` など）は、依存として使うときは効かない。
- **`Signature::decode`**（`lib.rs`）: `str::lines()` で 4 行を読み、5 行目以降は読まない。1 行目（untrusted comment）は中身を確かめない。2 行目は base64 を解いて 74 バイト、4 行目は 64 バイトであること、3 行目が `trusted comment: `（17 バイト）で始まること（`trusted_comment()` の `[17..]` はこの確認があるので文字の境目で切れる）。アルゴリズムは `Ed`（legacy）と `ED`（prehashed）だけ。**緩い**: 行の数、改行の形、untrusted comment の形を問わない。MKLM は先に `mklm-update` の `parse_signature_text` で厳密に読み（ちょうど 4 行、正準の base64、`ED` だけ）、2 つの解析の trusted comment が一致することを確かめている（`verify.rs` の手順 2）。
- **`base64.rs`**: 標準の文字の並び。パディングの数が合わないもの、余りのビットが 0 でないもの（非正準）、途中の空白や改行を拒む。文字の判定は分岐のない形。
- **`PublicKey::verify`**: 鍵 ID を比べ（違えば `UnexpectedKeyId`）、prehashed なら BLAKE2b-512 の 64 バイトに Ed25519 を確かめ、`allow_legacy = false` なら legacy を `UnexpectedAlgorithm` で拒む。続けて、全体の署名を「署名の 64 バイト + trusted comment（`trusted comment: ` を除いた部分）」について確かめる。全体の署名は鍵 ID とアルゴリズムの 2 バイトを含まないが、鍵 ID は別に比べ、legacy への書き換えは `allow_legacy = false` で拒まれる。MKLM が `allow_legacy = true` にするのは `xtask verify-signer`（公式の minisign の配布物）だけ。
- **`crypto/ed25519.rs`**: `s < L` を確かめ（署名の改変の防止）、単位元の公開鍵と全部 0 の公開鍵を拒む。`R` は点として解かず、`[s]B - [h]A` の正準の符号化とバイトで比べる（非正準の `R` は通らない）。cofactor を掛けない検証（ref10 と同じ）。**弱い点**: 公開鍵の `y` が正準か（`y < p`）と、単位元以外の位数の小さい点を拒まない。MKLM の公開鍵はビルドに埋め込んだ信頼の起点だけで、攻撃者が選べないので影響しない。比較は定数時間に近い形だが、検証は公開のデータしか扱わないので時間の差は問題にならない。
- **`crypto/curve25519.rs`**: 体の演算は fiat-crypto が生成した 51 ビットの 5 語の形（`carry_mul`、`carry_square`、`carry`、`add`、`sub`、`opp`、`to_bytes`）。点の演算、`from_bytes_negate_vartime`、`slide`、`double_scalarmult_vartime`、`sc_reduce`、`is_identity` は ref10 の移植。どれも可変時間（公開のデータだけなので問題ない）。`Fe::from_bytes` は長さが 32 でなければ panic するが、呼ぶのは 32 バイトの配列だけ。
- **`crypto/sha512.rs`、`crypto/blake2b.rs`、`crypto/cryptoutil.rs`**: SHA-512（80 の定数、128 ビットの長さの上位は 0）と BLAKE2b（12 回、`SIGMA` の 11、12 行目は 1、2 行目と同じ、ダイジェスト 64 バイト、鍵なし）の素直な実装。長さの数え方があふれるのは 2^61 バイト以上で、届かない。
- **確かめていないこと**: 定数（`FE_D`、`FE_D2`、`FE_SQRTM1`、基点の倍数の表 `BI`、SHA-512 の定数、BLAKE2b の IV と `SIGMA`）を 1 つずつ計算し直してはいない。代わりに、クレート自身のテスト（公式の minisign の署名の legacy と prehashed。`cargo test -p minisign-verify --lib`、overflow の検査のある debug で 5 件が通る）と `crates/mklm-update/tests/crate_apis.rs`（`minisign` 0.10.0 で作った使い捨ての鍵の署名が通り、改変が拒まれる）が通ることで、実際の署名で正しく動くことを確かめた。定数の誤りは正しい署名を通さなくする向きに出るので、テストが通る限り偽造を易しくする形では残りにくい。
- 読んだ人は AI（Claude）で、この記録はメンテナー自身の読み合わせの代わりにはならない。メンテナーが自分でも読むなら、この表に行を足す。

WP-U（2026-09-29）が実装のために読んだ範囲のメモ（レビューの読み合わせの代わりではない）: `Signature::decode` は 4 行を `lines()` で読み（5 行目以降は読まない）、2 行目の 74 バイトと 4 行目の 64 バイトを確かめ、アルゴリズムが `Ed`（legacy）と `ED`（prehashed）以外なら拒む。`PublicKey::verify` は鍵 ID を比べ、prehashed なら BLAKE2b-512 を署名し、`allow_legacy = false` なら legacy を `UnexpectedAlgorithm` で拒む。署名と、署名 + trusted comment の全体の署名の 2 つを Ed25519 で確かめる。鍵 ID は署名の対象に含まれない（`mklm-update` は鍵 ID で鍵を選んだ後、その鍵で検証する）。`mklm-update` は `Signature::decode` の前に、署名ファイルを自分で厳密に読む（ちょうど 4 行、正準形の base64、prehashed だけ）。
