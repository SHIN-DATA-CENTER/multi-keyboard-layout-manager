# M0 検証結果

開発機: Windows 11 Home 25H2（build 26200）、Fujitsu ノート PC。検証は `tools/m0/` のスクリプトで行う。

## 実験前の状態（2026-09-27）

| キーボード | ドライバー | Raw Input の種別 | デバイス側の override |
|---|---|---|---|
| 内蔵 `ACPI\FUJ0309\4&320DB4C2&0`（JIS） | i8042prt | 0x7 / 0x2 | なし |
| Keychron Receiver `HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000`（US） | kbdhid | 0x51 / 0x0 | なし |
| VXE R1SE+（BLE マウスのキーボード機能）COL02 | kbdhid | 0x51 / 0x0 | なし |

- 全体設定（`i8042prt\Parameters`）: `LayerDriver JPN=kbd106.dll`、`OverrideKeyboardIdentifier=PCAT_106KEY`、`OverrideKeyboardType=7`、`OverrideKeyboardSubtype=2`（固定モード）
- 入力方式: ユーザーは 00000411 と 00000409、サインイン画面（`.DEFAULT`）は 00000411
- サインインは指紋認証なので、配列の変化による影響はない
- バックアップ: デスクトップの `MKLM-backup-20260927\`（`reg export` 7 ファイル）

## #2a: 固定モードのまま Keychron に US（4/0）を書く

- 操作: `Set-KbdOverride.ps1 -Layout US` を実行し、ドングルを抜き差しした。
- Raw Input は **0x4 / 0x0** に変わった。抜き差しだけで kbdhid が値を読み直した。
- しかし文字は **JIS のまま**だった（Shift+2 で `"` が出た）。
- **結論**: 全体の `OverrideKeyboardType/Subtype` があると、キーボードごとの値は反映されない。この PC ではキーボードごとモードへの移行が必須で、「移行なし」の経路は使えない。
- **G3 は合格**: Raw Input の dwType は override を反映するので、反映の確認に使える。
- Keychron の 4/0 はそのまま残している（移行後に効く想定）。

## #4: PS/2 キーボードを固定したうえで全体の値を削除（この PC で直接実施）

- 操作: 管理者権限の PowerShell で `Invoke-M0Migration.ps1` を実行した後、PC を再起動する。
  - 内蔵キーボードの `Device Parameters` に `OverrideKeyboardType=7` / `OverrideKeyboardSubtype=2` を書く。
  - その後で、全体の `OverrideKeyboardType/Subtype` を削除する。
  - 変更前の値は `MKLM-backup-20260927\m0-4-premigration.json` に保存する。
- **再起動後に確認すること**:
  1. `tools\m0\Get-KbdState.ps1` の結果が、内蔵キーボード = 0x7/0x2、Keychron = 0x4/0x0、`Mode=per-keyboard` になっていること。
  2. メモ帳で IME をオフにして、Shift+2 を打つ。内蔵キーボードで `"`、Keychron で `@` が出れば合格。
- **元に戻す方法**: 管理者権限の PowerShell で `tools\m0\Invoke-M0Migration.ps1 -Undo` を実行して再起動する。スクリプトが使えない場合は、`MKLM-backup-20260927\i8042prt-Parameters.reg` をダブルクリックして再起動する。
- **結果（2026-09-27 13:00 に再起動）: 合格**
  - 内蔵キーボードの Raw Input は 0x7 / 0x2 のまま。全体の値がなくても、デバイス側の `OverrideKeyboardType/Subtype` を i8042prt が読んでいる。**G1 合格**。
  - Keychron の Raw Input は 0x4 / 0x0。
  - 全体設定は `Mode=per-keyboard`、`LayerDriver JPN=kbd106.dll`、`PCAT_106KEY`。
  - 打鍵テスト: 内蔵キーボードの Shift+2 で `"`（JIS）、Keychron の Shift+2 で `@`（US）。**G4 合格**。
  - **結論**: 移行手順（PS/2 に固定値を書く → 全体の Type/Subtype を削除 → PC の再起動）で、内蔵 JIS と外付け US の併用が実現できる。プランの 1.2 と 1.3 は、この PC では実機で確認できた。

## #2b: ソフトからのキーボードのリセット（USB、キーボードごとモード）

