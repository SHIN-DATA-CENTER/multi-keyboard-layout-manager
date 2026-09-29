# 起動 ID の不具合（デスクトップ PC、2026-09-29）

- 報告: 2026-09-29。v0.1.0 をインストールしたデスクトップ PC で、固定モードから設定を変えて再起動したあとの確認画面で、「このままにする」が押せない。ジャーナルは「PC の再起動待ち」のまま変わらず、helper は新しい変更を断る。
- PC: Windows 11 Pro 25H2（build 26200.9457、x64）、UEFI。高速スタートアップは有効（`HiberbootEnabled=1`）。OS のインストールは 2026-09-28 04:19。
- 調べ方: すべて読み取りだけ。ジャーナル（`HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal`）、System ログ、`NtQuerySystemInformation`、`KUSER_SHARED_DATA`。

## 見つかったこと

### ジャーナル

操作が 2 つ残っていた（schema 1。値の名前ごとに `crates/mklm-core/testdata/journal/legacy-guid/` にそのまま写した）。

| 操作 | 内容 | 状態 | `boot_id` | 履歴の `boot_time_hint` |
|---|---|---|---|---|
| `c10d2d38` | 移行（標準 JIS、PS/2 を JIS）。09-29 02:03 に書き、11:36 に取り消した | `reverted-pending-reboot` | `9845bda6-baa7-11f1-adca-ca988d513a4f` | 134350433725000000（02:03 の 3 行）、134351228605000000（11:36 の取り消しの 2 行） |
| `d724c149` | 移行（標準 US、USB キーボードを US）。11:36 に書いた | `pending-reboot`、`apply_pending` は `restart-pc`（`since` も同じ GUID） | 同じ GUID | 134351228605000000（3 行） |

### 起動 ID（`SystemBootEnvironmentInformation.BootIdentifier`）が変わらない

- 0.1.0 は、起動 ID に `NtQuerySystemInformation(SystemBootEnvironmentInformation = 90).BootIdentifier` の GUID を使っていた。
- この PC では、その GUID が `9845bda6-baa7-11f1-adca-ca988d513a4f`（バージョン 1、時刻ベースの UUID）のまま変わらない。
  - `c10d2d38` が 09-29 02:03（09-28 13:29 の起動の中）に記録した値と、11:37 の起動の後に読んだ値が同じ。
  - その間に、11:32:37、11:34:20、11:37:07 の 3 回、完全な起動をしている（Kernel-Boot のイベント 27 がどれも「ブートの種類は 0x0」。Kernel-General のイベント 12 も同じ時刻）。
- そのため、どの「同じ起動か」の判定も「同じ」と答え続けた。
  - 確認画面: `not_restarted` が真のままで、「このままにする」が押せない。
  - helper: `confirm` は「再起動の前に確定しようとした」として断り（`InvalidState`）、新しい変更は `PendingReboot` の操作があるので断る（`OpInProgress`）。
  - `c10d2d38` の「再起動が必要」も消えない。
- H.2 の R9/R10（起動 ID の性質の実機確認）は、M2 の完了条件だったが未実施のままだった（`m2-real-tests.md` の「未実施」）。

### 使える値

| 値 | この PC で読んだもの | 性質 |
|---|---|---|
| `KUSER_SHARED_DATA.BootId`（0x7FFE0000 + 0x2C4） | 7 | `PrefetchParameters\BootId` も 7。System ログにある起動（イベント 27）は OS のインストール以来ちょうど 7 回（09-28 04:17:30、04:19:12、13:23:08、13:29:32、09-29 11:32:37、11:34:20、11:37:07） |
| `SystemTimeOfDayInformation`（クラス 3） | `BootTime` = 134351230275000000、`BootTimeBias` = 0、`SleepTimeBias` = 0 | 11:37:07.5（JST）。WMI の `LastBootUpTime`（11:37:07）と同じ |
| 履歴の `boot_time_hint`（`BootTime - BootTimeBias`） | 134350433725000000、134351228605000000 | それぞれ 09-28 13:29:32.5、09-29 11:34:20.5 の起動。起動のたびに変わっていた |

