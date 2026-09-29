# M2 設計: 書き込みと安全網（mklm-engine / mklm-ipc / mklm-helper）

| 項目 | 内容 |
|---|---|
| 対象 | マイルストーン M2（プラン 6 章「書き込みと安全網」） |
| 根拠 | 承認済みプラン 1.2〜1.5、2.1〜2.3、3.4〜3.6、3.13、4.1、6 章。M0 の結果（`docs/research/m0-results.md`） |
| 状態 | 設計（レビュー 1 回目を反映済み。末尾の「レビュー対応」）。骨組みのコードはコンパイル済みで、処理の本体は `todo!()`（0.4 節） |
| 読み手 | M2 を分担して実装する人（K 章に作業の分け方） |

識別子（型、関数、値の名前）は英語のまま書く。「計画」は承認済みプラン、「#2a」などは M0 の検証番号を指す。「C3」「S5」などは末尾の「レビュー対応」の指摘番号を指す（C はクラッシュ時の整合性、S は API とセキュリティの観点）。

---

## 0. 前提と方針

### 0.1 M0 で確かめた事実

| 事実 | 設計への影響 |
|---|---|
| 全体の `OverrideKeyboardType/Subtype` があると、デバイスごとの値は効かない（#2a） | 固定モードの PC では移行が必須。固定モードで `set` を求められたら `MigrationRequired` を返す（D.2） |
| PS/2 にデバイス側の 7/2 を書いてから全体の Type/Subtype を消しても、内蔵キーボードは 7/2 のまま（#4、G1 合格） | 計画 1.3 の移行手順をそのまま実装する（D.3） |
| USB の kbdhid は `DIF_PROPERTYCHANGE`（`pnputil /restart-device` と同じ）で、その場で配列が変わる。終了コードは 0（#2b、G2 は USB で合格） | USB はリセットとカウントダウンの経路（D.2 a） |
| Raw Input の `dwType/dwSubType` は override を反映する（G3 合格） | リセット後の確認と、再起動後の確認に使う。ただし固定モードでは Keychron が 4/0、内蔵がどちらのモードでも 7/2 と報告するので、**移行が反映されたかは Raw Input では判別できない**（#2a）。移行の確認は起動 ID（C2）と打鍵テストに頼る |
| BLE と BT Classic のその場でのリセットは未実証 | 再接続の経路にする（`LIVE_RESET_TRANSPORTS = [Usb]` を変えない） |
| 「標準に従う」（値を消して 0x51）の打鍵確認は未実施 | 実行はできるが、結果に「未確認」の警告を付ける |
| シリアル番号付きの USB は、ポートを変えてもインスタンス ID が変わらない（#3b） | 適用範囲は「このデバイスのみ」でよい。「同じ機種ならどのポートでも」は M4 |

### 0.2 設計の原則

1. **どこで止まっても戻せる。** プロセスの強制終了でも電源断でも、ジャーナルを読めば「何をどこまで書いたか」と「どこへ戻すか」が決まる。
2. **回復は冪等。** 差分を戻すのではなく、記録した値で上書きする。回復の途中でまた止まっても、もう一度回復すれば同じ結果になる。
3. **書く前に検査する。** 新しい値は `mklm_core::check_plan`（許可リスト、INV-PS2、書く順序）を通す。戻す値は `mklm_core::plan_restore`（ジャーナルとの照合、INV-PS2、変化の向きで決める順序）を通す。値の名前は、最後の関所として `mklm_win::regwrite` でも確かめる（S9）。
4. **他者の変更は勝手に上書きしない。** 戻すときは常に compare-and-swap（CAS）で確かめ、合わなければ Conflict にしてユーザーに選んでもらう。衝突の解決も、ユーザーが見た値を CAS の期待値にする（C5）。
5. **分からないときは書かない。** ジャーナルが読めない、Keyboard クラスの devnode の集合・ドライバー・present のどれかが確定できない、のいずれかなら書き込みを止める。表示名や Raw Input など、書き込みの安全に関係しない読み取りの問題は警告にとどめる（S2、C3）。
6. **失敗は、閉じた状態かユーザーが決める状態で終わる。** クラッシュ以外の失敗で、エントリを書き込み中の状態のまま残さない。書き込みが繰り返し拒否されるなら `Conflict` にして、エラーを記録する。回復が同じ失敗を無限に繰り返さないようにするため（C3）。
7. **「効いていない」ことを隠さない。** 保存した値とドライバーが使っている値が食い違う可能性があれば、その操作が閉じた後も `apply_pending` としてジャーナルに残し、表示する（C1）。
8. **M1 のコードを使い回す。** `check_plan`、`CheckedPlan.steps`、`device_layout_writes`、`standard_layout_writes`、`ps2_pin_layout`、`live_reset_bans`、`device_apply_action`、`check_inv_ps2`、`assess`、`predict_type`、`mklm_win::read_keyboards`、`raw_keyboards` を呼ぶ。同じ規則を別の場所に書き直さない。計画を作る関数（`plan_set_layout`、`plan_migration`）も、非昇格の下見とエンジンで同じものを使う（S6）。

### 0.3 用語

| 用語 | 意味 |
|---|---|
| 操作（op） | 1 回の `set` / `migrate` / `restore --baseline`。ジャーナルのエントリ 1 件に対応する。取り消し（revert）と undo は新しい操作ではなく、元の操作の状態遷移 |
| 記録（record） | 1 つの操作が 1 つのレジストリ値に対して行うこと（`ValueRecord`） |
| `baseline` | MKLM が**初めて**その値を変える前の値（「値なし」を含む） |
| `before` | **その操作が**書く直前の値。操作を取り消すとこの値に戻る |
| `intended` | その操作が書く値 |
| `last_written` | その操作が最後に書いてフラッシュした値。書く前は `None`。CAS はこの値と比べる |
| open な状態 | ほかの書き込み、更新、自動再適用を止める状態（C.4） |
| 書き込み中の状態（in-flight） | プロセスが書いている最中、またはリセットしている最中の状態。所有者は書き込み中ずっとロックを持つので、**ロックを取れた時点で見つかった書き込み中のエントリは、所有者の生死によらず放棄されたもの**（C3） |
| 起動時の値 | i8042prt のデバイス値と全体の値。どちらも PC の再起動でしか読まれない |
| `apply_pending` | 保存した値と、ドライバーが今使っている値が食い違っているかもしれないこと。必要な対処（キーボードのリセット / 再接続 / PC の再起動）と対象のキーボードを持つ（C1） |
| `revert_mode` | `RevertPending` の間に何をしているか（取り消し / ロールバック / 衝突の解決）。回復がそれを続ける（C5） |
| undo | open で書き込み中ではないエントリ（確認待ち、再起動待ち、衝突）をまとめて取り消すこと。利用者の非常口（C7、D.10） |

### 0.4 このコミットに含まれる骨組み

処理の本体はすべて `todo!()`。未使用の引数とフィールドの警告は、モジュール単位の `#![allow(unused_variables, dead_code)]` と「Skeleton (M2)」のコメントで抑えている。実装したモジュールから順にこの allow を消す。

| 場所 | 中身 |
|---|---|
| `crates/mklm-core/src/journal.rs` | ジャーナルの型（`JournalEntry`、`ValueRecord`、`RegValue`、`OpState`、`OpKind`、`RevertMode`、`ApplyPending`、`SkipReason`、`BaselineRecord`、`Journal` など）、`value_eq`、保存場所の定数 |
| `crates/mklm-core/src/recovery.rs` | 回復の判定（`decide_recovery`、`attention`、`state_after_resolution`、`apply_pending_on_close`、`apply_pending_cleared`） |
| `crates/mklm-core/src/restore.rs` | 戻す計画（`plan_restore`、`check_restore_record`、`RestorePhase`） |
| `crates/mklm-core/src/operation.rs` | `set` と `migrate` の計画（`plan_set_layout`、`plan_migration`、`OperationPlan`）、書き込み内容（`set_layout_writes`、`migration_writes`、`physical_device_members`）、反映方法（`apply_method`） |
| `crates/mklm-core/src/report.rs` | エンジンと呼び出し元が共有する型（`ApplyOptions`、`ExpectedPlan`、`Event`、`Decision`、`OperationResult`、`ErrorInfo` など。S5 で ipc から移した） |
| `crates/mklm-core/src/recovery_assets.rs` | 復旧用ファイルの生成（`render_recovery_assets`、`is_cmd_safe`） |
| `crates/mklm-core/src/allowlist.rs` | 値の名前の許可リスト `DEVICE_VALUE_NAMES`、`GLOBAL_VALUE_NAMES` を追加（S9） |
| `crates/mklm-core` の `test-fixtures` feature | `fixtures`（開発機の M0 後の状態）をほかのクレートのテストから使えるようにする |
| `crates/mklm-ipc/` | コマンドライン（`HelperArgs`）、フレーミング（`FrameHeader`）、パイプで許す要求（`Request`）、ハンドシェイク（ビルド ID を含む） |
| `crates/mklm-engine/` | `RegistryBackend`、`DeviceController`、`Host`、`EventSink`、`Engine`、`params`（エンジン専用の引数の型）、インメモリ実装（`memory`）、Windows 実装（`win`）。ipc には依存しない |
| `crates/mklm-win/src/` | 書き込み用の新モジュール `regwrite`、`journal_store`、`devctl`、`pipe`、`elevation`、`proc_identity`、`protected_dir`、`session`。`Error` に 6 つのバリアントを追加。`ReadIssue` に種類（`ReadIssueKind`）を付けた（M1 の呼び出し箇所もすべて分類済み） |
| `apps/mklm-helper/` | `requireAdministrator` のマニフェスト、VERSIONINFO、`session.rs`（`dispatch` を含む） |
| `apps/mklm-cli/src/write.rs` | M2 のコマンド定義（clap）と終了コード。実行すると「未実装」のエラーを返す。`--yes` の制約（S12）は clap で実装済みで、テストがある |

---

## A. クレート構成

### A.1 依存関係

```
                        mklm-core（純粋。unsafe なし。report に共有の型）
                  ▲          ▲            ▲
                  │          │            │
             mklm-ipc   mklm-engine ──▶ mklm-win（Win32。unsafe はここだけ。
                  ▲       ▲    ▲           engine からは cfg(windows) のときだけ）
                  │       │    │
                  └─ mklm-helper   mklm-cli（core、win、ipc、engine）

         （M3）mklm GUI：core + win + ipc。engine は使わない（計画の確認は core でできる）
```

| クレート | 役割 | unsafe | 依存 |
|---|---|---|---|
| `mklm-core` | 規則と判定（許可リスト、INV-PS2、ジャーナルの状態機械、回復の判定、戻す計画、操作の計画、復旧用ファイルの生成）と、エンジンと呼び出し元が共有する型（`report`） | 禁止（`forbid`） | serde、thiserror |
| `mklm-ipc` | パイプで許す要求の定義とフレーミング。Windows を呼ばない。engine には依存しない | 禁止 | core、serde、serde_json、thiserror |
| `mklm-win` | Win32 のラッパー。M1 の読み取り専用モジュールと、M2 の書き込み用モジュール | ここだけ | core、windows、windows-registry |
| `mklm-engine` | トランザクション（適用、取り消し、undo、回復、移行、導入前に戻す、確定）。引数は `params`、出力は `mklm_core::report` の型。パイプのことは知らない | 禁止 | core、win（Windows のみ） |
| `mklm-helper` | 昇格して短時間だけ動くプロセス。パイプの要求を 1 つずつ明示的にエンジンの呼び出しに写す（`dispatch`） | 禁止（`mklm-win` 経由） | core、ipc、engine、win |
| `mklm-cli` | 読み取り（M1）と書き込み（M2）のコマンド。書き込みは常に helper を別プロセスとして起動する。エンジンを同じプロセスで動かすのは隠しオプションの `--in-process` だけ | 禁止 | core、win、ipc、engine |

**engine が ipc に依存しない理由**（S5）: エンジンの公開 API がパイプの型そのものだと、エンジンにできることがすべてパイプから届いてしまう（例: アンインストール専用のサイレントな「導入前に戻す」）。パイプで許す要求は ipc が定義し、helper がそれを 1 つずつエンジンの呼び出しに写す。エンジン内部の変更が `PROTOCOL_VERSION` の変更になることもない。

`mklm-helper` の依存にネットワーク系のクレート（`ureq` など）と UI ツールキット（`slint` など）を入れない。M5 で `deny.toml` に `cargo tree -p mklm-helper` の検査を加える。

### A.2 mklm-core に入れるもの（純粋な規則と共有の型）

判定はすべて core に置く。Windows がなくても単体テストでき、非昇格の GUI も同じ判定（`attention`、`plan_set_layout`）で「helper を起動する必要があるか」「何が書かれるか」を決められるため。

| モジュール | 主な公開 API | 役割 |
|---|---|---|
| `journal` | `JournalEntry`（`transition`、`take_over`）、`ValueRecord`、`RegValue`、`value_eq`、`ValueKey`、`OpId`、`BootId`（0.1.1 から `KUSER_SHARED_DATA.BootId` のカウンター形式。`from_boot_counter`、`boot_counter`、`is_legacy`。C.10）、`ProcessIdentity`、`Liveness`、`OpKind`、`LayoutChoice`、`RestoreScope`、`OpState`（`is_open`、`is_in_flight`、`can_transition_to`）、`RevertMode`、`ApplyPending`、`SkipReason`、`Countdown`、`FailureReason`、`TransitionRecord`、`BaselineRecord`、`Journal`（`parse`、`open_entries`、`latest_record`、`baseline`、`next_seq`、`prunable`、`resolve_prefix`、`needs_post_reboot_check`）、`JournalError`、保存場所の定数 | ジャーナルの型と状態機械（C 章） |
| `recovery` | `decide_recovery`、`RecoveryDecision`、`RecoveryContext`、`observe`、`attention`（`Attention::blocks_writes`）、`state_after_resolution`、`apply_pending_on_close`、`apply_pending_cleared` | 回復の判定表（C.7） |
| `boot`（0.1.1） | `CurrentBoot`、`LEGACY_BOOT_TIME_TOLERANCE`、`JournalEntry::{legacy_boot_is_current, adopt_legacy_boots}`、`Journal::adopt_legacy_boots` | 0.1.x が記録した GUID の起動 ID の判定と、メモリ上での読み替え（C.10「起動 ID」） |
| `restore` | `plan_restore`、`check_restore_record`、`RestorePlan`、`RestoreStep`、`RestorePhase`、`RestoreWrite`、`Expect`、`RestoreTo`、`RestoreError` | 戻す書き込みの検査と、変化の向きで決める順序（C.5） |
| `operation` | `plan_set_layout`、`plan_migration`、`OperationPlan`、`set_layout_writes`、`migration_writes`、`physical_device_members`、`apply_method`、`OperationError`、`DeviceWrites` | `set` と `migrate` の計画、書き込み内容、反映方法。下見とエンジンが同じ関数を使う |
| `report` | `ApplyOptions`、`ExpectedPlan`、`ConflictPolicy`、`ResolutionChoice`、`ValueChoice`、`Decision`、`Event`、`ExpectedKeyboard`、`Outcome`、`ConflictInfo`、`RecoveredOp`、`OperationResult`、`ErrorCode`、`ErrorInfo` | エンジン、パイプ、UI が共有する語彙 |
| `recovery_assets` | `render_recovery_assets`、`is_cmd_safe`、`RecoveryAssets`、ファイル名の定数 | `.reg`、`restore-offline.cmd`、`README.txt` の生成（G 章） |

既存モジュールの変更は 3 点だけ。`WriteTarget` に `Hash` を付けた。`fixtures` を `test-fixtures` feature で公開した。`allowlist` に値の名前の一覧（`DEVICE_VALUE_NAMES`、`GLOBAL_VALUE_NAMES`）を足した。

### A.3 mklm-win に足すもの（書き込み用。最小限の unsafe ラッパー）

読み取り専用のモジュール（M1）は、`ReadIssue` に種類を付けたこと以外は変えない。HKLM に対する `KEY_SET_VALUE` が現れるのは `regwrite` と `journal_store` だけ、HKCU の RunOnce への書き込みは `session` だけ、という形をレビューで確かめられるようにする。

| モジュール | 公開 API | 使う Win32 API と要点 |
|---|---|---|
| `regwrite` | `open_device_key_rw(instance_id)`、`open_global_key_rw()`、`WritableKey::{read, list, write, flush}`、読み取り専用の `open_device_key_read(instance_id)`、`open_global_key_read()`、`ReadOnlyKey::{read, list}`（`KEY_READ` と `RegDisposition_OpenExisting`。作らない。エンジンの読み取りはこちら。I7） | `CM_Locate_DevNodeW(CM_LOCATE_DEVNODE_PHANTOM)` → クラスが Keyboard であることを確認（違えば `Error::NotKeyboard`）→ `CM_Open_DevNode_Key(KEY_QUERY_VALUE \| KEY_SET_VALUE, 0, RegDisposition_OpenAlways, &hkey, CM_REGISTRY_HARDWARE)`。全体のキーは `RegOpenKeyExW(KEY_QUERY_VALUE \| KEY_SET_VALUE)` で、作成はしない。値は `RegQueryValueExW`、`RegEnumValueW`、`RegSetValueExW`、`RegDeleteValueW`（存在しなくても成功扱い）、`RegFlushKey`。**`write` は値の名前を静的な一覧で確かめ、それ以外は `Error::ValueNotAllowed`**（S9）。devnode が消えていれば `CR_NO_SUCH_DEVNODE` を返し、エンジンはそれを `DeviceRemoved` にする（「値なし」とは区別する。C3） |
| `journal_store` | `read_journal_store()`（非昇格でも可）、`JournalStore::{open_or_create, write, delete, flush}`、`JOURNAL_KEY_SDDL` | 作成は `RegCreateKeyExW`＋`SECURITY_ATTRIBUTES`（`ConvertStringSecurityDescriptorToSecurityDescriptorW`）。既存のキーは `RegGetKeySecurity` で所有者と DACL を検証する（C.1） |
| `devctl` | `restart_device(instance_id) -> RestartResult`、`devnode_state(instance_id)` | `SetupDiCreateDeviceInfoList(GUID_DEVCLASS_KEYBOARD)` → `SetupDiOpenDeviceInfoW` → `SetupDiSetClassInstallParamsW(SP_PROPCHANGE_PARAMS{DIF_PROPERTYCHANGE, DICS_PROPCHANGE, DICS_FLAG_CONFIGSPECIFIC, 0})` → `SetupDiCallClassInstaller(DIF_PROPERTYCHANGE)` → `SetupDiGetDeviceInstallParamsW` の `Flags` に `DI_NEEDREBOOT` か `DI_NEEDRESTART` があれば `NeedsReboot`。状態は `CM_Get_DevNode_Status`。**`restart_device` は期限を持たない同期呼び出しなので、エンジンの Windows 実装が別スレッドで期限付きで呼ぶ**（C18） |
| `pipe` | `PipeServer::{create, accept}`、`PipeConnection::{connect, peer_pid, set_read_timeout}`、`Read`/`Write`、`HELPER_PIPE_SDDL` | `CreateNamedPipeW`、`ConnectNamedPipe`（overlapped）、`GetNamedPipeClientProcessId`、`DisconnectNamedPipe`（PID が違う相手を切って待ち直す）、`CreateFileW(SECURITY_SQOS_PRESENT \| SECURITY_IDENTIFICATION)`、`WaitNamedPipeW`（`ERROR_PIPE_BUSY` の再試行）、`GetNamedPipeServerProcessId`、`PeekNamedPipe`（切断の検出）、`ReadFile`/`WriteFile`（overlapped）、`GetOverlappedResultEx`（タイムアウト）、`CancelIoEx`（E 章） |
| `elevation` | `is_elevated()`、`launch_elevated(exe, params, owner_window)`、`spawn_from_elevated(exe, params)`、`ElevatedProcess::{pid, exit_code, wait}`、`helper_path()`、`file_build_id(exe)`、`BUILD_ID_VERSION_KEY` | `GetTokenInformation(TokenElevation)`、`ShellExecuteExW("runas")`（`lpDirectory` = System32。`ERROR_CANCELLED` なら `Error::Cancelled`）、昇格済みなら `CreateProcessW`（UAC なし。C8）、`GetExitCodeProcess`、`WaitForSingleObject`、`GetFileVersionInfoW`＋`VerQueryValueW`（ビルド ID。S11） |
| `proc_identity` | `current_process_identity()`、`process_liveness(&ProcessIdentity)`、`process_image_nt_path(pid)`、`same_image_directory(pid)` | **`OpenProcess` を使わない**（S3）。`NtQuerySystemInformation(SystemProcessInformation)` で PID と `CreateTime` の組を、`SystemProcessIdInformation` で NT 形式の実行ファイルのパスを読む。別ユーザーのプロセスでも読める。自分の作成時刻は `GetProcessTimes` |
| `protected_dir` | `program_data_dir()`、`ensure_protected_dir(DataDir)`、`ProtectedDir::{path, quarantined, replace_file}`、`FileLock::acquire`、各 SDDL 定数 | `SHGetKnownFolderPath(FOLDERID_ProgramData)`、`CreateDirectoryW`＋SA、`CreateFileW(FILE_FLAG_BACKUP_SEMANTICS \| FILE_FLAG_OPEN_REPARSE_POINT)`、`GetFileInformationByHandleEx(FileAttributeTagInfo)`、`GetSecurityInfo`、`GetSecurityDescriptorControl`、`GetAce`、`SetFileInformationByHandle(FileRenameInfo)`（先回りして作られたフォルダーの隔離。S1）、`FlushFileBuffers`（一時ファイルとフォルダー）、`MoveFileExW(REPLACE_EXISTING \| WRITE_THROUGH)`（C10）、`LockFileEx`（D.9） |
| `session` | `boot_counter()`、`boot_id()`、`legacy_boot_guid()`、`boot_time_hint()`、`current_boot()`、`random_bytes()`、`new_uuid()`、`restart_pc()`、`register_post_reboot()`、`unregister_post_reboot()`、`RUN_ONCE_VALUE` | **0.1.1 から**起動 ID は `KUSER_SHARED_DATA.BootId`（0x7FFE0000 + 0x2C4 の u32 を volatile で読む。ローダーが起動のたびに 1 増やす。C2、C.10）。0.1.0 の `NtQuerySystemInformation(SystemBootEnvironmentInformation = 90)` の `BootIdentifier` は、0.1.x の記録を判定するためだけに `legacy_boot_guid()` で読む（起動ごとに変わらない PC があった。`docs/research/boot-id.md`）。`SystemTimeOfDayInformation` の `BootTime - BootTimeBias`（履歴の診断用。0.1.x の記録の判定にも使う）、`BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)`、`AdjustTokenPrivileges(SE_SHUTDOWN_NAME)` → `InitiateShutdownW`、HKCU の `RunOnce` |

`Error` に次のバリアントを足した: `NotKeyboard`、`Cancelled`、`Timeout`、`Insecure`、`PeerMismatch`、`ValueNotAllowed`。

**`ReadIssue` の種類**（`ReadIssueKind`。S2、C3）

| 種類 | 読めなかったもの | 書き込み |
|---|---|---|
| `Locate`、`Identity`、`Driver`、`Presence` | devnode の特定、インスタンス ID、サービス（ドライバー）、present | **止める**（`blocks_writes`）。INV-PS2 と許可リストが、間違った一覧で判断されるおそれがあるため |
| `Status`、`Container` | DevNodeStatus / ProblemCode、ContainerId | 続ける。そのフィールドが `None` になり、そのキーボードのその場でのリセットは `StatusUnknown` / `UnknownContainer` で禁止される |
| `Topology` | 親の系列、バスのサービス | 続ける。接続方式が `Unknown` になれば、リセットは禁止される |
| `Values` | override の値や全体の値（型違い、キーが開けない） | 続ける。エンジンは値をすべて `RegistryBackend` で読み直し、想定外の型は `RegValue::Other` として扱う |
| `Descriptive`、`RawInput`、`Environment` | 表示名とハードウェア ID、Raw Input、入力方式と OS の情報 | 続ける（警告）。Raw Input が読めなければ、確認の表示は「未確認」になる |

`windows` クレートの feature は、実装する人が必要な分だけ足す。見込みは `Win32_Security`、`Win32_Security_Authorization`、`Win32_Security_Cryptography`、`Win32_Storage_FileSystem`（`GetFileVersionInfoW` もここ）、`Win32_System_Pipes`、`Win32_System_IO`、`Win32_System_Threading`、`Win32_System_Shutdown`、`Win32_System_Console`、`Win32_UI_Shell`、`Wdk_System_SystemInformation`。`SystemBootEnvironmentInformation`（= 90）と `SYSTEM_BOOT_ENVIRONMENT_INFORMATION`、`SystemProcessIdInformation`（= 88）と `SYSTEM_PROCESS_ID_INFORMATION` が `windows` クレートにない場合は、`#[repr(C)]` で自前に定義する。

### A.4 mklm-engine

```rust
pub struct Engine<R: RegistryBackend, D: DeviceController, H: Host> { .. }
impl Engine {
    fn set_layout(&mut self, &SetLayoutParams, &mut dyn EventSink) -> Result<OperationResult, EngineError>;
    fn migrate(&mut self, &MigrateParams, ..);
    fn revert(&mut self, &OpId, &ApplyOptions, ..);
    fn confirm(&mut self, &OpId, ..);
    fn restore_baseline(&mut self, &RestoreBaselineParams, ..);   // mode: Interactive | Silent
    fn recover(&mut self, &ApplyOptions, ..);
    fn undo_open(&mut self, &ApplyOptions, ..);
    fn resolve_conflict(&mut self, &ResolveParams, ..);
    fn cleanup_values(&mut self, &CleanupParams, ..);                  // M3（D.11）
    fn set_machine_settings(&mut self, &MachineSettingsParams, ..);    // M3（D.12）
    fn read_journal(&self) -> Result<Journal, EngineError>;
}
```

- `backend`、`device`、`host`、`sink`: エンジンが外界に触れる口（B 章）
- `params`: エンジン専用の引数の型。`RestoreMode::Silent` を作るのは、helper の固定コマンドライン `--uninstall-restore`（M5）だけ
- `memory`: インメモリ実装と故障の注入（B.4）
- `win`（Windows のみ）: `WinRegistry`、`WinDevices`、`WinHost`。昇格したプロセスでだけ作る
- `error`: `EngineError` と `to_info()`（`mklm_core::ErrorInfo` への変換）

以前の `execute(&Request)` はなくした。パイプの要求を写すのは helper の `dispatch` の役目（S5）。

### A.5 mklm-ipc

| モジュール | 中身 |
|---|---|
| `args` | `HelperArgs`（パイプ名、nonce、呼び出し元の PID）、`HELPER_ARGS_PATTERN`、`command_line_tail`、`PipeName`、`Nonce`（`Debug` に値を出さない） |
| `frame` | 4 バイトの長さ＋JSON、上限 256 KiB（`MAX_FRAME_LEN`）、`Frame { v, seq, body }`、`FrameHeader { v, seq }` |
| `message` | `CallerMessage`、`HelperMessage`、`Request` とその中身（パイプで許すものだけ）、`Hello` / `Welcome`（ビルド ID を含む）。`Event`、`Decision`、`OperationResult` などは `mklm_core::report` を再公開する |
| `handshake` | `verify_hello`、`verify_welcome`（版、ビルド ID、nonce、PID）、タイムアウトの定数、`HEARTBEAT_INTERVAL` |

`PROTOCOL_VERSION` は、`message` か、メッセージが含む core の型（`report` を含む）を変えたら上げる。呼び出し元と helper は同じインストールに含まれるので、完全一致だけを受け付ける。加えて、ビルド ID（E.3）の完全一致を求める。版を上げ忘れた変更や、片方だけ再ビルドした開発中の食い違いを検出するため（S11）。

### A.6 mklm-helper

- マニフェストは `requireAdministrator`、サブシステムは `windows`（コンソールを出さない）。
- `[[bin]] test = false`。昇格が必要な exe はテストハーネスとして実行できないため。ロジックは ipc と engine に置いてそちらでテストする。
- 最初に `restrict_dll_search()` を呼ぶ。失敗したら終了する（管理者権限のプロセスなので、警告では済ませない）。続けて `SetCurrentDirectoryW(System32)` を呼ぶ（起動側も `lpDirectory` に System32 を渡す）。
- HKCU と `%APPDATA%` を読まない。デバイスは自分で列挙し直す。
- パイプに書くのは**書き込み専用のスレッド**だけ。エンジンのイベントはチャネル経由で渡し、要求の実行中は 10 秒ごとに `Event::Heartbeat` を送る。エンジンが `restart()` やロック待ちで止まっていても、呼び出し元は helper が生きていると分かる（S7）。
- `dispatch` で、パイプの `Request` を 1 つずつエンジンの呼び出しに写す（S5）。
- 終了する前に `WinDevices::join_pending` で、期限を過ぎたリセットのスレッドが終わるのを待つ（C18）。
- **M3 から（m3 WP-E3）: セッション終了への備え。** 起動直後に `SetProcessShutdownParameters(0x100, SHUTDOWN_NORETRY)`（`mklm_win::session_end::shut_down_after_callers`）で、呼び出し元（既定の 0x280）より後に終了させられるようにする。ハンドシェイクの後、表示しないトップレベル ウィンドウを持つスレッド（`SessionEndWindow`）を作る。`WM_QUERYENDSESSION` では `mklm_engine::SessionEnd::query_end_session` を呼び、動いているカウントダウンを `RevertNow` と同じ扱いで戻す（各要求のシンクを `SessionEndSink` で包み、カウントダウン中の判断待ちを 100 ms ごとに区切って確かめる。呼び出し元が先に答えていれば何もしない。届いている答えは、戻す前に 1 ms だけ読んで先に使う）。カウントダウンより前で書き込み中かリセット中の操作（`Planned` から `CountdownStarted` まで）も待つ。エンジンの取り消しの確認点でロールバックするか、始まったカウントダウンをすぐに戻す。戻した値が書かれてフラッシュされるか、操作が閉じるまで最大 3 秒待って `TRUE` を返す。`WM_ENDSESSION(TRUE)` では戻しが終わるまで最大 3 秒待つ。終了に向かう間は `check_cancelled` が true になり、新しい操作の書き込みも、変更を反映するためのリセットも始めない（カウントダウンを戻すときに元の配列を反映し直すリセットは行う）。再接続待ちの変更は `AwaitingConfirm` のまま（D.2 b）。`WM_ENDSESSION(FALSE)`（終了が取り消された）で元に戻る。どちらの準備も失敗は無視する（パイプの切断と次回起動時の回復が残る）。
- 終了コードは E.8。

