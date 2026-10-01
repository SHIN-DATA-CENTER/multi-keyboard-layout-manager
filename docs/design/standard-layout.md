# 設計: キーボードごとモードでの PC の標準配列の変更、リモート デスクトップの案内、再起動の後の表示

| 項目 | 内容 |
|---|---|
| 対象 | (1) キーボードごとモードのまま PC の標準配列を変える新しい操作 `SetStandard`（B 章）。(2) リモート デスクトップの正確な案内（C 章）。(3) 表示の修正 3 つと CI の Pester の修正（D 章）。(4) 2026-09-29 の実機の結果の記録先（E 章） |
| 根拠 | `docs/design/m2-engine.md`（C.3〜C.11、D.1〜D.10、E.6、F、I）、`m3-gui.md`（B.0〜B.17、D、E）、`m5b-updater.md`（D.3、D.9.3、G.6）、`docs/research/rdp-keyboard.md`、`boot-id.md`、`m0-results.md`、2026-09-29 の実機の結果（0.1 節） |
| 状態 | 設計と v0.2.0 向け実装の引継ぎ記録。1 回目のレビュー（安全性 8 件、画面と文言 16 件）を反映した（末尾の「レビュー対応」）。2026-10-01 の進捗・設計との差分は下記。設計の土台は `m5b/updater` の 167ca01、今回の検査対象は 921e50f 上の作業ツリー |
| 読み手 | 実装する人と、レビューする人（G 章に作業の分け方） |
| 関連 | 接続元の MKLM と接続先の MKLM の連携（リンク）は [rdp-link.md](rdp-link.md) で扱う。この設計はそれに頼らず、それと矛盾しないようにする（B.14）。今の製品に連携の機能はない |

識別子（型、関数、値の名前）は英語のまま書く。「m2 D.3」は m2-engine.md の節、「m3 B.9」は m3-gui.md の節、「m5b D.9.3」は m5b-updater.md の節を指す。画面の文言の決まり（［］と「」、「確定」を使わない、日本語の画面に出してよいラテン文字の語）は m3 D.5 に従う。文言の中の `{pc}` は、リモートのセッションでは「接続先の PC（PC の名前）」、そうでなければ「この PC」（C.3 の冒頭、UX-5）。

### 2026-10-01 の引継ぎ進捗

- 今回の範囲は v0.2.0 向けの未完了実装と RDP 設計の仕上げ。RDP のキー変換試作（P1）、製品への組み込み（P2）、インストール、版上げ、公開は行わない。
- `SetStandard` の core・engine・IPC・helper・client から CLI / GUI までを接続した。CLI は `mklm-cli standard <jis|us> [--follow <keyboard>]... [--other-input|--no-reset] [--dry-run] [--yes]`。`--yes` はリセット方法の指定を必須とし、`--follow #n` は拒否する。GUI の入口はキーボード一覧の［PC の標準配列を変更…］。
- README（日英）、インストールの案内、復旧ガイドに操作・下見・再起動・取り消しの手順を追加した。RDP の説明は、1 セッションに配列は 1 つであり、標準配列と接続元の報告の影響・反映時期は未確認であることを明記する。セッション開始時や再サインイン後を反映の境界として断定しない。
- GUI の標準配列画面は `vm::standard::page` の出力に接続した。固定モードでは入口を出さず、準備中・blocker・no-op では要求を送らない。再起動待ちの標準配列は「保存済み」と表示し、標準に従う行では現在の動作を新しい標準配列と断定せず、再起動後の打鍵確認を求める。forward / revert の同一起動と次の起動、GUI の準備・follow・UAC・要求送信を自動テストで確認する。
- 再起動確認は物理キーボードごとにまとめ、collection の不一致を成功より優先する。未報告の collection があれば全件成功とは表示しない。ContainerId が不明・GUID_NULL のデバイスは別グループ。`SetStandard` は標準追従の対象と影響する読み取り専用の仮想 collection を並べ、配列維持の pin を変更対象として並べない。
- D.1 の表示判定を CLI の journal・操作の見出し・undo/revert の下見・回復結果、GUI の履歴・undo/revert の下見へ接続した。CLI list/status は読み取りだけの journal 要約を足し、status/journal の JSON は保存状態 `state` と起動で判定した `shown` を分ける。表示のために journal を書き換えない。
- この文書の画面構成・文言表は目標設計として残す。現実装の RDP 案内は既存の入力方式の案内と標準配列画面にあり、独立した `Page::RemoteHelp`、PC 名を使う全画面の案内、セッションの打鍵結果を独立表示する機能、Keep 結果に確認不能な物理キーボード名をまとめた専用メッセージは未実装。これらを完了扱いにしない。出所を識別できない RDP キーを物理キーボードの成功判定へ結び付けない規則は維持する。
- 接続先ごとの変換プロファイルは [rdp-link.md](rdp-link.md) に従い P1/P2 の範囲。機能しない設定だけを P0 に足さない。旧 C/D 章の独立 RDP 画面・結果メッセージの詳細は将来の画面案として残し、今回の実装との差分を上に明記した。
- workspace はリリース工程で 0.2.0 に更新した。現時点で v0.2.0 は未公開。今回の検査結果は [v0.2.0-checks.md](v0.2.0-checks.md)、公開準備は [v0.2.0-release.md](v0.2.0-release.md) に記録する。
- **追加の実機テストは一部実施した。** 2026-10-01 に利用者と標準配列変更・配列維持・再起動・Keep・取り消し・再起動後の表示と打鍵を確認した。記録は `docs/research/v0.2.0-real-tests.md`。T-STD-6 / 6b、R-PHANTOM、残りの MT、RDP、アクセシビリティ等は未完了だが、利用者は追加の長い手動検証を省略して公開する運用を明示的に承認した。今回限りの例外はリリース準備記録へ記載し、設計上の仮説や未検証項目を合格とはしない。自動検査の成功で実機検証を代用しない。0.1 節の事実は引き続き 2026-09-29 の別の記録である。

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
| — | 接続元の PC（`DESKTOP-6FDOQLK`）にも MKLM が入っている（接続元のモードと標準配列は記録していない。E 章の E11 で記録する） | 利用者の報告 |

今（この設計を書いた時点。`mklm-cli status --json --all` を読み取りだけで確かめた）の接続先: キーボードごとモード、標準配列 JIS（`kbd106.dll`、`PCAT_106KEY`）。PS/2（`ACPI\PNP0303\0`）4/0、USB Keyboard（04D9:1818）4/0、Keychron Receiver（3434:D027）4/0。2.4G Wireless Device（1D57:FA60。`MI_00` と `MI_03` の 2 つのコレクションが同じコンテナー `{15A651F5-…}` で、メイン画面では 1 行）と VXE Mouse 1K Dongle（3554:F58E）は値がなく、標準（JIS）に従う。RDP のキーボードは `TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION2KEYBOARD0`。未接続のキーボードはない。接続元の報告は 7/2。再起動を待つ操作はない。

**今の手順の問題**: 標準配列を US から JIS に変えるのに、利用者は `d724c149` を取り消して固定モードに戻し（再起動 1）、移行をやり直して（再起動 2）、US のキーボードを 1 台ずつ `--also` で指定した。キーボードごとモードの PC で標準配列を変える操作がないため（`migrate` は「already in per-keyboard mode」で断り、`global` には `status` しかない）。

### 0.2 目的

1. キーボードごとモードのまま、PC の標準配列（`LayerDriver JPN` と `OverrideKeyboardIdentifier`）を 1 回の操作と 1 回の再起動で変えられるようにする。そのとき、**利用者が選ばない限り、PC を再起動した後にどのキーボードの配列も変わらない**ようにする（標準に従っているキーボードに、今の配列を先に割り当てる）。
   - 守れるのは、MKLM が値を書ける kbdhid と i8042prt のデバイスノードだけ（B.3）。リモート デスクトップのキー、ほかのドライバーのキーボード、MKLM が書かない仮想のキーボード、リモート操作のソフトが送るキーは、新しい標準配列に従うことがある。下見と確認の画面でそれを示す。
   - 再起動の前に新しいサインイン（サインアウト、ユーザーの切り替え、高速スタートアップのシャットダウンからの起動、RDP の新しいセッション）があると、その場で切り替えられなかったキーボードの配列が早めに変わることがある（H.3）。その場でリセットできる USB のキーボードは、書いた直後に切り替えてこの窓を閉じる（B.5）。
2. リモート デスクトップについて、確かめたことだけを、GUI、CLI、README、インストールの案内で正確に伝える。文は、キー配列を Windows がこの PC の標準配列と接続元の報告のどちらから決める場合にも、いつ決める場合にも正しいものにする（C.2）。
3. 再起動の後の表示、リモート デスクトップから見えないキーボード、昇格した CLI の案内を直す。CI の Pester の段が GitHub の `powershell` の包みで失敗する原因を直す。
4. きょうの実機の結果を研究メモに記録する場所と中身を決める。

### 0.3 範囲外

- 固定モードの PC で標準配列を変えること。固定モードでは標準配列がすべてのキーボードの配列なので、今までどおり `migrate --standard`（キーボードごとモードへの移行）で扱う。
- 割り当て済みのキーボード（値を持つもの）の配列を、同じ操作で変えること（`--also` に当たるもの）。`set` で行う（J.2）。
- リモート デスクトップのセッションのキー配列を MKLM が直接決めること。`Terminal Server\KeyboardType Mapping`、`Layout File`、RDP のキーボードの devnode への書き込みは、今までどおり行わない（rdp-keyboard.md 10 節）。
- 接続元の複数のキーボードをリモート デスクトップで区別すること、接続元と接続先の MKLM の連携（`design/rdp-link`）。

### 0.4 用語

| 用語 | 意味 |
|---|---|
| 標準配列 | `i8042prt\Parameters` の `LayerDriver JPN`（kbd106.dll = JIS、kbd101.dll = US）と、対になる `OverrideKeyboardIdentifier`（`PCAT_106KEY` / `PCAT_101KEY`）。MKLM は 2 つを常にそろえて書く（`check_global_writes`） |
| 標準に従うデバイスノード（follower） | kbdhid のデバイスノードで、読み取り専用ではなく、型とサブタイプの値が 2 つともあるか 2 つともなく、保存値から予想される種類が自分の表を持たないもの（`predicted_type(global).per_keyboard_table()` が `None`: 値なしの 0x51/0、7/0、そのほかの知らない種類）。接続中かどうかは問わない。値が片方だけのもの（`IncompletePair`）は含めない（B.3 の 6。操作を断る） |
| 固定できないキーボード | MKLM が値を書けない入力のうち、RDP のキーボードを除くもの: 読み取り専用の kbdhid（`Transport::Virtual`）と、ほかのドライバーのキーボード（Hyper-V の基本セッションの hyperkbd など）。種類によっては標準配列に従う（B.3 の 8） |
| 物理キーボード | `group_keyboards` のグループ（メイン画面の 1 行）。外付けは ContainerId ごと、内蔵と ContainerId のないものは 1 台ずつ |
| 固定する（pin） | follower に、今の標準配列と同じ配列の値（US なら 4/0、JIS なら 7/2）を書くこと。**画面では「固定」と書かない**（「固定モード」と紛れるため）。「今の配列のままにする」「US を割り当てる」と書く。CLI も「assign」 |
| 新しい標準に従う（follow） | 利用者が選んだ物理キーボードの follower には何も書かず、再起動の後は新しい標準配列で打たせること |
| 確認用の記録（guard） | `SetStandard` が、全体の `OverrideKeyboardType` と `OverrideKeyboardSubtype` について持つ記録。`before` も `intended` も `Absent` で、何も書かない。固定モードでないことを、書くとき、再起動の後、このままにするとき、取り消すときに確かめるためにある（B.2、B.7、B.8） |
| 接続先の PC / 接続元の PC | リモート デスクトップで接続される PC（MKLM の画面が動いている PC。0.1 節の `DESKTOP-3TCSIET`）と、利用者が前に座って接続する PC（`DESKTOP-6FDOQLK`）。リモートのセッションで出す文、README、インストールの案内では「この PC」と書かず、この 2 つの言い方を使う（`{pc}`。UX-5） |
| 接続元の報告 | 接続元の PC の Windows が、接続するときに知らせるキーボードの種類（Client Core Data。セッションの中の `GetKeyboardType`。`OsInfo::client_keyboard_type`）。接続元のキーボードの数や形ではなく、接続元の Windows の設定で決まる。接続元で MKLM が値を書くと変わるかは確かめていない（E11） |
| RDP で作ったセッション | リモート デスクトップで新しくサインインして作ったセッション（例: 23:14 のセッション 2） |
| コンソールで作ったセッションへの再接続 | コンソールでサインインして作ったセッションに、後から RDP でつないだもの（例: 14:20 のセッション 1） |
| 見えないキーボード | 接続中（`present`）なのに、そのセッションの Raw Input に並ばないキーボード。RDP で作ったセッションでは、この PC の物理キーボードがすべてこうなった（ほかの形のセッションで同じかは確かめていない） |

### 0.5 原則

1. **既存の規則を使い回す。** 書き込みの検査は `check_plan`（許可リスト、INV-PS2、順序）、戻しは `plan_restore`、回復は `decide_recovery`、ジャーナルは C.5 の順序。戻す順序は `plan_restore` に段階を 1 つ足して直す（B.6）。新しい操作だけの規則は、計画を作る関数（core）に閉じ込め、下見とエンジンで同じ関数を使う（m2 S6）。
2. **利用者が選ばない限り、打っている配列は変わらない。** 既定では、標準に従うキーボードをすべて今の配列に固定する。書く順序（固定 → 全体）と戻す順序（全体 → 固定を外す）で、途中で止まっても成り立つ（B.6）。守れるのは MKLM が書ける kbdhid と i8042prt のデバイスノードで、PC を再起動した後（再起動の前の新しいサインインで守れるのは、その場で切り替えたキーボードだけ。H.3）。
3. **PS/2 は常に、自分の表を持つ値で固定されている**（INV-PS2 と B.3 の 7）。この操作は PS/2 の値を書かない。固定値のない PS/2、自分の表を持たない値で固定された PS/2 があれば断る。
4. **古いビルドを壊さない。** 0.1.x と 0.2.0 がこの操作のエントリを読めるようにする。ダウングレードの後も、書き込みも自動更新も止まらない（B.8）。
5. **確かめていないことを書かない。** リモート デスクトップのキー配列が、この PC の標準配列と接続元の報告のどちらで決まるか、いつ決まるか、再起動の前の新しいサインインで何が起きるかは、まだ分からない。画面にも文書にも、どれかだとは書かず、どの場合にも正しい文にする（C.2）。

---

## A. 今のコードと、変える場所（地図）

| 層 | 今あるもの | この設計で足す・変えるもの |
|---|---|---|
| 計画（core `operation.rs`） | `plan_set_layout` / `set_layout_writes`（固定モードで `MigrationRequired`）、`plan_migration` / `migration_writes`（キーボードごとモードで `NotFixedMode`。全体のペアを削除し、`standard_layout_writes` のうち違う値だけ書く）、`physical_device_members`、`apply_method`、`OperationError` | `follows_standard`、`set_standard_writes`、`plan_set_standard`、`standard_guards`（B.4）。`OperationError::{UnknownStandard, NotFollowingStandard, IncompleteValues, Ps2WithoutTable}` |
| 許可リストと INV-PS2（core `allowlist.rs`、`safety.rs`） | `check_plan`（i8042prt → その他 → 全体の順に並べ、各ステップの後に `check_inv_ps2`）、`check_device_writes`（kbdhid は 4/0 か 7/2、読み取り専用と RDP を拒否）、`check_global_writes`（`LayerDriver JPN` と識別子はそろっていること。キーボードごとモードでも標準配列だけを書ける）、非公開の `explicit_standard`、`ps2_pin_layout`、`standard_layout_writes`、`device_layout_writes` | `explicit_standard` を `stored_standard` として公開する。許可リストそのものは変えない（書く名前と値は既存の 4 + 4 のうちのもの） |
| 戻す順序（core `restore.rs`） | `plan_restore`（段階: 固定を足す → ペアのある全体 → その他 → ペアのない全体 → 固定を外す） | 段階 `FollowStandard` を足す（キーボードごとモードのままの戻しで、書いた後に自分の表を持たない HID のステップを全体の後へ。B.6） |
| 配列の判定（core `layout.rs`、`device.rs`、`assess.rs`、`group.rs`） | `KeyboardType::per_keyboard_table`、`effective_layout`、`predict_type`、`keyboard_anomalies`、`assess`（`current` と `after_restart` はどちらも保存されている全体の値で求める）、`group_keyboards` | 変えない。follower と B.3 の判定はこれらで書く |
| ジャーナル（core `journal.rs`） | `OpKind::{SetLayout, Migrate, RestoreBaseline, Cleanup}`（`#[serde(tag = "kind", rename_all = "kebab-case")]`）、`OpKind::schema_version`（`Cleanup` だけ 2、ほかは 1）、`JOURNAL_SCHEMA_VERSION = 2`、`from_json` / `to_json`、`Journal::parse`（読めないエントリは `unreadable` → 書き込みを止める）、`FailureReason::WriteError { message }` | `OpKind::SetStandard`。JSON では `"kind": "migrate"` に印 `set_standard` を足した形で書く（版 1 のまま。B.8）。確認用の記録（`before` = `intended` = `Absent`）。`FailureReason::WriteError` に任意の `target`（UX-16） |
| 回復（core `recovery.rs`） | `entry_kind`（`SetLayout` / `Migrate` / `Cleanup` は `Change`）、`decide`、`attention`、`apply_pending_on_close` | `SetStandard` は `Change`。判定の表は変えない（確認用の記録が、外部の固定モードを `Elsewhere` にする。B.7） |
| 共有の型（core `report.rs`、`model.rs`） | `OperationResult`、`ExpectedPlan`、`ExpectedKeyboard { changes }`、`ErrorCode`、`OsInfo` | `OsInfo::computer_name`（任意。`#[serde(default)]`。UX-5）。ほかは変えない（D.2 は既存の型で足りる。J.6） |
| エンジン（engine `engine.rs`、`params.rs`、`error.rs`、`host.rs`） | `Engine::migrate`（D.3）、`confirm`（D.6。Raw Input が保存値どおりでないと「reconnect the keyboard」の警告）、`revert` / `revert_entry` / `reapply`、`undo_open` / `undo_conflict`、`recover` / `continue_revert`、`create` / `write_new` / `write_forward` / `apply_change`、`expected_keyboards`、`STANDARD_UNVERIFIED` | `Engine::set_standard`、`SetStandardParams`（`apply` を含む）、固定した follower のその場のリセット（B.5）、確認用の記録（`build_records`、`write_forward`、`create`）、書き込みの失敗の閉じ方と `DeviceRemoved`（B.6）、`undo_conflict` と `continue_revert` の規則（B.6）、取り消しの後にリセットしない規則（`reapply`。B.7）、`OperationError` の写し方、`Host::remote_session`、`confirm` の警告（D.2） |
| パイプ（ipc `message.rs`、`lib.rs`） | `Request::{SetLayout, Migrate, …}`、`MigrateRequest`、`PROTOCOL_VERSION = 3` | `Request::SetStandard(SetStandardRequest)`（`apply` を含む）、`PROTOCOL_VERSION = 4` |
| helper（`apps/mklm-helper/src/session.rs`） | `dispatch` が要求を 1 つずつエンジンに写す | `Request::SetStandard` → `engine.set_standard` |
| 共有クライアント（`crates/mklm-client`） | `gate`（`Gate::NewOp`、`post_reboot_entries`、`restart_reasons`、`run_once`）、`preview::check_rows`（`Migrate` ならすべての接続中の kbdhid と i8042prt をコレクションごとに並べる）、`session::plans_first`、`describe`、`startup::summarize`、`run_once::apply_run_once_rule`（昇格していれば `TellUser`） | `plans_first` と `check_rows` に `SetStandard`。`preview::check_groups`（物理キーボードごとにまとめる。B.10、D.2）。`preview::standard_rows`（下見の表）。`describe::shown_state`（D.1）。`CheckRow::present`（D.2） |
| CLI（`apps/mklm-cli`） | `migrate`（キーボードごとモードでは「Nothing to migrate」）、`global status`（読むだけ）、`revert` / `undo`（`--other-input` / `--no-reset`）、`keep`、`post-reboot`、`journal`（`render::state_text`、`entry_line`、`journal_view::entry_text`）、`list` / `status`（`text.rs` の `reported_text` は `unknown`）、`show_post_reboot_rule`（昇格時の案内）、`REMOTE_SESSION_NOTE`、`REMOTE_CHECK_NOTE` | 新しいコマンド `mklm-cli standard`（B.11）。取り消しでリセットしない案内（B.7）。状態の表示を判定した状態に（D.1）。見えないキーボードの表示（D.2）。昇格時の案内（D.3）。`migrate` の案内。RDP の注記（C.3） |
| GUI（`apps/mklm`） | `Page`（13 ページ）、`ChangeDraft`（固定モードの移行では `standard` を選ばせる）、`vm::status`（状態行の「標準配列」は表示だけ）、設定ページ、入力方式の案内（`other.slint` の RDP の段落）、再起動、再起動後の確認（`vm::post_reboot`、`Recognition::NotConnected` =「未接続」）、衝突（`vm::conflict`）、履歴（`i18n::entry_state`）、回復と取り消しの下見（`vm::recovery`）、結果（警告は「技術的な詳細」へ） | `Page::Standard` と `StandardDraft`、`Page::RemoteHelp`（C.4）、状態行の下と設定の［PC の標準配列を変更…］、`vm::standard`、`i18n::standard`、再起動と確認と衝突と履歴の文、取り消しの下見で方式を出さない規則（B.7）、見えないキーボードの文とセッションのキー配列の読み取り（D.2）、判定した状態（D.1）、RDP の案内（C 章） |
| CI（`.github/workflows/ci.yml`、`installer/tests`） | 最後の段（`shell: powershell`）で `check-nsi.ps1` と `Invoke-Pester` | D.4 のとおり直す |

---

## B. 新しい操作: 標準配列の変更（`SetStandard`）

### B.1 利用者から見た動き

