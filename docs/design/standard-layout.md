# 設計: キーボードごとモードでの PC の標準配列の変更、リモート デスクトップの案内、再起動の後の表示

| 項目 | 内容 |
|---|---|
| 対象 | (1) キーボードごとモードのまま PC の標準配列を変える新しい操作 `SetStandard`（B 章）。(2) リモート デスクトップの正確な案内（C 章）。(3) 表示の修正 3 つと CI の Pester の修正（D 章）。(4) 2026-09-29 の実機の結果の記録先（E 章） |
| 根拠 | `docs/design/m2-engine.md`（C.3〜C.11、D.1〜D.10、E.6、F）、`m3-gui.md`（B.2〜B.14、B.17、D）、`m5b-updater.md`（D.3、D.9.3、G.6）、`docs/research/rdp-keyboard.md`、`boot-id.md`、`m0-results.md`、2026-09-29 の実機の結果（0.1 節） |
| 状態 | 設計（未レビュー）。コードは書いていない。土台は `m5b/updater` の 167ca01（v0.2.0 の候補） |
| 読み手 | 実装する人と、レビューする人（G 章に作業の分け方） |
| 関連 | 接続元の MKLM と接続先の MKLM の連携（リンク）は別の設計（ブランチ `design/rdp-link`）で扱う。この設計はそれに頼らず、それと矛盾しないようにする（B.14） |

識別子（型、関数、値の名前）は英語のまま書く。「m2 D.3」は m2-engine.md の節、「m3 B.9」は m3-gui.md の節、「m5b D.9.3」は m5b-updater.md の節を指す。画面の文言の決まり（［］と「」、「確定」を使わない、日本語の画面に出してよいラテン文字の語）は m3 D.5 に従う。

---

## 0. 前提

### 0.1 2026-09-29 の実機の事実（接続先の PC `DESKTOP-3TCSIET`）

時刻は日本時間。ジャーナルの時刻と起動は、この設計を書くときに読み取りだけで確かめた（インストール済みの `mklm-cli journal --json`。`boot_time_hint` は起動 7 = 11:37:07.5、8 = 16:29:33.5、9 = 23:12:58.5 を指す）。

| 時刻 | できごと | 出どころ |
|---|---|---|
| 14:20 | セッション 1（11:37:20 にコンソールで作ったセッションに RDP で再接続したもの）で、標準配列 US、JIS の接続元（7/2 を報告）から打つと US（`@`、`[`） | 利用者の報告 |
| 16:18 | 統合ビルド 0.1.0+8994675d を、起動 ID の修正版の上に NSIS で上書き（M5b の NSIS の上書きを初めて実機で行った）。問題なし。RDP のキーボードの行は「リモート デスクトップ」 | 利用者の報告 |
| 16:21:01〜16:21:04 | GUI で `d724c149`（標準 US への移行、14:09 に Keep 済み）を元に戻す: `confirmed` → `revert-pending` → `reverted-pending-reboot`（起動 7） | ジャーナル |
| 16:29:33 | 再起動 1。KUSER の `BootId` 7 → 8、イベント 27 は 0x0。取り消しが効き、固定モード（英語、7/0、kbd101）に戻った | 利用者の報告、ジャーナルのヒント |
| 16:29〜16:35 | その間 `mklm-cli journal` は `d724c149` を「(reverted; the PC must restart)」と表示し続けた（次に helper がセッションを開くまで） | 利用者の報告 |
| 16:35:41 | **昇格した PowerShell から** `mklm-cli migrate --standard jis --also <USB Keyboard 04D9:1818>=us --also <Keychron Receiver 3434:D027>=us` → `271b6909`（`planned` 16:35:41.186、`pending-reboot` .229、起動 8）。同じ helper のセッションが `d724c149` を `reverted`（`reboot-observed`、.184）にした。PS/2 は 4/0 で固定、全体の Type/Subtype を削除、`LayerDriver JPN` = kbd106.dll、`PCAT_106KEY`。昇格した CLI なので RunOnce を登録せず、「may belong to another account」と表示した | ジャーナル、利用者の報告（ワークフローの説明では 22:5x とされていたが、ジャーナルは 16:35:41） |
| 23:12:58 | 再起動 2。8 → 9、0x0 | 利用者の報告、ジャーナルのヒント |
| 23:14:06〜23:14:14 | コンソールではなく **RDP で新しくサインイン**（セッション 2）。MKLM を手で開き、23:14:13 に「このままにする」→ `recover:reboot-observed`（23:14:14.163）→ `confirmed`（`keep`、.166）。`Conflict` にはならなかった。ただし USB Keyboard と Keychron について「Raw Input does not report the stored type yet; reconnect the keyboard」の警告が出た | ジャーナル、利用者の報告 |
| 23:1x | セッション 2（RDP で新しく作ったセッション）で、標準配列 JIS、JIS の接続元から打つと JIS | 利用者の報告 |
| — | RDP で作ったセッションの Raw Input には、この PC の物理キーボードが並ばない。そのセッションの `mklm-cli list` は、どのキーボードも Reported が `unknown` | 利用者の報告。この設計を書いたセッション（セッション 2 の RDP）でも同じ |
| — | 接続元の PC には JIS と US のキーボードがあり、接続元の US キーボードで打っても RDP では JIS で打たれる（リモートのキーボードは 1 つ、キー配列も 1 つ） | 利用者の報告 |

今（この設計を書いた時点。`mklm-cli status --json --all` を読み取りだけで確かめた）の接続先: キーボードごとモード、標準配列 JIS（`kbd106.dll`、`PCAT_106KEY`）。PS/2（`ACPI\PNP0303\0`）4/0、USB Keyboard（04D9:1818）4/0、Keychron Receiver（3434:D027）4/0。2.4G Wireless Device（1D57:FA60。`MI_00` と `MI_03` の 2 つのコレクションが同じコンテナー `{15A651F5-…}` で、メイン画面では 1 行）と VXE Mouse 1K Dongle（3554:F58E）は値がなく、標準（JIS）に従う。RDP のキーボードは `TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION2KEYBOARD0`。未接続のキーボードはない。接続元の報告は 7/2。再起動を待つ操作はない。

**今の手順の問題**: 標準配列を US から JIS に変えるのに、利用者は `d724c149` を取り消して固定モードに戻し（再起動 1）、移行をやり直して（再起動 2）、US のキーボードを 1 台ずつ `--also` で指定した。キーボードごとモードの PC で標準配列を変える操作がないため（`migrate` は「already in per-keyboard mode」で断り、`global` には `status` しかない）。

### 0.2 目的

1. キーボードごとモードのまま、PC の標準配列（`LayerDriver JPN` と `OverrideKeyboardIdentifier`）を 1 回の操作と 1 回の再起動で変えられるようにする。そのとき、**利用者が選ばない限り、どのキーボードの配列も変わらない**ようにする（標準に従っているキーボードに、今の配列を先に割り当てる）。
2. リモート デスクトップについて、確かめたことだけを、GUI、CLI、README、インストールの案内で正確に伝える。
3. 再起動の後の表示、リモート デスクトップから見えないキーボード、昇格した CLI の案内を直す。CI の Pester の段が GitHub の `powershell` の包みで失敗する原因を直す。
4. きょうの実機の結果を研究メモに記録する場所と中身を決める。

### 0.3 範囲外

- 固定モードの PC で標準配列を変えること。固定モードでは標準配列がすべてのキーボードの配列なので、今までどおり `migrate --standard`（キーボードごとモードへの移行）で扱う。
- 割り当て済みのキーボード（値を持つもの）の配列を、同じ操作で変えること（`--also` に当たるもの）。`set` で行う（J.2）。
- リモート デスクトップのセッションのキー配列を MKLM が直接決めること。`Terminal Server\KeyboardType Mapping`、`Layout File`、RDP のキーボードの devnode への書き込みは、今までどおり行わない（rdp-keyboard.md 10 節）。
- 接続元の複数のキーボードをリモート デスクトップで区別すること（`design/rdp-link`）。

### 0.4 用語

| 用語 | 意味 |
|---|---|
| 標準配列 | `i8042prt\Parameters` の `LayerDriver JPN`（kbd106.dll = JIS、kbd101.dll = US）と、対になる `OverrideKeyboardIdentifier`（`PCAT_106KEY` / `PCAT_101KEY`）。MKLM は 2 つを常にそろえて書く（`check_global_writes`） |
| 標準に従うデバイスノード（follower） | kbdhid のデバイスノードで、読み取り専用ではなく、保存値から予想される種類が自分の表を持たないもの（`predicted_type(global).per_keyboard_table()` が `None`: 0x51/0、7/0、そのほかの知らない種類）。接続中かどうかは問わない |
| 物理キーボード | `group_keyboards` のグループ（メイン画面の 1 行）。外付けは ContainerId ごと、内蔵と ContainerId のないものは 1 台ずつ |
| 固定する（pin） | follower に、今の標準配列と同じ配列の値（US なら 4/0、JIS なら 7/2）を書くこと。**画面では「固定」と書かない**（「固定モード」と紛れるため）。「今の配列のままにする」「US を割り当てる」と書く。CLI も「assign」 |
| 新しい標準に従う（follow） | 利用者が選んだ物理キーボードの follower には何も書かず、再起動の後は新しい標準配列で打たせること |
| RDP で作ったセッション | リモート デスクトップで新しくサインインして作ったセッション（例: 23:14 のセッション 2） |
| コンソールで作ったセッションへの再接続 | コンソールでサインインして作ったセッションに、後から RDP でつないだもの（例: 14:20 のセッション 1） |
| 見えないキーボード | 接続中（`present`）なのに、そのセッションの Raw Input に並ばないキーボード。RDP で作ったセッションでは、この PC の物理キーボードがすべてこうなった |

### 0.5 原則

1. **既存の規則を使い回す。** 書き込みの検査は `check_plan`（許可リスト、INV-PS2、順序）、戻しは `plan_restore`、回復は `decide_recovery`、ジャーナルは C.5 の順序。新しい操作だけの規則は、計画を作る関数（core）に閉じ込め、下見とエンジンで同じ関数を使う（m2 S6）。
2. **利用者が選ばない限り、打っている配列は変わらない。** 既定では、標準に従うキーボードをすべて今の配列に固定する。書く順序（固定 → 全体）で、途中で止まっても成り立つ（B.6）。
3. **PS/2 は常に固定されている**（INV-PS2）。この操作は PS/2 の値を書かない。固定値のない PS/2 があれば、今までどおり `check_plan` が拒否する。
4. **古いビルドを壊さない。** 0.1.x と 0.2.0 がこの操作のエントリを読めるようにする。ダウングレードの後も、書き込みも自動更新も止まらない（B.8）。
5. **確かめていないことを書かない。** リモート デスクトップのキー配列がこの PC の標準配列と接続元の報告のどちらで決まるかは、まだ分からない。画面にも文書にも、どちらかだとは書かない（C 章）。

---

## A. 今のコードと、変える場所（地図）