### A.7 mklm-cli

- `write.rs` に M2 のコマンドを置く（F 章）。`list`、`status`、`global status` は変えない。
- 依存に engine と ipc を加えた。
- 書き込みのコマンドは、昇格していてもいなくても helper を**別プロセス**として起動する（昇格済みなら UAC なしで `CreateProcessW`。C8）。エンジンを同じプロセスで動かすのは、`recover`、`undo`、`restore` の隠しオプション `--in-process` だけ。そのときは `restrict_dll_search()` の失敗を致命的なエラーにし、`SetConsoleCtrlHandler` を登録する（F.2）。
- 下見は `mklm_core::plan_set_layout` / `plan_migration` を使い、その結果を `ExpectedPlan` として送る（S6）。

---

## B. エンジンが外界に触れる口（トレイト）

### B.1 RegistryBackend

```rust
pub trait RegistryBackend {
    fn read_value(&self, target: &WriteTarget, name: &str) -> Result<RegValue, BackendError>;
    fn list_values(&self, target: &WriteTarget) -> Result<Vec<(String, RegValue)>, BackendError>;
    fn write_value(&mut self, target: &WriteTarget, name: &str, value: &RegValue) -> Result<(), BackendError>;
    fn flush_target(&mut self, target: &WriteTarget) -> Result<(), BackendError>;
    fn read_journal(&self) -> Result<JournalDump, BackendError>;
    fn write_journal(&mut self, slot: &JournalSlot, json: &str) -> Result<(), BackendError>;
    fn delete_journal(&mut self, slot: &JournalSlot) -> Result<(), BackendError>;
    fn flush_journal(&mut self) -> Result<(), BackendError>;
    // M3（D.12）: MACHINE_SETTINGS_KEY の REG_DWORD を 1 つ書いてフラッシュする。
    fn write_machine_setting(&mut self, name: &str, value: u32) -> Result<(), BackendError>;
}
```

| 項目 | 決めごと |
|---|---|
| 対象 | `WriteTarget::Device { instance_id }`（そのデバイスのハードウェアキー）と `WriteTarget::Global`（`i8042prt\Parameters`）。デバイスのキーは必ず CfgMgr32 経由で開き、Keyboard クラス以外は拒否する（計画 1.2、1.5） |
| 値 | `RegValue::{Absent, Dword, Sz, Other}`。`write_value(Absent)` は削除。存在しない値の削除は成功扱い。`Other` は想定外の型の値をそのまま保つためのもので、baseline を戻すときにしか書かない |
| 値の名前 | `write_value` は `DEVICE_VALUE_NAMES` / `GLOBAL_VALUE_NAMES` 以外の名前を `NameNotAllowed` で拒否する。インメモリ実装も同じ（S9） |
| ハイブ | デバイスの値と全体の値は SYSTEM ハイブ、ジャーナルは SOFTWARE ハイブにある。`RegFlushKey` はキーの属する**ハイブ全体**をフラッシュする。したがって `flush_target` は SYSTEM ハイブ、`flush_journal` は SOFTWARE ハイブのフラッシュになる |
| 原子性 | 各呼び出しは 1 回のレジストリ API 呼び出しで、原子的に反映される。フラッシュが返るまで、それ以前の書き込みは永続化されたとは限らない |
| 読み取り | ジャーナルは非昇格でも読める（C.1 の ACL）。デバイスと全体の値も Everyone が読める |
| エラー | `NotFound`（全体のキーなど、作ってはいけないキーがない）、`DeviceRemoved`（devnode そのものがない。「値なし」とは区別する。C3）、`NameNotAllowed`、`AccessDenied`、`Insecure`、`Os`、および故障注入用の `Crashed` と `Injected` |

### B.2 DeviceController

```rust
pub trait DeviceController {
    fn keyboards(&mut self) -> Result<Inventory, DeviceError>;   // Inventory { keyboards, warnings }
    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError>;
    fn wait_for_arrival(&mut self, instance_id: &str, timeout: Duration) -> Result<Arrival, DeviceError>;
    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError>;
}
```

- `keyboards()`: phantom を含むすべての Keyboard クラスの devnode（`read_keyboards(true, …)`）。**`blocks_writes` な `ReadIssue`（`Locate`、`Identity`、`Driver`、`Presence`）があるときだけ** `DeviceError::Incomplete` を返し、エンジンは何も書かない。それ以外の問題は `Inventory::warnings` に入り、結果の警告になる（A.3 の表。S2、C3）。`overrides` は信用しない。エンジンがすべての値を `RegistryBackend` で読み直す（値の出どころを 1 つにする）。型違いの override があっても止まらない（読み直すと `RegValue::Other` になる）。
- `restart()`: `DIF_PROPERTYCHANGE`。エンジンは `live_reset_bans` が空のキーボードにだけ、1 台ずつ呼ぶ。Windows 実装は別スレッドで呼び、`EngineConfig::restart_timeout`（20 秒）を過ぎたら `RestartOutcome::TimedOut` を返す（`NeedsReboot` と同じ扱い）。期限を過ぎたスレッドは保持しておき、helper は終了する前にそれが戻るのを待つ（`WinDevices::join_pending`）。プロセスの終了でクラスインストーラーの呼び出しを途中で断ち切らないため（C18）。
- `wait_for_arrival()`: 250 ms ごとに `devnode_state`（present、`DN_STARTED`、問題コード 0）と `raw_keyboards` を見る。Raw Input に現れたら `Arrival::Started { reported }`、期限を過ぎたら `TimedOut`。
- `reported_type()`: `raw_keyboards` から探す。

### B.3 Host

```rust
pub trait Host {
    type Lock;
    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError>;
    fn now(&self) -> Timestamp;
    fn monotonic(&self) -> Duration;
    fn new_op_id(&mut self) -> Result<OpId, HostError>;
    fn boot_id(&self) -> Result<BootId, HostError>;
    fn boot_time_hint(&self) -> Option<u64>;
    fn legacy_boot_guid(&self) -> Option<BootId>; // 0.1.1
    fn current_process(&self) -> Result<ProcessIdentity, HostError>;
    fn liveness(&self, process: &ProcessIdentity) -> Liveness;
    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError>;
    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError>;
    fn drain_warnings(&mut self) -> Vec<String>;
}
```

- `now()` は表示と整理にだけ使う。判定は壁時計に頼らない。
- カウントダウンは `EventSink::wait_decision(1 秒)` の呼び出し回数で数え、あわせて `monotonic()` で「秒数＋`countdown_slack`（5 秒）」を上限にする。シンクが 1 秒を守らずに待ち続けても、カウントダウンは終わる（C18）。テストの偽物の単調時計は、テストが進めたときだけ進む。
- `boot_id()` は起動ごとの ID（C2）。**0.1.1 から** `KUSER_SHARED_DATA.BootId` のカウンター形式（C.10「起動 ID」）。`boot_time_hint()` は履歴に記録する診断用の値で、判断に使うのは 0.1.x の記録の判定だけ。`legacy_boot_guid()`（0.1.1）は 0.1.x が起動 ID にしていたローダーの GUID で、その判定にだけ使う。既定の実装は持たせない（どのホストも転送しなければならない）。エンジンは `open()` の最初にこの 3 つから `CurrentBoot` を作り、ジャーナルを読んだ直後に `adopt_legacy_boots` を行う（D.1）。
- `liveness()` は、ロックを持つエンジンの判断には使わない（C3。C.7）。履歴と結果の表示のため。
- `system32_file_exists()` は計画 1.5 の「`LayerDriver JPN` の DLL が System32 に実在すること」の確認（S8）。
- `write_recovery_assets()` は耐久的に書く（G.1。C10）。
- `drain_warnings()` は、ホストが自分で出した警告（先回りして作られたフォルダーを隔離した、など。S1）を結果に載せるため。

### B.4 インメモリ実装と故障の注入（`mklm_engine::memory`）

**MemoryRegistry**

- 変更を伴う呼び出し（値の書き込みと削除、ジャーナルの書き込みと削除、フラッシュ）を順にログに残し、数える。
- `FaultPlan { crash_after, fail_at, deny_target }`
  - `crash_after = n`: n 回目まで成功し、以降の呼び出しは全部 `BackendError::Crashed`（プロセスが死んだ）。`Some(0)` は最初の書き込みの前に死ぬ。
  - `fail_at = n`: n 回目だけ `BackendError::Injected` で失敗し、以降は動く（エラー処理のテスト）。
  - `deny_target = Some(t)`: `t` への書き込みが**毎回** `AccessDenied` になる。`after_crash` の後も続く（EDR や改ざん防止機能の再現。C3）。
- `crash_images()` は、クラッシュ後にあり得る永続状態をすべて返す。
  - `CrashImage::ProcessKill`: 完了した呼び出しはすべて残る（OS は生きている）。
  - `CrashImage::PowerLoss { system_calls, software_calls }`: ハイブごとに、最後に完了したフラッシュまでは必ず残り、それ以降は任意の長さの**先頭部分**が残る（遅延書き込みがどこまで進んだか分からないが、同じハイブの中で順序が入れ替わることはない、というモデル）。2 つのハイブの組み合わせをすべて列挙する。
- `after_crash(image)` は、その状態を持つ新しい `MemoryRegistry` を返す（故障は `deny_target` だけを引き継ぐ。ログは空）。
- `seed()` は初期状態を作る（数えない）。`outside_edit()` は設定アプリや他の管理者による変更を再現する（数えない、失敗しない、すぐに永続化される）。`remove_devnode()` はデバイス マネージャーでの削除を再現する（以降、その devnode の読み書きは `DeviceRemoved`）。

**FakeDevices**

- キーボードの一覧は固定（`mklm_core::fixtures` を使う）。報告される種類は、リセットや再接続のときに `MemoryRegistry` の値から `predict_type` で計算し直す（kbdhid と同じ振る舞い）。
- `FakeReset::{Applies, NeedsReboot, NeverArrives, ArrivesUnchanged, Fails, Hangs}` で、リセットに対する反応をキーボードごとに決める。`Hangs` は `RestartOutcome::TimedOut`。
- `set_incomplete()`（書き込みを止める読み取りの問題）、`set_warnings()`（止めない問題）、`reconnect()`、`reboot()`（すべての報告値をレジストリから計算し直す）、`remove()`（一覧から消し、`remove_devnode` も行う）。

**FakeHost**

- `FakeLockCell` を複数の `FakeHost` で共有し、ロックの競合を再現する。
- `reboot()` で起動 ID を変え（0.1.1 から、カウンター形式の ID のカウンターを 1 増やす。最初は 1）、それまでのプロセスを全部死んだことにする。`legacy_boot_guid()` は既定では 0.1.0 のころの偽の GUID の列を返し、`set_legacy_guid()` で固定できる（再起動しても変わらない PC の再現）。`boot_time_hint()` は `set_boot_time()` で決める（既定は `None`）。`become_process()` で別のプロセス（例: 回復を行う GUI）として動く。`kill()` で特定のプロセスを死んだことにする。`advance()` で壁時計と単調時計を進める。
- `set_fail_assets(true)` で復旧用ファイルの書き込みを失敗させる（起動時の値を変える操作は `RecoveryAssetsUnavailable` で何も書かずに止まり、HID だけの操作は警告付きで続く）。`remove_system32_file()` で DLL の欠落を再現する。`push_warning()` でホストの警告を積む。

**ScriptedSink**

- イベントを記録し、決めておいた `DecisionPoll` を順に返す。尽きたら `NoDecision`。
- `cancel_after(n)`: n 個のイベントを受け取った後は、呼び出し元が去ったものとして `check_cancelled()` が true、`wait_decision` が `Disconnected` を返す（S4）。

### B.5 EventSink

```rust
pub trait EventSink {
    fn event(&mut self, event: &Event);
    fn check_cancelled(&mut self) -> bool;                       // 待たない
    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll;
}
```

- `check_cancelled()`: 待たずに、呼び出し元が去ったか（パイプの切断、`Bye`、同じプロセスで動く場合の Ctrl+C）を返す。helper は `PeekNamedPipe` で確かめる。エンジンが聞くのは 2 か所だけ（S4、C13）。
  1. `Planned` を書く直前: true なら何も書かずに `Cancelled` で終える（エントリも作らない）。
  2. `Restarting` に進む直前: true ならリセットせずにロールバックする（`Written` → `RevertPending` → `Reverted`、`failure = CallerDisconnected`）。変更はまだ効いていないので、`apply_pending` もない。
  その間（`Planned` から `Written` まで）は、始めた書き込みを書き切る。
- `wait_decision()` の**契約**: 決定が届くか、呼び出し元が去るか、`timeout` が過ぎるまで戻らない。早めに `NoDecision` を返してはならない。`NullSink` もこの契約に従って `timeout` の間眠る（以前の `NullSink` はすぐに返したので、カウントダウンが 0 秒になった。S4）。

---

## C. ジャーナル

### C.1 保存場所と ACL

```
HKLM\SOFTWARE\SHIN DATA CENTER\MKLM               （MSI のコンポーネントにはしない。計画 2.2）
  Journal                        StoreVersion = REG_DWORD 1
    Ops                          <op_id>                 = REG_SZ（JournalEntry の JSON）
    Baselines                    <ValueKey::canonical>   = REG_SZ（BaselineRecord の JSON）
```

- ACL（`JOURNAL_KEY_SDDL`）: `O:BAG:SYD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KR;;;BU)`。書けるのは SYSTEM と Administrators だけ。Users は読める。非昇格の GUI と CLI が、UAC なしで「反映待ち」を表示し、回復が必要かを判断するため。
- 書き込むときは毎回、`SHIN DATA CENTER` 以下の各キーの所有者が BA か SY で、それ以外の SID に書き込み系の権利を与える ACE がないことを確かめる。問題があれば `BackendError::Insecure` で止める。標準ユーザーは HKLM\SOFTWARE の下にキーを作れないので、先回りして作られる心配はない（ファイル側は D.9）。これは他のツールによる改変への備え。
- `Baselines` の値の名前（`ValueKey::canonical`）: `device|<大文字のインスタンス ID>|<値の名前>` または `global|<値の名前>`。インスタンス ID は大文字小文字を区別せずに比べるため、大文字にそろえる。

### C.2 JSON を REG_SZ に入れる理由

1. **遷移が 1 回の書き込みで済む。** 状態、`last_written`、履歴がエントリ全体としてまとめて置き換わるので、1 回の `RegSetValueExW` で原子的に遷移できる。サブキーに値を分けて持つと、1 回の遷移に複数の書き込みが要り、その途中の状態をまた設計しなければならない。
2. **スキーマを進化させやすい。** `schema_version` と serde で古い版を読み込める。任意のフィールドを足すだけなら `#[serde(default)]` で済む（C.10）。
3. **人が読める。** regedit、`reg query`、`reg export` でそのまま読めるので、サポートや手動復旧で使える。
4. **大きさは問題にならない。** 1 件はふつう 1〜3 KB、移行でも 10 KB 程度。レジストリの値の上限（約 1 MB）よりずっと小さい。「2 KB を超える値はファイルに」という指針は性能上の目安で、書く回数が少ない MKLM には当てはまらない。
5. **ほかの案を採らない理由。**
   - TxR は Microsoft が非推奨にしている（計画 2.3）。
   - ProgramData のファイルにすると、NTFS とレジストリの間で書き込みの順序を保証する仕組みを別に作る必要がある。計画 2.2 もジャーナルの場所を HKLM と決めている。

### C.3 エントリの項目

**JournalEntry**

| 項目 | 型 | 意味 |
|---|---|---|
| `schema_version` | u32 | `JOURNAL_SCHEMA_VERSION`（= 1）。**M3 から**: 読める最新は 2。書くときは種類で決まり（`OpKind::schema_version`）、`Cleanup` だけが 2、それ以外は 1 のまま（C.10、m3 K.13） |
| `op_id` | `OpId` | 小文字でハイフン付きの UUID v4（波かっこなし）。値の名前にも使う |
| `seq` | u64 | 作成順（既存の最大値＋1）。値ごとの「最新の操作」を決める |
| `kind` | `OpKind` | `SetLayout { requested, instance_ids, layout }`、`Migrate { standard, assignments }`、`RestoreBaseline { scope, silent, supersedes }`（`supersedes` は D.5）。M3 で `Cleanup { instance_id, names }`（D.11） |
| `state` | `OpState` | C.4 |
| `boot_id` | `BootId` | 直近の書き込みフェーズ（`Planned` と `RevertPending`）の起動 ID（C2）。0.1.1 からは `KUSER_SHARED_DATA.BootId` のカウンター形式（`00000007-0000-8000-8000-000000000000`）。0.1.0 はローダーの GUID を書いた（C.10「起動 ID」） |
| `owner` | `ProcessIdentity` | 直近の書き込みフェーズを行ったプロセス（PID と作成時刻）。表示用。判断はロックで行う（C3） |
| `created_at`、`updated_at` | `Timestamp` | 表示と整理用（UTC ミリ秒） |
| `apply` | `Option<PendingAction>` | 変更の反映方法（`ResetKeyboard`、`Reconnect`、`RestartPc`） |
| `countdown` | `Option<Countdown>` | リセット後の `AwaitingConfirm` の間だけ設定する（`seconds`、`deadline`） |
| `records` | `Vec<ValueRecord>` | 前向きに書く順（`CheckedPlan.steps` の順） |
| `context` | `Vec<ContextValue>` | 移行のときのスナップショット（計画 1.3 の手順 1）。情報用で、自動の復元には使わない |
| `failure` | `Option<FailureReason>` | 利用者が求めたとおりに終わらなかった理由。`Failed` と、ロールバックで終わった `Reverted` / `RevertedPendingReboot` に付く |
| `revert_mode` | `Option<RevertMode>` | `RevertPending` の間だけ設定する。`Revert`、`Rollback`、`Resolution`（C5） |
| `apply_pending` | `Option<ApplyPending>` | 保存値がまだ効いていないかもしれないこと。`{ action, instance_ids, since }`（C1、C.11） |
| `history` | `Vec<TransitionRecord>` | 遷移の記録（最大 64 行）。各行に診断用の `boot_time_hint` を持つ（0.1.1 からは、0.1.x の起動 ID の判定にも使う。C.10）。書き換えない |

**ValueRecord**

| 項目 | 意味 |
|---|---|
| `target` | `WriteTarget`（`Device { instance_id }` または `Global`） |
| `key_path` | コントロールセットからの相対パス。`Enum\<インスタンス ID>\Device Parameters` または `Services\i8042prt\Parameters`。オフライン復旧用で、エンジン自身は Enum のパスを開かない |
| `name` | 値の名前 |
| `baseline` | 初めて変える前の値（`BaselineRecord` の写し） |
| `before` | この操作の直前の値 |
| `intended` | この操作が書く値 |
| `last_written` | この操作が最後に書いてフラッシュした値（書く前は `None`、取り消した後は `before`） |
| `conflict` | CAS が合わなかったときに見た現在の値。解決はこの値を CAS の期待値にする |
| `resolve_to` | 衝突の解決（`revert_mode = Resolution`）を書いている間、**すべての記録に**設定する。ユーザーが選んだ値。`KeepCurrent` はそのとき見た現在の値（C5、D.8） |
| `write_error` | 書き込みが繰り返し失敗したときの最後のエラー。このとき操作は `Conflict` になる（C3） |
| `skipped` | 戻すときに意図して飛ばした理由。`DeviceRemoved`（devnode が消えた）、`ConflictSkipped`（サイレントモードの衝突） |

**JSON の例**（Keychron を US から JIS に。カウントダウン中）

```json
{
  "schema_version": 1,
  "op_id": "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f",
  "seq": 7,
  "kind": {
    "kind": "set-layout",
    "requested": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000",
    "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
    "layout": "jis"
  },
  "state": "awaiting-confirm",
  "boot_id": "00000007-0000-8000-8000-000000000000",
  "owner": { "pid": 12345, "creation_time": 134036790000000000 },
  "created_at": 1790500000000,
  "updated_at": 1790500004000,
  "apply": "reset-keyboard",
  "countdown": { "seconds": 20, "deadline": 1790500024000 },
  "records": [
    {
      "target": { "kind": "device", "instance_id": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000" },
      "key_path": "Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters",
      "name": "KeyboardTypeOverride",
      "baseline": { "kind": "dword", "value": 4 },
      "before": { "kind": "dword", "value": 4 },
      "intended": { "kind": "dword", "value": 7 },
      "last_written": { "kind": "dword", "value": 7 },
      "conflict": null,
      "resolve_to": null,
      "write_error": null,
      "skipped": null
    }
  ],
  "context": [],
  "failure": null,
  "revert_mode": null,
  "apply_pending": null,
  "history": [
    { "from": null, "to": "planned", "at": 1790500000000, "boot": "00000007-0000-8000-8000-000000000000",
      "by": { "pid": 12345, "creation_time": 134036790000000000 }, "reason": "set-layout",
      "boot_time_hint": 134036748000000000 }
  ]
}
```

（`KeyboardSubtypeOverride` の記録（0 → 2）と、`written`、`restarting`、`awaiting-confirm` の履歴は省略した。起動 ID は 0.1.1 のカウンター形式。0.1.0 が書いたエントリでは、`9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d` のような GUID になっている。）

### C.4 状態と遷移

| 状態 | 分類 | 意味 |
|---|---|---|
| `Planned` | open、書き込み中 | エントリと baseline をフラッシュ済み。対象への書き込みを始めているかもしれない |
| `Written` | open、書き込み中 | 対象の値をすべて書いてフラッシュ済み |
| `Restarting` | open、書き込み中 | キーボードをその場でリセットしている |
| `AwaitingConfirm` | open | 反映済み（または再接続待ち）。ユーザーが「このままにする / 元に戻す」を選ぶ。`countdown` があるときは、答えがなければ元に戻す（所有者がロックを持っている） |
| `PendingReboot` | open | 書き込み済み。次の PC の再起動で反映される |
| `RevertPending` | open、書き込み中 | `revert_mode` の内容（取り消し、ロールバック、解決）を書いている |
| `Conflict` | open | 値が想定外、または書き込みが繰り返し失敗した。ユーザーの判断を待つ |
| `Confirmed` | closed | 確定。取り消しはできる（最新の操作なら。「導入前に戻す」の操作は除く。C6） |
| `Reverted` | closed | `before` に戻した。リセットで反映できなかった HID は `apply_pending` に残る。ロールバックなら `failure` に理由 |
| `RevertedPendingReboot` | closed | 戻したが、起動時の値を含むか、リセットしたキーボードが戻らなかったので PC の再起動が必要 |
| `Failed` | closed | 変更を一度も残さずに終わった。何も書いていない（`NothingWritten`、`ConcurrentChange`）、利用者が外部の値を残した（`ConflictKeptCurrent`）、「導入前に戻す」に置き換えられた（`Superseded`） |

**以前との違い**: ロールバックの終わりを `Failed` から `Reverted` / `RevertedPendingReboot`（`failure` 付き）に改めた。途中まで書かれた起動時の値は、電源断の後の起動で一度読まれているかもしれないので、閉じた後も「再起動が必要」を示す必要がある（C1）。

open な状態の操作がある間は、`set` と `migrate`、更新（M5）、自動再適用（M4）を始めない（計画 2.2）。「導入前に戻す」は、書き込み中ではない open な操作を `Failed(Superseded)` で閉じてから進む（C7、D.5）。通常、open な操作は同時に 1 件まで。

**遷移表**（`OpState::can_transition_to` はこの表そのもの）

| 元 | 先 | きっかけ |
|---|---|---|
| （新規） | `Planned` | 計画をジャーナルに書いてフラッシュした |
| `Planned` | `Written` | 全ステップを書いてフラッシュした（回復の `CompleteForward` も） |
| `Planned` | `Failed` | 何も書いていない（最初の CAS が合わない。回復で全値が `before`） |
| `Planned` | `RevertPending` | 書き込みエラー、回復のロールバック |
| `Planned` | `PendingReboot` / `AwaitingConfirm` | 回復のロールフォワード（全値が `intended`） |
| `Planned` | `Conflict` | 回復で、`before` でも `intended` でもない値を見つけた。INV-PS2 が壊れていてロールバックもできない |
| `Written` | `Restarting` | リセットの経路 |
| `Written` | `AwaitingConfirm` | 再接続の経路。回復のロールフォワード |
| `Written` | `PendingReboot` | PC の再起動の経路 |
| `Written` | `Confirmed` | サイレントの「導入前に戻す」（その回復も） |
| `Written` | `RevertPending` | `Restarting` の直前の切断（S4）、回復（リセット経路の未確認、INV-PS2） |
| `Written` | `Conflict` | 回復で、外部の変更を見つけた |
| `Restarting` | `AwaitingConfirm` | キーボードが戻ってきた（カウントダウンあり。種類が変わっていなければ再接続待ち） |
| `Restarting` | `RevertPending` | 戻らない、`NeedsReboot`、期限切れ、エラー、回復 |
| `AwaitingConfirm` | `Confirmed` | Keep |
| `AwaitingConfirm` | `RevertPending` | RevertNow、カウントダウン切れ、切断、`revert`、`undo`、回復 |
| `AwaitingConfirm` | `Conflict` | 確定しようとしたら値が `intended` でなかった |
| `AwaitingConfirm` | `Failed` | 「導入前に戻す」に置き換えられた（`Superseded`） |
| `PendingReboot` | `AwaitingConfirm` | 起動 ID が変わり、値が `intended` のままで、INV-PS2 も保たれている |
| `PendingReboot` | `RevertPending` | 再起動の前に取り消した（`revert`、`undo`） |
| `PendingReboot` | `Conflict` | 再起動後に値が `intended` でなかった、または INV-PS2 が壊れていた |
| `PendingReboot` | `Failed` | `Superseded` |
| `Confirmed` | `RevertPending` | `revert`（最新の操作のみ。「導入前に戻す」の操作を除く） |
| `RevertPending` | `Reverted` | 全値を戻した（HID のみ） |
| `RevertPending` | `RevertedPendingReboot` | 全値を戻した（起動時の値を含む）。リセットしたキーボードが戻らなかった |
| `RevertPending` | `Confirmed` | 解決で、全値が `intended` になった |
| `RevertPending` | `Failed` | 解決で、`intended` でも `before` でもない値が残った（`ConflictKeptCurrent`） |
| `RevertPending` | `Conflict` | CAS が合わなかった。書き込みが繰り返し失敗した（C3） |
| `RevertedPendingReboot` | `Reverted` | 起動 ID が変わった |
| `Conflict` | `RevertPending` | 解決で値を書く。`undo` |
| `Conflict` | `Confirmed` / `Reverted` / `RevertedPendingReboot` / `Failed` | 解決で書く値がない（`state_after_resolution`）。`Failed` は `Superseded` も |

`Reverted` と `Failed` は終端。`Confirmed` は取り消しのためだけに先へ進める。

**状態を変えない書き直し**（`can_transition_to` を通らない。1 回の J → FJ）

- 引き継ぎ（`JournalEntry::take_over`）: 回復が書き込み中のエントリを続けるとき、`boot_id` と `owner` を自分に変え、履歴に 1 行足す。表示のためで、保護はロックが担う。
- `apply_pending` の設定と消去（C.11）。

### C.5 書き込みとフラッシュの順序

記号: J = `write_journal(Op)`、B = `write_journal(Baseline)`、FJ = `flush_journal`、T = `write_value`、FT = `flush_target`。

**新しい操作（`set` / `migrate` / `restore --baseline`）**

1. 計画する（D.1〜D.5。まだ何も書かない）。
2. 復旧用ファイルを、今の `Baselines` とこの操作で新しく記録する baseline から作り、**耐久的に**書く（G.1）。起動時の値を変える操作で失敗したら、何も書かずに `RecoveryAssetsUnavailable` で終える。HID だけの操作なら警告して続ける（C10）。この後で操作が失敗しても、ファイルに余分に載るのは「その時点の値」なので害はなく、次に書き直すときに消える。
3. `sink.check_cancelled()` が true なら、何も書かずに `Cancelled` で終える（S4）。
4. まだ baseline がない値ごとに B（値 = 現在の値 = `before`）。
5. J（`Planned`、`last_written = None`）→ **FJ**。B と J はこの 1 回の FJ でまとめて永続化される。
6. ステップの順に（`set` と `migrate` は `CheckedPlan.steps` の順 = i8042prt → その他のキーボード → 全体。「導入前に戻す」は `plan_restore` の段階の順）、ステップ（= 1 つのキー）ごとに:
   各値について、読んで `before` と一致することを確かめ（CAS。`value_eq`）、T を行う → **FT**。
7. J（`Written`、`last_written = intended`）→ **FJ**。

**対象の値を書かない遷移**（`Restarting`、`AwaitingConfirm`、`PendingReboot`、`Confirmed`、`Conflict`、`Failed`、`RevertedPendingReboot` → `Reverted`）

- J → **FJ**。遷移を知らせるイベント（`StateChanged`）は FJ の後に送る。GUI の「今すぐ再起動」は `PendingReboot` の `StateChanged` を受け取るまで押せない（計画 2.3）。

**取り消し、ロールバック、解決、undo**