- キーボードごとモードの PC で、GUI のメイン画面か設定の［PC の標準配列を変更…］、リモート デスクトップのページ（C.4）、または `mklm-cli standard <jis|us>` から始める。
- 下見は、物理キーボードごとに「設定した配列（今）」と「再起動の後」を示す。標準に従うキーボードは、既定で「今の配列のままにする」（固定する）。利用者は 1 台ずつ「新しい標準に従う」に変えられる。PS/2 と割り当て済みのキーボードは「変わりません」。固定できないキーボードは、新しい標準配列に従うかもしれないことを示す（B.3）。RDP のキーボードは「{to} になることがあります」と C 章の説明。
- 固定するキーボードのうち、その場でリセットできるもの（USB など）があれば、切り替え方を選ぶ（B.12）。「すぐに切り替える」なら、helper が書いた後にそのキーボードを 1 台ずつリセットし、割り当てた配列をすぐに効かせる。打つ配列は変わらない。
- UAC の後、helper が固定の値と全体の値を書き、`PendingReboot` になる。PC の再起動で効く（全体の値は、i8042prt が起動時に、user32 と IME がサインイン時に読む。`global_change_action` の注記）。MKLM は常に PC の再起動を求め、再起動の前にサインアウト、ユーザーの切り替え、シャットダウンをしないよう案内する（H.3）。
- 再起動の後は、移行と同じ再起動後の確認（m2 D.7、m3 B.9）で「このままにする / 元に戻す」を選ぶ。元に戻すと、もう一度の再起動で前の標準配列に戻り、固定の値も消える（その場のリセットはしない。B.7）。

### B.2 書く値と順序

- **デバイスの値**: 固定する follower ごとに、kbdhid の組 `KeyboardTypeOverride` / `KeyboardSubtypeOverride` を、変更前の標準配列の値（US → 4/0、JIS → 7/2。`device_layout_writes(Kbdhid, Some(from))`）にする。1 つのデバイスノードが 1 ステップ。
- **全体の値**: `standard_layout_writes(to)` のうち、今の値と違うもの（`value_eq`。大文字小文字の違いは変更にしない）。キーボードごとモードなので、全体の `OverrideKeyboardType/Subtype` はなく、書かない。
- **確認用の記録**（SAFETY-7）: 全体の `OverrideKeyboardType` と `OverrideKeyboardSubtype` について、`before` = `intended` = `Absent` の記録を 1 つずつ持つ（`build_records` の後に、`standard_guards()` のキーで足す）。**書かない**（`write_forward` は CAS だけをする）。全体のステップの先頭に置くので、計画の後に固定モードになっていれば、書く直前の CAS で全体を 1 つも書かずに止まる。再起動の後、このままにするとき、取り消すときも、ペアが現れていれば既存の規則で `Conflict` になる（B.7）。baseline は作らない（MKLM がその値を変えたことにならないため。`create` は `before` と `intended` が同じ記録の baseline を書かない）。`Event::Planned` の手順と下見の書く値の一覧には出さない（技術的な詳細の `plan_text` に「Check: no global OverrideKeyboardType/Subtype (the PC stays in per-keyboard mode)」を 1 行出す）。
- **順序**: `check_plan` が i8042prt（この操作にはない）→ その他（固定）→ 全体の順に並べる。すべての固定をフラッシュしてから全体を書くので、全体の値が新しくなった状態では、固定はすべて書かれている。戻すときは逆（全体 → 固定を外す。B.6）。
- **反映**: 全体の値を書くので常に `PendingAction::RestartPc`（`apply_method` の `writes_global`）。固定の値は、「すぐに切り替える」ならその場のリセットで、そうでなければ再起動（か抜き差し）で読まれる。どちらでも配列は同じ。
- **変更がない場合**: `to` が今の標準配列と同じなら、何も書かない（下見は「今の標準配列です。」、エンジンは `NoChange`）。

### B.3 固定するキーボードの決め方

| デバイスノード | 扱い | 下見の「再起動の後」（B.13） |
|---|---|---|
| kbdhid、読み取り専用でない、follower（0x51/0、7/0 など） | 既定で固定する（接続中でなくても。3） | 固定: {from}（変わりません）。外した: {to} に変わります |
| kbdhid、読み取り専用でない、自分の表を持つ種類（4/0、7/2、NEC） | 何もしない | その配列（変わりません） |
| kbdhid か i8042prt、書き込める、値が片方だけ（`IncompletePair`。phantom を含む） | 操作を断る（6） | — |
| i8042prt、自分の表を持つ値で固定済み | 何もしない（INV-PS2） | その配列（変わりません） |
| i8042prt、固定値がない | 断る（`check_plan` の `PlanError::InvPs2`。今までどおり） | — |
| i8042prt、自分の表を持たない値で固定済み（7/0 など。phantom を含む） | 断る（`Ps2WithoutTable`。7） | — |
| 固定できないキーボード: 読み取り専用の kbdhid（`Transport::Virtual`） | 書かない（`check_device_writes` が拒む） | 保存値が自分の表を持たなければ「{to} になります（新しい標準に従います。MKLM は配列を割り当てられません）」、持てば「変わりません」 |
| 固定できないキーボード: ほかのドライバー（hyperkbd など） | 書かない | 「標準配列に従う場合は {to} になります（MKLM は配列を割り当てられません）」（どの種類を報告するか分からない） |
| RDP のキーボード（`is_remote_desktop`） | 書かない | 「{to} になることがあります（下の説明）」（C 章） |

1. **グループ**: 利用者が選ぶ単位は物理キーボード（`group_keyboards`）。`--follow` や GUI のチェックで選ばれた物理キーボードの follower（`physical_device_members` で展開する。内蔵と ContainerId のないものはそのキーボードだけ）には何も書かない。選ばれた物理キーボードに follower が 1 つもなければ `OperationError::NotFollowingStandard { instance_id }`。
2. **混在**: 同じグループに follower と割り当て済みのコレクションが混ざっていれば、follower だけを固定する（割り当て済みのものは触らない）。下見の行は「一部のコレクションだけ標準に従っています」と添える。
3. **接続していないキーボード**: follower なら既定で固定する。次に接続したときも今の配列のままにするため（原則 2）。シリアル番号のない USB キーボードを別のポートに差すと別のデバイスノードになり、元のものは phantom として残る（M0 #3b の逆の場合）ので、これも固定しておくと、元のポートに戻したときに配列が変わらない。phantom への書き込みは m2 I.3 の未検証の点を含むので、**SetStandard を公開する前に VM で確かめる**（F.3 の R-PHANTOM。H.4）。書けなかった場合は、そのキーボードの名前を出して何も変えずに終わり、再起動を求めない（B.6、UX-16）。GUI では「未接続のキーボード {n} 台」の開閉ボタンにまとめ、閉じているときも何を書くかを文で示す（B.12）。
4. **非表示のキーボード**（`settings.keyboards.hidden`）も同じく固定する。下見には「（非表示）」の印付きで出す（書く先を隠さない）。
5. **7/0 など MKLM が書かない種類を持つ follower**: 固定で 4/0 か 7/2 に置き換える（baseline に元の値が残る）。下見の技術的な詳細に「7/0 → 4/0」と出す。
6. **値が片方だけのもの**（`IncompletePair`。書き込める kbdhid と i8042prt。phantom を含む。SAFETY-6）: ドライバーが片方だけの値をどう読むかは確かめていない（`check_inv_ps2`、`predict_type` の注記）。kbdhid が組でなければ値を読まないなら、そのキーボードは今は標準に従っている。今どの配列で打っているかが分からないので、固定も「そのまま」も正しくできない。`OperationError::IncompleteValues { instance_ids }` で断り、そのキーボードの［変更…］（`set`。値を組で書く）を案内する。i8042prt の片方だけの値は INV-PS2 でも拒まれるが、案内を 1 つにするため、この判定を先に行う。読み取り専用のキーボードの片方だけの値は断らない（書かないので、B.3 の表の固定できないキーボードとして示す）。
7. **自分の表を持たない値で固定された PS/2**（phantom を含む。SAFETY-5）: 7/0 や 0x51/0 を手で書いた場合など（`keyboard_anomalies` は `UnverifiedType` / `UnexpectedType` として示す。MKLM の移行は 4/0 か 7/2 しか書かない）。INV-PS2 は満たすが、そのキーボードは標準配列に従うので、黙って配列が変わる。`OperationError::Ps2WithoutTable { instance_ids }` で断り、そのキーボードの［変更…］で JIS か US を割り当てるよう案内する（原則 3）。同じ操作で from に固定し直す案は採らなかった（J.9）。
8. **固定できないキーボード**（SAFETY-4）: 原則 2 が守れるのは、MKLM が値を書ける kbdhid と i8042prt のデバイスノードだけ。読み取り専用の kbdhid は保存値から配列を予想できるので、標準に従うものは「{to} になります」と示す。ほかのドライバーのキーボードはどの種類を報告するか分からないので「標準配列に従う場合は」と示す。リモート操作のソフトが送るキー（デバイスノードがない）も標準配列に従うことがある（H.11）。これらは確認の画面の注記（B.13）と、再起動の画面のパスワードの警告（`SetStandard` では常に出す。B.12）で扱う。

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
/// A kbdhid keyboard that is not read-only, stores both values of its pair or neither, and
/// whose stored values give it no table of its own: it types with the PC's standard layout
/// (design standard-layout B.3). A lone value (`IncompletePair`) is no follower: the operation
/// refuses it.
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
    /// Keyboards MKLM cannot pin (read-only kbdhid, other drivers; not the Remote Desktop
    /// keyboard) and whether their stored values make them follow the standard (`None`:
    /// another driver, whose type is unknown).
    pub not_assignable: Vec<(String, Option<bool>)>,
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

/// The global values a standard change records without writing them (`before` = `intended`
/// = `Absent`): the fixed-mode pair, so that a pair written later by anyone else is a conflict
/// (design standard-layout B.2).
pub fn standard_guards() -> [ValueKey; 2];
```

拒否（どれも何も書かない。判定はこの順）:

| 場合 | エラー | 前面の案内（B.13、B.11） |
|---|---|---|
| 固定モード（`global.mode() == Fixed`） | `MigrationRequired { fixed: ps2_pin_layout(global) }`（既存） | 「固定モードでは、標準配列がすべてのキーボードの配列です。キーボードごとモードへ移行するときに選べます。」（CLI: `mklm-cli migrate --standard <jis\|us>`） |
| 今の標準配列が分からない（`stored_standard` が `None`: 値がない、kbd106n や kbdnec など、識別子と合わない） | `UnknownStandard { layer_driver, identifier }`（新規） | 「標準配列の値が、MKLM の知っている形ではありません。…キーボードごとの配列（各行の［変更…］）は今までどおり変えられます。」。技術的な詳細に今の値（UX-14） |
| 書き込める kbdhid か i8042prt に値が片方だけ（phantom を含む） | `IncompleteValues { instance_ids }`（新規。SAFETY-6） | 「{name} には、配列を決める 2 つの値のうち片方しかありません。…先にそのキーボードの［変更…］で配列を割り当ててください…」 |
| 自分の表を持たない値で固定された PS/2（phantom を含む） | `Ps2WithoutTable { instance_ids }`（新規。SAFETY-5） | 「{name}（PS/2）の値は、配列を決めない種類です。…先にそのキーボードの［変更…］で JIS か US を割り当ててください（PC の再起動が必要です）。」 |
| `--follow` のキーボードが見つからない | `UnknownKeyboard`（既存） | 既存 |
| `--follow` の物理キーボードに follower がない（割り当て済み、PS/2、RDP、固定できないキーボード） | `NotFollowingStandard { instance_id }`（新規） | CLI だけ（GUI のチェックは follower の行にしかない）。文は B.11（UX-14） |
| 固定値のない PS/2 がある | `Plan(PlanError::InvPs2)`（既存） | 既存（固定すべきキーボードの一覧） |
| 新しい `LayerDriver JPN` の DLL が System32 にない | `LayerDriverMissing`（既存。エンジンと CLI が確かめる。m2 S8） | 既存 |

`to == from` はエラーにせず、書き込みのない `StandardChange` を返す（前面は「今の標準配列です。」）。

エンジンの `error.rs` は、4 つの新しい変種を `ErrorCode::PlanRejected`（`plan_error` なし）に写す（`InconsistentGlobal` と同じ扱い）。前面は同じ関数で先に計画するので、ふつうは helper まで届かない。GUI の `i18n` は `OperationError` を網羅的な `match` で訳しているので、変種を足すとコンパイルが通らなくなり、訳の漏れに気付く（m3 D.4）。

### B.5 エンジンの手順（m2 に「D.13 標準配列の変更」として足す）

入力（`SetStandardParams`）: `standard: Layout`、`follow: Vec<String>`、`apply: ApplyOptions`（固定した follower のその場のリセットだけに使う。操作そのものは常に再起動で効く。カウントダウンはしないので `countdown_seconds` は使わないが、ほかの要求と同じく最初に `check_countdown` で 20 か 60 であることを確かめる）、`expected: Option<ExpectedPlan>`。

1. m2 D.1（ロック、ジャーナル、`Gate::NewOp`: open なエントリがあれば `OpInProgress`、読み直し）。
2. `plan_set_standard(&s.keyboards, &s.global, params.standard, &params.follow)`（下見と同じ関数）。
3. 全体の `LayerDriver JPN` の DLL が System32 にあることを `Host::system32_file_exists` で確かめる（`migrate` と同じコード。関数に切り出して 2 か所で使う）。
4. `check_expected`（`steps` と `apply`。下見の後に follower が増減していれば `PlanChanged` で何も書かない）。
5. `context` に、`migrate` と同じく `i8042prt\Parameters` の全値と全キーボードの `Device Parameters` の全値を記録する（`snapshot_context`。PC 全体の値を変えるので、サポートのために残す）。
6. `build_records` に、確認用の記録（`standard_guards()`。今の値を読んで `before` = `intended` = `Absent`）を全体のステップの先頭として足す。値を変える記録がなければ（確認用の記録だけなら）`NoChange`。
7. 新しい標準に従うキーボードがあり、`to == Jis` なら `STANDARD_UNVERIFIED` の警告を足す（標準が US のときの 0x51 は MT-1 で打鍵を確かめた。JIS はまだ。m0-results.md）。
8. `expected_keyboards(…, None)`（すべての kbdhid と i8042prt。固定したものは `changes: false`、新しい標準に従うものと、読み取り専用の kbdhid で標準に従うものは `changes: true`）。ほかのドライバーのキーボードは対象にしない（下見の表と確認の注記で示す）。
9. `create(OpKind::SetStandard { from, to, keyboards }, records, Some(RestartPc), context, keyboards)`。起動時の値を変えるので、復旧用ファイルを耐久的に書けなければ `RecoveryAssetsUnavailable` で何も書かない（m2 C10）。確認用の記録の baseline は書かない。
10. `write_new`（確認用の記録は CAS だけ。失敗の扱いは B.6）→ `Written`。
11. **固定した follower のその場のリセット**（SAFETY-2）: `apply.allow_live_reset` と `apply.other_input_available` がどちらも true で、`check_cancelled()` が false なら、固定した follower のうち、接続中で `live_reset_bans(kb, false)` が空のものを 1 台ずつリセットする（`reset_keyboards(…, stop_on_failure: false)`）。状態は `Written` のままで、`Restarting` にもカウントダウンにもしない（固定の値は今の表と同じなので、打つ配列は変わらない。確かめることがない）。戻らなかったキーボードは警告だけで続ける（固定は再起動で効くので、ロールバックしない）。途中でクラッシュしても、エントリは `Written` ですべての値が `intended` なので、回復は `RollForward(PendingReboot)`（同じ起動）になる。BLE / BT（`UnprovenTransport`）と接続していないキーボードはリセットしない（再起動で効く。H.3 の窓が残る）。
12. `apply_change` → `PendingReboot`（`apply_pending` は、リセットして戻ったキーボードを除く HID の follower。`close_pending` に `reapplied` を渡す）→ 結果 `PendingReboot`、`pending_action = RestartPc`。リセットで戻らなかったキーボードは結果の警告に入り、前面が B.13 の文で示す。

`OpKind::SetStandard.keyboards` は `(インスタンス ID, LayoutChoice)` の並び。固定したデバイスノードは `(id, from の Jis / Us)`、新しい標準に従わせたデバイスノード（接続中でないものも）は `(id, Standard)`。

### B.6 途中で止まった場合

**書くとき**

| 止まった所 | 永続化されている状態 | 回復（m2 C.7 の表のまま。`SetStandard` は `EntryKind::Change`） | 打つ配列（PC を再起動した後） |
|---|---|---|---|
| `Planned` の FJ の前 | エントリなし（復旧用ファイルは書き終えている） | 何も起きていない | 変わらない |
| 固定のステップの途中 | 一部の固定。全体は元のまま | 混在 → `RollBack(PartiallyWritten)` → 全体（書かれていないので何もしない）→ 固定を外す → `RevertedPendingReboot`（回復は電源断の前に何が読まれたかを知らないので、m2 I.12 のとおり保守的に再起動を求める） | 変わらない（固定は今の配列と同じ。全体は元のまま） |
| すべて書いた後、`Written` の前 | すべて `intended` | 同じ起動なら `RollForward(PendingReboot)`、再起動の後なら `RollForward(AwaitingConfirm)`（再起動後の確認へ）。固定値のない PS/2 が現れていれば `RollBack(InvPs2)` | 固定したキーボードは変わらない |
| 固定した follower のリセットの途中 | すべて `intended`、`Written` | 同じ起動なら `RollForward(PendingReboot)` | 変わらない |
| 電源断 | ハイブごとに上のどれかの先頭部分 | 同じ分類 | 同じ |

**書き込みの失敗**（プロセスの中。クラッシュではない。SAFETY-8、UX-16）

- **固定の書き込みが `DeviceRemoved`**（計画の後に devnode が消えた）: その記録を `skipped = DeviceRemoved` にして続ける（固定する相手がもういない）。`SetStandard` の HID の記録だけに当てはめる（ほかの操作の意味は変えない）。
- **それ以外の失敗**（`AccessDenied` を 1 回やり直しても失敗、phantom のハードウェア キーを書き込みで開けない、など）: 今までどおりロールバックする。`write_forward` は `Forward::Failed { message, record, wrote_boot_time }` を返す。**起動時の値を 1 つも書いていない**（全体のステップより前で止まった）なら、終わりは `Reverted` で再起動を求めない。エンジンは自分が何を書いたかを知っており、m2 I.12 の「電源断の後の起動で読まれているかもしれない」は、書いていない値には当てはまらない。全体のステップの途中か後で失敗したときは、今までどおり `RevertedPendingReboot`。どちらも `failure = WriteError { message, target }`（`target` は失敗した記録の対象）。前面は `target` からキーボードの名前を出し、再起動を勧めない（B.13）。この規則はほかの操作にも当てはまる（起動時の値を書く前に失敗した場合だけ `Reverted`。`Migrate` は PS/2 の固定から書くので、ふつうは今までどおり）。
- **確認用の記録の CAS が合わない**（計画の後に、設定アプリなどが固定モードにした）: 全体を 1 つも書かずに `Forward::Failed` になる。ロールバックは全体のステップ（確認用の記録を含む）から始まり（下の段階の順）、確認用の記録は外部のペアを消さないので CAS が合わずに止まる → `Conflict`。固定は外さない（固定モードでは固定は効かず、利用者がペアを消して解決すれば元の配列のまま）。

**戻すとき**（取り消し、ロールバック、undo、解決、回復の続き。SAFETY-1）

`plan_restore` の段階に `FollowStandard` を足す（`GlobalWithoutPair` と `RemovePins` の間）。

| 段階（`RestorePhase`） | 含むステップ |
|---|---|
| 1. `AddPins` | 今のまま |
| 2. `GlobalWithPair` | 今のまま |
| 3. `Other` | HID などそれ以外のキーボード。ただし下の 4b に入るものを除く |
| 4. `GlobalWithoutPair` | 今のまま |
| 4b. `FollowStandard`（新規） | **キーボードごとモードのままの戻し**で、書いた後に自分の表を持たない HID のキーボード（固定を外す、7/0 に戻す） |
| 5. `RemovePins` | 今のまま |

- **キーボードごとモードのままの戻し**: 全体のペアが、計画の前にも後にもないもの。計画の前は、全体のペアの記録があればその `Expect` の値（`Expect::Any` なら今の値）で、なければ今の値で見る。後は、今の値に戻す値を当てたもので見る。前を `Expect` で見るのは、外部の変更で今の値が変わっていても、ジャーナルが想定した順で書くため（`SetStandard` の取り消しでは確認用の記録の `Expect` が `Absent` なので、固定モードが現れていても全体のステップが先になり、その CAS の不一致で止まる。固定は 1 つも外さない）。
- 「書いた後に自分の表を持たない」は、`predict_type(Kbdhid, 書いた後の値, global).per_keyboard_table()` が `None`（devnode が消えていれば既定の値に書く値を当てたもの）。
- HID の値は INV-PS2 に関わらないので、INV-PS2 の確かめ（各ステップの後の `check_inv_ps2`）は変わらない。
- **固定モードへ出入りする戻し**（移行の取り消し、固定モードへの「導入前に戻す」とその取り消し、移行を残す解決）は今の順のまま。ペアがある間は HID の値が効かないので、HID を先に書けば、配列の状態は前と後の 2 つだけになる。
- これで `SetStandard` の取り消しとロールバックは前向きの順の逆（全体 → 固定を外す）になり、m2 C.5 の「取り消しとロールバックでは、これは結果として前向きの順の逆になる」が成り立つ。m2 C.5 の段階の表と文は WP-S9 で直す（直す文: 表に「4b. `FollowStandard` | キーボードごとモードのままの戻し（全体のペアが前にも後にもない。前はペアの記録の `Expect` で見る）で、書いた後に自分の表を持たない HID のキーボード」を足し、文の終わりに「`SetStandard` の固定は全体の後に外す（standard-layout B.6）」を足す）。

| 止まった所（戻すとき） | 永続化されている状態 | 回復 | 打つ配列（PC を再起動した後） |
|---|---|---|---|
| `RevertPending` の FJ の前 | 戻す前（固定＋新しい全体） | 元の状態のまま（`PendingReboot` / `AwaitingConfirm` / `Confirmed`） | 固定したものは from、新しい標準に従わせたものは to |
| 全体のステップの途中 | 固定あり。全体は一部だけ戻った | `ContinueRevert`（全体を書き切ってから固定を外す） | 固定したものは from（自分の表） |
| 固定を外す途中 | 全体は from。固定は一部 | `ContinueRevert` | 外したものは標準（from）に従い、残りは自分の表（from）。どちらも from |
| 全体のキーが書き込みを拒む（I7。1 回やり直しても失敗） | 固定あり。全体は to | `Conflict`（`write_error`）。固定は 1 つも外していない | 固定したものは from |
| 全体の値が外部で変わっていた（CAS の不一致。確認用の記録を含む） | 同上 | `Conflict` | 同上 |

**`undo_conflict`**（衝突した `SetStandard` の undo。m2 D.10）: 今の undo は衝突している記録を除いて戻すので、全体の記録が衝突していると、書けるのは固定だけになり、固定だけが消える。そこで、`SetStandard` のエントリで全体の記録（標準配列の 2 つと確認用の 2 つ）のどれかが衝突していれば、そのエントリには何も書かず `Conflict` のまま残し、警告する（「the standard layout was changed outside MKLM: resolve the conflict instead」。GUI は衝突の画面へ）。固定の記録だけが衝突している場合は、書ける記録を段階の順（全体 → 固定を外す）で戻す。回復の `ContinueRevert` が衝突の undo を続けるとき（`reverts_a_conflict`）も同じ規則にする（`RevertPending` に入った後で全体の値が外部で変わった場合）。

**衝突の解決**（m2 D.8）: 利用者が値ごとに選ぶ。書く順は上の段階の順なので、全体の値が書けなければ（CAS の不一致、書き込みエラー）その後の固定を外すステップは書かれない。全体の値を「今の値のまま」にして固定を外す組み合わせは、衝突の画面で利用者が両方の行を明示して選んだときだけ起きる（固定の行は衝突していなければ出ない）。

**打つ配列が変わらないこと（I8）**: 書くときは固定をすべてフラッシュしてから全体を書き、戻すときは全体を書き終えてから固定を外す（C.5 のステップごとの FT と、書けなかった値のあるステップの後は書かない規則）。したがって、どの永続状態でも「全体が固定の前提と違うのに、固定のない follower」はない。新しい標準に従わせたキーボードを除き、どの状態から再起動しても、各キーボードの予想される表は操作の前と同じ。これを crash の網羅テストの新しい不変条件 I8 にする（F 章）。I8 の対象は MKLM が書ける kbdhid と i8042prt のデバイスノードで、固定できないキーボードと RDP のキーボードは対象の外（テストで明示する。B.3 の 8）。

### B.7 再起動の後: 確認、確定、取り消し、undo、導入前に戻す

- **再起動後の確認**: `post_reboot_entries` に入る（`PendingReboot`）。`SetStandard` の確認の行は、**配列が変わる物理キーボードだけ**を、物理キーボードごとに 1 行で並べる（新しい標準に従わせたものと、読み取り専用の kbdhid で標準に従うもの。`preview::check_groups`。UX-6）。固定したキーボードは配列が変わらないので、打鍵の確認を求めない（Raw Input の確かめはエンジンの `confirm` が行い、保存値どおりでなければ警告する。D.2）。注記は B.13 の「確認の注記」の 1 つの版（配列が変わるキーボードがあるとき / ないとき）。ないときは RDP の但し書きを添え、リモートのセッションなら「ここで［このままにする］を選んでかまいません」を出す（UX-2、UX-6）。
- **このままにする**: `Request::Confirm`。エンジンは `RebootObserved`（INV-PS2 の確認を含む）→ `AwaitingConfirm` → 値の確認 → `Confirmed`（m2 D.6）。Raw Input の確認は D.2 のとおり。
- **固定モードが現れた場合**（SAFETY-7）: 書いてから確定までの間に、設定アプリなどが全体の `OverrideKeyboardType/Subtype` を書くと（m2 I.9。例: 設定アプリの「英語キーボード」で kbd101、`PCAT_101KEY`、7/0）、固定モードがすべての固定を無視する。確認用の記録の今の値が `Absent` でなくなるので、再起動の後の `RebootObserved` は `Elsewhere` で `Conflict`、`AwaitingConfirm` の `confirm` は「値が `intended` でない」で `Conflict`、取り消しは CAS の不一致で `Conflict` になる（どれも既存の規則のまま。古いビルドも同じ）。衝突の画面（`vm::conflict`）の全体の行に B.13 の「固定モードになりました」の注記を出し、おすすめは「変更前の値に戻す」（ペアを消し、キーボードごとモードに戻す。操作の意味を保つ）。「今の値のまま」を選べば固定モードのまま、操作は `Failed(ConflictKeptCurrent)`。
- **元に戻す / undo**: m2 D.4、D.10 の手順で、`plan_restore` の段階は B.6（全体 → 固定を外す）。**取り消しの後に HID のキーボードをリセットしない**（m2 D.4 の手順 9 と D.8 の手順 5 の例外。SAFETY-3）。この起動のセッションは、サインインのときに読んだ `LayerDriver JPN`（新しい to）を使い続けるので、固定を消した後にリセットすると、kbdhid は 0x51 を報告し、そのキーボードは再起動まで to で打つ（「すぐに元の配列に戻す」の逆になる）。そこで、**全体の記録を持つ操作**（`SetStandard`、標準配列を変えた `Migrate`、全体の値を含む `RestoreBaseline`）の取り消し、undo、解決では、`reapply` はリセットしない（`ApplyOptions` によらない。`revert_entry`、`undo_conflict`、`close_resolution`）。元の配列は PC の再起動で戻る。
  - GUI: 履歴の［元に戻す…］（`revert_page`）、「確認待ちの変更をすべて元に戻す」（`undo_page`）、回復の画面（`recovery_page`）は、そのエントリについて切り替え方の 2 択を出さず、B.13 の「PC の再起動で元の配列に戻ります」を出す（`touches_hid(entry) && !touches_global(entry)` のときだけ 2 択を出す）。
  - CLI: `revert` と `undo` は、`--other-input` を受け取っても、対象に全体の記録を持つ操作があればその理由を 1 行表示する（B.11）。
  - 今の標準配列を変えた移行（`d724c149`、`271b6909` の形）の取り消しにも同じ欠陥があるので、SetStandard を待たずに直す（G 章の WP-S10）。
  - その場でリセットした固定（B.5 の 11）を、同じ起動の中で取り消した場合: そのキーボードは固定の種類（from の表）を報告し続け、保存値は「標準（from）に従う」になる。どちらも from なので打つ配列は同じで、`RevertedPendingReboot` の再起動で保存値どおりになる。
- **後から戻す**: 取り消せるのは、この操作がすべての値の最新の記録のときだけ（m2 C.8）。後で固定したキーボードを `set` で変えると `NotLatest` になる。その場合は、標準配列をもう一度選び直す（新しい `SetStandard`。そのときの follower が新たに固定される）。GUI の履歴は［元に戻す…］を出さない（既存の `revertible`）。
- **導入前に戻す**: 変えない（baseline は最初の値。この PC では `LayerDriver JPN` の baseline は kbd101.dll、全体のペアは 7/0 で、`restore --baseline --all` は固定モード（英語）に戻す。確認用の記録は baseline を作らないので、ペアの baseline は最初の移行が記録した 7/0 のまま）。

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

（この PC で標準配列を JIS から US にし、標準に従う 3 つのデバイスノードを JIS に固定した場合の例。記録は固定 3 × 2、全体の 2（`LayerDriver JPN`、`OverrideKeyboardIdentifier`）、確認用の記録 2（全体の `OverrideKeyboardType/Subtype`。`before` = `intended` = `{"kind": "absent"}`）。確認用の記録は全体のステップの先頭。）

- 実装: `OpKind` に `#[serde(from = "OpKindWire", into = "OpKindWire")]`。`OpKindWire` は今の 4 つの変種と同じ形で、`Migrate` にだけ `#[serde(default, skip_serializing_if = "Option::is_none")] set_standard: Option<SetStandardMark>`（`SetStandardMark { from: Layout }`）を持つ。印があれば `SetStandard { from, to: standard, keyboards: assignments }`、なければ `Migrate`。`OpKind::schema_version` は `SetStandard` に 1 を返す。
- **印を失った場合の補い**: 0.1.x / 0.2.0 がこのエントリを書き直すと（取り消し、確定、`apply_pending` の掃除など、どの遷移でも）、印は消える（古いビルドの `OpKind::Migrate` は `standard` と `assignments` しか持たない）。そこで `JournalEntry::from_json` は、`Migrate` のうち、記録の全体の `OverrideKeyboardType/Subtype` が**ないか、確認用の記録（`before` = `intended` = `Absent`）だけで**、全体の `LayerDriver JPN` の記録があるものを `SetStandard` と読み直す（`from` はその記録の `before`。読めなければ `Migrate` のまま）。本物の移行は固定モードから始まるので、全体のペアの削除の記録（`before` が値を持つ）が必ずある（`ps2_pin_layout` が両方の値を求め、削除は必ず違う）。次に新しいビルドがこのエントリを書くと、印が戻る。
- **`FailureReason::WriteError` の `target`**: `#[serde(default, skip_serializing_if = "Option::is_none")] target: Option<WriteTarget>`。古いビルドは知らないフィールドを無視する（`FailureReason` に `deny_unknown_fields` はない）。