- `KUSER_SHARED_DATA.BootId` について公開されていること:
  - WDK の ntddk.h が、`KUSER_SHARED_DATA` の 0x2C4 に `ULONG BootId;` を置き、「Boot sequence, incremented for each boot attempt by the OS loader」と説明している。Windows 10 以降（Geoff Chappell の表: <https://geoffchappell.com/studies/windows/km/ntoskrnl/inc/api/ntexapi_x/kuser_shared_data/index.htm>）。
  - `windows` 0.62.2 の `Wdk::System::SystemServices::KUSER_SHARED_DATA` も、`AlternativeArchitecture`（0x2C0）の次に `BootId` を置く（mklm-win のテストで確かめている）。
  - NtDoc / phnt（<https://ntdoc.m417z.com/kuser_shared_data>）: 「Number of boots since OS install (really, boot attempts)」。winload が `\Windows\bootstat.dat`（`BSD_BOOT_STATUS_DATA.LastBootId`）を読み、1 増やして保存し、ローダーブロックでカーネルに渡す。
  - このページは、最小プロセス以外のすべてのプロセスで 0x7FFE0000 に読み取り専用で割り当てられる（ntdll と kernel32 が時刻をここから読む）。
  - 休止からの復帰（winresume）と休止イメージの復元でこの値がどうなるかは、どの資料にも書かれていない（Vergilius は `ULONG BootId; //0x2c4` だけ、Chappell は 10.0 以降の 0x02C4 だけ、NtDoc は上のローダーの動きまで）。
- この PC のローダーの記録（System ログ、読み取りだけ。2026-09-29 に読んだ）:
  - Kernel-Boot のイベント 20 は、起動のたびに前の起動の値（`LastBootId`）を記録している: 09-28 04:19:12 = 1、13:23:08 = 2、13:29:32 = 3、09-29 11:32:37 = 4、11:34:20 = 5、11:37:07 = 6（どれも `LastShutdownGood` と `LastBootGood` が true）。今の KUSER の値は 7、`PrefetchParameters\BootId` も 7 で、完全な起動ごとに 1 ずつ増えている。
  - 同じ時刻の Kernel-General のイベント 25 は、どれも `IsSoftBoot` = false、`LoaderTime` は `SystemTime` の 1〜2 秒前。
  - イベント 27 は OS のインストール以来 7 件すべて起動の種類 0x0 で、スリープのイベント（Kernel-Power 42）もない。したがってこの記録は完全な起動の数え方を裏付けるだけで、休止（0x2）と高速スタートアップ（0x1）については何も言えない。
- 期待される性質（MT-2〜MT-7 で確かめる。0.1.1 の公開の条件）:
  - スリープ（S3、モダンスタンバイ）: ローダーが動かないので変わらない。
  - 休止からの復帰（イベント 27 が 0x2）と、高速スタートアップの起動（0x1）: 休止イメージのカーネル（このページを含む）を戻すので、変わらない。ドライバーも値を読み直していないので、MKLM にはこれが必要。ローダーがこのとき `bootstat.dat` を数えるかは分からない。数えるなら、次の完全な起動で 2 以上増える（それでも正しい）。
  - 再起動と完全なシャットダウン（0x0）: winload が動くので増える（製品に要るのは「前と違う」ことだけ）。
  - 時刻の変更、`w32tm`: 変わらない。
  - 戻るのは、`bootstat.dat` が作り直されたとき（クリーンインストール。このときはジャーナルも消える。インプレースアップグレード、修復インストール、削除や破損）と、イメージの復元でジャーナルと一緒に巻き戻ったときだけ。

## 対処（0.1.1）

設計は `docs/design/m2-engine.md` の C.10「起動 ID」と H.2 R9/R10。

- 起動 ID を `KUSER_SHARED_DATA.BootId` に変えた（`mklm_win::session::boot_counter`）。
  - 記録の形は、カウンターを最初の組に入れたバージョン 8 の UUID（`00000007-0000-8000-8000-000000000000`）。0.1.0 の読み取りもそのまま受け付けるので、ジャーナルの版は上げない。
  - 「同じ起動か」は、これまでどおり等しいかどうかで決める。GUID や起動時刻を「すべて等しければ同じ起動」の形で足すと、「違う起動」と誤る場合（危険な側）が増えるだけなので、その形では足さない。
  - 起動時刻は「または」の形でだけ足す（カウンターの安全網）: カウンターが違っても、その ID の下の履歴行のヒントが今の起動時刻から 10 秒以内なら「同じ起動」。増えるのは「同じ起動」の答え（安全な側）だけで、休止や高速スタートアップでカウンターが動く Windows があっても、起動時刻が保たれていれば「再起動していない」のまま。