- 操作: `Set-KbdOverride.ps1 -Layout JIS -Restart` を実行した。値を書いたあと、`pnputil /restart-device` で devnode を再起動する（中身は DIF_PROPERTYCHANGE / DICS_PROPCHANGE）。抜き差しはしない。
- 結果:
  - `pnputil` の終了コードは 0 で、再起動は不要だった。
  - `LastArrivalDate` が更新された（13:00:49 → 13:05:11）。`DevNodeStatus=0x180000A`、`ProblemCode=0`。
  - Raw Input は 0x7 / 0x2 に変わり、Keychron の Shift+2 で `"`（JIS）が出た。
  - US に戻す操作（`-Layout US -Restart`）も同じく終了コード 0 で、Raw Input は 0x4 / 0x0 に戻った。
- **結論**: USB の kbdhid キーボードは、ソフトからのリセットだけで**その場で配列が切り替わる**。抜き差しも PC の再起動も要らない。**G2 は USB について合格**。
  - BLE と BT Classic は未検証（BLE キーボードの実機が必要）。
  - 内蔵キーボードには、プランのとおりリセットを使わない。

## #3b: Keychron のドングルを別の USB ポートに差し替える

- 結果: 差し替えたあとも、インスタンス ID は `HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000` のまま変わらなかった。
  - Raw Input は 0x4 / 0x0 で、Shift+2 で `@`（US）が出た。
  - 新しい USB の場所は `...#USBROOT(0)#USB(2)`。別ポート用の devnode（phantom）は作られなかった。
- **結論**: 親の USB デバイスがシリアル番号（`B76E483E3F08D96E`）を持っていれば、ポートを変えても設定は引き継がれる。適用範囲の既定値を「このデバイスのみ」にするというプランの方針（3.4）で問題ない。

## 実験前の記録の訂正（M1 の実装中に判明）

- **サインイン画面の入力方式**: `HKU\.DEFAULT` の Preload は、現在 `00000411, 00000409` になっている。実験前は `00000411` だけだった。先頭が日本語なので、サインイン画面の配列には影響しない。
- **未接続の BLE 機器（VID 045E、PID 0040）**: キーボードではなく、BLE マウス「X3-5.4 Mouse」のキーボード用コレクションだった。BLE キーボードのリセット検証には使えない。

## #8: Slint の試作（`prototypes/slint-proto/`）

- 自動で確認できたこと:
  - Slint 1.18.1 でビルドでき、起動と終了も正常だった。
  - `device_event` と `persistent_identifier()` を使って、キー入力を送ったキーボードを判別できた（Keychron 4/0、VXE 0x51/0、内蔵 7/2 が M0 の結果と一致）。
- 判明した問題:
  - Slint の `SystemTrayIcon` は、explorer.exe を再起動するとアイコンを再登録できない（HWND_MESSAGE を使っているため）。試作では独自の回避策を入れた。
  - バルーン通知の API がない。
  - femtovg で描画するとメモリを約 127 MB 使う（ソフトウェア描画なら約 28 MB）。
- **手動での確認（2026-09-27、ユーザーが実施）: A〜G のすべての項目で問題なし。**
  - A: 日本語 IME で入力できる。
  - B: 打鍵テストとキーボードの判別が正しい。内蔵 `"`、Keychron `@`。
  - C / D: テーマの切り替えと、無関係な `WM_SETTINGCHANGE` の後のタイトルバー。
  - E: トレイ。explorer.exe を再起動してもアイコンが残る（回避策が有効）。
  - F / G も問題なし。
  - **結論**: M3 の GUI は、Slint 1.18 を使い、試作で採った方式のまま作れる。アプリ側でのテーマ判定、`with_theme(Some)` と DWM、`device_event` によるキーボードの判別、トレイの回避策のいずれも、そのまま採用する。

## 未実施

- #1: Procmon による設定アプリの書き込み内容の採取。VM が必要。
- #5: 7/0 の挙動。v1 では 4/0 を使うため、優先度は低い。
- BLE / BT Classic でのソフトからのリセット。本物の BLE キーボードが必要。それまで BLE の反映方法は「再接続」とし、ソフトからのリセットは使わない（`LIVE_RESET_TRANSPORTS=[Usb]`）。
- 「標準に従う」（0x51）が、キーボードごとモードで本当に標準配列に従うかの打鍵確認。例: Keychron の値を消してリセットし、Shift+2 で `"` が出るかを見る。