1. J（`RevertPending`、`revert_mode`、`boot_id` と `owner` を今のものに。`Resolution` ならすべての記録に `resolve_to`）→ **FJ**。
2. `plan_restore` の段階の順（下の「戻す順序」）に、ステップごとに: 読む → 戻す値と同じなら飛ばす、`Expect` と一致すれば T、devnode が消えていれば `skipped = DeviceRemoved`、どれでもなければ `conflict` に記録 → **FT**。**1 つでも書けなかった値（CAS の不一致、書き込みエラー）があるステップの後は、残りのステップを書かない。** 後の段階は前の段階が済んだことを前提に INV-PS2 を確かめているため。
3. 書き込みがクラッシュ以外のエラー（`AccessDenied`、`Os` など）で失敗したら、1 回だけやり直す。それでも失敗したら、その記録の `write_error` に残し、2 の規則でそこで止める（C3）。
4. J（最終状態、`last_written = 戻した値`、`failure`、`apply_pending`）→ **FJ**。書けなかった値がある場合の最終状態は `Conflict`。

**戻す順序**（`plan_restore`。C4）

戻す計画は「全体 → その他 → i8042prt」の固定順ではなく、**変化の向き**で決める。固定順が正しいのは固定モードへ戻す場合だけで、キーボードごとモードへ向かう戻し（「導入前に戻す」の取り消しや、移行を残す解決）では、全体のペアを消す前に PS/2 を固定しなければならないため。各ステップを次の段階に分け、この順に書く。

| 段階（`RestorePhase`） | 含むステップ |
|---|---|
| 1. `AddPins` | i8042prt のキーボードで、書いた後に Type と Subtype の両方がある（固定を足す、値を変える） |
| 2. `GlobalWithPair` | 全体のキーで、書いた後に固定のペアがある（ペアを足す、残す） |
| 3. `Other` | HID などそれ以外のキーボード |
| 4. `GlobalWithoutPair` | 全体のキーで、書いた後に固定のペアがない（ペアを消す、もともとない） |
| 5. `RemovePins` | i8042prt のキーボードで、書いた後に固定がない（固定を外す） |

最終状態が INV-PS2 を満たすなら、途中のどの状態も満たす。ペアを消す（段階 4）までに固定は増える一方で、固定を外す（段階 5）ときには最終状態のペアがすでにある。それでも各ステップの後に INV-PS2 を確かめ直す（`check_plan` と同じ）。取り消しとロールバックでは、これは結果として前向きの順の逆になる。

**確定した「導入前に戻す」の後始末**

- J（`Confirmed`）→ FJ → 戻した値の `Baselines` の記録を削除（`delete_journal`）→ FJ → 復旧用ファイルを書き直す（失敗しても警告だけ。値はすでに baseline なので）。

**整理**

- 終端に遷移した後、同じロックの中で `delete_journal(Op)` → FJ（C.9）。

**この順序で守られること**

- **対象の値は、その変更を記したエントリが永続化される前には決して変わらない。** 対象への T の前に必ず FJ があるため。したがって、どのクラッシュ状態でも、変わった値には必ず `before` と `intended` の記録がある。
- **エントリは、永続化された値より先の状態を名乗らない。** `Written` の J は全ステップの FT の後に行うため。
- **INV-PS2 の順序が電源断でも保たれる。** i8042prt のステップを FT してから全体のステップを始めるため（戻すときは段階の順）。同じハイブの中で遅延書き込みが順序を入れ替えないとしても、ステップの間の FT があれば、その仮定に頼らずに済む（I.2）。
- **起動時の値が変わる前に、それを戻す手段がディスクにある。** 復旧用ファイルを耐久的に書いてからでないと、起動時の値を変える操作は始まらない（C10）。
- 回復は値を見て判断するので、J が遅れても（書いたのに状態が `Planned` のまま、など）結論は変わらない。

### C.6 baseline の意味

- `Baselines` には、MKLM が変えたことのある値ごとに、初めて変える前の値を 1 件だけ持つ（`Absent` と `Other` を含む）。
- 記録するのは新しい操作の `Planned` のとき、その値の baseline がまだない場合だけ。値は `before` と同じ。エントリより先に B を書き、同じ FJ で永続化する。
- 各記録にも `baseline` を写しておく。エントリ 1 件だけで自己完結し、オフライン用のファイルを作りやすくするため。
- **同じ値に対して操作が重なる場合**: 操作 1 が baseline b を記録する。操作 2 は記録しない（すでにあるため）。操作 2 の `before` は操作 1 の `last_written`（= 現在の値）で、`baseline` は b。操作 2 を取り消すと `before` に戻り、「導入前に戻す」と b に戻る。
- **削除するのは**、その値を含む「導入前に戻す」の操作が `Confirmed` になり、現在の値が baseline と一致したときだけ。その後に同じ値を変えると、新しい baseline を記録し直す。取り消し（revert）では削除しない。
- **`Confirmed` の「導入前に戻す」は取り消せない**（C6）。取り消すと MKLM の値が戻るのに、baseline の記録はもう消えているため、次の `set` がその値を新しい baseline として記録してしまう。利用者には「もう一度 `set` してください」と案内する（`InvalidState`）。`Confirmed` になる前（確認待ち、再起動待ち、衝突）の「導入前に戻す」は、baseline がまだあるので、取り消しも undo もできる。
- **M0 のときの手作業による変更は「MKLM の導入前」に含まれる。** 開発機の場合、Keychron の baseline は 4/0、内蔵キーボードは 7/2、全体はキーボードごとモード（Type/Subtype なし）になる。M0 より前の状態まで戻すには、M0 のバックアップ（`MKLM-backup-20260927\`）を使う。
- 初回ウィザードの「既存設定の取り込み」（計画 3.1 手順 2）は M3 で扱う。`Baselines` を明示的に書き換える操作の種類（`ImportBaseline`）を足す予定で、保存形式はそれに対応できる。

### C.7 回復の判定表

**対象にする条件**（計画 2.3、C3）

- 回復はロックを取ってから行う。所有者は、エントリが書き込み中の間とカウントダウンの間、ずっとロックを持っている。したがって、**ロックを取れた時点で見つかった書き込み中の状態（`Planned`、`Written`、`Restarting`、`RevertPending`）と `countdown` 付きの `AwaitingConfirm` は、所有者の生死によらず放棄されたもの**として回復する。同じ helper のセッションで、自分が直前の要求で残したエントリも同じ（エンジンがエラーで返った後の `Recover` が `Leave(OwnerAlive)` で何もしない、ということは起きない）。
- `PendingReboot` と `RevertedPendingReboot` は、起動 ID が今と違うときだけ先へ進める。「同じ起動」は `entry.boot_id == 今の起動 ID` の等しさだけで決める。0.1.x が書いた GUID の起動 ID は、ジャーナルを読んだときに判定済み（今の起動と判定したものは今の ID に読み替え、それ以外は前の起動として残す。C.10「起動 ID」）なので、この表は形式を区別しない。
- プロセスの生死（`Liveness`）は、ロックを取れない非昇格の GUI と CLI の判断（`attention`）にだけ使う。

**値の観測**（`observe`）: 各記録について、現在の値が `AtBefore`（`before` と同じ）、`AtIntended`（`intended` と同じ）、`Unchanged`（`before` = `intended` = 現在）、`Elsewhere`（どれとも違う）のどれかを決める。比較は `value_eq`（`LayerDriver JPN` と `OverrideKeyboardIdentifier` は大文字小文字を区別しない。C16）。devnode が消えた記録は観測から外す。

**INV-PS2 の確認**（C12）: エンジンは、今の全キーボード（phantom を含む）と全体の値で `check_inv_ps2` を求め、`RecoveryContext::inv_ps2` として渡す。書かずにエントリを先へ進める判定（ロールフォワード、再起動の観測）は、違反があれば行わない。

| 状態 | 条件 | 値 | 判定（`RecoveryDecision`） | 結果 |
|---|---|---|---|---|
| `Planned` | 「導入前に戻す」以外 | すべて `AtBefore` か `Unchanged` | `MarkNothingWritten` | `Failed(NothingWritten)` |
| `Planned` / `Written` / `Restarting` | サイレントの「導入前に戻す」以外 | `Elsewhere` がある | `Conflict` | `Conflict` |
| `Planned` | 「導入前に戻す」以外 | `AtBefore` と `AtIntended` が混在 | `RollBack(PartiallyWritten)` | `RevertPending` → `Reverted` / `RevertedPendingReboot`（`failure = Interrupted`） |
| `Written` / `Restarting` | 「導入前に戻す」以外 | `AtBefore` がある | `Conflict` | `Conflict`（`Written` の後に誰かが値を戻した） |
| `Planned` / `Written` / `Restarting` | `apply = ResetKeyboard`、「導入前に戻す」以外 | すべて `AtIntended` か `Unchanged` | `RollBack(LiveResetUnconfirmed)` | `RevertPending` → `Reverted`（`failure`、`apply_pending`） |
| `Planned` / `Written` | `apply = RestartPc`、同じ起動、「導入前に戻す」以外 | すべて `AtIntended` か `Unchanged`、INV-PS2 が成り立つ | `RollForward(PendingReboot)` | `PendingReboot` |
| `Planned` / `Written` | それ以外の `apply` か、起動が違う。「導入前に戻す」以外 | 同上 | `RollForward(AwaitingConfirm)` | `AwaitingConfirm`（`countdown` なし。`apply_pending`） |
| `Planned` / `Written` | 「導入前に戻す」以外 | すべて `AtIntended` か `Unchanged`、INV-PS2 が壊れている | `RollBack(InvPs2)`。`plan_restore` が拒否すれば `Conflict { inv_ps2 }` | `Reverted` / `RevertedPendingReboot`、または `Conflict` |
| `Planned` / `Written` / `Restarting` | 対話の「導入前に戻す」 | `AtBefore`、`AtIntended`、`Unchanged` だけ | `CompleteForward { to }` | 残りの記録を `before` の CAS で書き切る → `Written` → 起動時の値を含み同じ起動なら `PendingReboot`、それ以外は `AwaitingConfirm`（`countdown` なし、`apply_pending`） |
| `Planned` / `Written` | サイレントの「導入前に戻す」 | 何でも（`Elsewhere` は飛ばして記録） | `CompleteForward { to: Confirmed }` | 書き切る → `Written` → `Confirmed` → baseline の後始末 |
| `RevertPending` | — | — | `ContinueRevert` | `revert_mode` のとおりに続ける → 最終状態 |
| `AwaitingConfirm` | `countdown` あり、「導入前に戻す」以外 | — | `RollBack(CountdownExpired)` | `RevertPending` → `Reverted`（`failure`、`apply_pending`） |
| `AwaitingConfirm` | `countdown` あり、「導入前に戻す」 | — | `RollForward(AwaitingConfirm)` | `countdown` を外した `AwaitingConfirm`（ユーザーが決める） |
| `AwaitingConfirm` | `countdown` なし | — | `Leave(WaitingForUser)` | ユーザーに尋ねる |
| `PendingReboot` | 同じ起動 | — | `Leave(WaitingForReboot)` | 「再起動が必要」と表示 |
| `PendingReboot` | 起動が違う | すべて `AtIntended` か `Unchanged`、INV-PS2 が成り立つ | `RebootObserved(AwaitingConfirm)` | 再起動後の確認へ |
| `PendingReboot` | 起動が違う | それ以外 | `Conflict`（INV-PS2 の違反があれば `inv_ps2` 付き） | `Conflict` |
| `RevertedPendingReboot` | 起動が違う | — | `RebootObserved(Reverted)` | `Reverted` |
| `RevertedPendingReboot` | 同じ起動 | — | `Leave(WaitingForReboot)` | — |
| `Conflict` | — | — | `Leave(Conflict)` | ユーザーが解決するか undo する |
| closed な状態 | — | — | `Leave(Closed)` | `apply_pending` の掃除だけ（C.11） |

**判定の考え方**

- **ロールフォワード**（計画 2.3「全段階が終わっていて intended と一致すれば AwaitingConfirm に進める」）: 「全段階」は、すべての書き込みがフラッシュ済みであることと解釈する。進め先は、同じ起動で PC の再起動が必要なら `PendingReboot`、それ以外は `AwaitingConfirm`。**自動で確定することはない**（サイレントの「導入前に戻す」を除く）。INV-PS2 が今の機器構成で壊れていれば進めない（C12。例: 計画した後に、固定値を持たない i8042prt の devnode が現れた）。
- **リセット経路はロールフォワードしない**: その場でのリセットで反映する変更（USB）は、カウントダウンが安全網になっている。書き込み途中で止まった操作は、ユーザーが一度も確認していないので、「答えがなければ元に戻す」という規則をそのまま当てはめて戻す。やり直しは `set` をもう一度実行するだけで済む。
- **「導入前に戻す」は回復で逆向きにしない**（C14）: 利用者の意図は「MKLM の変更を外す」ことなので、止まった「導入前に戻す」をロールバックすると、その意図と逆の MKLM の値を書き戻すことになる。特にアンインストールでは、再インストール後の最初の回復で MKLM の設定が戻ってしまう。そこで前へ書き切る（`CompleteForward`）。対話の場合は確定せずに `AwaitingConfirm` / `PendingReboot` で止め、利用者が決める。サイレントの場合は `Confirmed` まで進める。
- **回復では、頼まれない限りキーボードをリセットしない**: 回復は GUI の起動時など、ユーザーが見ていないときにも動くため。値を戻した結果、キーボードの動作と保存値が食い違う場合は、`apply_pending` に残して表示する（C1、C.11）。対話的な回復（`mklm-cli recover`、GUI の「回復」ボタン）は `ApplyOptions` を持ち、利用者が別の入力手段を申告して許せば、`live_reset_bans` が空のキーボードを 1 台ずつリセットして `apply_pending` を消す（C1、C9）。
- **ロールバックも `plan_restore` を通す。** INV-PS2 を壊す場合（外部の変更で状態が変わっているなど）は、書かずに `Conflict` にする。
- **ロールバックの最終状態**: HID だけなら `Reverted`、起動時の値を含めば `RevertedPendingReboot`。どちらも `failure` に理由を残す。途中まで書かれた起動時の値は、電源断の後の起動ですでに読まれているかもしれないので、閉じた後も再起動を求める（C1）。

**死んだプロセスの書き込みのフラッシュ**: 回復は、何かを決めて動く前（`Leave` 以外）に、そのエントリの対象で `flush_target` を 1 回呼ぶ（`RegFlushKey` はハイブ全体をフラッシュする）。死んだプロセスが T の後、FT の前に止まっていると、回復が見た値はまだディスクにないかもしれない。それに頼って J を書いたり、後のステップを書いたりする前に永続化する（C.5「エントリは、永続化された値より先の状態を名乗らない」、I.2）。失敗は警告にとどめる（遅延書き込みがいずれフラッシュし、ここで失敗させると書き込み中のエントリが残るため）。読めない値（クラッシュ以外のエラー）があるエントリは判定できないので、回復が必要なもの（書き込み中かカウントダウン中）は `write_error` 付きで `Conflict` にし、それ以外はそのまま残して警告する（I7）。

**冪等性**: 回復の判定は、エントリと現在の値だけから決まる。回復が途中で止まっても、エントリは `RevertPending`（`revert_mode` 付き）か `Planned` として残る。次の回復は同じ判定で同じ最終状態にたどり着く。**クラッシュ以外の失敗は `Conflict` で止まる**ので、同じ失敗を繰り返す回復の無限ループにはならない（C3）。

**非昇格の判断**（`attention`。ロックを取らず、対象の値も読まない）

| 状態 | 条件 | `Attention` | 書き込みを止めるか（`blocks_writes`） |
|---|---|---|---|
| 書き込み中、`countdown` 付きの `AwaitingConfirm` | 同じ起動で、所有者が `Alive` | `Busy`（「別の MKLM が処理中」。helper を起動しない） | 止める |
| 同上 | 所有者が `Dead` か `Unknown`、または起動が違う | `Recover`（helper を `Recover` で起動する） | 止める |
| `AwaitingConfirm`（`countdown` なし） | — | `AwaitingUser` | 止める |
| `PendingReboot` | 同じ起動 | `WaitingForReboot` | 止める |
| `PendingReboot` | 起動が違う | `Recover`（再起動後の確認へ） | 止める |
| `Conflict` | — | `Conflict` | 止める |
| `RevertedPendingReboot` | 同じ起動 | `NeedsApply`（再起動が必要） | 止めない |
| closed な状態 | `apply_pending` がまだ有効（`apply_pending_cleared` が false） | `NeedsApply` | 止めない |
| それ以外 | — | `None` | 止めない |

`Unknown` を「生きている」ではなく「回復を試す」側に倒すのは、Windows 実装がプロセス一覧を `NtQuerySystemInformation` で読むので `Unknown` はまれで、生きている所有者がいれば helper はロックを取れずに `Busy` を返すだけだから（S3）。

### C.8 compare-and-swap と「導入前に戻す」

| 場面 | 戻す値 | `Expect`（現在の値に求めるもの） |
|---|---|---|
| 取り消し（`revert_mode = Revert`。`revert` と `undo`） | `before` | `last_written` |
| ロールバック（`revert_mode = Rollback`） | `before` | `last_written` があればそれ、なければ `intended` |
| 導入前に戻す | `baseline` | `Journal::latest_record(key).last_written`（記録がなければ `baseline`）。置き換えた（`Superseded`）操作の記録もここに含まれる |
| 衝突の解決（`revert_mode = Resolution`） | `resolve_to` | `conflict`（ユーザーが見た値）。`conflict` のない記録は `last_written` |
| 導入前に戻す、`ConflictPolicy::Overwrite` | `baseline` | `Any`（利用者が値を見たうえで上書きを選んだ） |

- 比較は `value_eq`。現在の値がすでに戻す値と同じなら、何もせずに済んだものとして扱う。
- devnode が消えた記録は `skipped = DeviceRemoved` で済んだものとする（C3）。
- どちらでもなければ Conflict。`baseline`、`before`、`intended`、`last_written`、現在の値を `ConflictInfo` で返す（計画 2.3）。サイレントモード（`ConflictPolicy::Skip`）では `skipped = ConflictSkipped` にしてログに残す。
- 読んで比べてから書くまでの間は原子的ではない。MKLM 同士はロックで直列化されるが、設定アプリなど MKLM 以外の書き手との競合は、ごく短い時間だけ残る。これは受け入れる。
- 取り消せるのは、その値を最後に変えた操作だけ（`latest_record` がその操作であること）。そうでなければ `NotLatest` を返し、「導入前に戻す」を案内する。ただし、後の操作が自分自身も戻されて（`Reverted`、`RevertedPendingReboot`、`Failed`）、値をこの操作が残した値（`last_written`）に戻しているなら、正味の変更はないので数えない（カウントダウン切れの `set` の後でも、その前の確定した変更を取り消せるように）。「導入前に戻す」の `Expect` は引き続き `latest_record` を使う。
- **解決は `Any` を使わない**（C5）。回復がクラッシュ後に解決を続けるとき、その間に利用者が設定アプリで変えた値を上書きしないため。合わなければもう一度 `Conflict` にする。

### C.9 整理（pruning）

- 終端の状態に遷移した後、同じロックの中で行う。
- 残すもの: open な操作すべて、closed な操作のうち新しい 32 件（`EngineConfig::keep_closed_ops`）、どれかの値の `latest_record` を持つ操作、`apply_pending` がまだ有効な操作。
- 残りを `delete_journal` で消してから FJ。途中で止まっても害はない。
- `Baselines` はここでは消さない（C.6 の条件でだけ消す）。

### C.10 スキーマの版と互換性

- 3 つの版がある: `JournalEntry.schema_version`、`BaselineRecord.schema_version`、`Journal\StoreVersion`（キーと値の名前の付け方）。
- 読み手は、古い版をメモリ上で変換し、次の遷移のときに新しい版で書き直す。新しい版は `JournalError::NewerSchema` として `Journal.unreadable` に入れる。読めないエントリが 1 件でもあれば、書き込みを止める（`JournalUnreadable`）。GUI と CLI は「MKLM を更新してください」と表示する。
- serde は未知のフィールドを無視するので、任意のフィールドを足すだけなら版を上げなくてよい。意味が変わる変更のときだけ上げる。レビューで足した `revert_mode`、`apply_pending`、`write_error`、`skipped`、`boot_time_hint` はすべて `#[serde(default)]` で、`BootId` の形式の変更（数値から GUID の文字列へ）は、まだどの PC にもジャーナルがない段階なので版 1 のままとする。
- 更新（M5）の新しい版は、それ以前のすべてのスキーマを読めなければならない。MSI はジャーナルのキーを持たない。open な操作がある間は更新を始めない（計画 4.2 の手順 1）ので、更新の時点で残っているのは closed な操作だけになる。
- **M3 の版 2**（m3 A.5、K.13）: `JOURNAL_SCHEMA_VERSION = 2` は `OpKind::Cleanup`（D.11）を足しただけ。版 2 で書くのは `Cleanup` のエントリだけで、ほかの種類は版 1 で書き続ける（`OpKind::schema_version`）。M2 のビルドは `Cleanup` のエントリを `NewerSchema` として扱い（書き込みを止めて「更新してください」）、ほかのエントリは読み続ける。版 1 の文書は変換なしにそのまま読める（開発機に M2 の実機テストで残ったエントリと baseline を `mklm_core::fixtures::schema_1_journal` に写し、core と engine のテストで読み書きを確かめる）。`BaselineRecord` と `StoreVersion` は 1 のまま。

**起動 ID（0.1.1。C2 の続き）**

0.1.0 は起動 ID に `SystemBootEnvironmentInformation.BootIdentifier`（ローダーの GUID）を使っていた。デスクトップ PC で、この GUID が完全な再起動を 3 回しても変わらず、`PendingReboot` の操作がいつまでも「再起動していない」と判定された（確認画面の「このままにする」が押せない、helper が確定も新しい変更も断る。`docs/research/boot-id.md`）。R9/R10 を行わないまま出荷したため。

- **今の起動 ID は `KUSER_SHARED_DATA.BootId`**（`mklm_win::session::boot_counter`）。ローダー（winload）が `\Windows\bootstat.dat` の値を起動のたびに 1 増やしてカーネルに渡す。スリープ、休止からの復帰、高速スタートアップの起動ではローダーが数えないので変わらず、再起動と完全なシャットダウンで 1 増える（H.2 R9/R10、MT-2〜MT-7 で確かめる）。
- **記録の形**: `BootId::from_boot_counter(n)` = `nnnnnnnn-0000-8000-8000-000000000000`（最初の組がカウンター。RFC 9562 のバージョン 8、バリアント 10、ほかのビットは 0）。ローダーの GUID はバージョン 1 か 4 なので、この形にはならない。この形でない ID を legacy と呼ぶ（`BootId::is_legacy`）。JSON の型は変えないので、**版は上げない**（エントリは 1、`Cleanup` は 2、`BaselineRecord` と `StoreVersion` は 1、IPC と M5b の Run 記録も同じ）。0.1.x の `BootId::parse` もこの形を読める。
- **カウンターだけを使う理由**: 「同じ起動か」は等しさで決める。危険なのは「違う起動」と誤ること（反映されていない変更を確定させてしまう。C2）。カウンターは起動ごとに 1 回だけ書かれる。GUID（高速スタートアップでの性質は未確認で、この PC では役に立たない）や起動時刻（休止をまたいで `BootTime - BootTimeBias` が動かないことは未確認）を足しても、「違う起動」の誤りが増えるだけ。カウンターが戻る（`bootstat.dat` の作り直し）と「同じ起動」と誤るが、それは安全な側で、次の再起動で直る。しかも、未解決のエントリが記録したちょうどその値に戻った場合だけ。
- **legacy の起動 ID の判定**（`JournalEntry::legacy_boot_is_current`）。対象は `boot_id` と `apply_pending.since` だけ。
  1. 今の ID と等しければ今の起動（テストでだけ起きる）。
  2. その ID の下で書いた履歴行のうち `boot_time_hint` を持つものがあり、今の起動の `BootTime - BootTimeBias` が読めれば、どれかのヒントとの差が 10 秒（`LEGACY_BOOT_TIME_TOLERANCE` = 100,000,000。FILETIME の単位）以内のときだけ今の起動。
  3. それ以外（ヒントのある行がない M2 のころのエントリや、エンジンの引き継ぎ行だけの場合、起動時刻が読めない場合）は 0.1.x と同じ: 今の GUID と等しければ今の起動、違えば前の起動、GUID が読めなければ今の起動（再起動していない。安全な側）。
- **ヒントで判定してよい理由**: ヒントも今の値も、その起動の中で読んだ `BootTime - BootTimeBias` で、時刻の変更はバイアスが吸収する（この PC では RTC の起動時刻の秒＋0.5 秒。旧開発機では 1.2 秒の時刻合わせの後も一致した）。2 つの起動の開始は、サインイン、変更、再起動、ファームウェアの分だけ少なくとも数十秒離れる。「どれかの行」で判定するのは保守的な側: 今の起動のヒントを持つ行があれば、その ID の下の書き込みがこの起動で行われたことになる。
- **読み替え（adopt）はメモリの上だけ**（`Journal::adopt_legacy_boots`）。今の起動と判定した legacy の ID は今のカウンターの ID に置き換え、前の起動と判定したものはそのまま残す（カウンターの ID と等しくならないので、どの比較も「前の起動」と読む）。エントリごとに判定する（この PC では、同じ GUID がエントリによって違う起動を指す）。何度行っても同じで、カウンターの ID と履歴は触らない。
  - 行う場所は 2 つ。エンジンの `open()`（ジャーナルを読んだ直後、関門、セッション、掃除より前。`Engine::read_journal` はそのまま）と、非昇格の `mklm_client::journal::read_journal`（GUI の `reader`、RunOnce の規則、`LiveJournal`、CLI の `start`、`keep`、`reboot`、`post-reboot`、`journal`）。
  - 判定のためだけにジャーナルを書くことはない。読み替えたエントリは、エンジンが別の理由（遷移、`apply_pending` の掃除）で書くときに新しい形で保存される。`history[].boot` は監査の記録で、判定にも使うので書き換えない。
  - `mklm-cli journal --json` は、今の起動と判定したエントリについて読み替えた `boot_id` を表示する（履歴行は保存された GUID のまま）。
- **この PC での結果**（今 = カウンター 7、`BootTime - BootTimeBias` = 134351230275000000）: `c10d2d38`（`RevertedPendingReboot`）はヒントとの差が 22 時間と 167 秒で前の起動 → 次のセッションの掃除で `Reverted`（`reboot-observed`）。`d724c149`（`PendingReboot`、`apply_pending` は再起動）は差 167 秒で前の起動 → `attention` は `Recover`、確認画面は「このままにする」を押せ、`RebootObserved` → `AwaitingConfirm` → `Confirmed`。`restart_reasons` は空。11:37 の再起動より前に修正版を入れていれば、どちらも今の起動と判定され、読み替えたうえで再起動を待ち続ける（どちらも正しい）。
- **0.1.0 の操作が、修正版を入れた起動の中で作られていた場合**: ヒントが今の起動時刻と等しいので、再起動待ちのまま。スリープ、休止、高速スタートアップのシャットダウンでは起動時刻が変わらないので、まだ待つ。再起動すると変わるので、再起動したと判定する。GUID が本当に起動ごとに変わる PC でも、ヒントが決めるので結果は同じ。
- **互換性**
  - 0.1.x は、修正版が書いたジャーナルもそのまま読める（`NewerSchema` にも `Malformed` にもならない）。
  - **ダウングレード**（修正版の上に 0.1.x を入れる）: NSIS のインストーラーは拒否しない（M5b の更新は `NotNewer` で拒否する）。0.1.x は GUID とカウンターの ID を比べて等しくならないので、新しい形のエントリをすべて前の起動のものと扱う。再起動の後なら正しいが、同じ起動の中だと、`PendingReboot` の操作の「このままにする」を押せてしまい、「再起動が必要」も消える（危険な側。その起動の中だけで、確認画面には Raw Input と Shift+2 のテストが残る）。**再起動待ちの変更がある間はダウングレードしない**こと。版を上げる案は採らなかった: 0.1.x が修正版の書くすべてのエントリを拒否し、何も待っていなくても書き込みが止まる（「更新してください」）ため。
  - ダウングレードの後に再びアップグレードした場合、0.1.x が書いた GUID の ID は同じ規則で判定する。
  - GUI の `settings.toml` の `recovery.prompted[].boot` は形が変わるので、更新の直後に自動の回復の問い合わせがもう一度出ることがある（見た目だけ）。
- **公開する順序**: 0.1.0 には更新の機能がないので、0.1.1 は NSIS のインストーラーで 0.1.0 の上に入れる（再起動は要らない）。次に GUI を起動したときの読み替えで、止まっていた 2 つの操作が解ける。M5b（更新）の最初の公開版はこの修正を含むこと（含まないと、止まった `PendingReboot` の PC は `check_journal` に断られて更新できない）。

### C.11 `apply_pending`（保存値がまだ効いていないこと）

C1 の指摘: 戻した値や、確認なしで先へ進めた値がまだドライバーに読まれていないのに、その事実がその場限りの結果にしか載らず、ジャーナルにも表示にも残らなかった。例えば、カウントダウン中に helper が強制終了すると、回復はレジストリを US に戻して `Reverted` にするが、Keychron はリセットで読んだ JIS のまま動き、次に抜き差ししたときに突然 US に変わる。