| 層 | 今あるもの | この設計で足す・変えるもの |
|---|---|---|
| 計画（core `operation.rs`） | `plan_set_layout` / `set_layout_writes`（固定モードで `MigrationRequired`）、`plan_migration` / `migration_writes`（キーボードごとモードで `NotFixedMode`。全体のペアを削除し、`standard_layout_writes` のうち違う値だけ書く）、`physical_device_members`、`apply_method`、`OperationError` | `follows_standard`、`set_standard_writes`、`plan_set_standard`（B.4）。`OperationError::{UnknownStandard, NotFollowingStandard}` |
| 許可リストと INV-PS2（core `allowlist.rs`、`safety.rs`） | `check_plan`（i8042prt → その他 → 全体の順に並べ、各ステップの後に `check_inv_ps2`）、`check_device_writes`（kbdhid は 4/0 か 7/2、読み取り専用と RDP を拒否）、`check_global_writes`（`LayerDriver JPN` と識別子はそろっていること。キーボードごとモードでも標準配列だけを書ける）、非公開の `explicit_standard`、`ps2_pin_layout`、`standard_layout_writes`、`device_layout_writes` | `explicit_standard` を `stored_standard` として公開する。許可リストそのものは変えない（書く名前と値は既存の 4 + 4 のうちのもの） |
| 配列の判定（core `layout.rs`、`device.rs`、`assess.rs`、`group.rs`） | `KeyboardType::per_keyboard_table`、`effective_layout`、`predict_type`、`assess`（`current` と `after_restart` はどちらも保存されている全体の値で求める）、`group_keyboards` | 変えない。follower の判定はこれらで書く |
| ジャーナル（core `journal.rs`） | `OpKind::{SetLayout, Migrate, RestoreBaseline, Cleanup}`（`#[serde(tag = "kind", rename_all = "kebab-case")]`）、`OpKind::schema_version`（`Cleanup` だけ 2、ほかは 1）、`JOURNAL_SCHEMA_VERSION = 2`、`from_json` / `to_json`、`Journal::parse`（読めないエントリは `unreadable` → 書き込みを止める） | `OpKind::SetStandard`。JSON では `"kind": "migrate"` に印 `set_standard` を足した形で書く（版 1 のまま。B.8） |
| 回復（core `recovery.rs`、`restore.rs`） | `entry_kind`（`SetLayout` / `Migrate` / `Cleanup` は `Change`）、`decide`、`attention`、`apply_pending_on_close`、`plan_restore`（段階: 固定を足す → ペアのある全体 → その他 → ペアのない全体 → 固定を外す） | `SetStandard` は `Change`。規則は変えない |
| 共有の型（core `report.rs`） | `OperationResult`、`ExpectedPlan`、`ExpectedKeyboard { changes }`、`ErrorCode` | 変えない（D.2 は既存の型で足りる。J.6） |
| エンジン（engine `engine.rs`、`params.rs`、`error.rs`） | `Engine::migrate`（D.3）、`confirm`（D.6。Raw Input が保存値どおりでないと「reconnect the keyboard」の警告）、`revert`、`recover`、`undo_open`、`create` / `write_new` / `apply_change`、`expected_keyboards`、`STANDARD_UNVERIFIED` | `Engine::set_standard`、`SetStandardParams`、`OperationError` の写し方。`confirm` の警告を見えないキーボードと再起動の後で分ける（D.2） |
| パイプ（ipc `message.rs`、`lib.rs`） | `Request::{SetLayout, Migrate, …}`、`MigrateRequest`、`PROTOCOL_VERSION = 3` | `Request::SetStandard(SetStandardRequest)`、`PROTOCOL_VERSION = 4` |
| helper（`apps/mklm-helper/src/session.rs`） | `dispatch` が要求を 1 つずつエンジンに写す | `Request::SetStandard` → `engine.set_standard` |
| 共有クライアント（`crates/mklm-client`） | `gate`（`Gate::NewOp`、`post_reboot_entries`、`restart_reasons`、`run_once`）、`preview::check_rows`（`Migrate` ならすべての接続中の kbdhid と i8042prt を並べる）、`session::plans_first`、`describe`、`startup::summarize`、`run_once::apply_run_once_rule`（昇格していれば `TellUser`） | `plans_first` と `check_rows` に `SetStandard`。`preview::standard_rows`（下見の表）。`describe::shown_state`（D.1）。`CheckRow::present`（D.2） |
| CLI（`apps/mklm-cli`） | `migrate`（キーボードごとモードでは「Nothing to migrate」）、`global status`（読むだけ）、`keep`、`post-reboot`、`journal`（`render::state_text`、`entry_line`、`journal_view::entry_text`）、`list` / `status`（`text.rs` の `reported_text` は `unknown`）、`show_post_reboot_rule`（昇格時の案内）、`REMOTE_SESSION_NOTE` | 新しいコマンド `mklm-cli standard`（B.11）。状態の表示を判定した状態に（D.1）。見えないキーボードの表示（D.2）。昇格時の案内（D.3）。`migrate` の案内 |
| GUI（`apps/mklm`） | `Page`（13 ページ）、`ChangeDraft`（固定モードの移行では `standard` を選ばせる）、`vm::status`（状態行の「標準配列」は表示だけ）、設定ページ、入力方式の案内（`other.slint` の RDP の段落）、再起動、再起動後の確認（`vm::post_reboot`、`Recognition::NotConnected` =「未接続」）、履歴（`i18n::entry_state`）、結果（警告は「技術的な詳細」へ） | `Page::Standard` と `StandardDraft`、状態行と設定ページの［変更…］、`vm::standard`、`i18n::standard`、再起動と確認と履歴の文、見えないキーボードの文（D.2）、判定した状態（D.1）、RDP の案内（C 章） |
| CI（`.github/workflows/ci.yml`、`installer/tests`） | 最後の段（`shell: powershell`）で `check-nsi.ps1` と `Invoke-Pester` | D.4 のとおり直す |

---

## B. 新しい操作: 標準配列の変更（`SetStandard`）

### B.1 利用者から見た動き

- キーボードごとモードの PC で、GUI の状態行か設定ページの［変更…］、または `mklm-cli standard <jis|us>` から始める。
- 下見は、物理キーボードごとに「今の設定」と「再起動の後」を示す。標準に従うキーボードは、既定で「今の配列のままにする」（固定する）。利用者は 1 台ずつ「新しい標準に従う」に変えられる。PS/2 と割り当て済みのキーボードは「変わりません」。RDP のキーボードは「変更できません」と C 章の説明。
- UAC の後、helper が固定の値と全体の値を書き、`PendingReboot` になる。PC の再起動で効く（全体の値は、i8042prt が起動時に、user32 と IME がサインイン時に読む。`global_change_action` の注記。MKLM は常に PC の再起動を求める。再起動の前の新しいサインインは H.3）。
- 再起動の後は、移行と同じ再起動後の確認（m2 D.7、m3 B.9）で「このままにする / 元に戻す」を選ぶ。元に戻すと、もう一度の再起動で前の標準配列に戻り、固定の値も消える。

### B.2 書く値と順序

- **デバイスの値**: 固定する follower ごとに、kbdhid の組 `KeyboardTypeOverride` / `KeyboardSubtypeOverride` を、変更前の標準配列の値（US → 4/0、JIS → 7/2。`device_layout_writes(Kbdhid, Some(from))`）にする。1 つのデバイスノードが 1 ステップ。
- **全体の値**: `standard_layout_writes(to)` のうち、今の値と違うもの（`value_eq`。大文字小文字の違いは変更にしない）。キーボードごとモードなので、全体の `OverrideKeyboardType/Subtype` はなく、書かない。
- **順序**: `check_plan` が i8042prt（この操作にはない）→ その他（固定）→ 全体の順に並べる。すべての固定をフラッシュしてから全体を書くので、全体の値が新しくなった状態では、固定はすべて書かれている（B.6）。
- **反映**: 全体の値を書くので常に `PendingAction::RestartPc`（`apply_method` の `writes_global`）。固定の値も、再起動で読まれる（その前でも、抜き差しすれば読まれるが、配列は同じ）。
- **変更がない場合**: `to` が今の標準配列と同じなら、何も書かない（下見は「変更はありません」、エンジンは `NoChange`）。

### B.3 固定するキーボードの決め方

| デバイスノード | 扱い |
|---|---|
| kbdhid、読み取り専用でない、follower（0x51/0、7/0 など） | 既定で固定する（接続中でなくても。B.3 の 3） |
| kbdhid、読み取り専用でない、自分の表を持つ種類（4/0、7/2、NEC） | 何もしない（「変わりません」） |
| i8042prt | 何もしない。INV-PS2 で固定されているはず。固定値がなければ `check_plan` が `PlanError::InvPs2` で拒否し、一覧を返す。前面の案内は「先にそのキーボードの［変更…］で配列を割り当ててください」（`set` で直る。m2 D.1 の表） |
| RDP のキーボード（`is_remote_desktop`）、仮想、ほかのドライバー | 書かない（`check_device_writes` が拒む）。下見に「変更できません」として出す |

1. **グループ**: 利用者が選ぶ単位は物理キーボード（`group_keyboards`）。`--follow` や GUI のチェックで選ばれた物理キーボードの follower（`physical_device_members` で展開する。内蔵と ContainerId のないものはそのキーボードだけ）には何も書かない。選ばれた物理キーボードに follower が 1 つもなければ `OperationError::NotFollowingStandard { instance_id }`。
2. **混在**: 同じグループに follower と割り当て済みのコレクションが混ざっていれば、follower だけを固定する（割り当て済みのものは触らない）。下見の行は「一部のコレクションだけ標準に従っています」と添える。
3. **接続していないキーボード**: follower なら既定で固定する。次に接続したときも今の配列のままにするため（原則 2）。シリアル番号のない USB キーボードを別のポートに差すと別のデバイスノードになり、元のものは phantom として残る（M0 #3b の逆の場合）ので、これも固定しておくと、元のポートに戻したときに配列が変わらない。GUI では「未接続のキーボード（n 台）」としてまとめ、同じチェックで選べる。phantom への書き込みは m2 I.3 の未検証の点を含む（H 章）。
4. **非表示のキーボード**（`settings.keyboards.hidden`）も同じく固定する。下見には「非表示」の印付きで出す（書く先を隠さない）。
5. **7/0 など MKLM が書かない種類を持つ follower**: 固定で 4/0 か 7/2 に置き換える（baseline に元の値が残る）。下見の技術的な詳細に「7/0 → 4/0」と出す。
6. **片方の値だけのもの**（`IncompletePair`）: 予想される種類で判定する（例: Type = 7 だけなら 7/0 で follower、Type = 4 だけなら 4/0 で割り当て済み）。固定は 2 つの値を書く。

### B.4 core の API と拒否

`crates/mklm-core/src/allowlist.rs`:

```rust
/// The standard layout as stored: an allowlisted `LayerDriver JPN` with its matching
/// `OverrideKeyboardIdentifier` (today's private `explicit_standard`). Never inferred from
/// missing values.
pub fn stored_standard(global: &GlobalSettings) -> Option<Layout>;
```

`crates/mklm-core/src/operation.rs`:

```rust
/// A kbdhid keyboard that is not read-only and whose stored values give it no table of its own:
/// it types with the PC's standard layout (design standard-layout B.3).
pub fn follows_standard(kb: &KeyboardDevice, global: &GlobalSettings) -> bool;

/// Device and global writes of "set the standard layout" in per-keyboard mode.
pub struct StandardChange {
    pub from: Layout,
    pub to: Layout,
    /// One pin per follower that is not left to follow, in inventory order.
    pub device_writes: DeviceWrites,
    /// The standard's values that differ from the stored ones (none when `to == from`).
    pub global_writes: Vec<PlannedWrite>,
    /// The followers pinned to `from`, and those left to follow `to` (instance IDs).
    pub pinned: Vec<String>,
    pub following: Vec<String>,
}

pub fn set_standard_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    to: Layout,
    follow: &[String],
) -> Result<StandardChange, OperationError>;

/// The checked plan (`check_plan`; `apply` is always `RestartPc`) with what it does to whom.
pub struct StandardPlan {
    pub plan: OperationPlan,
    pub change: StandardChange,
}

pub fn plan_set_standard(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    to: Layout,
    follow: &[String],
) -> Result<StandardPlan, OperationError>;
```

拒否（どれも何も書かない）:

| 場合 | エラー | 前面の案内 |
|---|---|---|
| 固定モード（`global.mode() == Fixed`） | `MigrationRequired { fixed: ps2_pin_layout(global) }`（既存） | 「固定モードでは、標準配列がすべてのキーボードの配列です。キーボードごとモードへ移行するときに選べます」（CLI: `mklm-cli migrate --standard <jis\|us>`） |
| 今の標準配列が分からない（`stored_standard` が `None`: 値がない、kbd106n や kbdnec など、識別子と合わない） | `UnknownStandard { layer_driver, identifier }`（新規） | 「標準配列の値が MKLM の知っている形ではありません（技術的な詳細）。今どの配列で打っているかが分からないので変更できません」 |
| `--follow` のキーボードが見つからない | `UnknownKeyboard`（既存） | 既存 |
| `--follow` の物理キーボードに follower がない（割り当て済み、PS/2、RDP、ほかのドライバー） | `NotFollowingStandard { instance_id }`（新規） | 「〇〇は標準に従っていません（US を割り当て済み）。標準に従わせるには、再起動の後にそのキーボードの［変更…］で「標準に従う」を選んでください」。PS/2 と RDP は、それぞれ変えられない理由 |
| 固定値のない PS/2 がある | `Plan(PlanError::InvPs2)`（既存） | B.3 の表 |
| 新しい `LayerDriver JPN` の DLL が System32 にない | `LayerDriverMissing`（既存。エンジンと CLI が確かめる。m2 S8） | 既存 |

`to == from` はエラーにせず、書き込みのない `StandardChange` を返す（前面は「変更はありません」）。

エンジンの `error.rs` は、2 つの新しい変種を `ErrorCode::PlanRejected`（`plan_error` なし）に写す（`InconsistentGlobal` と同じ扱い）。前面は同じ関数で先に計画するので、ふつうは helper まで届かない。GUI の `i18n` は `OperationError` を網羅的な `match` で訳しているので、変種を足すとコンパイルが通らなくなり、訳の漏れに気付く（m3 D.4）。

### B.5 エンジンの手順（m2 に「D.13 標準配列の変更」として足す）

入力（`SetStandardParams`）: `standard: Layout`、`follow: Vec<String>`、`expected: Option<ExpectedPlan>`。

