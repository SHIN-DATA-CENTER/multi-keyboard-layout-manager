# MKLM のインストール

Multi Keyboard Layout Manager（MKLM）は、日本語配列（JIS）と英語配列（US）のキーボードを 1 台の PC で使い分けるためのソフトです。

## 動く環境

- Windows 11 24H2（ビルド 26100）以降
- x64 版と ARM64 版がある
- インストールには管理者の権限が必要

## ダウンロード

[GitHub の Releases](https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases) から、PC に合ったインストーラーを選びます。

| PC | ファイル |
|---|---|
| ふつうの PC（Intel / AMD） | `MKLM-Setup-<版>-x64.exe` |
| ARM の PC（Snapdragon など） | `MKLM-Setup-<版>-arm64.exe` |

どちらか分からないときは、「設定 → システム → バージョン情報」の「システムの種類」を見てください。「ARM ベース プロセッサ」とあれば ARM64 版です。

### ファイルが正しいか確かめる（任意）

同じ Release にある `SHA256SUMS` と照らし合わせます。

```powershell
Get-FileHash .\MKLM-Setup-0.2.1-x64.exe -Algorithm SHA256
```

表示された値（小文字にしたもの）が、`SHA256SUMS` の同じファイル名の行と一致すれば問題ありません。

### 更新情報を手で確かめる（任意。v0.2.0 以降）

v0.2.0 以降の Release には、メンテナーがオフラインで署名した更新情報 `latest.json` と、その署名 `latest.json.minisig` があります。自動更新はこの 2 つを確かめてから動きますが、同じことを手でも確かめられます。`SHA256SUMS` だけでは、Release のファイルがまとめて差し替えられた場合に気付けないので、ほかの人の PC に入れるときなどは、こちらも確かめてください。