- **設定する時点**: 操作が閉じるとき、再接続の経路で `AwaitingConfirm` に進むとき、回復が `AwaitingConfirm` にロールフォワードするときに、`apply_pending_on_close` で求めて同じ J に入れる。
- **対象**: HID のキーボードのうち、その操作が `Written` に届いた（または回復が `AtIntended` を観測した）のに、最後の書き込みの後に成功したリセットがないもの。起動時の値は `PendingReboot` / `RevertedPendingReboot` の状態そのもので表す。ただし `Failed` や `Reverted` で閉じたエントリが起動時の値を含んでいれば `RestartPc` を入れる。
- **対処**: キーボードの `device_apply_action`。回復が書いた値は、回復がリセットしないので、`Reconnect` より軽くしない。リセットしたキーボードが戻らなかった場合は `RestartPc`。
- **消す時点**（状態を変えない J → FJ。エンジンがロックの中の前処理で行う）: 記録した起動（`since`）と今の起動が違う（すべてのドライバーが値を読み直した）。または、`ResetKeyboard` と `Reconnect` について、対象のすべてのキーボードで Raw Input が保存値から予想される種類を報告している（`apply_pending_cleared`）。
- **表示**: 非昇格の GUI と CLI は `Attention::NeedsApply` を「まだ反映されていません: 〜してください」と表示する。書き込みは止めない。Raw Input がすでに期待どおりなら表示しない（同じ `apply_pending_cleared` を使う）。
- **Keep と `apply_pending`**: 再接続の経路などで、Raw Input が期待どおりの種類を報告する前に Keep された場合は、`Confirmed` にしたうえで `apply_pending = Reconnect` を残す（確定を拒むと、open なエントリがほかの操作を止め続けるため）。

### C.12 M4 のサービスとの取り決め（前方互換）

C15 の指摘: 計画 2.2 は、サービスが再適用し直すループを防ぐため、helper が HKLM のプロファイルも同時に更新することを求めているが、M2 のトランザクションにはその場所も時点もない。M2 のうちに次の規則を決めておく。

- **意図した配列の出どころはジャーナル**: 各キーボードの「意図した配列」は、そのキーボードの値の `latest_record` が `Confirmed` の操作のものから求める。プロファイルを別に持つ場合も、`Confirmed` への遷移と、`Confirmed` の操作の取り消しのときだけ、**同じロックの中で**更新する。`Planned` の時点では書かない（カウントダウン切れで戻した後に、プロファイルだけが新しい配列のまま残るのを防ぐ）。
- **サービスが再適用しないもの**: open なエントリがあるデバイス。MKLM 自身のリセットによる到着（エントリが `Restarting` から `RevertPending` の間にある、またはその直後のもの）。
- サービスもロックを取ってから書き、ジャーナルに操作として記録する（計画 2.2 の「すべての書き込みはロックの後」）。

---

## D. 操作の手順

### D.1 共通の前処理（書き込むすべての操作）

1. `Host::acquire_lock(10 秒)`。取れなければ `Busy`。取れたら `Event::Locked`。
2. `read_journal` → `Journal::parse`。`unreadable` があれば `JournalUnreadable`。**0.1.1 から**、その前に `CurrentBoot`（`boot_id()`、`boot_time_hint()`、`legacy_boot_guid()`）を作り、読んだ直後に `adopt_legacy_boots` で 0.1.x の起動 ID を判定する（メモリの上だけ。C.10「起動 ID」）。セッションの起動 ID は `CurrentBoot::id`。
3. 他のエントリの確認:
   - 書き込み中のエントリか `countdown` 付きの `AwaitingConfirm` がある（ロックを持っているので、どれも放棄されたもの。C.7）: `recover` と `undo` はまずそれを回復する。それ以外の要求は `RecoveryNeeded`。
   - `set`、`migrate`: open なエントリがあれば `OpInProgress`。
   - `restore --baseline`: 書き込み中ではない open なエントリは拒否せず、置き換える（D.5）。
   - 既存の操作に対する要求（`revert`、`confirm`、`resolve`）: その操作の状態が要求を受け付けなければ `InvalidState`。
4. `DeviceController::keyboards()`。`Incomplete`（書き込みを止める読み取りの問題）なら `InventoryIncomplete`（何も書かない）。`warnings` は結果の警告に入れる。
5. すべてのキーボードについて 7 つの override 値を `read_value` で読み直し、`overrides` を置き換える。全体の 5 つの値も同様。以降の判断はこの読み直した値だけを使う。型違いの値は `RegValue::Other` として読め、止まらない（S2）。読めない値（クラッシュ以外のエラー）も止めずに警告にする。i8042prt の固定と全体のペアは「ない」とみなし（INV-PS2 を安全側で判断する）、それ以外の値は直前のモデルの値を残す。書く値は書く直前に読み直す（CAS）ので、古い値を書くことはない。この読み直しは書き込み中のエントリがある間にも走るので、ここで失敗して書き込み中のエントリを残してはならない（C3、K の確認事項）。Windows 実装は値を読み取り専用で開く（`regwrite::open_device_key_read` / `open_global_key_read`。書き込みを拒否された鍵も読め、読むだけで "Device Parameters" を作らない。I7）。
6. `Host::drain_warnings()` を結果の警告に入れる。
7. 前処理の掃除（それぞれ J → FJ）: 有効でなくなった `apply_pending` を消す（状態は変えない）。起動 ID が変わった `RevertedPendingReboot` を `Reverted` に遷移させる（C.11）。0.1.x のエントリで前の起動と判定したものもここで閉じる（読み替えていない GUID のまま保存する）。
8. 要求に `expected`（`ExpectedPlan`）があれば、自分の計画の `steps` と `apply` の両方と比べる。違えば `PlanChanged`（何も書かない）。反映方法が変わった場合（例: 下見ではリセットだったのに、UAC を待つ間にドングルを挿し直して未開始になり、再起動が必要になった）も止める（S6）。

**INV-PS2 を強制する場所**

| 場所 | 内容 |
|---|---|
| `check_plan`（`set`、`migrate`） | 各ステップの後と最後で INV-PS2 を確かめる。保たれていた状態を壊すステップも、違反が残る計画も拒否する |
| `plan_restore`（取り消し、ロールバック、undo、導入前に戻す、解決） | 変化の向きで決めた段階の順（C.5）で、各ステップの後に同じ確認をする。例外は、戻した後の状態が「i8042prt と全体の値がすべて baseline どおり」で、その baseline がもともと INV-PS2 に違反していた場合だけ。このときは `restores_inv_ps2_violation` に入れて警告する。「すべて」は計画の外の値も含む（`plan_restore` は `Journal::baselines` を受け取り、すべての i8042prt キーボードの固定と全体のペアを baseline と比べる。baseline のない値は MKLM が変えていない）。たとえばキーボードを指定した `restore --baseline` で PS/2 の固定だけを外し、移行で消したペアが消えたまま、という状態は拒否する。INV-PS2 が読む値（固定とペア）を 1 つも書かない計画（HID だけなど）は違反を変えられないので、見つかった違反を示すだけで拒否しない |
| 回復 | ロールバックも `plan_restore` を通す。拒否されたら書かずに `Conflict`。ロールフォワードと再起動の観測は、今の値で INV-PS2 が成り立つときだけ（C12） |
| 解決の `KeepCurrent` | 解決後の値で INV-PS2 が成り立たなければ拒否する（C12。D.8） |
| 書き込みの途中の失敗 | 書けなかった値があるステップの後は、残りのステップを書かない（C.5） |
| エンジン全体 | 検査を通った計画以外は書かない |

例えば、キーボードごとモードで i8042prt の phantom が固定値を持っていない場合（ドックに PS/2 キーボードがあった、など）は、HID だけの `set` も `check_plan` で拒否される。エラーには、固定すべきキーボードの一覧が入る。

### D.2 キーボードの配列を設定する（`SetLayout`）

入力（`SetLayoutParams`）: `instance_id`、`layout`（`jis` / `us` / `standard`）、`apply`（`ApplyOptions { allow_live_reset, other_input_available }`）、`expected`。

1. D.1。
2. `operation::plan_set_layout(keyboards, global, instance_id, choice, &apply)`（下見と同じ関数。S6）:
   - キーボードがなければ `UnknownKeyboard`。
   - 固定モードなら `MigrationRequired { fixed }`（M0 #2a: 固定モードではデバイスの値が効かない）。CLI は `migrate --also` を案内する。
   - 対象は `physical_device_members`: 外付けで ContainerId が分かっていれば、同じコンテナのすべての kbdhid コレクション（phantom を含む。計画 3.4）。i8042prt、内蔵、ContainerId が不明なら、そのキーボードだけ。
   - 書き込み内容は `device_layout_writes(driver, choice.layout())`。i8042prt に `standard` は `StandardNotAllowed`。
   - `check_plan(keyboards, global, writes, [])` → `CheckedPlan`（許可リスト、INV-PS2、順序）。
   - 反映方法: `apply = apply_method(members, false, only_usable, apply.allow_live_reset)`。`only_usable` は、`apply.other_input_available == false` のとき、または対象のコンテナ以外に「接続中で、`DN_STARTED` で、仮想でないキーボード」が 1 台もないときに true。
3. D.1 の 8（`expected` との比較）。
4. 記録を作る: 各書き込みについて、`before` = 現在の値、`intended` = `RegValue::from(op)`、`baseline` = 既存の記録、なければ `before`。`value_eq(before, intended)` の書き込みは記録にも書き込みにも含めない。何も残らなければ `NoChange`（ジャーナルに書かない）。
5. 復旧用ファイル（C.5 の 2。HID だけなので、失敗しても警告して続ける）。
6. `check_cancelled()` → true なら `Cancelled`（何も書かない）。
7. 新しい操作を作る（C.5 の 4〜5）→ `Event::Planned { steps, apply, keyboards }`。`keyboards` は、計画後のスナップショットを `assess` した結果から作る（期待する種類と、変更後に使われる配列）。
8. 書き込む（C.5 の 6）。
   - 最初の値の CAS が合わなければ、`Failed(ConcurrentChange)`。
   - 途中でエラーや CAS の不一致があれば、ロールバック（`RevertPending(Rollback)` → 書いた分だけ戻す → `Reverted`、`failure = WriteError`）。ロールバックも書けなければ `Conflict`（`write_error` 付き。C3）。
9. `Written` → FJ。
10. `apply` に応じて分かれる。

**a. リセットの経路（`ResetKeyboard`。USB）**

1. `check_cancelled()` → true なら、リセットせずにロールバックして `Reverted`（`failure = CallerDisconnected`）。変更はまだ効いていないので `apply_pending` もない。見ている人のいないキーボードを 2 回リセットしないため（C13、S4）。
2. `Restarting` → FJ。
3. 対象のキーボード（接続中のもの）を 1 台ずつ: `Event::ResettingKeyboard` → `restart()`（別スレッドで 20 秒の期限付き。C18）→ `wait_for_arrival(15 秒)` → `Event::KeyboardArrived { reported, expected }`。
4. `NeedsReboot`、`TimedOut`（リセットか到着の期限切れ）、エラーのいずれか: 値を戻す（`RevertPending(Rollback)`、`Expect = last_written`）→ `RevertedPendingReboot`、`failure = KeyboardDidNotReturn`、`apply_pending = RestartPc`。結果は `pending_action = RestartPc`（計画 1.4「規定時間内に戻らなければ、値を元に戻してから PC の再起動を案内する」）。
5. 戻ってきたが種類が変わっていない（`reported` が以前の種類のまま）: リセットでは反映されなかったものとして、`AwaitingConfirm`（`countdown` なし、`apply = Reconnect`、`apply_pending = Reconnect`）にして b の手順 2 以降へ進む。
6. すべて期待どおり（または Raw Input が読めない）: `AwaitingConfirm`（`countdown = 20 秒`）→ FJ → `Event::CountdownStarted { seconds, verified }`。**M3 から**秒数は要求の `ApplyOptions::countdown_seconds`（20 か 60。GUI の設定「確認の時間を長くする」で 60。m3 WP-E3）。ほかの値の要求は、ロックを取る前に `PlanRejected`（`EngineError::CountdownNotAllowed`）で何もしない。
7. カウントダウン: 残り 20 秒から 1 秒ずつ `Event::CountdownTick` を送り、`sink.wait_decision(1 秒)` を呼ぶ。`Host::monotonic` で「20 秒＋5 秒」（M3 から「要求の秒数＋5 秒」）を過ぎたら、呼び出し回数によらず時間切れとする（C18）。
   - `Keep`: `Confirmed` → FJ → 整理 → 結果 `Confirmed`。
   - `RevertNow`、`Disconnected`、時間切れ: 取り消す（次の手順）。
8. リセット経路での取り消し: `RevertPending(Rollback)` → 値を戻す → FT → 対象を**もう一度リセット**して元の配列を反映する（元の、使えていた配列に戻すため。1 回目で別の入力手段を確かめている）。成功すれば `Reverted`（`failure` は `CountdownExpired` か `CallerDisconnected`。利用者の RevertNow なら `None`）。リセットに失敗したら `RevertedPendingReboot`（`apply_pending = RestartPc`、`pending_action = RestartPc`）。

**b. 再接続の経路（`Reconnect`。BLE / BT、未接続のキーボード、`--no-reset`）**

1. `AwaitingConfirm`（`countdown` なし、`apply_pending = Reconnect`）→ FJ → **ここでロックを放す**（安定した状態なので、長い待ち時間の間ほかの処理を止めない）。
2. `Event::WaitingForReconnect` を 10 秒ごとに送りながら、最大 180 秒、`reported_type` が期待どおりになるのを待つ → `Event::KeyboardArrived`。
3. 最大 600 秒、判断を待つ。`Keep` なら `confirm()`、`RevertNow` なら `revert()`（どちらもロックを取り直し、状態を確かめ直す）。切断、または時間切れなら、`AwaitingConfirm` のまま結果を返す。**自動では戻さない**（カウントダウンは、実証できた接続方式でだけ使う。G2）。後から `mklm-cli keep`、`revert`、`undo` で決める。
4. Raw Input が期待どおりの種類を報告する前に Keep された場合は、`Confirmed` にしたうえで `apply_pending = Reconnect` を残す（C1。C.11）。

**c. PC の再起動の経路（`RestartPc`。PS/2、内蔵、I2C など）**

1. `PendingReboot` → FJ → 結果 `PendingReboot`、`pending_action = RestartPc`。
2. 呼び出し元が RunOnce を登録し、再起動を提案する（F.4）。呼び出し元がこの結果を受け取れなかった場合も、次にジャーナルを読んだときに登録する（C17）。

`standard` を選んだ場合は、結果に「標準に従う設定は M0 で打鍵を確かめていません」という警告を付ける。

### D.3 移行（`Migrate`。固定モード → キーボードごとモード）

入力（`MigrateParams`）: `standard`（移行後の PC の標準配列）、`assignments`、`expected`。

1. D.1。
2. `operation::plan_migration(keyboards, global, standard, assignments)`（下見と同じ関数）:
   - 固定モードでなければ `NotFixedMode`。
   - `ps2_pin_layout(global)` が `None` なら `InconsistentGlobal`（全体の値が不整合。ユーザーに任せる）。
   - **すべての i8042prt キーボード（phantom を含む）**を固定する。値は、割り当てがあればそれ（`standard` は `StandardNotAllowed`）、なければ `ps2_pin_layout`。固定モードの今の配列と同じなので、再起動まで挙動は変わらない（計画 1.3 の手順 2）。固定と割り当てを 1 回の書き込みにまとめる（`check_plan` が同じキーボードの重複を拒否するため）。
   - HID の割り当ては、`physical_device_members` に `device_layout_writes`。
   - 全体: `OverrideKeyboardType/Subtype` を削除し、`standard_layout_writes(standard)` のうち今と違う値だけを書く。
   - `check_plan` → `steps` は i8042prt → HID → 全体の順になる。INV-PS2 を各ステップで確かめる。`apply` は常に `RestartPc`。
3. 書く `LayerDriver JPN` の DLL が System32 にあるかを `Host::system32_file_exists` で確かめる。なければ `LayerDriverMissing`（何も書かない。計画 1.5、S8）。
4. D.1 の 8（`expected` との比較）。
5. `context` に、`i8042prt\Parameters` の全値と、全キーボードの `Device Parameters` の全値を `list_values` で記録する（計画 1.3 の手順 1）。
6. 記録を作る（すでに同じ値のものは除く。例: 7/2 で固定済みの PS/2）。全体の削除は、固定モードなので必ず残る。
7. 復旧用ファイルを耐久的に書く。**失敗したら `RecoveryAssetsUnavailable` で何も書かずに止める**（起動時の値を変えるため。C10）。
8. `check_cancelled()` → true なら `Cancelled`。
9. `Planned` → FJ → `Event::Planned`。`keyboards` には、移行によって**配列が変わるキーボード**を `changes: true` で示す。例えば、固定モードの間に 4/0 を保存していた Keychron は、移行後に US になる。
10. 書き込み → `Written` → FJ → `PendingReboot` → FJ。
11. 結果 `PendingReboot`、`RestartPc`。「設定アプリで『接続済みキーボード レイアウトを使用する』を選ぶのは、今後はかまいません」という案内は、この時点からだけ出す（計画 1.3）。
12. 再起動の後は D.7（再起動後の確認）。移行が反映されたかは Raw Input では判別できない（0.1）ので、確認は起動 ID の変化と Shift+2 のテストで行う。

**移行の途中で止まった場合**

| 止まった所 | 永続化されている状態 | 回復 |
|---|---|---|
| `Planned` の FJ の前 | エントリなし（復旧用ファイルは書き終えている） | 何も起きていない |
| i8042prt のステップの途中 | PS/2 が一部固定済み。全体は固定モードのまま | 混在 → ロールバック（固定を外す）→ `RevertedPendingReboot`（`failure = Interrupted`）。INV-PS2 は保たれたまま |
| HID のステップの途中 | 同上 | 同上 |
| 全体のステップの途中（Type だけ削除） | PS/2 は固定済み。全体は Subtype だけ | 混在 → ロールバック。段階の順（ペアを足す → HID → 固定を外す）で戻る |
| すべて書いた後、`Written` の前 | すべて `intended` | ロールフォワード → `PendingReboot`（同じ起動）または `AwaitingConfirm`（再起動後）。今の機器構成で INV-PS2 が壊れていれば（固定値のない PS/2 が現れた）ロールバック |
| 電源断 | ハイブごとに、上のどれかの先頭部分 | 同じ分類になる。途中まで書かれた全体の値が次の起動で読まれていても、ロールバックの終わりは `RevertedPendingReboot` なので「再起動が必要」が残る（C1） |

### D.4 取り消し（`Revert`）

入力: `op_id`、`apply`（`ApplyOptions`）。

1. D.1（ロック、ジャーナル、ほかのエントリの確認）。
2. 操作を探す（8 桁以上の先頭一致も可）。状態は `AwaitingConfirm`、`PendingReboot`、`Confirmed` のどれかであること。書き込み中の状態は回復で扱い、`Conflict` は解決（D.8）か undo（D.10）で扱う。**`Confirmed` の「導入前に戻す」は `InvalidState`**（「もう一度 `set` してください」。C6）。
3. すべての記録について、`latest_record(key)` がこの操作であること。そうでなければ `NotLatest { later }`。
4. 列挙し、値を読み直す。
5. `plan_restore(records, Before, Expect = last_written)`。
6. `RevertPending(Revert)`（`boot_id` と `owner` を更新）→ FJ。
7. 段階の順に書く。書けなかった値があるステップで止める（C.5）。
8. 書けなかった値があれば `Conflict` → FJ → 結果 `Conflict`（`ConflictInfo` の一覧）。
   なければ `last_written = before` とし、起動時の値を含むなら `RevertedPendingReboot`、含まなければ `Reverted` → FJ。
9. HID の記録のうち、元の操作の値が効いていたかもしれないもの（`Restarting` 以降に進んだ、`Confirmed` になった、再接続の経路だった）について:
   - `apply.allow_live_reset` と `apply.other_input_available` がどちらも true で、`live_reset_bans` が空なら、1 台ずつリセットして元の配列を反映する（カウントダウンなし。操作の前の、使えていた配列に戻すため）。戻らなければ `RevertedPendingReboot`、`apply_pending = RestartPc`。
   - そうでなければリセットせず、`apply_pending` に `device_apply_action` を残し、結果の `pending_action` で案内する（C9）。helper 側の「別のキーボード」の数え方は粗い（VXE のマウスのキーボード用コレクションも数える）ので、呼び出し元の申告なしにはリセットしない。
10. 整理。

### D.5 導入前に戻す（`RestoreBaseline`）

入力（`RestoreBaselineParams`）: `scope`、`on_conflict`、`mode`（`Interactive` / `Silent`）、`apply`。パイプから届くのは `Interactive` だけ。`Silent` は helper の固定コマンドライン `--uninstall-restore`（M5）からだけ作る（S5）。

1. D.1（書き込み中のエントリがあれば `RecoveryNeeded`。書き込み中ではない open なエントリは拒否しない）。
2. 範囲の値を集める: `All` なら `Baselines` のすべて。`Device` なら、そのキーボードの `physical_device_members` を対象とする値。
3. 各値について、現在の値、`baseline`、`Expect`（C.8）を見る。
   - 現在の値 = `baseline`: 何もしない。
   - 現在の値 = `Expect`: 書く。
   - devnode が消えている: `skipped = DeviceRemoved`。
   - それ以外は `on_conflict` に従う。
     - `Report`: 1 件でもあれば、**何も書かずに**結果 `Conflict`（`op_id` なし）と `ConflictInfo` を返す。呼び出し元はユーザーに選ばせ、`Skip` か `Overwrite` で送り直す（M3 では値ごとに選べるようにする）。
     - `Skip`: その値を飛ばして警告する（`skipped = ConflictSkipped`）。サイレントモードはこれ。
     - `Overwrite`: `Expect::Any` で書く。
4. 記録を作る（`before` = 現在の値、`intended` = `baseline`）→ 各記録に `check_restore_record` → `plan_restore(records, Baseline, …)`。順序は段階の順（C.5。C4）。
5. 置き換える操作を決める: 書き込み中ではない open なエントリ（`countdown` のない `AwaitingConfirm`、`PendingReboot`、`Conflict`）を、新しいエントリの `supersedes` に並べる（C7）。
6. 復旧用ファイル（起動時の値を含めば必須。C10）→ `check_cancelled()`。
7. `Planned`（`supersedes` 付き）→ FJ → 置き換える各エントリを `Failed(Superseded { by })` → FJ → 書く → `Written` → FJ。
   `Planned` を先に永続化するので、途中で止まっても、回復（`CompleteForward`）が置き換えの続きと書き込みを行う。置き換えられた操作の値は、この操作の `before` として記録されている。
8. 分岐:
   - `Silent`（アンインストール）: `Confirmed` → baseline の後始末（C.5）→ 結果 `Confirmed`。起動時の値を戻した場合は `pending_action = RestartPc`（MSI は 3010 に対応付ける。計画 4.1）。
   - `Interactive`: 起動時の値を含むなら `PendingReboot`、HID だけならリセット経路または再接続の経路（D.2 の a か b。同じ処理を使い、`apply` に従う）。
9. `Confirmed` になったら、baseline の後始末をする。
10. 回復は、止まった「導入前に戻す」を逆向きにせず、前へ書き切る（C.7。C14）。

「導入前に戻す」自体の取り消しは、`Confirmed` になる前（確認待ち、再起動待ち）ならできる。置き換えた操作は `Failed` のまま開き直さない（値は戻るので、起動時の値なら `RevertedPendingReboot` として再起動が必要と表示される）。

### D.6 確定（`Confirm` / `keep`）

- `AwaitingConfirm`: 値がすべて `intended` であることを確かめる（`value_eq`。違えば `Conflict`）。対象の接続中のキーボードについて Raw Input を読み、保存値から予想される種類と違うか読めなければ、`Confirmed` にしたうえで `apply_pending = Reconnect` を残し、警告する（C1）。→ `Confirmed` → FJ → 整理。「導入前に戻す」の操作なら、baseline の後始末もする。
- `PendingReboot` で、起動 ID が変わっている: まず回復の遷移（`RebootObserved` → `AwaitingConfirm`。INV-PS2 の確認を含む）を行い、続けて上と同じ手順で確定する。
- `PendingReboot` で、同じ起動: `InvalidState`（「先に PC を再起動してください。シャットダウンではなく再起動」）。
- カウントダウン中の Keep は、`Confirm` の要求ではなく、同じセッションの `Decision::Keep` で受け取る。
- **CLI の `keep <op>`** は、再起動で反映する操作（移行、PS/2 への割り当て、起動時の値を含む「導入前に戻す」）に対しては、`post-reboot` と同じ確認画面（Raw Input の表と Shift+2 のテスト、y/N の質問）を経てから `Confirm` を送る（C2）。移行の確定は、起動 ID の変化と利用者の打鍵確認の両方がそろったときだけになる。

### D.7 回復（`Recover`）と再起動後の確認

**回復を行う場面**

1. GUI と CLI の起動時。非昇格のまま `read_journal_store` → `Journal::parse` →（0.1.1 から）`adopt_legacy_boots(current_boot())`（`mklm_client::journal::read_journal`。C.10「起動 ID」）→ エントリごとに `attention(entry, boot_id(), process_liveness(owner))` を求める（C.7 の表）。
   - `Recover`: ユーザーに伝えて、helper を `Request::Recover` で起動する。**自動で UAC を出すのは、同じエントリについて 1 回の起動につき 1 回まで**（呼び出し元が HKCU に覚える）。それ以降は「回復」ボタンを出すだけにする。ジャーナル自体に書けないなど、回復が同じ理由で失敗し続ける場合に、起動のたびに UAC が出続けないようにするため（C3）。
   - `Busy`: 「別の MKLM が処理中です」と表示する。
   - `AwaitingUser`、`WaitingForReboot`、`Conflict`、`NeedsApply`: それぞれの画面を出す。
2. `mklm-cli recover`。昇格したコンソール（セーフモードを含む）でも helper を別プロセスとして起動する（UAC なし。C8）。helper を起動できないときの最後の手段として `--in-process` がある。
3. 再起動後の RunOnce（F.4）。
4. M5 の更新前の確認と、アンインストールのカスタムアクション。

**`Engine::recover` の手順**

1. ロックを取り、ジャーナルを読む（読めなければエラー）。列挙し、値を読み直し、今の INV-PS2 を求める（D.1）。
2. open なエントリを古い順に: 記録の現在の値を読み、`decide_recovery(entry, current, ctx)` を求める。
3. 判定を実行する:
   - `MarkNothingWritten`: J（`Failed(NothingWritten)`）→ FJ。
   - `RollForward { to }`: `last_written = intended`、`countdown` を外し、`apply_pending` を求める → J（`to`）→ FJ。
   - `CompleteForward { to }`: 引き継ぎ（J → FJ）→ まだ置き換えていない `supersedes` を `Failed(Superseded)` に → FJ → `before` のままの記録を CAS 付きで `intended` に書く（合わなければ `Conflict`。サイレントなら飛ばす）→ J（`Written`）→ FJ → J（`to`）→ FJ。`to = Confirmed` なら baseline の後始末。
   - `RollBack { reason }`: J（`RevertPending`、`Rollback`）→ FJ → `plan_restore(Before, Expect = last_written か intended)` → 段階の順に書く → J（最終状態、`failure`、`apply_pending`）→ FJ。最終状態は、HID だけなら `Reverted`、起動時の値を含めば `RevertedPendingReboot`（C.7）。
   - `ContinueRevert`: `revert_mode` のとおりに続ける。`Revert` と `Rollback` は上と同じ。`Resolution` は、`resolve_to` と現在の値が違う記録だけを、`Expect = conflict`（なければ `last_written`）で書く。合わなければもう一度 `Conflict`（C5）。
   - `RebootObserved { to }`: J（`to`）→ FJ。
   - `Conflict { records, inv_ps2 }`: 記録に `conflict` を入れる → J（`Conflict`）→ FJ。INV-PS2 の違反は結果の `inv_ps2_violation` で示す。
   - `Leave`: 何もしない（closed なエントリは `apply_pending` の掃除だけ）。
   クラッシュ以外の書き込みエラーは、1 回やり直してから `write_error` に残して `Conflict` にする（C.5。C3）。
4. `apply` が許せば（`allow_live_reset` と `other_input_available` がどちらも true）、`apply_pending = ResetKeyboard` のキーボードのうち `live_reset_bans` が空のものを 1 台ずつリセットし、Raw Input が保存値どおりになれば `apply_pending` を消す（C1）。
5. 結果は `Outcome::Recovered`。`recovered`（`RecoveredOp` の一覧）、`conflicts`、`pending_action`、`inv_ps2_violation`、`warnings` を返す。
6. 何も戻さなかったのに `AwaitingUser`、`WaitingForReboot`、`Conflict` のエントリが残っている場合、CLI は「確認待ちの操作があります。`mklm-cli undo` で戻せます」と表示する（C7）。以前のように、何もしていないのに「Recovered」とだけ表示することはない。

**再起動後の確認**（CLI の `post-reboot`。GUI では `mklm.exe --post-reboot`、M3）

1. 非昇格でジャーナルを読み、再起動で反映する `PendingReboot` と `AwaitingConfirm` のエントリを探す。
2. 起動 ID が変わっていなければ、「まだ反映されていません。『シャットダウン』ではなく『再起動』してください」と表示する（高速スタートアップのシャットダウンでは起動 ID が変わらない。0.1.1 の起動 ID `KUSER_SHARED_DATA.BootId` について R9 と MT-4 で確かめる。0.1.0 の GUID は、完全な再起動でも変わらない PC があった。C.10「起動 ID」）。RunOnce を登録し直して終わる。
3. 起動 ID が変わっていれば、記録の対象キーボードについて、期待する種類と Raw Input の報告値を並べて示す（例: 内蔵 0x7/0x2 ✓、Keychron 0x4/0x0 ✓）。移行については Raw Input で判別できないことを添え、Shift+2 のテストを案内する。
4. 「このままにしますか? [y/N]」と尋ねる。**自動では戻さない**（計画 3.6）。
   - `y`: helper に `Confirm`（回復の遷移を含む）。
   - `n`: helper に `Revert`。
   - Enter だけ、または入力の終わり: 何もしない。RunOnce を登録し直し、次回また尋ねる。

### D.8 衝突の解決（`ResolveConflict`）

入力（`ResolveParams`）: `op_id`、`choices`、`apply`。