**古いビルドから見た動き**（0.1.0 と 0.2.0。どちらも `OpKind` と `FailureReason` は `deny_unknown_fields` を持たないので、`set_standard` と `target` は無視される）:

| 場面 | 動き | 正しさ |
|---|---|---|
| 読む | `Migrate { standard: to, assignments }` として読む。`unreadable` にならない | 書き込みも M5b の更新（`check_journal`）も止まらない |
| 確認用の記録 | 値を変えない記録として読む（`observe` は `Unchanged`。固定モードが現れれば `Elsewhere` で `Conflict`）。取り消しでも書かない（今の値が戻す値と同じ） | 正しい |
| 回復、確定、導入前に戻す | どれも記録だけで決まる（`decide_recovery`、`latest_record`） | 正しい |
| 取り消しと undo の順序 | 古いビルドは段階 `FollowStandard` を知らないので、固定を外してから標準配列を戻す（B.6 の前の順）。途中で止まると、固定していたキーボードが再起動の後に新しい標準配列で打つおそれがある。取り消しの後に HID のキーボードをリセットしうる（B.7 の欠陥） | **ダウングレードの注意**に書く（下） |
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

**ダウングレードの注意**（m2 C.10 の注意に足す）: 標準配列の変更が再起動を待っている間に古いビルドを入れると、古いビルドはそれを移行として扱う。同じ起動の中で 0.1.x に戻すと、起動 ID の判定の注意（m2 C.10）がそのまま当てはまる。**標準配列の変更を古いビルドで取り消さない**（上の表の順序とリセット）: ダウングレードする前に、新しいビルドで「このままにする」か「元に戻す」を済ませる。

**文書**: m2 C.3 の `kind` と記録の行（確認用の記録）、C.5（段階 `FollowStandard`）、C.10 の版の段落（「`SetStandard`（版 1、`migrate` の形と印）」）、D.4 の手順 9 と D.8 の手順 5（全体の記録を持つ操作はリセットしない）、D.13（B.5）、E.6 の要求の表を直す（WP-S9）。

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
    /// Only the in-place reset of the pinned followers (B.5 step 11); the change itself always
    /// takes effect at a PC restart. The caller's declaration, as for `SetLayoutRequest`.
    pub apply: ApplyOptions,
    /// See [`SetLayoutRequest::expected`].
    pub expected: Option<ExpectedPlan>,
}
// Request::SetStandard(SetStandardRequest)
```

- `PROTOCOL_VERSION` を 4 にする（m2 A.5 の規則）。呼び出し元と helper は同じビルド ID を求めるので、版の違う組み合わせは起きない。M5b の更新の要求（m5b D.3）は、入っている版の GUI と helper の間で行うので、版を上げても影響しない。
- helper の `dispatch`: `Request::SetStandard(r)` → `engine.set_standard(&SetStandardParams { standard: r.standard, follow: r.follow.clone(), apply: r.apply, expected: r.expected.clone() }, sink)`。
- helper は呼び出し元の `follow` のインスタンス ID を、自分の列挙を引くキーとしてだけ使う（m2 E.6）。
- `messages.rs` の JSON の形のスナップショットに `set-standard` を足す。

### B.10 共有クライアント

- `session::plans_first`: `Request::SetStandard` は `Planned` を先に書く要求（取り消しの扱いは `Migrate` と同じ。m3 A.2.2）。
- `gate`: `Gate::NewOp`（`set`、`migrate` と同じ）。
- `preview::check_rows`: `OpKind::SetStandard` は、`keyboards` の `Standard`（新しい標準に従わせたもの）と、読み取り専用の kbdhid で標準に従うもの（接続中のもの）。
- `preview::check_groups(snapshot, entry) -> Vec<CheckGroup>`（新規。D.2）: `check_rows` を物理キーボード（`group_keyboards`）ごとにまとめる。「Windows の認識」は、メンバーがすべて一致すれば ✓、1 つでも違えば ⚠、見えなければ D.2 の文。GUI の確認の表、CLI の確認の表、「このままにする」の後の名前の一覧はこれを使う（どの種類の操作でも。2.4G Wireless Device の 2 つのコレクションは 1 行になる）。
- `preview::standard_rows(snapshot, plan: &StandardPlan) -> Vec<StandardRow>`: 下見の表の中身（CLI と GUI が共有）。

```rust
pub struct StandardRow {
    /// The group's ID (container ID, else the first instance ID), as the main screen's rows.
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub present: bool,
    pub hidden: bool,
    /// What its stored values give it now (`effective_layout` of today's values, the main
    /// screen's "設定した配列"), and after the plan (`None` when no table is known, e.g. the
    /// Remote Desktop keyboard or another driver's keyboard).
    pub before: Option<EffectiveLayout>,
    pub after: Option<LayoutTable>,
    pub role: StandardRole,
    /// A pinned follower that is connected and may be reset in place (no structural ban): the
    /// switch-now choice applies to it (B.5 step 11, B.12).
    pub can_reset: bool,
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
    /// The Remote Desktop keyboard (C).
    RemoteDesktop,
    /// MKLM cannot write it (read-only kbdhid, another driver). `follows`: its stored values
    /// make it follow the standard (`Some(true)`) or give it a table (`Some(false)`); `None`
    /// for another driver, whose type is unknown.
    NotAssignable { follows: Option<bool> },
}
```

「設定した配列（今）」は Raw Input ではなく保存値から予想する（RDP のセッションでは Raw Input が物理キーボードを並べないため。D.2）。メイン画面と同じ `effective_layout` の文にする（UX-12）。

### B.11 CLI: `mklm-cli standard`

```text
mklm-cli standard <jis|us> [--follow <keyboard>]... [--other-input | --no-reset] [--dry-run] [--yes]
```

| 引数 | 意味 |
|---|---|
| `<jis\|us>` | 再起動の後の PC の標準配列（`StandardArg`） |
| `--follow <keyboard>` | この物理キーボードは今の配列のままにせず、新しい標準配列に従わせる（再起動で配列が変わる）。繰り返せる。`<keyboard>` はインスタンス ID か `#n` |
| `--other-input` / `--no-reset` | 固定する follower をその場でリセットしてよいか（`ApplyArgs`。`set` と同じ）。どちらもなく、リセットできるキーボードがあれば尋ねる（`OTHER_INPUT_QUESTION`） |
| `--dry-run` | 下見を表示して終わる。helper を起動せず、何も書かない |
| `--yes` | 確認を省く（UAC は出る）。`--other-input` か `--no-reset` が要り、`#n` とは組み合わせられない（使い方の誤り、終了コード 2。m2 S12） |

**`#n` の規則**（m2 F.1 のまま）: `#n` は `mklm-cli list`（接続中のキーボードだけ、`list` と同じ並び）の行番号。接続していないキーボードはインスタンス ID で指定する。解決したキーボードは、名前とインスタンス ID をそれぞれ表示してから進む。PowerShell では `"#2"` のように引用符で囲む（`#` が注釈になる）。`--follow` が follower のない物理キーボードを指したら、次の文で終わる（終了コード 1。UX-14）:

| 指したもの | 文 |
|---|---|
| 割り当て済み | "Keychron Receiver does not follow the standard (US is assigned). Leave it out of --follow; to let it follow the new standard later, run `mklm-cli set <keyboard> --layout standard` after the restart." |
| PS/2 | "標準 PS/2 キーボード is a PS/2 keyboard, which always keeps a layout of its own. Leave it out of --follow." |
| RDP | "The Remote Desktop keyboard cannot be changed. Leave it out of --follow." |
| 固定できないキーボード | "{name} is not a keyboard MKLM can assign a layout to. Leave it out of --follow." |

**流れ**（`migrate` と `set` の流れを基にする。`commands.rs`）:

1. `start`（OS、`--dry-run` でなければ RunOnce の規則、ジャーナル、起動 ID）→ `inventory()`（書き込みを止める読み取りの問題があれば止める）。
2. `--follow` を 1 つずつ解決して表示する。
3. `gate(Gate::NewOp)`（ふさがっていれば終了コード 6。`--dry-run` なら注記だけ）。
4. `plan_set_standard`。エラーは B.4 の案内（CLI の英語の文。`UnknownStandard` には "Layouts per keyboard can still be changed with `mklm-cli set`." を添え、今の値を表示する）。`to == from` なら「Nothing to change: the PC's standard layout already is JIS.」で 0。
5. DLL の確認（`migrate` と共通の関数）。
6. 固定する follower にその場でリセットできるものがあり、`--other-input` も `--no-reset` もなければ、"The keyboards that follow the standard now (2.4G Wireless Device, VXE Mouse 1K Dongle) can be reset in place so that the layout assigned to them takes effect at once; what they type does not change, and each stops for a few seconds." と表示して `OTHER_INPUT_QUESTION` を尋ねる（`--dry-run` では尋ねず、答えで何が変わるかを表示する）。
7. 下見を表示する（下の例）。RDP のキーボードがあるか、RDP のセッションなら C.3 の注記。標準に従わせるキーボードがあり `to` が JIS なら、打鍵が未確認の注記。
8. `--dry-run` なら `dry_run_done`。そうでなければ UAC の説明と「Continue? [y/N]」（`--yes` で省く）。
9. `Request::SetStandard { standard, follow: 解決したインスタンス ID, apply, expected: Some(preview::expected(&plan.plan)) }` を `run_request` で送る（`After::Standard`: `PendingReboot` なら "After the restart, MKLM (or `mklm-cli post-reboot`) asks whether to keep the change." と "Restart the PC now (Restart, not Shut down). Signing out, switching users or shutting down before the restart may change layouts early (not verified)."）。昇格した CLI の RunOnce の案内は D.3。

**下見の例**（この PC、JIS → US、`--dry-run --other-input`）:

```text
The PC's standard layout: JIS -> US (takes effect at the next PC restart: Restart, not Shut down)
Keyboards (Stored: the layout their stored values give them):
  Keyboard                     Stored                      After  What happens
  標準 PS/2 キーボード          US                          US     PS/2: assigned (unchanged)
  USB Keyboard                 US                          US     assigned (unchanged)
  2.4G Wireless Device         follows the standard (JIS)  JIS    JIS is assigned (unchanged); reset now
  Keychron Receiver            US                          US     assigned (unchanged)
  VXE Mouse 1K Dongle          follows the standard (JIS)  JIS    JIS is assigned (unchanged); reset now
  リモート デスクトップ ...     -                           -      Remote Desktop keyboard: may become US (see the note)
Planned changes (they apply to every user of this PC):
  1. HKLM\SYSTEM\CurrentControlSet\Enum\HID\VID_1D57&PID_FA60&MI_00\7&14A99BDA&0&0000\Device Parameters
       KeyboardTypeOverride     (none) -> 7
       KeyboardSubtypeOverride  (none) -> 2
  …
  4. HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters
       LayerDriver JPN             "kbd106.dll" -> "kbd101.dll"
       OverrideKeyboardIdentifier  "PCAT_106KEY" -> "PCAT_101KEY"
  Check: no global OverrideKeyboardType/Subtype (the PC stays in per-keyboard mode)
Takes effect: at the next PC restart (Restart, not Shut down). Signing out, switching users or
shutting down before the restart may change layouts early (not verified).
INV-PS2: holds after every step
Layout driver kbd101.dll: present in System32.
To let a keyboard switch with the standard instead, add --follow <keyboard>.
```

固定できないキーボードの行は "cannot be assigned: becomes US (it follows the new standard)" か "cannot be assigned: becomes US if it follows the standard"。新しい標準に従わせた行は "follows the new standard: becomes US"。

**終了コード**（m2 F.5 のまま）: 0 変更なし・下見だけ（下見でふさがっていれば、`migrate` と同じくその終了コード）、1 失敗・拒否、2 使い方の誤り、3 取り消した、5 衝突、6 止められた、3010 PC の再起動が必要（ふつうの成功）。

**置き場所**: 最上位のコマンド `Command::Standard(write::StandardArgs)`（ヘルプ: "Change the PC's standard layout (per-keyboard mode; one PC restart). Keyboards that follow the standard now keep their layout unless given with --follow."）。`global` の下には足さない（`global` は読み取りだけのまま。`global status` が結果を示す）。

**ほかの CLI の変更**:

- `migrate` がキーボードごとモードで何もしないとき（今は「Nothing to migrate」）に、`--standard` が今の標準配列と違えば、「The PC is already in per-keyboard mode; to change its standard layout use `mklm-cli standard <jis|us>`.」と表示して 1 で終わる（頼まれた標準配列にならなかったのに 0 を返さない）。`--standard` がないか同じなら今までどおり 0。`operation_error` の `NotFixedMode` の案内にも `standard` を足す。
- `Command::waits_for_updates` に `Standard` を足す（書き込みのコマンドなので、更新の間は 6 で止まる。m5b D.14）。
- `render::kind_text`: `SetStandard` は「set the PC's standard layout to US (was JIS); JIS assigned to `<id>`, …; `<id>` follows the standard」。
- `revert` と `undo`（SAFETY-3）: 対象に全体の記録を持つ操作があれば、今の「Keyboards are not reset in place (add --other-input to allow it)…」の代わりに "Keyboards are not reset in place for {op}: it changed the PC's standard layout, which Windows reads at sign-in, so the old layouts come back at the next PC restart (Restart, not Shut down)." を表示する（`--other-input` があっても同じ。エンジンもリセットしない）。

### B.12 GUI

**入口**（どれもキーボードごとモードのときだけ。同じ操作のボタンの名前はどこでも［PC の標準配列を変更…］。UX-8）:

1. メイン画面: 状態行の下の独立した行に［PC の標準配列を変更…］（`mode_note` と同じ置き方。UX-9）。状態行の 4 つの項目（`StatusItem`）は文字だけのまま（1 つの `text` の読み上げ、折り返す値）。書き込みがふさがっている間（`StartupSummary::blocks_writes`）はボタンを無効にし、横に `i18n::cannot_change_now` の文を見える形で出し、同じ文をボタンの `accessible-description` にする（無効のボタンは Tab で止まらないため、理由は画面に出す）。
2. 設定ページ: 行「PC の標準配列: JIS」＋［PC の標準配列を変更…］。固定モードでは B.13 の固定モードの文を出し、ボタンは出さない。
3. リモート デスクトップのページ（C.4）の［PC の標準配列を変更…］。

初回セットアップ（m3 B.1 の手順 4）は変えない。キーボードごとモードでは標準配列を読み取り専用で示し、「変えるには、設定の［PC の標準配列を変更…］を使います。」と添えるだけにする（ウィザードの変更は 1 台ずつの `set` のままにする）。

**ページ**（`Page::Standard`。ナビゲーションにはない。m3 B.0 のページの作りのとおり、題 / 本文 / ボタンの行）:

```text
┌ PC の標準配列 ────────────────────────────────────────────────────┐
│ PC の標準配列は、配列を割り当てていないキーボード（標準に従うキーボー│
│ ド）が使う配列です。                                                │
│ ○ JIS（日本語 106/109 キー）（現在）                                │
│ ◉ US（英語 101/102 キー）                                           │
│ PC の再起動の後は、次のようになります。標準に従っているキーボードに │
│ は、先に今の配列を割り当てるので、PC を再起動した後も配列は変わりま │
│ せん。                                                              │
│ 標準 PS/2 キーボード                                                │
│   設定した配列（今）: US → 再起動の後: US（変わりません）           │
│ 2.4G Wireless Device                                                │
│   設定した配列（今）: 標準に従う（JIS）→ 再起動の後: JIS（変わりませ│
│   ん。今の配列を割り当てます）                                      │
│   ☑ 今の配列のままにする                                            │
│ …                                                                   │
│ リモート デスクトップ                                               │
│   設定した配列（今）: — → 再起動の後: US になることがあります（下の │
│   説明）                                                            │
│   接続元の報告: 日本語キーボード (JIS)。このセッションのキー配列と同│
│   じとは限りません                                                  │
│ [▸ 未接続のキーボード 1 台（すべて今の配列のままにします）]          │
│ 切り替え方                                                          │
│ ◉ すぐに切り替える                                                  │
│   標準に従っていたキーボード（2.4G Wireless Device、VXE Mouse 1K    │
│   Dongle）を数秒間止めて、割り当てる配列をすぐに効かせます。…      │
│ ○ PC の再起動で切り替える                                           │
│ ⓘ リモート デスクトップ: …［リモート デスクトップのキー配列について…］│
│ ⓘ この設定は、この PC のすべてのユーザーに適用されます。            │
│ PC の再起動が 1 回必要です（シャットダウンではなく再起動）。再起動す│
│ るまで、MKLM でほかの変更はできません。                              │
│ ☐ 技術的な詳細を表示する                                            │
│ 次に Windows の確認画面が出ます（発行元は「不明」）。                │
│ mklm-helper.exe であることを確かめて「はい」を押してください。       │
│                       [キャンセル] [変更する（次に Windows の確認が出ます）] │
└──────────────────────────────────────────────────────────────────┘
```

- 選択肢は `RadioList`（m3 B.4 と同じ。グループの読み上げ名は「再起動の後の PC の標準配列」）。今の標準配列に「（現在）」。今と同じものを選んでいる間は表を出さず、「今の標準配列です。」、［変更する］は無効。
- **キーボードのブロック**（UX-10）: 1 台を 1 つのブロックにする。1 行目に名前（非表示、未接続なら「（非表示）」「（未接続）」を文で添える）、2 行目に「設定した配列（今）: {now} → 再起動の後: {after}」（折り返す）、チェックはその下の自分の行。std-widgets の CheckBox は文を折り返さないので、チェックの文は短い「今の配列のままにする」だけにし、結果は 2 行目に出す。ブロックは `list-item` で、読み上げの要約は「{name}（印）。設定した配列（今）: {now}。再起動の後: {after}」。チェックの読み上げ名は「{name} を {layout} のままにする」、説明（`accessible-description`）は「再起動の後: {after}」（チェックを変えると変わる）。
- 「設定した配列（今）」は `i18n::effective`（メイン画面と同じ「標準に従う（JIS）」「US」。UX-12）。「再起動の後」は B.13 の文。
- チェックを外すと、そのブロックの「再起動の後」は「US に変わります（新しい標準に従う）」（警告の色は付けない。利用者が選んだ変更）。`to` が JIS なら「打鍵での確認がまだです。後で Shift+2 で確かめてください。」を添える（m3 B.5 の「標準に従う」と同じ）。
- 固定できないキーボードと RDP のブロックにはチェックがない（B.3 の表の文）。RDP のブロックには `remote_client_report` の注記を出す（UX-3）。
- **未接続のキーボード**: あるときだけ、開閉のボタン 1 つにまとめる（`accessible-role: button`、`accessible-expandable: true`、`accessible-expanded`。Slint 1.18 にある）。閉じている間もボタンの文が何を書くかを言う（「未接続のキーボード 1 台（すべて今の配列のままにします）」など。B.13）。開くと同じ形のブロック（チェック付き）が並ぶ。既定は閉じる（ふだん使わない行で表を長くしない）が、書き込みに失敗して戻ったときは開いた状態で、失敗したブロックに「書き込めませんでした」を文で出す（色だけで示さない。UX-16）。
- **切り替え方**（SAFETY-2）: 固定するキーボードにその場でリセットできるもの（`StandardRow::can_reset`）があるときだけ出す（`RadioList`、グループ名「切り替え方」。文は B.13）。既定は `vm::change::default_apply_method`（対象はリセットできる固定のキーボード。m3 B.5 と同じ規則: ほかのキーボードかポインターを 10 分以内に使っていれば「すぐに」、対象だけなら「再起動」と計画 1.4 の警告、何も見ていなければ「再起動」）。「すぐに」は `ApplyOptions { allow_live_reset: true, other_input_available: true }`、「再起動」はどちらも false。どちらでも計画（`ExpectedPlan`）は同じ（リセットは計画の外の B.5 の 11）。
- 技術的な詳細: 書く値の一覧（`plan_text` と同じ内容。確認用の記録の 1 行を含む）、7/0 などの置き換え（B.3 の 5）、INV-PS2 の結果。
- **準備**: 選択肢を選ぶと I/O ワーカーの `IoTask::PrepareStandard { token, to }`（`read_inventory` → `read_journal` → `gate::blocker(Gate::NewOp)`）。結果は既存の `PreparedChange` と同じ形で受け取る。止められていれば、その `BlockReason` の文を出し、UAC は出さない（m3 B.5 と同じ）。
- **チェックを変えたとき**: 準備した `snapshot` から `plan_set_standard` を UI スレッドで作り直す（純粋な関数で、キーボードの数だけの計算。m3 A.4 の規則 1 の「ごく短い処理」）。表示、`ExpectedPlan`、送る `follow` を常に一致させる（m2 S6）。
- **状態**: `AppState::standard: Option<StandardDraft>`。

```rust
pub struct StandardDraft {
    pub to: Option<Layout>,
    /// Row IDs (groups) the user set to follow the new standard.
    pub follow: Vec<String>,
    /// Whether the disconnected group is expanded.
    pub disconnected_open: bool,
    /// The switch-now / at-restart choice (`None` while the page offers none).
    pub method: Option<ApplyMethod>,
    pub method_default: Option<MethodDefault>,
    pub preparing: Option<u64>,
    pub prepared: Option<PreparedChange>,
    pub failure: Option<PrepareFailure>,
    /// The plan shown (made from `prepared`, `to` and `follow`); sent as `ExpectedPlan`.
    pub plan: Option<Result<StandardPlan, OperationError>>,
    /// The row whose pin could not be written by the last attempt (UX-16).
    pub failed_row: Option<String>,
}
```

- **メッセージ**: `AppMsg` の `OpenStandard`、`StandardChosen(Layout)`、`StandardFollowToggled { row, follow }`、`StandardDisconnectedToggled`、`StandardMethodChosen(ApplyMethod)`、`StandardPrepared { token, result }`、`StandardApply`、`CancelStandard`。
- **UAC の説明**: 初回は既存の `UacNoticeScreen`（`UacNoticeOrigin` に `Standard` を足し、［キャンセル］で標準配列のページに戻る）。2 回目からはボタンの上の 1〜2 行（m3 B.5）。
- **セッション**: `AppMsg::StartRequest` → `Request::SetStandard`（`apply` は切り替え方の選択。出していなければどちらも false）、目的は `SessionPurpose::Change`。進みぐあいは既存の progress（リセットの間は既存の `ResettingKeyboard` の文）。結果（m3 B.17）は `PendingReboot` → 次の手順「再起動…」→ 再起動の画面。リセットで戻らなかったキーボードは、結果に B.13 の文（配列は変わらないこと、抜き差しか再起動で戻ること）。
- **書き込みの失敗**（UX-16）: 結果の `failure = WriteError { target }` の `target` が固定の devnode なら、汎用のレジストリのエラーではなく B.13 の「固定の書き込みの失敗」の文でキーボードの名前を出し、再起動を勧めない。［閉じる］で標準配列のページに戻り、失敗したブロックを示す（未接続なら開閉ボタンを開く）。
- **再起動の画面**（m3 B.8）: 理由は「PC の標準配列を JIS から US に: PC の再起動待ち」。パスワードの警告（`layout_changes`）は、`SetStandard` では**常に出す**（今の `changes_a_layout` のまま。前の版の「新しい標準に従うキーボードがあるときだけ」はやめた。SAFETY-2: 再起動の前の新しいサインインで、その場で切り替えられなかったキーボードが変わりうる。固定できないキーボードと RDP も変わりうる）。さらに `SetStandard` が理由にあるとき、B.13 の「早めに変わる」の文を出す（SAFETY-2、UX-7）。RDP のキーボードがあるか、リモートのセッションなら、パスワードの警告に RDP の 1 行を足す（UX-2）。
- **再起動後の確認**（m3 B.9）: 操作の行は「PC の標準配列を JIS から US に」。表は B.7（配列が変わる物理キーボードだけ、物理キーボードごとに 1 行）。「設定」の列は `i18n::effective`（「標準に従う（US）」「JIS」。UX-12。「（割り当て）」は使わない）。注記は B.13 の 1 つの版。［元に戻す（もう一度 PC の再起動が必要）］。
- **履歴**（m3 B.11）: 「PC の標準配列を JIS から US に」。状態の文は D.1。
- **取り消しの下見**: B.7（方式を出さず「PC の再起動で元の配列に戻ります」）。
- **再起動を待つ間のメイン画面**（UX-7）: 状態行の標準配列は、この起動で `LayerDriver JPN` を書いた `PendingReboot` のエントリ（`SetStandard`、標準配列を変える `Migrate`）があれば、その記録の `before` を「今」、保存値を「再起動の後」として「US（再起動の後。今は JIS）」と書く（保存値はもう新しいが、効いているのは前の値のため。`Assessment` は保存値で求める: `assess.rs` の注記）。行の「現在の動作」は、`RestartWaits::all` の間、表が標準配列で決まる行（`basis == Standard`）について「PC を再起動すると US（新しい標準配列）になります。それまでは、ふつうは JIS（変更前）です」とする（今は保存値の新しい標準配列の名前が出てしまう。「ふつうは」は H.3 の窓のため）。リモートのセッションで見えない行（D.2）は「このリモート デスクトップのセッションからは見えません。」の後にこの文を続ける。固定したが再起動前の行の「保存済み（反映待ち）」には「PC を再起動した後も、配列は変わりません」を添える（`current.table == after_restart.table` のとき）。その場でリセットして戻った行は、`apply_pending` から外れるので反映待ちにならない。

### B.13 文言（日英。`i18n::standard` に置く。画面の固定の文は `@tr` と `.po`）

`{pc}` は `i18n::pc(remote, computer_name)`: 日本語は「この PC」か「接続先の PC（DESKTOP-3TCSIET）」（名前が読めなければ「接続先の PC」）、英語は "this PC" か "the PC you connect to (DESKTOP-3TCSIET)"。日本語の文で `{pc}` の後に助詞が続くとき、空白は `{pc}` が「PC」で終わるときだけ入れる（「この PC の」「接続先の PC（…）の」）。そのため `{pc}` を含む文は Rust（`i18n`）で組み立てる。

**ページ（`Page::Standard`）**

| 場所 | 日本語 | English |
|---|---|---|
| 題 | PC の標準配列 | The PC's standard layout |
| 説明 | PC の標準配列は、配列を割り当てていないキーボード（標準に従うキーボード）が使う配列です。 | The PC's standard layout is the layout of every keyboard without a layout of its own (keyboards that follow the standard). |
| 選択肢のグループ（読み上げ名） | 再起動の後の PC の標準配列 | The PC's standard layout after the restart |
| 選択肢 | JIS（日本語 106/109 キー）/ US（英語 101/102 キー）。今のものに「（現在）」 | JIS (Japanese 106/109 keys) / US (English 101/102 keys); the current one with " (current)" |
| 変更なし | 今の標準配列です。 | This is the current standard layout. |
| 表の前 | PC の再起動の後は、次のようになります。標準に従っているキーボードには、先に今の配列を割り当てるので、PC を再起動した後も配列は変わりません。 | After the PC restarts, the keyboards type as follows. MKLM first assigns their current layout to the keyboards that follow the standard, so they keep it after the restart. |
| ブロックの印 | （非表示）/（未接続） | (hidden) / (not connected) |
| ブロックの 2 行目 | 設定した配列（今）: {now} → 再起動の後: {after} | Set now: {now} → After the restart: {after} |
| ブロックの読み上げ（`list-item`） | {name}{印}。設定した配列（今）: {now}。再起動の後: {after} | {name}{mark}. Set now: {now}. After the restart: {after} |
| 再起動の後: 割り当て済み（PS/2 を含む） | {layout}（変わりません） | {layout} (unchanged) |
| 再起動の後: 固定する | {from}（変わりません。今の配列を割り当てます） | {from} (unchanged: its current layout is assigned) |
| 再起動の後: 新しい標準に従う | {to} に変わります（新しい標準に従う） | Becomes {to} (follows the new standard) |
| 同（`to` が JIS）に続ける | 打鍵での確認がまだです。後で Shift+2 で確かめてください。 | Not verified by typing yet; check it with Shift+2 afterwards. |
| 再起動の後: 混在 | {after}（一部のコレクションだけ標準に従っています） | {after} (only some of its collections follow the standard) |
| 再起動の後: 固定できない、標準に従う | {to} になります（新しい標準に従います。MKLM はこのキーボードに配列を割り当てられません） | Becomes {to} (it follows the new standard; MKLM cannot assign it a layout) |
| 再起動の後: 固定できない、種類が分からない | 標準配列に従う場合は {to} になります（MKLM はこのキーボードに配列を割り当てられません） | Becomes {to} if it follows the standard (MKLM cannot assign it a layout) |
| 再起動の後: 固定できない、自分の表を持つ | {layout}（変わりません） | {layout} (unchanged) |
| 設定した配列（今）: RDP、ほかのドライバー | — | — |
| 再起動の後: RDP | {to} になることがあります（下の説明） | May become {to} (see below) |
| RDP のブロックの注記 | `remote_client_report`（今のまま。UX-3） | (unchanged) |
| 固定のチェック | 今の配列のままにする | Keep its current layout |
| チェックの読み上げ名 | {name} を {layout} のままにする | Keep {name} as {layout} |
| チェックの説明（読み上げ） | 再起動の後: {after} | After the restart: {after} |
| 未接続の開閉ボタン | 未接続のキーボード {n} 台（すべて今の配列のままにします） | {n} disconnected keyboard(s) (all keep their current layout) |
| 同（一部を外した） | 未接続のキーボード {n} 台（{k} 台は新しい標準に従います） | {n} disconnected keyboard(s) ({k} follow the new standard) |
| 同（書き込めなかったものがある） | 未接続のキーボード {n} 台（書き込めなかったキーボードがあります） | {n} disconnected keyboard(s) (one could not be written) |
| 失敗したブロックの印 | 書き込めませんでした | Could not be written |
| 切り替え方（グループ名） | 切り替え方 | How to switch |
| すぐに切り替える | すぐに切り替える / 標準に従っていたキーボード（{names}）を数秒間止めて、割り当てる配列をすぐに効かせます。打つ配列は変わりません。その間は、ほかのキーボードかマウスで操作します。 | Switch now / The keyboards that followed the standard ({names}) stop for a few seconds so that the layout assigned to them takes effect at once. What they type does not change. Use another keyboard or the mouse meanwhile. |
| PC の再起動で切り替える | PC の再起動で切り替える / 再起動するまでにサインアウト、ユーザーの切り替え、シャットダウンをすると、これらのキーボードの配列が早めに変わることがあります（まだ確かめていません）。 | Switch at the PC restart / If you sign out, switch users or shut down before the restart, these keyboards may change their layout early (not verified). |
| 最近の入力が対象のキーボードだけ（計画 1.4。既存の 1 台用の文の代わり） | 最近入力があったのは、標準に従っていたキーボードだけです。PC の再起動で切り替えることをおすすめします。 | Only the keyboards that followed the standard were used recently. Switching at the PC restart is recommended. |
| RDP の ⓘ | C.3 の「標準配列のページ」 | C.3 |
| 適用範囲 | この設定は、この PC のすべてのユーザーに適用されます。（既存） | (existing) |
| 再起動 | PC の再起動が 1 回必要です（シャットダウンではなく再起動）。再起動するまで、MKLM でほかの変更はできません。 | The PC must restart once (Restart, not Shut down). Until then MKLM cannot make other changes. |
| 操作の名前 | PC の標準配列を {from} から {to} に | Standard layout {from} → {to} |

**入口**

| 場所 | 日本語 | English |
|---|---|---|
| メイン画面と設定のボタン | PC の標準配列を変更… | Change the PC's standard layout… |
| 同（読み上げ名） | PC の標準配列を変更 | Change the PC's standard layout |
| 同（無効のときの横の文と説明） | `i18n::cannot_change_now`（既存の文） | (existing) |
| 設定の行 | PC の標準配列: {layout} | The PC's standard layout: {layout} |
| 設定の行（固定モード） | 固定モードです。すべてのキーボードが {layout} で動きます。キーボードごとモードへは、キーボードの［変更…］から移行できます。 | Fixed mode: every keyboard types {layout}. To switch to per-keyboard mode, use a keyboard's "Change…". |
| 初回セットアップ（キーボードごとモード） | 変えるには、設定の［PC の標準配列を変更…］を使います。 | To change it, use "Change the PC's standard layout…" in Settings. |

**拒否**

| 場所 | 日本語 | English |
|---|---|---|
| 固定モード | 固定モードでは、標準配列がすべてのキーボードの配列です。キーボードごとモードへ移行するときに選べます。 | In fixed mode the standard layout is every keyboard's layout. You choose it when switching to per-keyboard mode. |
| `UnknownStandard` | 標準配列の値が、MKLM の知っている形ではありません。今どの配列で打っているかが分からないため、変更できません。キーボードごとの配列（各行の［変更…］）は今までどおり変えられます。（技術的な詳細に今の値） | The standard layout's values are not in a form MKLM knows. MKLM cannot tell what the keyboards type now, so it cannot change it. Layouts per keyboard (each row's "Change…") can still be changed. (the stored values in the technical details) |
| `IncompleteValues` | {names} には、配列を決める 2 つの値のうち片方しかありません。今どの配列で打っているかが分からないので、PC の標準配列を変えられません。先にそのキーボードの［変更…］で配列を割り当ててください（未接続のキーボードは「非表示と未接続も表示」で表示できます）。 | {names}: only one of the two values that set a layout is stored, so MKLM cannot tell what it types now and cannot change the PC's standard layout. First assign it a layout with its "Change…" (turn on "Show hidden and disconnected keyboards" to see a disconnected one). |
| `Ps2WithoutTable` | {names}（PS/2）の値は、配列を決めない種類です。PC の標準配列を変えると、このキーボードの配列も変わります。先にそのキーボードの［変更…］で JIS か US を割り当ててください（PC の再起動が必要です）。 | The values of {names} (PS/2) do not set a layout, so it would change with the PC's standard layout. First assign it JIS or US with its "Change…" (a PC restart is needed). |
| `NotFollowingStandard` | GUI にはない（チェックは follower の行にしかない） | CLI only (B.11) |

**結果**

| 場所 | 日本語 | English |
|---|---|---|
| 固定の書き込みの失敗（未接続） | 未接続のキーボード「{name}」に書き込めませんでした。変更はしていません。その行の「今の配列のままにする」を外すと、そのキーボードには書かずに進めます（次に接続したとき、新しい標準に従います）。 | MKLM could not write to the disconnected keyboard "{name}". Nothing was changed. Clear "Keep its current layout" on its row to go on without writing to it (it follows the new standard when it is connected again). |
| 固定の書き込みの失敗（接続中） | 「{name}」に書き込めませんでした。変更はしていません。もう一度試しても同じなら、技術的な詳細のエラーを確かめてください。 | MKLM could not write to "{name}". Nothing was changed. If it fails again, see the error in the technical details. |
| リセットの後に戻らなかった | {names} がリセットの後に戻りませんでした。抜き差しするか、PC を再起動してください（配列は変わりません）。 | {names} did not come back after the reset. Unplug and replug it, or restart the PC (its layout does not change). |

**再起動の画面**

| 場所 | 日本語 | English |
|---|---|---|
| 理由 | PC の標準配列を {from} から {to} に: PC の再起動待ち | Standard layout {from} → {to}: waits for a PC restart |
| 早めに変わる（`SetStandard` が理由にあるとき） | 再起動の前にサインアウト、ユーザーの切り替え、シャットダウンをすると、配列が早めに変わることがあります（まだ確かめていません）。今すぐ再起動することをおすすめします。 | Signing out, switching users or shutting down before the restart may change layouts early (not verified). Restart now. |
| パスワードの警告の RDP の行 | リモート デスクトップで{pc}のサインイン画面からサインインする場合は、キー配列が変わることがあります。 | When you sign in on the sign-in screen of {pc} over Remote Desktop, the key table may change. |