- 0.1.x が記録した GUID の起動 ID（legacy）は、ジャーナルを読むとき（エンジンがセッションを開くとき、GUI と CLI が読むとき）に、メモリの上でだけ判定する（`mklm_core::boot`）。
  - その GUID の下で書いた履歴行の `boot_time_hint` のどれかが、今の起動の `BootTime - BootTimeBias` から 10 秒以内なら、今の起動。`boot_id`（と `apply_pending.since`）を今のカウンターの ID に置き換える。
  - それ以外は、前の起動のまま残す。カウンターの ID とは決して等しくならないので、どの比較も「前の起動」と読む。
  - ヒントのある行がない（M2 のころのエントリ）か、起動時刻が読めないときは、0.1.x と同じく GUID で決める。GUID も読めなければ「今の起動」（再起動していない。安全な側）とする。
  - 履歴行は書き換えない。判定のためだけにジャーナルを書くこともない。
- この PC での結果（今 = カウンター 7、`BootTime - BootTimeBias` = 134351230275000000）:
  - `c10d2d38`: ヒントとの差は 22 時間と 167 秒で、前の起動。次に helper がセッションを開いたとき、`Reverted`（`reboot-observed`）になる。
  - `d724c149`: 差は 167 秒で、前の起動。確認画面は「このままにする」を押せる。押すと `RebootObserved` → `AwaitingConfirm` → `Confirmed`。
- 11:37 の再起動より前に修正版を入れていた場合は、どちらも今の起動と判定され、再起動を待ち続ける（どちらも正しい）。

## 修正版の CLI で読んだ結果（読み取りだけ）

`fix/boot-id` のビルド（`target\debug\mklm-cli.exe`）で、この PC のジャーナルを読んだ（2026-09-29 12:36、11:37:07 の起動の中）。比べるために、インストール済みの 0.1.0 の `mklm-cli journal` も読んだ。

- `cargo test -p mklm-win print_boot_id -- --ignored --nocapture`（13:17 に、生の値も表示するようにしたビルドで読み直した）:

  ```text
  KUSER_SHARED_DATA.BootId = Ok(7)
  boot_id = 00000007-0000-8000-8000-000000000000
  BootTime = 134351230275000000 (Some("2026-09-29 11:37:07 +09:00"))
  CurrentTime = 134351290367224580 (Some("2026-09-29 13:17:16 +09:00"))
  BootTimeBias = 0 (signed)
  SleepTimeBias = 0
  BootTime - BootTimeBias = Ok(134351230275000000) (Some("2026-09-29 11:37:07 +09:00"))
  legacy boot GUID = 9845bda6-baa7-11f1-adca-ca988d513a4f
  ```

- 13:17 にレビュー対応（カウンターの安全網、`parse_journal`）の後のビルドで読み直しても、下の結果と同じだった（`c10d2d38` は `attention` = `none`、`d724c149` は `recover`、どちらも保存された GUID のまま。`status` も同じ）。

- 0.1.0 の `mklm-cli journal`（抜粋）:

  ```text
  c10d2d38  migrate to per-keyboard mode (standard layout JIS), ACPI\PNP0303\0 = JIS  (reverted; the PC must restart)
      Attention: not in effect yet
  d724c149  migrate to per-keyboard mode (standard layout US), HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000 = US  (waiting for a PC restart)
      Attention: waits for a PC restart: `mklm-cli reboot`
      Not in effect yet for HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000: restart the PC (Restart, not Shut down)
  ```