- 状態は `Conflict` であること。`choices` で指定のない記録は `KeepCurrent`。
- 記録ごとの `resolve_to`: `KeepCurrent` → 現在の値（利用者が見た値）。`UseBefore` → `before`。`UseIntended` → `intended`。`UseBaseline` → `baseline`。
- 解決後の値（すべての `resolve_to`）で INV-PS2 を確かめる。成り立たなければ `PlanRejected(InvPs2)` で拒否し、固定値のない PS/2 の一覧を返す（C12）。利用者は、全体の値に `UseBefore` を選んで固定モードに戻すか、undo を選ぶ。**違反が残る `KeepCurrent` で閉じることはない。**
- 書く値がなければ（すべて `resolve_to` = 現在の値）: `last_written = 現在の値` として受け入れる → `state_after_resolution` で最終状態を決める → J → FJ。
- 書く値があれば:
  1. **すべての記録に** `resolve_to` を入れる → J（`RevertPending`、`revert_mode = Resolution`）→ FJ（C5）。
  2. `plan_restore(…, Resolution, Expect = conflict か last_written)` で段階の順と INV-PS2 を確かめる。
  3. 段階の順に書く → FT。書けなかった値があるステップで止め、もう一度 `Conflict`。
  4. `last_written = resolve_to`、`resolve_to = None` → `state_after_resolution` → J → FJ。
  5. HID のリセットは D.4 の 9 と同じ（`apply` に従う。C9）。
- `state_after_resolution`:
  - すべて `intended` → `Confirmed`。
  - すべて `before` → `Reverted` か `RevertedPendingReboot`。
  - それ以外 → `Failed(ConflictKeptCurrent)`。
- 解決の途中で止まった場合、回復は `ContinueRevert` で、`resolve_to` と現在の値が違う記録だけを、利用者が見た値を期待値にして書く。`KeepCurrent` を選んだ記録は `resolve_to` = 現在の値なので、書かれない（C5 の、残すと決めた移行が取り消されてしまう問題を防ぐ）。

### D.9 排他制御と将来の更新

- ロックファイル: `%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock`。DACL は `LOCK_FILE_SDDL`（SY と BA だけ）。
  標準ユーザーがこのファイルを読めると、`LockFileEx` でロックを奪って MKLM の書き込みを止められる（DoS）。そのため、`MKLM` フォルダーが Users に読み取りを許していても、ロックファイルは継承しない明示的な DACL で作り、使うたびに確かめる。
- 手順: `ensure_protected_dir(Base)` → `FileLock::acquire`（`LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY` を 100 ms ごとに再試行し、10 秒で `Timeout`）。プロセスが死ぬとロックは自動で外れる。
- **ファイル側の先回り作成**（S1）: `%ProgramData%` は Users にフォルダーの作成を許している（この開発機でも `icacls` で確認）。M2 にはインストーラーがないので、MKLM が初めて昇格して動く前なら、どのローカルユーザーでも `SHIN DATA CENTER`、`MKLM`、`mklm.lock` を先に作って所有者になれる。レジストリ側（C.1）と違って防げない。以前の設計のように「検証に失敗したら中止」だと、recover を含むすべての書き込みが永久に止まり、安全網そのものが無効になる。
  - そこで、検証に失敗した階層は**中身を信用せず、読まずに隔離する**: 親を handle で固定したまま、その階層を `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` と `DELETE` で開き（リパースポイントならリンクそのものを扱う）、`SetFileInformationByHandle(FileRenameInfo)` で `<名前>.untrusted-<uuid>` に改名し、保護 SDDL で作り直す。結果に警告を載せる。ロックファイルも同じ。
  - 改名できない場合（ほかのプロセスが `FILE_SHARE_DELETE` なしでフォルダーを開いたままにしている）は、`Insecure` で止め、「ほかのユーザーをサインアウトさせるか、PC を再起動してからもう一度実行してください」と案内する。残るリスクは I.18。
  - FakeHost では「先回りして作られた」状態は Windows 実装の中の話なので、R15（VM）で確かめる。エンジンのテストは `push_warning` で警告の受け渡しだけを確かめる。
- 書き手は helper、M4 のサービス、M5 の更新、MSI のカスタムアクション、`--in-process` の CLI で、すべて同じロックを使う。
- ロックを持つ期間は、1 つの要求の間ずっと（カウントダウンの 20 秒を含む）。例外は再接続の経路の待ち時間で、ここでは放す（D.2 b）。
- **M5 の更新に向けて**: 更新処理はロックを取り、open な操作がないこと（すべての `attention` の `blocks_writes` が false）を確かめ、ロックを持ったまま msiexec を実行する。アンインストールのカスタムアクションは、アップグレードのときは動かない（`NOT UPGRADINGPRODUCTCODE`）ので、自分自身を待って止まることはない。アンインストールのときは、タイムアウト付きでロックを取る（取れなければ計画 4.1 のとおり `Return=ignore`）。ジャーナルの互換性は C.10。

### D.10 undo（open な変更をまとめて取り消す）

C7 の指摘: 再起動した後に内蔵キーボードの配列がおかしい、といういちばん起きやすい失敗の状態では、エントリは `PendingReboot` か、回復後の `AwaitingConfirm`（カウントダウンなし）にある。この状態で、以前の設計では `recover` は何も戻さず、`restore --baseline --all` は `OpInProgress` で拒否され、効くのは操作 ID を打つ `revert <op>` と、ジャーナルを無視する `restore-offline.cmd` だけだった。スクリーンキーボードで 8 桁の ID を探して打つのは現実的ではない。

- `Request::Undo { apply }` / `Engine::undo_open` / `mklm-cli undo`。docs/recovery.md と README.txt の先頭に載せる。
- 手順:
  1. D.1。書き込み中のエントリがあれば、まず D.7 の回復を行う（どの状態からでも効くように）。
  2. 書き込み中ではない open なエントリ（`AwaitingConfirm`、`PendingReboot`、`Conflict`）を**新しい順に**取り消す。
     - `AwaitingConfirm`、`PendingReboot`: D.4 と同じ（`revert_mode = Revert`、`Expect = last_written`）。
     - `Conflict`: 現在の値が `last_written`（なければ `intended`）の記録だけを `before` に戻す。衝突している記録は書かずに `ConflictInfo` で示す（`Report` と同じ）。衝突が残ればエントリは `Conflict` のまま。この取り消しが途中で止まった場合、回復の `ContinueRevert` も同じ分け方をする（`RevertPending` に `Conflict` から入ったことを履歴で見分け、`before` でも `last_written` でもない記録は書かずに `Conflict` に戻す。I4）。
  3. HID のリセットは D.4 の 9 と同じ（`apply` に従う）。
  4. 結果は `Outcome::Recovered`（`decision = "undo"`）。戻した操作、残った衝突、`pending_action` を返す。
- `Confirmed` の操作は undo の対象にしない（取り消しは `revert <op>`）。
- 取り消した値が起動時の値を含めば `RevertedPendingReboot` になり、再起動を案内する。

### D.11 ドライバーが読まない値の削除（`CleanupValues`。M3 で追加）

m3 A.5（WP-E1）の「削除する」。入力（`CleanupParams`）: `instance_id`、`names`。

1. D.1（`set` と同じく、open なエントリがあれば `OpInProgress`）。
2. `instance_id` は Keyboard クラスの列挙にあること（なければ `UnknownKeyboard`。マウスのコレクションなどはここで止まる）。`mklm_core::check_cleanup`: 名前は、そのキーボードのドライバーが読まない側の型とサブタイプの組（kbdhid なら `OverrideKeyboardType/Subtype`、i8042prt なら `KeyboardTypeOverride/SubtypeOverride`。`unread_value_names`）だけ。ドライバーが読む名前、ほかの名前、ほかのドライバー、仮想キーボードは `PlanRejected`。今の値で INV-PS2 が壊れていれば、`check_plan` と同じく `PlanRejected`。
3. 記録を作る（値がない名前は除く。何も残らなければ `NoChange`）→ C.5 の新しい操作と同じ順序（B と J(`Planned`) → FJ → 削除 → FT → J(`Written`) → FJ）。`apply` は `None`（反映するものがない）。
4. `Written` → `AwaitingConfirm`（カウントダウンなし、`apply_pending` なし）→ 結果 `AwaitingConfirm`。利用者が `Confirm`（確定）か `Revert` / `undo` で決める。自動では確定しない。

- 回復は `SetLayout` と同じ判定（`Planned` の途中ならロールバック、書き終えていれば `AwaitingConfirm` へロールフォワード）。取り消しは `Reverted`（再起動は要らない）。`apply_pending_on_close` は常に `None`、リセットの対象にもならない。
- 読まれない値は起動時の値でもない: HID コレクション（`HID\…`）の `OverrideKeyboard*` は `ValueRecord::is_boot_time` が false。i8042prt の devnode の HID の名前は元から起動時の値ではない。したがって復旧用ファイルは必須にならない（書けなければ警告）。`plan_restore` は、キーボードのドライバーが読まない側の組も戻してよい名前に含め（`check_restore_record`）、INV-PS2 に関わる値としては数えない。「導入前に戻す」で読まれない値だけを戻す場合も `apply = None` で、確認待ちになる。どの種類の操作でも、読まれない値の記録のためにキーボードが反映待ち（`apply_pending`）になることはない（エンジンの `close_pending` が、`apply_pending_on_close` に渡す前にその記録を除く。回復で閉じる場合も同じ）。
- ジャーナルの版は 2（C.10）。I1〜I7 のクラッシュの網羅テストに 4 つのシナリオ（削除、削除して確定、削除の取り消し、確定した削除の後の導入前に戻す）と I7 を足した。

### D.12 マシン全体の設定（`SetMachineSettings`。M3 で追加）

m3 B.14（WP-E2）の「アンインストール時にキーボードの設定を元に戻す」。`Engine::set_machine_settings` はロックを取り、`RegistryBackend::write_machine_setting`（`HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Settings` の `RestoreOnUninstall`、`MACHINE_SETTING_NAMES` の名前だけ。キーはジャーナルと同じ DACL で作り、書いてからフラッシュする）を 1 回呼ぶ。ジャーナルには書かない。open なエントリや読めないジャーナルでも止めない（キーボードの値に触れないため）。結果は `Confirmed`（`op_id` なし）。

---

## E. helper との通信（mklm-ipc）

### E.1 役割と起動の流れ

呼び出し元（GUI か CLI）が**パイプのサーバー**になり、helper は**クライアント**として接続する。理由は 3 つある。

1. 呼び出し元は、helper が存在する前にパイプの名前を自分のものにできる。`FILE_FLAG_FIRST_PIPE_INSTANCE` により、同じ名前が先に作られていれば作成に失敗するので、そこで止められる。
2. DACL を呼び出し元が決められる。
3. 昇格した helper が外へ向かって、`SECURITY_IDENTIFICATION` の SQOS 付きで接続するので、非昇格のサーバーは管理者のトークンを偽装できない。
   逆の構成（helper がサーバー）だと、同じユーザーのほかのプロセスが、昇格したサーバーに先に接続しようとする余地が生まれる。

```
呼び出し元                                        mklm-helper（昇格）
 helper のビルド ID を VERSIONINFO から読み、自分と比べる（違えば UAC を出さずに止める）
 uuid, nonce を生成
 CreateNamedPipeW(FIRST_INSTANCE, DACL)
 非昇格: ShellExecuteExW("runas") ──UAC──▶ 起動（lpDirectory = System32）、引数を検証
 昇格済み: CreateProcessW（UAC なし） ────▶
 ConnectNamedPipe（120 秒 / helper の終了）◀──── CreateFileW(SQOS Identification)
 クライアントの PID ＝ 起動した PID か               サーバーの PID ＝ --caller-pid か
 （違えば切断して待ち直す）                          呼び出し元の exe が同じフォルダーにあるか
                                  ◀──── Hello { protocol, nonce, helper_pid, version, build_id }
 verify_hello
 Welcome { protocol, caller_pid, build_id } ──▶ verify_welcome
 Request ────────────────────────────▶ dispatch → engine
                                  ◀──── Event …（書き込み専用スレッド。10 秒ごとに Heartbeat）
 Decision（カウントダウン中）─────────▶
                                  ◀──── Result または Error
 Request … / Bye ───────────────────▶ 終了（期限を過ぎたリセットのスレッドを待ってから）
```

**helper の場所**

- M2: 呼び出し元の exe と同じフォルダーの `mklm-helper.exe`（`elevation::helper_path()`）。`current_exe()` から求めた絶対パスで、リパースポイントでない通常のファイルであることを確かめる。
- M5: MSI が HKLM に記録するインストール先（`SOFTWARE\SHIN DATA CENTER\MKLM\InstallDir`）だけを使う。

**起動の方法**

- 非昇格: `ShellExecuteExW` の設定は `lpVerb = "runas"`、`lpFile` = 絶対パス、`lpParameters` = `HelperArgs::to_parameters()`、`lpDirectory` = System32、`fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI`、`nShow = SW_HIDE`、`hwnd` = GUI のウィンドウ（CLI は `GetConsoleWindow()`）。UAC を断られたら `Error::Cancelled`。
- 昇格済み（管理者のコンソール、セーフモード）: `CreateProcessW`（`elevation::spawn_from_elevated`）。UAC は出ず、Appinfo サービスにも頼らない。helper はそれでも別プロセスなので、コンソールを閉じる、Ctrl+C を押す、のいずれでもパイプが切れ、カウントダウン中なら helper がすぐに戻す（C8）。

### E.2 パイプの名前とセキュリティ

| 項目 | 値 |
|---|---|
| 名前 | `\\.\pipe\SHINDATACENTER.MKLM.<uuid v4 小文字>` |
| `dwOpenMode` | `PIPE_ACCESS_DUPLEX \| FILE_FLAG_FIRST_PIPE_INSTANCE \| FILE_FLAG_OVERLAPPED` |
| `dwPipeMode` | `PIPE_TYPE_BYTE \| PIPE_READMODE_BYTE \| PIPE_WAIT \| PIPE_REJECT_REMOTE_CLIENTS` |
| インスタンス数 | 1 |
| バッファ | 64 KiB |
| DACL | `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;RC;;;OW)`（`HELPER_PIPE_SDDL`） |
| クライアント | `CreateFileW(GENERIC_READ \| GENERIC_WRITE, 0, NULL, OPEN_EXISTING, SECURITY_SQOS_PRESENT \| SECURITY_IDENTIFICATION \| FILE_FLAG_OVERLAPPED)`。`ERROR_PIPE_BUSY` なら `WaitNamedPipeW` で待ち直す |

- **計画との違い**: 計画 2.2 の DACL には `(A;;GA;;;<userSID>)` がある。helper のパイプではこれを外す。昇格した helper は BA の ACE に一致するので、ユーザーの SID は要らない。多重起動防止用のパイプ（M3。`activate` と `quit` だけを受け付ける）では、計画どおりユーザーの SID を入れる。
- **所有者の暗黙の権利**（S10）: パイプの所有者は作成した呼び出し元（利用者）なので、DACL に書かなくても `READ_CONTROL` と `WRITE_DAC` が残り、同じユーザーのプロセスは `WRITE_DAC` だけを指定して唯一のインスタンスに接続できる。`OWNER RIGHTS`（`OW`）の ACE で所有者の権利を `READ_CONTROL` に限る。
- **防御の本体は PID の確認**: DACL は接続できる相手を減らすだけで、決め手は両側の PID の確認。呼び出し元は `GetNamedPipeClientProcessId` が起動した helper の PID と一致することを確かめる（プロセスのハンドルを持っているので、PID の再利用は起こらない）。**一致しなければ `DisconnectNamedPipe` で切って、`CONNECT_TIMEOUT` まで待ち直す**。先に接続したプロセスがセッション全体を失敗させることはできない。helper は `GetNamedPipeServerProcessId` が `--caller-pid` と一致し、そのプロセスの実行ファイルが helper と同じフォルダーにあることを確かめる（`proc_identity::same_image_directory`。M5 ではインストール先）。
- **別の管理者の資格情報で昇格した場合**（S3）: 標準ユーザー A が UAC で管理者 B の資格情報を入れると、helper は B として別のログオン SID で動く。通常のプロセスの DACL は Administrators に何も与えないので（この開発機で `GetSecurityInfo` により確認）、`OpenProcess` で A のプロセスを調べることはできない。そこで `proc_identity` は `OpenProcess` を使わず、`NtQuerySystemInformation(SystemProcessIdInformation)` で PID から NT 形式のパスを読む（どのユーザー、どの整合性レベルからも読める）。パスは両側とも NT 形式のまま、大文字小文字を区別せずに比べる。生死の判定も `SystemProcessInformation` の（PID、CreateTime）の組で行う。`SeDebugPrivilege` は使わない。
- 整合性レベルのラベルは既定（作成者の medium）のまま。high の helper が medium のパイプに書くのは問題ない。
- **残るリスク**: 同じユーザーの悪意あるプロセスが、自分で作ったパイプを指定して helper を起動することはできる。ただし UAC の確認画面が出るので、ユーザーの承認が必要になる。承認されても、得られるのは helper の機能（パイプで許した要求と、許可リストで制限された書き込み）だけ。サイレントの「導入前に戻す」はパイプから届かない（S5）。

### E.3 ハンドシェイク

1. 呼び出し元: helper の実行ファイルの VERSIONINFO からビルド ID（`BUILD_ID_VERSION_KEY`）を読み、自分のビルド ID と違えば、UAC を出す前に「helper の版が違います。再インストール（開発中は両方を再ビルド）してください」で止める（S11）。
2. 呼び出し元: `BCryptGenRandom` で 32 バイトの nonce を作り、16 進でコマンドラインに渡す。昇格したプロセスのメモリは、非昇格のプロセスからは読めない。
3. helper: 接続し、PID を確かめてから `Hello { protocol, nonce_hex, helper_pid, helper_version, build_id }` を送る。
4. 呼び出し元: `verify_hello`（版、ビルド ID の完全一致、nonce の定数時間比較、`helper_pid` = 起動した PID）→ `Welcome { protocol, caller_pid, build_id }`。
5. helper: `verify_welcome`（版、ビルド ID、PID）。
6. どれかが失敗したら、要求を 1 つも読まずに切断する。helper は終了コード 3 で終わる。

**ビルド ID**: パッケージの版と、`mklm-core`、`mklm-ipc`、`mklm-engine`、`mklm-win` のソースのハッシュ。両方の `build.rs` が同じ方法で計算し、`env!` で埋め込み、VERSIONINFO の文字列にも入れる（WP5 で実装）。`PROTOCOL_VERSION` を上げ忘れた変更（エンジンの規則の変更など）や、`cargo build -p mklm-cli` だけで古い helper が残った状態を、書き込む前に検出する。

### E.4 helper のコマンドライン

```
mklm-helper.exe --pipe SHINDATACENTER.MKLM.<uuid> --nonce <64 桁の 16 進> --caller-pid <10 進>
```

`HELPER_ARGS_PATTERN`（先頭から末尾まで一致すること）:

```
^--pipe SHINDATACENTER\.MKLM\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12} --nonce [0-9a-f]{64} --caller-pid [1-9][0-9]{0,9}$
```

- `GetCommandLineW` の生の文字列から、`command_line_tail` で先頭のプログラム名と空白 1 つを取り除き、残りを `HelperArgs::parse` で検証する。CRT の argv 分解は使わない。引用符の扱いの違いを突かれないようにするため。
- 正規表現のクレートは使わず、手で書いた厳密なパーサーにする。テストで、この正規表現と同じものだけを受け付けることを確かめる。PID は u32 に収まること。
- M5 のアンインストール用には、別の固定書式（`--uninstall-restore`）を足す。サイレントの「導入前に戻す」（`RestoreMode::Silent`）を作れるのはこのコマンドラインだけ（S5）。

### E.5 フレーミング

- 4 バイトのリトルエンディアンの長さ＋UTF-8 の JSON（`Frame { v, seq, body }`）。
- 長さが 0、または `MAX_FRAME_LEN`（256 KiB）を超えたら、読み込む前に切断する。
- まず `FrameHeader { v, seq }` だけを解析し（本文は無視される）、`v` が `PROTOCOL_VERSION` と違えば `FrameError::Version` で切断する。版の違う相手の本文が「壊れた JSON」として報告され、原因が分かりにくくなるのを防ぐ（S11）。その後で本文を解析する。
- `seq` は向きごとに 1 から始め、欠けや重複があれば切断する。
- 1 フレームは 1 回の `write_all` で書く。helper では書き込み専用のスレッドだけが書く。

### E.6 メッセージ

**呼び出し元 → helper（`CallerMessage`）**: `Welcome`、`Request`、`Decision`、`Bye`

| `Request` | 中身 | helper が呼ぶエンジンの処理 |
|---|---|---|
| `SetLayout` | `instance_id`、`layout`、`apply`、`expected` | `set_layout`（D.2） |
| `Migrate` | `standard`、`assignments`、`expected` | `migrate`（D.3） |
| `Revert` | `op_id`、`apply` | `revert`（D.4） |
| `Confirm` | `op_id` | `confirm`（D.6） |
| `RestoreBaseline` | `scope`、`on_conflict`、`apply`（`silent` はない） | `restore_baseline`（`mode = Interactive`。D.5） |
| `Recover` | `apply` | `recover`（D.7） |
| `Undo` | `apply` | `undo_open`（D.10） |
| `ResolveConflict` | `op_id`、`choices`、`apply` | `resolve_conflict`（D.8） |
| `CleanupValues`（M3） | `instance_id`、`names` | `cleanup_values`（D.11） |
| `SetMachineSettings`（M3） | `restore_on_uninstall` | `set_machine_settings`（D.12） |

`apply` は `ApplyOptions { allow_live_reset, other_input_available }`。キーボードをリセットし得るすべての要求に付ける。既定（両方 false）ではリセットしない（C9）。`expected` は `ExpectedPlan { steps, apply }`（S6）。M3（`PROTOCOL_VERSION` 2）で `countdown_seconds`（既定 20、20 か 60 だけ。D.2 a の 6）を足した。

`Decision` は `Keep { op_id }` と `RevertNow { op_id }`。

**helper → 呼び出し元（`HelperMessage`）**: `Hello`、`Event`、`Result`、`Error`

| `Event` | 送る時点 |
|---|---|
| `Locked` | ロックを取った |
| `Planned { op_id, steps, apply, keyboards }` | `Planned` をフラッシュした（まだ何も書いていない） |
| `StepWritten { op_id, step, of }` | ステップを書いてフラッシュした |
| `StateChanged { op_id, state }` | 遷移をフラッシュした |
| `ResettingKeyboard`、`KeyboardArrived` | リセットの経路 |
| `CountdownStarted { seconds, verified }`、`CountdownTick { remaining }` | カウントダウン |
| `WaitingForReconnect` | 再接続の経路（10 秒ごと） |
| `RecoveryAssetsWritten { directory, skipped }` | 復旧用ファイルを書いた |
| `Warning { message }` | 続けられる問題 |
| `Heartbeat` | helper の書き込み専用スレッドが、要求の実行中に 10 秒ごとに送る（エンジンは送らない。S7） |

`OperationResult { op_id, outcome, failure, pending_action, conflicts, inv_ps2_violation, recovered, warnings }`。`Outcome` は `NoChange`、`Confirmed`、`AwaitingConfirm`、`PendingReboot`、`Reverted`、`RevertedPendingReboot`、`Failed`、`Conflict`、`Recovered`。`ConflictInfo` は `baseline`、`before`、`intended`、`last_written`、`current`、`write_error` を持つ。

`ErrorInfo { code, message, op_id, plan_error }`。`ErrorCode` は `Busy`、`OpInProgress`、`RecoveryNeeded`、`JournalUnreadable`、`PlanRejected`、`PlanChanged`、`MigrationRequired`、`NotFixedMode`、`UnknownKeyboard`、`UnknownOp`、`NotLatest`、`InvalidState`、`InventoryIncomplete`、`RecoveryAssetsUnavailable`、`Cancelled`、`Registry`、`Device`、`Host`、`Protocol`、`Internal`。

これらの型（`Request` とその中身、`Hello`、`Welcome` を除く）は `mklm_core::report` にあり、ipc は再公開するだけ（S5）。

フレームの例（M3 の `PROTOCOL_VERSION` 2。版 1 には `countdown_seconds` がなかった）:

```json
{"v":2,"seq":2,"body":{"type":"request","data":{"kind":"set-layout","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","layout":"jis","apply":{"allow_live_reset":true,"other_input_available":true,"countdown_seconds":20},"expected":null}}}
```

helper は、呼び出し元から受け取った値のうち上の項目以外を信用しない。デバイスは自分で列挙し直し、値は自分で読み直す（計画 2.2）。呼び出し元から来たインスタンス ID は、helper 自身の列挙結果を引くキーとしてだけ使い、Win32 に渡すのは列挙結果の文字列にする。

### E.7 タイムアウトと切断

| 場面 | 期限 | 期限切れや切断のとき |
|---|---|---|
| 呼び出し元が helper の接続を待つ | 120 秒（`CONNECT_TIMEOUT`）、または helper が終了するまで | 失敗。UAC を断られた場合は、それ以前に `Cancelled` になる。PID の違う相手は切って待ち直す |
| ハンドシェイクの各フレーム | 10 秒 | 切断。helper は終了コード 3 |
| helper が次の要求を待つ | 60 秒（`REQUEST_IDLE_TIMEOUT`） | 終了コード 0 で終わる |
| 呼び出し元がイベントを待つ | 30 秒（`MESSAGE_TIMEOUT`）。helper の書き込み専用スレッドが 10 秒ごとに `Heartbeat` を送るので、エンジンが `restart()` やロック待ちで止まっていても途切れない（S7）。helper のプロセスが終了したら、期限を待たずに失敗とする（呼び出し元はプロセスのハンドルを持っている） | 30 秒途切れるのは helper のプロセスが固まったときだけ。切断して、「helper が応答しません。終了するまで待つか、タスク マネージャーで終了してから `mklm-cli recover` を実行してください」と表示する |
| **計画中（`Planned` の前）の切断** | — | `check_cancelled` で検出し、**何も書かずに終える**（`Cancelled`。S4） |
| **書き込み中（`Planned` から `Written` まで）の切断** | — | 始めた書き込みは `Written` まで書き切る |
| **`Restarting` の直前の切断** | — | `check_cancelled` で検出し、リセットせずにロールバックする（`Reverted`、`failure = CallerDisconnected`。C13） |
| **カウントダウン中の切断** | — | **すぐに取り消す**（`Disconnected` → `RevertPending`）。元の配列に戻すリセットは行う |
| **カウントダウン中の helper の死**（呼び出し元から見て、パイプが切れ、helper のプロセスが終了した） | — | 呼び出し元は「helper が止まりました。変更を戻します」と表示し、**すぐに新しい helper を起動して `Recover` を送る**（非昇格なら UAC がもう一度出る。C8）。回復は `CountdownExpired` でロールバックする。利用者が別の入力手段を申告していれば、同じ `ApplyOptions` でリセットまで行う |
| 再接続の経路で判断を待つ | 600 秒（`DECISION_TIMEOUT`） | `AwaitingConfirm` のまま（自動では戻さない） |
| 再接続と再起動の経路の書き込み後の切断 | — | `AwaitingConfirm` / `PendingReboot` のまま残る。呼び出し元は次にジャーナルを読んだときにそれを知り、RunOnce を登録する（C17） |

書き込み中の切断に対する扱いは、同じ時点で helper が死んだ場合の回復の判定（C.7）と同じ結果になるように決めている。

### E.8 helper の終了コード

| コード | 意味 |
|---|---|
| 0 | 正常に終了した（`Bye`、切断、待ち時間切れ） |
| 1 | その他の失敗（ログに記録） |
| 2 | コマンドラインが固定書式でない |
| 3 | 接続かハンドシェイクに失敗した（PID、nonce、版、ビルド ID、期限） |
| 4 | 昇格していない |
| 5 | 対応していない OS（build 26100 未満） |

呼び出し元は、パイプがつながらなかったときにプロセスのハンドルから終了コードを読んで、エラーの表示に使う。

---

## F. CLI（apps/mklm-cli）の書き込みコマンド

### F.1 コマンド

`<keyboard>` はインスタンス ID か `#n`。`#n` は `mklm-cli list`（接続中のキーボードだけ、`list` と同じ並び）の行番号。phantom はインスタンス ID で指定する。CLI は常に、解決した名前とインスタンス ID を表示してから進む。

| コマンド | 内容 |
|---|---|
| `set <keyboard> --layout jis\|us\|standard [--no-reset] [--other-input] [--yes] [--answer keep\|revert]` | 配列を割り当てる（D.2） |
| `migrate [--standard jis\|us] [--also <keyboard>=<layout>]... [--yes]` | 固定モードからキーボードごとモードへ（D.3）。`--standard` を省くと、今の固定の配列 |
| `undo [--no-reset] [--other-input] [--yes]` | 確認待ち、再起動待ち、衝突の変更をまとめて取り消す（D.10）。**困ったときに最初に使うコマンド** |
| `revert <op> [--no-reset] [--other-input] [--yes]` | 操作を取り消す（D.4） |
| `restore --baseline (--all \| <keyboard>) [--on-conflict report\|skip\|overwrite] [--no-reset] [--other-input] [--yes]` | 導入前に戻す（D.5） |
| `recover [--no-reset] [--other-input] [--yes]` | 中断された操作を回復する（D.7） |
| `resolve <op> [--all keep-current\|before\|intended\|baseline] [--value <n>=<choice>]... [--no-reset] [--other-input] [--yes]` | 衝突を解決する（D.8）。`--value` は記録ごとの選択で、`--all` より優先する。どちらもなければ、値の一覧を表示して記録ごとに尋ねる。M2 に CLI の解決手段がなかったので足した（C11 の「現在の値を残す」を CLI でも選べるようにするため） |
| `keep <op>`（別名 `confirm`） | 確認待ちの操作を確定する（D.6）。再起動で反映する操作には、`post-reboot` と同じ確認画面を出す |
| `reboot [--yes]` | PC を再起動する（F.4） |
| `post-reboot`（ヘルプには出さない） | RunOnce から起動され、再起動後の確認をする（D.7） |
| `journal [--json]` | ジャーナルを表示する（読み取り専用） |