**メイン画面（再起動を待つ間）**

| 場所 | 日本語 | English |
|---|---|---|
| 状態行の標準配列 | {to}（再起動の後。今は {from}） | {to} (after the restart; {from} now) |
| 標準に従う行の現在の動作 | PC を再起動すると {to}（新しい標準配列）になります。それまでは、ふつうは {from}（変更前）です | Becomes {to} (the new standard layout) when the PC restarts; until then usually {from} (as before) |
| 同（リモートで見えない行） | このリモート デスクトップのセッションからは見えません。PC を再起動すると… | Not visible in this Remote Desktop session. Becomes {to}… |
| 固定した行の反映待ちに添える | PC を再起動した後も、配列は変わりません | Its layout stays the same after the PC restarts |

**再起動後の確認**

| 場所 | 日本語 | English |
|---|---|---|
| 操作の行 | PC の標準配列を {from} から {to} に | Standard layout {from} → {to} |
| 「設定」の列 | `i18n::effective`（「標準に従う（US）」「JIS」） | (effective) |
| 確認の注記（配列が変わるキーボードがある） | 標準に従うキーボードが新しい標準配列で打てるかは、Windows の認識では分かりません。そのキーボードで Shift+2 を押して確かめてください（" なら JIS、@ なら US）。 | Windows cannot show whether the keyboards that follow the standard type with the new standard layout. Press Shift+2 on each of them (" means JIS, @ means US). |
| 確認の注記（ない） | {pc}につないだキーボードで、配列が変わるものはありません。リモート デスクトップのキー配列は、PC の標準配列に従う場合は変わります（まだ確かめていません）。 | No keyboard attached to {pc} changes its layout. The Remote Desktop key table changes if it follows the PC's standard layout (not verified). |
| 確認の注記に足す（ほかのドライバーのキーボードがある） | MKLM が配列を割り当てられないキーボード（{names}）は、新しい標準配列で打つことがあります。 | Keyboards MKLM cannot assign a layout to ({names}) may type with the new standard layout. |
| リモートで、配列が変わるものがない | この変更で{pc}につないだキーボードの配列は変わらないので、ここで［このままにする］を選んでかまいません。 | This change does not change the layout of any keyboard attached to {pc}, so you may choose "Keep" here. |

**衝突と取り消し**

| 場所 | 日本語 | English |
|---|---|---|
| 衝突の画面の全体の行（確認用の記録が衝突） | 設定アプリなどで、PC が固定モード（{layout}）になりました。今の値のままにすると、すべてのキーボードが {layout} で打たれ、割り当てた配列は効きません。変更前の値に戻すと、キーボードごとモードに戻ります。 | The PC was switched to fixed mode ({layout}), for example by the Settings app. If you keep it, every keyboard types {layout} and assigned layouts do not apply. Putting the value back returns the PC to per-keyboard mode. |
| 取り消しの下見（全体の記録を持つ操作。方式の 2 択の代わり） | この変更は PC の標準配列を変えているので、キーボードをその場でリセットしません。PC の再起動で元の配列に戻ります（シャットダウンではなく再起動）。 | This change altered the PC's standard layout, so no keyboard is reset in place. The old layouts come back when the PC restarts (Restart, not Shut down). |

`vm::unexpected_latin` の許可リストに足す語はない（JIS、US、PC、PS/2、Shift、IME、Windows、MKLM だけを使う。PC の名前は機器名と同じ扱いで、テストの機器名の一覧に入れる）。「確定」と、pin の意味の「固定」を画面の文に使わない。「固定モード」という語があるので `japanese_notation` で「固定」を検出はせず、B.13 と C.3 と D.2 の文は日英のスナップショットのテストで押さえる（F.1。`vm::standard`、`vm::status`、`vm::keyboards`、`vm::post_reboot`、`vm::restart`、`vm::recovery`、`vm::conflict`、`vm::result`、設定、ウィザード、リモート デスクトップのページ）。

### B.14 リモート デスクトップとの関係、リンクの設計との関係

- RDP のキーボード（terminpt）は値を読まないので、固定できない。セッションのキー配列は Windows が決める（C.1）。この操作は、この PC の物理キーボードの配列を保ったまま、標準配列だけを変える手段になる。
- もし Windows が RDP のキー配列を**この PC の標準配列**から決めるなら、この操作が（PC を再起動した後に始めたセッションの）RDP のキー配列を選ぶ手段になる。**接続元の報告**から決めるなら、この操作は RDP に影響しない。どちらでも、接続元の報告が JIS（7/2）なら「標準配列 JIS ＋接続先の PC につないだ US キーボードに US」で JIS になるはず（C.1）。E7（E 章）でどちらかを確かめる。
- 接続元と接続先の両方に MKLM があるときの連携は `design/rdp-link` で扱う（利用者の要望: 「RDP 先にも RDP 元にも MKLM が入っているので、リンクできたらもっと良さそう」）。この設計は連携を前提にせず、連携がなくても完結する。今の製品に連携の機能はなく、画面と文書は連携を約束しない（「まだない」とは書かず「ありません」と書く。UX-4）。連携が接続先の標準配列を変える必要があれば、この操作（`Request::SetStandard`）を使える。接続元の MKLM が接続元の値を書くと接続元の報告が変わるかもしれないこと（E11）は、連携の設計にも関わる。

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
| F6 | RDP で作ったセッションの Raw Input は、この PC の物理キーボードを並べない（ほかの形のセッションは観察していない） | 0.1 節 |
| F7 | コンソールで作ったセッションに RDP で再接続しても、接続元の報告（7/2）の表にはならなかった。2 回とも、そのセッションを作ったときの PC の状態の表で打った（06:57 までのセッション 1: 固定モード（英語）の起動の中で作り、101 の表。14:20 のセッション 1: 標準 US の起動の中で作り、US） | rdp-keyboard.md 1、6 節、0.1 節 |

分かっていないこと:

1. 新しいセッションのキー配列を、Windows が**この PC の標準配列**から決めるのか、**接続元の報告**（7/2 → kbd106 など）から決めるのか。F4 はどちらでも JIS になるので区別できない。E7（標準配列 US の PC に JIS の接続元から新しいセッション）で決まる: H2 が棄却されたので、JIS なら接続元の報告、US（101）なら標準配列。
2. **いつ決まるのか**: セッションを作ったときか（H1(a)）、起動のときの状態が再起動まで全セッションに効くのか（H1(b)）。再起動の前に標準配列を変えて新しくサインインしたセッションがどうなるか（E6 は実施できなくなった。H.3 と同じ問い）。前の版の F5「キー配列はセッションが始まったときに決まる（再起動を待つ変更は、再起動と新しいサインインの後）」は、この 2 つを分けていなかったので、分かっていることから外した（UX-1）。
3. RDP で作ったセッションに再接続したときの表（観察していない）。
4. 接続元の報告が接続元の Windows のどの設定で決まり、接続元の MKLM の書き込み（移行、標準配列の変更）で変わるか（E11）。

どの場合にも成り立つこと（H1(a) と (b)、標準配列と接続元の報告の 4 通りのどれでも）: 今のセッションのキー配列は、セッションが終わるまで変わらない（F7 と、どの仮説でもセッションの途中で表を読み直さないこと）。キーボードごとの割り当て（物理キーボードの devnode の値）は、RDP のキーボードの表を選ばない（F1。表を選ぶのは標準配列か接続元の報告）。標準配列を変えて再起動すると、その後に始めたセッションの表は、標準配列に従う場合だけ変わる。

### C.2 書いてよいこと、書かないこと

- **書いてよい**: F1〜F4、F6、F7。「Windows がこのキー配列を、{pc}の標準配列と接続元の報告のどちらから決めるのかは、まだ確かめていません」。「今のセッションのキー配列は、セッションが終わるまで変わりません（切断したセッションに再接続しても同じです）」。「キーボードごとに割り当てた配列は、リモート デスクトップのキー配列を変えません」。「PC の標準配列を変えて PC を再起動すると、その後に始めたセッションのキー配列が変わることがあります（まだ確かめていません）」。接続元の報告が「日本語キーボード (JIS)」のときの勧め（C.3 の段落 3。報告が JIS なら、標準配列と報告のどちらでも JIS）。確かめた日付と「1 台の PC で JIS で打てた」は研究メモと README に書き、画面の文には書かない（UX-3）。
- **書かない**: 「接続元の配列に従います」「標準配列に従います」「接続元のキーボードごとに配列が使われます」「MKLM ではリモート デスクトップの配列を変えられません」（標準配列に従うなら変えられる）。条件なしの「再起動して新しくサインインするまで反映されません」「…届くのは、PC を再起動して新しくサインインした後です」「reaches it only after」「fixed when the session starts」「セッションが始まったとき（サインインしたとき）に決まります」（UX-1: 接続元の報告に従うなら何も届かず、割り当ては決して届かず、H.3 のとおり再起動の前に届くかもしれない）。接続元のキーボードで勧めを分けること（「JIS のキーボードの接続元なら」。報告で分ける。UX-3）。報告が英語（7/0、4/x）の接続元にどうすればよいか（確かめていない。E7b）。「連携の機能はまだない」（UX-4）。リモートのセッションで出す文の「この PC」（UX-5）。
- **テストで守る**:
  - CLI の `the_remote_session_note_says_only_what_is_known`（`main.rs`）: 含む: "one key table"、"not known yet"、"until it ends"、"not verified"。含まない: "follows the standard"、"follows the client"、"the client's keyboard layout applies"、"applies to the whole session"、"reaches it only after"、"fixed when the session starts"、"at sign-in"、"the remote PC"。
  - GUI の `i18n` のテストと `tests/translations.rs`: 含まない: 「接続元の配列に従」「標準配列に従います」「まで反映されません」「届くのは」「サインインしたとき」「まだない」。RDP の説明の文は「まだ確かめていません」を含む。
  - バッジのテスト `the_remote_desktop_badge_holds_whatever_the_session_follows` は短い文（C.3）を確かめるように直す（「このキーボードは変更できません」を含み、「MKLM」「キーの割り当て」「反映されません」を含まない）。
  - `the_remote_desktop_paragraph_keeps_its_terms_apart` は、入力方式の案内の英数の段落（C.3）について「割り当て」の規則（「キーの割り当て」を含まない、「割り当て」は 1 回、「上の手順で Ctrl+Space を割り当てて」、「そこで」を含まない、「Shift+英数（英数キーの働き）」を含む）だけを確かめるように直す（「キー配列はセッションが始まったときに決まる」の確かめは外す）。
  - 今の「applies to the whole session」の禁止は、古い誤った注記（接続元の配列がセッション全体に効く）を防ぐためのものなので残し、新しい文は「1 つのキー配列で打たれます」と書く。

### C.3 各場所の文

`{pc}` は B.13 の冒頭のとおり。リモートのセッションで出す文は、接続元の PC を「接続元の PC（今お使いの PC）」/ "the PC you connect from (the one you are using)" と書く（UX-5）。英語では "the remote PC" をどちらの意味にも使わない（UX-12）。

**GUI の RDP のキーボードの行**（UX-11）

| 場所 | 日本語 | English |
|---|---|---|
| バッジの読み上げ（`i18n::badge(BadgeKind::RemoteDesktop)` の長い文） | リモート デスクトップ: 接続元の PC から届くキー入力です。このキーボードは変更できません | Remote Desktop: keys sent by the PC you connect from. This keyboard cannot be changed |
| 行のボタン（［変更…］の位置） | 説明… | About… |
| 同（読み上げ名） | リモート デスクトップのキー配列について | About the Remote Desktop key table |
| 行の注記（`remote_client_report`） | 今のまま（「接続元の報告: 日本語キーボード (JIS)。このセッションのキー配列と同じとは限りません」） | (unchanged) |

RDP の行には変更のページがないので、行の既定の操作（Enter、Space、ダブルクリック）と行のボタンがリモート デスクトップのページ（C.4）を開く。長い説明を行の読み上げに入れないので、Tab で行を移るたびに長い文が読まれることはない。

**状態行の下のリモートの注記**（D.2。リモートのセッションで見えないキーボードがあるとき）

| 場所 | 日本語 | English |
|---|---|---|
| 注記 | リモート デスクトップで接続しています。{pc}につないだキーボードの動作は、このセッションからは見えません。 | This is a Remote Desktop session. The keyboards attached to {pc} cannot be seen from this session. |
| ボタン | リモート デスクトップのキー配列について… | About the Remote Desktop key table… |

**入力方式の案内**（`other.slint`。今の段落（`msgid` が "Over Remote Desktop," で始まるもの）を置き換える。前の版の「今の段落は変えず」はやめた。UX-1: 段落 1 の「各キーの働きは接続先の PC が決めます」は、報告に従う場合もあるという文と並ぶと矛盾に読める）

| 場所 | 日本語 | English |
|---|---|---|
| 英数の段落 | リモート デスクトップで英数キーを押しても日本語入力が切り替わらないときは、Shift+英数（英数キーの働き）、Ctrl+英数（ひらがな）、Alt+英数（カタカナ）、Alt+半角/全角（IME のオン/オフ）を使うか、上の手順で Ctrl+Space を割り当ててください。 | If the 英数 key does not switch Japanese input over Remote Desktop, use Shift+英数 (what the 英数 key does), Ctrl+英数 (ひらがな), Alt+英数 (カタカナ) or Alt+半角/全角 (IME on/off), or assign Ctrl+Space with the steps above. |
| 段落の下のボタン | リモート デスクトップのキー配列について… | About the Remote Desktop key table… |

**リモート デスクトップのページ**（C.4。段落 1〜5 の文はここだけに置く）

| 場所 | 日本語 | English |
|---|---|---|
| 題 | リモート デスクトップで使うとき | Using Remote Desktop |
| 段落 1（1 つのキー配列） | リモート デスクトップでは、接続元の PC（今お使いの PC）で押したキーが、{pc}のリモート デスクトップのキーボードから入ります。接続元の PC にキーボードが何台あっても、セッションのキーは 1 つのキー配列で打たれます。接続元の US キーボードで打っても、キー配列が JIS なら JIS として打たれます。 | Over Remote Desktop, the keys you press on the PC you connect from (the one you are using) come in through the Remote Desktop keyboard of {pc}. However many keyboards the PC you connect from has, the session types every key with one key table: keys from its US keyboard are typed as JIS when that table is JIS. |
| 段落 2（分かっていること、分かっていないこと） | Windows がこのキー配列を、{pc}の標準配列と接続元の報告のどちらから決めるのかは、まだ確かめていません。今のセッションのキー配列は、セッションが終わるまで変わりません（切断したセッションに再接続しても同じです）。キーボードごとに割り当てた配列は、リモート デスクトップのキー配列を変えません。PC の標準配列を変えて PC を再起動すると、その後に始めたセッションのキー配列が変わることがあります（まだ確かめていません）。 | Whether Windows sets that table from the standard layout of {pc} or from the client's report is not known yet. The session keeps its key table until it ends (also when you reconnect). Layouts assigned per keyboard do not change it. Changing the PC's standard layout and restarting may change it for sessions started afterwards (not verified). |
| 段落 3（勧め。キーボードごとモード。SetStandard と同じ版から） | リモート デスクトップの行に「接続元の報告: 日本語キーボード (JIS)」と出る場合は、{pc}の標準配列を JIS にし、{pc}につないだ US キーボードには US を割り当ててください。標準配列と接続元の報告がどちらも JIS になるので、どちらから決まっても、新しく始めたセッションは JIS になるはずです（標準配列を変えたときは、PC を再起動した後に始めたセッション）。「接続元の報告: 英語キーボード (101/102 キー)」の場合にどう設定すればよいかは、まだ確かめていません。 | When the Remote Desktop row shows "The client reports a Japanese keyboard (JIS)", make the standard layout of {pc} JIS and assign US to the US keyboards attached to {pc}. The standard layout and the client's report are then both JIS, so new sessions should type JIS either way (after a change of the standard layout: sessions started after the PC restarts). For "The client reports an English keyboard (101/102 keys)", no setup has been verified yet. |
| 段落 3（固定モード。UX-8） | リモート デスクトップの行に「接続元の報告: 日本語キーボード (JIS)」と出る場合は、キーボードの［変更…］でキーボードごとモードへ移行するときに、標準配列に JIS を選び、{pc}につないだ US キーボードには US を割り当ててください。（後半は上と同じ） | When the Remote Desktop row shows "The client reports a Japanese keyboard (JIS)", choose JIS as the standard layout when you switch to per-keyboard mode with a keyboard's "Change…", and assign US to the US keyboards attached to {pc}. (the rest as above) |
| 段落 4（接続元の報告） | 接続元の報告: 接続元の PC の Windows が、接続するときに知らせるキーボードの種類です。接続元のキーボードの数や形ではなく、接続元の Windows の設定で決まります。接続元の PC で移行や標準配列の変更をすると、変わるかもしれません（まだ確かめていません）。変えた後は、リモート デスクトップの行の「接続元の報告」を確かめてください。 | The client's report: the keyboard type that Windows on the PC you connect from reports when it connects. It follows that Windows' settings, not the number or kind of its keyboards. Switching modes or changing the standard layout on that PC may change it (not verified); after such a change, check the client's report on the Remote Desktop row. |
| 段落 5（両方の MKLM） | 接続元の PC にも MKLM を入れてかまいません。接続元の MKLM が書き込むのは、接続元の PC の値だけです。接続先と接続元の MKLM の間に連携の機能はありません。 | MKLM may run on the PC you connect from too; it writes only that PC's values. There is no link between the MKLM on the two PCs. |
| ボタン | PC の標準配列を変更…（キーボードごとモード）/ 閉じる | Change the PC's standard layout… (per-keyboard mode) / Close |

**標準配列のページ**（B.12 の ⓘ）

| 場所 | 日本語 | English |
|---|---|---|
| ⓘ | リモート デスクトップ: キーボードごとに割り当てた配列は、リモート デスクトップのキー配列を変えません。PC の標準配列を変えて PC を再起動すると、その後に始めたセッションのキー配列が変わることがあります（まだ確かめていません）。 | Remote Desktop: layouts assigned per keyboard do not change the Remote Desktop key table. Changing the PC's standard layout and restarting may change it for sessions started afterwards (not verified). |
| ボタン | リモート デスクトップのキー配列について… | About the Remote Desktop key table… |

**CLI**（英語）

- `REMOTE_SESSION_NOTE`（`main.rs`）を関数 `remote_session_note(computer_name)` にする: "note: this is a Remote Desktop session. Keys typed here come from the PC you connect from and are all typed with one key table, whichever of its keyboards they come from. Whether Windows sets that table from the standard layout of the PC you connect to (DESKTOP-3TCSIET) or from the keyboard type the client reports is not known yet. The session keeps its key table until it ends (also when you reconnect). Layouts assigned per keyboard do not change it; changing the PC's standard layout and restarting may change it for sessions started afterwards (not verified). MKLM's per-keyboard layouts apply to the keyboards attached to the PC you connect to."
- `status` の RDP のキーボードの「Remote Desktop」の項目: "types the keys the client sends with one key table, which Windows sets from the standard layout of the PC you connect to or from the client's report (not known which); the session keeps it until it ends; read-only"。
- `standard` の下見の注記: "Remote Desktop: the session types every key with one key table, which Windows sets from the standard layout of the PC you connect to or from the keyboard type the client reports (not known which). Layouts assigned per keyboard do not change it. When `mklm-cli status` in a Remote Desktop session shows \"Remote Desktop session: yes (client reports keyboard type 0x7/0x2)\", a JIS standard layout with US assigned to the US keyboards attached to the PC you connect to should give JIS either way in new sessions (sessions started after the restart, when the standard layout changes). For other client reports no setup has been verified."
- `REMOTE_CHECK_NOTE` と `list` の最後の行は D.2。

**README**（日本語と英語の「できない」の行を書き直し、「リモート デスクトップで使う」の小見出しを足す。接続先と接続元の言い方で書く。UX-5）:

- できない: 「リモート デスクトップの接続先で、接続元のキーボードごとに配列を変えること。接続元から届くキーは、セッションごとに 1 つのキー配列で打たれる（接続元に JIS と US のキーボードがあっても同じ表）。その表を Windows が接続先の PC の標準配列と接続元の報告のどちらから決めるかは未確認（[docs/research/rdp-keyboard.md](docs/research/rdp-keyboard.md)）。接続元と接続先の MKLM の連携の機能はない」
- 小見出し「リモート デスクトップで使う」:
  1. 接続先の PC の MKLM の「リモート デスクトップ」の行に「接続元の報告: 日本語キーボード (JIS)」と出るなら（`mklm-cli status` では `client reports keyboard type 0x7/0x2`）、接続先の PC の標準配列を JIS にし（`mklm-cli standard jis`。固定モードからは `migrate --standard jis`）、接続先の PC の US キーボードに US を割り当てる。2026-09-29 に 1 台の PC で、この形の接続先に新しく始めたセッションが JIS で打てた。報告が英語（101/102 キー）の接続元は未確認。
  2. 今のセッションのキー配列は、セッションが終わるまで変わらない（再接続しても同じ）。キーボードごとの割り当ては、セッションのキー配列を変えない。標準配列を変えて再起動すると、その後に始めたセッションのキー配列が変わることがある（未確認）。
  3. 英数キーの代わりのキー（今の文）。
  4. RDP で新しく始めたセッションからは、接続先の PC のキーボードの動作を確かめられなかった（Raw Input に並ばない）。キーボードの確認は接続先の PC の前で。
  5. 接続元の PC にも MKLM を入れてよい（接続元の PC の値だけを書く）。接続元で移行や標準配列の変更をすると接続元の報告が変わるかもしれない（未確認）。連携の機能はない。
- 書き込みのコマンドの表に `mklm-cli standard <jis|us> [--follow <キーボード>]... [--other-input | --no-reset]` を足す。

**インストールの案内**（`docs/install-guide.ja.md`）: 「最初に起動したとき」の後に「## リモート デスクトップで接続して使う場合」を足す。README の小見出しの 1)〜5) を短くしたもの（接続先の PC と接続元の PC の言い方で書く。両方に MKLM を入れてよいが、それぞれの PC の値だけを書き、連携の機能はないこと。接続元で設定を変えると接続元の報告が変わるかもしれないこと）、詳しくは GUI のリモート デスクトップのページと README へ。