1. m2 D.1（ロック、ジャーナル、`Gate::NewOp`: open なエントリがあれば `OpInProgress`、読み直し）。
2. `plan_set_standard(&s.keyboards, &s.global, params.standard, &params.follow)`（下見と同じ関数）。
3. 全体の `LayerDriver JPN` の DLL が System32 にあることを `Host::system32_file_exists` で確かめる（`migrate` と同じコード。関数に切り出して 2 か所で使う）。
4. `check_expected`（`steps` と `apply`。下見の後に follower が増減していれば `PlanChanged` で何も書かない）。
5. `context` に、`migrate` と同じく `i8042prt\Parameters` の全値と全キーボードの `Device Parameters` の全値を記録する（`snapshot_context`。PC 全体の値を変えるので、サポートのために残す）。
6. `build_records`。空なら `NoChange`。
7. 新しい標準に従うキーボードがあり、`to == Jis` なら `STANDARD_UNVERIFIED` の警告を足す（標準が US のときの 0x51 は MT-1 で打鍵を確かめた。JIS はまだ。m0-results.md）。
8. `expected_keyboards(…, None)`（すべての kbdhid と i8042prt。固定したものは `changes: false`、新しい標準に従うものは `changes: true`）。
9. `create(OpKind::SetStandard { from, to, keyboards }, records, Some(RestartPc), context, keyboards)`。起動時の値を変えるので、復旧用ファイルを耐久的に書けなければ `RecoveryAssetsUnavailable` で何も書かない（m2 C10）。
10. `write_new` → `apply_change(ApplyOptions::default())` → `PendingReboot`。結果 `PendingReboot`、`pending_action = RestartPc`。

`OpKind::SetStandard.keyboards` は `(インスタンス ID, LayoutChoice)` の並び。固定したデバイスノードは `(id, from の Jis / Us)`、新しい標準に従わせたデバイスノード（接続中でないものも）は `(id, Standard)`。

### B.6 途中で止まった場合

| 止まった所 | 永続化されている状態 | 回復（m2 C.7 の表のまま。`SetStandard` は `EntryKind::Change`） | 打つ配列 |
|---|---|---|---|
| `Planned` の FJ の前 | エントリなし（復旧用ファイルは書き終えている） | 何も起きていない | 変わらない |
| 固定のステップの途中 | 一部の固定。全体は元のまま | 混在 → `RollBack(PartiallyWritten)` → 固定を消す → `RevertedPendingReboot`（全体の記録を含むので、m2 I.12 のとおり保守的に再起動を求める） | 変わらない（固定は今の配列と同じ。全体は元のまま） |
| すべて書いた後、`Written` の前 | すべて `intended` | 同じ起動なら `RollForward(PendingReboot)`、再起動の後なら `RollForward(AwaitingConfirm)`（再起動後の確認へ）。固定値のない PS/2 が現れていれば `RollBack(InvPs2)` | 固定したキーボードは変わらない |
| 電源断 | ハイブごとに上のどれかの先頭部分 | 同じ分類 | 同じ |

**打つ配列が変わらないこと（I8）**: 固定をすべてフラッシュしてから全体を書く（C.5 のステップごとの FT）ので、どの永続状態でも「全体が新しいのに固定が書かれていない follower」はない。したがって、新しい標準に従わせたキーボードを除き、どの状態から再起動しても、各キーボードの予想される表は操作の前と同じ。これを crash の網羅テストの新しい不変条件 I8 にする（F 章）。

### B.7 再起動の後: 確認、確定、取り消し、undo、導入前に戻す

- **再起動後の確認**: `post_reboot_entries` に入る（`PendingReboot`）。`check_rows` は `Migrate` と同じく、接続中のすべての kbdhid と i8042prt を並べる（新しい標準に従うキーボードの配列が変わるため）。画面の注記は移行の注記に代えて「標準に従うキーボードが新しい標準配列で打てるかは、Windows の認識では分かりません（種類を報告しないため）。そのキーボードで Shift+2 を押して確かめてください」。標準に従わせたキーボードがなければ「この変更で配列が変わるキーボードはありません」。
- **このままにする**: `Request::Confirm`。エンジンは `RebootObserved`（INV-PS2 の確認を含む）→ `AwaitingConfirm` → 値の確認 → `Confirmed`（m2 D.6）。Raw Input の確認は D.2 のとおり。
- **元に戻す / undo**: m2 D.4、D.10 のまま。`plan_restore` の段階で、固定を外す（その他）→ 標準配列を戻す（ペアのない全体）。`RevertedPendingReboot` → 再起動 → `Reverted`。
- **後から戻す**: 取り消せるのは、この操作がすべての値の最新の記録のときだけ（m2 C.8）。後で固定したキーボードを `set` で変えると `NotLatest` になる。その場合は、標準配列をもう一度選び直す（新しい `SetStandard`。そのときの follower が新たに固定される）。GUI の履歴は［元に戻す…］を出さない（既存の `revertible`）。
- **導入前に戻す**: 変えない（baseline は最初の値。この PC では `LayerDriver JPN` の baseline は kbd101.dll、全体のペアは 7/0 で、`restore --baseline --all` は固定モード（英語）に戻す）。

### B.8 ジャーナルの形と互換性

**決めたこと**: Rust では新しい変種 `OpKind::SetStandard { from, to, keyboards }` にし、JSON では既存の `migrate` の形に印を足して書く。エントリの `schema_version` は 1 のまま、`JOURNAL_SCHEMA_VERSION`（2）、`BaselineRecord`、`StoreVersion` も変えない。

```json
"kind": {
  "kind": "migrate",
  "standard": "us",
  "assignments": [
    ["HID\\VID_1D57&PID_FA60&MI_00\\7&14A99BDA&0&0000", "jis"],
    ["HID\\VID_1D57&PID_FA60&MI_03\\7&3A907B5E&0&0000", "jis"],
    ["HID\\VID_3554&PID_F58E&MI_00\\8&18DB8B6B&0&0000", "jis"]
  ],
  "set_standard": { "from": "jis" }
}
```

（この PC で標準配列を JIS から US にし、標準に従う 3 つのデバイスノードを JIS に固定した場合の例。記録は固定 3 × 2 と全体の 2。）

- 実装: `OpKind` に `#[serde(from = "OpKindWire", into = "OpKindWire")]`。`OpKindWire` は今の 4 つの変種と同じ形で、`Migrate` にだけ `#[serde(default, skip_serializing_if = "Option::is_none")] set_standard: Option<SetStandardMark>`（`SetStandardMark { from: Layout }`）を持つ。印があれば `SetStandard { from, to: standard, keyboards: assignments }`、なければ `Migrate`。`OpKind::schema_version` は `SetStandard` に 1 を返す。
- **印を失った場合の補い**: 0.1.x / 0.2.0 がこのエントリを書き直すと（取り消し、確定、`apply_pending` の掃除など、どの遷移でも）、印は消える（古いビルドの `OpKind::Migrate` は `standard` と `assignments` しか持たない）。そこで `JournalEntry::from_json` は、`Migrate` のうち、記録に全体の `OverrideKeyboardType/Subtype` がなく、全体の `LayerDriver JPN` の記録があるものを `SetStandard` と読み直す（`from` はその記録の `before`。読めなければ `Migrate` のまま）。本物の移行は固定モードから始まるので、全体のペアの削除の記録が必ずある（`ps2_pin_layout` が両方の値を求め、`value_eq` で同じなら記録しないが、削除は必ず違う）。次に新しいビルドがこのエントリを書くと、印が戻る。

**古いビルドから見た動き**（0.1.0 と 0.2.0。どちらも `OpKind` は `deny_unknown_fields` を持たないので、`set_standard` は無視される）:

| 場面 | 動き | 正しさ |
|---|---|---|
| 読む | `Migrate { standard: to, assignments }` として読む。`unreadable` にならない | 書き込みも M5b の更新（`check_journal`）も止まらない |
| 回復、取り消し、undo、確定、導入前に戻す | どれも記録だけで決まる（`decide_recovery`、`plan_restore`、`latest_record`） | 正しい |
| 再起動後の確認 | `Migrate` なので、すべての接続中の kbdhid と i8042prt を並べる。`follows_standard`（GUI）は `assignments` の `standard` を見て「標準に従う（JIS）」と表示する | 正しい |
| `restart_waits`、`main_pair_name` | `Migrate` → すべてのキーボードが再起動待ち、全体の値は「標準配列 US」（`PairName::Standard`） | 正しい |
| 文言 | 履歴、再起動の画面、CLI の `journal` が「キーボードごとモードへ移行（標準配列 US）」「migrate to per-keyboard mode (standard layout US)」と書く。確認画面の注記も移行のもの | 見出しだけが違う。受け入れる |
| 書き直し | 印を落とす | 新しいビルドが記録から補う（上） |

**採らなかった形**:

| 案 | 理由 |
|---|---|
| 新しい種類 `set-standard` を版 3 で書く（M3 の `Cleanup` と同じやり方） | 0.1.x / 0.2.0 がこのエントリを `NewerSchema` として `unreadable` にし、書き込みがすべて止まる。エントリは `LayerDriver JPN` の最新の記録なので整理（C.9）でも消えず、止まり続ける。さらに 0.2.0 の更新は `check_journal` が `JournalUnreadable` で断るので、ダウングレードした PC は自動更新でも戻れない（手でのインストールが要る）。起動 ID の修正（m2 C.10）が版を上げなかったのと同じ理由 |
| 新しい種類を版 1 のまま書く | 古いビルドは知らない `kind` を `Malformed` として `unreadable` にする。上より悪い（「更新してください」とも言えない） |
| 印なしの `Migrate` として書く | 新しいビルドも区別できない（記録から補えるが、`journal --json` を読む人と監査のために、書くときは明示する） |

**ダウングレードの注意**（m2 C.10 の注意に足す）: 標準配列の変更が再起動を待っている間に古いビルドを入れると、古いビルドはそれを移行として扱う（動きは正しい）。同じ起動の中で 0.1.x に戻すと、起動 ID の判定の注意（m2 C.10）がそのまま当てはまる。

**文書**: m2 C.3 の `kind` の行、C.10 の版の段落（「`SetStandard`（版 1、`migrate` の形と印）」）、D.13（B.5）、E.6 の要求の表を直す。

### B.9 ipc と helper

```rust
/// Per-keyboard mode: change the PC's standard layout (design standard-layout B). Every
/// keyboard that follows the standard now is assigned its current layout first, except the
/// physical keyboards of `follow`, which follow the new standard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetStandardRequest {
    pub standard: Layout,
    /// Instance IDs (any collection of each physical keyboard).
    pub follow: Vec<String>,
    /// See [`SetLayoutRequest::expected`].
    pub expected: Option<ExpectedPlan>,
}
// Request::SetStandard(SetStandardRequest)
```

- `PROTOCOL_VERSION` を 4 にする（m2 A.5 の規則）。呼び出し元と helper は同じビルド ID を求めるので、版の違う組み合わせは起きない。M5b の更新の要求（m5b D.3）は、入っている版の GUI と helper の間で行うので、版を上げても影響しない。
- helper の `dispatch`: `Request::SetStandard(r)` → `engine.set_standard(&SetStandardParams { standard: r.standard, follow: r.follow.clone(), expected: r.expected.clone() }, sink)`。
- helper は呼び出し元の `follow` のインスタンス ID を、自分の列挙を引くキーとしてだけ使う（m2 E.6）。
- `messages.rs` の JSON の形のスナップショットに `set-standard` を足す。

### B.10 共有クライアント

- `session::plans_first`: `Request::SetStandard` は `Planned` を先に書く要求（取り消しの扱いは `Migrate` と同じ。m3 A.2.2）。
- `gate`: `Gate::NewOp`（`set`、`migrate` と同じ）。
- `preview::check_rows`: `OpKind::SetStandard` も「すべての接続中の kbdhid と i8042prt」。
- `preview::standard_rows(snapshot, plan: &StandardPlan) -> Vec<StandardRow>`: 下見の表の中身（CLI と GUI が共有）。

```rust
pub struct StandardRow {
    /// The group's ID (container ID, else the first instance ID), as the main screen's rows.
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub present: bool,
    /// The layout its stored values give it now (`after_restart` of today's values) and after
    /// the plan (`None` when no table is known, e.g. the Remote Desktop keyboard).
    pub before: Option<LayoutTable>,
    pub after: Option<LayoutTable>,
    pub role: StandardRole,
}

pub enum StandardRole {
    /// Assigned already (a table of its own); PS/2 included (`ps2: true`).
    Assigned { ps2: bool },
    /// Follows the standard now and is assigned its current layout (the default).
    Pinned,
    /// Follows the standard now and follows the new one (the user's choice).
    Follows,
    /// Some collections follow the standard (pinned), others are assigned.
    Mixed,
    /// Never written: the Remote Desktop keyboard, virtual keyboards, other drivers.
    RemoteDesktop,
    ReadOnly,
}
```

「今の設定」は Raw Input ではなく保存値から予想する（RDP のセッションでは Raw Input が物理キーボードを並べないため。D.2）。

### B.11 CLI: `mklm-cli standard`

```text
mklm-cli standard <jis|us> [--follow <keyboard>]... [--dry-run] [--yes]
```

| 引数 | 意味 |
|---|---|
| `<jis\|us>` | 再起動の後の PC の標準配列（`StandardArg`） |
| `--follow <keyboard>` | この物理キーボードは今の配列のままにせず、新しい標準配列に従わせる（再起動で配列が変わる）。繰り返せる。`<keyboard>` はインスタンス ID か `#n` |
| `--dry-run` | 下見を表示して終わる。helper を起動せず、何も書かない |
| `--yes` | 確認を省く（UAC は出る）。`#n` とは組み合わせられない（使い方の誤り、終了コード 2。m2 S12）。リセットを伴わないので `--other-input` / `--no-reset` は持たない |