`<op>` は操作 ID か、8 桁以上の一意な先頭部分。

**リセットの申告**（`--other-input`、`--no-reset`。C9、S12）

- キーボードをリセットし得るすべてのコマンドが持つ（`ApplyArgs`）。どちらもなければ、CLI はリセットが必要になった時点で「別のキーボードかマウスとスクリーンキーボードで入力できますか? [y/N]」と尋ねる。`--no-reset` なら尋ねずにリセットしない。
- `set` と `restore` の `--yes` は、`--other-input` か `--no-reset` のどちらかがないと使い方の誤り（終了コード 2）にする（clap の `requires = "reset_choice"`。テスト済み）。`--yes` だけでは申告が「なし」になり、USB キーボードの変更が再起動の経路（`PendingReboot`）に落ちて、再起動まで以後の操作がすべて止まってしまうため。
- `revert`、`undo`、`recover`、`resolve` は、申告がなければリセットしない（戻した値は `apply_pending` として案内する）。止まることはないので `--yes` だけでもよい。
- `--yes` と `#n` の組み合わせは拒否し、インスタンス ID を求める（`list` と `set` の間にキーボードが増減すると、`#n` が別のキーボードを指すため）。`--yes` がなければ、`#n` を解決した名前とインスタンス ID を表示して確認を取る。

### F.2 共通の流れ

1. `restrict_dll_search()`。OS のビルドを確認する。
2. 前回のセッションの取りこぼしを拾う: 非昇格でジャーナルを読み、再起動待ちのエントリがあれば RunOnce を登録する（F.4。C17）。
3. `<keyboard>` を解決する。
4. **非昇格で下見をする**: `snapshot_report(all = true)` を実行し、`blocks_writes` な読み取りの問題があれば止める（それ以外は警告を表示して続ける。S2）。core の `plan_set_layout` / `plan_migration`（エンジンと同じ関数）で、変わる値、必要な操作（リセット / 再接続 / 再起動）、全ユーザーに適用されることを表示する。UAC の事前説明（計画 3.5: 「次の確認画面では発行元が『不明』と表示されます。プログラム名が mklm-helper.exe であることを確認して『はい』を押してください」）も出す。`--yes` がなければ「続けますか? [y/N]」と尋ねる。下見の `steps` と `apply` を `expected` として送る。
5. helper のビルド ID を確かめる（E.3）。
6. パイプを作り、helper を起動する（非昇格は UAC、昇格済みは `CreateProcessW`。E.1）。イベントと答えを中継する。
   - `--in-process`（`recover`、`undo`、`restore` の隠しオプション）の場合だけ、昇格していることを確かめてから `Engine<WinRegistry, WinDevices, WinHost>` を同じプロセスで動かす。このとき `restrict_dll_search()` の失敗は致命的なエラーにする（管理者権限で動くため）。`SetConsoleCtrlHandler` を登録し、`CTRL_C`、`CTRL_BREAK`、`CTRL_CLOSE` を「今すぐ戻す」に対応させ、少なくともレジストリの書き戻しと J の FJ を終えてから返す（`CTRL_CLOSE` の猶予はおよそ 5 秒。C8）。
7. 結果を表示する。セッションがどう終わっても（エラーでも）、非昇格でジャーナルを読み直し、再起動待ちのエントリがあれば RunOnce を登録する（F.4。C17）。終了コードを返す（F.5）。

### F.3 カウントダウンの表示

CLI の出力は、当面は英語（M1 と同じ）。

```
Keychron Receiver: switched to JIS (Raw Input reports 0x7/0x2, as expected).
Type Shift+2 in any text box: " means JIS, @ means US.
Keep this layout? [y/N]  reverting automatically in 20 s
```

- コンソールでは、最後の行を毎秒 `\r` で書き換える。リダイレクトされている場合は、5 秒ごとに 1 行ずつ出す。
- 入力: `y` または `yes` で Keep。`n`、`no`、Enter だけで RevertNow。入力の終わり（EOF）は「答えなし」として扱い、カウントダウンが切れるのを待つ。
- 標準入力は別のスレッドで 1 行ずつ読み、チャネルに送る。中継のループは `recv_timeout` で待ち、答えを `Decision` としてパイプに送る。
- Ctrl+C やウィンドウを閉じる操作で CLI が終わると、パイプが切れて helper がすぐに戻す（E.7）。
- `--answer keep` は、Raw Input が期待どおりの種類を報告していれば Keep、そうでなければ RevertNow を送る。`--answer revert` は RevertNow。スクリプトから試すための指定で、打鍵の確認を代わりにするものではない。

### F.4 再起動と再起動後の確認

- **`reboot`**
  - 今の起動で `PendingReboot` か `RevertedPendingReboot` になった操作、または `apply_pending = RestartPc` の操作がなければ、「再起動が必要な変更はありません」と表示して終わる。
  - サインイン画面で入力できないときの手段（スクリーンキーボード、PIN、別のキーボード）を表示する。`--yes` がなければ `restart` と入力させる（計画 3.6 の確認チェックに相当）。
  - `session::restart_pc()` を呼ぶ（`SE_SHUTDOWN_NAME` を有効にしてから `InitiateShutdownW(SHUTDOWN_RESTART | SHUTDOWN_RESTARTAPPS, SHTDN_REASON_MAJOR_OPERATINGSYSTEM | SHTDN_REASON_MINOR_RECONFIG | SHTDN_REASON_FLAG_PLANNED)`）。昇格は要らない。
- **RunOnce**
  - 登録するのは非昇格の呼び出し元。`HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` に `SHINDATACENTER.MKLM.PostReboot` = `"<絶対パス>\mklm-cli.exe" post-reboot`。
  - **登録の条件はジャーナルで決める**（結果を受け取ったかどうかではない。C17）: 同じ起動の `PendingReboot`、または再起動で反映する操作の `AwaitingConfirm`（`apply = RestartPc`）があれば登録する。確かめる時点は、helper のセッションが終わるたび（エラーや切断でも）、`recover` の後、書き込みのコマンドを始めるとき（F.2 の 2）。回復のロールフォワードで `PendingReboot` になった場合や、helper が `PendingReboot` をフラッシュした直後に死んだ場合も、これで拾える。
  - helper は HKCU に触れない（計画 2.2）。
  - 標準ユーザーが別の管理者の資格情報で昇格した場合も、登録するのは非昇格の呼び出し元（正しいユーザー）なので問題ない。`--in-process` で昇格したまま動いた場合は、別の管理者アカウントで動いている可能性があるので、登録せずに「再起動後に `mklm-cli post-reboot` を実行してください」と表示する。昇格したコンソールで実行した場合（`--in-process` でなくても）も CLI 自身が昇格したアカウントで動くので、同じく登録せずに表示する。昇格しているかを確かめられないときも登録しない。
  - 計画 3.6 では GUI が `mklm.exe --post-reboot` を登録する。CLI から操作した場合は CLI を登録する（M3 では、GUI がインストールされていれば GUI を登録する）。
- **`post-reboot`**: D.7 の「再起動後の確認」。

### F.5 終了コード

| コード | 意味 |
|---|---|
| 0 | 完了（確定、依頼どおりの取り消し・undo、変更なし、回復済み）。すでにキーボードごとモードの PC での `migrate`（`--also` なし）も「変更なし」 |
| 1 | 失敗（何も変わっていないか、エラーのためにすべて戻した。メッセージで区別する）。`--also` 付きの `migrate` がキーボードごとモードの PC で `NotFixedMode` になった場合も 1（割り当ては `set` で行う） |
| 2 | 使い方の誤り（clap。`--yes` の制約を含む） |
| 3 | 取り消した（UAC を断った、書く前の確認で「いいえ」、呼び出し元が去った `Cancelled`） |
| 4 | 自動で元に戻した（カウントダウン切れ、確認の失敗、キーボードが戻らない。`failure` が `CountdownExpired`、`KeyboardDidNotReturn` など） |
| 5 | 衝突（`resolve <op>` か `undo`、または GUI で解決する） |
| 6 | 止められた（ロック中、ほかの操作が open、回復が必要） |
| 10 | 書いた。再接続して `keep <op>` か `revert <op>` を待っている |
| 3010 | PC の再起動が必要（msiexec の `ERROR_SUCCESS_REBOOT_REQUIRED` と同じ値） |

読み取り専用のコマンド（`list`、`status`、`global status`、`journal`）は、これまでどおり 0 / 1 / 2 を使う。書き込みコマンドの `--json` 出力は M2 では作らない（M3 で必要なら足す）。

---

## G. 復旧用ファイルと docs/recovery.md

### G.1 生成するファイル

| 項目 | 内容 |
|---|---|
| 場所 | `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery\` |
| DACL | `RECOVERY_DIR_SDDL`: SY と BA がフルコントロール、Users が読み取り |
| ファイル | `restore-offline.cmd`、`mklm-baseline.reg`、`README.txt`（それぞれ直前の版を `*.prev` として残す） |
| 書く時点 | 新しい baseline を記録する操作の `Planned` より前（C.5 の 2）と、baseline を消したとき。加えて、隔離（D.9）で `SHIN DATA CENTER` か `MKLM` が改名され、`Recovery` ごと動いたとき（`Host::take_recovery_assets_moved`）は、`Baselines` が空でなければ同じ要求の前処理（D.1 の 6 の後）で書き直す（失敗しても警告だけ） |
| 内容のもと | `Baselines` のすべてと、その操作で新しく記録する baseline（その PC の実際のインスタンスパス） |
| 書き方 | `WriteFile` → 一時ファイルの `FlushFileBuffers` → 今のファイルを `*.prev` に → 一時ファイルを `MoveFileExW(REPLACE_EXISTING \| WRITE_THROUGH)` で置き換え → フォルダーのハンドルの `FlushFileBuffers`（C10） |
| 失敗したとき | i8042prt の値か全体の値を変える操作は、**対象の値を書く前に中止する**（`RecoveryAssetsUnavailable`。何も書かず、ジャーナルにも書かない）。HID だけの操作は警告にとどめる |

**耐久性の理由**（C10）: `MOVEFILE_WRITE_THROUGH` が保証するのは名前の変更だけで、一時ファイルの中身がディスクに書かれたとは限らない。NTFS はメタデータしかジャーナルに記録しないので、置き換えの直後に電源が落ちると、名前は新しいのに中身がゼロや古いデータ、ということが起こり得る。起動時の値を変える移行でそれが起きると、WinRE で `restore-offline.cmd` が実行できず、オフラインで戻す手段がないまま起動時の値が変わる。

**計画との違い**: 計画 2.2 では ProgramData のフォルダーを SY と BA だけにしている。`Recovery` には Users の読み取りを足す。ユーザーが自分で USB メモリなどにコピーできるようにするため（計画 2.3「USB メモリなどに控えておくよう案内する」）。
そうしないと、エクスプローラーで開いたときに「アクセス許可がありません」→「続行」でユーザーの ACE が足され、その後の検証に失敗するおそれがある。中身はインスタンス ID と値だけで、秘密ではない（Enum は Everyone が読める）。
`SHIN DATA CENTER` と `MKLM` のフォルダー自体にも、継承しない Users の一覧表示を付ける（`BASE_DIR_SDDL`）。`logs` と `Updates` は SY と BA だけ。

### G.2 `.reg`

- UTF-16LE で BOM 付き。`Windows Registry Editor Version 5.00` で始める。
- 現在の Windows で使う。ダブルクリックすると UAC が出る。
- **順序**（C4）: ファイルは現在の値を知らないので、どこから始めても INV-PS2 を壊さない固定の順にする。i8042prt の値を**設定する**もの → 全体の値を設定するもの → HID → 全体の値を**削除する**もの → i8042prt の値を削除するもの。固定とペアは、それに頼るものを消す前に必ずそろう。スクリプトが途中で止まっても（電源断など）、baseline そのものが違反していない限り INV-PS2 は壊れない。
- 書式: `"名前"=dword:0000000N`、`"名前"="文字列"`（`\` と `"` はエスケープ）、`"名前"=-`（削除）、`Other` は `"名前"=hex(N):..`。

```
Windows Registry Editor Version 5.00

; MKLM baseline (values before MKLM changed them), generated 2026-09-27 04:00 UTC by MKLM 0.1.0
; Order: PS/2 values set, global values set, HID keyboards, global values deleted, PS/2 values deleted.
; global values set
[HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters]
"OverrideKeyboardType"=dword:00000007
"OverrideKeyboardSubtype"=dword:00000002

; HID keyboards
[HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters]
"KeyboardTypeOverride"=-
"KeyboardSubtypeOverride"=-

; PS/2 values deleted
[HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters]
"OverrideKeyboardType"=-
"OverrideKeyboardSubtype"=-
```

（これは、M0 の前の固定 JIS の状態から MKLM が移行した場合の例。空になる段階は書かない。）

### G.3 `restore-offline.cmd`

- ASCII、改行は CRLF。`reg add` と `reg delete` だけを使い、記録した値だけを扱う。順序は G.2 と同じ。
- `online`: Windows やセーフモードで、管理者として実行する（`HKLM\SYSTEM\CurrentControlSet`）。
- `offline <ドライブ>`: WinRE のコマンドプロンプトで実行する。SYSTEM ハイブを読み込み、**`Select\Default`**（次の起動で使われる制御セット）から `ControlSet00N` を求める。`Select\Current` と違うときは、何も書かずにメッセージを出して止める（C11。以前の版は `Current` を使っていた）。
- **コマンドの注入を防ぐ**: インスタンス ID には USB のシリアル番号が入るので、デバイスが細工した文字列を含む可能性がある。cmd に書く文字列はすべて `is_cmd_safe` を通す。許すのは ASCII の英数字、空白、`_ - . \ & # { } ( ) , ; =` だけで、`"`、`%`、`!`、`^`、`<`、`>`、`|`、制御文字、非 ASCII は拒否する。拒否した記録は書かず、`rem` 行に番号だけを書く（元の文字列は書かない）。
- `Other` の値は、`REG_BINARY`（3）なら `reg add /t REG_BINARY` で書く。それ以外の型は cmd では扱わず、`rem` で `.reg` を使うよう案内する。

```bat
@echo off
rem MKLM recovery script, generated 2026-09-27 04:00 UTC by MKLM 0.1.0.
rem Puts every keyboard value MKLM changed back to its value before MKLM (baseline).
rem   restore-offline.cmd online        Windows or Safe Mode, from an administrator prompt
rem   restore-offline.cmd offline D:    WinRE command prompt; D: is the Windows drive
rem Run this copy (on the Windows drive). A copy on a USB stick is for reading only.
setlocal EnableExtensions DisableDelayedExpansion
if /i "%~1"=="online" goto :online
if /i "%~1"=="offline" goto :offline
goto :usage

:online
set "ROOT=HKLM\SYSTEM\CurrentControlSet"
call :apply
exit /b %ERRORLEVEL%

:offline
if "%~2"=="" goto :usage
if not exist "%~2\Windows\System32\config\SYSTEM" (echo SYSTEM hive not found under %~2\Windows& exit /b 2)
reg load HKLM\MKLM_OFFLINE "%~2\Windows\System32\config\SYSTEM" >nul || (echo reg load failed& exit /b 3)
set "CS="
set "CUR="
for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Default ^| findstr /c:"Default"') do set /a CS=%%A
for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Current ^| findstr /c:"Current"') do set /a CUR=%%A
if not defined CS (reg unload HKLM\MKLM_OFFLINE >nul & echo Select\Default not found& exit /b 4)
if not "%CS%"=="%CUR%" goto :cs_mismatch
set "CSN=00%CS%"
set "ROOT=HKLM\MKLM_OFFLINE\ControlSet%CSN:~-3%"
call :apply
set "RC=%ERRORLEVEL%"
reg unload HKLM\MKLM_OFFLINE >nul
exit /b %RC%

:cs_mismatch
reg unload HKLM\MKLM_OFFLINE >nul
echo Select\Default is %CS% but Select\Current is %CUR%. Nothing was changed; see docs\recovery.md.
exit /b 5

:apply
set "FAILED="
rem global values set
reg add "%ROOT%\Services\i8042prt\Parameters" /v "OverrideKeyboardType" /t REG_DWORD /d 7 /f >nul || set "FAILED=1"
reg add "%ROOT%\Services\i8042prt\Parameters" /v "OverrideKeyboardSubtype" /t REG_DWORD /d 2 /f >nul || set "FAILED=1"
rem HID keyboards
call :del "%ROOT%\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters" "KeyboardTypeOverride"
call :del "%ROOT%\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters" "KeyboardSubtypeOverride"
rem PS/2 values deleted
call :del "%ROOT%\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" "OverrideKeyboardType"
call :del "%ROOT%\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" "OverrideKeyboardSubtype"
if defined FAILED (echo Some values could not be restored.& exit /b 1)
echo Done. Restart Windows (a restart, not a shutdown).
exit /b 0

:del
reg delete "%~1" /v "%~2" /f >nul 2>&1
reg query "%~1" /v "%~2" >nul 2>&1 && set "FAILED=1"
exit /b 0

:usage
echo usage: restore-offline.cmd online ^| offline D:
exit /b 2
```

`reg delete` は値がなければ失敗するので、結果は無視する。代わりに `reg query` で値が残っていないことを確かめる。括弧の中の `echo` にかっこを書かない（ブロックが閉じてしまう）ため、制御セットの不一致はラベル `:cs_mismatch` で扱う。

### G.4 README.txt

UTF-8（BOM 付き）で、日本語と英語。書く内容:

- **最初に試すこと**: `mklm-cli undo`（確認待ち、再起動待ち、衝突の変更をまとめて戻す）。次に `mklm-cli recover`、`mklm-cli restore --baseline --all`。
- このフォルダーを USB メモリなどにコピーしておくこと。ただし**実行するのはディスク上の `...\ProgramData\SHIN DATA CENTER\MKLM\Recovery\restore-offline.cmd` だけ**で、USB のコピーはインスタンス ID と手作業の手順を読むための控えであること（S13。USB 上のファイルは利用者のプロセスが書き換えられ、WinRE では SYSTEM 権限で実行されるため）。
- 3 つのファイルの役割。
- `docs/recovery.md` の要約（WinRE での手順）。
- 生成した日時と MKLM の版。

### G.5 docs/recovery.md の構成（日本語。M2 で書く）

1. **この文書を使う場面**: サインインできない、キーの配列がおかしい、MKLM が起動しない。
2. **まず試すこと（1 行）**: 管理者でなくてよいコマンドプロンプトで `mklm-cli undo`（UAC が出る）。操作 ID を知らなくても、確認待ち、再起動待ち、衝突の変更をまとめて戻す（C7）。その後、PC を**再起動**する。
3. **Windows にサインインできる場合**
   - GUI の「元に戻す」。
   - `mklm-cli undo`、`mklm-cli recover`、`mklm-cli restore --baseline --all`。
   - 最後に PC を**再起動**する（シャットダウンではない。高速スタートアップの注意）。
4. **サインイン画面で入力できない場合**
   - スクリーンキーボード（サインイン画面の右下のアクセシビリティ → スクリーン キーボード）。
   - PIN（数字だけの PIN は配列の影響を受けない）、指紋、顔。
   - 別のキーボードを USB でつなぐ。
   - 記号を含むパスワードの場合の JIS と US の違い（例: Shift+2 は JIS で `"`、US で `@`）。主な記号の対応表を載せる。
5. **セーフモードでの復旧**
   - 起動方法: サインイン画面の電源ボタン → Shift を押しながら「再起動」→ トラブルシューティング → 詳細オプション → スタートアップ設定 → 再起動 → 4。
   - 管理者のコマンドプロンプトで `mklm-cli undo`、`mklm-cli recover`、`restore --baseline --all`、または `restore-offline.cmd online` を実行する（昇格済みなので UAC は出ない。helper は別プロセスとして動く）。
   - 再起動する。
6. **回復環境（WinRE）での手動復旧**
   - 起動方法（上と同じ手順で「コマンド プロンプト」を選ぶ）。
   - **BitLocker（デバイスの暗号化）**: 回復キーを求められる。回復キーの場所（Microsoft アカウントの「デバイス」→「回復キー」、https://aka.ms/myrecoverykey）。求められない場合は `manage-bde -status` と `manage-bde -unlock C: -RecoveryPassword <キー>`。
   - Windows のドライブ文字を確かめる（WinRE では D: になることが多い。`dir D:\Windows`）。
   - **スクリプトを使う場合**（C11。空白を含むパスなので、フォルダーに移ってから実行する）:
     1. `cd /d "D:\ProgramData\SHIN DATA CENTER\MKLM\Recovery"`
     2. `restore-offline.cmd offline D:`
     - `"` のキーの位置を書き添える: JIS 配列では Shift+2、US 配列では Shift+'（Enter の左）。WinRE で選んだ配列とキーボードが違うと迷うため。
     - USB メモリのコピーではなく、ディスク上のこのファイルを実行する（S13）。
   - 手作業の場合:
     1. `reg load HKLM\MKLM_OFFLINE D:\Windows\System32\config\SYSTEM`
     2. `reg query HKLM\MKLM_OFFLINE\Select /v Default` で番号を見る（例: `0x1` → `ControlSet001`）。オフラインでは `CurrentControlSet` は存在しない。`Current` と違う場合は作業をやめてサポートに相談する。
     3. 全体の値を固定モードに戻す: `reg add "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters" /v OverrideKeyboardType /t REG_DWORD /d 7 /f`（Subtype は 2）。
     4. 必要なら、キーボードごとの値を消す: `reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v KeyboardTypeOverride /f`（インスタンス ID は README.txt か `.reg` に書いてある）。
     5. `reg unload HKLM\MKLM_OFFLINE` → `exit` → 「続行」で Windows を起動する。
   - 固定モード（全体 7/2）に戻せば、PS/2 の内蔵キーボードは必ず JIS として動く。サインインできるようにするための最小限の手順はこれだけ、と明記する。
   - **オフラインで戻した後**: ジャーナルには open なエントリが残っていることがある。次に MKLM を起動すると衝突（`Conflict`）が表示されるので、「現在の値を残す」を選ぶ（CLI では `mklm-cli resolve <op> --all keep-current`）（C11）。
7. **事前の準備**: Recovery フォルダーのコピー（読むための控え）、BitLocker の回復キーの確認、PIN の設定、スクリーンキーボードの出し方の確認。
8. **よくある質問**: 再起動しても変わらない（シャットダウンと再起動の違い）、「まだ反映されていません」と表示される（`apply_pending`。キーボードを抜き差しするか再起動する）、`Conflict` と表示された、M0 の手作業のバックアップとの関係、など。

---

## H. テスト計画

### H.1 単体テスト（`cargo test --workspace`。昇格不要、実機の設定を変えない）

**mklm-core**

| モジュール | テスト |
|---|---|
| `journal` | `OpId::parse` と `BootId::parse`（正しい形、大文字や波かっこは拒否）。`value_eq`（`LayerDriver JPN` と `OverrideKeyboardIdentifier` だけ大文字小文字を区別しない。DWORD や他の名前は完全一致）。11 状態の分類（`is_open`、`is_in_flight`）。**11×11 のすべての組み合わせ**で `can_transition_to` が C.4 の表と一致すること（表を元にしたテスト）。`take_over` が状態を変えないこと。JSON の往復（省略可能なフィールドがない古い JSON も読める）。新しい版の拒否（`NewerSchema`）。`Journal::parse` が壊れた値を `unreadable` に入れること。`latest_record`、`prunable`（C.9 の条件。`apply_pending` が有効な操作は残る）、`resolve_prefix`（あいまいな指定）、`needs_post_reboot_check` |
| `recovery` | `decide_recovery` を、状態 × 観測の組み合わせ × 起動（同じ / 違う）× `apply` × `countdown` × 操作の種類（設定 / 移行 / 対話の導入前に戻す / サイレントの導入前に戻す）× INV-PS2（成り立つ / 壊れている）の**すべての組み合わせ**で、C.7 の表と照合する。`attention`（`Busy`、`Recover`、`NeedsApply` を含む C.7 の表と、`blocks_writes`）。`state_after_resolution`。`apply_pending_on_close`（`Written` に届かなかった操作は `None`、リセット済みのキーボードは除く、回復が書いた値は `Reconnect` より軽くしない）と `apply_pending_cleared`（起動の変化、Raw Input の一致） |
| `restore` | **段階の順**（C.5）: 移行済み → 固定モード（ペアを足す → HID → 固定を外す）、固定モード → キーボードごとモード（固定を足す → HID → ペアを消す）、PS/2 の値だけを変える、全体の `LayerDriver JPN` だけを変える、の各場合で、各ステップの後に INV-PS2 が成り立つこと。値の名前の許可リスト。`Other` は baseline としてだけ許すこと。`RestoreTo::Resolution` で `resolve_to` のない記録を拒否すること（`NoResolution`）。もともとの違反を戻す場合の `restores_inv_ps2_violation` |
| `operation` | `plan_set_layout`（固定モードで `MigrationRequired`、PS/2 に `standard`、同じコンテナのコレクションへの展開、phantom のコレクション、`other_input_available = false` で `only_usable`、`allow_live_reset = false` で `Reconnect`）。`plan_migration`（phantom の PS/2 も固定する、PS/2 への割り当てが固定値に優先する、全体は違う値だけ書く、固定 US からは US で固定する、`apply = RestartPc`）。`apply_method` の組み合わせ表 |
| `recovery_assets` | 開発機の baseline から作った出力をゴールデンファイルと比べる。**固定の順序**（PS/2 の設定 → 全体の設定 → HID → 全体の削除 → PS/2 の削除）。`Select\Default` を使い、`Current` と違えば止まる行があること。`is_cmd_safe` の表（`"&calc&"`、`%PATH%`、`^`、`!x!`、非 ASCII など、注入を狙った文字列を含む）。`.reg` が UTF-16LE の BOM 付きであること。削除の書式 |
| `report` | すべての型の serde の往復 |
| `boot`（0.1.1） | カウンター形式（`from_boot_counter(7)` の文字列、1、7、0xffff_ffff の往復、0.1.x の形の検査に通ること）。`is_legacy`（ローダーの GUID、`BootId(0)`、`BootId(1)`、カウンター形式の予約・バージョン・バリアントのビットを 1 つ反転したもの）。**デスクトップ PC の 2 つのエントリをそのまま**（`testdata/journal/legacy-guid`）: 再起動の後の起動時刻では前の起動（読み替えなし、`attention` は `Recover` / `None`、`RebootObserved`、`apply_pending_cleared`）、書いた起動の起動時刻では今の起動（読み替え、`WaitingForReboot` / `NeedsApply`、`Leave`）。許容幅の境界（±100,000,000 と ±100,000,001、0 と `u64::MAX` の付近）。ヒントがない場合と起動時刻がない場合の GUID の規則（等しい / 違う / 読めない）。`boot_id` と `since` を別々に、エントリごとに判定すること。履歴を書き換えない、カウンターの ID に触れない、2 回行っても同じ、`to_json` の版とフィールドの順 |

**mklm-ipc**

- `HelperArgs::parse` と `HELPER_ARGS_PATTERN` の一致。受け付けないもの: 空白 2 つ、タブ、大文字の 16 進、余分な引数、引用符、PID 0、u32 を超える PID、先頭の 0、Unicode の数字。
- `command_line_tail`（引用符あり / なしの argv[0]、空白を含むパス）。
- フレーム（往復、長さ 0、上限超え、途中で切れる、壊れた JSON、**版の違う相手の本文が `Json` ではなく `Version` になること**）。
- ハンドシェイク（nonce の不一致、PID の不一致、ビルド ID の不一致）。
- すべてのメッセージの serde の往復。`restore-baseline` の要求に `"silent": true` を付けても、サイレントの操作にはならないこと（フィールドがない）。
- JSON の形を固定したスナップショットのテスト。形が変わったら `PROTOCOL_VERSION` を上げる必要があることに気付けるようにする。

**mklm-engine**（`MemoryRegistry`、`FakeDevices`、`FakeHost`、`ScriptedSink`、`mklm_core::fixtures`）

- **正常系**
  - USB の `set` → Keep → `Confirmed`。RevertNow、時間切れ、切断のそれぞれで `Reverted` になり、もう一度リセットされること。
  - BLE の `set` → 再接続待ちの `AwaitingConfirm`（`apply_pending = Reconnect`）。Raw Input が変わる前の Keep → `Confirmed` と `apply_pending` が残る。`reconnect()` の後の前処理で消える。
  - PS/2 の `set` → `PendingReboot`。
  - M0 の前の開発機（固定 JIS）の `migrate` → `PendingReboot` → `reboot()` → `recover` → `AwaitingConfirm` → `confirm` → `Confirmed`。
  - 移行の `revert` → `RevertedPendingReboot` → 再起動 → `Reverted`。
  - `restore --baseline --all`。
  - `undo`: `PendingReboot` の移行、`AwaitingConfirm` の再接続待ち、`Conflict` のエントリ（衝突している記録は書かれず、ほかは戻る）のそれぞれ。