**rdp-keyboard.md 9 節**（利用者への案内）: E 章の追記と同じ内容で、11:32 以降の案内を今の状態（標準配列 JIS、RDP で作ったセッションで JIS）に直す（WP-S0）。

**版の分け方**: 段落 3、標準配列のページ、README の 1) の `mklm-cli standard` は、SetStandard と同じ版で出す。それより前の版（SetStandard を含まない v0.2.0 にする場合）では、段落 3 を出さず、リモート デスクトップのページのボタンは［閉じる］だけにし、README の 1) は移行（`migrate --standard jis`）だけを書く（G 章）。

### C.4 リモート デスクトップのページ（`Page::RemoteHelp`。新規。UX-11）

- ナビゲーションにはない。題 / 本文 / ボタンの行（m3 B.0）。ページが開くとフォーカスは見出しに移り、読み上げは題から始まる。
- 開く所: RDP の行の［説明…］と行の既定の操作、状態行の下のリモートの注記の［リモート デスクトップのキー配列について…］、入力方式の案内の英数の段落の下、標準配列のページの ⓘ。［閉じる］で開いたページに戻る。
- 本文は C.3 の段落 1〜5。キーボードごとモードなら［PC の標準配列を変更…］（B.12 の入口の 3）、固定モードなら段落 3 の固定モードの版を出す。リモートのセッションなら `{pc}` に PC の名前（`OsInfo::computer_name`）を入れる。
- 入力方式の案内の見出しへ飛ぶ案（レビューの案）は採らなかった: Slint 1.18 には、フォーカスした要素や途中の見出しの位置へ `ScrollView` を動かす仕組みがない（`viewport-y` を位置から計算して動かすしかなく、文字の倍率や折り返しで位置が変わる）。別のページなら、見出しへのフォーカス（m3 E.1）がそのまま使える（J.11）。

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
    /// Desktop session; the texts then speak of this session only (sessions started over Remote
    /// Desktop were seen not to list the PC's keyboards; reconnected console sessions were not
    /// observed).
    NotListed { remote: bool },
}
pub fn raw_input_view(kb: &KeyboardDevice, os: &OsInfo) -> RawInputView;
```

「リモート デスクトップのセッションでは見えない」とは決めつけず、キーボードごとに「接続中なのに並ばない」ことを見て、セッションがリモートなら**このセッションについてだけ**その理由を言う（UX-15。コンソールで作ったセッションへの再接続で見えるかは確かめていないため）。

**エンジン**（`Engine::confirm`、`engine.rs` 790〜806 行）: 警告の文を分ける。英語の診断なので、`OperationResult` の型は変えない（プロトコルも変わらない）。エンジンはセッションがリモートかを `Host::remote_session()`（新規。helper のセッションの `GetSystemMetrics(SM_REMOTESESSION)`。helper は呼び出し元と同じセッションで動く。`FakeHost` は設定できる）で知る。

| Raw Input | 値を書いた起動 | 文 |
|---|---|---|
| 並ばない、リモート | 前の起動（再起動で効いている） | "{id}: Raw Input in this Remote Desktop session does not list the keyboard, so its type could not be checked here. The values have been in effect since the restart; check the layout at the PC." |
| 並ばない、コンソール | 前の起動 | "{id}: Raw Input in this session does not list the keyboard, so its type could not be checked here. The values have been in effect since the restart; check the layout with Shift+2." |
| 並ばない、リモート | この起動（再接続の経路） | "{id}: Raw Input in this Remote Desktop session does not list the keyboard, so MKLM cannot tell whether it runs with the stored values yet. Check it at the PC; if it still types the old layout there, reconnect it." |
| 並ばない、コンソール | この起動 | "{id}: Raw Input in this session does not list the keyboard, so MKLM cannot tell whether it runs with the stored values yet; if it still types the old layout, reconnect it." |
| 違う種類 | 前の起動 | "{id}: reports {reported} although the PC has restarted since the change (expected {expected}); the change may not apply to it. Check it with Shift+2 at the PC and revert if it types the wrong layout." |
| 違う種類 | この起動 | 今の文（"does not report the stored type yet; reconnect the keyboard"） |

`apply_pending` の規則は変えない（前の起動の値は `apply_pending_on_close` が `None` を返す。この起動の再接続待ちは `Reconnect` が残る。見えないものを「効いた」とはしない）。

**GUI**（文は下の表）:

- 再起動後の確認（`vm::post_reboot`）: `CheckRow` に `present: bool` を足し（`preview::check_rows` が `KeyboardAssessment::present` から入れる）、表は `preview::check_groups`（物理キーボードごとに 1 行。B.10）。「Windows の認識」の列は、未接続なら「未接続」、接続中で見えずリモートなら「このリモート デスクトップのセッションからは見えません」、接続中で見えずコンソールなら「確かめられません」（`Recognition` に 2 つの変種）。リモートで見えない行があれば、表の下に注記（配列が変わるキーボードがあるときとないときで分ける。UX-6）。キーのテストの案内 `key_test_remote_prompt` は下の表の文にする。
- 「このままにする」の結果（`vm::result`）: 要求が `Confirm` で、確認に使った行にリモートで見えないものがあれば、**配列が変わる物理キーボードだけ**を 1 台 1 回ずつ名前に出してメッセージにする（UX-6。配列が変わるものがなければ出さない）。再接続の経路なら最後の文を変える。英語の警告は今までどおり「技術的な詳細」へ。
- メイン画面の行（`vm::keyboards::current_state`）: 接続中で `current` がないとき、リモートなら「このリモート デスクトップのセッションからは見えません」、そうでなければ今の「動作を確認できません」。状態行の下に、リモートで見えないキーボードがあるとき中立の注記と［リモート デスクトップのキー配列について…］（C.3）。
- **セッションのキー配列の読み取り**（UX-6）: リモートのセッションのキーのテストで Shift+2 を押すと（入力方式が日本語のとき。英語 (US) なら今の `key_test_not_japanese`）、出た文字からそのセッションのキー配列を示す（下の表）。どちらの仕組みでも正しく、E7 を手順なしで記録できる（E 章）。キーはどのキーボードの結果にもしない（今の規則のまま）。

| 場所 | 日本語 | English |
|---|---|---|
| 「Windows の認識」: 見えない、リモート | このリモート デスクトップのセッションからは見えません | not visible in this Remote Desktop session |
| 「Windows の認識」: 見えない、コンソール | 確かめられません | cannot be checked |
| 表の下の注記（リモート、配列が変わるキーボードがある） | このリモート デスクトップのセッションからは、{pc}につないだキーボードの Windows の認識を確かめられません。{pc}の前で確かめてから［このままにする］を押すか、［後で決める］を選んでください。 | Windows' recognition of the keyboards attached to {pc} cannot be checked from this Remote Desktop session. Check them at {pc} before you choose "Keep", or choose "Decide later". |
| 同（配列が変わるキーボードがない） | このリモート デスクトップのセッションからは、{pc}につないだキーボードの Windows の認識を確かめられません。この変更で{pc}につないだキーボードの配列は変わらないので、ここで［このままにする］を選んでかまいません。 | Windows' recognition of the keyboards attached to {pc} cannot be checked from this Remote Desktop session. This change does not change the layout of any keyboard attached to {pc}, so you may choose "Keep" here. |
| Keep の結果（再起動で効いている） | このリモート デスクトップのセッションからは、{pc}につないだキーボード（{names}）を確かめられません。{pc}の前で、それぞれのキーボードで Shift+2 を押して確かめてください（" なら JIS、@ なら US）。違っていたら、履歴の［元に戻す…］で戻せます。 | The keyboards attached to {pc} ({names}) cannot be checked from this Remote Desktop session. At {pc}, press Shift+2 on each of them (" means JIS, @ means US). If one is wrong, revert the change with "Revert…" in the history. |
| 同（再接続の経路）の最後の文 | 違っていたら、そのキーボードを抜き差ししてから確かめてください。 | If one is wrong, unplug and replug it, then check again. |
| メイン画面の行の現在の動作 | このリモート デスクトップのセッションからは見えません | Not visible in this Remote Desktop session |
| 状態行の下の注記 | C.3 の「状態行の下のリモートの注記」 | C.3 |
| キーのテストの案内（`key_test_remote_prompt`） | リモート デスクトップで接続しています。ここで押したキーは接続元の PC（今お使いの PC）から届くため、{pc}のキーボードの確認にはなりません。{pc}の前で、{pc}につないだキーボードで押してください（Shift+2 で @ なら US、" なら JIS）。 | This is a Remote Desktop session. Keys pressed here come from the PC you connect from (the one you are using), so they check none of the keyboards of {pc}. Press them at {pc}, on the keyboards attached to it (Shift+2: @ means US, " means JIS). |
| セッションのキー配列（" が出た） | このリモート デスクトップのセッションでは、Shift+2 で " が出ました。このセッションのキー配列は JIS です。 | In this Remote Desktop session Shift+2 typed "; this session's key table is JIS. |
| 同（@ が出た） | このリモート デスクトップのセッションでは、Shift+2 で @ が出ました。このセッションのキー配列は US です。 | In this Remote Desktop session Shift+2 typed @; this session's key table is US. |

**CLI**（英語。"not visible in this Remote Desktop session" の 1 つの言い方にそろえる。UX-12）:

- `list` / `status`（`text.rs`）: `reported_text` は `NotListed { remote: true }` なら "not visible in this Remote Desktop session"、`remote: false` なら "unknown"（今のまま）。`list` の最後に、見えないキーボードがあれば "Raw Input in this Remote Desktop session does not list N keyboard(s) attached to the PC you connect to (DESKTOP-3TCSIET), so Reported and Now show \"not visible in this Remote Desktop session\" for them; run `mklm-cli list` at that PC to see them."（前の版の「stay empty」は列の文と合わないので直した）。
- `post-reboot` / `keep` の表（`preview::check_text`。物理キーボードごと）: 印の列は "not connected" / "not visible in this Remote Desktop session" / "cannot be checked" を分ける。`REMOTE_CHECK_NOTE`: "This is a Remote Desktop session: keys typed in it come from the PC you connect from, so they check none of the keyboards above, and Raw Input in this session does not list the keyboards attached to the PC you connect to. Do the typing test at that PC, on its own keyboards, before you keep a change that changes their layout."
- `keep` / `post-reboot` の後: 結果の警告に加えて、リモートで見えないキーボードのうち配列が変わるものがあれば "Check at the PC you connect to: press Shift+2 on USB Keyboard and Keychron Receiver (\" means JIS, @ means US); revert with `mklm-cli revert <op>` if one types the wrong layout."（物理キーボードごとに 1 回）。

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

- 0 節に、15:30 以降の時間の流れ（0.1 節の 16:18〜23:14）と、今の接続先（標準配列 JIS、PS/2・USB Keyboard・Keychron は US、2.4G Wireless と VXE は標準に従う）を足す。接続元にも MKLM が入っていること（モードと標準配列は未記録）を足す。
- 8 節の表: **E8** の結果に「14:20 のセッション 1（11:37:20 にコンソールで作成、RDP で再接続）、標準配列 US、JIS の接続元: US で打たれた（`@`、`[`）。予想どおり、どの仮説とも合う（区別できない）」。**E7c** の結果に「23:1x のセッション 2（RDP で新しく作成。23:12:58 の再起動の後）、標準配列 JIS（`271b6909`）、JIS の接続元: JIS で打たれた（利用者の報告。どのキーで確かめたかは記録にない。次は E1 の記号で記録する）。H2 を棄却。標準配列と接続元の報告のどちらかは残る」。**E7** の予想を「H2 が棄却されたので、E7 だけで決まる: JIS なら接続元の報告、101 なら標準配列」に直す。E7 の記録には、キーのテストのセッションのキー配列の読み取り（D.2）を使ってよいこと、**接続元の報告、接続元の MKLM のモードと標準配列を必ず添える**こと（UX-4）を書く。
- 8 節の表に **E11**（新規。UX-4）: 「接続元の PC（`DESKTOP-6FDOQLK`）で MKLM の移行や標準配列の変更をする前と後（接続元の再起動の後）に、接続先の RDP のセッションで `mklm-cli status` の『Remote Desktop session: yes (client reports keyboard type …)』を記録する。接続先には何も書かない。変われば、接続元の報告は接続元の MKLM の書き込みで変わる（C.3 の段落 4 の『かもしれません』を確かめた事実にできる）。変わらなければ、報告は接続元の別の設定で決まる」。
- 7 節の H2 を「棄却（E7c の形の 23:1x の結果）」に。H1 の (a) と (b) は分けていないことを 1 節と 9 節の案内にも書く（前の案内の「キー配列はセッションが始まったときに決まる」を直す。UX-1）。
- 新しい節「接続元の複数のキーボード」: 接続元の PC（`DESKTOP-6FDOQLK`）には JIS と US のキーボードがあり、接続元の US キーボードで打っても JIS で打たれた。キーはスキャンコードで届き、どのキーボードからかは届かない（3 節）ので、セッションのキー配列は 1 つ。MKLM の今の仕組みでは対処できない。接続元と接続先の両方の MKLM を連携させる案は別の設計（ブランチ `design/rdp-link`）。
- 9 節の案内を C.3 のとおりに直す。12 節の問いを「E7 の結果」「E7b（英語を報告する接続元）」「E11（接続元の MKLM と接続元の報告）」「H1 の (a) と (b)（再起動の前の新しいサインイン。H.3 の T-STD-6 と同じ問い）」「RDP で作ったセッションで物理キーボードが Raw Input に並ばない理由（コンソールで作ったセッションへの再接続でも同じか）」に直す。

**`docs/research/m0-results.md`**（「未実施」）:

- リモート デスクトップの項に「2026-09-29: 標準配列 JIS の PC に RDP で新しく作ったセッションで、JIS の接続元が JIS で打てた（H2 棄却）。標準配列と接続元の報告のどちらに従うかは E7 で確かめる（rdp-keyboard.md）」。
- 「標準に従う（0x51）」の項は、標準 JIS の打鍵確認がまだのまま（この PC では 2.4G Wireless Device と VXE が標準 JIS に従っているが、打鍵では確かめていない）。T-STD-2（F 章）で確かめられることを添える。

**加えて `docs/research/m5-install-tests.md`**（研究メモの 3 つのほか）: 「その 4: M5b の NSIS での上書き（2026-09-29 16:18、統合ビルド 0.1.0+8994675d を起動 ID の修正版の上に）: 合格。RDP のキーボードの行は『リモート デスクトップ』」。

---

## F. テスト

### F.1 単体テスト（`cargo test --workspace`。昇格なし、実機の設定を変えない）

**fixtures**: `mklm_core::fixtures::desktop_pc()`（0.1 節の今の接続先: PS/2 4/0、USB Keyboard 4/0、Keychron 4/0、2.4G Wireless の 2 つのコレクション（同じコンテナー）と VXE（値なし）、RDP のキーボード、キーボードごとモード、標準 JIS）。値は `mklm-cli status --json --all` を読み取りだけで取って写す。インスタンス ID は実物のまま。変種: follower の phantom を足したもの、PS/2 が 7/0 のもの（SAFETY-5）、kbdhid の phantom が Type だけのもの（SAFETY-6）、読み取り専用の kbdhid（0x51）とほかのドライバーのキーボード（hyperkbd）を足したもの（SAFETY-4）。

| crate | テスト |
|---|---|
| core `operation` | `set_standard_pins_every_follower_by_default`（desktop_pc で US へ: 2.4G の 2 つと VXE を 7/2、ほかは書かない、ステップは HID → 全体、`apply` は `RestartPc`、INV-PS2 が各ステップで成り立つ）。`set_standard_follow_leaves_the_device`（2.4G の片方の ID で両方が残る）。`set_standard_refusals`（固定モード → `MigrationRequired`、kbdnec / 識別子の不一致 / 値なし → `UnknownStandard`、片方だけの値（kbdhid の phantom、PS/2）→ `IncompleteValues`、7/0 の PS/2（phantom を含む）→ `Ps2WithoutTable`、PS/2・Keychron・RDP・hyperkbd の `--follow` → `NotFollowingStandard`、知らない ID → `UnknownKeyboard`、固定値のない PS/2 → `Plan(InvPs2)`。判定の順も）。`set_standard_to_the_same_layout_writes_nothing`。`set_standard_pins_phantoms_and_replaces_7_0`。`set_standard_lists_keyboards_it_cannot_pin`（読み取り専用の kbdhid は `Some(true)`、hyperkbd は `None`、RDP は入らない）。`follows_standard_table`（0x51、7/0、4/0、7/2、NEC、片方だけの値は follower でない、i8042prt、RDP、読み取り専用）。`standard_guards_are_the_global_pair` |
| core `allowlist` | `stored_standard` が `explicit_standard` と同じ（kbd106 + PCAT_106KEY、大文字小文字、kbd106n と kbdnec と不一致は `None`） |
| core `restore` | `a_standard_change_is_reverted_standard_first`（`SetStandard` の記録を `Before` で: 段階は全体 → `FollowStandard`。各ステップの後で I8 と INV-PS2）。`a_rollback_of_a_standard_change_puts_the_standard_back_first`（`Expect` = `intended`）。`the_order_follows_the_expected_pair_not_the_current_one`（今の全体にペアがあっても、確認用の記録の `Expect` が `Absent` なら全体が先）。`pins_added_by_a_restore_go_before_the_standard`（キーボードごとモードの「導入前に戻す」の取り消し）。既存の `migrated_back_to_fixed_mode_adds_the_pair_first` と `fixed_back_to_per_keyboard_mode_pins_first` は変えずに通る（固定モードへ出入りする戻しの順は変わらない） |
| core `journal` | `set_standard_is_written_as_a_migration_with_a_mark`（JSON の形、確認用の記録、`schema_version` 1、往復）。`a_0_2_0_build_reads_set_standard`（テストの中に 0.2.0 の `OpKind` と `FailureReason` の写し `v0_2_0::*` を置き、同じ JSON を読んで `Migrate { standard, assignments }` になること。印と `target` は無視される）。`a_mark_dropped_by_an_older_build_is_derived_again`（印のない `migrate` で、全体のペアの記録が確認用の記録だけか、ない。`LayerDriver JPN` の記録がある → `SetStandard`、`from` は `before`）。`a_real_migration_stays_a_migration`（`schema_1_journal`、`legacy_guid_journal`、`271b6909` の形のエントリ）。`a_write_error_without_a_target_reads` |
| core `recovery` | `decide_recovery` の表の `Change` の行に `SetStandard` を足す（`migrate` と同じ結果）。`a_fixed_pair_that_appears_makes_a_standard_change_a_conflict`（確認用の記録が `Elsewhere` → `Conflict`、`PendingReboot` の再起動の後と `Planned`） |
| engine `tests/engine.rs` | `set_standard_on_the_desktop_pc`（`PendingReboot`、記録、確認用の記録と baseline がないこと、`context`、接続中の固定したキーボードの `apply_pending = RestartPc`）。`set_standard_resets_the_pinned_followers_when_allowed`（`FakeDevices` の `restart` は接続中の固定した USB の follower だけ、状態は `Written` のまま、戻らないキーボードがあってもロールバックせず `PendingReboot` と警告、`apply_pending` から戻ったものを除く）。`set_standard_without_reset_permission_resets_nothing`。`set_standard_keep_after_the_restart`（`reboot()` → `confirm` → `Confirmed`）。`set_standard_keep_in_fixed_mode_is_a_conflict`（再起動の後に全体のペアを書く → `Conflict`、確認用の記録に `conflict`）。`set_standard_refuses_to_write_when_fixed_mode_appears`（計画の後にペアを書く → 全体を書かずに `Conflict`、固定は外していない）。`set_standard_revert_needs_a_restart_then_closes`。`set_standard_revert_after_the_restart_resets_nothing`（`allow_live_reset` と `other_input_available` で取り消しても `restart` の呼び出しがない）。`a_standard_changing_migration_revert_resets_nothing`（`271b6909` の形。WP-S10）。`set_standard_undo`。`set_standard_undo_of_a_conflict_on_the_standard_writes_nothing`（全体の記録が衝突 → 何も書かず `Conflict`、固定は残る）。`set_standard_pin_write_failure_needs_no_restart`（phantom の固定に `deny_target` → `Reverted`、`failure = WriteError { target }`、全体は書いていない）。`set_standard_skips_a_pin_whose_devnode_is_gone`（`build_records` の後に `remove_devnode` → `skipped = DeviceRemoved`、`PendingReboot`）。`set_standard_refused_while_an_operation_is_open`。`set_standard_missing_layer_driver`（`remove_system32_file("kbd101.dll")`）。`set_standard_plan_changed`（下見の後に follower の phantom を足す → `PlanChanged`、何も書かない） |
| engine `tests/crash.rs` | シナリオ「standard change」「standard change with resets」「standard change revert」「standard change undo」「standard change rollback after a failed global step」（前向きの全体のステップに `deny_target`）で I1〜I7。新しい **I8**: どのクラッシュ状態でも、新しい標準に従わせたキーボードを除き、各キーボードの予想される表（その状態の全体の値で求めたもの）が操作の前と同じ。回復の後も同じ。対象は kbdhid と i8042prt のデバイスノードで、RDP のキーボード、読み取り専用の kbdhid、ほかのドライバーのキーボードはテストの中で対象の外として列挙する。**I7 に足す**: 取り消しの間に全体のキーが書き込みを拒む（`deny_target(Global)`）→ `Conflict`（`write_error`）で、固定は 1 つも消えていない |
| engine（D.2） | `keep_in_a_session_that_lists_no_keyboards`（`FakeDevices` の Raw Input を空にし、`FakeHost::set_remote_session`）: 再起動の後なら `Confirmed`、`apply_pending` なし、警告は「this Remote Desktop session does not list」「in effect since the restart」で「reconnect」を含まない。コンソール（`remote_session` false）では「Remote Desktop」を含まない。同じ起動（再接続の経路）なら `apply_pending = Reconnect` が残り、警告は「check it at the PC」 |
| ipc | `set-standard` の要求の往復（`apply` を含む）、JSON の形のスナップショット、知らないフィールドの拒否、`PROTOCOL_VERSION == 4` |
| client | `plans_first(SetStandard)`、`check_rows` が `SetStandard` で配列が変わるものだけを並べ `present` を入れる、`check_groups`（2.4G の 2 つのコレクションが 1 行、1 つが違えば ⚠）、`standard_rows`（desktop_pc の役割、`NotAssignable`、`can_reset`、`hidden`）、`shown_state`（`legacy_guid_journal` の再起動の後と前、`d724c149` の形の `RevertedPendingReboot` を次の起動で読む） |
| CLI | 構文（`standard jis`、`--follow "#3"` の繰り返し、`--other-input` / `--no-reset`、`--yes` にはどちらかが要る、`--yes` と `#n` → 2、`--dry-run`、`waits_for_updates`）。下見の文（desktop_pc。"Stored" の列、確認用の記録の行、早めに変わる注意）。`NotFollowingStandard` の 4 つの文。`revert` / `undo` の全体の記録を持つ操作の注記。`journal` の判定した状態（`a_reverted_pending_reboot_of_an_earlier_boot_reads_as_reverted`、`a_pending_reboot_of_an_earlier_boot_waits_for_the_check`、`not_in_effect_lines_of_an_earlier_boot_are_not_shown`）。`list` / `status` のリモートの見えないキーボード（"not visible in this Remote Desktop session" と最後の行）。`check_text` の 3 つの印と物理キーボードごとの行。D.3 の文。`the_remote_session_note_says_only_what_is_known` の更新（C.2。PC の名前が入ること）。`migrate --standard` がキーボードごとモードで違う標準を求めたら 1 |
| GUI | `state`: 標準配列のページを状態行の下と設定から開く、固定モードでは開かない、ふさがっている間はボタンが無効で理由の文が出る、選択 → 準備 → 表、チェックで計画を作り直す、未接続の開閉、切り替え方の既定と選択、［変更する］→ `StartSession(SetStandard { apply, expected })`、初回の UAC の説明から戻る、止められたとき、キャンセル、書き込みの失敗から失敗した行を開いて戻る。`vm::standard` の日英のスナップショット（desktop_pc: JIS → US、2.4G のチェックを外した場合、RDP の行と注記、未接続のまとめの 3 つの文、固定できないキーボード、切り替え方、ブロックとチェックの読み上げ）。`vm::status`（入口の行、再起動待ちの標準配列）。`vm::keyboards`（再起動待ちの行の文、リモートの見えない行、RDP の行のボタンと短いバッジ）。`vm::post_reboot`（`SetStandard` の注記の 2 つの版、リモートで Keep を勧める文、物理キーボードごとの行、「このリモート デスクトップのセッションからは見えません」）。`vm::journal`（D.1 の 2 つの状態）。`vm::result`（Keep の後のリモートの文、再起動で効いている場合と再接続の場合、配列が変わるものだけ、固定の書き込みの失敗、リセットで戻らなかったもの）。`vm::restart`（`SetStandard` では常に `layout_changes`、早めに変わる文、RDP の行）。`vm::recovery`（全体の記録を持つ操作には方式を出さず、再起動で戻る文）。`vm::conflict`（確認用の記録の注記とおすすめ）。リモート デスクトップのページ（キーボードごとモードと固定モード、`{pc}` の有無）。キーのテストのセッションのキー配列の読み取り。設定の行。ウィザードの文。`unexpected_latin`、`japanese_notation`、`tests/translations.rs`（新しい `@tr`、置き換えた英数の段落、C.2 の禁止語） |
| Pester | D.4 の直し。`smoke-test.Tests.ps1` の拒否は関数で、`find-marker.Tests.ps1` は `$LASTEXITCODE` を戻す |

### F.2 CI と同じ形の確かめ（FULL CHECKS）

ワークフローの FULL CHECKS に加えて、`Invoke-InstallerChecks.ps1` を D.4 の 5 の形で動かし、0 で終わること。

### F.3 実機のテスト（利用者の同意を得て、1 項目ずつ手順を渡し、ジャーナルと照らしてから次へ）

**締め出しの予防**（T-STD-2、6、6b の前に毎回確かめる）: PIN を設定しておく。パスワードを打つときは、配列を割り当てたキーボード（この PC では PS/2、USB Keyboard、Keychron。どれも自分の表を持つので、標準配列が変わっても配列は変わらない。サインイン画面の入力方式が日本語のとき）を使う。サインイン画面で入れなくなったら、サインイン画面の右下の電源のメニューから「再起動」を選ぶ（再起動で変更と固定がそろって効く）。

| # | 内容 | 再起動 | 公開の条件 |
|---|---|---|---|
| T-STD-1 | `mklm-cli standard us --dry-run`（コンソールでも RDP でもよい）: 表が B.11 の例のとおり、何も書かれない（`mklm-cli journal` が変わらない） | なし | — |
| T-STD-2 | GUI で標準配列を JIS → US（既定のまま、切り替え方は「すぐに切り替える」）→ 2.4G Wireless と VXE がリセットされ、2.4G Wireless の Shift+2 がその場で " のまま → 再起動の画面 →「再起動」→ **コンソールで**サインイン → 確認画面（配列が変わるキーボードはないので、注記は「…配列が変わるものはありません。…」）→ PS/2、USB Keyboard、Keychron、2.4G Wireless で Shift+2（どれも今までと同じ配列）→ このままにする。`journal --json` で `set_standard` の印、確認用の記録、`confirmed` | 1 | — |
| E7 | T-STD-2 の後（標準配列 US、PC のキーボードは変わらない）、コンソールからサインアウトし、JIS の接続元から **RDP で新しいセッション**を作って E1（Shift+2、P の右。キーのテストのセッションのキー配列の読み取りでもよい。D.2）と E2（`tools/rdp/Watch-Keys.ps1`）。接続元の報告、接続元の MKLM のモードと標準配列を記録する。JIS なら接続元の報告、US なら標準配列（C.1） | 0（T-STD-2 の再起動を使う） | — |
| T-STD-3 | 履歴から T-STD-2 を元に戻す（または標準配列を JIS に選び直す）→ 下見に切り替え方の 2 択が出ず、「PC の再起動で元の配列に戻ります」→ 取り消しの後も 2.4G は " のまま → 再起動 → 標準配列 JIS に戻る。RDP で作ったセッションで JIS | 1 | — |
| T-STD-4 | RDP で作ったセッションで再起動後の確認をする（T-STD-2 か 3 の途中で）: 「Windows の認識」が「このリモート デスクトップのセッションからは見えません」、配列が変わらない変更なので「ここで［このままにする］を選んでかまいません」、このままにした後の文が D.2 のもので「抜き差し」を言わない | —（2、3 の中で） | — |
| T-STD-5 | 昇格した PowerShell で `mklm-cli standard … --other-input`（T-STD-3 の代わりに行ってもよい）: D.3 の文が出る。再起動の後に MKLM を開くと確認画面 | —（3 の中で） | — |
| T-STD-6 | **再起動の前の新しいサインイン**（H.3）。(a) 標準配列を JIS → US、切り替え方「PC の再起動で切り替える」（リセットしない）→ 再起動の画面で「後で」→ コンソールでサインアウト → サインイン（パスワードは割り当てたキーボードで）→ メモ帳で 2.4G Wireless の Shift+2 を記録（" なら窓はなかった、@ なら H.3 が起きる）→ 再起動 → 確認画面で元に戻す（もう一度再起動）。(b) 同じことを「すぐに切り替える」で行い、サインアウトとサインインの後も 2.4G Wireless が " のままであること（リセットで窓が閉じた） | 2〜4 | **SetStandard の公開の条件** |
| T-STD-6b | 高速スタートアップ版: T-STD-6 (a) のサインアウトの代わりに、コンソールで スタート → 電源 → シャットダウン → 電源を入れる（イベント 27 が 0x1 であること。0x0 なら結論なしとしてやり直す）。サインイン画面のパスワードの欄で 2.4G Wireless の Shift+2 を打ち、欄の表示のボタン（目の形）で文字を見てから消す → 割り当てたキーボードでサインイン → メモ帳でも確かめる → 再起動 → 元に戻す。(b) の「すぐに切り替える」でも行う | 2〜4 | **SetStandard の公開の条件** |
| R-PHANTOM | USB のキーボードを渡せる VM か使い捨ての PC（この PC の設定は変えない）で m2 I.3 を確かめる: USB キーボードを別のポートに差し直して phantom を作り、`mklm-cli standard us` で phantom の固定が書けること。書けなければ、B.6 のとおり `Reverted` で名前が出て再起動を求めないこと | VM の中 | **SetStandard の公開の条件**（書けないと分かったら、未接続のキーボードを既定で固定せず「接続していないため、今の配列を割り当てられません。次に接続したときに新しい標準に従います」と示す形に変えてから公開する） |
| E11 | E 章の E11（接続元の MKLM と接続元の報告）。接続元の PC を変えるので、利用者が接続元で行う | 接続元で 1〜2 | — |

T-STD-2 と T-STD-3 は、MT-2〜MT-9（起動 ID）に要る「再起動で反映する変更」としても使える（PC のキーボードの配列が変わらないため）。そうするかは I 章の問い 1。

---

## G. 作業の分担と版

| WP | 範囲 | 主なファイル | 依存 | 版 |
|---|---|---|---|---|
| WP-S0 | E 章の記録 | `docs/research/*.md` | なし | すぐ |
| WP-S1 | D.4 CI の Pester | `installer/smoke-test.ps1`、`installer/tests/*.Tests.ps1`、`installer/tests/Invoke-InstallerChecks.ps1`、`.github/workflows/ci.yml`、m5b D.9.3 / G.6 | なし | v0.2.0（今の CI の段が落ちるので、タグの前に必要） |
| WP-S2 | D.1、D.3 | `crates/mklm-client/src/describe.rs`、`apps/mklm-cli/src/write/{render,journal_view,commands}.rs`、`apps/mklm-cli/src/text.rs`、`apps/mklm/src/{i18n.rs,vm/journal.rs}` | なし | v0.2.0 を勧める（プロトコルもジャーナルも変えない） |
| WP-S3 | D.2（エンジンの警告と `Host::remote_session`、`check_groups`、見えないキーボードの文、キーのテストのセッションのキー配列の読み取り） | core（`raw_input_view`）、engine（`confirm`、`host.rs`）、client（`CheckRow::present`、`check_groups`）、CLI、GUI | なし | v0.2.0 を勧める（プロトコルを変えない。エンジンの文が変わるのでビルド ID は変わる） |
| WP-S4 | C 章の文（段落 3、標準配列のページ、README の 1) を除く）、リモート デスクトップのページ（C.4）、`OsInfo::computer_name` と `{pc}` | `apps/mklm/src/i18n.rs`、`other.slint`、新しい `ui/screens/remote_help.slint`、`.po`、`main.rs`、`text.rs`、`mklm-win` の OS の読み取り、README、install-guide（rdp-keyboard.md 9 節は WP-S0） | なし | v0.2.0 を勧める |
| WP-S10 | 取り消しの後にリセットしない規則（B.7。全体の記録を持つ操作。標準配列を変えた `Migrate` の今の欠陥を直す）と、GUI の下見と CLI の注記 | engine（`reapply`）、`vm/recovery.rs`、`i18n/journal_pages.rs`、`commands.rs` | なし | v0.2.0 を勧める（プロトコルとジャーナルを変えない） |
| WP-S5 | B.4、B.8、B.6 の `plan_restore` の段階（core） | `operation.rs`、`allowlist.rs`、`restore.rs`、`journal.rs`、fixtures | なし | SetStandard の版 |
| WP-S6 | B.5、B.6、B.9（engine、ipc、helper）と crash の網羅（I8、I7 の追加） | `engine.rs`、`params.rs`、`error.rs`、`message.rs`、`lib.rs`、`session.rs`、`tests/*` | WP-S5 | 同上 |
| WP-S7 | B.10、B.11（client、CLI） | `preview.rs`、`session.rs`、`write.rs`、`commands.rs`、`main.rs` | WP-S6 | 同上 |
| WP-S8 | B.12、B.13（GUI）と段落 3 | `state.rs`、`state/standard.rs`（新）、`vm/standard.rs`（新）、`i18n/standard.rs`（新）、`ui/screens/standard.slint`（新）、`main.slint`（入口の行）、`app.rs`、`worker.rs`、`vm/{status,keyboards,post_reboot,restart,journal,conflict,result}.rs` | WP-S7 | 同上 |
| WP-S9 | 文書（m2 C.3・C.5・C.10・D.4・D.8・D.13・E.6・F.1、m3 B.2・B.8・B.9・B.10・B.13・B.14・H.4（T-A11Y-4 に標準配列の入口の行とページ）・K、README の表、recovery.md の FAQ「標準配列を変えたい」）と F.3 の実機テスト | `docs/**` | WP-S8 | 同上 |

