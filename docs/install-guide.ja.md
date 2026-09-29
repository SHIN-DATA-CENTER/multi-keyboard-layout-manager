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
Get-FileHash .\MKLM-Setup-0.1.0-x64.exe -Algorithm SHA256
```

表示された値（小文字にしたもの）が、`SHA256SUMS` の同じファイル名の行と一致すれば問題ありません。

## インストール

MKLM は、まだコード署名をしていません。そのため、次のような警告が出ます。

1. **ブラウザーの警告**（Edge で「一般的にダウンロードされていません」など）: 「…」→「保存」→「保持」を選びます。
2. **SmartScreen**（「Windows によって PC が保護されました」）: 「詳細情報」を押し、「実行」を選びます。
3. **UAC**（「このアプリがデバイスに変更を加えることを許可しますか？」）: 発行元が「不明な発行元」と表示されます。プログラム名が `MKLM-Setup-…exe` であることを確かめて、「はい」を選びます。

「スマート アプリ コントロール」が有効な PC では、署名のないソフトは実行できません。MKLM が署名されるまでは使えません。

インストール先は `C:\Program Files\SHIN DATA CENTER\MKLM` で、スタートメニューに「Multi Keyboard Layout Manager」が追加されます。最後の画面で「MKLM を起動する」にチェックを入れると、そのまま起動します。

すでにインストールしてある場合は、新しいインストーラーを実行するだけで上書き更新できます。動いている MKLM は自動的に終了します。設定と変更の記録は、そのまま引き継がれます。

古い版のインストーラーで入れ直す（ダウングレードする）のは、PC の再起動を待っている変更がないときだけにしてください。再起動を待っている間に古い版に戻すと、古い版は、まだ反映されていない変更を「再起動した後」と誤って扱うことがあります。

## 最初に起動したとき

初回はセットアップのウィザードが開きます。キーボードごとに JIS か US かを決め、必要なら PC を 1 回再起動します。詳しい使い方は、アプリの画面の説明に従ってください。

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
| 画面の設定（テーマ、言語など） | `%APPDATA%\SHIN DATA CENTER\MKLM\settings.toml` |
| 変更の記録（ジャーナル） | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal` |
| 復旧用のファイル | `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` |
| サインイン時の起動 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` の `SHINDATACENTER.MKLM` |

困ったときは、同じフォルダーにある `recovery.md`（キーボードが思いどおりに動かないときの戻し方）を見てください。

---

# Installing MKLM (English summary)

- Requires Windows 11 24H2 (build 26100) or later, x64 or ARM64, and administrator rights.
- Download `MKLM-Setup-<version>-x64.exe` (Intel/AMD) or `-arm64.exe` from GitHub Releases. Optionally compare it with `SHA256SUMS`.
- MKLM is not code-signed yet. Keep the download in the browser, choose "More info → Run anyway" in SmartScreen, and "Yes" in UAC ("Unknown publisher"). It cannot run where Smart App Control is on.
- It installs into `C:\Program Files\SHIN DATA CENTER\MKLM` with a Start menu entry. Running a newer installer upgrades in place and keeps your settings. Do not go back to an older version while a change waits for a PC restart: the older version may take it for already restarted.
- Uninstall from Settings → Apps. You are asked whether to put the keyboard settings back to how they were before MKLM; the journal and the recovery files stay.