- 修正版の `mklm-cli journal`（抜粋。値の行は省いた）:

  ```text
  c10d2d38  migrate to per-keyboard mode (standard layout JIS), ACPI\PNP0303\0 = JIS  (reverted; the PC must restart)
      c10d2d38-ec81-4c57-a7d6-9592199945ff  #1  created 2026-09-29 02:03:51 +09:00, updated 2026-09-29 11:36:14 +09:00
      Takes effect: restart the PC (Restart, not Shut down)
  d724c149  migrate to per-keyboard mode (standard layout US), HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000 = US  (waiting for a PC restart)
      d724c149-9bee-4348-9481-a544f9980f4b  #2  created 2026-09-29 11:36:35 +09:00, updated 2026-09-29 11:36:35 +09:00
      Attention: needs recovery: run `mklm-cli recover`
      Takes effect: restart the PC (Restart, not Shut down)
      Not in effect yet for HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000: restart the PC (Restart, not Shut down)
  ```

  - `c10d2d38` の「Attention」の行がなくなった（`attention` が `None`。「再起動が必要」ではない）。見出しの「(reverted; the PC must restart)」は保存された状態の名前で、次に helper がセッションを開いたときの掃除で `Reverted` になる。
  - `d724c149` は「再起動待ち」から、再起動後の確認が必要な状態（`Recover`。GUI では確認画面の「このままにする」が押せる）に変わった。`Not in effect yet` の行は保存された `apply_pending` をそのまま表示したもので、確定か掃除のときに消える。
- 修正版の `mklm-cli journal --json`（要点）: 今の `boot_id` は `00000007-0000-8000-8000-000000000000`。`c10d2d38` は `attention` = `none`、`d724c149` は `recover`。どちらも前の起動と判定したので、エントリの `boot_id` と `apply_pending.since` は保存された GUID のまま（読み替えていない）。
- `mklm-cli status` はジャーナルを読まない（キーボードと全体の値だけ）。「Pending nothing」、INV-PS2 は接続中のキーボードで成り立つ。PS/2 の `ACPI\PNP0303\0` は 4/0（`d724c149` が書いた値）で、devnode は「not started, problem code 24」。

## MT-1 の結果（2026-09-29、合格）

設計 H.2 の MT-1（報告された場合）。ユーザーがこの PC のコンソールで行い、ジャーナルはインストール済みの修正版の `mklm-cli journal`（`--json` も）で読んだ（読み取りだけ）。

| 項目 | 結果 |
|---|---|
| 日時 | 2026-09-29 14:08〜14:09。11:37:07 の起動の中（カウンター 7、`BootTime - BootTimeBias` = 134351230275000000）で、再起動していない |
| セッション | コンソール（RDP ではない） |
| ビルド | `fix/boot-id`（5386de6）からローカルでビルドした x64 の NSIS インストーラー。ビルド ID `0.1.0+56b6ae1136c97a41af3a17a64c3db43b`（`mklm-cli --version` は `0.1.0`） |
| 手順 | 1. インストール済みの v0.1.0 の上に、NSIS のインストーラーで修正版を入れた（14:08。動いていた 0.1.0 の GUI は 14:08:11 に自動で終わり、修正版の GUI が 14:08:20 に起動した）。2. 再起動しないで、GUI の再起動後の確認画面を開いた。3. 打鍵テスト（日本語入力はオフ）。4. 「このままにする」→ UAC で「はい」（14:09:16） |
| 確認画面 | 「このままにする」が押せた（`can_keep` は `!not_restarted` なので、「まだ反映されていません」の行もない）。押した後は「再起動の後に確認する変更はありません。」になり、ボタンは右下の「閉じる」だけ |
| 打鍵テスト | Keychron Receiver（値なし、0x51/0）で Shift+2 → `@`、P の右隣のキー → `[`。US 配列（この PC の標準配列）。下の「0x51 のキーボード」 |
| `d724c149` | `pending-reboot` → `awaiting-confirm`（`recover:reboot-observed`、14:09:16.171）→ `confirmed`（`keep`、14:09:16.172）。`apply_pending` はなし、`attention` は `none`。CLI の見出しは「(kept)」 |
| `c10d2d38` | `reverted-pending-reboot` → `reverted`（`reboot-observed`、14:09:16.166）。`apply_pending` はなし、`attention` は `none`。「再起動が必要」は消えた |
| 履歴の新しい 3 行 | どれも同じ helper（pid 7388、14:09:16.130 に起動）が書いた。`boot` はカウンターの ID `00000007-0000-8000-8000-000000000000`、`boot_time_hint` は 134351230275000000。エントリの `boot_id` は保存された GUID `9845bda6-…` のまま（前の起動と判定したので読み替えない）で、C.10 のとおり |
| 全体 | `mklm-cli status`: `Pending nothing`、`Mode per-keyboard`、`Standard layout US`（`LayerDriver JPN` = `kbd101.dll`、`PCAT_101KEY`）。open の操作がないので、新しい変更を受け付ける。USB Keyboard（04D9:1818）は保存値も報告値も 4/0（US） |
| RDP の項 | コンソールで行ったので対象外。RDP のセッションでの確認画面と Keep（TERMINPUT_BUS のセッション用キーボードで `Conflict` にならないか）は、設計 H.2 の MT-6b で見る（再起動で反映する変更を 1 つ作り、再起動して、RDP の新しいセッションでサインインする）。`rdp-keyboard.md` の E7 は打鍵の表を調べる実験で、Keep の手順はない。RDP で打ったキーはこの PC のキーボードの確かめにならない（Raw Input に名前がない）ので、打鍵の確認はコンソールで行う |
| 判定 | **合格**。0.1.0 が止めた 2 つの操作が、再起動なしで解けた（修正版は 11:37 の再起動の後に入れたので、どちらの操作も前の起動のものと判定された） |

