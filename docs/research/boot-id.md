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
- 期待される性質（MT-2〜MT-7 で確かめる）:
  - スリープ（S3、モダンスタンバイ）: ローダーが動かないので変わらない。
  - 休止からの復帰（イベント 27 が 0x2）と、高速スタートアップの起動（0x1）: 休止イメージのカーネル（このページを含む）を戻すだけで winload は数えないので、変わらない。ドライバーも値を読み直していないので、MKLM にはこれが必要。
  - 再起動と完全なシャットダウン（0x0）: winload が動くので 1 増える。
  - 時刻の変更、`w32tm`: 変わらない。
  - 戻るのは、`bootstat.dat` が作り直されたとき（クリーンインストール。このときはジャーナルも消える。インプレースアップグレード、修復インストール、削除や破損）と、イメージの復元でジャーナルと一緒に巻き戻ったときだけ。

## 対処（0.1.1）

設計は `docs/design/m2-engine.md` の C.10「起動 ID」と H.2 R9/R10。

- 起動 ID を `KUSER_SHARED_DATA.BootId` に変えた（`mklm_win::session::boot_counter`）。
  - 記録の形は、カウンターを最初の組に入れたバージョン 8 の UUID（`00000007-0000-8000-8000-000000000000`）。0.1.0 の読み取りもそのまま受け付けるので、ジャーナルの版は上げない。
  - 「同じ起動か」は、これまでどおり等しいかどうかだけで決める。GUID や起動時刻を足すと、「違う起動」と誤る場合（危険な側）が増えるだけなので、足さない。
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

- `cargo test -p mklm-win print_boot_id -- --ignored --nocapture`:

  ```text
  KUSER_SHARED_DATA.BootId = Ok(7)
  boot_id = 00000007-0000-8000-8000-000000000000
  BootTime - BootTimeBias = Ok(134351230275000000) (Some("2026-09-29 11:37:07 +09:00"))
  legacy boot GUID = 9845bda6-baa7-11f1-adca-ca988d513a4f
  ```

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

## 残っている確かめ

`docs/design/m2-engine.md` の H.2 の MT-1〜MT-9（R9/R10 を含む）。MT-1 は、この PC に修正版をインストーラーで上書きして確かめる。