- **クラッシュ時の整合性**: シナリオ S ごとに、n = 0 から「S の変更を伴う呼び出しの回数」まで `crash_after = n` で実行する。すべての `crash_images()` について、`after_crash` で新しいエンジンを作り（`become_process` で別プロセスとして。再起動した場合としない場合の両方）、`recover` して次を確かめる。シナリオは、USB の `set`、BLE の `set`、PS/2 の `set`、`migrate`、移行の `revert`、**移行済みの状態からの `restore --baseline --all`**（C4）、`undo`、**衝突の解決**（`UseIntended` と `KeepCurrent` の組み合わせ。C5）。
  - **I1**: 操作ごとに、値がすべて `before` か、すべて `intended` になっている（外部の変更がないので `Conflict` は出ない）。「導入前に戻す」は、すべて `intended`（= baseline）になっている（C14）。さらに、各キーボードについて、報告値が保存値から予想される種類と一致するか、`apply_pending` が残っている（C1）。
  - **I2**: INV-PS2 が、**すべてのクラッシュ状態で**（回復の前でも）保たれている。S の前に保たれていた場合。
  - **I3**: 回復の後に、書き込み中の状態のエントリが残っていない。`countdown` 付きの `AwaitingConfirm` も残っていない。
  - **I4**: 回復を 2 回行っても 1 回と同じ結果になる。回復の途中でクラッシュさせて（`crash_after` を回復に適用）もう一度回復しても、中断しなかった場合と同じ結果になる。
  - **I5**: ジャーナルが読める。書いた値にはすべて baseline がある。
  - **I6**: どのクラッシュ状態でも、初期状態から変わった対象の値があるなら、`Planned` のエントリが永続化されている。起動時の値が変わっているなら、復旧用ファイルが耐久的に書かれている。
  - **I7**（C3）: `deny_target` で同じ対象への書き込みを毎回失敗させても、回復はエントリを `Conflict`（`write_error` 付き）にして終わり、2 回目の回復は何も書かない（繰り返さない）。`attention` は `Recover` ではなく `Conflict` になる。
  - 解決のシナリオでは、`KeepCurrent` を選んだ記録が、どのクラッシュ状態から回復しても書き換えられないこと（C5）。
- **エラーの注入**（`fail_at = n`）: 操作は失敗で終わり、値はすべて `before`（`Reverted` と `failure = WriteError`、または `Failed(ConcurrentChange)`）。ロックは解放され、書き込み中の状態は残らない。
- **取り消し点**（S4、C13）: `cancel_after(0)` の `set` と `migrate` は何も書かず、エントリも作らない（`Cancelled`）。`Planned` のイベントの後に去った USB の `set` は、`Written` まで書いた後、リセットせずに（`restarted()` が空）`Reverted` になる。
- **カウントダウンの上限**（C18）: シンクが答えないまま `FakeHost::advance` で単調時計を 25 秒以上進めると、呼び出し回数によらず時間切れになる。`FakeReset::Hangs` は `RevertedPendingReboot`。
- **CAS**
  - `Written` の後、取り消しの前に `outside_edit` → `Conflict` と値の一覧。
  - `KeepCurrent` で解決 → `Failed(ConflictKeptCurrent)`、`last_written` = 現在の値。
  - `UseBaseline` で解決 → 値が baseline になる。
  - `KeepCurrent` の後の「導入前に戻す」では、衝突が起きない。
  - 解決の途中でクラッシュし、回復までの間に `outside_edit` があった → 回復はもう一度 `Conflict` にし、利用者が変えた値を上書きしない（C5）。
  - `outside_edit` で `LayerDriver JPN` を `KBD106.DLL` に書き直しても衝突にならない（C16）。
- **baseline**
  - Keychron に 3 回続けて操作する（US → JIS → 標準）。baseline は最初の値のまま。操作 3 の `before` は JIS。操作 3 を取り消すと JIS。導入前に戻すと最初の値。
  - baseline が `Absent` なら値が削除される。`Other` はバイト単位で保たれる。
  - `Confirmed` になった「導入前に戻す」の後に、baseline の記録が消える。その操作の `revert` は `InvalidState`（C6）。restore → set → restore で、最初の baseline に戻る。
  - `remove_devnode` で消えたキーボードの「導入前に戻す」は、`skipped = DeviceRemoved` で完了する（C3）。
- **置き換え**（C7）: `PendingReboot` の移行がある状態で `restore --baseline --all` → 移行は `Failed(Superseded)`、値は baseline。`Planned` と置き換えの間でクラッシュ → 回復（`CompleteForward`）が置き換えと書き込みを終える。
- **許可リストと INV-PS2**
  - PS/2 に `standard` → 拒否。
  - phantom の PS/2 がある状態の `migrate` → phantom も固定される。
  - キーボードごとモードで固定値のない PS/2 がある → HID の `set` も拒否（`InvPs2`）。
  - ジャーナルを改ざんして記録の名前を `Start` にする → `revert` は `NameNotAllowed` で拒否。`MemoryRegistry` も `NameNotAllowed` を返す（S9）。
  - 移行の `Written` の後に、固定値のない PS/2 の phantom を足してから回復 → `RollBack(InvPs2)`。`PendingReboot` の後なら `Conflict { inv_ps2 }`。`KeepCurrent` で違反が残る解決は `PlanRejected`（C12）。
  - `remove_system32_file("kbd101.dll")` の後の `migrate --standard us` → `LayerDriverMissing`、何も書かない（S8）。
- **ロックと前処理**: ロック中の別エンジンは `Busy`。open な操作があれば `set` は `OpInProgress`。読めないジャーナルは `JournalUnreadable`。`set_incomplete` は `InventoryIncomplete` で、何も書かない。`set_warnings`（型違いの override、表示名の読み取り失敗）は警告だけで、`set`、`revert`、`recover` が通る（S2）。`expected` の `steps` か `apply` の不一致は `PlanChanged` で、何も書かない（S6）。同じエンジン（同じプロセス）が直前の要求で残した書き込み中のエントリも、次の `recover` で回復される（C3）。
- **復旧用ファイル**: `set_fail_assets(true)` の `migrate` は `RecoveryAssetsUnavailable` で何も書かない。USB の `set` は警告付きで続く（C10）。
- **整理**: 40 件の操作 → 新しい 32 件と、`latest_record` を持つ操作と、`apply_pending` が有効な操作が残る。

**mklm-cli**

- コマンドの解析（このコミットで追加済み。`--yes` は `--other-input` か `--no-reset` を求めること、`undo`、`revert` の `ApplyArgs`、`resolve`）。
- `--yes` と `#n` の組み合わせを拒否すること。
- 終了コードの対応。`#n` の解決。
- RunOnce を登録するかの判断（`Journal::needs_post_reboot_check`）。
- カウントダウンの中継に、決めた入力を与えるテスト（標準入力を差し替えられる形で作る）。

**mklm-win**（昇格しなくてもできるもの）

- テスト用の DACL（ユーザーの SID を含む）で作ったパイプを、同じプロセス内で往復させ、PID を確認する。PID の違う相手を切って待ち直すこと。
- `is_elevated()` が通常の `cargo test` では false になる。
- `boot_id()` が 2 回呼んでも同じ。`new_uuid()` の形。`program_data_dir()`。
- 0.1.1: `boot_counter()` が 0 でなく、呼ぶたびに同じ。`boot_id()` がそのカウンター形式で legacy ではない。`legacy_boot_guid()` が 0 でなく同じ。`current_boot()` が 3 つの関数と一致する。`KUSER_SHARED_DATA` の `BootId` のオフセットが `windows` クレートの定義（テストだけで有効にする 2 つの feature）と一致する。`print_boot_id`（`#[ignore]`）は、カウンター、ID、`BootTime - BootTimeBias`（現地時刻つき）、GUID を表示する（R9/R10 用: `cargo test -p mklm-win print_boot_id -- --ignored --nocapture`）。
- 0.1.1（engine）: `WinHost` の `boot_id()` がカウンター形式、`legacy_boot_guid()` と `boot_time_hint()` が `Some`。`FakeHost` はカウンター形式で、既存のテストはそのまま通る。`tests/legacy_boot.rs` がデスクトップ PC のジャーナルで、再起動後の確定と掃除、書いた起動の中での拒否（何も書かない）、今の起動と判定したエントリを取り消したときの保存形式、`read_journal` がそのままの GUID を返すことを確かめる。
- `process_liveness(current)` が `Alive`、終了した子プロセスが `Dead`。`process_image_nt_path(自分の PID)` がテストの exe で終わる。`same_image_directory(自分の PID)` が true。
- `ReadIssueKind::blocks_writes` の表。
- 保護フォルダーの作成と隔離は昇格が必要なので、実機テスト（H.2）で扱う。

### H.2 実機テスト（ユーザーの同意と昇格が必要。後でオーケストレーターが実施する）

**毎回の準備**: `mklm-cli status --json --all > before.json`。M0 の安全手順（`reg export` で `i8042prt\Parameters`、`ACPI\FUJ0309`、Keychron の `Device Parameters` をデスクトップと USB メモリに保存。PIN とスクリーンキーボードの確認）。
**毎回の後始末**: `status --json --all` を取り直して比べる。

| # | 内容 | 手順と期待する結果 | 再起動 |
|---|---|---|---|
| R1 | 昇格と通信だけ（書き込みなし） | ジャーナルが空の状態で `mklm-cli recover` → UAC → helper → `Recovered`（対象なし）。昇格、パイプ、DACL、ハンドシェイク（ビルド ID を含む）を確かめる | なし |
| R2 | Keychron を US → JIS → US（その場でリセット、Keep） | `mklm-cli set <keychron の ID> --layout jis --other-input` → UAC → カウントダウン → ユーザーが Shift+2 で `"` を確かめて `y` → `Confirmed`。status で保存値と報告値が 7/2。続けて `--layout us` で同様に Keep → 4/0。同じレシーバーのマウス（Col03）への影響を観察する | なし |
| R3 | カウントダウン切れで自動的に戻る | `set … --layout jis` の後、答えない → `Reverted`、報告値は 4/0 に戻り、Keychron で `@` が出る。終了コード 4 | なし |
| R4 | 取り消しと導入前に戻す | R2 の JIS を Keep → `mklm-cli revert <op> --other-input` → US。`restore --baseline <keychron>` → baseline（M0 の結果から 4/0。すでに同じなら `NoChange`）。その「導入前に戻す」の `revert` が拒否されること | なし |
| R5 | 強制終了からの回復 | デバッグビルドだけの一時停止（helper が `MKLM_DEBUG_PAUSE=after-first-write` を読む。`cfg(debug_assertions)` のときだけ）→ `taskkill /F /IM mklm-helper.exe` → `mklm-cli recover` → ロールバック（`LiveResetUnconfirmed` / `Interrupted`）で値が `before` に戻り、`apply_pending` が表示される。カウントダウン中の強制終了では、呼び出し元がすぐに新しい helper を起動して戻すこと（`CountdownExpired`。C8） | なし |
| R6 | 外部の変更による衝突 | JIS を Keep → ユーザーの同意を得て、昇格したコンソールで `reg add … KeyboardTypeOverride /d 4`（外部の変更）→ `mklm-cli revert <op>` → `Conflict` と値の一覧 → `mklm-cli resolve <op> --all keep-current` → `Failed(ConflictKeptCurrent)` | なし |
| R7 | 復旧用ファイルと ACL | R2 の後に Recovery のファイルと `*.prev` があり、内容が baseline と一致する。`icacls` で、フォルダーは SY と BA がフル、BU が読み取り。ロックファイルは SY と BA だけ。`restore-offline.cmd` は目で確かめるだけにする（同意があれば `online` で実行してよい。Keychron の baseline は 4/0 なので影響はない） | なし |
| R8 | 移行の往復（**ユーザーが同意した場合だけ**） | 開発機は M0 #4 ですでにキーボードごとモードなので、先に**ユーザー自身が** `tools\m0\Invoke-M0Migration.ps1 -Undo` を実行して再起動し、固定 JIS に戻す。→ `mklm-cli migrate --standard jis --also <keychron の ID>=us` → `PendingReboot` → `mklm-cli reboot --yes` → サインイン後に RunOnce の `post-reboot` → 内蔵 7/2、Keychron 4/0、Shift+2 のテスト → Keep → `mklm-cli restore --baseline --all --no-reset` → `PendingReboot` → 再起動 → `post-reboot` → 固定 JIS に戻る。途中で一度、`PendingReboot` の状態から `mklm-cli undo` で戻せることも確かめる | 3〜4 回 |
| R9 | シャットダウンと再起動の違い、起動 ID と高速スタートアップ | **0.1.1 で書き直した**（0.1.0 は未実施のまま出荷し、GUID が完全な再起動でも変わらない PC があった。C.10「起動 ID」）。下の MT-4（高速スタートアップのシャットダウンでは変わらない）、MT-6（再起動では 1 増える）、MT-7（完全なシャットダウンでも 1 増える）。Windows の機能更新の後にもやり直す | シャットダウン 2 回、再起動 1 回 |
| R10 | 起動 ID の安定性 | **0.1.1 で書き直した**。下の MT-2（スリープ）、MT-3（休止）、MT-5（時刻の変更と `w32tm /resync /force`）で、カウンターも `BootTime - BootTimeBias` も変わらないこと。0.1.x の記録の判定は MT-8、MT-9 | 休止 1 回 |

**起動 ID の手動テスト（0.1.1。R9/R10 を置き換える。1 項目ずつ行う）**

毎回、各手順の前後に読み取りだけで記録する: KUSER のカウンター（`[Runtime.InteropServices.Marshal]::ReadInt32([IntPtr]0x7FFE02C4)`）、最新の起動の種類（`Get-WinEvent -FilterHashtable @{LogName='System';ProviderName='Microsoft-Windows-Kernel-Boot';Id=27} -MaxEvents 1`。0x0 は完全な起動、0x1 は高速スタートアップ、0x2 は休止からの復帰）、`cargo test -p mklm-win print_boot_id -- --ignored --nocapture`（カウンター、ID、`BootTime - BootTimeBias`、GUID）、`mklm-cli journal`、GUI の再起動後の確認画面（「このままにする」が押せるか）。

| # | 内容 | 手順と期待する結果 |
|---|---|---|
| MT-1 | 報告された場合 | 0.1.0 の上に修正版を NSIS のインストーラーで入れ、再起動しない。GUI が `d724c149` の確認画面を開き、「このままにする」が押せ、「まだ反映されていません」の行がない。`c10d2d38` の「再起動が必要」が消える。Shift+2 のテストをしてから Keep か元に戻す。`mklm-cli journal` で `d724c149` が `Confirmed`（履歴に `recover:reboot-observed` と `keep`）か `Reverted`、`c10d2d38` が `Reverted`（`reboot-observed`）。新しい変更を受け付ける。RDP で接続している場合は、TERMINPUT_BUS のセッション用キーボードで Keep が `Conflict` にならないかも見る |
| MT-2 | スリープ（R10） | 修正版で、再起動で反映する変更を 1 つ行う（例: 内蔵キーボードの配列）→ `PendingReboot`。`journal` の `boot_id` が `0000000N-0000-8000-8000-000000000000`。スリープ（S3）→ 復帰。カウンターも `BootTime - BootTimeBias` も変わらず、確認画面は「まだ反映されていません」、「このままにする」は押せない |
| MT-3 | 休止（R10） | 同じ操作が残った状態で `shutdown /h` → 電源を入れる。イベント 27 は 0x2。カウンターと `BootTime - BootTimeBias` は変わらず、まだ再起動していない扱い |
| MT-4 | 高速スタートアップ（R9） | 同じ状態で、スタート → 電源 → シャットダウン → 電源を入れる。イベント 27 は 0x1。カウンターと `BootTime - BootTimeBias` は変わらず、まだ再起動していない扱い。サインインのときに RunOnce がもう一度尋ねる |
| MT-5 | 時刻（R10） | 同じ状態で、時計を数分進めて戻し、`w32tm /resync /force`（管理者。同意を得る）。カウンターも `BootTime - BootTimeBias` も変わらない（バイアスが吸収する）。まだ再起動していない扱い |
| MT-6 | 再起動（R9） | スタート → 電源 → 再起動。イベント 27 は 0x0、カウンターは 1 増える。サインインすると確認画面が開き（RDP でも）、「このままにする」が押せる。Keep で `Confirmed` |
| MT-7 | 完全なシャットダウン | 再起動で反映する変更をもう 1 つ行い、`shutdown /s /full /t 0`（または Shift を押しながらシャットダウン）→ 電源を入れる。0x0、カウンターは 1 増え、再起動した扱い（ドライバーが読み直した）。その後、元に戻す |
| MT-8 | 0.1.x の記録、再起動していない | 何も open でない状態で、修正版の上に 0.1.0 を入れる（ダウングレード）。0.1.0 で再起動で反映する変更を行う（`PendingReboot`、GUID の形）。再起動せずに修正版を入れ直す。ヒントが今の起動時刻と等しいので「まだ反映されていません」、Keep は押せない。スリープと復帰の後もまだ。再起動すると Keep が押せる。元に戻す |
| MT-9 | 0.1.x の記録と高速スタートアップ | MT-8 と同じだが、再起動の前に高速スタートアップのシャットダウンをする。0x1 の起動の後も、0.1.x の操作はまだ再起動していない扱い（`BootTime - BootTimeBias` がハイブリッドの起動をまたいで保たれることの確認）。本当の再起動の後は再起動した扱い |
| R11 | パイプの防御 | セッション中に、通常の PowerShell から `NamedPipeClientStream` でパイプに接続しようとすると拒否される（`WRITE_DAC` だけの指定を含む。S10）。helper を不正な引数で手動起動すると、接続せずに終了コード 2 で終わる | なし |
| R12 | UAC を断る | `set` で UAC を「いいえ」→ 終了コード 3。何も書かれず、ジャーナルも変わらない | なし |
| R13 | BLE の再接続の経路 | 本物の BLE キーボードがないので保留 | — |
| R14 | 標準ユーザーと別の管理者の資格情報（VM で可） | 標準ユーザーで `mklm-cli set … --no-reset` → UAC で管理者の資格情報を入力 → helper がハンドシェイクに成功し（`proc_identity` が別ユーザーのプロセスのパスを読める）、書き込みが終わる。RunOnce は標準ユーザーの HKCU に登録される（S3） | なし |
| R15 | ProgramData の先回り作成（VM で可） | 別の標準ユーザーで `mkdir "C:\ProgramData\SHIN DATA CENTER\MKLM"` → 管理者で `mklm-cli recover` → 隔離の警告が出て、`SHIN DATA CENTER.untrusted-…` ができ、回復が動く。フォルダーを開いたままにした場合は、案内付きの `Insecure` で止まる（S1） | なし |
| R16 | 昇格したコンソールでの Ctrl+C | 管理者のターミナルで `set … --other-input` → カウントダウン中に Ctrl+C、別の回ではウィンドウを閉じる → helper がすぐに戻し、Keychron で `@` が出る（C8） | なし |

### H.3 CI

- 単体テストは `windows-latest`（非昇格）で実行する。実機の設定を変えるテストは CI に入れない。
- `cargo clippy --workspace --all-targets -- -D warnings` と `cargo fmt --check` を通す。

---

## I. 未解決の問題とリスク

1. **起動 ID の性質の確認**（C2）: 起動 ID を `BootTime`（時刻の補正でずれる）から、起動ごとの GUID（`SystemBootEnvironmentInformation.BootIdentifier`）に改めた。**0.1.0 は R9、R10 を行わないまま出荷し、デスクトップ PC でこの GUID が完全な再起動でも変わらず、`PendingReboot` が終わらなくなった**（`docs/research/boot-id.md`）。0.1.1 で `KUSER_SHARED_DATA.BootId`（ローダーが起動のたびに増やすカウンター）に切り替え、0.1.x の記録は履歴の起動時刻で判定する（C.10「起動 ID」）。残るリスク:
   - カウンターの性質は ntddk.h の注釈と解析（phnt/NtDoc: winload が `bootstat.dat` の `LastBootId` を増やす）でしか裏付けがない。休止や高速スタートアップからの復帰で増える Windows があれば、危険な側（再起動していないのに再起動したと判断する）に誤る。MT-3、MT-4 で確かめ、Windows の機能更新の後にも R9/R10 をやり直す。
   - `bootstat.dat` が書かれない環境（UWF/HORM などの書き込みフィルター、読み取り専用や故障したディスク）では、続く起動が同じカウンターになり、再起動しても「再起動していない」と読む。安全な側（増える再起動まで止まる。取り消しはできる）。
   - カウンターが戻る（`bootstat.dat` の作り直し）と、未解決のエントリが記録した値と重なった場合だけ、その起動の間「再起動していない」と読む。安全な側で、次の再起動で直る。
   - 0.1.x の記録の判定は、起動の間 `BootTime - BootTimeBias` が 10 秒以内に保たれることを前提にする。休止からの復帰でバイアスなしに `BootTime` が動くと、修正版を入れた起動の中で作られた 0.1.x の操作を再起動したと読むおそれがある（危険な側。その 1 回の起動だけで、確認画面の Raw Input と Shift+2 のテストは残る）。MT-8、MT-9 で確かめる。
   - ヒントのない 0.1.x のエントリ（M2 の開発中のものだけ）は GUID で判定するので、GUID が変わらない PC では再起動していない扱いのままになる（取り消しはできる）。v0.1.0 は必ずヒントを記録しているので、公開版を使っている PC には関係しない。
   - 同じ起動の中で 0.1.x にダウングレードすると、0.1.x は新しい形のエントリを前の起動のものと読む（危険な側。その起動の間だけ）。更新はダウングレードしないが、NSIS のインストーラーはできる。「再起動待ちの変更がある間はダウングレードしない」と案内する（インストーラーでの防止は任意）。
   - 固定アドレス 0x7FFE0000 の読み取り: ユーザーモードの ABI（kernel32 と ntdll が時刻をここから読む）。ランダム化されたのはカーネル側の書き込み用の別名だけ（MSRC 2022）。「最小」プロセスにはこの割り当てがないが、MKLM のプロセスは最小ではない。ARM64 上の x64 エミュレーションでも同じ。
2. **フラッシュとハイブの原子性の仮定**: 設計が頼るのは、(a) 各 API 呼び出しが原子的であること、(b) `RegFlushKey` がそれ以前の書き込みを永続化すること、の 2 点だけ。同じハイブの中で順序が入れ替わらないことには頼らない（ステップごとに FT するため。C.5）。インメモリのモデル（B.4）は、これより厳しい条件でテストする。
3. **phantom の devnode への書き込み**: `CM_Open_DevNode_Key(RegDisposition_OpenAlways)` で phantom のハードウェアキーを書き込み用に開けるかは未検証（M1 では読み取りだけ確かめた）。開発機には phantom の PS/2 がないので、実機では確かめにくい。VM での確認を提案する。
4. **`DI_NEEDREBOOT` の意味**: kbdhid のリセットで立つことがあるかは未確認。立った場合は安全側（元に戻して PC の再起動を案内）に倒している。
5. **BLE / BT のリセット**: 未実証のまま、再接続の経路にしている。再接続したかどうかは報告値の変化でしか分からないので、「まだ再接続していない」と「再接続したが反映されない」を区別できない。180 秒で見切って `AwaitingConfirm`（`apply_pending = Reconnect`）のまま返す。
6. **「唯一の入力手段」の判定**: helper 側の判定は粗い。VXE のマウスのキーボード用コレクションも「キーボード」に数えてしまう。そのため、キーボードをリセットし得る**すべての**要求で、呼び出し元の申告（`ApplyOptions::other_input_available`）を必須にした（C9）。CLI では `--other-input` か確認の質問で答えさせる。
7. **Explorer がフォルダーの ACL を書き換える可能性**: 保護フォルダーの ACL が変えられると、検証に失敗する。Users に読み取りを与えて、エクスプローラーの「続行」が出ないようにしている（G.1）。それでも失敗した階層は隔離して作り直す（D.9）。
8. **標準ユーザーのアカウント**: UAC で別の管理者の資格情報を入力すると、helper は別のユーザーとして動く。呼び出し元の確認と生死の判定は `OpenProcess` を使わない方式に改めた（S3。E.2）。HKCU の RunOnce は、非昇格の呼び出し元（正しいユーザー）が書くので問題ない。R14 で確かめる。
9. **Windows Update と設定アプリによる書き換え**: `Conflict` やドリフトの原因になる。ドリフトの検知は M4。`LayerDriver JPN` の大文字小文字の書き直しは衝突にしない（C16）。設定アプリが実際に書く値は M0 #1（Procmon による採取）を実施していないので未確認。
10. **「標準に従う」（0x51）**: 打鍵で確かめていないので、警告を付ける。
11. **`PendingReboot` 中は新しい `set` ができない**: 移行した後、再起動する前に Keychron も変えたい、という場合は先に再起動が必要になる。計画 2.2 どおりの制約だが、v1.x で「`PendingReboot` の操作に積み重ねる」ことを検討する。取り消したいだけなら `undo` がどの状態からでも効く（D.10）。
12. **`RevertedPendingReboot` は保守的**: 同じ起動の中で書いて戻した i8042prt の値は、ドライバーが一度も読んでいないので、実際には再起動は要らない。計画 2.3 に合わせて、再起動を求めている。ロールバックも同じ規則にした（C1。電源断の後の起動で読まれている場合があるため、区別しない）。
13. **`Written` の後に値が `before` に戻っていた場合を `Conflict` にしている**: 慎重すぎるかもしれない。実際に起きる頻度を見て見直す。
14. **同じレシーバーのほかの機能への影響**: Keychron の Col01 をリセットしたとき、同じレシーバーのマウス（Col03）が影響を受けるかは記録していない。R2 で観察する。
15. **ロックを持ち続ける時間**: カウントダウンの間（最大 25 秒）は、ほかの書き手（別セッションの GUI の回復など）が `Busy` になる。許容範囲と考える。
16. **ARM64**: コードは同じだが、実機での試験は M6。
17. **windows クレートのバインディング**: `NtQuerySystemInformation` の `SystemBootEnvironmentInformation`（90）と `SystemProcessIdInformation`（88）の構造体が見つからなければ、自前で定義する（A.3）。
18. **ProgramData の先回り作成の残るリスク**（S1）: 先回りして作ったフォルダーを、攻撃者が `FILE_SHARE_DELETE` なしで開いたままにすると、隔離の改名ができない。自動起動で開き続けられると、再起動しても止まったままになり得る。M2 では案内付きで止めるにとどめる。M5 ではインストーラーが昇格してフォルダーを作るので窓は狭まるが、インストールより前の先回りは残る。根本策の候補は、ロックをファイルではなく、境界記述子に Administrators を入れたプライベート名前空間のミューテックスにすること（標準ユーザーは同じ名前空間を作れない）。計画 2.2 の「`LockFileEx`」からの変更になるので、M5 の前に判断する。
    - 各階層を固定するハンドルはデータへのアクセス（`FILE_LIST_DIRECTORY`、`FILE_ADD_FILE` など）を求めない（`READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE`）。それらは共有モードの検査に加わり、Users はフォルダーを開けるので、ユーザーが開いたままにするだけで固定（したがって recover を含むすべての書き込み）を止められてしまうため。検証済みの階層の改名は、DACL が SY と BA にしか `DELETE` / `FILE_DELETE_CHILD` を与えないことで防ぐ。フォルダーのフラッシュ（C10）には、その時だけ `FILE_ADD_FILE` の短命なハンドルを開く。
    - **復旧用ファイルの置き換えを読み手が止める**: Users は `Recovery` とそのファイルを読めるので、`FILE_SHARE_DELETE` なしでファイルを開いたまま（またはフォルダーを書き込み共有なしで開いたまま）にすると、`MoveFileExW(REPLACE_EXISTING)` とフォルダーのフラッシュが失敗する。ウイルス対策やエクスプローラーのプレビューのような短時間の保持は、約 2 秒の再試行で吸収する。それでも失敗すれば、エラーにファイル名と「ほかのプログラムが開いている」旨を示し、ファイルは直前の完全な版のまま残る。起動時の値を変える操作は C10 どおり `RecoveryAssetsUnavailable` で止まり、HID だけの操作は警告で続く。根本策（読み手が止められない配置、たとえば世代ごとのファイル名と索引）は M5 の前に判断する。
19. **対話の「導入前に戻す」の回復は確定しない**（C14）: 前へ書き切るが、利用者が一度も確認していないので、`AwaitingConfirm` / `PendingReboot` で止める。利用者が気付かないと open のまま残り、`set` を止める。GUI と CLI の起動時の `attention` で必ず尋ねる。
20. **`undo` は衝突を残すことがある**: `Conflict` のエントリで、外部に変えられた値は `undo` でも書かない（CAS の原則）。その場合は `resolve` で決める。
21. **置き換えられた操作は開き直さない**: 「導入前に戻す」が `PendingReboot` の移行を置き換えた後、その「導入前に戻す」を取り消すと、値は移行の値に戻るが、移行のエントリは `Failed(Superseded)` のまま。起動時の値なので `RevertedPendingReboot` として再起動が必要と表示されるが、移行の確認画面は出ない。利用者が自分で戻した結果として受け入れる。
22. **ジャーナル自体に書けない場合**: J が書けないと、エントリは書き込み中のまま残る。回復も同じ理由で書けないが、値は変えないので害はない。起動のたびに UAC が出続けないよう、自動で回復を求めるのは 1 回の起動につき 1 回までにする（D.7）。

---

## J. 計画からの変更点

