# M5a インストーラーの実機テスト

- 実施日: 2026-09-29
- 開発機: Windows 11 Home 25H2（build 26200、x64）
- インストーラー: `dist\MKLM-Setup-0.1.0-x64.exe`。`m5/installer` の `299a313` でビルドし、NSIS 3.12 を使った。

## その 1: 新規インストール（合格）

- ユーザーの操作: インストーラーを実行 → UAC で「はい」→ ようこそ → インストール → 完了（「MKLM を起動する」にチェック）。
- ユーザーの報告: 画面は日本語。最後に MKLM のウィンドウが開いた。気になる表示はなかった。
- 確認できたこと:
  - `C:\Program Files\SHIN DATA CENTER\MKLM` に、`mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe`、`uninstall.exe`、`LICENSE.txt`、`recovery.md`、`install-guide.md` がある。
  - ACL は Program Files から継承しており、Users は RX だけ。
  - 「アプリ」一覧に `SHINDATACENTER.MKLM` がある（DisplayName、DisplayVersion 0.1.0、Publisher SHIN DATA CENTER、DisplayIcon、UninstallString、QuietUninstallString /S、NoModify、NoRepair、EstimatedSize）。
  - スタートメニュー（全ユーザー）に「Multi Keyboard Layout Manager.lnk」がある。
  - `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM` の下は、helper の `Journal` だけ（インストーラーは何も作らない）。
  - 起動した `mklm.exe`（Program Files）のトークンは**昇格していない**。Explorer 経由の起動が効いている。
  - インストール先の `mklm-cli.exe` は、同じフォルダーの `mklm-helper.exe` を使い、ビルド ID が一致した。
- 気付いた点: 開発用ビルド（`target\release\mklm.exe`）の MKLM が動いたままだった。exe の場所が違うので、多重起動の防止は別のアプリとして扱い、2 つが並んで動いた（設計どおり）。開発用ビルドは `mklm.exe --quit` で終了させた。

## その 2: 上書きでのアップグレード（合格）

- インストールした MKLM が動いている状態で、同じインストーラーを実行した。
- ユーザーの報告: 「MKLM が実行中です…［再試行］」の画面は出ず、自動で終了した。最後にもう一度ウィンドウが開いた。
- 確認できたこと:
  - `uninstall.exe` が上書きの時刻（00:08:16）に書き直された。他の exe の時刻は、元のビルドの時刻が残る（NSIS の仕様）。
  - 新しい `mklm.exe`（PID 8532、00:08:38 に起動）は昇格していない。

## その 3: アンインストール（合格）

- 実行前の状態: 「サインイン時の起動」の値（HKCU Run の `SHINDATACENTER.MKLM`）は、開発用ビルドを指していた。`restore --baseline --all --dry-run` は「Nothing to restore」だった（Keychron の baseline 4/0 が現在の値と同じ）。
- ユーザーの操作: 設定 → アプリ → アンインストール → UAC で「はい」→ 確認 →「キーボードの設定を…戻しますか？」で「はい」→ 完了。
- ユーザーの報告: 画面は日本語。復元の質問が出た。数秒で完了した。エラーはなかった。
- 確認できたこと:
  - `C:\Program Files\SHIN DATA CENTER` が親フォルダーごと消えた。
  - 「アプリ」一覧とスタートメニューのショートカットが消えた。
  - `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal` は残った（設計どおり）。
  - HKCU Run の `SHINDATACENTER.MKLM` は消えた（開発用ビルドを指していた値も消えるが、アンインストール後には意味がないので問題ない）。
  - `mklm*.exe` のプロセスは残っていない。
  - キーボードの値は変わらない（内蔵 7/2、Keychron 4/0）。ジャーナルに新しい操作はない（`--uninstall-restore` は NoChange で、何も記録しない）。

## 未実施

- ARM64 版のインストーラーの実機テスト（ARM64 の PC がない）
- 復元で実際に値が戻る場合（USB キーボードは再接続、内蔵と全体の値は再起動）のアンインストール
- `/S` のサイレント アンインストール、`QuietUninstallString`