- **SetStandard の版**: v0.2.0 は起動 ID の MT-2〜MT-9 と ARM64 の確認を待っている（m2 C.10「公開する順序」）。SetStandard を v0.2.0 に入れるか、その次（0.3.0）にするかは I 章の問い 1。この設計は、どちらでも WP の中身を変えない。入れない場合、C 章の文は段落 3 を除いたものにし、段落 3 と README の 1) の `standard` は SetStandard の版で出す（C.3「版の分け方」）。
- **SetStandard の公開の条件**: FULL CHECKS、F.3 の T-STD-6、T-STD-6b、R-PHANTOM。
- **レビューで確かめること**（m2 K、m3 I、m5b G.6 への追加）: `SetStandard` のすべての書き込みが `check_plan` を通る。全体のステップが固定のステップのフラッシュの後にしかない（I8）。戻すときは全体の後に固定を外す（`FollowStandard`。固定モードへ出入りする戻しの順は変わらない）。確認用の記録は書かれず、baseline も作られない。固定した follower のリセットは `Written` のまま行い、失敗してもロールバックしない。全体の記録を持つ操作の取り消し、undo、解決では、キーボードをリセットしない。JSON の `kind` は `migrate` で、`schema_version` は 1。古いビルドの型の写しで読めるテストがある。画面の RDP の文に C.2 の禁止の表現がなく、リモートのセッションの文は `{pc}` で両端を書く。CI の段が明示の `exit` で終わり、Pester のコンテナーの失敗も見る。

---

## H. リスクと未解決

1. **リモート デスクトップのキー配列の決まり方**が分かっていない（C.1）。E7 まで、案内は「どちらかは未確認」のまま。E7 の結果によっては、B.14 の「この操作が RDP のキー配列を選ぶ手段になる」が当たらない。どちらでも、C.3 の報告が JIS の接続元への勧めは成り立つ（どちらも JIS）。
2. **英語を報告する接続元**（4/0 か 7/0 を報告する接続元）にどう設定すればよいかは分からない（E7b）。案内には書かない。接続元の報告が接続元の MKLM の書き込みで変わるかも分からない（E11）。
3. **再起動の前の新しいサインイン**（SAFETY-2）: `global_change_action` の注記では、user32 と IME は `LayerDriver JPN` をサインイン時に読む。変更を書いた後、再起動の前に新しいセッションができると（サインアウトしてのサインイン、ユーザーの切り替え、RDP の新しいセッション、とくに高速スタートアップの「シャットダウン」からの電源投入: ドライバーは休止から戻り、起動 ID は変わらず `PendingReboot` のまま、ログオンは新しいセッション）、そのセッションでは新しい標準配列が使われうる。固定の値は、キーボードのデバイスが起動し直すまで kbdhid に読まれないので、そのセッションでは固定したキーボードもサインイン画面から新しい標準配列で打ちうる。**この設計の前提では起きるものとして扱う**: (a) その場でリセットできる固定したキーボードは、書いた直後にリセットして窓を閉じる（B.5 の 11。既定は m3 B.5 の規則）。(b) `SetStandard` では常にパスワードの警告と「サインアウト、ユーザーの切り替え、シャットダウンではなく今すぐ再起動を」を出し、画面と CLI の「配列は変わりません」は「PC を再起動した後は」に限る（B.12、B.13）。(c) T-STD-6 と T-STD-6b を公開の条件にする（F.3）。BLE / BT の follower、未接続の follower、リセットで戻らなかったキーボード、「PC の再起動で切り替える」を選んだ場合は窓が残る。
4. **phantom への書き込み**（m2 I.3）: 接続していないキーボードの固定は、phantom のデバイスノードのハードウェア キーを書き込みで開く。未検証。開けなければ書き込みエラー → ロールバックで、全体のステップの前なので `Reverted`（再起動を求めない）、`failure = WriteError { target }`。前面はそのキーボードの名前を出し、その行のチェックを外せば進めることを案内する（B.13。UX-16）。R-PHANTOM（VM）を SetStandard の公開の条件にし、書けないと分かれば未接続のキーボードを既定で固定しない形に変える（F.3）。
5. **0x51 が標準 JIS に従うこと**は打鍵で確かめていない（m0-results.md）。「新しい標準に従う」で JIS を選んだ場合と、固定しない既定外の使い方で残る。警告を出す（B.5 の 7）。
6. **`OverrideKeyboardIdentifier` と IME**: 標準配列を US にすると識別子は `PCAT_101KEY` になり、IME は 101 キーの PC として振る舞いうる（rdp-keyboard.md の H4 は、101 の表の原因としては棄却したが、IME の振る舞いそのものは調べていない）。JIS を割り当てたキーボードで、IME のキーの扱いが変わるかは未確認。MKLM は 2 つの値を常にそろえる（`check_global_writes`）ので、分けられない。
7. **古いビルドとダウングレード**: ダウングレードすると、標準配列の変更が「移行」と表示される（B.8）。古いビルドで取り消すと、固定を外してから標準配列を戻し（段階 `FollowStandard` を知らない）、取り消しの後にリセットしうる。ダウングレードの前に、新しいビルドで「このままにする」か「元に戻す」を済ませるよう案内する（B.8）。
8. **表示の範囲**: 再起動を待つ間の「現在の動作」を直す（B.12）のは、表が標準配列で決まる行だけ。`Migrate` の再起動待ちの間の行（固定モードの名残）も同じ問題を持つが、この設計では標準配列で決まる行だけを直す。
9. **CI の Pester**: Pester 3.4 と 5 の結果のオブジェクトの違いを、プロパティがあるときだけ見ることで吸収する。GitHub のランナーの Pester の版が変わると、見落としがありうる（明示の `exit` の前の判定で、少なくともテストの失敗と 0 件は拾う）。
10. **標準配列の値がない PC**: `LayerDriver JPN` と識別子がない Windows（日本語の設定をしたことのない PC）は、今の表を US と推定するが打鍵で確かめていない（`assess.rs` の `inferred_standard_is_not_verified`）。この設計は推定から固定を書かず、`UnknownStandard` で断る。その PC では標準配列を変えられない（1 台ずつの `set` はできる。前面もそう案内する）。
11. **固定できない入力**（SAFETY-4）: ほかのドライバーのキーボード、MKLM が書かない仮想のキーボード、リモート操作のソフト（RDP 以外）が送るキーは、新しい標準配列に従うことがある。原則 2 はそれらには及ばない。下見と確認の注記とパスワードの警告で示すだけで、防げない。リモート操作のソフトは列挙にも出ないので、画面では名前を出せない（README の説明に書く）。
12. **固定モードの出現**（SAFETY-7）: 確認用の記録で `Conflict` にするが、設定アプリが書いた固定モードを、利用者が衝突の画面で「今の値のまま」にすれば、固定モードが残る（利用者の選択。B.13 の注記で結果を示す）。
13. **`{pc}` の名前**: `OsInfo::computer_name` が読めない PC では「接続先の PC」とだけ書く。名前が長いと、ボタンではなく文の中なので折り返す。

---

## I. 利用者に決めてもらうこと

1. **SetStandard をどの版に入れるか**: (a) v0.2.0 に入れる。MT-2〜MT-9 に要る「再起動で反映する変更」を、PC のキーボードの配列を変えずに作れ（T-STD-2、T-STD-3）、E7 もその再起動で行える。ただし v0.2.0 の変更とレビューの範囲が広がり、プロトコルが 4 になり、公開の条件（T-STD-6、T-STD-6b、R-PHANTOM）が v0.2.0 の条件に加わる。(b) v0.2.0 の後（0.3.0）。v0.2.0 には D 章と C 章の一部と WP-S10（WP-S1〜S4、S10）だけを入れる。
2. **昇格した CLI の RunOnce**: 今は、昇格したプロセスは HKCU に書かない（別の管理者のアカウントかもしれないため。2026-09-28 の決定）ので、文だけを直す（D.3）。昇格したトークンの利用者の SID が、そのセッションにサインインしている利用者の SID と同じとき（ふつうの管理者の昇格）は登録する、に変えるか。
3. **E7 をいつ行うか**: 接続先の標準配列を一度 US にして再起動し（SetStandard なら PC のキーボードは変わらない）、JIS の接続元から新しいセッションで確かめる。RDP のキー配列の決まり方が分かる唯一の方法（E7b は接続元の設定の変更が要る）。
4. **公開の条件の実機テストを行えるか**: T-STD-6 と T-STD-6b は、この PC の標準配列を一時的に US にし、再起動の前にサインアウトや高速スタートアップのシャットダウンをする（締め出しの予防は F.3）。R-PHANTOM は、USB のキーボードを渡せる VM か使い捨ての PC が要る。どちらもなければ、未接続のキーボードを既定で固定しない形で公開するか。E11 は接続元の PC の設定を変える。

---

## J. 採らなかった案