**`#n` の規則**（m2 F.1 のまま）: `#n` は `mklm-cli list`（接続中のキーボードだけ、`list` と同じ並び）の行番号。接続していないキーボードはインスタンス ID で指定する。解決したキーボードは、名前とインスタンス ID をそれぞれ表示してから進む。PowerShell では `"#2"` のように引用符で囲む（`#` が注釈になる）。`#n` が PS/2、RDP、割り当て済みのキーボードを指したら `NotFollowingStandard` の案内で終わる（終了コード 1）。

**流れ**（`migrate` の流れを基にする。`commands.rs`）:

1. `start`（OS、`--dry-run` でなければ RunOnce の規則、ジャーナル、起動 ID）→ `inventory()`（書き込みを止める読み取りの問題があれば止める）。
2. `--follow` を 1 つずつ解決して表示する。
3. `gate(Gate::NewOp)`（ふさがっていれば終了コード 6。`--dry-run` なら注記だけ）。
4. `plan_set_standard`。エラーは B.4 の案内。`to == from` なら「Nothing to change: the PC's standard layout already is JIS.」で 0。
5. DLL の確認（`migrate` と共通の関数）。
6. 下見を表示する（下の例）。RDP のキーボードがあるか、RDP のセッションなら C.3 の注記。標準に従わせるキーボードがあり `to` が JIS なら、打鍵が未確認の注記。
7. `--dry-run` なら `dry_run_done`。そうでなければ UAC の説明と「Continue? [y/N]」（`--yes` で省く）。
8. `Request::SetStandard { standard, follow: 解決したインスタンス ID, expected: Some(preview::expected(&plan.plan)) }` を `run_request` で送る（`ApplyOptions::default()`、`After::Standard`: `PendingReboot` なら「After the restart, MKLM (or `mklm-cli post-reboot`) asks whether to keep the change.」）。昇格した CLI の RunOnce の案内は D.3。

**下見の例**（この PC、JIS → US、`--dry-run`）:

```text
The PC's standard layout: JIS -> US (takes effect at the next PC restart: Restart, not Shut down)
Keyboards (the layout their stored values give them):
  Keyboard                     Now        After  What happens
  標準 PS/2 キーボード          US         US   PS/2: assigned (unchanged)
  USB Keyboard                 US         US   assigned (unchanged)
  2.4G Wireless Device         JIS        JIS  follows the standard now: JIS is assigned
  Keychron Receiver            US         US   assigned (unchanged)
  VXE Mouse 1K Dongle          JIS        JIS  follows the standard now: JIS is assigned
  リモート デスクトップ ...     -          -    Remote Desktop keyboard: never written (see the note)
Planned changes (they apply to every user of this PC):
  1. HKLM\SYSTEM\CurrentControlSet\Enum\HID\VID_1D57&PID_FA60&MI_00\7&14A99BDA&0&0000\Device Parameters
       KeyboardTypeOverride     (none) -> 7
       KeyboardSubtypeOverride  (none) -> 2
  …
  4. HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters
       LayerDriver JPN             "kbd106.dll" -> "kbd101.dll"
       OverrideKeyboardIdentifier  "PCAT_106KEY" -> "PCAT_101KEY"
Takes effect: at the next PC restart (Restart, not Shut down)
INV-PS2: holds after every step
Layout driver kbd101.dll: present in System32.
To let a keyboard switch with the standard instead, add --follow <keyboard>.
```

**終了コード**（m2 F.5 のまま）: 0 変更なし・下見だけ（下見でふさがっていれば、`migrate` と同じくその終了コード）、1 失敗・拒否、2 使い方の誤り、3 取り消した、5 衝突、6 止められた、3010 PC の再起動が必要（ふつうの成功）。

**置き場所**: 最上位のコマンド `Command::Standard(write::StandardArgs)`（ヘルプ: "Change the PC's standard layout (per-keyboard mode; one PC restart). Keyboards that follow the standard now keep their layout unless given with --follow."）。`global` の下には足さない（`global` は読み取りだけのまま。`global status` が結果を示す）。

**ほかの CLI の変更**:

- `migrate` がキーボードごとモードで何もしないとき（今は「Nothing to migrate」）に、`--standard` が今の標準配列と違えば、「The PC is already in per-keyboard mode; to change its standard layout use `mklm-cli standard <jis|us>`.」と表示して 1 で終わる（頼まれた標準配列にならなかったのに 0 を返さない）。`--standard` がないか同じなら今までどおり 0。`operation_error` の `NotFixedMode` の案内にも `standard` を足す。
- `Command::waits_for_updates` に `Standard` を足す（書き込みのコマンドなので、更新の間は 6 で止まる。m5b D.14）。
- `render::kind_text`: `SetStandard` は「set the PC's standard layout to US (was JIS); JIS assigned to `<id>`, …; `<id>` follows the standard」。

### B.12 GUI

**入口**（どれもキーボードごとモードのときだけ。固定モードでは出さない）:

1. メイン画面の状態行の「標準配列」の下に、小さなボタン［変更…］（読み上げ名「PC の標準配列を変更」）。書き込みがふさがっている間（`StartupSummary::blocks_writes`）は無効にし、理由は行の理由と同じ文（`i18n::cannot_change_now`）。
2. 設定ページに行「PC の標準配列: JIS［変更…］」。固定モードでは「固定モードです。すべてのキーボードが JIS で動きます。キーボードごとモードへは、キーボードの［変更…］から移行できます」を出し、ボタンは出さない。
3. 入力方式の案内のリモート デスクトップの段落の下に［PC の標準配列…］（C.3）。

初回セットアップ（m3 B.1 の手順 4）は変えない。キーボードごとモードでは標準配列を読み取り専用で示し、「変えるには、設定の［PC の標準配列…］を使います」と添えるだけにする（ウィザードの変更は 1 台ずつの `set` のままにする）。

**ページ**（`Page::Standard`。ナビゲーションにはない。m3 B.0 のページの作りのとおり、題 / 本文 / ボタンの行）:

```text
┌ PC の標準配列 ────────────────────────────────────────────────────┐
│ PC の標準配列は、配列を割り当てていないキーボード（標準に従うキーボー│
│ ド）が使う配列です。                                                │
│ ◉ JIS（日本語 106/109 キー）（現在）                                │
│ ○ US（英語 101/102 キー）                                           │
│ PC の再起動の後は、次のようになります。                              │
│ キーボード               今の設定      再起動の後                    │
│ 標準 PS/2 キーボード      US           US（変わりません）            │
│ USB Keyboard             US           US（変わりません）            │
│ 2.4G Wireless Device     JIS（標準）   ☑ 今の配列のままにする → JIS  │
│ Keychron Receiver        US           US（変わりません）            │
│ VXE Mouse 1K Dongle      JIS（標準）   ☑ 今の配列のままにする → JIS  │
│   [キー入力なし]                                                     │
│ リモート デスクトップ      —           変更できません（下の説明）      │
│ ⓘ リモート デスクトップ: …（C.3）                                    │
│ ⓘ この設定は、この PC のすべてのユーザーに適用されます。            │
│ PC の再起動が 1 回必要です。再起動するまで、MKLM でほかの変更はできま│
│ せん。                                                              │
│ ☐ 技術的な詳細を表示する                                            │
│ 次に Windows の確認画面が出ます（発行元は「不明」）。                │
│ mklm-helper.exe であることを確かめて「はい」を押してください。       │
│                       [キャンセル] [変更する（次に Windows の確認が出ます）] │
└──────────────────────────────────────────────────────────────────┘
```

- 選択肢は `RadioList`（m3 B.4 と同じ）。今の標準配列に「（現在）」。今と同じものを選んでいる間は表を出さず、「今の標準配列です。」、［変更する］は無効。
- チェックを外すと、その行は「→ US に変わります（新しい標準に従う）」（警告の色は付けない。利用者が選んだ変更）。`to` が JIS なら「打鍵での確認がまだです。後で Shift+2 で確かめてください」を添える（m3 B.5 の「標準に従う」と同じ）。
- 「今の設定」は保存値から予想した配列（B.10）。RDP のセッションでも表を出せる。
- 表の行は `preview::standard_rows`。行の並びと名前はメイン画面と同じ。非表示の行は「非表示」の印付きで出す（B.3 の 4）。未接続のものは、あるときだけ「未接続のキーボード（n 台）」の 1 行にまとめ、開くと同じ形の行（チェック付き）が並ぶ。
- 技術的な詳細: 書く値の一覧（`plan_text` と同じ内容）、7/0 などの置き換え（B.3 の 5）、INV-PS2 の結果。
- **準備**: 選択肢を選ぶと I/O ワーカーの `IoTask::PrepareStandard { token, to }`（`read_inventory` → `read_journal` → `gate::blocker(Gate::NewOp)`）。結果は既存の `PreparedChange` と同じ形で受け取る。止められていれば、その `BlockReason` の文を出し、UAC は出さない（m3 B.5 と同じ）。
- **チェックを変えたとき**: 準備した `snapshot` から `plan_set_standard` を UI スレッドで作り直す（純粋な関数で、キーボードの数だけの計算。m3 A.4 の規則 1 の「ごく短い処理」）。表示、`ExpectedPlan`、送る `follow` を常に一致させる（m2 S6）。
- **状態**: `AppState::standard: Option<StandardDraft>`。

```rust
pub struct StandardDraft {
    pub to: Option<Layout>,
    /// Row IDs (groups) the user set to follow the new standard.
    pub follow: Vec<String>,
    pub preparing: Option<u64>,
    pub prepared: Option<PreparedChange>,
    pub failure: Option<PrepareFailure>,
    /// The plan shown (made from `prepared`, `to` and `follow`); sent as `ExpectedPlan`.
    pub plan: Option<Result<StandardPlan, OperationError>>,
}
```

- **メッセージ**: `AppMsg` の `OpenStandard`、`StandardChosen(Layout)`、`StandardFollowToggled { row, follow }`、`StandardPrepared { token, result }`、`StandardApply`、`CancelStandard`。
- **UAC の説明**: 初回は既存の `UacNoticeScreen`（`UacNoticeOrigin` に `Standard` を足し、［キャンセル］で標準配列のページに戻る）。2 回目からはボタンの上の 1〜2 行（m3 B.5）。
- **セッション**: `AppMsg::StartRequest` → `Request::SetStandard`、`ApplyOptions::default()`、目的は `SessionPurpose::Change`。進みぐあいは既存の progress。結果（m3 B.17）は `PendingReboot` → 次の手順「再起動…」→ 再起動の画面。
- **再起動の画面**（m3 B.8）: 理由は「PC の標準配列を JIS から US に: PC の再起動待ち」。パスワードの警告（`layout_changes`）は、`SetStandard` では「新しい標準に従うキーボードがあるとき」だけにする（`changes_a_layout` を種類ごとに: `SetStandard` は `keyboards` に `Standard` があるか）。固定だけなら、どのキーボードの配列も変わらないため。
- **再起動後の確認**（m3 B.9）: 操作の行は「PC の標準配列を JIS から US に」。表は B.7。「設定」の列: 固定した行は「JIS（割り当て）」、標準に従う行は「標準に従う（US）」（`follows_standard` は `SetStandard` の `keyboards` の `Standard` も見る）。注記は B.7。［元に戻す（もう一度 PC の再起動が必要）］。
- **履歴**（m3 B.11）: 「PC の標準配列を JIS から US に」。状態の文は D.1。
- **再起動を待つ間のメイン画面**: 状態行の標準配列は、この起動で `LayerDriver JPN` を書いた `PendingReboot` のエントリ（`SetStandard`、標準配列を変える `Migrate`）があれば、その記録の `before` を「今」、保存値を「再起動の後」として「US（再起動の後。今は JIS）」と書く（保存値はもう新しいが、効いているのは前の値のため。`Assessment` は保存値で求める: `assess.rs` の注記）。行の「現在の動作」は、`RestartWaits::all` の間、表が標準配列で決まる行（`basis == Standard`）について配列名を出さず「PC の再起動まで、変更前の標準配列で動いています」とする（今は保存値の新しい標準配列の名前が出てしまう）。固定したが再起動前の行の「保存済み（反映待ち）」には「配列は変わりません」を添える（`current.table == after_restart.table` のとき）。

### B.13 文言（日英。`i18n::standard` に置く。画面の固定の文は `@tr` と `.po`）