| # | 計画 | この設計 | 理由 |
|---|---|---|---|
| 1 | パイプの DACL に `(A;;GA;;;<userSID>)` を含める（2.2） | helper のパイプでは外し、`OWNER RIGHTS` を `READ_CONTROL` に限る。多重起動防止用のパイプは計画どおり | helper は BA の ACE で接続できる。所有者の暗黙の `WRITE_DAC` も消す。防御の本体は両側の PID の確認（E.2。S10） |
| 2 | 回復で「途中まで書かれていれば baseline に戻す」（2.3） | その操作の直前の値（`before`）に戻す。`baseline`（初めての変更の前の値）は「導入前に戻す」専用 | 操作が重なったとき、2 つ目の操作の途中で止まって baseline に戻すと、1 つ目の操作まで消えてしまう。操作が 1 つだけなら 2 つは同じ値になる |
| 3 | ロールフォワードは `AwaitingConfirm` へ（2.3） | 同じ起動で PC の再起動が必要なら `PendingReboot`。リセットの経路（USB）はロールフォワードせず元に戻す。INV-PS2 が壊れていれば進めない | 再起動の前に「確認待ち」にすると、反映されていない変更を確定させてしまう。USB の変更の安全網はカウントダウンなので、確認されなかったものは戻す（C.7。C12） |
| 4 | 状態の一覧（2.3）に再接続待ちがない | 再接続の経路は、`countdown` のない `AwaitingConfirm` で表す（新しい状態は作らない） | ユーザーが決める状態であり、回復のときの扱い（自動では何もしない）も同じだから |
| 5 | ProgramData のフォルダーは SY と BA だけ（2.2） | 検証の条件を「SY と BA 以外に書き込み系の権利を与えない」とする。`Recovery` は Users が読める。ロックファイルは SY と BA だけ | ユーザーが復旧用ファイルを USB メモリにコピーできるようにするため（2.3）。また、エクスプローラーによる ACL の書き換えを防ぐため（G.1、D.9） |
| 6 | helper は HKLM に記録したインストール先から起動する（2.2） | M2 では呼び出し元の exe と同じフォルダー。M5 で HKLM に切り替える | インストーラーは M5 のため（依頼どおり） |
| 7 | RunOnce には `mklm.exe --post-reboot` を登録する（3.6） | CLI から操作した場合は `mklm-cli post-reboot` を登録する。登録の条件はジャーナルで決める | GUI は M3 のため。結果を受け取れなかった場合も拾うため（C17） |
| 8 | —（取り消しの範囲は規定なし） | `revert` できるのは、その値を最後に変えた操作だけ。`Confirmed` の「導入前に戻す」は取り消せない | 古い操作を単独で戻すと、新しい操作の CAS の前提が崩れる。古い状態へは「導入前に戻す」で戻す。「導入前に戻す」の取り消しは baseline を失う（C6） |
| 9 | 移行は、全体と異なる配列を初めて割り当てたときに提案する（1.3） | 固定モードでの `set` は `MigrationRequired` で止め、`migrate --also` を案内する | 固定モードでは値を書いても効かない（M0 #2a）ので、提案の手段として止める |
| 10 | — | 要求に `expected`（`steps` と `apply`）を持たせ、helper の計画と違えば書かない | ユーザーが確認画面で見た内容（反映方法を含む）と、実際に書く内容を一致させるため（S6） |
| 11 | — | Conflict の解決で「現在の値を残す」を選んだ場合は `Failed(ConflictKeptCurrent)` で閉じる。INV-PS2 が壊れたままなら閉じない | 新しい状態を増やさずに、操作の意図が実現しなかったことを表すため（C12） |
| 12 | `boot_id`（カーネルの起動時刻）（2.3） | 0.1.0 は起動ごとの GUID（`SystemBootEnvironmentInformation.BootIdentifier`）。**0.1.1 から `KUSER_SHARED_DATA.BootId`（ローダーが起動のたびに増やすカウンター）**。起動時刻は履歴に残し、0.1.x の記録の判定にだけ使う | 起動時刻は時刻の補正でずれ、「再起動した」と誤判定すると、反映されていない移行を確定させてしまう（C2）。GUID は完全な再起動でも変わらない PC があった（C.10「起動 ID」、`docs/research/boot-id.md`） |
| 13 | 回復は「`boot_id` が違うか、所有者のプロセスがすでにない項目だけ」（2.3） | ロックを取れた時点で見つかった書き込み中のエントリは、所有者の生死によらず回復する。生死は非昇格の表示にだけ使う | 所有者は書き込み中ずっとロックを持つので、ロックが取れれば放棄されている。生死の判定に頼ると、エラーで返った同じ helper の回復が何もしない、生死が分からないと回復しない、といった行き止まりが生まれる（C3） |
| 14 | —（回復の対象の区別はない） | 「導入前に戻す」は回復で逆向きにせず、前へ書き切る。サイレントはロールバックしない | 利用者の「MKLM を外したい」という意図と逆の値を書き戻さないため（C14） |
| 15 | セーフモードでは、昇格したコンソールで直接実行する（2.1） | 昇格していても helper を別プロセスとして起動する（`CreateProcessW`、UAC なし）。同じプロセスで動かすのは隠しオプションの `--in-process` だけ | 同じプロセスだと、Ctrl+C やウィンドウを閉じる操作でカウントダウンの安全網が働かない（C8） |
| 16 | ProgramData の検証に問題があれば中止する（2.2） | 先回りして作られた階層は、中身を信用せずに隔離して作り直す。隔離できなければ中止する | 中止だけだと、ローカルユーザーが先にフォルダーを作るだけで、recover を含むすべての書き込みを永久に止められる（S1） |
| 17 | 復元やタグ変更の際は、helper が HKLM のプロファイルも同時に更新する（2.2） | 意図した配列はジャーナルの `Confirmed` の操作から求める。プロファイルを持つ場合も、`Confirmed` への遷移と取り消しのときだけ、同じロックの中で更新する | `Planned` の時点で書くと、カウントダウン切れで戻した後もプロファイルだけが新しい配列のまま残り、M4 のサービスが再適用を繰り返す（C.12。C15） |
| 18 | — | `undo`（open な変更をまとめて取り消す）と `resolve`（衝突を CLI で解決する）を足した | 利用者の非常口が、いちばん起きやすい失敗の状態で効かなかったため（C7。C11） |
| 19 | 「規定時間内に戻らなければ、値を元に戻してから PC の再起動を案内する」（1.4） | 計画どおりに加えて、リセットの呼び出し自体にも 20 秒の期限を付ける | `SetupDiCallClassInstaller` は期限を持たず、戻らないと helper がロックを持ったまま固まるため（C18） |

---

## K. 実装の分担

依存が少ない順に並べた。WP1〜WP3 は並行して進められる。型とトレイトはこのコミットで決まっているので、WP4 と WP5 も WP1〜WP3 の完成を待たずに始められる。

| WP | 範囲 | 主なファイル | 依存 | 完了の条件 |
|---|---|---|---|---|
| WP1 | core の規則 | `journal.rs`、`recovery.rs`、`restore.rs`（段階の順）、`operation.rs`（`plan_set_layout`、`plan_migration`）、`recovery_assets.rs`（固定の順序、`Select\Default`）、`report.rs` | なし | H.1 の core のテストが通る |
| WP2 | ipc | `args.rs`、`frame.rs`（`FrameHeader`）、`handshake.rs`（ビルド ID）、`message.rs` | なし | H.1 の ipc のテストが通る |
| WP3 | win の書き込み用モジュール | `regwrite`（名前の関所）、`journal_store`、`devctl`、`protected_dir`（隔離、耐久的な置き換え）、`session`（起動 ID。0.1.1 から `KUSER_SHARED_DATA.BootId`）、`proc_identity`（`NtQuerySystemInformation`）、`elevation`（`spawn_from_elevated`、`file_build_id`）、`pipe`（待ち直し）、feature の追加 | なし | 非昇格でできる H.1 の win のテストが通る。unsafe ブロックにはすべて `// SAFETY:` を書く |
| WP4a | engine の偽物 | `memory.rs`（`MemoryRegistry` の故障注入と `crash_images`、`deny_target`、`remove_devnode`、`FakeDevices::reboot` などの追加、`FakeHost` の単調時計と System32、`ScriptedSink::cancel_after`） | WP1 の型 | 偽物そのもののテスト（クラッシュ状態の列挙が正しいこと） |
| WP4b | engine の本体 | `engine.rs`、`error.rs` | WP1、WP4a | H.1 の engine のテスト。とくにクラッシュの網羅テスト（I1〜I7） |
| WP4c | engine の Windows 実装 | `win.rs`（リセットの別スレッドと期限、`join_pending`、`ReadIssueKind` による振り分け） | WP3 | 実機テスト R1 |
| WP5 | helper と CLI | `apps/mklm-helper/src/session.rs`（`dispatch`、書き込み専用スレッドと Heartbeat、`SetCurrentDirectoryW`）、両方の `build.rs`（ビルド ID）、`apps/mklm-cli/src/write.rs`（パイプの中継、helper の死の検出と再回復、RunOnce の規則、`post-reboot`、`undo`、`resolve`、`journal`、`--in-process` と Ctrl ハンドラー） | WP2、WP4 | H.1 の cli のテストと R1〜R16 |
| WP6 | 文書 | `docs/recovery.md`（G.5）、README の追記 | G.5 | レビュー |

**レビューで確かめること**

- HKLM に対する `KEY_SET_VALUE` は `regwrite` と `journal_store` にしか出てこない。HKCU の RunOnce への書き込みは `session` にしか出てこない。
- `regwrite::WritableKey::write` が値の名前を許可リストで確かめている。
- engine が書く値は、すべて `check_plan` か `plan_restore` を通っている。
- すべての遷移が C.5 の順序（復旧用ファイル → J → FJ → T → FT → J → FJ）どおりになっている。
- クラッシュ以外のエラーで、エンジンが書き込み中のエントリを残して返る経路がない（C3）。
- `OpenProcess` をプロセスの確認に使っていない（S3）。
- helper の依存関係にネットワークと UI のクレートが入っていない。
- パイプから `RestoreMode::Silent` を作る経路がない（S5）。

---

## レビュー対応

M2 設計の 1 回目のレビュー（2 つの観点）への対応。すべての指摘を採用した（一部は指摘の案と違う形で）。「節」は変更した箇所。

### 観点 1: クラッシュ時の整合性とユーザーの安全（C1〜C18）

| # | 指摘 | 採否 | 変更 / 理由 |
|---|---|---|---|
| C1 | 戻した値がまだ効いていないのに閉じた状態で終わり、必要な対処がその場限りの結果にしか残らない。再接続前の Keep で `Confirmed` になる | 採用（一部変更） | `JournalEntry::apply_pending`（`ApplyPending { action, instance_ids, since }`）を追加し、閉じるときに `apply_pending_on_close` で求めて永続化する。`Attention::NeedsApply`（書き込みを止めない）を追加し、起動の変化か Raw Input の一致で `apply_pending_cleared` が消す。ロールバックの終わりを `Failed` から `Reverted` / `RevertedPendingReboot`（`failure` 付き）に改め、起動時の値を含めば再起動を求める。再接続前の Keep は、確定を拒むと open なエントリがほかの操作を止め続けるので、**拒まずに** `Confirmed` と `apply_pending = Reconnect` にした。対話的な回復は `ApplyOptions` でリセットを提案する。I1 に報告値の条件を加えた（0.2、C.4、C.7、C.11、D.2、D.6、D.7、H.1） |
| C2 | `BootTime` は時刻の補正でずれ、再起動なしに `PendingReboot` から進んでしまう。`keep <op>` は Raw Input を見ずに確定する | 採用（一部変更） | `BootId` を `SystemBootEnvironmentInformation.BootIdentifier`（起動ごとの GUID）に変え、`BootTime - BootTimeBias` は `TransitionRecord::boot_time_hint` として診断にだけ残す。R9 と R10 を、スリープ、休止、高速スタートアップ、`w32tm /resync /force`、完全な再起動に広げ、高速スタートアップで変わる場合の代案（`KUSER_SHARED_DATA.BootId`）を決めた。`keep <op>` は、エンジンでは利用者が見たかを確かめられないので、CLI の `keep` が再起動で反映する操作に `post-reboot` と同じ確認画面を出す形にした（A.3、C.3、D.6、H.2、I.1、J.12）。**後日**: R9/R10 を行わないまま 0.1.0 を出荷し、GUID が完全な再起動でも変わらない PC で `PendingReboot` が終わらなくなった。0.1.1 で代案の `KUSER_SHARED_DATA.BootId` に切り替えた（下の「0.1.0 の不具合」、C.10「起動 ID」） |
| C3 | クラッシュ以外の恒常的な失敗で回復が無限に繰り返され、すべての操作が止まる。見た目だけの ReadIssue、同じ helper の再回復、生死不明でも止まる | 採用 | (a) ロックを取れた時点の書き込み中のエントリは、生死によらず回復する（`RecoveryContext` から生死を外し、`LeaveReason::OwnerAlive` をなくした）。(b) クラッシュ以外のエラーは 1 回やり直した後、`ValueRecord::write_error` に残して `Conflict` にする（原則 6）。(c) `BackendError::DeviceRemoved` と `SkipReason::DeviceRemoved` を追加し、「値なし」と区別する。(d) `ReadIssueKind` を追加し、書き込みを止めるのは devnode の特定、ID、ドライバー、present だけにした（M1 の呼び出し箇所を分類済み）。(e) `FaultPlan::deny_target` と I7 のテストを追加。自動の UAC は 1 回の起動につき 1 回までにした（A.3、B.1、B.4、C.5、C.7、D.1、D.7、H.1、I.22） |
| C4 | `plan_restore` の固定順序（全体 → その他 → i8042prt）では、キーボードごとモードへ向かう戻しが必ず拒否され、行き止まりになる | 採用 | 順序を変化の向きで決める 5 段階（`RestorePhase`: 固定を足す → ペアのある全体 → HID → ペアのない全体 → 固定を外す）にした。取り消しとロールバックでは結果として前向きの逆順になり、「導入前に戻す」と解決にも同じ規則が使える。復旧用ファイルは現在の値を知らないので、どこから始めても安全な固定の順（設定してから削除）にした。移行済みの状態からの `restore --baseline --all` をすべての `crash_after` で試すテストを追加（C.5、G.2、G.3、H.1、`restore.rs`、`recovery_assets.rs`） |
| C5 | 解決の途中でクラッシュすると、`ContinueRevert` が `KeepCurrent` の記録まで `before` に戻す。`Expect::Any` が利用者の新しい変更を上書きする | 採用 | `JournalEntry::revert_mode`（`Revert` / `Rollback` / `Resolution`）を `RevertPending` と一緒に永続化し、解決ではすべての記録に `resolve_to` を入れる（`KeepCurrent` は現在の値）。回復は `resolve_to` と違う記録だけを、利用者が見た値（`conflict`）を期待値にして書き、合わなければもう一度 `Conflict`。解決は `Expect::Any` を使わない（C.3、C.5、C.8、D.7、D.8、`RestoreTo::Resolution`） |
| C6 | `Confirmed` の「導入前に戻す」を取り消すと baseline が失われる | 採用（案 b） | `Confirmed` の「導入前に戻す」の `revert` を `InvalidState` にし、「もう一度 `set` してください」と案内する。案 a（baseline の記録を書き戻す）は、書き戻しと値の書き込みの間の新しいクラッシュ窓を増やすので採らなかった。`Confirmed` になる前の取り消しと undo はできる（C.6、D.4、H.1、J.8） |
| C7 | 文書の非常口（`recover`、`restore --baseline`）が、再起動後の失敗の状態で効かない | 採用 | `undo`（`Request::Undo`、`Engine::undo_open`、`mklm-cli undo`）を追加し、recovery.md と README の先頭に載せた。「導入前に戻す」は、書き込み中ではない open なエントリを `Failed(Superseded)` で閉じてから進む（`FailureReason::Superseded`。回復でも続きができるよう `Planned` を先に永続化する）。回復が何も戻さなかったときは undo を案内する（C.4、D.5、D.7、D.10、F.1、G.4、G.5） |
| C8 | 昇格したコンソールで同じプロセスで動かすと、Ctrl+C やウィンドウを閉じる操作でカウントダウンの安全網が働かない | 採用（範囲を広げた） | 昇格していても helper を別プロセスとして起動する（`elevation::spawn_from_elevated`。`CreateProcessW` は Appinfo に頼らないので、セーフモードでも同じ方式にした）。同じプロセスで動かすのは隠しオプション `--in-process` だけで、そのときは `SetConsoleCtrlHandler` で「今すぐ戻す」に対応させる。カウントダウン中に helper が死んだら、呼び出し元はすぐに新しい helper で `Recover` を送る（A.7、E.1、E.7、F.2、F.3、H.2 R16） |
| C9 | `Revert` と `RestoreBaseline` に「別の入力手段がある」の申告がないのに、キーボードをリセットする | 採用 | `ApplyOptions { allow_live_reset, other_input_available }` を `report` に置き、リセットし得るすべての要求（`SetLayout`、`Revert`、`RestoreBaseline`、`ResolveConflict`、`Recover`、`Undo`）に付けた。既定はリセットしない。CLI は共通の `ApplyArgs`（`--other-input`、`--no-reset`）で申告させる（D.4、D.8、E.6、F.1、`report.rs`、`message.rs`、`write.rs`） |
| C10 | 復旧用ファイルの置き換えは中身の耐久性を保証しない。書けなくても移行が続く | 採用 | 一時ファイルの `FlushFileBuffers` → `.prev` を残す → `MoveFileExW(WRITE_THROUGH)` → フォルダーの `FlushFileBuffers` の順にした。書く時点を `Planned` の前に移し、起動時の値を変える操作は書けなければ何も書かずに `RecoveryAssetsUnavailable` で止める。HID だけの操作は警告（B.3、C.5、D.3、D.5、G.1、`ErrorCode::RecoveryAssetsUnavailable`） |
| C11 | WinRE の手順: 空白を含むパスの引用、`Select\Current` の使用、オフラインで戻した後の衝突の説明がない | 採用 | `cd /d "…\Recovery"` の後に `restore-offline.cmd offline D:` を実行する形にし、`"` のキーの位置を添えた。スクリプトは `Select\Default` を使い、`Current` と違えば止める（ブロック内の括弧を避けてラベルで扱う）。オフラインで戻した後は「現在の値を残す」を選ぶよう書いた。M2 に CLI の解決手段がなかったので `mklm-cli resolve` を足した（F.1、G.3、G.5） |
| C12 | ロールフォワードと `KeepCurrent` が今の機器構成で INV-PS2 を確かめ直さない | 採用 | エンジンが今の値で `check_inv_ps2` を求めて `RecoveryContext::inv_ps2` として渡し、違反があればロールフォワードせずにロールバック（`RollBackReason::InvPs2`）か、固定値のない PS/2 の一覧付きの `Conflict` にする。`RebootObserved` も同じ。違反が残る解決は `PlanRejected` で拒否する（C.7、D.1、D.8、`OperationResult::inv_ps2_violation`） |
| C13 | 書き込み中の切断でも `Restarting` に進み、見ている人のいないキーボードを 2 回リセットする | 採用 | `EventSink::check_cancelled`（待たない）を追加し、`Restarting` の直前に確かめる。去っていればリセットせずにロールバックして `Reverted`（`CallerDisconnected`）（B.5、D.2 a、E.7） |
| C14 | 止まった「導入前に戻す」を回復が MKLM の値に戻し、利用者の意図と逆になる。サイレントでは再インストール後に戻ってしまう | 採用（一部変更） | `RecoveryDecision::CompleteForward` を追加し、「導入前に戻す」は前へ書き切る。サイレントは `Confirmed` まで進めて後始末する。対話は、利用者が確認していないので確定はせず、`AwaitingConfirm` / `PendingReboot` で止める（カウントダウン中に止まったものも、カウントダウンを外した `AwaitingConfirm`）。その会話中のカウントダウン切れでは、これまでどおり戻す（計画 1.4 の安全網）（C.7、D.5、I.19） |
| C15 | M4 のサービスとプロファイルの更新の時点が決まっておらず、戻すと再適用するを行き来する | 採用 | C.12 に規則を書いた: 意図した配列はジャーナルの `Confirmed` の操作から求める。プロファイルを持つ場合も、`Confirmed` と取り消しのときだけ、同じロックの中で更新する。サービスは open なエントリがあるデバイスと、MKLM 自身のリセットによる到着を再適用のきっかけにしない（C.12、J.17） |
| C16 | `LayerDriver JPN` などの REG_SZ を大文字小文字を区別して比べ、書き直しを衝突にする | 採用 | `journal::value_eq(name, a, b)` を追加し、観測、CAS、「before と intended が同じなら記録しない」の 3 か所で使う。区別しないのはこの 2 つの名前だけ（C.7、C.8、D.2、H.1） |
| C17 | RunOnce を登録するのが `PendingReboot` の結果を受け取ったときだけで、回復のロールフォワードや切断で漏れる | 採用 | 登録の条件をジャーナルで決める（`Journal::needs_post_reboot_check`）。helper のセッションが終わるたび（エラーでも）、`recover` の後、書き込みコマンドの開始時に確かめる（F.2、F.4、J.7） |
| C18 | `restart()` に期限がなく、戻らないとロックを持ったまま固まる。カウントダウンを呼び出し回数だけで数えている | 採用 | Windows 実装は `restart_device` を別スレッドで 20 秒の期限付きで呼び、期限切れは `RestartOutcome::TimedOut`（`NeedsReboot` と同じ扱い）。helper は終了前に `join_pending` でそのスレッドを待つ。カウントダウンは `Host::monotonic` でも上限を付ける。`Restarting` の回復は devnode の状態を見て `apply_pending` に残す（B.2、B.3、D.2 a、`EngineConfig::restart_timeout`、`countdown_slack`、`FakeReset::Hangs`） |

### 観点 2: API / 統合の健全性とセキュリティ（S1〜S13）

| # | 指摘 | 採否 | 変更 / 理由 |
|---|---|---|---|
| S1 | ProgramData を先回りして作られると、recover を含むすべての書き込みが永久に止まる | 採用 | 検証に失敗した階層（ロックファイルを含む）は、中身を読まずに `<名前>.untrusted-<uuid>` に改名して隔離し、保護 SDDL で作り直して警告する（`ProtectedDir::quarantined`、`Host::drain_warnings`）。改名できない場合（開いたままにされている）は案内付きで止め、残るリスクと根本策の候補（プライベート名前空間のミューテックス）を I.18 に書いた。R15 を追加（A.3、D.9、I.18、J.16、`protected_dir.rs`） |
| S2 | ReadIssue が 1 件でもあると全部止まり、`RegValue::Other` の経路にも回復にも届かない | 採用 | `ReadIssueKind` を追加し、M1 の全呼び出し箇所を分類した。書き込みを止めるのは `Locate`、`Identity`、`Driver`、`Presence` だけ（`blocks_writes`）。`Status` と `Container` はそのキーボードのリセットを禁止する形で続け、`Values` はエンジンの読み直しに任せ、ほかは警告にする。`DeviceController::keyboards` は `Inventory { keyboards, warnings }` を返す（A.3、B.2、D.1、F.2、H.1、`error.rs`、`devices.rs` ほか） |
| S3 | 別の管理者の資格情報で昇格すると、helper の `OpenProcess` が必ず失敗する | 採用 | `proc_identity` は `OpenProcess` を使わず、`NtQuerySystemInformation` の `SystemProcessIdInformation`（NT 形式のパス）と `SystemProcessInformation`（PID と CreateTime）を使う。パスは NT 形式のまま比べる（`process_image_nt_path`、`same_image_directory`）。`SeDebugPrivilege` は使わない。生死が `Unknown` なら回復を試す側に倒す。R14 を追加（A.3、C.7、E.2、I.8） |
| S4 | `EventSink` では `Planned` の前の切断を検出できず、Ctrl+C の後でも移行を書き切る。`NullSink` がすぐに返すのでカウントダウンが 0 秒になる | 採用 | `EventSink::check_cancelled` を追加し、`Planned` の直前（何も書かずに `Cancelled`）と `Restarting` の直前（リセットせずにロールバック）で確かめる。`wait_decision` は「期限まで戻らない」を契約にし、`NullSink` は眠る。`ScriptedSink::cancel_after` を追加（B.4、B.5、C.5、E.7、`ErrorCode::Cancelled`） |
| S5 | エンジンの公開 API がパイプの型そのもので、`silent` がパイプから届く | 採用 | 共有の型を `mklm_core::report` に移し、engine の ipc への依存をなくした。エンジンは `params`（`SetLayoutParams`、`RestoreBaselineParams { mode }` など）を受け取り、`execute(&Request)` をなくした。ipc はパイプで許す要求だけを定義し、`RestoreBaselineRequest` から `silent` を消した。helper の `dispatch` が明示的に写す。`RestoreMode::Silent` は `--uninstall-restore` からだけ作る（A.1、A.4〜A.6、E.4、E.6） |
| S6 | `expected_steps` が反映方法を比べないので、承認したリセットが再起動待ちに変わる。計画の作り方が 3 か所に分かれる | 採用 | `ExpectedPlan { steps, apply }` にして両方を比べる。`operation::plan_set_layout` / `plan_migration`（`OperationPlan`）を下見、GUI、エンジンで共通に使う（A.2、D.1、D.2、D.3、F.2、`EngineError::PlanChanged { plan }`） |
| S7 | Heartbeat をエンジンの呼び出しの中でしか送れず、`restart()` などが長く止まると呼び出し元が切断する | 採用 | helper に書き込み専用のスレッドを置き、要求の実行中は 10 秒ごとに `Event::Heartbeat` を送る（`HEARTBEAT_INTERVAL`）。呼び出し元は helper のプロセスが終了したら期限を待たずに失敗とし、30 秒の沈黙は helper のプロセスが固まったときだけにした（A.6、E.1、E.6、E.7、`session.rs`） |
| S8 | 計画 1.5 の「`LayerDriver JPN` の DLL が System32 にある」確認が設計にない | 採用 | `Host::system32_file_exists` と `OperationError::LayerDriverMissing` を追加し、エンジンが `check_plan` の前に確かめる。`FakeHost::remove_system32_file` でテストする。baseline を戻すときは確かめない（導入前の値なので）（B.3、D.3、H.1） |
| S9 | SYSTEM ハイブへの唯一の関所 `regwrite` が値の名前を確かめない | 採用 | `mklm_core::DEVICE_VALUE_NAMES` / `GLOBAL_VALUE_NAMES` を追加し、`WritableKey::write` がそれ以外を `Error::ValueNotAllowed` で拒否する。`RegistryBackend` の両実装も `BackendError::NameNotAllowed` で同じ規則を守る（A.3、B.1、K） |
| S10 | パイプの所有者の暗黙の `WRITE_DAC` で、同じユーザーのプロセスが唯一のインスタンスに接続し、セッションを失敗させられる | 採用 | SDDL に `(A;;RC;;;OW)` を加えた。PID の違う相手は `DisconnectNamedPipe` で切って待ち直し、helper は `ERROR_PIPE_BUSY` を `WaitNamedPipeW` で再試行する。E.2 は「防御の本体は PID の確認」と書き直した（A.3、E.2、`pipe.rs`） |
| S11 | 版の違う相手の本文が JSON のエラーになる。古い helper が残っても検出できない。版の確認が UAC の後 | 採用 | `FrameHeader { v, seq }` を先に解析して版を確かめる。ビルド ID（パッケージの版と共有クレートのソースのハッシュ）を両方の exe に埋め込み、`Hello` / `Welcome` で完全一致を求める。呼び出し元は起動前に helper の VERSIONINFO からビルド ID を読み、違えば UAC を出さずに止める（`elevation::file_build_id`）。`build.rs` の実装は WP5（A.5、E.3、`frame.rs`、`handshake.rs`、`message.rs`） |
| S12 | `--yes` で `--other-input` を付け忘れると再起動待ちになる。`#n` が別のキーボードを指し得る | 採用 | `set` と `restore` の `--yes` は `--other-input` か `--no-reset` を必須にした（clap の `requires = "reset_choice"`。テスト済み）。`--yes` と `#n` の組み合わせは拒否し、インスタンス ID を求める（F.1、`write.rs`、`main.rs` のテスト） |
| S13 | USB にコピーした `restore-offline.cmd` は書き換えられ、WinRE では SYSTEM 権限で実行される | 採用 | recovery.md と README.txt に「実行するのはディスク上の版だけ、USB のコピーは読むための控え」と書き、スクリプトの冒頭のコメントにも入れた（G.3、G.4、G.5） |

### 観点 2 の総評で挙がった小さな不整合

| 指摘 | 採否 | 変更 / 理由 |
|---|---|---|
| A.7 では CLI が engine と ipc に依存するとしているが、`Cargo.toml` に入っていない | 採用 | `apps/mklm-cli/Cargo.toml` に `mklm-ipc` と `mklm-engine` を足した |
| K 章の「`KEY_SET_VALUE` は `regwrite` と `journal_store` だけ」が、`session::register_post_reboot`（HKCU の RunOnce）と矛盾する | 採用 | 「HKLM への `KEY_SET_VALUE` は `regwrite` と `journal_store` だけ、HKCU の RunOnce は `session` だけ」に書き直した（A.3、K） |
| 昇格したまま同じプロセスで動く CLI で、`restrict_dll_search` の失敗が警告だけ | 採用 | `--in-process` のときは致命的なエラーにする（A.7、F.2） |
| helper の現在のフォルダーが固定されていない | 採用 | helper は起動直後に `SetCurrentDirectoryW(System32)` を呼び、起動側も `lpDirectory` / `CreateProcessW` の現在のフォルダーに System32 を渡す（A.6、E.1、`elevation.rs`、`session.rs`） |

### 0.1.0 の不具合（公開後に見つかったもの）

| # | 不具合 | 対応 | 変更 / 理由 |
|---|---|---|---|
| B1 | デスクトップ PC（build 26200、UEFI、高速スタートアップ有効）で、`SystemBootEnvironmentInformation.BootIdentifier` が完全な再起動を 3 回しても変わらなかった。固定モードから移行して再起動しても `PendingReboot` のままで、確認画面の「このままにする」が押せず、helper は確定（`InvalidState`）も新しい変更（`OpInProgress`）も断った。取り消した移行の「再起動が必要」も消えなかった（2026-09-29 報告。`docs/research/boot-id.md`） | 0.1.1 で修正 | 起動 ID を `KUSER_SHARED_DATA.BootId` のカウンター形式にした（C2 の代案）。版は上げない。0.1.x が記録した GUID の起動 ID は、ジャーナルを読むとき（エンジンの `open()` と `mklm_client::journal::read_journal`）に、その GUID の下の履歴行の起動時刻（`boot_time_hint`）と今の `BootTime - BootTimeBias` を比べてメモリの上で判定する。利用者向けの文言、IPC、スキーマは変えない。R9/R10 を MT-1〜MT-9 に書き直した（A.2、A.3、B.3、B.4、C.3、C.7、C.10、D.1、D.7、H.1、H.2、I.1、J.12） |