### 0x51 のキーボード（標準配列に従う）

- Keychron Receiver `HID\VID_3434&PID_D027&MI_00&COL01\7&5211D3A&0&0000`（USB のシリアル番号 `B76E483E3F08D96E`。M0 の開発機で使ったものと同じレシーバー）。この PC ではデバイス側の値がなく、Raw Input の報告は 0x51/0。
- この PC はキーボードごとモードで、標準配列は US（`LayerDriver JPN` = `kbd101.dll`）。入力方式は 00000411（日本語。IME はオフで打った）。
- Shift+2 で `@`、P の右隣で `[` が出た（JIS なら `"` と `@`）。**値のない HID キーボード（0x51）が PC の標準配列に従うことを、標準が US の場合について初めて実機で確かめた**（`m0-results.md` の「未実施」の項）。
- 標準が JIS の場合（0x51 のキーボードで `"` が出るか）はまだ確かめていない。標準が US のときは、「0x51 を 101 キーとして扱う」場合とも結果が同じなので、この 1 回では区別できない。CLI と GUI の「実機で確かめていない」の注記はそのまま残す。

## 2026-09-29 夕方から夜の実機（MT の途中経過）

統合ビルド 0.1.0+8994675d（`m5b/updater` の 167ca01。16:18 に起動 ID の修正版の上へ NSIS で上書きした）で、利用者が行った。ジャーナルはインストール済みの `mklm-cli journal --json` で読んだ（読み取りだけ）。時刻は日本時間。

| # | 時刻 | 何をしたか | 起動（KUSER、イベント 27） | ジャーナル | MT との関係 |
|---|---|---|---|---|---|
| 1 | 16:21:01〜04 | GUI で `d724c149`（標準 US への移行、14:09 に Keep 済み）を元に戻す | 7 | `confirmed` → `revert-pending`（16:21:01.574）→ `reverted-pending-reboot`（16:21:04.214） | — |
| 2 | 16:29:33 | 再起動（完全な起動） | 7 → 8、0x0 | 16:35:41.184 に `reverted`（`reboot-observed`、起動 8、pid 19232） | MT-6 の「0x0 でカウンターが増える」の証拠（打鍵と Keep はしていない） |
| 3 | 16:35:41 | 昇格した PowerShell で移行（`271b6909`: 標準 JIS、USB Keyboard 04D9:1818 と Keychron Receiver 3434:D027 を US、PS/2 は 4/0 で固定） | 8 | `planned`（.186）→ `written`（.227）→ `pending-reboot`（.229） | RunOnce は登録されなかった（昇格した CLI は「may belong to another account」と表示して何も登録しない。設計 standard-layout D.3） |
| 4 | 23:12:58 | 再起動 | 8 → 9、0x0 | — | MT-6 の証拠の 2 回目 |
| 5 | 23:14:06〜14 | RDP で新しくサインイン（セッション 2）、MKLM を手で開き Keep | 9 | `recover:reboot-observed`（23:14:14.163、pid 4312）→ `confirmed`（`keep`、.166）。`Conflict` なし | MT-6b の Keep の部分は合格（TERMINPUT_BUS のキーボードで `Conflict` にならない）。RunOnce は登録されていなかったので「確認画面が開く」は見ていない。警告の文は誤り（USB Keyboard と Keychron に「Raw Input does not report the stored type yet; reconnect the keyboard」。RDP で作ったセッションの Raw Input にはこの PC の物理キーボードが並ばないため。設計 standard-layout D.2）。コンソールでの打鍵の確認はまだ |