| 場所 | 日本語 | English |
|---|---|---|
| 題 | PC の標準配列 | The PC's standard layout |
| 説明 | PC の標準配列は、配列を割り当てていないキーボード（標準に従うキーボード）が使う配列です。 | The PC's standard layout is the layout of every keyboard without a layout of its own (keyboards that follow the standard). |
| 選択肢 | JIS（日本語 106/109 キー）/ US（英語 101/102 キー） | JIS (Japanese 106/109 keys) / US (English 101/102 keys) |
| 表の前 | PC の再起動の後は、次のようになります。標準に従っているキーボードには、先に今の配列を割り当てるので、配列は変わりません。 | After the PC restarts, the keyboards type as follows. MKLM first gives the keyboards that follow the standard their current layout, so they keep it. |
| 割り当て済み | {} （変わりません） | {} (unchanged) |
| 固定のチェック | 今の配列のままにする | Keep its current layout |
| チェックの読み上げ名 | {name} を {layout} のままにする | Keep {name} as {layout} |
| 標準に従わせる | → {to} に変わります（新しい標準に従う） | → becomes {to} (follows the new standard) |
| 混在 | 一部のコレクションだけ標準に従っています | Only some of its collections follow the standard |
| RDP の行 | 変更できません（下の説明） | Cannot be changed (see below) |
| 変更なし | 今の標準配列です。 | This is the current standard layout. |
| 再起動 | PC の再起動が 1 回必要です。再起動するまで、MKLM でほかの変更はできません。 | The PC must restart once. Until then MKLM cannot make other changes. |
| 操作の名前 | PC の標準配列を {from} から {to} に | The PC's standard layout from {from} to {to} |
| 確認の注記（標準に従うものあり） | 標準に従うキーボードが新しい標準配列で打てるかは、Windows の認識では分かりません。そのキーボードで Shift+2 を押して確かめてください（" なら JIS、@ なら US）。 | Windows cannot show whether the keyboards that follow the standard type with the new standard layout. Press Shift+2 on each of them (" means JIS, @ means US). |
| 確認の注記（なし） | この変更で配列が変わるキーボードはありません。 | No keyboard changes its layout with this change. |
| 固定モード | 固定モードでは、標準配列がすべてのキーボードの配列です。キーボードごとモードへ移行するときに選べます。 | In fixed mode the standard layout is every keyboard's layout. You choose it when switching to per-keyboard mode. |
| `UnknownStandard` | 標準配列の値が、MKLM の知っている形ではありません。今どの配列で打っているかが分からないため、変更できません。 | The standard layout's values are not in a form MKLM knows. MKLM cannot tell what the keyboards type now, so it cannot change it. |
| `NotFollowingStandard` | {name} は標準に従っていません（{layout} を割り当て済み）。標準に従わせるには、再起動の後にその行の［変更…］で「標準に従う」を選んでください。 | {name} does not follow the standard ({layout} is assigned). To make it follow the standard, choose "Follow the standard" with its "Change…" after the restart. |

`vm::unexpected_latin` の許可リストは変えない（JIS、US、PC、PS/2、Shift だけを使う）。「確定」と、pin の意味の「固定」を画面の文に使わない。「固定モード」という語があるので `japanese_notation` で「固定」を検出はせず、B.13 の文は日英のスナップショットのテストで押さえる。

### B.14 リモート デスクトップとの関係、リンクの設計との関係

- RDP のキーボード（terminpt）は値を読まないので、固定できない。セッションのキー配列は、セッションが始まるときに Windows が決める（C.1）。この操作は、この PC の物理キーボードの配列を保ったまま、標準配列だけを変える手段になる。
- もし Windows が RDP のキー配列を**この PC の標準配列**から決めるなら、この操作が RDP のキー配列を選ぶ手段になる。**接続元の報告**から決めるなら、この操作は RDP に影響しない。どちらでも、JIS の接続元には「標準配列 JIS ＋この PC の US キーボードに US」で JIS になるはず（C.1）。E7（E 章）でどちらかを確かめる。
- 接続元と接続先の両方に MKLM があるときの連携は `design/rdp-link` で扱う（利用者の要望: 「RDP 先にも RDP 元にも MKLM が入っているので、リンクできたらもっと良さそう」）。連携が接続先の標準配列を変える必要があれば、この操作（`Request::SetStandard`）を使える。この設計は連携を前提にせず、連携がなくても完結する。

---

## C. リモート デスクトップの案内

### C.1 分かっていること、分かっていないこと

分かっていること（2026-09-29 まで）:

| # | 事実 | 出どころ |
|---|---|---|
| F1 | RDP で届くキーはスキャンコードで、どのキーボードからかは届かない。セッションのキーは 1 つの RDP キーボード（terminpt）から入る | rdp-keyboard.md 3 節 |
| F2 | 接続元の US キーボードで打っても、JIS の接続元から JIS で打たれた（セッションのキー配列は 1 つ） | 0.1 節 |
| F3 | コンソールで作ったセッションへの再接続、標準配列 US、JIS の接続元 → US | 0.1 節（14:20） |
| F4 | RDP で作ったセッション、標準配列 JIS、JIS の接続元 → JIS。H2（キーボードごとモードの RDP は kbdjpn の既定の kbd101 になる）は棄却 | 0.1 節（23:1x） |
| F5 | キー配列はセッションが始まったときに決まる（再起動を待つ変更は、再起動と新しいサインインの後） | rdp-keyboard.md 1、9 節 |
| F6 | RDP で作ったセッションの Raw Input は、この PC の物理キーボードを並べない | 0.1 節 |

分かっていないこと: 新しいセッションのキー配列を、Windows が**この PC の標準配列**から決めるのか、**接続元の報告**（7/2 → kbd106 など）から決めるのか。F4 はどちらでも JIS になるので区別できない。E7（標準配列 US の PC に JIS の接続元から新しいセッション）で決まる: H2 が棄却されたので、JIS なら接続元の報告、US（101）なら標準配列。コンソールで作ったセッションへの再接続がどう決まるか（F3 はどの仮説とも合う）も分かっていない。

### C.2 書いてよいこと、書かないこと

- 書いてよい: F1〜F6。「Windows が、この PC の標準配列と接続元の報告のどちらかから決めます（どちらかはまだ確かめていません）」。「JIS の接続元から使うときは、標準配列を JIS にし、この PC の US キーボードに US を割り当てておくと、標準配列と接続元の報告のどちらも JIS なので、JIS になるはずです（この形の PC で確かめました）」。確かめた日付は研究メモと README に書き、画面の文には書かない。
- 書かない: 「接続元の配列に従います」「標準配列に従います」「接続元のキーボードごとに配列が使われます」「MKLM ではリモート デスクトップの配列を変えられません」（標準配列に従うなら変えられる）。US の接続元にどうすればよいか（確かめていない）。
- テストで守る: CLI の `the_remote_session_note_says_only_what_is_known`（`main.rs`）の禁止語に「follows the standard」「follows the client」「the client's keyboard layout applies」を足し、「one key table」「which of the two is not known yet」を含むことを確かめる。GUI は `i18n` のテストと `tests/translations.rs` に同じ規則（「接続元の配列に従」「標準配列に従います」を含まない、「まだ確かめていません」を含む）。今の「applies to the whole session」の禁止は、古い誤った注記（接続元の配列がセッション全体に効く）を防ぐためのものなので残し、新しい文は「1 つのキー配列で打たれます」と書く。

### C.3 各場所の文

**GUI の RDP のキーボードの行**（`i18n::badge(BadgeKind::RemoteDesktop)` の読み上げの文。今のテストは先頭の「リモート デスクトップ: 接続元の PC から届くキー入力です。」を確かめている）:

- 日本語: 「リモート デスクトップ: 接続元の PC から届くキー入力です。このキーボードは変更できません。接続元にキーボードが何台あっても、セッションのキーは 1 つのキー配列で打たれます。そのキー配列は、セッションが始まったとき（サインインしたとき）に Windows が、この PC の標準配列か接続元の報告から決めます（どちらかはまだ確かめていません）。再起動を待つ変更は、再起動して新しくサインインするまで反映されません」
- English: "Remote Desktop: keys sent by the PC you connect from. This keyboard cannot be changed. Every key of the session is typed with one key table, however many keyboards the client has. Windows sets it when the session starts (at sign-in), from this PC's standard layout or from what the client reports (which of the two is not known yet), so a change that waits for a restart reaches it only after the restart and a new sign-in"
- 行の注記（`remote_client_report`）は今のまま（「接続元の報告: 日本語キーボード (JIS)。このセッションのキー配列と同じとは限りません」）。

**入力方式の案内**（`other.slint`）: 今の段落（`msgid` が "Over Remote Desktop," で始まるもの。`the_remote_desktop_paragraph_keeps_its_terms_apart` の制約はそのまま守る）は変えず、その後に 2 つの段落を足す。

- 段落 2（新）: 「接続元の PC にキーボードが何台あっても、セッションのキーは 1 つのキー配列で打たれます。接続元の US キーボードで打っても、キー配列が JIS なら JIS として打たれます。Windows がこのキー配列を、この PC の標準配列と接続元の報告のどちらから決めるのかは、まだ確かめていません。」 / "However many keyboards the client PC has, the session types every key with one key table: keys from the client's US keyboard are typed as JIS when that table is JIS. Whether Windows sets it from this PC's standard layout or from what the client reports is not known yet."
- 段落 3（新。SetStandard と同じ版から）: 「JIS のキーボードの接続元から使うときは、この PC の標準配列を JIS にし、この PC の US キーボードには US を割り当ててください。標準配列と接続元の報告がどちらも JIS になるので、どちらから決まっても JIS になるはずです（この形の PC に新しいセッションで接続して、JIS で打てることを確かめました）。」＋［PC の標準配列…］（キーボードごとモードのときだけ） / "To connect from a PC with a JIS keyboard, make this PC's standard layout JIS and assign US to this PC's US keyboards. Then this PC's standard layout and the client's report are both JIS, so the session should type JIS either way (a new session to a PC set up like this was seen to type JIS)." 画面の文には日付を入れない（研究メモと README の変更履歴が日付を持つ）。

**標準配列のページ**（B.12 の ⓘ）: 段落 2 と段落 3 の 1 文目、それに「リモート デスクトップのセッションに届くのは、PC を再起動して新しくサインインした後です。」。

**CLI**:

- `REMOTE_SESSION_NOTE`（`main.rs`）: "note: this is a Remote Desktop session. Keys typed here come from the client PC and are all typed with one key table, whichever of the client's keyboards they come from. Windows sets that table when the session starts, from this PC's standard layout or from what the client reports (which of the two is not known yet); a change that waits for a restart reaches it only after the restart and a new sign-in. MKLM's per-keyboard layouts apply to the keyboards attached to this PC."
- `status` の RDP のキーボードの「Remote Desktop」の項目: "types the keys the client sends with one key table, set by Windows when the session started (from this PC's standard layout or the client's report; not known which); read-only"。
- `standard` の下見の注記: "Remote Desktop: the session types every key with one key table, set when it starts from this PC's standard layout or from what the client reports (not known which). With a JIS client, a JIS standard layout and US assigned to this PC's US keyboards should give JIS either way (a new session to a PC set up this way was seen to type JIS)."

**README**（日本語と英語の「できない」の行を書き直し、「リモート デスクトップで使う」の小見出しを足す）:

- できない: 「リモートデスクトップの接続先で、接続元のキーボードごとに配列を変えること。接続元から届くキーは、セッションごとに 1 つのキー配列で打たれる（接続元に JIS と US のキーボードがあっても同じ表）。その表はセッションが始まったときに Windows が決め、この PC の標準配列と接続元の報告のどちらに従うかは未確認（[docs/research/rdp-keyboard.md](docs/research/rdp-keyboard.md)）」
- 小見出し「リモート デスクトップで使う」: 1) JIS の接続元なら、標準配列を JIS にし（`mklm-cli standard jis`。移行のときは `migrate --standard jis`）、この PC の US キーボードに US を割り当てる。2) 変更は、接続先の PC を再起動して新しくサインインしたセッションから。3) 英数キーの代わりのキー（今の文）。4) リモート デスクトップのセッションからは、この PC のキーボードの動作を確かめられない。確認はこの PC の前で。
- 書き込みのコマンドの表に `mklm-cli standard <jis|us> [--follow <キーボード>]...` を足す。

**インストールの案内**（`docs/install-guide.ja.md`）: 「最初に起動したとき」の後に「## リモート デスクトップで接続して使う場合」を足す。README の小見出しの 1)〜4) を短くしたもの、接続先と接続元の両方に MKLM を入れてよいこと（それぞれの PC の物理キーボードに効く。連携の機能はまだない）、詳しくは入力方式の案内（GUI）と README へ。

**rdp-keyboard.md 9 節**（利用者への案内）: E 章の追記と同じ内容で、11:32 以降の案内を今の状態（標準配列 JIS、RDP で作ったセッションで JIS）に直す。

**版の分け方**: 段落 3、標準配列のページ、README の 1) の `mklm-cli standard` は、SetStandard と同じ版で出す。それより前の版（SetStandard を含まない v0.2.0 にする場合）では、段落 2、行の文、CLI の注記、README の「できない」の行と 2)〜4) だけを出す（G 章）。

---

## D. 表示の修正

### D.1 再起動の後の状態（判定した状態で表示する）