1. **固定モードに戻してから移行し直す**（今のやり方）: 再起動が 2 回要り、US のキーボードを 1 台ずつ指定し直す必要がある（0.1 節）。
2. **割り当て済みのキーボードの配列も同じ操作で変える**（`--also`）: 標準配列の変更と、キーボードの配列の変更が 1 つの確認画面に混ざる。割り当て済みのキーボードは `set` で変える。USB は再起動なしで変えられるので、再起動の回数はふつう増えない。PS/2 だけは 2 回目の再起動が要るが、まれ。
3. **割り当て済みのキーボードを「新しい標準に従う」にする**（固定を外す）: B.3 の follower だけを対象にし、割り当て済みのものは `set --layout standard` にする。操作の意味（「今の配列を保つ」）を狭く保つため。
4. **固定を再起動でだけ効かせる**（前の版の既定。固定した USB キーボードをその場でリセットしない）: H.3 の窓（再起動の前の新しいサインインで、固定したキーボードも新しい標準配列で打つ）が USB のキーボードにも残るので、採らなかった（SAFETY-2）。前の版が挙げたリセットの失敗の扱いは、固定の値が今の表と同じなので「警告だけで続ける」で足りる（B.5 の 11）。キーボードが数秒止まることは、切り替え方の選択で利用者が決める。
5. **CLI で 1 台ずつ「固定しますか?」と尋ねる**: 既定（すべて固定）と `--follow` で足りる。下見が全部を表示する。
6. **`confirm` の結果に型付きの「確かめられなかったキーボード」を足す**（`OperationResult` の変更）: 前面が自分の読み取り（`check_rows` と `present`）で同じことを言えるので、プロトコルを変えずに済ませた（D.2）。
7. **RDP のキー配列を直接変える**（`KeyboardType Mapping`、`Layout File`、RDP のキーボードへの書き込み）: rdp-keyboard.md 10 節の理由のまま採らない。
8. **ジャーナルの版を上げる、新しい `kind` を書く**: B.8 の表。
9. **自分の表を持たない値で固定された PS/2 を、同じ操作で from に固定し直す**（SAFETY-5 の案の 2 つ目）: PS/2 の書き込み（起動時の値）をこの操作に持ち込み、戻すときの i8042prt の段階（書いた後も 2 つの値がある PS/2 の固定の変更）にも I8 の分け方が要る。原則 3（この操作は PS/2 の値を書かない）を保ち、まれな場合なので、断って［変更…］を案内する。
10. **固定モードの出現を、記録なしにエンジンの規則で `Conflict` にする**（SAFETY-7 の案）: `Conflict` にしても衝突の画面に出す値（記録）がなく、利用者が解決できない。古いビルドも見分けられない。確認用の記録にすれば、書くとき、再起動の後、確定、取り消し、回復のすべてで既存の規則がそのまま効き、古いビルドも同じに動く（B.2、B.7）。
11. **入力方式の案内の新しい見出しへ飛ぶ**（UX-11 の案）: Slint 1.18 には、フォーカスした要素や途中の見出しの位置へ `ScrollView` を動かす仕組みがない（`viewport-y` を位置から計算するしかない）。別のページ（C.4）なら、ページが変わるとフォーカスが見出しに移る既存の規則（m3 E.1）で、読み上げが題から始まる。
12. **状態行の項目の中に［変更…］を置く**（前の版）: 状態行は折り返す 4 列で、最小の幅では 1 列が約 83 px（560 − 160 − 32 − 36 を 4 で割る）。ボタンは折り返せず、`StatusItem` は 1 つの `text` の読み上げで子は読まれない（UX-9）。独立した行にした（B.12）。

---

## レビュー対応

1 回目のレビュー（2026-09-30。観点: SetStandard の安全性と、画面と文言）への対応。すべての指摘を採用した（SAFETY-4、SAFETY-7、UX-5、UX-11 の 4 件は、指摘の案と一部違う形で）。「節」は変更した箇所。

### 観点 1: 新しい操作 SetStandard の安全性（SAFETY-1〜8）

| 指摘 | 採否 | 変更 / 理由（節） |
|---|---|---|
| SAFETY-1（major）: `plan_restore` は HID を段階 3、ペアのない全体を段階 4 に置くので、SetStandard の取り消しも前向きと同じ「固定 → 全体」になる。固定を消した後、全体を戻す前に止まると（全体の書き込みの拒否、電源断）、固定していた follower が新しい標準配列の下に置かれ、I8 が破れる。衝突した SetStandard の undo は固定だけを消す | 採用 | `plan_restore` に段階 `FollowStandard` を足した: キーボードごとモードのままの戻し（全体のペアが計画の前にも後にもない。前はペアの記録の `Expect` で見る）では、書いた後に自分の表を持たない HID のステップを全体の後に、固定を足す・変えるステップを前に置く。固定モードへ出入りする戻しの順は変えない（既存の restore のテストがそのまま通る）。`Expect` で見るので、外部の固定モードが現れていても全体が先になり、その CAS の不一致で止まる。`undo_conflict` と、衝突の undo を続ける `ContinueRevert` は、SetStandard の全体の記録（確認用の記録を含む）のどれかが衝突していれば何も書かず `Conflict` のまま。衝突の解決は段階の順で書くので、全体が書けなければ固定は外れない。B.6 に戻すときの止まった所の表を足し、m2 C.5 の直す文を書いた（WP-S9）。crash の網羅に「revert」「undo」「全体のステップの失敗の後のロールバック」の I8 と、I7 の「戻しの間に全体が書き込みを拒む → 固定は 1 つも消えない」を足した（A、B.6、B.8、F.1、G） |
| SAFETY-2（major）: I8 は永続状態の不変条件だが、危険なのは効いている状態。新しい標準配列は次のサインインで効き、固定は kbdhid がデバイスを起動し直すまで読まれない。再起動の前の新しいセッション（とくに高速スタートアップのシャットダウン）で、固定したキーボードがサインイン画面から新しい標準配列で打つ。それなのに「配列は変わりません」と書き、固定だけならパスワードの警告を出さず、H.3 と T-STD-6 を任意にしている | 採用 | (1) H.3 を「起きるものとして扱う」に書き直した。`SetStandard` ではパスワードの警告を常に出し（今の `changes_a_layout` のまま。前の版の制限をやめた）、「再起動の前にサインアウト、ユーザーの切り替え、シャットダウンをすると、配列が早めに変わることがあります（まだ確かめていません）。今すぐ再起動することをおすすめします」を出す。「配列は変わりません」は「PC を再起動した後は」に限った。(2) 固定をその場で効かせる案を採った: `Written` の後、`PendingReboot` の前に、接続中でその場でリセットできる固定した follower を 1 台ずつリセットする（打つ配列は変わらない）。戻らなくてもロールバックせず警告だけ。状態は `Written` のままなので、途中のクラッシュはロールフォワードになる。呼び出し元の申告（`ApplyOptions`）が要り、GUI は切り替え方の 2 択（既定は m3 B.5 の規則）、CLI は `--other-input` / `--no-reset` と質問、ipc は `SetStandardRequest::apply`。BLE / BT と未接続には窓が残ることを書いた。(3) T-STD-6 と高速スタートアップ版の T-STD-6b を公開の条件にし、締め出しの予防（割り当てたキーボードでパスワードを打つ、PIN、サインイン画面の電源メニューから再起動）を手順に書いた（0.2、0.5、B.1、B.5、B.9、B.11、B.12、B.13、F.3、H.3、J.4） |
| SAFETY-3（major）: 取り消しの後の D.4 の手順 9 のリセットは、固定を消した後なので kbdhid が 0x51 を報告し、このセッションが読んだ新しい標準配列で打たせる。GUI は最近マウスを使っていれば「すぐに元の配列に戻す」を既定にする。標準配列を変えた移行の取り消しも同じ | 採用 | 全体の記録を持つ操作（`SetStandard`、標準配列を変えた `Migrate`、全体の値を含む `RestoreBaseline`）の取り消し、undo、解決では、`reapply` はリセットしない（`revert_entry`、`undo_conflict`、`close_resolution`。`ApplyOptions` によらない）。GUI の `revert_page`、`undo_page`、`recovery_page` はそのエントリに方式の 2 択を出さず「PC の再起動で元の配列に戻ります」を出す。CLI は `--other-input` があっても理由を表示する。エンジンのテスト（SetStandard と `271b6909` の形の移行）と GUI のスナップショットを足した。今の移行の欠陥なので、SetStandard を待たない WP-S10（v0.2.0 を勧める）にした（B.7、B.11、B.13、F.1、G） |
| SAFETY-4（minor）: 固定できない入力（ほかのドライバーのキーボード、書き込まない仮想の kbdhid、リモート操作のソフトのキー）も標準配列に従うのに、下見は「変更できません」、I8 は予想される表が `None` なので中身なしに成り立ち、パスワードの警告も出ない | 採用（一部を変えて） | 読み取り専用の kbdhid は保存値から予想できるので「{to} になります（新しい標準に従います。MKLM はこのキーボードに配列を割り当てられません）」、ほかのドライバーは報告する種類が分からないので「標準配列に従う場合は {to} になります」と示す（指摘の文は「従います」と言い切るが、hyperkbd などの種類は確かめていないため）。`StandardRole::NotAssignable { follows }`、`StandardChange::not_assignable`。0.2 と B.3 に「原則 2 が守れるのは kbdhid と i8042prt のデバイスノードだけ」を書いた。パスワードの警告は常に出す（SAFETY-2）。確認の注記に固定できないキーボードの文を足した。I8 のテストは対象の外を明示する（0.2、0.4、B.3、B.4、B.6、B.10、B.13、F.1、H.11） |
| SAFETY-5（minor）: 自分の表を持たない種類（7/0 など）で固定された PS/2 は INV-PS2 を満たすが標準配列に従うのに、設計は PS/2 をすべて「変わりません」とする | 採用（案の 1 つ目） | phantom を含め、そのような i8042prt があれば `OperationError::Ps2WithoutTable` で断り、［変更…］で JIS か US を割り当てるよう案内する（原則 3 を「自分の表を持つ値で固定されている」に強めた）。同じ操作で from に固定し直す案は、PS/2 の書き込みと戻しの i8042prt の段階の分け方を持ち込むので採らなかった（J.9）。fixtures とテストを足した（0.5、B.3、B.4、B.13、F.1） |
| SAFETY-6（minor）: 片方だけの値を `predict_type` の値ごとの既定で判定しているが、ドライバーの扱いは確かめていない。固定しても、しなくても、どちらかの動きでは配列が変わる | 採用 | 書き込める kbdhid と i8042prt（phantom を含む）に `IncompletePair` があれば `OperationError::IncompleteValues` で断り、［変更…］（`set`。値を組で書く）を案内する（未接続なら「非表示と未接続も表示」）。follower の定義から片方だけの値を外した。読み取り専用のものは書かないので断らない（0.4、B.3、B.4、B.13、F.1） |
| SAFETY-7（minor）: 記録に全体のペアがないので、書いてから確定までに設定アプリが固定モードにしても、`RebootObserved` も `confirm` もそのまま確定する | 採用（案と違う形で） | エンジンの特別な規則ではなく、**確認用の記録**にした: 全体の `OverrideKeyboardType/Subtype` の記録を `before` = `intended` = `Absent` で持ち、書かず、baseline も作らない。ペアが現れると、書くときの CAS、`RebootObserved`（`Elsewhere`）、`confirm`、取り消しの CAS、回復のすべてで既存の規則が `Conflict` にし、衝突の画面に値が出る（記録なしの `Conflict` は解決する値がない。J.10）。古いビルドも同じに動く。衝突の画面に「固定モードになりました」の注記を出し、おすすめは「変更前の値に戻す」。B.8 の印の補いの規則を「確認用の記録だけか、ない」に直した（0.4、B.2、B.5、B.7、B.8、B.13、F.1、H.12） |
| SAFETY-8（minor）: phantom を既定で固定するので、どの SetStandard も未検証の m2 I.3 に依存する。書けなければ毎回ロールバックで、全体に達していなくても再起動を求める。計画の後に phantom が消えても失敗する | 採用 | (a) VM での m2 I.3 の確認（R-PHANTOM）を SetStandard の公開の条件にし、書けなければ未接続を既定で固定しない形に変えてから公開する。(b) プロセスの中の失敗で起動時の値を 1 つも書いていなければ、ロールバックの終わりを `Reverted` にする（`Forward::Failed { wrote_boot_time }`。I.12 の電源断の理由は書いていない値には当てはまらない。クラッシュからの回復は今までどおり保守的）。(c) SetStandard の固定の書き込みが `DeviceRemoved` なら `skipped` にして続ける（B.3、B.6、F.1、F.3、H.4） |

### 観点 2: 画面と文言、多言語、アクセシビリティ（UX-1〜16）

| 指摘 | 採否 | 変更 / 理由（節） |
|---|---|---|
| UX-1（major）: 「再起動して新しくサインインするまで反映されません」「届くのは…の後」などは、報告に従うなら何も届かず、割り当ては決して届かず、H.3 のとおり再起動の前に届くかもしれないので誤り。F5 は H1(a) と (b) を分けていないのに「分かっていること」にある。段落 1 と段落 2 が矛盾して読める | 採用 | どの仕組みとどの H1 でも成り立つ文に置き換えた（「今のセッションのキー配列は、セッションが終わるまで変わりません（再接続しても同じ）」「キーボードごとに割り当てた配列は、リモート デスクトップのキー配列を変えません」「標準配列を変えて再起動すると、その後に始めたセッションのキー配列が変わることがあります（まだ確かめていません）」）。F5 を「分かっていないこと」に移し、観察した事実は F7（再接続で報告の表にならなかった）として書いた。入力方式の案内の段落 1 は残さず、英数の代わりのキーの段落に書き直した。C.2 の禁止に条件なしの「まで反映されません」「届くのは」「reaches it only after」「fixed when the session starts」「サインインしたとき」を足し、3 つのテストの直し方を書いた（C.1、C.2、C.3、E） |
| UX-2（major）: 固定だけの SetStandard で「配列が変わるキーボードはありません」「変更できません」と書き、パスワードの警告も出さないが、RDP の表は標準配列に従うかもしれない。NLA なしの RDP のサインイン画面のパスワードも同じ | 採用 | 文を「{pc}につないだキーボード」に限り、RDP の但し書きを足した。RDP の行の「再起動の後」は「{to} になることがあります（下の説明）」。RDP のキーボードがあるかリモートのセッションなら、パスワードの警告に RDP の 1 行を足す（警告そのものは SAFETY-2 で常に出す）（B.3、B.7、B.12、B.13） |
| UX-3（major）: 勧めを接続元のキーボードで分けているが、報告に従うなら効くのは接続元の Windows の設定で決まる報告。「接続元の報告」の説明がなく、下見の RDP の行に報告が出ず、「確かめました」は 1 回の報告に頼る | 採用 | 勧めを報告（「接続元の報告: 日本語キーボード (JIS)」と出る場合）で分け、どこに出るかを書いた（CLI は `client reports keyboard type 0x7/0x2`）。「接続元の報告」を 1 か所で平易に定義した（段落 4、0.4）。標準配列のページの RDP の行に `remote_client_report` を出す。英語の報告は未確認と書く。「確かめました」は画面から外し、README と研究メモにだけ「1 台の PC で」と日付付きで書く（0.4、B.12、C.2、C.3） |
| UX-4（major）: 利用者は両方の PC で MKLM を使う。報告に従うなら、接続元の MKLM の書き込みで報告が変わるかもしれないのに、インストールの案内は影響がないように読め、「まだない」は連携を約束する | 採用 | 段落 4 とインストールの案内に「接続元の MKLM が書くのは接続元の値だけ。ただし接続元で移行や標準配列の変更をすると報告が変わるかもしれない（未確認）。変えた後は報告を確かめる」を書いた。実験 E11（接続元の変更の前後で接続先の報告を記録）を E と F.3 に足し、E7 の記録に接続元の報告、モード、標準配列を必ず添えることにした。「連携の機能はありません」に直した（0.1、0.4、B.14、C.3、E、F.3） |
| UX-5（major）: RDP のセッションでは「この PC」が接続先か接続元か分からない。この利用者は接続元でも MKLM を使う | 採用（一部を変えて） | リモートのセッションで出す文、README、インストールの案内では、「接続先の PC（この画面の PC の名前）」と「接続元の PC（今お使いの PC）」の両方を書く（`{pc}`、`i18n::pc`）。PC の名前は `OsInfo::computer_name`（新規、任意）。段落 3 は「{pc}の標準配列を JIS にし、{pc}につないだ US キーボードに US」。英語も "the PC you connect to (NAME)" / "the PC you connect from"。名前は RDP の行の注記ではなく、状態行の下のリモートの注記とリモート デスクトップのページに出す（行の注記は報告の文のまま）（0.4、A、B.13、C.3、C.4、D.2） |
| UX-6（minor）: 固定だけの変更を RDP から確かめるときも「この PC の前で確かめて」を求め、Keep の結果はコレクションごとに全行（マウスのドングルも）を出す。セッションのキー配列を確かめる手段がない | 採用 | 確認の行は配列が変わる物理キーボードだけ、物理キーボードごとに 1 行（`check_groups`。どの種類の操作でも）。配列が変わるものがなければ「ここで［このままにする］を選んでかまいません」。Keep の結果の名前も同じ。リモートのセッションのキーのテストに、Shift+2 の文字からセッションのキー配列を示す読み取りを足した（どちらの仕組みでも正しく、E7 の記録にも使える）（B.7、B.10、B.13、D.2、E、F.3） |
| UX-7（minor）: 「変更前の標準配列で動いています」は配列の名前がなく、H.3 のとおり偽になりうる。再起動の前のサインアウトを止める文がない。RDP のセッションでどちらの文が出るかが決まっていない | 採用 | 「PC を再起動すると US（新しい標準配列）になります。それまでは、ふつうは JIS（変更前）です」にした。再起動の画面に「早めに変わることがあります…今すぐ再起動を」を足した（SAFETY-2 と共通）。リモートのセッションで見えない行は「このリモート デスクトップのセッションからは見えません。」の後にこの文を続ける（B.12、B.13） |
| UX-8（minor）: 1 つの操作に［変更…］［PC の標準配列…］の 3 つの名前がある。段落 3 は固定モードでも出るが、そこではボタンがなく移行が要る | 採用 | どこでも［PC の標準配列を変更…］/ "Change the PC's standard layout…" にし、ウィザードの文も合わせた。固定モードでは段落 3 の固定モードの版（移行のときに標準配列に JIS を選ぶ）を出す（B.12、B.13、C.3、C.4） |
| UX-9（minor）: 状態行は折り返す 4 列で、最小の幅では 1 列が約 83 px。ボタンは折り返せず、`StatusItem` は 1 つの `text` で子は読まれない。ふさがっている理由に見える場所も読み上げもない | 採用 | 入口を状態行の下の独立した行に移し（`mode_note` と同じ）、ふさがっている間は `cannot_change_now` の文を横に出し、同じ文をボタンの `accessible-description` にする。`StatusItem` は文字だけのまま。T-A11Y-4（m3 H.4）に、最小の大きさと文字の倍率でこの行とページを見る項目を足す（WP-S9）（B.12、G、J.12） |
| UX-10（minor）: 3 列の表で最後の列にチェックと「→ JIS」があり、CheckBox は折り返さない。チェックの読み上げ名が外したときの結果を言わない。未接続のまとめは既定で閉じていて書く先を隠し、役割と開閉の状態が決まっていない | 採用 | 1 台を 1 つのブロックにした（名前、「設定した配列（今）: … → 再起動の後: …」、チェックは自分の行）。ブロックは `list-item` で要約を読み上げ、チェックの説明は結果の配列（チェックで変わる）。未接続の開閉は `button` で `accessible-expandable` / `accessible-expanded`（Slint 1.18.1 にあることを確かめた）、閉じた文が何を書くかを言う（B.12、B.13） |
| UX-11（minor）: バッジの長い文は読み上げ名だけで、行を移るたびに約 200 文字が読まれ、目で見る人には見えない。入力方式の案内にしかなく、見出しも行からのリンクもない | 採用（案と違う形で） | バッジの読み上げは短い文にした。RDP の行の［変更…］の位置に［説明…］を置き、行の既定の操作と合わせて新しい「リモート デスクトップで使うとき」のページ（`Page::RemoteHelp`）を開く。状態行の下のリモートの注記、入力方式の案内、標準配列のページからも開く。入力方式の案内の見出しへ飛ぶ案は、Slint 1.18 に見出しの位置へ `ScrollView` を動かす仕組みがないので、別のページにした（J.11）（C.3、C.4） |
| UX-12（minor）: 同じ状態に違う言葉（「標準に従う（JIS）」「標準（JIS）」「JIS（標準）」、「（割り当て）」、列の名前、CLI の Now、'not visible' の 3 つの英語、'the remote PC' の 2 つの意味、確認の注記の 2 つの版、再起動の文の「シャットダウンではなく」の抜け、英語の操作の名前） | 採用 | ページと確認の「設定」に `i18n::effective` を使い、「（割り当て）」をやめた。列は「設定した配列（今）」「再起動の後」/ 'Set now' 'After the restart'。CLI の列は 'Stored'。英語は 'not visible in this Remote Desktop session' の 1 つにし、`list` の最後の行も直した。'the remote PC' をやめ 'the PC you connect to / from' にした。確認の注記は B.13 の 1 つの版。再起動の文に「（シャットダウンではなく再起動）」。英語の操作の名前は 'Standard layout JIS → US'（B.10、B.11、B.12、B.13、C.3、D.2） |
| UX-13（minor）: 多くの新しい文が日本語の本文か画面の絵にしかなく、B.13 に日英がない | 採用 | B.13 をページ、入口、拒否、結果、再起動の画面、再起動を待つ間、再起動後の確認、衝突と取り消しに分けて日英で埋め、C.3 と D.2 にも日英の表を置いた。F.1 の GUI のスナップショットの対象（`vm::standard`、`vm::status`、`vm::keyboards`、設定、ウィザードなど）に入れた（B.13、C.3、D.2、F.1） |
| UX-14（minor）: `NotFollowingStandard` は何も書いていないのに再起動を待たせ、GUI では起きない（CLI が GUI のボタンを指す）。英語の選択肢の名前が違い、PS/2 と RDP の文がない。`UnknownStandard` は次の手を言わない | 採用 | `NotFollowingStandard` は CLI だけの文にし、割り当て済み（`mklm-cli set <keyboard> --layout standard` を再起動の後に）、PS/2、RDP、固定できないキーボードの 4 つを書いた。`UnknownStandard` に「キーボードごとの配列は今までどおり変えられます」と、技術的な詳細の今の値を足した（B.4、B.11、B.13） |
| UX-15（minor）: D.2 は「RDP のセッションはキーボードを見られない」と決めつけないと言いながら、文で一般化している | 採用 | 文を「このリモート デスクトップのセッション」についてだけ言う形にし、括弧の理由は `NotListed { remote: true }` のときだけ付ける。エンジンは `Host::remote_session()` でリモートかを知り、コンソールでは RDP に触れない文にする（D.2、F.1） |
| UX-16（minor）: phantom の固定が書けないとき、利用者が見るのは汎用のレジストリのエラーで、キーボードの名前がなく、効かない再起動を勧める。未接続の行は閉じている | 採用 | `FailureReason::WriteError` に任意の `target` を足し（古いビルドは無視する）、固定の書き込みの失敗に専用の文（名前、何も変えていないこと、未接続ならその行のチェックを外せば進めること）を出し、再起動を勧めない（SAFETY-8 (b) で終わりも `Reverted`）。ページに戻ると未接続のまとめを開き、失敗した行を文で示す（B.6、B.8、B.12、B.13） |