- ワークフローの説明では 3 の移行は「22:5x」とされていたが、ジャーナルの記録は 16:35:41（`planned` の `at` = 1790667341186）。この表はジャーナルに従う。
- 16:29:33〜16:35:41 の間、`mklm-cli journal` は `d724c149` を「(reverted; the PC must restart)」と表示し続けた（保存された状態をそのまま出していた。次に helper がセッションを開いた 16:35:41 に `reverted` になった。設計 standard-layout D.1 で、起動で判定した状態を出すように直す）。
- `boot_time_hint` は、起動 8 = 134351405735000000（16:29:33.5）、起動 9 = 134351647785000000（23:12:58.5）。どちらの起動も、ジャーナルの行の `boot` はカウンターの ID（`00000008-…`、`00000009-0000-8000-8000-000000000000`）。
- 23:1x、セッション 2（RDP で新しく作ったセッション。標準配列 JIS）で、JIS の接続元から打つと JIS で打たれた（利用者の報告。`rdp-keyboard.md` の E7c）。

## 残っている確かめ

`docs/design/m2-engine.md` の H.2 の MT-1〜MT-9（R9/R10 を含む）。**0.1.1 の公開の条件**で、すべて通るまでタグを付けない（0.1.0 は R9/R10 を行わずに出荷し、この不具合になった）。

- MT-1 は 2026-09-29 に合格した（上の「MT-1 の結果」）。
- MT-6: カウンターの部分は 2 回確かめた（16:29:33 と 23:12:58 の再起動で、どちらもイベント 27 が 0x0 でカウンターが 1 ずつ増えた。上の「夕方から夜の実機」）。コンソールでの打鍵と Keep が残る。
- MT-6b: Keep の部分は済み（23:14、RDP で新しく作ったセッションで `Conflict` にならずに `confirmed`）。RunOnce からの起動（昇格した CLI では登録されないので、ふつうの権限の GUI か CLI で変更して確かめる）と、その後のコンソールでの確認が残る。
- MT-2〜MT-5、MT-7〜MT-9 は未実施のまま。
- この PC（高速スタートアップが既定で有効）で行う。
- スリープ、休止、シャットダウンはコンソールで行い、イベント 27（スリープは Kernel-Power 42 と Power-Troubleshooter 1）で狙った状態になったかを確かめる。ならなければ結論なしとしてやり直す。
- 打鍵の確認もコンソールで行う。RDP で行うのは MT-6b だけ。2026-09-29 14:13 から、利用者はセッション 1 に RDP で再接続している（`qwinsta` で `rdp-tcp#0`、LocalSessionManager のイベント 25、接続元は 0x7/0x2 を報告）ので、MT-2 の前にコンソールに戻る。
- 記録は `mklm-cli journal --json` で読む（文字の形は履歴と起動 ID を表示しない）。
- 止める規則: スリープ、0x2 か 0x1 の起動をまたいで KUSER のカウンターが変わった、または `BootTime - BootTimeBias` が 10 秒を超えて動いた、0x0 の起動でカウンターが増えなかった、のどれかが起きたら公開せず、設計を見直す。
- ARM64 の Windows 11 で `cargo test -p mklm-win session:: -- --include-ignored --nocapture` を一度実行し、カウンターが 0 でなく `PrefetchParameters\BootId` と等しいことを確かめて、ここに記録する。M5b の更新情報は arm64 のアセットを必ず持つので、arm64 版だけを外して出すことはできない。確かめられなければ v0.2.0 のリリース全体を延期する（設計 C.10「公開する順序」、m5b B.5 の手順 0）。