**問題**: 保存されている状態のまま表示するので、再起動の後も、次に helper がセッションを開くまで古い状態が出る。例: 16:29:33 の再起動の後、`mklm-cli journal` は `d724c149` を「(reverted; the PC must restart)」と表示し、16:35:41 に `reverted` になるまで続いた。同じく、前の起動の `PendingReboot` は「waiting for a PC restart」と表示され、`Attention` の行は「needs recovery: run `mklm-cli recover`」になる（実際に必要なのは再起動後の確認）。GUI の履歴の `i18n::entry_state` も「元に戻しました（PC の再起動が必要）」「PC の再起動待ち」を出す。

**判定する関数**（`mklm-client::describe`。CLI と GUI で共有）:

```rust
/// What an entry's state means in this boot (design standard-layout D.1). `boot` is the boot ID
/// of this boot (the journal is read with `read_journal`, so 0.1.x boot IDs are judged already).
pub enum ShownState {
    /// The stored state is what it is now.
    Stored(OpState),
    /// `PendingReboot` written in an earlier boot: the PC has restarted; the check after the
    /// restart (keep or revert) is due.
    RestartedCheckDue,
    /// `RevertedPendingReboot` written in an earlier boot: reverted, and the restart put it into
    /// effect (the next helper session closes it as `Reverted`).
    RevertedAndRestarted,
}

pub fn shown_state(entry: &JournalEntry, boot: Option<BootId>) -> ShownState;
```

`boot` が読めないとき（`None`）は `Stored`。`apply_pending` も同じく、`since` が今の起動でなければ「効いている」とする（`apply_pending_cleared` の起動の規則）。

**CLI**:

| 場所 | 今 | 直した後 |
|---|---|---|
| `render::entry_line`（`journal`、`recover` の一覧、`keep` / `revert` / `resolve` の誤り、`undo` の下見、`reboot` の理由、`post-reboot` の見出し、`report` の回復の注記） | `state_text(entry.state)` | `entry_line_at(entry, boot)`: `RestartedCheckDue` → "restarted; waiting for the check after the restart (keep or revert)"、`RevertedAndRestarted` → "reverted (in effect since the restart)"。どの呼び出し元も起動 ID を読んでいるか読める（`start`、`reboot`、`post-reboot`、`report` の読み直し。`preview::undo_text` は引数に `boot` を足す） |
| `journal_view::attention_text` | `Recover` → "needs recovery: run `mklm-cli recover`" | `PendingReboot` の `Recover` は "the PC has restarted: keep or revert it with `mklm-cli post-reboot` (or open MKLM)"。ほかの `Recover` は今のまま |
| `journal_view::entry_text` の「Not in effect yet for …」 | 保存された `apply_pending` をいつも表示 | `since` が今の起動のときだけ表示。前の起動のものは「In effect since the restart」 |
| `status` と `list` | ジャーナルを読まない | 最後に 1 行「Journal: nothing waits」/「Journal: 271b6909 waits for the check after the restart (`mklm-cli post-reboot`, or open MKLM)」など（`startup::summarize` と `shown_state`。読めなければ「Journal: could not be read (`<error>`)」で、ほかの表示は止めない）。`status --json` に `journal: [{op_id, state, shown, attention}]` を足す（足すだけ） |

**GUI**: `i18n::entry_state` に `ShownState` を渡す（`vm::journal::journal_rows` に `boot: Option<BootId>` を足す。`SystemRead::boot`）。`RestartedCheckDue` →「再起動しました（再起動後の確認を待っています）」/ "Restarted; waiting for the check after the restart"、`RevertedAndRestarted` →「元に戻しました（再起動で反映済み）」/ "Reverted (in effect since the restart)"。`undo_state`（undo の下見）と回復の画面の項目も同じ関数を使う。確認画面（`not_restarted`）、再起動の画面（`restart_reasons`）、バナー（`attention`）はすでに起動で判定しているので変えない。

### D.2 リモート デスクトップから見えないキーボード

**問題**: RDP で作ったセッションでは、この PC の物理キーボードが Raw Input に並ばない（F6）。今の表示は、それを「未接続」「unknown」「reconnect the keyboard」と言う。23:14 の「このままにする」では、再起動で値がすでに効いていたのに「Raw Input does not report the stored type yet; reconnect the keyboard」が 2 つ出た。

**事実の分け方**（core。純粋な関数）:

```rust
/// What Raw Input says about a keyboard in this session (design standard-layout D.2).
pub enum RawInputView {
    NotConnected,
    Reported(KeyboardType),
    /// Connected, but this session's Raw Input does not list it. `remote`: this is a Remote
    /// Desktop session (a session started over Remote Desktop does not see the keyboards
    /// attached to the PC).
    NotListed { remote: bool },
}
pub fn raw_input_view(kb: &KeyboardDevice, os: &OsInfo) -> RawInputView;
```

「リモート デスクトップのセッションでは見えない」とは決めつけず、キーボードごとに「接続中なのに並ばない」ことを見て、セッションがリモートならその理由を添える（コンソールで作ったセッションへの再接続で見えるかは確かめていないため）。

**エンジン**（`Engine::confirm`、`engine.rs` 790〜806 行）: 警告の文を 4 つに分ける。英語の診断なので、`OperationResult` の型は変えない（プロトコルも変わらない）。

| Raw Input | 値を書いた起動 | 文 |
|---|---|---|
| 並ばない | 前の起動（再起動で効いている） | "{id}: this session's Raw Input does not list the keyboard, so its type could not be checked here (a Remote Desktop session does not see the keyboards attached to the PC). The values have been in effect since the restart; check the layout at the PC." |
| 並ばない | この起動（再接続の経路） | "{id}: this session's Raw Input does not list the keyboard, so MKLM cannot tell whether it runs with the stored values yet (a Remote Desktop session does not see the keyboards attached to the PC). Check it at the PC; if it still types the old layout there, reconnect it." |
| 違う種類 | 前の起動 | "{id}: reports {reported} although the PC has restarted since the change (expected {expected}); the change may not apply to it. Check it with Shift+2 at the PC and revert if it types the wrong layout." |
| 違う種類 | この起動 | 今の文（"does not report the stored type yet; reconnect the keyboard"） |

`apply_pending` の規則は変えない（前の起動の値は `apply_pending_on_close` が `None` を返す。この起動の再接続待ちは `Reconnect` が残る。見えないものを「効いた」とはしない）。

**GUI**:

- 再起動後の確認（`vm::post_reboot`）: `CheckRow` に `present: bool` を足し（`preview::check_rows` が `KeyboardAssessment::present` から入れる）、「Windows の認識」の列は、未接続なら「未接続」、接続中で見えずリモートなら「リモート デスクトップからは見えません」/ "not visible from Remote Desktop"、接続中で見えずコンソールなら「確かめられません」/ "cannot be checked"（`Recognition` に 2 つの変種）。リモートで見えない行があれば、表の下に「リモート デスクトップのセッションからは、この PC につないだキーボードの Windows の認識を確かめられません。この PC の前で確かめてから［このままにする］を押すか、［後で決める］を選んでください。」（キーのテストの案内 `key_test_remote_prompt` は今のまま）。
- 「このままにする」の結果（`vm::result`）: 要求が `Confirm` で、確認に使った行（`check_rows`）にリモートで見えないものがあれば、メッセージに「リモート デスクトップからは、この PC につないだキーボード（USB Keyboard、Keychron Receiver）を確かめられません。この PC の前で、それぞれのキーボードで Shift+2 を押して確かめてください（" なら JIS、@ なら US）。違っていたら、履歴の［元に戻す…］で戻せます。」（再起動で効いている場合）。再接続の経路なら最後の文を「違っていたら、そのキーボードを抜き差ししてから確かめてください。」にする。英語の警告は今までどおり「技術的な詳細」へ。
- メイン画面の行（`vm::keyboards::current_state`）: 接続中で `current` がないとき、リモートなら「リモート デスクトップからは見えません」、そうでなければ今の「動作を確認できません」。状態行に、リモートで見えないキーボードがあるとき中立の注記「リモート デスクトップで接続しています。この PC のキーボードの動作は、ここからは見えません。」。

**CLI**:

- `list` / `status`（`text.rs`）: `reported_text` は `NotListed { remote: true }` なら "not visible (Remote Desktop)"、`remote: false` なら "unknown"（今のまま）。`list` の最後に、見えないキーボードがあれば "Raw Input in this Remote Desktop session does not list N keyboard(s) attached to this PC, so Reported and Now stay empty for them; run `mklm-cli list` at the PC to see them."
- `post-reboot` / `keep` の表（`preview::check_text`）: 印の列は "not connected" / "not visible here (Remote Desktop)" / "cannot be checked" を分ける。`REMOTE_CHECK_NOTE` に「Raw Input in this session does not list the keyboards attached to this PC, so the Raw Input column cannot show them」を足す。
- `keep` / `post-reboot` の後: 結果の警告に加えて、リモートで見えないキーボードがあれば "Check at the PC: press Shift+2 on USB Keyboard and Keychron Receiver (\" means JIS, @ means US); revert with `mklm-cli revert <op>` if one types the wrong layout."

### D.3 昇格した CLI と再起動後の確認

**問題**: 昇格したコンソールの CLI は、別の管理者のアカウントで動いているかもしれないので HKCU の RunOnce を登録しない（m2 F.4、`RunOnce::TellUser`）。今の案内「After the restart, run `mklm-cli post-reboot` to keep or revert the change (this elevated process may belong to another account, so it registers nothing).」は、何が起きなかったのか、再起動の後に何をすればよいのかが分かりにくい。16:35:41 の移行で利用者が見た。

**直した文**（`commands.rs` の `show_post_reboot_rule`。CLI は英語）:

```text
The check after the restart was NOT set up to open by itself: this command runs as administrator,
and an administrator window may belong to another account, so MKLM does not add anything to the
signed-in user's startup list.
After the restart, sign in and open MKLM (Start menu > Multi Keyboard Layout Manager): it shows the
check and asks whether to keep the change. If MKLM starts at sign-in, it shows the check by itself.
Without the GUI, run `mklm-cli post-reboot` in a normal (not administrator) console.
```

- `reboot`（`post_reboot_rule` を呼ぶ）でも同じ文。
- GUI の `run_once_note(Elevated)` と `run_once_problem(true)` はすでに「MKLM を開いてください」と言っているので変えない。
- テスト: `the_elevated_message_says_to_open_mklm_after_the_restart`（`show_post_reboot_rule` を `Ok(RunOnceOutcome::TellUser)` で呼び、文に「open MKLM」「NOT set up」「mklm-cli post-reboot」があること。HKCU には触れない）。
- 登録そのものを変える案（昇格したトークンの利用者の SID が、セッションにサインインしている利用者の SID と同じなら登録する）は I 章の問い 2。今の決まり（昇格したプロセスは HKCU に書かない。2026-09-28 の利用者の決定）を変えるため、この設計では文だけを直す。

### D.4 CI の Pester（GitHub の `powershell` の包みで失敗する）

**原因**（この PC の Windows PowerShell 5.1 で、`cmd /c` を子にして確かめた。smoke-test.ps1 は動かしていない）:

1. GitHub の `shell: powershell` は、段のスクリプトの前に `$ErrorActionPreference = 'stop'` を、後に `if ((Test-Path -LiteralPath variable:\LASTEXITCODE)) { exit $LASTEXITCODE }` を足して `powershell -command ". '{0}'"` で動かす。
2. `smoke-test.Tests.ps1` の最初のテストは、子の `powershell -File smoke-test.ps1 … 2>&1` を動かす。Windows PowerShell 5.1 では、ネイティブのコマンドの標準エラーを `2>&1` でつなぐと各行が `ErrorRecord`（`NativeCommandError`）になり、`ErrorActionPreference = 'stop'` のもとでは最初の行で終了する例外になる（確かめた: `RemoteException / NativeCommandError`、`$LASTEXITCODE` は -1）。テストが失敗する。
3. テストがすべて通っても、最後に動いたネイティブのコマンドの終了コードが `$LASTEXITCODE` に残る（`find-marker.Tests.ps1` の最後の呼び出しは、わざと 1 で終わるもの。`smoke-test.Tests.ps1` の拒否も 1）。包みの後の行がそれで `exit` するので、段が 1 で終わる。`build-installer.Tests.ps1` だけは `finally` で `$global:LASTEXITCODE = 0` にしている。

**直し方**:

1. **smoke-test.ps1 を子として動かさない**: 193 行の拒否（`if (-not $OnThrowawayMachine) { throw … }`）を関数 `Assert-OnThrowawayMachine([bool]$Confirmed)` にし、実行の本体（`if ($MyInvocation.InvocationName -ne '.')`）の最初の文で呼ぶ。テストは今の他のテストと同じく dot-source して（定義しか動かない）`Assert-OnThrowawayMachine $false` が 'throwaway' を含む例外を投げること、本体の最初の文がその呼び出しであること（スクリプトの文字列を読む。今の MECHANICS-3 のテストと同じやり方）を確かめる。子のプロセスも `2>&1` もなくなり、「smoke-test.ps1 は使い捨ての機械でしか動かさない」がテストでも守られる。
2. **ネイティブのコマンドを動かすテストは、終わりに `$LASTEXITCODE` を戻す**: `find-marker.Tests.ps1` に `finally { $global:LASTEXITCODE = 0 }`（`build-installer.Tests.ps1` と同じ形）。今後 `2>&1` を使うときは、その `It` の中で `$ErrorActionPreference = 'Continue'` にしてから呼ぶ（テストのファイルの先頭の注記に、この 2 つの規則を書く）。
3. **段の本体をファイルにする**: `installer/tests/Invoke-InstallerChecks.ps1`（新規）。先頭で `$ErrorActionPreference = 'Stop'`（GitHub の包みと同じ条件を、手元でも作る）。`check-nsi.ps1` → `Invoke-Pester -Path installer/tests -PassThru` → 失敗の判定 → **明示の `exit 0`**。失敗の判定は `FailedCount -gt 0`、`TotalCount -eq 0` に加えて、Pester 5 の `FailedContainersCount` と `FailedBlocksCount`（あるときだけ見る。Pester 3.4 にはない）。明示の `exit` で、残った `$LASTEXITCODE` が段の結果を決めることはなくなるが、壊れたテストのファイル（コンテナーの失敗）を見落とさない。
4. `ci.yml` の最後の段は `run: ./installer/tests/Invoke-InstallerChecks.ps1` だけにする（`shell: powershell` のまま）。
5. **手元での再現**（FULL CHECKS の「CI と同じ形の Pester」）: `powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'stop'; . './installer/tests/Invoke-InstallerChecks.ps1'; if ((Test-Path -LiteralPath variable:\LASTEXITCODE)) { exit $LASTEXITCODE }"` が 0 で終わること。
6. m5b D.9.3 と G.6 の「その Pester のテスト」の行に、この段のファイルと 2 つの規則を書く。

---

## E. 実機の結果の記録先（2026-09-29）

この設計のコミットでは研究メモを書き換えない。実装の最初の作業（G 章の WP-S0）で、次のとおり記録する。時刻とジャーナルの値は 0.1 節（ジャーナルは読み取りだけで確かめたもの）。

**`docs/research/boot-id.md`**: 新しい節「2026-09-29 夕方から夜の実機（MT の途中経過）」。

| # | 時刻 | 何をしたか | 起動（KUSER、イベント 27） | ジャーナル | MT との関係 |
|---|---|---|---|---|---|
| 1 | 16:21:01〜04 | GUI で `d724c149` を元に戻す | 7 | `confirmed` → `revert-pending` → `reverted-pending-reboot` | — |
| 2 | 16:29:33 | 再起動（完全な起動） | 7 → 8、0x0 | 16:35:41.184 に `reverted`（`reboot-observed`、起動 8、pid 19232） | MT-6 の「0x0 でカウンターが増える」の証拠（打鍵と Keep はしていない） |
| 3 | 16:35:41 | 昇格した PowerShell で移行（`271b6909`） | 8 | `planned` → `written` → `pending-reboot` | RunOnce は登録されなかった（D.3） |
| 4 | 23:12:58 | 再起動 | 8 → 9、0x0 | — | MT-6 の証拠の 2 回目 |
| 5 | 23:14:06〜14 | RDP で新しくサインイン（セッション 2）、MKLM を手で開き Keep | 9 | `recover:reboot-observed`（.163）→ `confirmed`（`keep`、.166）。`Conflict` なし | MT-6b の Keep の部分は合格（TERMINPUT_BUS のキーボードで `Conflict` にならない）。RunOnce は登録されていなかったので「確認画面が開く」は見ていない。警告の文は誤り（D.2）。コンソールでの打鍵の確認はまだ |

あわせて: 16:29:33〜16:35:41 の間、`mklm-cli journal` が `d724c149` を「(reverted; the PC must restart)」と表示した（D.1）。`boot_time_hint` は起動 8 = 134351405735000000（16:29:33.5）、起動 9 = 134351647785000000（23:12:58.5）。「残っている確かめ」の MT-6 を「カウンターの部分は 2 回確かめた。コンソールでの打鍵と Keep が残る」に、MT-6b を「Keep の部分は済み（RunOnce からの起動と、その後のコンソールでの確認が残る）」に直す。MT-2〜MT-5、MT-7〜MT-9 は未実施のまま。

**`docs/research/rdp-keyboard.md`**:

- 0 節に、15:30 以降の時間の流れ（0.1 節の 16:18〜23:14）と、今の接続先（標準配列 JIS、PS/2・USB Keyboard・Keychron は US、2.4G Wireless と VXE は標準に従う）を足す。
- 8 節の表: **E8** の結果に「14:20 のセッション 1（11:37:20 にコンソールで作成、RDP で再接続）、標準配列 US、JIS の接続元: US で打たれた（`@`、`[`）。予想どおり、どの仮説とも合う（区別できない）」。**E7c** の結果に「23:1x のセッション 2（RDP で新しく作成。23:12:58 の再起動の後）、標準配列 JIS（`271b6909`）、JIS の接続元: JIS で打たれた（利用者の報告。どのキーで確かめたかは記録にない。次は E1 の記号で記録する）。H2 を棄却。標準配列と接続元の報告のどちらかは残る」。**E7** の予想を「H2 が棄却されたので、E7 だけで決まる: JIS なら接続元の報告、101 なら標準配列」に直す。
- 7 節の H2 を「棄却（E7c の形の 23:1x の結果）」に。
- 新しい節「接続元の複数のキーボード」: 接続元の PC（`DESKTOP-6FDOQLK`）には JIS と US のキーボードがあり、接続元の US キーボードで打っても JIS で打たれた。キーはスキャンコードで届き、どのキーボードからかは届かない（3 節）ので、セッションのキー配列は 1 つ。MKLM の今の仕組みでは対処できない。接続元と接続先の両方の MKLM を連携させる案は別の設計（ブランチ `design/rdp-link`）。
- 9 節の案内を C.3 のとおりに直す。12 節の問いを「E7 の結果」「E7b（US を報告する接続元）」「コンソールで作ったセッションへの再接続の決まり方」「RDP で作ったセッションで物理キーボードが Raw Input に並ばない理由（コンソールで作ったセッションへの再接続でも同じか）」に直す。

**`docs/research/m0-results.md`**（「未実施」）:

- リモート デスクトップの項に「2026-09-29: 標準配列 JIS の PC に RDP で新しく作ったセッションで、JIS の接続元が JIS で打てた（H2 棄却）。標準配列と接続元の報告のどちらに従うかは E7 で確かめる（rdp-keyboard.md）」。
- 「標準に従う（0x51）」の項は、標準 JIS の打鍵確認がまだのまま（この PC では 2.4G Wireless Device と VXE が標準 JIS に従っているが、打鍵では確かめていない）。T-STD-2（F 章）で確かめられることを添える。

**加えて `docs/research/m5-install-tests.md`**（研究メモの 3 つのほか）: 「その 4: M5b の NSIS での上書き（2026-09-29 16:18、統合ビルド 0.1.0+8994675d を起動 ID の修正版の上に）: 合格。RDP のキーボードの行は『リモート デスクトップ』」。

---

## F. テスト

### F.1 単体テスト（`cargo test --workspace`。昇格なし、実機の設定を変えない）

**fixtures**: `mklm_core::fixtures::desktop_pc()`（0.1 節の今の接続先: PS/2 4/0、USB Keyboard 4/0、Keychron 4/0、2.4G Wireless の 2 つのコレクション（同じコンテナー）と VXE（値なし）、RDP のキーボード、キーボードごとモード、標準 JIS）。値は `mklm-cli status --json --all` を読み取りだけで取って写す。インスタンス ID は実物のまま。

| crate | テスト |
|---|---|
| core `operation` | `set_standard_pins_every_follower_by_default`（desktop_pc で US へ: 2.4G の 2 つと VXE を 7/2、ほかは書かない、ステップは HID → 全体、`apply` は `RestartPc`、INV-PS2 が各ステップで成り立つ）。`set_standard_follow_leaves_the_device`（2.4G の片方の ID で両方が残る）。`set_standard_refusals`（固定モード → `MigrationRequired`、kbdnec / 識別子の不一致 / 値なし → `UnknownStandard`、PS/2・Keychron・RDP の `--follow` → `NotFollowingStandard`、知らない ID → `UnknownKeyboard`、固定値のない PS/2 → `Plan(InvPs2)`）。`set_standard_to_the_same_layout_writes_nothing`。`set_standard_pins_phantoms_and_replaces_7_0`。`follows_standard_table`（0x51、7/0、4/0、7/2、NEC、片方だけの値、i8042prt、RDP） |
| core `allowlist` | `stored_standard` が `explicit_standard` と同じ（kbd106 + PCAT_106KEY、大文字小文字、kbd106n と kbdnec と不一致は `None`） |
| core `journal` | `set_standard_is_written_as_a_migration_with_a_mark`（JSON の形、`schema_version` 1、往復）。`a_0_2_0_build_reads_set_standard`（テストの中に 0.2.0 の `OpKind` の写し `v0_2_0::OpKind` を置き、同じ JSON を読んで `Migrate { standard, assignments }` になること。印は無視される）。`a_mark_dropped_by_an_older_build_is_derived_again`（印のない `migrate` で、全体のペアの記録がなく `LayerDriver JPN` の記録があるもの → `SetStandard`、`from` は `before`）。`a_real_migration_stays_a_migration`（`schema_1_journal`、`legacy_guid_journal`、`271b6909` の形のエントリ） |
| core `recovery` | `decide_recovery` の表の `Change` の行に `SetStandard` を足す（`migrate` と同じ結果） |
| engine `tests/engine.rs` | `set_standard_on_the_desktop_pc`（`PendingReboot`、記録、`context`、接続中の固定したキーボードの `apply_pending = RestartPc`）。`set_standard_keep_after_the_restart`（`reboot()` → `confirm` → `Confirmed`）。`set_standard_revert_needs_a_restart_then_closes`。`set_standard_undo`。`set_standard_refused_while_an_operation_is_open`。`set_standard_missing_layer_driver`（`remove_system32_file("kbd101.dll")`）。`set_standard_plan_changed`（下見の後に follower の phantom を足す → `PlanChanged`、何も書かない） |
| engine `tests/crash.rs` | シナリオ「standard change」と「standard change revert」で I1〜I7。新しい **I8**: どのクラッシュ状態でも、新しい標準に従わせたキーボードを除き、各キーボードの予想される表（その状態の全体の値で求めたもの）が操作の前と同じ。回復の後も同じ |
| engine（D.2） | `keep_in_a_session_that_lists_no_keyboards`（`FakeDevices` の Raw Input を空にする）: 再起動の後なら `Confirmed`、`apply_pending` なし、警告は「does not list」「in effect since the restart」で「reconnect」を含まない。同じ起動（再接続の経路）なら `apply_pending = Reconnect` が残り、警告は「check it at the PC」 |
| ipc | `set-standard` の要求の往復、JSON の形のスナップショット、知らないフィールドの拒否、`PROTOCOL_VERSION == 4` |
| client | `plans_first(SetStandard)`、`check_rows` が `SetStandard` ですべての接続中の kbdhid と i8042prt を並べ `present` を入れる、`standard_rows`（desktop_pc の役割）、`shown_state`（`legacy_guid_journal` の再起動の後と前、`d724c149` の形の `RevertedPendingReboot` を次の起動で読む） |
| CLI | 構文（`standard jis`、`--follow "#3"` の繰り返し、`--yes` と `#n` → 2、`--dry-run`、`waits_for_updates`）。下見の文（desktop_pc）。`journal` の判定した状態（`a_reverted_pending_reboot_of_an_earlier_boot_reads_as_reverted`、`a_pending_reboot_of_an_earlier_boot_waits_for_the_check`、`not_in_effect_lines_of_an_earlier_boot_are_not_shown`）。`list` / `status` のリモートの見えないキーボード。`check_text` の 3 つの印。D.3 の文。`the_remote_session_note_says_only_what_is_known` の更新（C.2）。`migrate --standard` がキーボードごとモードで違う標準を求めたら 1 |
| GUI | `state`: 標準配列のページを状態行と設定から開く、固定モードでは開かない、選択 → 準備 → 表、チェックで計画を作り直す、［変更する］→ `StartSession(SetStandard { expected })`、初回の UAC の説明から戻る、止められたとき、キャンセル。`vm::standard` の日英のスナップショット（desktop_pc: JIS → US、2.4G のチェックを外した場合、RDP の行、未接続のまとめ）。`vm::post_reboot`（`SetStandard` の注記、「リモート デスクトップからは見えません」）。`vm::journal`（D.1 の 2 つの状態）。`vm::result`（Keep の後のリモートの文、再起動で効いている場合と再接続の場合）。`vm::keyboards`（リモートの見えない行）。`vm::restart`（`SetStandard` の `layout_changes`）。`unexpected_latin`、`japanese_notation`、`tests/translations.rs`（新しい `@tr` と段落 2・3。RDP の段落の制約を守ること） |
| Pester | D.4 の直し。`smoke-test.Tests.ps1` の拒否は関数で、`find-marker.Tests.ps1` は `$LASTEXITCODE` を戻す |