1. [minisign](https://jedisct1.github.io/minisign/) を用意します。
2. `latest.json`、`latest.json.minisig`、インストーラーを同じフォルダーにダウンロードします。
3. 署名を確かめます（`<公開鍵>` は下の表の値）。

   ```powershell
   minisign -Vm latest.json -x latest.json.minisig -P <公開鍵>
   ```

   `Signature and comment signature verified` と表示され、`Trusted comment:` が `mklm-latest-json v1 version=<版>` で始まれば正しい署名です。
4. `latest.json` の `assets` の、インストーラーと同じ名前の項目の `sha256` と、`Get-FileHash` の値（小文字にしたもの）が一致することを確かめます。

| 役割 | 鍵 ID | 公開鍵 |
|---|---|---|
| 通常用 | `D25C95894801CEE6` | `RWTmzgFIiZVc0ooRIP6vgNPGaN+OgLelDvccLF0MMqspl/oqiZTfHFrH` |
| バックアップ用 | `F65C01E2BA6A6156` | `RWRWYWq64gFc9uj4YnziIbenhebU5l1/aLlqyRNM++cDqcEkAK/L8v3b` |

鍵を替えている間は、`latest.json.alt.minisig`（副署名）もあります。どちらかの署名が上の表の鍵で確かめられれば正しい更新情報です。

## インストール

MKLM は、まだコード署名をしていません。そのため、次のような警告が出ます。

1. **ブラウザーの警告**（Edge で「一般的にダウンロードされていません」など）: 「…」→「保存」→「保持」を選びます。
2. **SmartScreen**（「Windows によって PC が保護されました」）: 「詳細情報」を押し、「実行」を選びます。
3. **UAC**（「このアプリがデバイスに変更を加えることを許可しますか？」）: 発行元が「不明な発行元」と表示されます。プログラム名が `MKLM-Setup-…exe` であることを確かめて、「はい」を選びます。

「スマート アプリ コントロール」が有効な PC では、署名のないソフトは実行できません。MKLM が署名されるまでは使えません。

インストール先は `C:\Program Files\SHIN DATA CENTER\MKLM` で、スタートメニューに「Multi Keyboard Layout Manager」が追加されます。最後の画面で「MKLM を起動する」にチェックを入れると、そのまま起動します。

すでにインストールしてある場合は、新しいインストーラーを実行するだけで上書き更新できます。動いている MKLM は自動的に終了します。設定と変更の記録は、そのまま引き継がれます。

古い版のインストーラーで入れ直す（ダウングレードする）のは、PC の再起動を待っている変更がないときだけにしてください。再起動を待っている間に古い版に戻すと、古い版は、まだ反映されていない変更を「再起動した後」と誤って扱うことがあります。

v0.1.x には自動更新がありません。v0.2.0（自動更新に対応した最初の版）は、この手順で手でインストールしてください。v0.2.0 からは、次の「自動更新」で新しい版を入れられます。

## 自動更新（v0.2.0 以降）

- MKLM は **1 日に 1 回、GitHub で新しい版があるかを確かめます**（起動の 1〜3 分後と、その後は約 24 時間ごと）。送るのは MKLM の版と PC の種類（x64 / ARM64）だけで、利用者や PC を識別する情報は送りません。設定の「更新」欄でオフにできます（オフのときは、［今すぐ確認］を押したときだけ確かめます）。
- 新しい版があれば、**インストーラーを自動でダウンロードし**、メンテナーの署名と SHA-256 で確かめます（従量制の接続でもダウンロードします。数 MB です）。確かめられなかったものは使いません。
- **インストールは、あなたが更新のページで［今すぐ更新］を押したときだけ**行います。［この版をスキップ］［後で］も選べます。
- 動いている MKLM と同じ種類（x64 版なら x64 版）のまま更新します。ARM の PC で x64 版を使っている場合も、自動では ARM64 版に切り替えません。

### ［今すぐ更新］の後に出る Windows の確認（UAC）

MKLM はまだコード署名をしていないので、UAC の確認画面の発行元は「不明」と表示されます。ほかのプログラムも同じ見た目の画面を出せるので、「はい」を押す前に次を確かめてください。

1. 「**詳細を表示**」を押し、**プログラムの場所**が `C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe` であることを確かめます（Program Files が C: 以外にある PC では、そのドライブの同じ場所）。違う場所なら「いいえ」を押します。
2. **MKLM が許可を求めるのは、あなたが［今すぐ更新］（またはキーボードの設定の変更）を押した直後だけ**です。何も押していないのに出た確認には「いいえ」を押してください。

「はい」を押すと、MKLM はいったん終了し、更新が終わると 1 分ほどで自動で開きます。開き直すまで、サインアウトや再起動はしないでください。2 分たっても開かない場合は、スタート メニューから MKLM を開いてください。更新はキーボードの設定を変えません。

- 別のユーザーが同じ PC で MKLM を開いたまま何もしていない場合（ユーザーの切り替え）、その MKLM は終了してから更新されます。そのユーザーの MKLM は、そのユーザーが開き直すか、次にサインインするまで動きません。そのユーザーの MKLM が何かの途中（キーボードの変更の確認待ちなど）なら、更新はしません。
- 更新できなかった場合は、理由と次の手順が画面に出ます。困ったときは `recovery.md` の「更新が途中で止まったとき」「更新の後に MKLM が起動しないとき」を見てください。

### 更新情報の有効期限

更新情報には、発行から 180 日の有効期限があります。期限を過ぎると、設定の「更新」欄と更新のページに「更新情報の有効期限を過ぎています」と出ます。新しい版が長く公開されていないか、古い情報が届いているかのどちらかです。GitHub のリリース ページで新しい版を確かめてください。期限を過ぎていても、正しく署名された新しい版のインストールは止まりません。キーボードの機能にも影響しません。

### 自動更新が使えない場合

- **Windows の統合認証（NTLM / Kerberos）を求めるプロキシ**の内側では、自動更新は使えません。MKLM は、Windows の資格情報をプロキシにもサーバーにも送らないためです（同じネットワークの攻撃者に資格情報を渡さないため）。[GitHub の Releases](https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases) から新しいインストーラーをダウンロードし、手で実行してください。認証の要らないプロキシ、PAC、社内の証明書（TLS の検査）は、Windows の設定のまま使えます。
- 30 日以上確かめられていない場合は、メイン画面に 30 日に 1 回お知らせが出ます。

### ほかの人の PC で、管理者として更新を承認するとき

標準ユーザーの PC で MKLM の更新やインストーラーの実行を承認する（UAC の画面で管理者の資格情報を入れる）ときは、その PC の画面に出た確認を信用せず、**あなた自身がリリース ページからダウンロードし、`SHA256SUMS` と署名（上の「更新情報を手で確かめる」）で確かめたインストーラー**を実行してください。利用者のフォルダーにあるファイルは、その利用者（やその利用者の権限で動くプログラム）が差し替えられます。更新のページの［インストーラーを実行］も、利用者のフォルダーのインストーラーを使います。

## コマンドラインで更新を確かめる

`mklm-cli` は、更新を確かめることと、状態の表示だけを行います（インストールはしません）。スクリプトや管理ツールから入れるときは、下の「サイレント インストール」でインストーラーを直接実行してください。

| コマンド | 内容 |
|---|---|
| `mklm-cli update --check [--json]` | GUI と同じ確認をします（ダウンロードもインストールもしません）。有効期限切れや、古い更新情報を無視したときは警告を 1 行出します |
| `mklm-cli update --status [--json]` | 最後の更新の結果、進行中の更新、インストールされているファイルの版、最後の確認とその結果を表示します |

`update --check` の終了コード:

| コード | 意味 |
|---|---|
| 0 | 最新です |
| 20 | 新しい版があります（MKLM を開いてインストールしてください） |
| 21 | 手で更新する必要があります（リリース ページから） |
| 22 | このビルドでは自動更新を使えません |
| 6 | 更新の途中です。しばらくしてからもう一度実行してください |
| 1 | 確認できませんでした |
| 2 | 使い方の誤り |

`update --status` の終了コードは 0（表示した）、1（表示を書き出せなかった）、2（使い方の誤り）です。読めなかった記録があっても終了コードは 0 のままで、読めなかったものを標準エラーに `warning:` の行で出します（`--json` では `warnings` にも入ります）。監視のスクリプトで読めなかったことを知るには、`warnings` が空でないことを確かめてください。

## サイレント インストール（管理者向け）

インストーラーに `/S` を付けると、画面を出さずにインストール（上書き更新）します。管理者の権限が必要です。

```powershell
Start-Process .\MKLM-Setup-0.2.1-x64.exe -ArgumentList '/S' -Verb RunAs -Wait -PassThru | Select-Object ExitCode
```

| 終了コード | 意味 | 何か置き換えたか |
|---|---|---|
| 0 | 成功 | はい |
| 1 | 利用者が取り消した（`/S` では起きない） | いいえ |
| 2 | そのほかの中止 | いいえ |
| 20 | Windows 11 24H2（ビルド 26100）より古い | いいえ |
| 21 | ARM64 のインストーラーを ARM64 以外の PC で実行した | いいえ |
| 22 | `mklm-helper.exe` が動いている | いいえ |
| 23 | `mklm-cli.exe` が動いている | いいえ |
| 24 | `mklm.exe` が終了しなかった | いいえ |
| 26 | 別のプログラムが MKLM のファイルを開いていて、置き換えられなかった | いいえ |
| 27 | 新しいファイルを書き込めなかった（ディスクの空き、ウイルス対策ソフト） | いいえ |

アンインストーラーは、PC の再起動が必要なときに 3010 を返します。

## 最初に起動したとき

初回はセットアップのウィザードが開きます。キーボードごとに JIS か US かを決め、必要なら PC を 1 回再起動します。詳しい使い方は、アプリの画面の説明に従ってください。

### PC の標準配列を変える（v0.2.0 向け）

キーボードごとモードでは、キーボード一覧の［PC の標準配列を変更…］から JIS / US を選べます。標準に従っているキーボードは、既定では今の配列に固定され、再起動後もその配列を保ちます。新しい標準に従わせたいキーボードだけを選んでください。固定モードでは、初回の移行で標準配列を選びます。

CLI で下見だけをするには `mklm-cli standard jis --dry-run`、適用するには `mklm-cli standard jis --no-reset` を使います。`--follow "<インスタンス ID>"` を繰り返して、新しい標準に従わせるキーボードを指定できます。USB をその場でリセットでき、別の入力手段がある場合は `--other-input` を選べます。`--yes` には `--other-input` か `--no-reset` が必須で、行番号 `#n` は使えません。

変更後は **PC を再起動**してください。サインアウト・ユーザーの切り替え・シャットダウンでは代用できません。再起動前に新しくサインインすると、配列が早めに変わる可能性があります（実機では未確認）。PIN など別のサインイン方法を用意してください。取り消しは `mklm-cli undo`（確定後は `mklm-cli revert <操作 ID>`）を実行して再起動します。詳しくは[復旧ガイド](recovery.md)を参照してください。

### リモート デスクトップで使うとき

RDP の画面で MKLM を動かした場合、設定を変えるのは **接続先の PC（その画面の PC）** です。接続元の PC（手元の PC）で動く MKLM が書くのは接続元の設定です。両方の MKLM が設定を同期する機能はありません。

RDP のセッションのキー配列は 1 つで、接続元の JIS / US キーボードを別々には扱えません。接続先の標準配列と接続元の報告がどう影響するか、変更がいつ反映されるかは未確認です。接続元の MKLM で移行や標準配列の変更をした場合に報告が変わるかも未確認なので、変更後は接続先の `mklm-cli list` の報告と、セッション内の Shift+2 を確認してください。

英数キーで切り替わらない場合は、接続先の Microsoft IME の「キーとタッチのカスタマイズ」で Ctrl+Space に「IME-オン/オフ」を割り当てる方法があります。[RDP の調査](research/rdp-keyboard.md)と[連携の設計](design/rdp-link.md)に、観察したことと未確認事項をまとめています。

## アンインストール

「設定 → アプリ → インストールされているアプリ」で「Multi Keyboard Layout Manager」を選び、「アンインストール」を押します。

途中で「キーボードの設定を、MKLM を使い始める前の状態に戻しますか？」と聞かれます。

| 選択 | 結果 |
|---|---|
| はい | MKLM が変更した値を元に戻します。外部で変更された値はそのままにします。外付けのキーボードは、次に抜き差し（Bluetooth は再接続）したときに元の配列になります。内蔵キーボードや全体の設定を戻した場合は、最後の画面で PC の再起動を勧められます |
| いいえ | 今の設定のまま、MKLM だけを削除します |

次のものはアンインストールしても残ります。

- 変更の記録（`HKLM\SOFTWARE\SHIN DATA CENTER\MKLM`）
- 復旧用のファイル（`%ProgramData%\SHIN DATA CENTER\MKLM\Recovery`）

## 設定の保存場所

| もの | 場所 |
|---|---|
| 画面の設定（テーマ、言語、自動更新のオン / オフなど） | `%APPDATA%\SHIN DATA CENTER\MKLM\settings.toml` |
| 変更の記録（ジャーナル） | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal` |
| 復旧用のファイル | `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` |
| サインイン時の起動 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` の `SHINDATACENTER.MKLM` |
| ダウンロードした更新（利用者ごと） | `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\`（`state.json`、`latest.json`、インストーラー） |
| 更新の記録（PC 全体。helper だけが書く） | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update`（`Trust`、`Run`、`LastResult`） |
| 更新の作業フォルダーとログ | `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\`、`%ProgramData%\SHIN DATA CENTER\MKLM\logs\update.log` |
| 更新の後に開き直すための登録（1 回だけ） | `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` の `SHINDATACENTER.MKLM.AfterUpdate` |

困ったときは、同じフォルダーにある `recovery.md`（キーボードが思いどおりに動かないときの戻し方）を見てください。

---

# Installing MKLM (English summary)

- Requires Windows 11 24H2 (build 26100) or later, x64 or ARM64, and administrator rights.
- Download `MKLM-Setup-<version>-x64.exe` (Intel/AMD) or `-arm64.exe` from GitHub Releases. Optionally compare it with `SHA256SUMS`.
- MKLM is not code-signed yet. Keep the download in the browser, choose "More info → Run anyway" in SmartScreen, and "Yes" in UAC ("Unknown publisher"). It cannot run where Smart App Control is on.
- It installs into `C:\Program Files\SHIN DATA CENTER\MKLM` with a Start menu entry. Running a newer installer upgrades in place and keeps your settings. Do not go back to an older version while a change waits for a PC restart: the older version may take it for already restarted.
- Uninstall from Settings → Apps. You are asked whether to put the keyboard settings back to how they were before MKLM; the journal and the recovery files stay.
- In per-keyboard mode, use "Change the PC's standard layout…" on the keyboard list, or preview `mklm-cli standard jis --dry-run` and apply with `mklm-cli standard jis --no-reset` (for v0.2.0). Existing followers keep their layout after restarting unless selected with `--follow "<instance ID>"`. Restart after applying; signing in again before restarting may change the layout early (unverified). Keep another sign-in method ready. Undo with `mklm-cli undo` or, after confirmation, `mklm-cli revert <op>`, then restart.
- In Remote Desktop, MKLM edits the PC you connect to. MKLM on the PC you connect from edits only that PC; the two do not synchronize. The session uses one keyboard layout. How the host standard and client report determine it, and when changes apply, remain unverified. Check the client report and Shift+2 after changes. Ctrl+Space can be assigned to IME on/off in Microsoft IME settings on the PC you connect to. See the [RDP design](design/rdp-link.md) (Japanese).
- **Automatic updates (from v0.2.0; install v0.2.0 by hand once).** MKLM checks GitHub once a day (it sends only its version and x64/ARM64; you can turn this off in Settings), downloads a new installer automatically and verifies it against the maintainer's offline minisign signature of `latest.json` and its SHA-256. It installs **only when you press "Update now"**. The UAC prompt shows "Unknown publisher": click "Show more details" and check that the program location is `C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe`; MKLM asks for permission only right after you press a button. MKLM closes and reopens by itself; keyboard settings are not touched. It stays on the same architecture (no automatic x64 → ARM64 switch).
- Manifests expire 180 days after they are issued; an expired one only shows a notice. Behind a proxy that needs Windows integrated authentication, automatic updates do not work (MKLM never sends Windows credentials); download from the release page instead.
- To verify by hand: `minisign -Vm latest.json -x latest.json.minisig -P <public key>` (the keys are listed above once they exist), then compare the installer's SHA-256 with `assets[].sha256` in `latest.json`. When approving an update as an administrator on someone else's PC, download and verify the installer yourself.
- `mklm-cli update --check [--json]` exit codes: 0 up to date, 20 update available, 21 manual update required, 22 updates unavailable in this build, 6 an update is in progress, 1 check failed, 2 usage error. `mklm-cli update --status [--json]`: 0 shown, 1 the report could not be written, 2 usage error; records that could not be read keep the code at 0 and are reported as `warning:` lines on standard error (and in `warnings` with `--json`). The CLI never installs.
- Silent install: `MKLM-Setup-<version>-<arch>.exe /S` (elevated). Exit codes: 0 success, 1 cancelled, 2 aborted, 20 Windows too old, 21 wrong architecture, 22 / 23 / 24 helper / CLI / GUI still running, 26 files in use, 27 files could not be written; nothing was replaced for any code but 0.