### F.2 CI と同じ形の確かめ（FULL CHECKS）

ワークフローの FULL CHECKS に加えて、`Invoke-InstallerChecks.ps1` を D.4 の 5 の形で動かし、0 で終わること。

### F.3 実機のテスト（利用者の同意を得て、1 項目ずつ手順を渡し、ジャーナルと照らしてから次へ）

| # | 内容 | 再起動 |
|---|---|---|
| T-STD-1 | `mklm-cli standard us --dry-run`（コンソールでも RDP でもよい）: 表が B.11 の例のとおり、何も書かれない（`mklm-cli journal` が変わらない） | なし |
| T-STD-2 | GUI で標準配列を JIS → US（既定のまま）→ 再起動の画面 →「再起動」→ **コンソールで**サインイン → 確認画面 → PS/2、USB Keyboard、Keychron、2.4G Wireless で Shift+2（どれも今までと同じ配列）→ このままにする。`journal --json` で `set_standard` の印、`confirmed` | 1 |
| E7 | T-STD-2 の後（標準配列 US、PC のキーボードは変わらない）、コンソールからサインアウトし、JIS の接続元から **RDP で新しいセッション**を作って E1（Shift+2、P の右）と E2（`tools/rdp/Watch-Keys.ps1`）。JIS なら接続元の報告、US なら標準配列（C.1） | 0（T-STD-2 の再起動を使う） |
| T-STD-3 | 履歴から T-STD-2 を元に戻す（または標準配列を JIS に選び直す）→ 再起動 → 標準配列 JIS に戻る。RDP で作ったセッションで JIS | 1 |
| T-STD-4 | RDP で作ったセッションで再起動後の確認をする（T-STD-2 か 3 の途中で）: 「Windows の認識」が「リモート デスクトップからは見えません」、このままにした後の文が D.2 のもので「抜き差し」を言わない | —（2、3 の中で） |
| T-STD-5 | 昇格した PowerShell で `mklm-cli standard …`（T-STD-3 の代わりに行ってもよい）: D.3 の文が出る。再起動の後に MKLM を開くと確認画面 | —（3 の中で） |
| T-STD-6（任意） | 変更を書いた後、再起動の前にサインアウトしてサインインし直し、固定した（まだ抜き差ししていない）2.4G Wireless で Shift+2: `LayerDriver JPN` がサインインで読まれるなら、配列が変わって見える（H.3）。確かめるだけで、終わったら再起動する | 1 |

T-STD-2 と T-STD-3 は、MT-2〜MT-9（起動 ID）に要る「再起動で反映する変更」としても使える（PC のキーボードの配列が変わらないため）。そうするかは I 章の問い 1。

---

## G. 作業の分担と版

| WP | 範囲 | 主なファイル | 依存 | 版 |
|---|---|---|---|---|
| WP-S0 | E 章の記録 | `docs/research/*.md` | なし | すぐ |
| WP-S1 | D.4 CI の Pester | `installer/smoke-test.ps1`、`installer/tests/*.Tests.ps1`、`installer/tests/Invoke-InstallerChecks.ps1`、`.github/workflows/ci.yml`、m5b D.9.3 / G.6 | なし | v0.2.0（今の CI の段が落ちるので、タグの前に必要） |
| WP-S2 | D.1、D.3 | `crates/mklm-client/src/describe.rs`、`apps/mklm-cli/src/write/{render,journal_view,commands}.rs`、`apps/mklm-cli/src/text.rs`、`apps/mklm/src/{i18n.rs,vm/journal.rs}` | なし | v0.2.0 を勧める（プロトコルもジャーナルも変えない） |
| WP-S3 | D.2 | core（`raw_input_view`）、engine（`confirm` の警告）、client（`CheckRow::present`）、CLI、GUI | なし | v0.2.0 を勧める（プロトコルを変えない。エンジンの文が変わるのでビルド ID は変わる） |
| WP-S4 | C 章の文（段落 3、標準配列のページ、README の 1) を除く） | `apps/mklm/src/i18n.rs`、`other.slint`、`.po`、`main.rs`、`text.rs`、README、install-guide（rdp-keyboard.md 9 節は WP-S0） | なし | v0.2.0 を勧める |
| WP-S5 | B.4、B.8（core） | `operation.rs`、`allowlist.rs`、`journal.rs`、fixtures | なし | SetStandard の版 |
| WP-S6 | B.5、B.9（engine、ipc、helper）と crash の網羅 | `engine.rs`、`params.rs`、`error.rs`、`message.rs`、`lib.rs`、`session.rs`、`tests/*` | WP-S5 | 同上 |
| WP-S7 | B.10、B.11（client、CLI） | `preview.rs`、`session.rs`、`write.rs`、`commands.rs`、`main.rs` | WP-S6 | 同上 |
| WP-S8 | B.12、B.13（GUI）と段落 3 | `state.rs`、`state/standard.rs`（新）、`vm/standard.rs`（新）、`i18n/standard.rs`（新）、`ui/screens/standard.slint`（新）、`app.rs`、`worker.rs`、`vm/{status,post_reboot,restart,journal}.rs` | WP-S7 | 同上 |
| WP-S9 | 文書（m2 C.3・C.10・D.13・E.6・F.1、m3 B.2・B.8・B.9・B.13・B.14・K、README の表、recovery.md の FAQ「標準配列を変えたい」）と F.3 の実機テスト | `docs/**` | WP-S8 | 同上 |

- **SetStandard の版**: v0.2.0 は起動 ID の MT-2〜MT-9 と ARM64 の確認を待っている（m2 C.10「公開する順序」）。SetStandard を v0.2.0 に入れるか、その次（0.3.0）にするかは I 章の問い 1。この設計は、どちらでも WP の中身を変えない。入れない場合、C 章の文は段落 2 と CLI の注記までにし、段落 3 と README の 1) は 0.3.0 で出す（C.3「版の分け方」）。
- **レビューで確かめること**（m2 K、m3 I、m5b G.6 への追加）: `SetStandard` のすべての書き込みが `check_plan` を通る。全体のステップが固定のステップのフラッシュの後にしかない（I8）。JSON の `kind` は `migrate` で、`schema_version` は 1。古いビルドの型の写しで読めるテストがある。画面の RDP の文に C.2 の禁止の表現がない。CI の段が明示の `exit` で終わり、Pester のコンテナーの失敗も見る。

---

## H. リスクと未解決

1. **リモート デスクトップのキー配列の決まり方**が分かっていない（C.1）。E7 まで、案内は「どちらかは未確認」のまま。E7 の結果によっては、B.14 の「この操作が RDP のキー配列を選ぶ手段になる」が当たらない。どちらでも、C.3 の JIS の接続元への勧めは成り立つ（どちらも JIS）。
2. **US の接続元**（4/0 か 7/0 を報告する接続元）にどう設定すればよいかは分からない（E7b）。案内には書かない。
3. **再起動の前のサインイン**: `global_change_action` の注記では、user32 と IME は `LayerDriver JPN` をサインイン時に読む。そうなら、変更を書いた後、再起動の前にサインアウトしてサインインし直す（または RDP で新しくサインインする）と、そのセッションでは新しい標準配列が使われうる。固定の値は、キーボードのデバイスが起動し直すまで kbdhid に読まれないので、そのセッションでは固定したキーボードも新しい標準配列で打つかもしれない（原則 2 が再起動の前の新しいサインインでは守られないおそれ）。未確認。対策: 再起動の画面が「今すぐ再起動」を勧める（今のまま）。T-STD-6 で確かめる。確かめて起きるなら、固定した USB キーボードを書いた直後にその場でリセットする案（リセットの失敗の扱いが要る。J.4）を改めて検討する。
4. **phantom への書き込み**（m2 I.3）: 接続していないキーボードの固定は、phantom のデバイスノードのハードウェア キーを書き込みで開く。未検証。開けなければ書き込みエラー → ロールバック（全体の記録を含むので `RevertedPendingReboot`、`failure = WriteError`。書いた固定は消え、全体の値は書かれていない）で安全だが、操作ができない。その場合の案内は既存の書き込みエラーの文で、利用者はそのキーボードの行のチェックを外せば進める。この PC には今、未接続のキーボードがない（`list --all`）ので、m2 I.3 の提案どおり VM で確かめる。
5. **0x51 が標準 JIS に従うこと**は打鍵で確かめていない（m0-results.md）。「新しい標準に従う」で JIS を選んだ場合と、固定しない既定外の使い方で残る。警告を出す（B.5 の 7）。
6. **`OverrideKeyboardIdentifier` と IME**: 標準配列を US にすると識別子は `PCAT_101KEY` になり、IME は 101 キーの PC として振る舞いうる（rdp-keyboard.md の H4 は、101 の表の原因としては棄却したが、IME の振る舞いそのものは調べていない）。JIS を割り当てたキーボードで、IME のキーの扱いが変わるかは未確認。MKLM は 2 つの値を常にそろえる（`check_global_writes`）ので、分けられない。
7. **古いビルドの見出し**: ダウングレードすると、標準配列の変更が「移行」と表示される（B.8）。動きは正しい。
8. **表示の範囲**: 再起動を待つ間の「現在の動作」を配列名なしにする（B.12）のは、表が標準配列で決まる行だけ。`Migrate` の再起動待ちの間の行（固定モードの名残）も同じ問題を持つが、この設計では標準配列で決まる行だけを直す。
9. **CI の Pester**: Pester 3.4 と 5 の結果のオブジェクトの違いを、プロパティがあるときだけ見ることで吸収する。GitHub のランナーの Pester の版が変わると、見落としがありうる（明示の `exit` の前の判定で、少なくともテストの失敗と 0 件は拾う）。
10. **標準配列の値がない PC**: `LayerDriver JPN` と識別子がない Windows（日本語の設定をしたことのない PC）は、今の表を US と推定するが打鍵で確かめていない（`assess.rs` の `inferred_standard_is_not_verified`）。この設計は推定から固定を書かず、`UnknownStandard` で断る。その PC では標準配列を変えられない（1 台ずつの `set` はできる）。

---

## I. 利用者に決めてもらうこと

1. **SetStandard をどの版に入れるか**: (a) v0.2.0 に入れる。MT-2〜MT-9 に要る「再起動で反映する変更」を、PC のキーボードの配列を変えずに作れ（T-STD-2、T-STD-3）、E7 もその再起動で行える。ただし v0.2.0 の変更とレビューの範囲が広がり、プロトコルが 4 になる。(b) v0.2.0 の後（0.3.0）。v0.2.0 には D 章と C 章の一部（WP-S1〜S4）だけを入れる。
2. **昇格した CLI の RunOnce**: 今は、昇格したプロセスは HKCU に書かない（別の管理者のアカウントかもしれないため。2026-09-28 の決定）ので、文だけを直す（D.3）。昇格したトークンの利用者の SID が、そのセッションにサインインしている利用者の SID と同じとき（ふつうの管理者の昇格）は登録する、に変えるか。
3. **E7 をいつ行うか**: 接続先の標準配列を一度 US にして再起動し（SetStandard なら PC のキーボードは変わらない）、JIS の接続元から新しいセッションで確かめる。RDP のキー配列の決まり方が分かる唯一の方法（E7b は接続元の設定の変更が要る）。

---

## J. 採らなかった案

1. **固定モードに戻してから移行し直す**（今のやり方）: 再起動が 2 回要り、US のキーボードを 1 台ずつ指定し直す必要がある（0.1 節）。
2. **割り当て済みのキーボードの配列も同じ操作で変える**（`--also`）: 標準配列の変更と、キーボードの配列の変更が 1 つの確認画面に混ざる。割り当て済みのキーボードは `set` で変える。USB は再起動なしで変えられるので、再起動の回数はふつう増えない。PS/2 だけは 2 回目の再起動が要るが、まれ。
3. **割り当て済みのキーボードを「新しい標準に従う」にする**（固定を外す）: B.3 の follower だけを対象にし、割り当て済みのものは `set --layout standard` にする。操作の意味（「今の配列を保つ」）を狭く保つため。
4. **固定した USB キーボードを書いた直後にその場でリセットする**: 再起動の前でも固定が効く（H.3 の窓がなくなる）が、リセットの失敗（戻らない）を扱う経路が要り、配列は変わらないのにキーボードが数秒止まる。H.3 を確かめてから決める。
5. **CLI で 1 台ずつ「固定しますか?」と尋ねる**: 既定（すべて固定）と `--follow` で足りる。下見が全部を表示する。
6. **`confirm` の結果に型付きの「確かめられなかったキーボード」を足す**（`OperationResult` の変更）: 前面が自分の読み取り（`check_rows` と `present`）で同じことを言えるので、プロトコルを変えずに済ませた（D.2）。
7. **RDP のキー配列を直接変える**（`KeyboardType Mapping`、`Layout File`、RDP のキーボードへの書き込み）: rdp-keyboard.md 10 節の理由のまま採らない。
8. **ジャーナルの版を上げる、新しい `kind` を書く**: B.8 の表。

---

## レビュー対応

（まだない）
