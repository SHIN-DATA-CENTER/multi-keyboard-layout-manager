# M3 設計: GUI（apps/mklm）と共有クライアント（mklm-client）

| 項目 | 内容 |
|---|---|
| 対象 | マイルストーン M3（プラン 6 章「GUI」） |
| 根拠 | 承認済みプラン 1.4、2.1〜2.2、3.1〜3.13、用語、6 章。M2 設計（`docs/design/m2-engine.md`）。M0 と M2 の実機結果（`docs/research/m0-results.md`、`docs/research/m2-real-tests.md`）。Slint 試作（`prototypes/slint-proto`、手動確認 A〜G 合格） |
| 状態 | 設計（レビュー対応済み。末尾の「レビュー対応」）。骨組みのコードはコンパイル済みで、`cargo build / clippy -D warnings / test --workspace` が通る（テスト 512 件、既存の 450 件を含む）。GUI は起動して実データの一覧を表示する（0.3） |
| 読み手 | M3 を分担して実装する人（I 章に作業の分け方） |

識別子（型、関数、値の名前）は英語のまま書く。「計画」は承認済みプラン、「m2 D.2」は M2 設計の節、「R5」などは M2 の実機テスト、「M0 #8」は M0 の検証番号、「U3」「A5」などは末尾のレビュー対応の番号を指す。画面の文言は日本語 UI のもので、英語 UI では D 章の仕組みで英語になる。

---

## 0. 前提と方針

### 0.1 これまでに確かめた事実と、GUI への影響

| 事実 | 出典 | GUI への影響 |
|---|---|---|
| Slint 1.18.1 と winit 0.30 で、日本語 IME、IME を通さない `FocusScope` の打鍵テスト、`device_event` によるキーボードの特定、トレイ、アプリ側のテーマ判定がすべて動いた | M0 #8（A〜G 合格） | 試作の方式をそのまま使う（C 章、A.4） |
| winit の `set_theme` は設定を保持しない。`with_theme(Some)` で作ったウィンドウだけが、無関係な `WM_SETTINGCHANGE` の後もテーマを保つ | M0 #8 D | ウィンドウは解決済みのテーマで作り、以後は `set_theme` と `DwmSetWindowAttribute(20)` を両方呼ぶ（C.1） |
| Slint のトレイはメッセージ専用ウィンドウなので、explorer の再起動後に戻らない。バルーンの API もない | M0 #8 E、F | 表示しないトップレベル ウィンドウで `TaskbarCreated` を受けて作り直す。初回の通知はバルーンを使わず、ウィンドウ内に出す（B.16） |
| ソフトウェア描画は約 28 MB、femtovg は約 127 MB | M0 #8 | 常駐アプリなのでソフトウェア描画を既定にする（C.3） |
| `WindowEvent::KeyboardInput` の `device_id` はダミー。キーボードの特定には `DeviceEvent::Key` を使うしかない。離したときの文字は配列を反映しないことがある | M0 #8、試作 README 3, 18 | 押したときだけを使う。入力元は直前の `DeviceEvent::Key` で決める（B.3） |
| `DeviceEvent`（Raw Input）はウィンドウが前面のときだけ届く（winit 0.30.13 の既定 `WhenFocused`） | 骨組みの確認 | マウスだけの利用者はキー入力が見えない。入力の申告の既定値にポインターの使用も数える（B.5）。質問が開いたらウィンドウを前面に出す（B.6） |
| USB の kbdhid はその場のリセットで反映される。BLE と BT は未実証で、再接続の経路 | M0 #2b、G2 | USB はカウントダウン（B.6）、BLE / BT は再接続待ち（B.7） |
| Raw Input は override を反映するが、移行の反映は Raw Input では判別できない | M0 #2a、G3、m2 0.1 | 再起動後の確認は「Windows の認識」と打鍵テストの両方を出し、移行では打鍵で決める（B.9） |
| helper を別プロセスにしたので、呼び出し元が強制終了されても、カウントダウン中の変更は helper が戻す | R16（m2 C8） | GUI の強制終了では安全。ただしサインアウトやシャットダウンでは helper も終了させられうるので、最後の砦は次回起動時の回復になる（F.5） |
| 利用者は説明を読む前に UAC の「はい」を押しやすい | R12 | UAC の説明は UAC を出す前に必ず見せる。初回は独立した画面、2 回目からは変更ボタンの横の 1〜2 行（B.5） |
| ジャーナルの時刻が UTC で表示される | R4 | ローカル時刻で表示する（G.1） |
| helper がリセットの前に止まっても、回復の理由が「リセットの後に確定されなかった」と表示される | R5 | リセットに届いたかで文言を変える（G.2） |

### 0.2 設計の原則

1. **GUI は書かない。** `mklm.exe` は `asInvoker` で動き、キーボードの設定（HKLM）には一切書かない（計画 2.1）。書き込みはすべて `mklm-helper.exe` が行う。GUI 自身が書くのは利用者ごとのもの 3 つだけ: `settings.toml`、HKCU の自動起動の値、HKCU の RunOnce の値（どちらも `mklm_win::session` 経由）。
2. **CLI と GUI は同じクライアント コードを使う。** helper の起動、パイプの中継、下見、結果の分類、RunOnce の規則、起動時の確認は `crates/mklm-client` にあり、CLI と GUI が共有する（A.2）。規則の判断は M2 と同じく `mklm-core` にある。フロントエンドに残すのは、表示と入力だけ。
3. **UI スレッドでは待たない。** 列挙、ジャーナルの読み取り、helper の起動（UAC）、ファイルの保存は、すべてワーカー スレッドで行う（A.4）。例外は、セッション終了の通知の中での上限つきの処理だけ（F.5）。
4. **画面は状態の関数。** `AppState` を `update` だけが変え、ビュー モデル（`vm`）が状態から画面の内容を作る。どちらも Slint にも Windows にも依存しないので、ウィンドウなしでテストできる（A.6、H.1）。
5. **翻訳は型で漏れを防ぐ。** Rust が組み立てる文言は、型ごとの網羅的な `match` で日英を持つ。`.slint` の固定の文言は `@tr` と同梱の `.po` で、テストが漏れを検出する（D 章）。
6. **利用者が見てから決める。** 確定は、カウントダウン中の打鍵テストか、再起動後の確認画面を見た利用者だけが行う。GUI は、利用者がボタンを押したときにだけ UAC を出す。唯一、helper を失った直後の回復は GUI の側から尋ねる（オーバーレイで理由を示し、「今すぐ元に戻す」が押されたときだけ helper を起動する。A.2.3、K.6）。
7. **キーボードだけで操作でき、読み上げで状態が分かる。** 状態は色だけで示さない（E 章）。
8. **決める画面には技術用語を出さない。** 「Raw Input」、16 進の値、KLID、値の名前、「helper」は、利用者が判断する画面に出さず「技術的な詳細」に置く。「✓」は「設定どおり」の意味だけに使う（D.5、B.2）。

### 0.3 このコミットに含まれる骨組み

「実装済み」は動作しテストがあるもの、「スタブ」は型と関数の形だけがあり、本体が `todo!()` か未配線のもの（I 章の WP で実装する）。GUI のスタブは実行時に呼ばれないので、起動しても `todo!()` に当たらない。

| 場所 | 中身 | 状態 |
|---|---|---|
| `crates/mklm-client/` | `session`（`Link`、`Frontend`（`confirm_recovery` を含む）、`SessionView`、`relay`、`plans_first`、取り消し）、`orchestrator`（`Orchestrator::run` / `run_then`、`LaunchError`、`RecoverySkip`）、`outcome`、`gate`、`startup`、`preview`、`values`、`describe`、Windows 専用の `launch`、`journal`、`inventory`、`run_once`（`run_request`） | 実装済み。CLI から移したコードと、テスト 19 件 |
| `apps/mklm-cli/src/write/` | `relay.rs`、`launch.rs`、`outcome.rs`、`checks.rs`、`preview.rs`、`render.rs`、`commands.rs` の一部が、`mklm-client` の薄いアダプターになった | 実装済み。CLI の動作と 51 件のテストは変わらない |
| `crates/mklm-win/src/ui/`（feature `gui`） | `theme`（UISettings、テキストのサイズ、ハイコントラスト、DWM）、`shell_window`（`TaskbarCreated`、`WM_SETTINGCHANGE`、セッション終了、復帰）、入力言語、表示言語、設定フォルダー、`open_settings_page`（System32 のプログラムは絶対パス） | 実装済み |
| 同 `ui/mod.rs` | `copy_text_to_clipboard` | スタブ（WP-W2） |
| `crates/mklm-win/src/` | `time`（ローカル時刻）、`machine_settings::read_machine_settings`、`session::autostart_state` | 実装済み |
| 同 | `instance`（多重起動の防止。`is_our_instance` とテストは実装済み）、`notify`（キーボードの到着）、`machine_settings::write_restore_on_uninstall`、`session::register_autostart` / `unregister_autostart` | スタブ（純粋な部分とテストは実装済み） |
| `apps/mklm/` | `Cargo.toml`（slint ~1.18、`unstable-winit-030`）、`build.rs`（slint-build、同梱の翻訳、embed-resource、ビルド ID、リンク オプション）、`res/`（マニフェスト、VERSIONINFO） | 実装済み |
| `apps/mklm/ui/` | `app.slint`（ナビゲーション、12 ページ、7 オーバーレイ、トレイ）、`structs.slint`、`theme.slint`（文字の倍率）、`widgets.slint`（`RadioList`、`CaptureArea`、`DialogFrame` / `DialogBody` / `DialogButtons`、`KeyTestArea`）、`screens/*.slint` | 画面の骨組み。メイン画面は実データを表示する。ほかの画面は表示だけで、コールバックの多くは未配線 |
| `apps/mklm/translations/ja/` | `mklm.po`（124 件） | 実装済み。`tests/translations.rs` が漏れを検出する |
| `apps/mklm/src/` | `app`（起動、テーマ、描画（モデルの差分更新）、トレイ、ウォッチャー、セッション終了）、`args`、`theme`、`i18n`、`settings`、`state`（セッションの段階と ID）、`detect`、`worker`（`SessionShared`、パニック時の終了通知、回復の質問）、`reader`、`watchers`、`input_capture`（キーとポインター）、`tray`、`icon`、`vm::{status, keyboards, session, journal, keytest}`、`vm::{change::default_apply_method, conflict::choices, wizard::wizard_plan, list_ops}` | 実装済み（テスト 35 件） |
| 同 | `vm::{change, conflict, post_reboot, recovery, restart, result, wizard}` の画面の組み立て、`single_instance`、`autostart` の書き込み | スタブ |

**起動の確認**（この PC、デバッグ ビルド）: `target\debug\mklm.exe --exit-after=5 --lang=ja` と `--lang=en --theme=light` は、どちらもパニックせずに終了コード 0 で終わり、プロセスは残らなかった。以前の確認では、日本語とダーク、英語とライトのどちらでも、状態行とキーボード一覧（内蔵 PS/2 と Keychron Receiver）が実データで表示され、英語 UI でも日本語の機器名が正しく描画された（ワーキング セット約 35 MB）。

---

## A. アーキテクチャ

### A.1 クレート構成と依存

```
                     mklm-core（純粋。規則、判定、共有の型）
                ▲        ▲          ▲            ▲
                │        │          │            │
           mklm-ipc  mklm-engine ─▶ mklm-win（Win32。unsafe はここだけ。
                ▲     ▲      ▲        ▲         feature "gui" で GUI 用の ui モジュール）
                │     │      │        │
                │  mklm-helper  mklm-cli ──┐
                │                          ▼
                └──────────────── mklm-client（呼び出し側の共有コード。unsafe なし）
                                           ▲
                                           │
                                  mklm（GUI。lib = mklm_gui、bin = mklm.exe）
                                  core + ipc + client + win(gui) + slint
```

| クレート | M3 での役割 | unsafe |
|---|---|---|
| `mklm-client`（新規） | helper の起動と中継、下見、結果の分類、ジャーナルの規則、起動時の確認。CLI と GUI が共有する（A.2） | 禁止（`forbid`） |
| `mklm`（新規） | GUI。ライブラリ `mklm_gui` と薄いバイナリ `mklm.exe` | 禁止（`deny`。Slint の生成コードのモジュールだけ許可） |
| `mklm-win` | GUI 用の `ui` モジュール（feature `gui`）と、`instance`、`notify`、`time`、`machine_settings`、自動起動を追加（A.5） | ここだけ |
| `mklm-core`、`mklm-ipc`、`mklm-engine`、`mklm-helper` | M3 で足すのは、設定の保存（WP-E2）、ドライバーが読まない値の削除（WP-E1）、確認の時間とセッション終了時の戻し（WP-E3）だけ（A.5） | 変わらない |

- GUI は engine に依存しない（m2 A.1 のとおり）。計画の確認は core、実行は helper。
- `mklm-helper` の依存に UI のクレートは入らない。feature `gui` は `apps/mklm` だけが有効にする。ワークスペースを一度にビルドすると feature が統合されて helper にも `ui` のコードがリンクされうるが、使われないので最終的な実行ファイルには残らず、M5 の `cargo tree -p mklm-helper` の検査（クレート単位）にも影響しない。
- ビルド ID（m2 E.3）は GUI も同じ `apps/build_id.rs` で計算して埋め込む。GUI も helper を起動するため。`mklm-client` はハッシュの対象に含めない。helper は client を使わず、呼び出し元と helper の間の約束（ipc と core）はすでに対象だから。

### A.2 共有クライアント（`crates/mklm-client`）

#### A.2.1 モジュール

| モジュール | 公開 API | 役割 |
|---|---|---|
| `session` | `Link`、`Recv`、`SessionEnd`、`RelayConfig`、`Frontend`、`SessionView`、`Prompt`、`Arrival`、`Notice`（`Starting`、`Connected`、`HelperLost`、`RecoveringAfterLoss`）、`SessionKind`、`CancelAction`、`cancel_action`、`plans_first`、`relay` | 1 つの要求の中継。表示とは無関係 |
| `orchestrator` | `Orchestrator<L, J>`（`new`、`run`、`run_then`）、`Launcher`、`HelperLink`、`JournalSource`、`RequestReport`（`recovery_skipped` を含む）、`RequestEnd`、`RecoverySkip`、`LaunchError`、`LaunchFailure` | 要求の全体: 起動 → 中継 → helper を失ったら、同意を得て回復（m2 E.7、C8。A.2.3） |
| `outcome` | `OutcomeClass`、`classify_result`、`classify_lost_recovery`、`classify_error`、`reverted_automatically`、`HelperExit`、`HelperExitKind` | 結果の分類。CLI は終了コードに、GUI は結果画面に写す |
| `gate` | `Gate`、`blocker`、`BlockReason`、`OpRef`、`RunOnce`、`run_once`、`takes_effect_at_restart`、`post_reboot_entries`、`restart_reasons` | 非昇格でジャーナルから決めること（m2 C.7 の `attention`、D.7、F.4） |
| `startup` | `summarize`、`StartupSummary`（`needs_recovery`、`blocks_writes`、`post_reboot_due`、`with`）、`AttentionItem` | 起動時と読み直しのたびの「注意が必要なもの」のまとめ（B.12） |
| `preview` | `expected`、`nothing_to_change`、`no_other_input`、`preview_restore`、`RestorePreview`、`RestoreRow`、`other_keyboard_usable`、`undo_preview`、`UndoPreview`、`check_rows`、`CheckRow` | 下見のデータ（m2 F.2 の 4） |
| `values` | `model_value`、`op_value` | 非昇格のスナップショットから読んだ値 |
| `describe` | `reset_phase`、`reset_phase_from`、`ResetPhase` | 文言を正しく選ぶための事実（G.2） |
| `launch`（Windows） | `LaunchConfig`（`current`）、`start`、`checked_helper_path`、`HelperSession`、`HelperLauncher` | helper の起動とハンドシェイク（m2 E.1〜E.3）。起動の失敗は Win32 のエラー コードを `LaunchFailure::StartFailed(Option<u32>)` に残す（B.17） |
| `journal`（Windows） | `read_journal`、`JournalRead`、`liveness`、`boot_id`、`LiveJournal` | 非昇格でのジャーナルの読み取り |
| `inventory`（Windows） | `read_inventory`、`Inventory`、`InventoryError`、`read_display_snapshot` | 計画用の列挙（書き込みを止める読み取りの問題を区別）と、表示用の列挙 |
| `run_once`（Windows） | `apply_run_once_rule`、`run_request`、`RequestOutcome`、`PostRebootCommand`（`Cli`、`Gui`）、`RunOnceOutcome`、`RunOnceError`、`GUI_EXE` | RunOnce の規則（m2 F.4、C17）。`run_request` は要求の直後に同じスレッドで規則を適用する |

`mklm-client` は利用者向けの文言を持たない。持つ文字列は、ログと CLI 用の英語の診断（`LaunchError::Failed::message`、`HelperExit` の `Display`）だけ。

#### A.2.2 `Frontend` の契約

```rust
pub trait Frontend {
    fn event(&mut self, event: &Event, view: &SessionView) -> io::Result<Option<Decision>>;
    fn poll(&mut self, view: &SessionView, now: Instant) -> io::Result<Option<Decision>>;
    fn finish(&mut self) -> io::Result<()>;
    fn cancel_requested(&mut self) -> bool { false }
    fn notice(&mut self, notice: &Notice) -> io::Result<()> { Ok(()) }
    fn confirm_recovery(&mut self, countdown: bool) -> io::Result<bool> { Ok(true) }
}
```

| メソッド | 呼ばれる時点 | GUI | CLI |
|---|---|---|---|
| `event` | helper のイベントを受けるたび。`view` はそのイベントを反映済み（`SessionView::observe` の後）。Heartbeat も届く | Heartbeat 以外を `AppMsg::SessionEvent` として UI スレッドへ送る。決定は返さない | `Presenter` で表示する。`--answer` の決定を返す。質問の直後に先行入力を捨てる |
| `poll` | `RelayConfig::tick`（200 ms）ごと | ボタンからの決定をチャネルから取り出す（`try_recv`） | 標準入力の行と、`--answer keep` の時間切れを見る |
| `finish` | 要求が終わる直前 | 何もしない | 書き換え中のカウントダウン行を閉じる |
| `cancel_requested` | 毎 tick と、helper を失った後の回復の前 | 終了やセッション終了のフラグ（`SessionShared::cancel`、`AtomicBool`） | 常に false（Ctrl+C はプロセスを終わらせ、パイプが切れる） |
| `notice` | セッションの間（`Starting`、`Connected`、`HelperLost`、`RecoveringAfterLoss`） | `AppMsg::SessionNotice` として UI へ送る。`Connected` で状態が「UAC 待ち」から「実行中」に移る（B.18） | （WP-C1 で）「The helper stopped …」の 2 行を出す |
| `confirm_recovery` | helper を失い、ジャーナルが回復を求めたとき（`cancel_requested` が false のときだけ） | `AppMsg::RecoveryQuestion` でオーバーレイを出し、答えをチャネルで待つ。待つ間の終了は「いいえ」 | 既定の true（すぐに回復する。m2 E.7 のまま） |

**決定の検査**: relay は、決定が開いている質問に答えるとき（`SessionView::accepts`: カウントダウンか再接続待ちで、同じ操作 ID）だけ送る。それ以外は捨てる。質問の前に押されたボタンや、別の操作への答えが、見ていない変更を確定させないため。送ったら `Prompt::Answered` にする。

**取り消し**（`cancel_action(view, request)`）: `cancel_requested` が true の間、relay は毎 tick 次を判断する。「計画を先に記録する要求」（`plans_first`）は `SetLayout`、`Migrate`、`RestoreBaseline`（と WP-E1 の `CleanupValues`）だけ。engine はこれらだけを `create` で記録して `Planned` を送り、書く前に `check_cancelled` で呼び出し元の離脱を確かめる（m2 S4）。`Revert`、`Undo`、`Recover`、`Confirm`、`ResolveConflict` は既存のエントリに対して、`Planned` なしで書き込みやリセットをしうる（A4）。

| 状態（`SessionView`） | 動作 | 理由 |
|---|---|---|
| カウントダウン中 | `Decision::RevertNow` を送り、結果を待つ（1 回だけ） | すぐに元に戻すのが最も安全。切断しても helper は戻す（C8）が、結果を受け取れない |
| 再接続待ち | 中継をやめる（`SessionEnd::Abandoned { planned: true }`）。パイプが閉じ、変更は `AwaitingConfirm` のまま残る | 自動では戻さない規則（m2 D.2 b）。GUI は終了前にこの状況を利用者に尋ねる（F.5） |
| `plans_first` の要求で、まだ `Planned` が来ていない | 中継をやめる（`Abandoned { planned: false }`） | helper は `check_cancelled` で何も書かずに終える（m2 S4） |
| それ以外（書き込み、リセット、戻し、`Planned` のない要求） | 待つ（次の tick でまた判断する） | 途中で切っても helper は書き切る。結果（衝突、再起動が必要など）を捨てないため。後でカウントダウンが始まれば、その時点で `RevertNow` を送る |

#### A.2.3 `Orchestrator::run`

```rust
pub fn run(&mut self, request: Request, apply: ApplyOptions, frontend: &mut dyn Frontend)
    -> io::Result<RequestReport>;
pub fn run_then<T>(&mut self, request: Request, apply: ApplyOptions, frontend: &mut dyn Frontend,
    after: impl FnOnce() -> T) -> (io::Result<RequestReport>, T);
pub struct RequestReport {
    pub first: RequestEnd,                              // NotLaunched(LaunchError) | Ended(SessionEnd)
    pub lost_needs_recovery: Option<Result<bool, String>>,
    pub recovery: Option<RequestEnd>,
    pub recovery_skipped: Option<RecoverySkip>,         // Cancelled | Declined
}
```

1. `notice(Starting(Request))` → `Launcher::launch`（UAC はここで出る）→ `notice(Connected(Request))` → `relay` → `HelperLink::close`（`Bye` を送り、最大 5 秒待つ）。
2. 最初のセッションが `SessionEnd::Lost` なら、`notice(HelperLost)` → `JournalSource::needs_recovery`（非昇格でジャーナルを読み、`attention == Recover` の項目があるか）。
3. 回復が必要なら:
   - `cancel_requested()` が true なら、起動しない（`recovery_skipped = Cancelled`）。終了を選んだ後に UAC を出さない（A1）。
   - `confirm_recovery(countdown)` が false なら、起動しない（`Declined`）。回復は起動時の確認とバナーが案内する（B.12）。
   - true なら、`notice(RecoveringAfterLoss { countdown })` → 同じ `apply` で `Request::Recover` のセッション（非昇格なら UAC がもう一度出る）。
4. RunOnce の規則（C17）は `run_then` の `after` で、同じスレッドで、結果によらず（`run` がエラーでも）適用する。Windows では `run_once::run_request(launch, request, apply, frontend, command)` がこれをまとめる。GUI のセッション ワーカーはこれを使い、結果を `SessionEnded` に入れて UI へ送るので、終了が規則の適用を追い越さない（A3）。CLI は WP-C1 で移る。

GUI の回復の質問（`RecoveryConfirmDialog`）:「新しい配列を試している間に、MKLM の管理用プログラム（mklm-helper.exe）が止まりました。キーボードを今すぐ元に戻すには、Windows がもう一度管理者の許可を求めます（発行元は「不明」と表示されます）」［後で］［今すぐ元に戻す］。「後で」には「MKLM が後でもう一度回復を案内します。それまで、キーボードの配列は変更できません」を添える。

`Launcher` と `JournalSource` を差し替えられるので、偽物の helper でテストできる（`orchestrator::tests`: UAC の拒否、失った helper の回復と通知の順序、同意しなければ起動しない、終了の後は起動しない、`after` は必ず走る）。Windows では `HelperLauncher { config: LaunchConfig }` と `LiveJournal` を使う。`LaunchConfig { build_id, elevated, owner_window }` の `owner_window` は、GUI ではメイン ウィンドウの HWND（UAC のダイアログの親）、CLI ではコンソール ウィンドウ。

#### A.2.4 抽出の状況（このコミットと WP-C1）

| 部分 | このコミット | WP-C1 |
|---|---|---|
| 中継ループ、`Link`、`SessionEnd` | client に移した。CLI は `CliFrontend` で使う。取り消しは要求の種類で決める | — |
| helper の起動 | client に移した。CLI は `LaunchConfig` を渡すだけ | — |
| 結果の分類 | client の `OutcomeClass`。CLI は `class_exit_code` で終了コードに写す | — |
| ジャーナルの規則 | client の `gate`（型付きの `BlockReason`）。CLI は同じ英語の文言に描く | — |
| 下見のデータ | client の `preview`、`values`。CLI の `undo_text` は `undo_preview` から描く | — |
| ジャーナル、列挙、RunOnce | client の `journal`、`inventory`、`run_once`（`run_request`）。CLI は警告の表示だけ | CLI も GUI がインストールされていれば GUI を登録する（`PostRebootCommand::preferred`、F.2） |
| 要求の全体（helper を失った後の回復と RunOnce） | client の `Orchestrator`（テスト済み）と `run_request`。GUI はこれを使う | CLI の `commands::session` / `finish` を `run_request` と `Frontend::notice` に置き換え、重複を消す。CLI は `confirm_recovery` の既定（true）を使うので、出力と終了コードは変わらない |

### A.3 CLI の扱い

- CLI の動作（出力、終了コード、質問）は変えない。`cargo test -p mklm-cli` の 51 件は、このコミットで手を加えずに通る（テスト モジュールの `use` だけ直した）。
- CLI の英語の文言は CLI に残る（`render.rs`、`preview.rs`、`checks.rs::blocker_text`）。client の型から描く。
- `SessionEnd::Abandoned` は CLI では起きない（CLI の `Frontend` は取り消しを求めない）が、網羅のため「Cancelled.」と終了コード 3 に写した。
- G 章の 2 つの修正（ローカル時刻、回復の理由）は CLI にも入れる（WP-G）。

### A.4 GUI のスレッド モデル

| スレッド | 数 | やること | UI スレッドへの戻り方 |
|---|---|---|---|
| UI（メイン） | 1 | Slint のイベント ループ。`AppState`、ビュー モデル、Slint のオブジェクト（ウィンドウ、トレイ）、`slint::Timer`、隠しシェル ウィンドウのウィンドウ プロシージャ、winit の `device_event` | — |
| セッション ワーカー（`mklm-session`） | 要求ごとに 1（同時に 1 つまで） | `run_once::run_request`: helper の起動（UAC を待つ）、パイプの中継、同意を得た回復、RunOnce の規則 | `slint::invoke_from_event_loop` で、セッション ID の付いた `AppMsg::{SessionNotice, SessionEvent, RecoveryQuestion, SessionEnded}`。決定と回復の答えは `mpsc` チャネル、取り消しは `SessionShared` |
| I/O ワーカー（`mklm-io`） | 1（常駐） | `IoTask`: 列挙とジャーナル（`Read`）、計画用の列挙（`PrepareChange`、WP-U3）、設定の保存、起動時と「後で決める」の RunOnce の規則、設定ページを開く、PC の再起動（`restart_pc`、WP-U4） | `invoke_from_event_loop` で `AppMsg::SystemRead` など。連続した `Read` は 1 回にまとめる |
| 多重起動のパイプ（`mklm-instance`） | 1（WP-W1） | `activate` / `quit` を待つ（1 接続 1 秒まで） | `invoke_from_event_loop` |
| WinRT のスレッド プール | OS | `UISettings.ColorValuesChanged`、`TextScaleFactorChanged` | `invoke_from_event_loop` |
| CfgMgr32 のスレッド プール | OS | `CM_Register_Notification`（キーボードの到着と取り外し、WP-W1） | `invoke_from_event_loop`、UI 側で 750 ms にまとめてから `Read` |

**セッションは同時に 1 つ**（A2）: `update` は `Effect::StartSession` を出すのと同じ呼び出しで `SessionPhase::Launching { id }` にする。`Idle` 以外では新しいセッションを始めない（2 回目のクリックや、UAC の表示中のトレイの「元に戻す」は何もしない）。ワーカーのメッセージにはセッション ID が付き、古い ID のものは捨てる。`Controller::start_session` も、生きているワーカー（`!finished()`）があれば始めない。`LaunchConfig::current` やスレッドの起動に失敗したら、`SessionEnded(NotLaunched(Failed))` を送って `Idle` に戻す。

**`SessionShared`**（`worker.rs`）: `cancel`（`AtomicBool`）、`journaled`（`Planned` かカウントダウンを見た、または `Planned` のない要求の helper がつながった）、終わりの合図（`Mutex<bool>` と `Condvar`）。今のセッションの `SessionShared` は `Mutex` の中の静的な場所にも置き、セッション終了の通知からは `RefCell` を借りずに触る（F.5、A5）。ワーカーのクロージャーには終了を必ず送るガードがあり、パニックしても `SessionEnded(Lost { "the session worker panicked" })` が届く（A11）。

**規則**

1. UI スレッドで呼ばないもの: `snapshot_report` と `read_inventory`（数百 ms）、`read_journal`、`launch::start`（UAC）、`restart_pc`、`settings.toml` の書き込み、`ShellExecuteW`。例外は、ごく短い読み取り（`GetKeyboardLayout`、`SystemParametersInfoW`、`GetSysColor`、`UISettings.TextScaleFactor`、起動前の `settings.toml` の読み取り）と、F.5 のセッション終了時の処理（RunOnce の値を 1 つ書く、最大 3 秒待つ）。
2. ワーカーは Slint の型に触れない。UI へは `AppMsg`（`Send` な値）だけを送る。
3. シェル ウィンドウのプロシージャと winit の `device_event` は UI スレッドで呼ばれる。シェル ウィンドウからは原則 `invoke_from_event_loop` で予約するだけにする。例外は `WM_QUERYENDSESSION`（取り消しのフラグを直接立て、RunOnce を登録）と `WM_ENDSESSION`（最大 3 秒待つ）。予約した処理がセッションの終わりまでに走る保証がないため（F.5）。`device_event` は Slint のコールバックの外で呼ばれるので、`dispatch` で直ちに処理してよい（打鍵テストが、直後に来るウィンドウのキー イベントより先に入力元を知るため。M0 #8）。
4. `invoke_from_event_loop` は順序を保つ。セッションのイベントは届いた順に画面に反映される。
5. 入力方式（HKL）: `WM_INPUTLANGCHANGE` はフォーカスのあるウィンドウ（winit のもの）にしか届かないので、ウィンドウが見えている間だけ 500 ms ごとに `GetKeyboardLayout(0)` を読む（`slint::Timer`。`Effect::ShowWindow` で開始、`HideWindow` で停止。A10）。Preload の変更は `WM_SETTINGCHANGE("intl")` と、ウィンドウを表示したときの `Read` で拾う。
6. ウィンドウが非表示の間は描画しない（`Effect::Render` は `visible` のときだけ）。表示したときに 1 回描く。

### A.5 ほかのクレートへの追加

| クレート | 追加 | WP |
|---|---|---|
| `mklm-win` `ui::theme` | `read_os_theme`、`OsThemeWatcher`、`read_text_scale_factor`、`TextScaleWatcher`、`set_title_bar_dark(hwnd, dark)`、`high_contrast_on`、`system_colors`、`apps_use_light_theme`（診断用） | 済 |
| `mklm-win` `ui::shell_window` | `ShellWatcher`、`ShellEvent::{TaskbarCreated, SettingChange, QueryEndSession, EndSession, Resumed}` | 済 |
| `mklm-win` `ui` | `active_keyboard_layout`、`user_default_ui_language`、`user_settings_dir`、`allow_set_foreground_window`、`open_settings_page(SettingsPage)`（許可リストの 6 ページだけ: 言語と地域、キーボードの詳細設定、Microsoft IME、`intl.cpl`、タスク バー、サインイン オプション。`control.exe` は `GetSystemDirectoryW` から作った絶対パスで、作業フォルダーも System32。A9） | 済 |
| `mklm-win` `ui` | `copy_text_to_clipboard(text)`（結果画面の「詳細をコピー」。B.17）、`error_dialog(title, text)`（`MessageBoxW`。起動時のエラー、F.6） | W2 |
| `mklm-win` `instance` | `acquire_instance`、`InstanceServer`、`send_to_instance`、`InstanceCommand`、`InstanceReply`、`ServerIdentity`、`is_our_instance`、`INSTANCE_READ_TIMEOUT`、名前と SDDL（F.1） | W1（純粋な部分は済） |
| `mklm-win` `notify` | `KeyboardWatcher`、`DeviceChange` | W1 |
| `mklm-win` `pipe` | `PipeServer::accept_any(timeout)`（相手の PID を確かめない accept。多重起動のパイプ用） | W1 |
| `mklm-win` `time` | `local_time(Timestamp)`、`LocalTime::to_iso_text` | 済 |
| `mklm-win` `machine_settings` | `read_machine_settings`（済）、`write_restore_on_uninstall`（helper 専用） | E2 |
| `mklm-win` `session` | `autostart_state`（済）、`register_autostart`、`unregister_autostart`、`AUTOSTART_VALUE` | W3 |
| `mklm-ipc` | `Request::SetMachineSettings { restore_on_uninstall }`、`Request::CleanupValues { instance_id, names }`、`ApplyOptions` の確認の時間。`PROTOCOL_VERSION` を 2 に上げる | E1、E2、E3 |
| `mklm-engine` | `Engine::set_machine_settings`（ロックを取り、許可リストの名前だけを書いてフラッシュ。`RegistryBackend` に `write_machine_setting` を追加）、`Engine::cleanup_values`（F 以下の規則）、カウントダウンの秒数を `ApplyOptions` から取る（20 か 60 だけ） | E1、E2、E3 |
| `mklm-core` | `cleanup_candidates(device) -> Vec<&'static str>`（ドライバーが読まない値の名前）、`OpKind::Cleanup { instance_id, names }`。この種類のエントリだけ `schema_version = 2`、`JOURNAL_SCHEMA_VERSION = 2`。`ApplyOptions::countdown_seconds`（既定 20、許可は 20 と 60） | E1、E3 |
| `mklm-helper` | `dispatch` に 2 つの要求。セッション終了への備え（下の WP-E3） | E1、E2、E3 |

**WP-E3（M2 側への追補。A5、U10）**

- 確認の時間: `ApplyOptions` に `countdown_seconds`（serde の既定 20）を足す。engine と helper は 20 と 60 だけを受け付け、それ以外は `PlanRejected` にする。GUI の設定「確認の時間を長くする（60 秒）」で 60 を送る。CLI は 20 のまま。計画 3.5 の「15〜20 秒」からの変更（K.16）。
- セッション終了: helper は起動直後に `SetProcessShutdownParameters(0x100, SHUTDOWN_NORETRY)` を呼び、GUI（既定 0x280）より後に終了させられるようにする。表示しないトップレベル ウィンドウを持つスレッドを 1 つ作り、`WM_QUERYENDSESSION` を受けたら、動いているカウントダウンを `RevertNow` と同じ扱いで戻し（呼び出し元が先に送っていれば何もしない）、`TRUE` を返す。再接続待ちの変更は `AwaitingConfirm` のまま残す（自動で戻さない規則）。
- どちらもジャーナルの形式は変えない。`PROTOCOL_VERSION` 2 は E1、E2 と同時に上げる。

**レビューで確かめる規則の更新**（m2 K の追補）: HKLM に `KEY_SET_VALUE` を使うのは `regwrite`、`journal_store`、`machine_settings` だけ。HKCU に書くのは `session`（RunOnce と Run）だけ。`ShellExecuteW` に渡す文字列は `SettingsPage` の定数と、System32 から作った絶対パスだけ。3 つ目として、`open_recovery_folder` が `ShellExecuteExW`（`lpClass = "Folder"`）に渡す `Recovery` フォルダーのパス（known folder と定数から作り、`verify_protected_dir` で所有者と DACL を確かめ、ハンドルで固定したもの。B.11）。

**`CleanupValues`（WP-E1）の規則**: 計画 3.1 の手順 2 の「削除する」のため。対象は Keyboard クラスの devnode に限り（計画 1.5）、そのドライバーが**読まない**名前だけを消す（i8042prt の devnode にある HID の名前、kbdhid の devnode にある PS/2 の名前）。読まれない値なので、消しても入力の動作は変わらず、INV-PS2 も変わらない。反映の操作は要らず、`Written` の後は `AwaitingConfirm`（カウントダウンなし）で利用者が確定する（自動では確定しない原則）。baseline は通常どおり記録するので、「導入前に戻す」で元に戻る。キーボード以外のコレクション（マウスの Col03 など）の値は、計画 1.5 のとおり書かない。画面に手順（`reg delete` の例とデバイス マネージャーでの確認）を示すだけにする（B.1、J.9）。

### A.6 GUI の内部構造

```
 winit / Slint コールバック / ワーカー / ウォッチャー
            │ AppMsg
            ▼
   state::update(&mut AppState, AppMsg) -> Vec<Effect>     ← 純粋。テスト対象
            │ Effect
            ▼
   app::Controller::run(Effect)  ── Read / SaveSettings / RunOnceRule ──▶ I/O ワーカー
            │                    ── StartSession / SendDecision / CancelSession / AnswerRecovery ──▶ セッション ワーカー
            │                    ── ShowWindow / HideWindow / Quit
            ▼ Render（見えているときだけ）
   vm::*(&AppState の中身) -> 純粋な VM   ← 純粋。テストとスナップショットの対象
            │
            ▼
   app.rs の変換（VM → Slint の struct）→ AppWindow のプロパティ。一覧は保持した VecModel に差分で
```

| モジュール | 内容 | 純粋 |
|---|---|---|
| `args` | コマンドライン（F.2） | ○ |
| `theme` | `ThemeMode`、`ResolvedTheme`、`is_dark_rgb` | ○ |
| `i18n` | `Lang`、`LangChoice`、型ごとの文言（D.4） | ○ |
| `settings` | `Settings`（TOML）、読み書き（F.4） | ○（ファイル I/O のみ） |
| `state` | `AppState`、`Page`、`OverlayKind`、`AppMsg`、`Effect`、`update`、`SystemRead`、`ChangeDraft`、`SessionPhase`、`SessionId`、`SessionOutcome` | ○ |
| `detect` | 配列判定（B.3） | ○ |
| `vm::*` | 画面ごとのビュー モデル、`SnapshotText`、`ListOp` / `list_ops` | ○ |
| `app` | 起動、`Controller`（`dispatch`、`post`、効果の実行、描画、テーマ）、コールバックの配線 | — |
| `worker`、`reader`、`watchers`、`input_capture`、`tray`、`single_instance`、`autostart` | A.4、B.16、F 章 | — |
| `ui`（生成） | `slint::include_modules!()` | — |

`AppState` は UI スレッドの `RefCell` にだけある。`Controller::handle` は `update` の間だけ借用し、効果の実行中は借用しない（効果が次の `dispatch` を呼んでも二重借用にならない）。

**描画とフォーカス**（A7）: Slint 1.18 の Repeater は、別のモデルを渡されると全行を作り直す（`i-slint-core` の `model/repeater.rs`）。行は `FocusScope` なので、作り直すとフォーカスが消える。そこで:

- 一覧のモデル（`Rc<VecModel<KeyboardRowVm>>`）は起動時に 1 つ作り、ウィンドウに渡したまま保持する。描画のたびに、前回の行（純粋な `KeyboardRow`）と新しい行を `vm::list_ops`（行 ID で照合）で比べ、`set_row_data`、`insert`、`remove` だけを行う。変わらない行は触らない。ほかの一覧（履歴、衝突、確認の表）も WP で同じようにする。
- `AppMsg::DeviceKey` は、見えるものが変わったときだけ `Render` を返す（特定中の強調が別の行に移った、「キー入力なし」のバッジが消えた、実物の配列を知った、判定が進んだ）。同じキーボードで打ち続けても描画しない。これは `state::tests` で固定した。

### A.7 1 回の適用の流れ（Keychron を JIS に、USB）

```
UI                           I/O ワーカー                 セッション ワーカー              helper
「変更…」→ 変更ページ（ChangeDraft）
 JIS を選ぶ ──PrepareChange──▶ read_inventory, read_journal,
                               gate::blocker(NewOp), plan_set_layout
 ◀───────────ChangePrepared(plan, blocker)──
 切り替え方の既定（B.5: ほかのキーボードかポインターの使用 → すぐに）、「確認しています…」が消える
 「変更する（次に Windows の確認が出ます）」（初回だけ UAC の説明ページを挟む）
   ──StartRequest → Launching{id}, StartSession──────────────▶ run_request: launch::start ──UAC──▶ 起動
 Progress「管理者の確認を待っています…」                        Connected ──▶ Running{id}
                                                                 relay ◀── Locked, Planned, …
 ◀──────────────────────────SessionEvent{id} ×n──────────────────
 Countdown（前面に出す。打鍵テスト、20 秒）                       ◀── CountdownStarted, Tick
 「このままにする」──SendDecision(Keep)──────────────────────────▶ poll → Keep ──▶
                                                                 ◀── Result(Confirmed)
                                                                 RunOnce の規則（同じスレッド）
 ◀──────────────────────────SessionEnded{id}(report, run_once)───
 Result（今の状態は次の Read で埋まる）、Read
```

---

## B. 画面と状態

### B.0 画面の構成と遷移

- 1 つのウィンドウに、左のナビゲーション（キーボード / 履歴 / 入力方式 / 設定 / このアプリについて）と、右のページ（`Screen`）がある。モーダルな段階はページの上のオーバーレイ（`Overlay`）で、同時に 1 つだけ。
- ページ（12）: `wizard`、`main`、`change`（配列の変更: 選択肢、判定、切り替え方、UAC の 1 行）、`uac-notice`（初回だけ）、`restart`、`post-reboot`、`conflict`、`journal`、`recovery`、`ime-help`、`settings`、`about`。
- オーバーレイ（7）: `progress`、`countdown`、`reconnect`、`result`、`recovery-confirm`（helper を失った後の回復の質問）、`close-notice`、`quit-confirm`。
- 最もよくある操作（1 台の配列を変える）は 6 操作: 「変更…」→ 配列を選ぶ →「変更する」→ UAC の「はい」→ 打鍵テスト →「このままにする」（U14）。結果は閉じるだけ。
- ウィザード、変更の途中、セッション中、再起動後の確認の間は、ナビゲーションを無効にする（`navigation-enabled`）。戻るには各画面の「キャンセル」「後で」を使う。
- ページが変わったら、フォーカスはページの見出し（`PageHeader`、フォーカスできる）に移る。読み上げは新しいページの題から始まり、Tab で最初の操作に進む（U16）。
- どのページとダイアログも、題 / スクロールする本文（`DialogBody`）/ 常に見えるボタンの行（`DialogButtons`）の 3 つに分ける。大きな表示倍率や文字サイズでも、ボタンが枠の外に出ない（U11、E.4）。
- 状態遷移の表は B.18。

### B.1 初回セットアップ ウィザード（計画 3.1）

`settings.wizard.completed` が false のとき、起動時に開く（`--tray` でも開く）。「後でセットアップする」で閉じると completed を true にし、設定画面から開き直せる（最後の手順の「完了」の画面には出さない。そこで押すと、サインイン時の起動のチェックが黙って無視されるため）。手順は 4 つ（U3）。見出しは「手順 3 / 4: キーボードの配列」のように、位置を文で示す（色や太さだけで示さない。U16）。手順が変わるたびに、フォーカスを題に戻し、新しい手順の見出しから読み上げる（画面は作り直されないため。E.1）。

```
┌ MKLM のセットアップ ─────────────────────────────────────────────────┐
│ 手順 3 / 4: キーボードの配列                                          │
│ つないでいるキーボードごとに、実物の配列を選んでください。            │
│ 日本語 PS/2 キーボード (106/109 …)   JIS として動作中       [JIS ▾]   │
│ Keychron Receiver                   JIS として動作中       [US  ▾]   │
│   ⚠ 設定に問題: キーボードのドライバーが読まない値があります           │
│     [そのまま（おすすめ） ▾]                                          │
│ わからないときは、そのキーボードで Backspace の左のキーを押してください: │
│ （打鍵テスト）→ Keychron Receiver は US 配列のキーボードです ✓        │
│                         [戻る] [後でセットアップする] [次へ]          │
└───────────────────────────────────────────────────────────────────────┘
```

| 手順 | 内容 | データ |
|---|---|---|
| 1. ようこそ | MKLM がすること、設定が PC の全ユーザーに適用されること、書き込みには管理者の確認が要ること | — |
| 2. 入力方式 | 利用者の Preload とサインイン画面（`HKU\.DEFAULT`）の Preload を言語名で示す（「日本語」「英語 (US)」。KLID は詳細へ）。`mklm_core::input_warnings` の各警告を文にする。サインイン画面が日本語でなければ「intl.cpl → 管理 → 設定のコピー」の手順を示す。`ms-settings:regionlanguage`、`ms-settings:keyboard-advanced`、`intl.cpl` を開くボタン | `SystemSnapshot.input`、`vm::wizard::input_methods_page` |
| 3. キーボードの配列 | つないでいるキーボードを 1 行ずつ（メイン画面と同じまとめ方）。各行に JIS / US の選択（既定は今の動作）と、その場の配列判定（B.3。その行のキーボードのキーだけを数える）。問題のある値（`keyboard_anomalies`: HID の名前が ACPI にある、片方だけ、7/0、固定モードで無視されている）だけを、その行に説明つきで出す。選択肢は「そのまま（おすすめ）/ 削除する（ドライバーが読まない値のときだけ）」。キーボード以外のコレクションの値は「手動で削除してください」と手順だけ | `vm::wizard::keyboards_page`、`WizardKeyboard` |
| 4. まとめ | `vm::wizard::wizard_plan` の結果を文にする。すべて今と同じなら「変更は不要です」で終わる。固定モードで違うものがあれば、割り当てを含む 1 回の `Migrate`（「PC の再起動が 1 回必要です」、標準配列の選択「標準配列（おすすめ: 今の JIS）」、書き込みの順序、サインイン画面で入力できないときの手段）→ 変更の流れ（B.5）→ 再起動の画面（B.8）。キーボードごとモードでは、普通の変更を 1 台ずつ（同時に開ける操作は 1 つなので順に） | `WizardPlan`、`plan_migration` |
| 完了 | 自動起動を有効にし（既定オン、F.3）、`wizard.completed = true` | — |

- 移行は計画 1.3 のとおり「全体と違う配列を初めて割り当てるときに提案し、割り当てを同じトランザクションで書く」。固定 JIS の PC に US のキーボードを足した場合、手順 3 で US を選べば、`Migrate { standard: Jis, assignments: [US キーボード = US] }` になり、1 回の再起動で US になる（`wizard::tests` で固定）。
- 標準配列は、固定モードの移行のときだけ選ばせる。キーボードごとモードでは読み取り専用の説明にする（`Migrate` は `NotFixedMode` を返すので、選んでも意味がない）。
- 設定アプリで「接続済みキーボード レイアウトを使用する」を先に選ぶと内蔵キーボードが US になることは、移行の説明に明記する（計画 1.3）。
- 問題のない既存の値には選択を求めない（MKLM は、初めて変える前の値を baseline として記録する。m2 C.6）。「削除する」は `CleanupValues`（A.5）。WP-E1 を後に回した場合は、説明だけを出す（J.8）。
- 計画 3.1 からの変更は K.4 と K.15。

### B.2 メイン画面（計画 3.2）

```
┌──────────┬──────────────────────────────────────────────────────────────────────┐
│ キーボード│ キーボード                                                              │
│ 履歴      │ 入力方式 [日本語 IME ✓]  モード [キーボードごと]  標準配列 [JIS]         │
│ 入力方式  │ サインイン画面 [日本語 ✓]                                               │
│ 設定      │ ┌──────────────────────────────────────────────┐                       │
│ このアプリ│ │ ⚠ PC の再起動を待っている変更があります       │ [再起動…]             │
│ について  │ └──────────────────────────────────────────────┘                       │
│           │ [キーを押して特定] [最新の情報に更新] ☐非表示と未接続も表示             │
│           │ ┌────────────────────────────────────────────────────────────────────┐ │
│           │ │ Keychron Receiver                  設定した配列: US        [変更…] │ │
│           │ │ USB  3434:D027                     US として動作中 ✓ 設定どおり     │ │
│           │ │ [◀ いま押したキーボード] [レシーバー]                               │ │
│           │ ├────────────────────────────────────────────────────────────────────┤ │
│           │ │ VXE R1SE+                          設定した配列: 標準に従う（JIS）  │ │
│           │ │ Bluetooth LE  25A7:FA6C            JIS として動作中（PC の標準配列）│ │
│           │ │ [キー入力なし]                     今は変更できません: PC の再起動を │ │
│           │ │                                    待っている変更があります         │ │
│           │ └────────────────────────────────────────────────────────────────────┘ │
└──────────┴──────────────────────────────────────────────────────────────────────┘
```

**状態行**（`vm::status::status_line`）

| 項目 | 出どころ | 表示 |
|---|---|---|
| 入力方式 | UI スレッドのアクティブな HKL（A.4 の 5）。`hkl_has_japanese_layout` | 「日本語 IME ✓」（成功）/「英語 (US) ⚠ キーボードごとの配列は無効」/「その他の言語（xxxxxxxx）⚠ …」（警告） |
| モード | `Assessment.mode` | 「キーボードごと」/「固定」。固定なら下に「すべてのキーボードが JIS として動きます。ほかの配列のキーボードは、その行の［変更…］で変えられます」（U4） |
| 標準配列 | `Assessment.standard_layout` | JIS / US / その他 |
| サインイン画面 | `InputMethods.sign_in_preload` の先頭 | 「日本語 ✓」/「英語 (US) ⚠ サインイン画面では、すべてのキーボードが US 配列になります」（言語名。KLID は出さない。U5） |
| バナー | `StartupSummary`（B.12）。優先順は Recover → Conflict → AwaitingUser → WaitingForReboot → Busy → NeedsApply。読めないジャーナルは最優先 | 文とボタン（回復… / 確認… / 再起動… / 今すぐ反映…） |

状態行は同じ幅の 4 列で、項目名の下に値を出す。値は折り返すので、最小サイズ（560 × 400、E.4）でも警告の文が切れない。「非表示と未接続も表示」は、ボタンの下の行に置く（ボタンの横では最小幅に収まらない）。キーボードの行の中央の列は 260px（文字の倍率を掛ける）までで、狭いときは名前の列の次に縮む（ボタンは縮まない）。

**キーボードの行**（`vm::keyboards::keyboard_rows`）

- 行は物理デバイス（`Assessment.groups`、ContainerId ごと。内蔵コンテナと ContainerId のないものは 1 台ずつ）。行の ID は ContainerId（なければ最初のインスタンス ID）。
- 行を代表するのは、グループの最初の接続中のキーボード（なければ最初のもの）。
- 名前は `DeviceGroup.display_name`（親をたどった bus 名。「Keychron Receiver」「VXE R1SE+」）。接続方式と VID:PID。
- **3 つの状態**（用語「状態表示」）:
  - 設定した配列 = `after_restart`（保存値から予想される配列）。`KeyboardType` 由来なら「JIS」「US」、標準に従うなら「標準に従う（JIS）」、固定モードなら「固定モード（JIS）」。グループ内で違えば「混在（コレクションごとに違います）」。
  - 保存済み（反映待ち）= グループ内で最も重い `pending_action`。用語どおり「保存済み（反映待ち: キーボードのリセット / 抜き差しか再接続 / PC の再起動が必要）」と書き、次の行に利用者がすることを添える（`i18n::pending_hint`: 「抜き差しするか、［今すぐ反映…］を押してください」「PC を再起動してください（シャットダウンではなく再起動）」。U8）。ジャーナルが `apply_pending` のリセットを持つキーボードには「今すぐ反映…」ボタン（`Request::Recover` と切り替え方の選択。B.12）。
  - 現在の動作 = `current`（Raw Input が報告する種類）から。「✓」は「設定どおり」の意味だけに使う（U4）:
    - 明示的に割り当てた配列（`LayoutBasis::KeyboardType`）が効いている:「JIS として動作中 ✓ 設定どおり」（成功）。
    - 標準に従う / 固定モードで、予想どおり:「JIS として動作中（PC の標準配列）」「…（固定モード）」（中立。色も ✓ も付けない）。
    - 予想と違う:「US として動作中 ⚠」（警告）。未接続なら「未接続」。
  - 実物の配列が分かっていて、動作と違う:「⚠ 実物は US 配列ですが JIS として動いています」（警告）と、その行の「変更…」。実物の配列は、配列判定の結果（JIS / US）と、JIS にしかないキー（0x70、0x79、0x7B、0x7D、0x73）を押したこと（JIS だけ）から学び、`settings.keyboards.physical` にインスタンス ID ごとに残す（キーの内容は残さない）。
- **バッジ**（U17）: キー入力なし（このデバイスのキー入力をまだ見ていない。内蔵キーボードには付けない。「マウスなどの付属機能のことがあります。［キーを押して特定］で確かめられます」）、◀ いま押したキーボード（特定中。B.3）、レシーバー（名前に Receiver / Dongle / Unifying / Bolt / レシーバー を含むか、既知の VID:PID。「このレシーバーにつないだすべてのキーボードに同じ配列が適用されます」）、内蔵、読み取り専用（`Transport::Virtual` か、kbdhid / i8042prt 以外のドライバー）、リモート デスクトップ（`KeyboardDevice::is_remote_desktop`: `TERMINPUT_BUS\…` か、ハードウェア ID `TS_INPT\TS_KBD`。読み取り専用の代わりに付け、内蔵とキー入力なしは付けない。「接続元の PC から届くキー入力です。キーの割り当てはセッションが始まったとき（サインインしたとき）に決まり、MKLM では変更できません」。接続方式は「リモート デスクトップ」、現在の動作は「接続元の PC からの入力」で、配列の名前は出さない。リモート セッションで接続元が種類を報告していれば、行の注記に「接続元の報告: 日本語キーボード (JIS)。このセッションのキーの割り当てと同じとは限りません」。docs/research/rdp-keyboard.md）、未接続、非表示、設定に問題（`anomalies` がある）。「打鍵での確認がまだ」は変更ページの「標準に従う」の説明にだけ書き、バッジにはしない。
- **非表示**: 行のメニュー（右クリックか Shift+F10、WP-U1）で「非表示にする / 表示する」。`settings.keyboards.hidden` に行の ID を入れる。既定では、非表示の行と、接続中のメンバーがない行を出さない。「非表示と未接続も表示」で出す。
- マウスを判定に使わない（Keychron のレシーバーにもマウスのコレクションがある）。キー入力を見ていないデバイス（マウスのキーボード用コレクション）は「キー入力なし」のまま残り、利用者が非表示にできる。
- 「変更…」は、読み取り専用の行と、ジャーナルが書き込みを止めている間（`StartupSummary::blocks_writes`）は無効。止めている間は行に理由（`i18n::cannot_change_now`:「今は変更できません: PC の再起動を待っている変更があります」など）を書き、行の読み上げにも含める（U16）。ボタンの読み上げ名は「Keychron Receiver の配列を変更」。
- キーボードが到着・取り外されたら（WP-W1 の通知）、750 ms 待ってまとめて `Read` する。

### B.3 キーを押して特定と、配列判定（計画 3.3）

**キーを押して特定**（U12）: ボタン（またはトレイのメニュー）で開始すると、キーの受け取り欄（`CaptureArea`）が現れてフォーカスを取る。欄は Tab と Esc 以外のすべてのキーを受け取るので、探しているキーボードで Space や Enter を押しても、ボタンや行が反応しない。Esc で終わる。以後の `DeviceEvent::Key`（winit。ウィンドウが前面のときだけ届く）のインスタンス ID を含む行に「◀ いま押したキーボード」のバッジを付け、枠で囲み、スクロールして見せ、「Keychron Receiver のキーが押されました」を polite で読み上げる。ハイコントラストでは行を塗らず、3px の Highlight の枠とバッジで示す（塗ると WindowText との対比が 2:1 を下回るテーマがある）。キーの内容は保持しない（インスタンス ID だけ）。

**配列判定**（`detect::Detection`）は、変更ページ（B.4）とウィザードの手順 3 の中で、そのキーボードについて行う（独立した画面はない。U14）。

| 質問 | JIS | US |
|---|---|---|
| Backspace の左のキー | 0x7D（¥） | 0x0D（=） |
| 右 Shift の左のキー | 0x73（ろ） | 0x35（/） |

- スキャン コードは winit の `PhysicalKeyExtScancode::to_scancode()`（Raw Input のセット 1 のメイク コード）。物理的な位置なので、保存値や IME に左右されない。
- 変更ページでは、対象の行のキーボード（同じ物理デバイスのすべてのコレクション、`Detection::for_keyboards`）のキーだけを数える。ほかのキーボードのキーは数えず、「内蔵キーボードのキーです。Keychron Receiver で押してください」と出す（`Press::OtherKeyboard`）。
- 対象を決めない判定（`Detection::new`）では、答えになるキー（0x7D、0x0D、0x73、0x35）を最初に押したキーボードに固定する。Tab や文字のキーでは固定しない。
- 0x70（かな）、0x79（変換）、0x7B（無変換）、0x7D、0x73 のどれかが押されたら「JIS の可能性が高い」と添える。
- 2 つの答えが食い違ったら「判定できませんでした」と「やり直す」（`Detection::restart`）。
- 結果は、その選択肢を 1 クリックで選べるようにし、`settings.keyboards.physical` に残す（B.2）。
- キーの受け取りは IME を通さない `KeyTestArea`（`FocusScope`。フォーカスがある間は IME が無効になる。M0 #8 B）。

### B.4 配列の変更ページ（計画 3.4、3.5 の 1 と 2。U14）

割り当て、適用内容の確認、UAC の説明を 1 ページにまとめた（`Page::Change`、`vm::change`）。

```
┌ Keychron Receiver の配列 ────────────────────────────────────────┐
│ ◉ JIS                                                              │
│ ○ US（現在）                                                       │
│ ○ 標準に従う（今は JIS）  値を消して PC の標準配列に従わせます。     │
│                           打鍵での確認がまだです                    │
│ わからないときは、このキーボードで Backspace の左のキーを押して     │
│ ください（打鍵テスト）                                              │
│ 切り替え方                                                          │
│ ◉ すぐに切り替えて 20 秒間試す                                      │
│   このキーボードは数秒間使えません。その間は、ほかのキーボードか     │
│   マウスで操作します。                                              │
│ ○ PC の再起動で切り替える                                          │
│   再起動するまで、ほかのキーボードの配列も変更できません。           │
│ ⓘ この設定は、この PC のすべてのユーザーに適用されます。           │
│ ☐ 技術的な詳細を表示する                                            │
│ 次に Windows の確認画面が出ます（発行元は「不明」）。               │
│ mklm-helper.exe であることを確かめて「はい」を押してください。       │
│ [MKLM 導入前に戻す…]      [キャンセル] [変更する（次に Windows の確認が出ます）] │
└──────────────────────────────────────────────────────────────────┘
```

- 選択肢は JIS / US / 標準に従う（`RadioList`: ◉ / ○、選んだものは塗り、フォーカスは外側の枠。上下の矢印キーで選択が動く。U16）。「標準に従う」は kbdhid だけに出す（i8042prt には出さない。`StandardNotAllowed`）。今と同じ配列には「（現在）」を付ける。
- 適用範囲は「このデバイスのみ」だけを示す（「同じ機種ならどのポートでも」は M4。m2 0.1）。同じ物理デバイスのすべての kbdhid コレクションに書くことは `physical_device_members` が決める。
- US を選ぶと IME の切り替えの案内を出す（計画 3.4）。変更を「このままにする」で終えた後の結果画面にも出す（B.13、B.17。U19）。
- 固定モードの PC では、`plan_set_layout` が `MigrationRequired` を返す。そのときは同じページで「この PC は固定モードです。キーボードごとモードへ移行し、この割り当ても同時に書きます（PC の再起動が 1 回必要）」と説明し、標準配列の選択（「標準配列（おすすめ: 今の JIS）」）を出して、`MigrateRequest { standard, assignments: [この割り当て] }` を送る（B.1 の手順 4 と同じ内容）。
- 「MKLM 導入前に戻す…」は、この行の `RestoreScope::Device` で同じページの形の下見（`vm::change::restore_page`、`preview_restore`）へ。全体の「導入前に戻す」は設定画面から。

### B.5 準備、切り替え方、UAC の説明（計画 3.5、1.4）

**準備**（I/O ワーカーの `PrepareChange`、WP-U3）: 選択肢を選ぶと始まり、終わるまで「確認しています…」を出す。`read_inventory`（書き込みを止める読み取りの問題があれば止める）→ `read_journal` → `gate::blocker(Gate::NewOp)`（止められていれば、その `BlockReason` を文にして結果画面を出し、UAC は出さない）→ `plan_set_layout`（選んでいる切り替え方の `ApplyOptions` で）→ `nothing_to_change` なら「変更はありません」。

**切り替え方**（`ApplyMethod`。U1。m2 C9 の申告をこの 2 択にした）: 計画がその場でリセットしうるとき（`other_input_available = true` の計画でリセットになるとき）だけ出す。

| 選択肢 | 送る `ApplyOptions` | 文（`i18n::apply_method`） |
|---|---|---|
| すぐに切り替えて 20 秒間試す | `allow_live_reset`、`other_input_available` とも true | 「このキーボードは数秒間使えません。その間は、ほかのキーボードかマウスで操作します。」 |
| PC の再起動で切り替える | どちらも false | 「再起動するまで、ほかのキーボードの配列も変更できません。」 |

- 既定値（`vm::change::default_apply_method`、10 分以内の入力。計画 1.4）:
  - 対象以外のキーボード（別の物理デバイス）のキー入力か、ポインター（マウスのボタン、移動、ホイール）の使用を見た →「すぐに」。マウスの利用者はスクリーン キーボードで続けられる。ポインターは `input_capture` が 5 秒に 1 回まで `AppMsg::PointerUsed` で知らせる。
  - 対象のキーボードのキー入力だけを見た →「再起動」と、計画 1.4 の警告「このキーボードは、最近入力のあった唯一のキーボードです。PC の再起動で反映することをおすすめします」。
  - 何も見ていない →「再起動」。警告は出さない（根拠のない警告を出さない）。
- 切り替え方を変えたら、その `ApplyOptions` で計画を作り直して表示を更新する（表示と `ExpectedPlan` と実際の書き込みを一致させる。m2 S6）。
- 反映方法の文は `i18n::takes_effect(plan.apply, 秒数)`。キーボードのリセットには「（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）」を、PC の再起動には「再起動するまで、MKLM でほかの変更はできません」を含める（U15 e、U1）。
- 同じ 2 択は、回復、「確認待ちの変更をすべて元に戻す」、履歴の「元に戻す」の下見でも使う（どれも `ApplyOptions` を持つ要求）。
- 「標準に従う」には「打鍵での確認がまだです。後で Shift+2 で確かめてください」を添える（m2 D.2）。
- INV-PS2 が保たれない計画は「変更する」を無効にし、理由を出す（通常は `check_plan` が先に拒否する）。
- 送る要求は `SetLayoutRequest { instance_id, layout, apply, expected: Some(preview::expected(&plan)) }`。移行なら `MigrateRequest`、「導入前に戻す」なら `RestoreBaselineRequest`（衝突は B.10 と同じ見せ方で決め、`ConflictPolicy` にする）。

**UAC の説明**（R12 の教訓: 説明を読む前に UAC が出ないように）

- 初回（`settings.change.uac_notice_seen` が false）は、「変更する」の後に独立した画面（`UacNoticeScreen`）を出す。「確認画面へ進む」で `settings.change.uac_notice_seen = true` にして始める。
- 2 回目からは、変更ページのボタンの上の 1〜2 行と、ボタンの文「変更する（次に Windows の確認が出ます）」で説明する。毎回同じ画面を通らせると、読まずに進む癖がつくため（U14）。
- 昇格したまま GUI を起動した場合（非推奨。F.2）は UAC が出ないので、どちらも出さず、ボタンは「変更する」。

初回の画面の文（Windows 11 の未署名のプログラムの UAC に合わせた。U18。T-APPLY-1 で実際の画面と照らす）:

```
┌ 次に、Windows が管理者の許可を求めます ─────────────────────────┐
│ 次の画面に「不明な発行元からのこのアプリがデバイスに変更を加える │
│ ことを許可しますか?」と表示されます。MKLM はまだコード署名をして │
│ いないため、確認済みの発行元が「不明」になります。画面の中ほどに │
│ 「mklm-helper.exe」と表示されていることを確かめてから「はい」を   │
│ 押してください。                                                 │
│ ┌ Windows の確認画面の例 ─────────────────────────────┐          │
│ │ ユーザー アカウント制御                               │          │
│ │ 不明な発行元からのこのアプリがデバイスに変更を加える… │          │
│ │ mklm-helper.exe                                       │          │
│ │ 確認済みの発行元: 不明                                │          │
│ │ ファイルの入手先: このコンピューター上のハード ドライブ│          │
│ │ ［はい］［いいえ］                                    │          │
│ └───────────────────────────────────────────────────┘          │
│ 管理者でないアカウントでは、「はい」の代わりに管理者のパスワードか │
│ PIN の入力を求められます。「いいえ」を選んだ場合は、何も変更しません。│
│                               [キャンセル] [確認画面へ進む]      │
└─────────────────────────────────────────────────────────────────┘
```

- 「確認画面へ進む」（2 回目からは「変更する」）で `AppMsg::StartRequest` → `Effect::StartSession` → Progress「管理者の確認を待っています…」。UAC の親はメイン ウィンドウ。

### B.6 カウントダウン（計画 3.5 の 3、m2 D.2 a）

```
┌ 新しい配列を試してください ─────────────────────────────────┐
│ Keychron Receiver を JIS に切り替えました。あと 20 秒で自動的に │
│ 元に戻ります。そのキーボードで Shift+2 を押して確かめてから    │
│ （" なら JIS、@ なら US）、Tab で［このままにする］へ移って押して│
│ ください。                                                     │
│ ┌ Windows の認識: Keychron Receiver は JIS 配列です ✓ ───────┐ │
│ ┌─────────────────────────────────────────────────────────┐   │
│ │                        "                                │   │
│ │      Shift+2 → " : ✓ 期待どおり JIS です                 │   │
│ │      このキーを送ったキーボード: Keychron Receiver       │   │
│ └─────────────────────────────────────────────────────────┘   │
│ ████████████░░░░░░░   あと 12 秒で自動的に元に戻します         │
│                                 [元に戻す] [このままにする]    │
└─────────────────────────────────────────────────────────────┘
```

- `CountdownStarted` で開く（`vm::session::countdown`）。残り秒は helper の `CountdownTick`。GUI 側で時間を数えない（決めるのは helper）。秒数は 20、設定「確認の時間を長くする（60 秒）」なら 60（WP-E3。U10）。
- 開くときにウィンドウを表示して前面に出し、前面に出られなければタスク バーを点滅させる（`request_user_attention(Critical)`）。UAC の間にほかのアプリへ移っていても、打鍵テストと Raw Input が働くように（U10）。
- 開いたら打鍵テストの欄にフォーカスを移す。欄の読み上げの説明は上の文全体（時間の制限と、残し方）で、同じ文を開いたときに 1 回だけ assertive で読み上げる。残り 10 秒と 5 秒のときだけ「あと 10 秒で元に戻ります」を polite で読み上げる（毎秒は読まない）。
- Enter や Space は打鍵テストの入力として扱い、確定には使わない（打鍵のつもりの Enter で確定しないため）。「このままにする」は Tab で移ってから押す（フォーカスの順では、打鍵テストの欄の次が「このままにする」。見た目の並び［元に戻す］［このままにする］は `layout-order` で保つ）。Esc は欄が受け取らず、ダイアログ（`DialogFrame::escape`）で「元に戻す」になる。
- 「Windows の認識」は、到着したキーボードの報告を配列名で示す。確かめられないときは「Windows の認識をまだ確かめられません。打鍵テストで確かめてください」（「Raw Input」や 0x7/0x2 は詳細へ。U5）。
- 打鍵テストの判定（`vm::keytest::key_pressed`。U2）は、期待する配列（この操作の `layout_after`）、対象のキーボード、アクティブな入力方式と照らす:
  - 期待どおり:「✓ 期待どおり JIS です」（成功）。
  - 違う:「⚠ JIS になるはずが US です。［元に戻す］をおすすめします。」（危険）。
  - 対象以外のキーボードから:「このキーは 内蔵キーボード から送られました。Keychron Receiver で押してください」（情報。判定しない）。
  - 入力方式が日本語でない（英語 (US) なら、どのキーボードも `@`）:「入力方式が日本語ではないため判定できません。Win+Space で日本語に切り替えてください。」（警告）。
  - 期待がない場面（入力方式の案内など）: 中立の文。
- 「このままにする」→ `Decision::Keep`、「元に戻す」→ `Decision::RevertNow`。押したらボタンを無効にし、「このままにします…」などを出して結果を待つ。
- ウィンドウの × は「元に戻す」と同じ（`state::update` の `WindowCloseRequested`）。終了の要求は F.5。
- 時間切れで元に戻った場合は、結果（B.17）に「時間内に［このままにする］が選ばれなかったため、元に戻しました」。

### B.7 再接続待ち（m2 D.2 b。BLE / BT、未接続のキーボード）

```
┌ キーボードを接続し直してください ───────────────────────────┐
│ Microsoft Bluetooth Keyboard を抜いて差し直してください       │
│ （Bluetooth は電源をオフにしてからオンにします）。その後      │
│ Shift+2 で確かめてください。                                  │
│ ┌ ⓘ 接続し直すのを待っています…（2 分 10 秒）─────────────┐ │
│ （打鍵テスト）                                                │
│ 決めるまで、ほかのキーボードの配列も変更できません。          │
│              [後で決める] [元に戻す] [このままにする]         │
└─────────────────────────────────────────────────────────────┘
```

- `WaitingForReconnect` で開き（前面に出す）、`KeyboardArrived` で「接続し直したキーボードが新しい種類を報告しています ✓」にする。
- 最大 180 秒の待ちと 600 秒の判断待ちは helper が持つ。「後で決める」は取り消し（`Abandoned { planned: true }`）で、変更は確認待ちのまま残り、メイン画面のバナーから決められる（B.12）。自動では戻さない。
- 報告値が変わる前の「このままにする」も受け付ける（m2 C.11: `Confirmed` と `apply_pending = Reconnect`）。

### B.8 PC の再起動画面（計画 3.6）

```
┌ PC を再起動して完了してください ─────────────────────────────┐
│ 「シャットダウン」ではなく「再起動」を選んでください。高速スタ │
│ ートアップが有効だと、シャットダウンではドライバーが読み直され │
│ ません。                                                       │
│ ・キーボードごとモードへ移行（標準配列 JIS）: PC の再起動待ち  │
│ ┌⚠ サインイン画面で入力できないときは、スクリーン キーボード──┐ │
│ │ （右下のアクセシビリティ）、PIN や指紋、別のキーボード。     │ │
│ ┌⚠ パスワードに記号がある場合、配列が変わると同じキーで別の ──┐ │
│ │ 記号が入力されます。PIN（数字）でサインインするか、再起動の │ │
│ │ 前に PIN を設定してください。 [サインイン オプションを開く]  │ │
│ ☐ キーボードの配列が変わってもサインインする方法を確認しました │
│ 「今すぐ再起動」を押す前に、開いているファイルを保存してください。│
│ 再起動するまで、ほかのキーボードの配列も変更できません。       │
│ [変更を元に戻す…]                    [後で] [今すぐ再起動]    │
└───────────────────────────────────────────────────────────────┘
```

- 理由は `gate::restart_reasons`（この起動の `PendingReboot` / `RevertedPendingReboot`、`apply_pending = RestartPc`）。
- パスワードの警告は、再起動でどれかのキーボードの動作が変わるとき（`vm::restart::Restart::layout_changes`）に出す（U15 d）。固定 JIS の US キーボードで覚えたパスワードの記号は、移行と US の割り当ての後に別のキーになる。「サインイン オプションを開く」は `ms-settings:signinoptions`（`SettingsPage::SignInOptions`）。
- 「今すぐ再起動」は、確認のチェックがオンで、かつ最新の読み取りでジャーナルに理由がある（`PendingReboot` がフラッシュ済み。計画 2.3）ときだけ押せる。押すと I/O ワーカーで RunOnce の規則を適用してから `session::restart_pc()`（`InitiateShutdownW(SHUTDOWN_RESTART | SHUTDOWN_RESTARTAPPS, …)`）。失敗したらエラーを出す。
- 「後で」はメイン画面へ。バナーが残る。「後で」の前に、それまでほかの変更ができないことを書いておく（U15 b）。
- 「変更を元に戻す…」は `Undo`（B.12）。
- `PendingReboot` になった時点で、RunOnce（`"<dir>\mklm.exe" --post-reboot`）はセッションの直後の規則で登録済み（A.2.3、F.2）。

### B.9 再起動後の確認（m2 D.7、計画 3.6）

`gate::post_reboot_entries` があり、`StartupSummary::post_reboot_due`（起動 ID が変わった `PendingReboot`、または再起動で反映する `AwaitingConfirm`）なら、起動時に最前面で開く。`--post-reboot` で起動した場合も、既存のインスタンスが `activate` を受けた場合も、判断は次の `SystemRead` のジャーナルで行う（`activate` はページを変えない。F.1）。

```
┌ 再起動後のキーボードを確認してください ──────────────────────┐
│ キーボードごとモードへ移行（標準配列 JIS）                     │
│ キーボード                        設定   Windows の認識   打鍵  │
│ 日本語 PS/2 キーボード (106/109…)  JIS    JIS 配列 ✓       ✓    │
│ Keychron Receiver                  US     US 配列 ✓        未    │
│ 移行が反映されたかは、Windows の認識では分かりません（内蔵キー │
│ ボードはどちらでも JIS と報告します）。打鍵テストで確かめてくだ │
│ さい。                                                         │
│ （打鍵テスト）                                                  │
│ ☐ 技術的な詳細を表示する                                        │
│ 決めるまで、ほかのキーボードの配列も変更できません。            │
│    [後で決める] [元に戻す（もう一度 PC の再起動が必要）] [このままにする] │
└───────────────────────────────────────────────────────────────┘
```

- 行は `preview::check_rows`。列は「キーボード / 設定 / Windows の認識 / 打鍵」で、値は JIS / US / 標準。「✓」は一致、「未接続」は報告なし、「違います」は不一致（警告）。0x7/0x2 などの数値は「技術的な詳細」へ（U5）。
- 「打鍵」は、その行のキーボードで Shift+2 を押した結果（`post_reboot::Typed`: 未 / ✓ / ⚠）。打鍵テストの判定は B.6 と同じく、その行の期待する配列と照らす。変更したキーボードが ⚠ なら、「このままにする」は押せるままにして、警告「⚠ Keychron Receiver が期待と違う配列で動いています。『元に戻す』をおすすめします」を出す（U2）。
- 同じ起動のまま（`PendingReboot` で起動 ID が同じ）なら、「まだ反映されていません。『シャットダウン』ではなく『再起動』してください」を出し、「このままにする」を無効にする（「元に戻す」はできる）。RunOnce を登録し直す。
- 「このままにする」→ `Request::Confirm`（エンジンが `RebootObserved` の遷移と INV-PS2 の確認を行ってから確定する。m2 D.6）。「元に戻す」→ `Request::Revert`。戻しの下見が再起動で反映するとき（移行、内蔵キーボード）は、ボタンを「元に戻す（もう一度 PC の再起動が必要）」にする（U15 c）。「後で決める」→ 何もしない。RunOnce を登録し直し、バナーに残る。
- 自動では戻さない（計画 3.6）。
- 最前面に出す方法: ウィンドウを表示して `focus_window`、前面に出られなければ `request_user_attention(Critical)`、利用者が操作するか 60 秒たつまで `WindowLevel::AlwaysOnTop`（J.6）。

### B.10 衝突の解決（m2 D.8。U6）

```
┌ MKLM 以外がキーボードの設定を変更しました ──────────────────────┐
│ Keychron Receiver を JIS に                                       │
│ Keychron Receiver                                                 │
│ 今の値: 不明な種類（8/2）— MKLM 以外が変更                       │
│ [変更前の US に戻す（おすすめ） ▾]                                │
│ ☐ 詳細を表示する                                                  │
│ [確認待ちの変更をすべて元に戻す…]  [キャンセル] [選んだとおりにする] │
└─────────────────────────────────────────────────────────────────┘
```

- 1 行は 1 台のキーボード（同じ対象の記録の組: デバイスの Type と Subtype、または PC 全体の値）。`vm::conflict::conflict_keyboards`。
- 値は配列名で示す（「JIS」「US」「不明な種類（8/2）」）。選択肢は配列名で、重複を除く:「変更前の US に戻す」（`UseBefore`）、「MKLM が設定した JIS にする」（`UseIntended`）、「今の値のまま」（`KeepCurrent`）、「MKLM 導入前の US に戻す」（`UseBaseline`。「変更前」と同じなら出さない）。
- おすすめ（`vm::conflict::recommend`）: 今の組が MKLM の書く組（4/0、7/2）の外なら「変更前に戻す」、中（例: 設定アプリで変えた）なら「今の値のまま」。選択の既定はおすすめ。
- 組の 2 つの値は同じ選択になる（4/2 や 8/0 のような組を作れない）。値ごとの数値（今の値、MKLM が最後に書いた値、操作の前の値、操作で書こうとした値、MKLM 導入前の値）と、値ごとの選択の上書きは「詳細」に畳む（`ConflictValue::choice`）。`vm::conflict::choices` が要求の `ValueChoice` にする（テスト済み）。
- 用語はどこでも「操作の前の値 / 操作で書こうとした値 / MKLM 導入前の値 / 今の値」にそろえる。
- 書き込みを繰り返し拒否された値（`write_error`）は、そのエラーを行に出す。
- INV-PS2 を壊す選択はエンジンが `PlanRejected` で拒否するので、固定値のない PS/2 の一覧を出し、「全体の値を『操作の前の値』にする（固定モードに戻す）か、すべて元に戻す」を案内する（m2 D.8）。
- 「導入前に戻す」の `ConflictPolicy::Report` の結果（`op_id` なし）も同じ見せ方で、「そのままにする（Skip）/ 導入前の値にする（Overwrite）」を選ばせて送り直す。

### B.11 履歴（ジャーナル）

```
┌ 履歴 ──────────────────────────────────────────────────────────┐
│ 2026/09/27 22:36  Keychron Receiver を JIS                       │
│ 元に戻しました — ［このままにする］が選ばれる前に MKLM が終了したため、元に戻しました │
│ 1ef48b2f                                                          │
│ 2026/09/27 22:25  Keychron Receiver を JIS                [元に戻す…] │
│ このままにしました                                                │
│ 371b1633                                                          │
│ [復旧用ファイルのフォルダーを開く]                                │
└───────────────────────────────────────────────────────────────┘
```

- 新しい順（`seq` の降順）。時刻はローカル時刻（G.1）。何の操作か（`vm::journal::kind_text`。インスタンス ID は機器名に置き換える）、状態、理由（G.2 の文言）。
- 状態の文（`i18n::entry_state`）: `Planned` / `Written` は、書き手が動いていれば「処理中」、そうでなければ「途中で止まりました（回復が必要）」（「中断?」のような疑問の形は使わない）。`Failed` で理由が `ConflictKeptCurrent` なら「MKLM 以外の値を残しました」（「変更は残っていません」とは書かない。U5）。
- 「元に戻す…」は、その値を最後に変えた操作だけに出す（`NotLatest` にならないもの。m2 C.8）。`Confirmed` の「導入前に戻す」には出さない（m2 C6）。読み上げ名は「2026/09/27 22:25 の変更を元に戻す」。押すと下見（WP-U5。切り替え方の 2 択を含む）→ `Request::Revert`。
- 「復旧用ファイルのフォルダーを開く」は `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery`（Users が読める。m2 G.1）をエクスプローラーで開く。フォルダーの外のパスは開かない。開く前に、各階層を読み取りだけで検証する（`protected_dir::verify_protected_dir`: リパース ポイントでない、所有者が Administrators か SYSTEM、保護された DACL、SY と BA 以外に書き込み系の権利がない）。通らなければ開かない（helper が初めて動く前に別の利用者が作ったフォルダーを、MKLM の復旧用ファイルとして見せないため。m2 の squatting の対策と同じ）。検証の間に開いたハンドルは `ShellExecuteExW` が戻るまで保ち、`SEE_MASK_CLASSNAME` と `lpClass = "Folder"` でフォルダーとしてだけ開く。
- 読めないエントリ（新しい版）があれば、先頭に「MKLM を更新してください」。

### B.12 起動時の確認（attention）と回復

起動時と、`Read` のたびに `startup::summarize` を求める。

「確認待ちの変更」は、このままにするか元に戻すかをまだ決めていない変更（`AwaitingConfirm`、`PendingReboot`、`Conflict`）をまとめて指す言葉として、バナー、ボタン、トレイのメニューでそろえる（`Request::Undo` の対象と同じ。m2 D.10）。

| attention | 表示 | 操作 |
|---|---|---|
| `Recover`（中断、所有者のいないカウントダウン） | 起動時に 1 回、回復の画面を出す（同じエントリについて 1 回の起動につき 1 回まで。`settings.recovery.prompted` に op と boot を記録）。以後はバナー | 「回復する（おすすめ）」→ `Request::Recover`（UAC の説明は B.5 と同じ）。「後で」 |
| `Recover`（起動 ID の変わった `PendingReboot`） | 再起動後の確認（B.9） | B.9 |
| `AwaitingUser` | バナー「確認待ちの変更があります」 | 「確認…」→ 回復の画面で「このままにする / 元に戻す / 確認待ちの変更をすべて元に戻す」 |
| `WaitingForReboot` | バナー「PC の再起動を待っている変更があります」 | 「再起動…」→ B.8 |
| `Conflict` | バナー | 「確認…」→ B.10 |
| `Busy` | バナー「別の MKLM が処理中です」 | 変更のボタンを無効にする |
| `NeedsApply` | 行の「保存済み（反映待ち）」と対処の文、バナー（情報）。Raw Input がすでに期待どおりなら出さない（`apply_pending_cleared`） | 「今すぐ反映…」→ 切り替え方の 2 択（B.5）→ `Request::Recover { apply }`。対話的な回復は、`apply` が許せば `apply_pending = ResetKeyboard` のキーボードを 1 台ずつリセットして `apply_pending` を消す（m2 D.7 の 4。U8）。BLE / BT は抜き差しの案内、内蔵は PC の再起動 |

```
┌ 確認が必要なことがあります ─────────────────────────────────┐
│ Keychron Receiver を JIS に（途中で止まりました）              │
│ ┌ → 回復すると US（変更前）に戻ります。キーボードの動作は変わって │
│ │   いません                                                    │
│ 回復するまで、キーボードの配列は変更できません。               │
│                                 [後で] [回復する（おすすめ）]  │
└─────────────────────────────────────────────────────────────┘
```

- 項目ごとに、回復で何が起きるかを文で示す（`vm::recovery`。U9）。`mklm_core::decide_recovery` は純粋な関数なので、GUI が非昇格で、表示用の列挙から読んだ今の値で予測できる（実際の判断は helper がロックの下でもう一度行う）:
  - `RollBack`:「→ 回復すると US（変更前）に戻ります」と、リセットに届いていなければ「キーボードの動作は変わっていません」（`describe::reset_phase`）。
  - `RollForward` / `CompleteForward`:「→ 書き込みは終わっています。回復後に、このままにするか元に戻すかを選べます」（再起動で反映するなら、その旨）。
  - `Conflict`:「→ MKLM 以外の変更が見つかりました。回復の後で、どうするかを選びます」。
  - `MarkNothingWritten`:「→ 何も書き込まれていませんでした。記録を閉じるだけです」。
- 主ボタンは「回復する（おすすめ）」の 1 つだけ。「確認待ちの変更をすべて元に戻す…」は `AwaitingUser` のエントリがあるときだけ出す。「後で」には「回復するまで、キーボードの配列は変更できません」を添える。
- **GUI は、利用者がボタンを押したときにだけ UAC を出す**（m2 D.7 の「自動で UAC を出すのは 1 回まで」より厳しくした。K.6）。例外は helper を失った直後の回復の質問（A.2.3）で、これもオーバーレイで理由を示し、利用者が「今すぐ元に戻す」を押したときだけ起動する。
- 回復の結果は `Outcome::Recovered` の `recovered` を行ごとに出す（「途中で止まりました → 元に戻しました（リセットの前に止まっていたため、キーボードの動作は変わっていません）」。G.2）。何も戻さず、利用者を待つエントリが残る場合は「確認待ちの変更があります。［確認待ちの変更をすべて元に戻す…］で戻せます」（m2 C7）。結果の部品（結果、回復した操作ごとの行、反映待ちとその次の手順）は 1 行に 1 つずつ出す。「今すぐ反映…」の結果は、回復した操作がなければ「反映しました。」（「回復しました。」ではない）。
- 「確認待ちの変更をすべて元に戻す」は `Request::Undo`（m2 D.10）。下見は `preview::undo_preview`。

### B.13 入力方式の案内（計画 3.10）

案内と、許可リストの設定ページを開くボタンだけ（`SettingsPage::{RegionLanguage, KeyboardAdvanced, JapaneseIme, IntlControlPanel}`）。MSIME の非公開のレジストリ値には書かない。

- キーボードごとの配列は日本語 IME の間だけ有効。英語を打つときは英語 (US) に切り替えず IME をオフにする（Alt+` か「半角/全角」）。
- US 配列のキーボードには「半角/全角」がない。日本語入力のオン/オフは Alt+`。Ctrl+Space を使うには: Microsoft IME の設定 → キーとタッチのカスタマイズ →「各キー/キーの組み合わせに好みの機能を割り当てます」をオン → Ctrl + Space: IME-オン/オフ（U19）。
- US の割り当てを「このままにする」で終えた結果画面にも、「US 配列には『半角/全角』キーがありません。日本語入力のオン/オフは Alt+` です［入力方式の案内］」を出す（計画 3.4「US を割り当てた直後」）。
- サインイン画面の入力方式を合わせる手順（intl.cpl → 管理 → 設定のコピー）。

### B.14 設定

| 項目 | 保存先 | 備考 |
|---|---|---|
| テーマ（ライト / ダーク / システム） | `settings.toml` | その場で適用（C.1） |
| 言語（日本語 / English / システム） | `settings.toml` | その場で切り替え（D.3） |
| サインイン時に MKLM をタスク バー（「^」の中）で起動する | HKCU の Run（F.3） | 値そのものが状態。タスク マネージャーで無効にされていれば、その旨を添えてチェックを無効にする |
| 確認の時間を長くする（60 秒） | `settings.toml`（`change.countdown_seconds`） | 既定 20 秒。helper は 20 と 60 だけを受け付ける（WP-E3。U10） |
| アンインストール時にキーボードの設定を元に戻す（既定オン） | HKLM（`machine_settings`。helper 経由） | 「変更…」→ 説明 → UAC の説明 → `Request::SetMachineSettings`。非昇格で読んで表示する（計画 3.13） |
| 非表示と未接続のキーボードも表示する | `settings.toml` | メイン画面と同じ |
| 初回セットアップをもう一度行う | — | B.1 |
| すべてのキーボードを MKLM 導入前に戻す… | — | `RestoreScope::All` で B.4 の下見 |

設定の各コンボボックスには読み上げ名（「テーマ」「言語」）を付ける。「アプリ内のアンインストール」（計画 3.13）は M5（インストーラー）で扱う。

### B.15 このアプリについて（計画 3.12）

- 製品名、版とビルド ID、「© 2026 SHIN DATA CENTER」、「Apache License 2.0 で提供しています」。
- 第三者のライセンス一覧: M5 の cargo-about が生成する `THIRD-PARTY-LICENSES` を埋め込んで表示する（WP-U7。それまでは主要なもの: Slint（Royalty-free License 2.0）、windows-rs、serde、toml）。
- `AboutSlint`（Slint の Royalty-free ライセンスの表記義務。README のバッジと合わせて）。`Palette.color-scheme` を常に明示するので、表示は崩れない（計画 3.7）。

### B.16 トレイと、初めてトレイに入るときの通知（計画 3.9。U13）

- アイコンは Slint の `SystemTrayIcon`（M0 #8 E で合格）。ツールチップは「MKLM」、注意があれば「MKLM — PC の再起動を待っている変更があります」。
- 左クリック → ウィンドウを表示して前面に。右クリックのメニュー: 「MKLM を表示」「キーを押してキーボードを特定」「確認待ちの変更をすべて元に戻す…」（開いている変更があるときだけ有効）「終了」。
- explorer の再起動（`TaskbarCreated`）で作り直す（試作の回避策）。
- **ウィンドウの ×**: トレイに隠す（`CloseRequestResponse::HideWindow`）。トレイを作れなかった場合は終了する。セッション中は F.5。
- **初回の通知**（`settings.tray.close_notice_shown` が false のときの 1 回だけ）: 最初の × では必ずウィンドウ内の通知（`close-notice`）を出し、ウィンドウは隠さない。「OK」で `close_notice_shown = true` にして隠す。バルーンは使わない: Windows 11 は「応答不可」や通知のオフでバナーを出さないが、API は成功するので「出した」と誤って記録してしまう。新しいアイコンは既定で「^」の中に隠れるので、何も見えずに MKLM が消えたように見える。
  - 文:「MKLM はタスク バー右端の「^」（隠れているインジケーター）の中で動き続けます。アイコンをクリックするとこの画面に戻ります。終了するには、アイコンを右クリックして「終了」を選びます。」
  - ボタン:［タスク バーの設定を開く］（`ms-settings:taskbar`、`SettingsPage::Taskbar`）［MKLM を終了］［OK］。Esc は OK。
  - M3 ではドリフトの見守り（M4）をしないので、「見守ります」とは書かない。「通知領域」という語も使わない（Windows 11 の日本語 UI にない）。
- メニューの色は OS のダークに追従しない（M0 #8 E。J.2）。

### B.17 結果とエラーの表示

`vm::result::result_view(report, run_once, after, lang)`（WP-U3）:

- 先頭に「今の状態」: 要求の対象のキーボードごとに、セッションの後の `Read`（`SystemRead`）から「Keychron Receiver: US として動作中（変更前のまま）」のように書く。読み取りが届くまでは「確かめています…」（U7）。
- 「変更は元に戻されています」は、ジャーナルのエントリがそう言うとき（`Reverted`、`Failed`）だけ書く。エラー コードから推測しない。

| 入力 | 題 | 内容 |
|---|---|---|
| `NotLaunched(Declined)` | 取り消しました（何も変更していません） | — |
| `NotLaunched(Failed{kind})` | 完了できませんでした | `i18n::launch_error`（下の表）。英語の `message` は「技術的な詳細」 |
| `Ended(Finished(result))` | `OutcomeClass`（`classify_result`）の題 | 理由（`i18n::failure` と `describe::reset_phase_from`）、残る操作（`pending_action`）、衝突、INV-PS2、警告（英語は詳細へ） |
| `Ended(Failed(info))` | `classify_error` の題 | `i18n::error_code`（次の手順を含む）。`plan_error` と英語の `message` は詳細へ |
| `Ended(Lost)` と回復 | 回復の結果を `classify_lost_recovery` で | 「MKLM の管理用プログラムが止まったため、すぐに回復しました」。回復しなかった（`recovery_skipped`）なら「変更は確認待ちのまま残っています。［回復…］で回復できます」 |
| `Ended(Unresponsive)` | 完了できませんでした | `i18n::unresponsive`:「MKLM の管理用プログラム（mklm-helper.exe）が応答しません。1 分待っても変わらなければ PC を再起動してください。再起動後に MKLM が回復を案内します。」（標準ユーザーは昇格したプロセスをタスク マネージャーで終了できないため） |
| `Ended(Abandoned)` | 出さない | — |

**エラーごとの次の手順**（U7）

| 種類 | 文 |
|---|---|
| `Registry`、`Device`、`Host`、`Protocol`、`Internal`、`RecoveryAssetsUnavailable` | 何が失敗したか（「キーボードの設定を読み書きできませんでした」など）＋「PC を再起動してからもう一度試してください。直らない場合は［詳細をコピー］を押して、その内容を添えて報告してください。」 |
| `HelperMissing` | 「MKLM の管理用プログラム（mklm-helper.exe）が見つかりません。ウイルス対策ソフトが mklm-helper.exe を隔離していないか、Windows セキュリティ → ウイルスと脅威の防止 → 保護の履歴 で確かめ、許可してから MKLM を修復（インストールし直し）してください。何も変更していません。」（未署名なので隔離がありうる） |
| `StartFailed(225 / 226)` | 「…がウイルス対策ソフトに止められました。」＋上と同じ案内 |
| `StartFailed(1260)` | 「この PC のポリシーで … の実行が禁止されています。PC の管理者に相談してください。」 |
| `StartFailed(その他)`、`Setup`、`NoConnection` | 「…を起動できませんでした。何も変更していません。PC を再起動してからもう一度試してください。」 |
| `ExitedEarly(code)` | m2 E.8 の分類で: 1 / 0 / その他 →「途中で終了しました。PC を再起動して…」、2 / 3 →「MKLM と接続できませんでした。MKLM を修復…」、4 →「管理者の許可が得られませんでした…」、5 →「この Windows では動きません（24H2 以降が必要）」 |
| `OtherBuild`、`Handshake` | 「…の版が MKLM と合いません / 接続を確認できませんでした。MKLM を修復（インストールし直し）してください。何も変更していません。」 |

- 「詳細をコピー」は、英語の診断、操作 ID の先頭 8 桁、ビルド ID をクリップボードに入れる（`mklm_win::ui::copy_text_to_clipboard`、WP-W2）。キーの内容は含まない。
- 次の手順のボタン: `RestartRequired` → 再起動の画面、`Conflict` → 衝突の解決、`Blocked`（回復が必要）→ 回復の画面、US の確定 → 入力方式の案内。
- RunOnce の規則が失敗した、または昇格した GUI で登録できない（`TellUser`）ときは、その旨を結果に添える（`run_once_note`）。
- 結果のバナーは `accessible-live-region: assertive`（E.3）。
- 結果の後は `Read`。RunOnce の規則はワーカーがすでに適用している（A.2.3）。

### B.18 状態遷移

セッションの段階（`SessionPhase`）: `Idle` →（`StartRequest`）`Launching { id }` →（`Notice::Connected`）`Running { id, view }` →（`SessionEnded`）`Idle`。helper を失った後の回復では、`Notice::Starting(Recovery)` で `Launching` に戻り、`Connected` で `Running` になる。

| 状態（ページ / オーバーレイ） | きっかけ | 次 |
|---|---|---|
| main | 「変更…」 | change |
| main | 「今すぐ反映…」 | 切り替え方の 2 択 → `Recover` のセッション |
| change | 選択肢を選ぶ | change（`PrepareChange` の結果で切り替え方と文を表示）/ 結果（止められた、変更なし） |
| change | 「変更する」 | uac-notice（初回のみ。昇格済みなら省く）/ progress（`StartRequest`） |
| uac-notice | 「確認画面へ進む」 | progress（`StartRequest`） |
| Idle 以外 | `StartRequest` | 何もしない（同時に 1 つ） |
| progress | `CountdownStarted` | countdown（ウィンドウを前面に） |
| progress | `WaitingForReconnect` | reconnect（ウィンドウを前面に） |
| progress / countdown | helper を失い、回復が必要 | recovery-confirm（ウィンドウを前面に）→「今すぐ元に戻す」: progress、「後で」: 結果（回復は後で） |
| progress / countdown / reconnect | `SessionEnded`（今の ID） | result（`Abandoned` なら閉じる）、次の手順に応じて restart / conflict / recovery / ime-help / main |
| 任意 | 古い ID のメッセージ | 何もしない |
| countdown | 「このままにする」「元に戻す」、Esc、ウィンドウの × | progress（結果待ち） |
| reconnect | 「後で決める」 | main（バナー） |
| 起動 / `SystemRead` | `post_reboot_due` | post-reboot |
| 起動 / `SystemRead` | `needs_recovery`（今回の起動で未提示） | recovery |
| 起動 | ウィザード未完了 | wizard |
| 任意 | `activate`（2 つ目の起動、RunOnce） | ウィンドウを表示して前面へ。ページは変えない（F.1） |
| 任意 | ウィンドウの ×（セッションなし、初回） | close-notice →「OK」: 非表示、「MKLM を終了」: 終了 |
| 任意 | ウィンドウの ×（セッションなし、2 回目以降） | 非表示 |
| 任意 | 終了（セッションなし） | 終了 |
| Launching（UAC 待ち） | 終了 | すぐに終了（取り消しも立てる。F.5） |
| Running | 終了 | quit-confirm（再接続待ち）/ `RevertNow` して結果の後に終了（カウントダウン）/ 結果の後に終了（書き込み中、`Planned` のない要求） |

`state::tests` で固定した遷移: 2 つ目のセッションの拒否と古い ID の破棄、UAC 待ちの終了、実行中の終了、初回の × と通知、カウントダウン中の ×、回復の質問（1 回だけ答えられる）、同じキーボードの打鍵で描画しない、JIS のキーで実物の配列を学ぶ。

---

## C. テーマ、ハイコントラスト、描画、フォント

### C.1 テーマ（計画 3.7。試作どおり）

1. 起動時、ウィンドウを作る前に `mklm_win::ui::theme::read_os_theme()`（`UISettings.GetColorValue(Foreground)` の明るさ）で OS のテーマを読み、選択（ライト / ダーク / システム）と合わせて解決する（`ThemeMode::resolve`。読めなければライト）。
2. `BackendSelector::with_winit_window_attributes_hook` で `with_theme(Some(解決結果))` を付けてウィンドウを作る（M0 #8 D）。
3. `AppWindow.apply-color-scheme(dark)` で `Palette.color-scheme` に Dark か Light を明示する（Unknown は使わない）。`Theme.dark` も同時に設定する。
4. winit のウィンドウができたら（`spawn_local` で `winit_window().await`）、`set_theme(Some)` と `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE)` を両方呼ぶ（M0 #8 の 12）。
5. OS の変化: `OsThemeWatcher`（`ColorValuesChanged`、スレッド プール → `invoke_from_event_loop`）と、シェル ウィンドウの `WM_SETTINGCHANGE("ImmersiveColorSet")`。「システム」のときだけ適用し直す。計画の `RegNotifyChangeKeyValue(AppsUseLightTheme)` の代わりに、試作で確かめた `WM_SETTINGCHANGE` を補助にする（K.1）。
6. 設定を変えたらその場で 3、4 を行い、`settings.toml` に保存する。
7. `MenuBar` は使わない。

### C.2 ハイコントラスト

- `high_contrast_on()`（`SPI_GETHIGHCONTRAST`）を起動時と `WM_SETTINGCHANGE` のたびに読み、オンなら `system_colors()`（`GetSysColor`: Window、WindowText、Highlight、HighlightText、ButtonFace、ButtonText、GrayText、HotLight）を `Theme` の `hc-*` に渡す。
- 自前の部品（バッジ、行、バナー、打鍵テスト、ダイアログの枠、ナビゲーション、選択肢）は `Theme` のトークンで色を決め、ハイコントラストでは背景色の代わりに枠線を描き、フォーカス枠を 3px にする。
- 「キーを押して特定」で強調した行は、ハイコントラストでは塗らない（`Theme.highlight-row` は Window のまま）。3px の Highlight の枠（`highlight-row-border`）と「◀ いま押したキーボード」のバッジで示す。Highlight を背景にすると、行の WindowText との対比が 2:1 を下回るテーマ（「夜空」など）があるため（U12）。選んだ選択肢の塗り（`selected-fill`）は Highlight と HighlightText の組で、同じ問題はない。
- std-widgets（Button、CheckBox、ComboBox、ScrollView）の色は `Palette` から来て上書きできない。ハイコントラストの背景が暗ければダーク、明るければライトの配色にする（`ResolvedTheme::new` の `hc_background_dark`）。完全に追従させるには自前の部品に置き換える必要がある（J.3）。
- 色だけで情報を伝えない（E.4）ので、ハイコントラストでも意味は失われない。

### C.3 描画方式

- 既定はソフトウェア描画（`renderer_name("software")`）。理由: 常駐するアプリで、メモリが約 1/4.5（28 MB と 127 MB、M0 #8。M3 の骨組みのデバッグ ビルドでも約 35 MB）。GL コンテキストがないので、スリープ復帰やドライバーの更新で描画が失われる心配がない。画面は静的で、アニメーションはカウントダウンの進捗だけなので速度は足りる。
- 開発用に `--renderer=femtovg` で切り替えられる（F.2）。手動テスト T-LONG で長時間の非表示とスリープ復帰を確かめ、問題があれば既定を見直す（J.4）。
- skia は使わない（計画 2.5）。

### C.4 フォント（計画 3.8）

- 日本語 UI は `Yu Gothic UI`、英語 UI は `Segoe UI`（`Lang::font_family`）。`Window.default-font-family` を `Theme.font-family` に結び付け、言語を切り替えたら変える。
- 文字の大きさは `Theme.font-scale`（Windows の「テキストのサイズ」）の倍数: 本文 14px、補足 12px、見出し 20px、ダイアログの題 18px、打鍵テストの文字 36px。`Window.default-font-size` も本文の大きさにする（E.4）。
- 英語 UI でも日本語の機器名（「日本語 PS/2 キーボード」）が表示されることを、骨組みの起動で確かめた（フォールバック）。
- 等幅が要る箇所（レジストリのパス、インスタンス ID）は `Consolas`。

---

## D. 多言語

### D.1 方針

- 日本語と英語。既定は Windows の表示言語に従う（`GetUserDefaultUILanguage` の主言語が日本語なら日本語、それ以外は英語）。設定で固定できる。
- 翻訳は 2 系統で、それぞれ担当を分ける。
  - `.slint` の固定の文言: `@tr("英語")` と、同梱の `translations/ja/LC_MESSAGES/mklm.po`。
  - Rust が組み立てる文言（状態、理由、結果、エラー、バッジ、ビュー モデルのすべて）: `src/i18n.rs` の型ごとの関数。

### D.2 Slint 側

- `build.rs` は `with_bundled_translations("translations")` と `DefaultTranslationContext::None`（文脈なしの msgid）でコンパイルする。原文は英語。
- 更新の手順: `slint-tr-extractor --no-default-translation-context -o mklm.pot ui/*.slint ui/screens/*.slint` → `msgmerge` で `mklm.po` に取り込む → 訳す。
- `tests/translations.rs` が、`ui/` のすべての `@tr` の msgid に空でない訳があること、使われなくなった項目がないこと、`{}` の数が同じことを確かめる。
- 複数形は日本語にないので使わない（`Plural-Forms: nplurals=1`）。数は `{}` で埋め込む。

### D.3 言語の選び方と切り替え

1. 起動時: `args.lang` → `settings.language` → `LangChoice::resolve(user_default_ui_language())`。
2. 最初のコンポーネント（`AppWindow`）を作った後で `slint::select_bundled_translation(lang.bundle())`（日本語は `"ja"`、英語は `""` = 原文）。
3. フォントを `Theme.font-family` に設定する。
4. 設定で変えたら、2 と 3 を行い、`AppState.lang` を変えて全体を描き直す（Rust の文言はビュー モデルから作り直される）。再起動は要らない。
5. トレイのメニューも `@tr` なので切り替わる（Slint がメニューを作り直す）。

### D.4 Rust 側と、エラー文言の責任

| 層 | 持つもの | 持たないもの |
|---|---|---|
| `mklm-core`、`mklm-engine` | 英語の診断（`ErrorInfo::message`、`Warning` の文、`Display`）。ログと CLI 用 | 利用者向けの訳 |
| `mklm-client` | 型（`OutcomeClass`、`BlockReason`、`LaunchFailure`、`HelperExit`、`ResetPhase`）と英語の診断 | 利用者向けの文言 |
| CLI（`render.rs` ほか） | 英語の文言（M1、M2 と同じ） | — |
| GUI（`i18n.rs`） | 型から日英の文言への写像のすべて: `mode`、`table`、`effective`、`pending`、`pending_hint`、`takes_effect`、`apply_method`、`klid_name`、`cannot_change_now`、`transport`、`badge`、`input_method`、`state`、`entry_state`、`failure`、`outcome_title`、`error_code`、`launch_error`、`helper_exit`、`unresponsive`、`attention`、ほか WP で追加 | 英語の診断の翻訳（詳細にそのまま出す） |

- 型ごとに網羅的な `match` で書くので、core に列挙子が増えると GUI がコンパイルできなくなり、訳の漏れに気付く。
- 文言は `vm` のスナップショット テスト（H.3）で日英とも固定する。

### D.5 翻訳の保守

- 用語は計画の「用語」の表に従う（「PC の標準配列」「固定モード」「キーボードごとモード」「標準に従う」「キーボードのリセット」「PC の再起動」「状態表示」）。
- 「再起動」と「シャットダウン」を混同しない。「PC の再起動」はいつも「シャットダウンではなく」を添える画面がある（B.8、B.9）。
- 画面の言葉（U5）:
  - `mklm-helper.exe` は「MKLM の管理用プログラム（mklm-helper.exe）」。「helper」とは書かない。
  - Raw Input の報告は「Windows の認識」。値は配列名（「JIS 配列」「US 配列」「不明な種類（8/2）」）で、0x7/0x2 などは「技術的な詳細」だけ。
  - 入力方式は言語名（「日本語」「英語 (US)」「その他の言語（xxxxxxxx）」）。
  - 値の名前（`KeyboardTypeOverride` など）とレジストリのパスは「技術的な詳細」だけ。
  - 「確認待ちの変更」は、このままにするか元に戻すかをまだ決めていない変更（`AwaitingConfirm`、`PendingReboot`、`Conflict`）をまとめて指す（B.12）。「確定」「確定していない」は画面に使わない（状態は「このままにしました」、理由は「［このままにする］が選ばれる前に…」）。
  - 「キーボードのリセット」には、キーボード本体の設定は変わらないことを添える（U15 e）。
  - 疑問の形の状態（「中断?」）を使わない。
  - 表記: MKLM のボタン名は［］（［このままにする］［今すぐ反映…］）、そのほかの引用（Windows の画面の語、選択肢やチェックボックスの名前）は「」。『』は使わない。2 文以上の文は「。」で終える（一覧の読み上げに連結される短い説明を除く）。日本語の文は空白を挟まずにつなぐ。「確定」と『』は `i18n::tests::japanese_notation` が検出する。
  - 「通知領域」は使わず「タスク バー（「^」の中）」と書く（Windows 11 の日本語 UI の言い方。B.16）。
- 日本語の画面の出力に含まれてよいラテン文字の語は、機器名と許可リストだけ（H.1 の `vm::unexpected_latin`）。

---

## E. アクセシビリティ

### E.1 キーボード操作とフォーカス

- すべての画面をキーボードだけで操作できる。Tab / Shift+Tab でフォーカスが移り、フォーカス枠が見える（`Theme.focus-ring`、ハイコントラストで 3px）。
- フォーカス順は画面の上から下、左から右（Slint の宣言順）。ナビゲーション → ページ見出し → 本文の操作 → 画面下のボタン。
- ページが変わると（ページは `if` で作り直される）、フォーカスは新しいページの見出し（`PageHeader`、フォーカスできる）に移る。フォーカスが消えることはない（U16）。
- 自前の部品（ナビゲーションの項目、キーボードの行）は `FocusScope` で、Enter と Space で既定の操作、`accessible-action-default` でも同じ操作。
- 選択肢（配列、切り替え方）は `RadioList`: グループ全体で 1 つの Tab の止まり、上下（左右）の矢印キーで選択が動く。選んだものは ◉ と塗りで、フォーカスは外側の別の枠で示すので、選択とフォーカスを取り違えない（U16）。
- オーバーレイが開いている間、下のページは操作できない（`DialogFrame` がクリックを吸収し、ページ側の部品を `enabled: false` にする。WP-U7）。開いたら最初の操作（カウントダウンなら打鍵テスト）にフォーカスを移し、閉じたら開く前の部品に戻す。
- ショートカット: F5 = 最新の情報に更新、Esc = オーバーレイの「キャンセル / 元に戻す / 閉じる」（カウントダウンでは「元に戻す」。打鍵テストの欄は Esc を受け取らず、`DialogFrame::escape` に渡す）。確定の操作にショートカットは付けない（B.6）。
- 打鍵テストの欄は Tab と Esc を受け取らない（`reject`）ので、欄から抜けられる。
- 「キーを押して特定」の間は、キーの受け取り欄（`CaptureArea`）がフォーカスを取り、Tab と Esc 以外のキーを吸収する。Esc で終わる（B.3、U12）。
- 一覧の行は描画のたびに作り直さない（A.6）ので、Tab で選んだ行のフォーカスは、ほかのキーボードの打鍵でも失われない（A7）。

### E.2 アクセシブル名と役割

| 部品 | `accessible-role` | 名前・説明・値 |
|---|---|---|
| ナビゲーション | `tab-list` / `tab` | 項目名、選択中は `accessible-item-selected` |
| ページの本体 | `main` | — |
| ページの見出し | `text`（フォーカスできる） | 題、説明に副題 |
| キーボードの一覧 | `list` / `list-item` | 行の `accessible-summary`（名前、接続方式、設定した配列、反映待ち、現在の動作、実物の配列の警告、バッジの説明、変更できない理由を 1 文にしたもの） |
| 行の「変更…」 | `button` | 「Keychron Receiver の配列を変更」 |
| 履歴の「元に戻す…」 | `button` | 「2026/09/27 22:25 の変更を元に戻す」 |
| 選択肢 | `radio-group` / `radio-button` | グループ名（「配列」「切り替え方」）、選択肢の文、説明、`accessible-checked`、`accessible-enabled` |
| コンボボックス | `combobox` | 「テーマ」「言語」「PC の標準配列」「Keychron Receiver の配列」「Keychron Receiver をどうするか」など、何を選ぶかを名前にする |
| 打鍵テスト | `text-input` | 名前「打鍵テスト」、説明に指示（カウントダウンでは時間の制限と残し方を含む全文）、値に最後の文字 |
| 特定のキーの受け取り欄 | `text-input` | 名前「キーを押して特定」、説明に指示 |
| バッジ | `text` | 長い説明（「キー入力なし: このデバイスからのキー入力をまだ見ていません…」） |
| ダイアログの枠 | `groupbox` | ダイアログの題 |
| 状態行の各項目 | `text` | 「入力方式: 日本語 IME ✓」 |
| 再起動後の確認の行 | `list-item` | 「Keychron Receiver: US、US 配列 ✓、未」 |

### E.3 状態の読み上げ

- バッジと状態は、色ではなく文で読み上げる（E.2 の長い説明）。
- 進行状況（Progress の手順）、再接続待ちの状況、配列判定の結果、打鍵テストの判定は `accessible-live-region: polite`、結果のバナーは `assertive`。
- カウントダウン（U10）: 開いたときに、何を切り替えたか、あと何秒で戻るか、どう残すかの 1 文を 1 回だけ assertive で読み上げる。同じ文を打鍵テストの欄の説明にも入れる（フォーカスが欄に移ったときにも読まれる）。残り秒は 10 秒と 5 秒のときだけ polite で読み上げる。毎秒は読まない。`ProgressIndicator` の `accessible-label`「残り N 秒」で、利用者が尋ねたときにも分かる。
- 「キーを押して特定」で行が変わったら「Keychron Receiver のキーが押されました」を polite で読み上げる（B.3）。

### E.4 そのほか

- 状態を色だけで示さない。✓、⚠、◉ / ○、「◀ いま押したキーボード」のような文を併記する。ハイコントラストでは、特定した行を塗らずに 3px の Highlight の枠で示す（C.2）。
- 表示スケールは Slint と winit の DPI 対応（PerMonitorV2）に従う。Windows の「テキストのサイズ」（`UISettings.TextScaleFactor`）は Slint が適用しないので、アプリが読んで `Theme.font-scale` に渡し、`TextScaleFactorChanged` で追従する（U11）。自前の部品のすべての文字の大きさと、打鍵テストの欄の高さは、その倍数で決める（`Theme.font-body` など）。`Window.default-font-size` も同じ値にする。std-widgets がこれに従うかは T-A11Y-4 で確かめ、従わない部品は J.3 の置き換えと合わせて決める。
- ウィンドウの最小の大きさは 560 × 400（論理ピクセル）。200 % の 1920 × 1080（作業領域は約 960 × 492）と、150 % の 1366 × 768（約 910 × 480）に収まる。狭いときはナビゲーションを 160px まで縮める。
- どのダイアログとページも、ボタンの行を本文のスクロールの外に置く（B.0）。
- 点滅やアニメーションを使わない（進捗バーを除く）。
- 手動確認は T-A11Y-1〜6（H.4）。

---

## F. プロセスまわり

### F.1 多重起動の防止（計画 2.2、3.9。A6）

ミューテックスの名前もパイプの名前も秘密ではない。SID とセッション ID は推測でき、パイプの名前空間はマシン全体で共通なので、別のアカウント（や、同じセッションの `runas` のプロセス）が先に作れる。名前は衝突を起こりにくくするだけで、なりすましを防ぐのは次の確認である。

- ミューテックス `Local\SHINDATACENTER.MKLM.Instance.<利用者の SID>`（`CreateMutexW`）。`ERROR_ALREADY_EXISTS` なら 2 つ目。`ERROR_ACCESS_DENIED`（ほかの利用者が制限的な DACL で先に作った）は、ログに書いて 1 つ目として起動を続ける（乗っ取られた名前で MKLM を止めない）。`Local\` はセッションごと。計画 2.2 が禁じる `Global\` のミューテックスは書き込みのロックの話で、これには当たらない。
- 1 つ目はパイプ `\\.\pipe\SHINDATACENTER.MKLM.Instance.<セッション ID>.<SID>` を作る。`FILE_FLAG_FIRST_PIPE_INSTANCE`、`PIPE_REJECT_REMOTE_CLIENTS`、DACL は計画どおり `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;<SID>)`（`instance_pipe_sddl`）。作成が `ERROR_ACCESS_DENIED` か `ERROR_PIPE_BUSY`（名前が取られている）で失敗したら、ログに書いてパイプなしで起動を続ける。
- 受け付けるのは 1 接続につき 1 つのコマンドだけ: `activate\n` か `quit\n`（最大 16 バイト）。接続から 1 秒（`INSTANCE_READ_TIMEOUT`）以内に届かなければ `DisconnectNamedPipe` する（つないだまま黙るクライアントで受け付けが止まらないように）。返事は `ok\n` か `busy\n`。それ以外は切断する。データは何も運ばない。
- 2 つ目: 接続（`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`。最大 5 秒再試行。サインイン時は Run と RunOnce が同時に起動し、ミューテックスの直後にパイプができるため。1 つ目は、コマンドを UI スレッドに渡せるようになった時点（Slint のバックエンドを選んだ直後、ウィンドウ、トレイ、ウォッチャーより前）でパイプを作って受け付けを始める。届いたコマンドはイベント ループが動くまで Slint のキューで待ち、パイプのスレッドはその分も見込んで 4 秒まで返事を待つ）→ サーバーを確かめる（`ServerIdentity`、`is_our_instance`）:
  - `GetNamedPipeServerSessionId` が自分のセッションと同じ。
  - `GetNamedPipeServerProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` → `OpenProcessToken` のユーザー SID が自分と同じ。
  - `QueryFullProcessImageNameW` が自分の実行ファイルと同じパス（大文字小文字を区別しない）。
  
  すべて一致したときだけ `AllowSetForegroundWindow(その PID)` → `activate`（`--quit` なら `quit`）→ 終了コード 0 で終わる。一致しなければ何も送らず、ログに書いて自分が 1 つ目として動く。M5 の昇格した送り手（更新）にも同じ確認を必須にする。
- 1 つ目は `activate` で、ウィンドウを表示し、`focus_window` で前面に出し（計画 3.9）、`Read` する。**ページは変えない**（A8）: `--tray` のインスタンスが出した再起動後の確認や、作業中の変更ページを置き換えないため。再起動後の確認や回復の画面に移るかどうかは、次の `SystemRead` で `post_reboot_due` などから決める（B.9）。`--post-reboot` の情報は運ばない。
- `quit`: セッションがなければ終了して `ok`。セッション中は `busy` を返し、F.5 の手順で終わり次第終了する。

### F.2 コマンドライン

| 引数 | 意味 |
|---|---|
| （なし） | ウィンドウを表示して起動（2 つ目なら 1 つ目を前面に） |
| `--tray` | タスク バー（「^」の中）で起動（HKCU の Run）。ただし、ウィザードが未完了か、再起動後の確認・回復・衝突があればウィンドウを出す |
| `--post-reboot` | RunOnce から。表示する内容はジャーナルで決める。確認することがあれば最前面に出す。なければ `--tray` と同じ（F.5 のセッション終了時の登録は、結果が確定していないうちに無条件に行うため）。`--tray` より優先 |
| `--quit` | 動いているインスタンスに `quit` を送って終わる（M5 のインストーラー） |
| `--theme=light\|dark\|system`、`--lang=ja\|en\|system`、`--renderer=software\|femtovg`、`--exit-after=秒` | 開発と自動確認用。ヘルプには出さない |

- 解釈できない引数はログに書いて無視する（起動は止めない）。
- RunOnce の値は `"<dir>\mklm.exe" --post-reboot`（`PostRebootCommand::Gui`）。登録は、どのセッションの後でも（結果によらず）ジャーナルで決める（m2 F.4、C17）。GUI ではセッション ワーカーが要求の直後に同じスレッドで行う（`run_request`、A.2.3）。GUI は常に GUI を登録する。CLI は、隣の `mklm.exe` が同じビルド ID を持てば GUI を、なければ CLI を登録する（`PostRebootCommand::preferred`、WP-C1。m2 F.4 の「M3 では GUI がインストールされていれば GUI を登録する」）。
- GUI を昇格したまま起動した場合（管理者として実行）: 動作はするが、RunOnce は登録しない（別の管理者の HKCU かもしれないため。`RunOnceOutcome::TellUser`）。その旨をバナーと結果で知らせる。

### F.3 自動起動（計画 3.9）

- HKCU の `Software\Microsoft\Windows\CurrentVersion\Run` の `SHINDATACENTER.MKLM` = `"<dir>\mklm.exe" --tray`。書くのは GUI 自身（利用者ごとの値なので許される。計画 3.9）。書き込みは `mklm_win::session` だけ。
- 既定はオン: ウィザードの最後で登録する。設定画面の文は「サインイン時に MKLM をタスク バー（「^」の中）で起動する」。
- 設定画面のスイッチは値そのもの（`autostart_state`）を表示する。`settings.toml` には持たない。
- 起動時、値が別のパスを指していれば（MKLM を移した）書き直す（`autostart::needs_repair`）。タスク マネージャーで無効にされていれば（`StartupApproved\Run` の先頭バイトが奇数）、決して有効に戻さず、設定画面に「Windows のスタートアップ設定で無効になっています」と示す。
- MSI の機能にはしない。アンインストール（M5）で値を消す。
- **昇格している MKLM は、この値を書かない**（起動時の書き直しも含む）。昇格したプロセスは、サインインしている利用者とは別の管理者として動いている場合があり、その HKCU は別のアカウントのものになるため（F.2 の RunOnce と同じ規則。2026-09-28 にユーザーが決定）。
  - 昇格しているかどうかが分からないときも、昇格しているものとして扱う。
  - 設定画面は、値の有無をそのまま示す。スイッチは使えない状態にし、「管理者として実行している MKLM では、サインイン時の起動を変更できません。MKLM を通常の方法で開き直してから変更してください。」と示す（`AutostartNote::Elevated`）。

### F.4 設定の保存

- `%APPDATA%\SHIN DATA CENTER\MKLM\settings.toml`（`FOLDERID_RoamingAppData`、計画 3.7）。
- 内容（`Settings`）: `schema`、`theme`、`language`、`wizard.completed`、`tray.close_notice_shown`、`keyboards.hidden`、`keyboards.seen`、`keyboards.show_hidden`、`keyboards.physical`（インスタンス ID ごとの実物の配列と、その出どころ: 判定か JIS にしかないキー。B.2）、`recovery.prompted`、`change.uac_notice_seen`、`change.countdown_seconds`（20 か 60）。キーの内容は持たない（`seen` と `physical` はインスタンス ID と、そこから分かった事実だけ）。
- 読み取りは起動時に 1 回（UI スレッド、ウィンドウの前）。ないなら既定値。読めなければ既定値で続け、次の保存で `settings.toml.bad` に残す（WP-U6）。
- 保存は変更のたびに I/O ワーカーで、一時ファイルに書いてから名前を変える（`settings.toml.tmp` → `settings.toml`）。
- 自動起動の状態、「アンインストール時に元に戻す」、キーボードの設定は、ここには持たない（それぞれ HKCU の Run、HKLM、ジャーナル）。

### F.5 閉じる、終了する、セッションの終わり、更新

| 場面 | セッションなし | UAC 待ち（`Launching`） | カウントダウン中 | 書き込み・リセット中、`Planned` のない要求 | 再接続待ち |
|---|---|---|---|---|---|
| ウィンドウの × | トレイへ（初回はウィンドウ内の通知。B.16） | 閉じない | 「元に戻す」と同じ（`RevertNow`）。ウィンドウは閉じない | 閉じない（「処理中です」） | 閉じない（ダイアログの「後で決める」を使う） |
| トレイの「終了」、`quit` | 終了 | すぐに終了（取り消しも立てる）。パイプのサーバーが消えるので、後で「はい」が押されても helper は接続できず、何も書かない | `RevertNow` → 結果の後に終了 | 結果の後に終了（`quit_pending`） | 確認（`quit-confirm`: 元に戻して終了 / 終了して後で決める / キャンセル） |
| サインアウト、シャットダウン、再起動（`WM_QUERYENDSESSION`） | — | 取り消しを立てる | 取り消しを直接立てる（relay は次の tick で `RevertNow`）。記録済みなので RunOnce を登録する | 取り消しを立てる。記録済みなら RunOnce を登録する | 取り消しを立てる（relay は中継をやめる）。RunOnce を登録する |
| 同（`WM_ENDSESSION`） | 終了 | 終了 | ワーカーの終わりの合図を最大 3 秒待って終了 | 同じく最大 3 秒 | 待たずに終了（確認待ちのまま残る） |
| 強制終了 | — | — | helper がパイプの切断で戻す（R16） | helper が書き切る | 確認待ちのまま残る |

- `WM_QUERYENDSESSION` は拒否しない（書き込みは helper とジャーナルが守る）。
- **セッション終了の扱い**（A5）: シェル ウィンドウのシンクは、`WM_QUERYENDSESSION` を受けた時点で、今のセッションの `SessionShared::cancel`（`Mutex` の中の静的な場所にある `Arc`。`RefCell` の外）を直接立てる。予約した処理（`invoke_from_event_loop`）は、セッションが終わるまでに走る保証がないため。記録済みの操作が開いていれば（`journaled`）、同じ場所で HKCU の RunOnce（`--post-reboot`）を無条件に登録する（ジャーナルがまだ途中の状態でも、次のサインインで回復か確認が出るように）。`WM_ENDSESSION` では、ワーカーが要求と RunOnce の規則を終えた合図（`Condvar`）を最大 3 秒待ち、残りの時間（最大 1 秒）で I/O ワーカーの保存を待つ。合わせて 3 秒を超えない（Windows が応答なしとみなす 5 秒の内）。`SessionEnded` の処理は待たない。昇格した GUI は RunOnce を登録しない（F.2。起動時に求めた昇格の状態を `RefCell` の外の静的な値に置く）。
- **正直な限界**: サインアウトやシャットダウンでは、helper（ウィンドウのない windows サブシステムのプロセス）も同じセッション終了で終了させられうる。そのため、パイプの切断で helper が戻す動作（C8）は、この場面では保証されない。WP-E3 で helper が `SetProcessShutdownParameters(0x100, SHUTDOWN_NORETRY)` と自分の `WM_QUERYENDSESSION` の処理を持ち、カウントダウンを自分で戻すようにする（A.5）。それでも間に合わなければ、確定していない配列が次のサインイン画面で有効になりうる。最後の砦は次回起動時の回復（RunOnce で必ず出る）である。
- 終了の順序: トレイを消す → ウォッチャーを止める → `APP` を空にする。I/O ワーカーの未処理の保存は、終了前に最大 1 秒待つ（WP-U6）。RunOnce の規則はセッション ワーカーが `SessionEnded` の前に終えているので、終了が追い越すことはない（A3）。
- ワーカーがパニックしても、ガードが `SessionEnded` を送るので、終了の要求が止まったままにならない（A11）。
- **M5 の更新**（A12）: 更新の helper は、元の GUI のセッションの中で動く。helper は複製の準備ができたら、そのセッションに専用の最後のメッセージ（更新の準備ができた）を送ってセッションを閉じ、GUI はそれを受けてセッションを終え、そのまま終了する（計画 4.2 の 6「GUI はパイプの指示で終了する」）。helper は GUI の PID の終了を待ってから msiexec を実行する。多重起動のパイプの `quit` はセッション中は `busy` を返すので、更新には使わない（互いに待ち合って止まるため）。`quit` はインストーラーとアンインストーラーの `--quit` 専用。

### F.6 起動時の検査とエラー表示

1. `restrict_dll_search()`（失敗は警告。GUI は非昇格で動くため。CLI と同じ）。
2. `read_os_info` の build が 26100 未満なら「Windows 11 24H2 以降が必要です」を出して終了（計画 4.1）。
3. 多重起動（F.1）。
4. 設定、言語、テーマ → ウィンドウ。
5. ウィンドウを作る前のエラー（OS の版、描画の初期化）は、リリース ビルドにはコンソールがないので `mklm_win::ui::error_dialog`（`MessageBoxW`、WP-W2）で出す。骨組みは標準エラーに書くだけ。

### F.7 DLL、マニフェスト、VERSIONINFO、アイコン

- DLL: 起動直後に `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)`、リンク時に `/DEPENDENTLOADFLAG:0x800`、VC ランタイムは静的、UCRT は動的（CLI と helper と同じ。`build.rs`）。
- マニフェスト（`res/mklm.exe.manifest`）: `asInvoker`、`supportedOS`（Windows 10/11）、`activeCodePage` UTF-8、`longPathAware`、`dpiAwareness` PerMonitorV2、Common Controls 6（エラー ダイアログ用）。
- VERSIONINFO（`res/mklm.rc`）: `CompanyName` SHIN DATA CENTER、`FileDescription` / `ProductName` Multi Keyboard Layout Manager、`LegalCopyright` © 2026 SHIN DATA CENTER、`MKLMBuildId`。
- サブシステム: リリースは `windows`、デバッグは `console`（ログを見るため）。
- アイコン: ウィンドウとトレイは `icon.rs` がコードで描く。実行ファイルのアイコン リソース（`.ico`）は WP-U7（M5 で正式な画像に置き換える）。

### F.8 ログ

- `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\logs\mklm.log`（1 MB で 1 世代を `mklm.log.1` に回す）。英語。起動と終了、読み取りの問題、セッションの始まりと終わり（要求の種類、結果の分類、英語の診断）、ウォッチャーのエラー。
- **書かないもの**: キーの文字、スキャン コード、打鍵テストの内容（計画 2.2）。
- M3 は依存を増やさない小さな書き込み関数にする（WP-U6）。計画 2.5 の `tracing` は M5 で検討する（K.10）。

---

## G. M2 実機テストからの修正

### G.1 ジャーナルの時刻をローカル時刻で表示する（R4）

- ジャーナルは UTC のミリ秒（`Timestamp`）のまま保存する（形式は変えない）。表示だけ変える。
- `mklm_win::time::local_time(Timestamp) -> LocalTime`（実装済み）: FILETIME → `FileTimeToSystemTime` → `SystemTimeToTzSpecificLocalTimeEx(None)`（その時点の夏時間の規則を使う）→ 差から UTC との差（分）。`to_iso_text()` は `2026-09-27 22:22:05 +09:00`。
- CLI（WP-G）: `journal_view::timestamp_text` を `local_time` に置き換え、`2026-09-27 22:22:05 +09:00` の形にする（変換に失敗したら従来の UTC 表記）。`journal --json` は機械向けなので UTC のミリ秒のまま（変えない）。テストは形（長さと区切り）と、固定の `LocalTime` の書式を確かめる。
- GUI: 履歴（B.11）は日本語で `2026/09/27 22:22`、英語で `2026-09-27 22:22`（秒と差は詳細の表示だけ）。`vm::journal::journal_rows` は時刻の書式関数を引数に取るので、テストは固定の書式で行う。

### G.2 回復の理由の文言（R5）

- 事実: `LiveResetUnconfirmed` は「リセット経路の変更が確定されないまま回復された」ことで、リセットの前（`Planned`、`Written`）にも後（`Restarting`、カウントダウン中）にも付く。R5 はリセットの前だった。
- 理由の値（ジャーナルの形式）は変えない。文言を選ぶための事実を `mklm_client::describe` に置いた:
  - `reset_phase(entry)`: 履歴に `Restarting` への遷移があれば `Reached`、なければ `NotReached`。
  - `reset_phase_from(RecoveredOp.from)`: `Planned` と `Written` は `NotReached`、それ以外は `Reached`。
- 文言:
  - NotReached: 「キーボードをリセットする前に止まったため、回復で元に戻しました（キーボードの動作は変わっていません）」/ "The writer stopped before the keyboard reset; recovery put the values back (the keyboard never switched)"
  - Reached: 「キーボードのリセット後に［このままにする］が選ばれなかったため、回復で元に戻しました」/ "The change was never kept after the keyboard reset; recovery put it back"（D.5 のとおり「確定」は画面に使わない）
- GUI は `i18n::failure(reason, phase, lang)`（実装済み、テスト済み）。CLI（WP-G）は `render::failure_text(failure, phase)` にし、`result_text` の回復の行は `reset_phase_from(recovered.from)`、`journal` は `reset_phase(entry)` を使う。

---

## H. テスト計画

### H.1 単体テスト（ウィンドウなし。`cargo test --workspace`）

| 対象 | テスト |
|---|---|
| `mklm-client::session` | `SessionView::observe` がライブ リセットの一連のイベントを正しく追う。質問の前の決定と別の操作への決定を捨てる。ボタンの決定を送る。取り消し: カウントダウン中は `RevertNow`、`plans_first` の要求の `Planned` の前は `Abandoned`、書き込み中は待ってからカウントダウンで `RevertNow`、再接続待ちは `Abandoned { planned: true }`、`Revert` と `Undo` は待って結果を受け取る。helper の喪失と無応答（済） |
| `mklm-client::orchestrator` | UAC の拒否、カウントダウン中に失った helper の直後の回復（通知の順序、`Connected` を含む）、回復が要らない場合、正常な結果、同意しなければ回復を起動しない、終了の後は回復を起動しない、`run_then` の `after` は必ず走る（済）。WP-C1 で、CLI の出力が変わらないことを CLI のテストで確かめる |
| `mklm-client::outcome`、`gate`、`describe` | 分類、型付きの `BlockReason`、`ResetPhase`（済）。CLI の既存テストが、同じ文言と終了コードを確かめる |
| `mklm-cli` | 既存の 51 件がそのまま通る。WP-G で、ローカル時刻の書式と G.2 の文言 |
| `mklm-win` | `time`（書式、この PC の時差で妥当な値）、`instance`（コマンドの厳密な解釈、名前と SDDL、`is_our_instance` がほかの利用者・セッション・プログラムを拒む）、`machine_settings` の読み取り、`ui::theme` の読み取り（テキストのサイズを含む）（済）。WP-W1 で、同じプロセス内で多重起動のパイプを往復させる（非昇格でできる） |
| `mklm-engine`（WP-E1、E2、E3） | `cleanup_values`: ドライバーが読む名前は拒否、Keyboard 以外の devnode は拒否、baseline の記録、クラッシュの網羅（m2 H.1 と同じ I1〜I7）、schema 2 のエントリの読み書き、古いビルドが `NewerSchema` として扱うこと。`set_machine_settings`: ロック、許可リストの名前、フラッシュ。`countdown_seconds`: 20 と 60 だけを受け付ける、既定は 20 |
| `mklm-helper`（WP-E3） | セッション終了の通知で、動いているカウントダウンを戻す（偽のウィンドウ メッセージで。再接続待ちは戻さない） |
| `mklm-ipc` | 2 つの要求と `ApplyOptions` の serde の往復と、JSON の形のスナップショット（`PROTOCOL_VERSION` 2） |
| GUI `args`、`theme`、`i18n`、`settings`、`detect`、`autostart` | 済。`settings` は実物の配列の記録（判定を JIS のキーで上書きしない）、`detect` は答えのキーでだけ固定・対象以外のキーボード・やり直し |
| GUI `state` | 済（B.18 の末尾）。WP ごとに B.18 の残りの遷移を足す |
| GUI `vm::*` | 状態行（固定モード、英語のサインイン画面）と一覧（✓ の規則、実物の配列、変更できない理由）、カウントダウン（読み上げの文、10 秒の知らせ）、打鍵テスト（期待との比較、ほかのキーボード、入力方式）、切り替え方の既定値、衝突のおすすめと選択、ウィザードの移行の割り当て、一覧の差分（済）。WP で各画面 |
| GUI の文言 | 日本語の画面の出力に含まれてよいラテン文字の語は、機器名と許可リスト（Shift、Alt、Ctrl、Space、Backspace、Tab、Esc、Enter、Win、USB、PS/2、Bluetooth、LE、JIS、US、MKLM、mklm-helper.exe、PIN、IME、PC、Microsoft、Windows）だけ。16 進の ID は除く（`vm::unexpected_latin`。状態行、一覧、カウントダウンで済。WP で各画面） |
| GUI の翻訳 | `tests/translations.rs`（済） |

### H.2 共有クライアントと CLI の回帰

- `mklm-client` を変えたら、`cargo test -p mklm-cli -p mklm-client` が通ること。CLI のテストは、出力の文字列と終了コードを直接確かめているので、抽出による変化を検出できる。
- WP-C1 の完了条件に、M2 の実機テストのうち再起動の要らないもの（R1、R2、R3、R12、R16）を CLI でもう一度行うことを含める（ユーザーの同意を得て）。

### H.3 画面の文言のスナップショット

- `vm::SnapshotText` を各ビュー モデルに実装し、1 行 1 項目の文字列にする。
- 期待値はテストに埋め込む（小さいもの）か、`apps/mklm/tests/snapshots/<名前>.<ja|en>.txt` に置く（大きいもの、WP-U1 以降）。`MKLM_BLESS=1` のときだけ期待値を書き直す小さな補助関数を `tests/common` に置く（依存を増やさない）。
- 入力は `mklm_core::fixtures`（開発機、固定 JIS の開発機）と、ジャーナルのエントリの組み立て関数。各画面の主要な状態（B 章の図の状態）を日英で固定する。

### H.4 手動テストのチェックリスト（後でユーザーの同意を得て行う）

**準備**: M0 の安全手順（`reg export`、PIN、スクリーン キーボード、BitLocker の回復キー）。`mklm-cli status --json --all > before.json`。リリース ビルド（`cargo build --release --workspace`）の `mklm.exe` と `mklm-helper.exe` を同じフォルダーで使う。書き込みを伴う項目（★）は、1 項目ずつ同意を得てから行う。

**起動と多重起動**

- [ ] T-START-1: `mklm.exe` を起動 → 1 秒程度でウィンドウ。状態行（入力方式、モード、標準配列、サインイン画面）と一覧（内蔵、Keychron）が正しい。内蔵キーボードに「キー入力なし」が付かない
- [ ] T-START-2: もう一度起動 → 2 つ目は出ず、1 つ目が前面に出る。1 つ目のページは変わらない
- [ ] T-START-3: `mklm.exe --tray`（ウィザード完了後）→ ウィンドウなしでトレイだけ。トレイのクリックで表示
- [ ] T-START-4: `mklm.exe --quit` → 動いているインスタンスが終わる
- [ ] T-START-5: 初回起動（`settings.toml` を退避）→ ウィザードが開き、見出しが「手順 1 / 4」。「後でセットアップする」で閉じ、設定から開き直せる

**テーマと言語**

- [ ] T-THEME-1: ライト / ダーク / システムの切り替えで、画面とタイトルバーが両方変わる
- [ ] T-THEME-2: 「システム」のまま Windows のアプリ モードを切り替える → 追従する
- [ ] T-THEME-3: OS と逆のテーマに固定し、`SystemPropertiesAdvanced` → 環境変数 → OK → タイトルバーが変わらない
- [ ] T-THEME-4（ハイコントラスト）: 設定 → アクセシビリティ → コントラスト テーマを「夜空」「砂漠」にする → 自前の部品がシステムの色になり、枠とフォーカスが見える。「キーを押して特定」で強調した行の文字がすべて読める（塗らずに枠とバッジ）。std-widgets の見え方を記録する
- [ ] T-LANG-1: 設定で日本語 ⇄ English → 再起動なしで全画面とトレイのメニューが切り替わる。フォントが Yu Gothic UI / Segoe UI
- [ ] T-LANG-2: 英語 UI で日本語の機器名が正しく表示される

**IME と入力方式**

- [ ] T-IME-1: 日本語 IME で入力欄（LineEdit を使う画面）に日本語を入力・変換できる
- [ ] T-IME-2: 入力方式を英語 (US) に切り替える → 0.5 秒以内に状態行が「英語 (US) ⚠」になる。戻すと「日本語 IME ✓」。トレイに隠している間は読みに行かない（T-LONG-2 の CPU 時間）
- [ ] T-IME-3: 入力方式の案内の各ボタンで、設定アプリの正しいページが開く。「地域（設定のコピー）」は System32 の `control.exe` で開く

**特定と判定**

- [ ] T-ID-1: 「キーを押して特定」→ Keychron で打つと Keychron の行、内蔵で打つと内蔵の行に「◀ いま押したキーボード」が付き、枠で囲まれる。「キー入力なし」が消え、次回の起動でも消えたまま
- [ ] T-ID-2: ほかのアプリを前面にして打っても強調は変わらない（前面のときだけ）
- [ ] T-ID-3: 特定の間に、探しているキーボードで Space と Enter を押す → 特定は続き、ボタンや行は反応しない。Esc で終わる
- [ ] T-DETECT-1: Keychron の変更ページで、Keychron の Backspace の左（=）と右 Shift の左（/）→「US」。途中で内蔵を押すと「内蔵キーボードのキーです。Keychron Receiver で押してください」で、数えない。内蔵の変更ページで ¥ と ろ →「JIS」
- [ ] T-DETECT-2: 打鍵テストの欄では IME がオンでも変換されない

**メイン画面**

- [ ] T-MAIN-1: Keychron のドングルを抜く → 1 秒ほどで行が消える（「非表示と未接続も表示」で「未接続」）。差すと戻る
- [ ] T-MAIN-2: VXE のマウスのキーボード用コレクションが「キー入力なし」で出る。行のメニューで非表示にでき、表示に戻せる
- [ ] T-MAIN-3: Keychron に「レシーバー」のバッジと説明が出る
- [ ] T-MAIN-4: Tab で Keychron の行にフォーカスを置き、内蔵キーボードで何度か打鍵する → フォーカスは Keychron の行に残り、Enter で変更ページが開く（A7）

**適用（★ ユーザーの同意を得て。Keychron、USB）**

- [ ] T-APPLY-1 ★: Keychron の「変更…」→ JIS を選ぶ →「確認しています…」の後、切り替え方が「すぐに切り替えて 20 秒間試す」になっている（マウスを使ったため）→ 初回は UAC の説明の画面 →「確認画面へ進む」→ UAC（題、プログラム名 mklm-helper.exe、確認済みの発行元「不明」、ファイルの入手先の表示を、説明の画面と照らして記録する）→「はい」→ カウントダウン（ウィンドウが前面）。打鍵テストで Shift+2 →「✓ 期待どおり JIS です」、入力元が Keychron →「このままにする」→ 結果「完了しました」、今の状態「Keychron Receiver: JIS として動作中」。一覧が「JIS として動作中 ✓ 設定どおり」。続けて US に戻す（2 回目は説明の画面がなく、ボタンの横の 1 行だけ）→ `@`。結果に IME の案内が出る
- [ ] T-APPLY-2 ★: JIS にしてカウントダウンで何もしない → 20 秒で元に戻り、結果「自動で元に戻しました」。Keychron で `@`
- [ ] T-APPLY-3 ★: UAC で「いいえ」→ 「取り消しました（何も変更していません）」。ジャーナルが増えない
- [ ] T-APPLY-4 ★: カウントダウン中にトレイの「終了」→ すぐに元に戻り、MKLM が終わる。Keychron で `@`。UAC がもう一度出ることはない
- [ ] T-APPLY-5 ★: カウントダウン中にタスク マネージャーで `mklm.exe` を強制終了 → helper が戻す（R16 と同じ）。次の起動で履歴に「［このままにする］が選ばれる前に MKLM が終了したため、元に戻しました」
- [ ] T-APPLY-6: CLI でカウントダウン中に、GUI で変更を始めようとする → 「別の MKLM が処理中です」で UAC を出さない
- [ ] T-APPLY-7 ★: カウントダウン中に Esc → 元に戻る
- [ ] T-APPLY-8 ★: カウントダウン中に helper を `taskkill /F` で止める（昇格したターミナル）→ 「MKLM の管理用プログラムが止まりました」の質問が出る。「後で」→ UAC は出ず、バナーに回復が出る。もう一度同じ手順で「今すぐ元に戻す」→ UAC → 回復して `@`
- [ ] T-APPLY-9 ★: キーボードだけで（マウスに触れずに）Keychron を JIS に → 切り替え方の既定が「すぐに」（内蔵キーボードで打ったため）。Keychron だけで操作した場合は「PC の再起動」と「唯一のキーボード」の警告（確かめるだけで、適用はしない）

**再起動と再起動後の確認（★ 再起動 1〜2 回。内蔵キーボードには触れない）**

- [ ] T-POST-1 ★: Keychron を JIS に。切り替え方で「PC の再起動で切り替える」を選ぶ → 反映方法の文が「PC を再起動すると反映されます … 再起動するまで、MKLM でほかの変更はできません」になることを確かめて適用 → 再起動の画面。確認のチェックなしでは「今すぐ再起動」が押せない。パスワードの警告と「サインイン オプションを開く」がある
- [ ] T-POST-2 ★: 「今すぐ再起動」→ サインイン後、確認画面が最前面に出る。表で Keychron「JIS / JIS 配列 ✓ / 未」、打鍵テストで `"` → 打鍵の列が「✓」→「元に戻す」→ 次は Keychron の再接続か再起動で US に戻ることを確かめる（結果の案内どおり）
- [ ] T-POST-3（任意）: T-POST-1 の後、シャットダウン → 起動 → 「まだ反映されていません。『シャットダウン』ではなく『再起動』…」（高速スタートアップが有効な場合）
- [ ] T-END-1 ★（任意）: Keychron のカウントダウン中にサインアウトする → 次のサインインで確認か回復の画面が出る（RunOnce）。Keychron の状態と履歴を記録する（helper がセッション終了で戻せたかどうか。WP-E3 の前後で比べる）

**トレイ**

- [ ] T-TRAY-1: 初めての × → ウィンドウ内に通知が出て、ウィンドウは隠れない。「OK」で隠れる。2 回目の × は通知なしで隠れる。「タスク バーの設定を開く」でタスク バーの設定が開く
- [ ] T-TRAY-2: explorer を再起動 → アイコンが戻り、メニューが動く。MKLM を管理者として実行した状態でも同じ（隠しウィンドウが `ChangeWindowMessageFilterEx` で `TaskbarCreated` を通す。UIPI）
- [ ] T-TRAY-3: 「応答不可」をオンにして、`settings.toml` の `close_notice_shown` を false に戻し、× → 通知がウィンドウ内に出る（バルーンに頼らない）

**アクセシビリティ**

- [ ] T-A11Y-1: マウスを使わずに、メイン画面 → 変更ページ（矢印キーで選択が動き、◉ で選択が分かる）→ キャンセルまで操作できる。フォーカス枠が常に見え、ページが変わると見出しにフォーカスがある
- [ ] T-A11Y-2: ナレーター（Ctrl+Win+Enter）で、キーボードの行が名前と状態とバッジの説明まで読まれる。「変更…」が「Keychron Receiver の配列を変更」と読まれる。ナビゲーションがタブとして読まれる
- [ ] T-A11Y-3: 結果のバナーが読み上げられる
- [ ] T-A11Y-4: 表示スケール 200%（1920×1080）とテキストのサイズ 150% で、文字が切れず、文字サイズが大きくなる。カウントダウン、再起動後の確認、衝突の解決のボタンが画面内に見えて押せる。std-widgets の文字の大きさを記録する
- [ ] T-A11Y-5 ★: ナレーターでカウントダウンを開く → 「あと 20 秒で自動的に元に戻ります…Tab で［このままにする］へ」が読まれる。Tab を 1 回押すと「このままにする」に移る。10 秒と 5 秒で知らせがある。設定で 60 秒にすると 60 秒になる（WP-E3 の後）
- [ ] T-A11Y-6: 特定で行が変わると「Keychron Receiver のキーが押されました」が読まれる

**設定と自動起動**

- [ ] T-SET-1: テーマと言語が再起動後も保たれる（`settings.toml`）
- [ ] T-AUTO-1: 自動起動をオフ → サインインで起動しない。オン → `--tray` で起動
- [ ] T-AUTO-2: タスク マネージャーのスタートアップで無効にする → 設定画面に「Windows のスタートアップ設定で無効」、MKLM は有効に戻さない
- [ ] T-SET-2 ★: 「アンインストール時に元に戻す」をオフ → UAC → 値が HKLM に保存され、画面に反映される。オンに戻す

**長時間**

- [ ] T-LONG-1: トレイに隠して数時間（スリープからの復帰を挟む）→ 表示して正しく描画される。タイトルバーのテーマが保たれる
- [ ] T-LONG-2: 常駐中のメモリ（ワーキング セット）と CPU 時間を記録する（目安 40 MB 以下、リリース ビルド。トレイにある間は CPU 時間がほぼ増えない）

**後始末**: `mklm-cli status --json --all` を取り直して `before.json` と比べる。Keychron が 4/0 であること。

---

## I. 作業の分担

型、トレイト、画面の骨組みはこのコミットで決まっている。依存の少ない順に並べた。W、E、C、G は互いに独立で、U は client（済）の上に並行して進められる。

| WP | 範囲 | 主なファイル | 依存 | 完了の条件 |
|---|---|---|---|---|
| WP-C1 | CLI の要求の全体を `run_once::run_request` に置き換える。`Frontend::notice` で CLI の 2 行を出す（`confirm_recovery` は既定のまま）。`PostRebootCommand::preferred`（隣の `mklm.exe` のビルド ID が同じなら GUI） | `apps/mklm-cli/src/write/commands.rs`、`relay.rs`、`crates/mklm-client/src/run_once.rs` | なし | CLI のテストがすべて通り、出力と終了コードが変わらない。重複した `finish` がなくなる。H.2 の実機の再確認（同意を得て） |
| WP-G | G.1（CLI の `journal` のローカル時刻）と G.2（CLI の理由の文言） | `apps/mklm-cli/src/write/journal_view.rs`、`render.rs` | なし | テスト。R4 と R5 の表示の再確認は任意 |
| WP-W1 | `instance`（ミューテックス、`InstanceServer`（1 秒の読み取り期限、名前を取られても起動を続ける）、`send_to_instance`（`ServerIdentity` の確認））、`pipe::PipeServer::accept_any`、`notify::KeyboardWatcher`、キーボード以外のコレクションの値の読み取り（ウィザード用） | `crates/mklm-win/src/instance.rs`、`pipe.rs`、`notify.rs`、`devices.rs` | なし | 同じプロセス内のパイプの往復テスト。別の実行ファイルのサーバーを拒むテスト。T-START-2、T-MAIN-1 |
| WP-W2 | `ui::copy_text_to_clipboard`、`ui::error_dialog` | `crates/mklm-win/src/ui/` | なし | T-APPLY の結果画面で「詳細をコピー」。`unsafe` にはすべて `// SAFETY:` |
| WP-W3 | `session::register_autostart` / `unregister_autostart` | `crates/mklm-win/src/session.rs` | なし | 検証関数のテスト（書き込みのテストは実機で） |
| WP-E1 | `CleanupValues`: core（`cleanup_candidates`、`OpKind::Cleanup`、schema 2）、engine、ipc、helper | 各クレート | なし（E2、E3 と `PROTOCOL_VERSION` の変更をそろえる） | H.1 の engine と ipc のテスト（クラッシュの網羅を含む）。縮退案は J.8 |
| WP-E2 | `SetMachineSettings`: engine（ロック、`RegistryBackend::write_machine_setting`）、`machine_settings::write_restore_on_uninstall`、ipc、helper | 各クレート | なし | テスト。T-SET-2 |
| WP-E3 | `ApplyOptions::countdown_seconds`（20 / 60）、helper のセッション終了への備え（`SetProcessShutdownParameters`、隠しウィンドウのスレッド、カウントダウンを戻す）（A.5） | core、ipc、engine、helper | なし（E1、E2 と `PROTOCOL_VERSION` 2 をそろえる） | H.1 のテスト。T-A11Y-5、T-END-1 |
| WP-U1 | メイン画面の完成: 非表示のメニュー、「非表示と未接続も表示」の保存、到着の通知と 750 ms のまとめ、バナーのボタン（今すぐ反映を含む）、`NeedsApply` の行と `apply_now` のインスタンス ID、止めている理由の文、特定の読み上げ、F5、行の強調のスクロール | `apps/mklm/src/vm/keyboards.rs`、`state.rs`、`app.rs`、`ui/screens/main.slint` | W1（到着の通知。なくても進められる） | スナップショット（日英）。T-MAIN、T-ID |
| WP-U2 | ウィザード（B.1）: 4 手順、入力方式、キーボードの配列と判定、問題のある値、まとめと `wizard_plan` の実行 | `vm/wizard.rs`、`ui/screens/wizard.slint` | U3（変更の流れ）、W1（キーボード以外の値）、E1（削除する） | スナップショット。T-START-5 |
| WP-U3 | 変更の流れ（B.3〜B.7、B.17、B.18）: 変更ページ（判定、選択肢、`PrepareChange`、切り替え方と既定値、UAC の 1 行と初回の画面）、セッション（進捗、カウントダウンの読み上げ、再接続、取り消し、回復の質問）、打鍵テストの期待値の配線、結果（今の状態、次の手順、詳細のコピー） | `vm/{change,session,result,keytest}.rs`、`state.rs`、`worker.rs`、`reader.rs`、`ui/screens/{change,session}.slint` | なし | `state::update` の流れのテスト（偽の helper で）、スナップショット。T-APPLY-1〜9、T-DETECT |
| WP-U4 | 再起動（B.8）と再起動後の確認（B.9。打鍵の列）、「後で決める」の RunOnce、`restart_pc`、最前面の表示 | `vm/{restart,post_reboot}.rs`、`ui/screens/restart.slint` | U3 | テスト。T-POST-1〜3 |
| WP-U5 | 衝突（B.10。キーボードごと、おすすめ）、履歴（B.11。ローカル時刻、状態の文）、起動時の確認と回復（B.12。`decide_recovery` の予測）、undo | `vm/{conflict,journal,recovery}.rs`、`ui/screens/other.slint` | U3 | テスト。R6 相当の衝突を実機で解決できる（同意を得て） |
| WP-U6 | プロセス（F 章）: 多重起動、閉じる・終了・セッション終了（F.5 の表）、初回の通知、自動起動の修復、設定の保存と `.bad`、ログ、起動時のエラー表示、終了前の I/O の待ち | `app.rs`、`single_instance.rs`、`autostart.rs`、`tray.rs`、`settings.rs`、新しい `log.rs` | W1、W2、W3 | T-START、T-TRAY、T-AUTO、T-SET-1、T-END-1 |
| WP-U7 | 設定画面（B.14。60 秒の設定を含む）、このアプリについて（B.15。ライセンス）、入力方式の案内（B.13）、実行ファイルのアイコン、アクセシビリティ（E 章）とハイコントラスト（C.2）の仕上げ、std-widgets と文字の倍率（J.3）、オーバーレイ中のページの無効化 | `ui/screens/other.slint`、`widgets.slint`、`res/` | E2（アンインストール時の設定）、E3（60 秒） | T-THEME、T-LANG、T-A11Y |

**レビューで確かめること**

- GUI が HKLM に書く経路がない。HKCU に書くのは `mklm_win::session` だけ。`ShellExecuteW` に渡すのは `SettingsPage` の定数と、System32 から作った絶対パスだけ。ほかに `ShellExecuteExW` に渡すのは、所有者と DACL を確かめて固定した `Recovery` フォルダーのパスだけ（`Folder` クラスで。B.11）。昇格した GUI は RunOnce を登録しない（セッション終了時も。F.2）。
- UI スレッドで、A.4 の規則 1 に挙げた呼び出しをしていない（F.5 のセッション終了時の上限つきの処理を除く）。
- ワーカーが Slint の型に触れていない。ワーカーのメッセージにセッション ID が付いている。
- 決定は `SessionView::accepts` を通ったものだけが送られる（relay が保証する。GUI 側で迂回しない）。
- 利用者がボタンを押していない UAC がない（helper を失った後の回復も、質問に「今すぐ元に戻す」と答えたときだけ）。
- ログと設定にキーの内容やスキャン コードがない（`seen` と `physical` はインスタンス ID と、そこから分かった事実だけ）。
- 利用者向けの文言が `i18n.rs` と `.po` 以外にない（`vm` は `i18n` を呼ぶ）。日本語の出力のラテン文字は許可リストの語だけ（H.1）。
- 新しい `@tr` を足したら `mklm.po` も更新されている（テストが検出する）。
- 一覧の描画はモデルを作り直さない（`list_ops`）。

---

## J. リスクと未解決の問題

1. **初回の通知はバルーンを使わない**（B.16）: レビュー U13 で、Slint の内部（`SlintSystemTrayWindow`、アイコン ID 1）に頼るバルーンをやめ、最初の × で必ずウィンドウ内の通知を出すことにした。Slint の更新で壊れる部分はなくなった。トレイのメニューのダーク化などのために、トレイを自前の `Shell_NotifyIconW` 実装（`mklm-win`）に置き換える案は残る（M4 以降）。
2. **トレイのメニューが OS のダークに追従しない**（M0 #8 E）: 受け入れる。1 と同じく自前実装で解決できる。
3. **ハイコントラストと文字の倍率で std-widgets が追従しない**（C.2、E.4）: `Palette` の色は上書きできない。`default-font-size` を std-widgets が使うかは未確認。T-THEME-4 と T-A11Y-4 で確かめ、不足があれば M3 の中で Button、CheckBox、ComboBox を自前の部品に置き換える（WP-U7 の判断）。
4. **ソフトウェア描画の文字の質と速さ**: サブピクセルのアンチエイリアスがない。高 DPI の大きな画面での再描画の速さも未確認。T-LONG と T-A11Y-4 で確かめ、問題があれば femtovg を既定にする（メモリ増は受け入れる）。
5. **自前の部品の読み上げ**（E.2、E.3）: Slint 1.18 の AccessKit 経由の UI Automation で、`list-item`、`radio-button`（`Rectangle` に付けた役割）、live region（`accessible-live-region`）がナレーターにどう読まれるか未確認。T-A11Y-2、5、6 で確かめ、足りなければ `StandardListView` などの標準部品に寄せる。
6. **サインイン直後に前面に出られないことがある**（B.9）: フォアグラウンドのロックで、RunOnce から起動したウィンドウが前面に出ないことがある。タスク バーの点滅（`request_user_attention`）と常に手前を併用する。それでも気付かれない場合に備え、トレイのツールチップとバナーにも残す。
7. **`WM_INPUTLANGCHANGE` を直接受けられない**（A.4）: winit がウィンドウ プロシージャを持つため。見えている間だけの 500 ms のポーリングで代える。ウィンドウのサブクラス化（`SetWindowSubclass`）は winit との干渉を避けるため採らない。
8. **「削除する」の範囲**（A.5、WP-E1）: 読まれない値の削除だけでも、エンジン、ipc、ジャーナルの形式（schema 2）まで変わる。M3 の終盤で時間が足りなければ、WP-E1 を v1.x に回し、ウィザードは問題のある値の説明と手順の案内だけにする（その場合は計画 3.1 からの縮退として K 章に記録する）。
9. **キーボード以外のコレクションの値**（B.1）: 計画 1.5 で書き込みを禁じているので、MKLM は消さない。手順の案内だけで利用者が消す。案内の文面は WP-U2 でレビューする。
10. **`settings.toml` がローミングされる**: ドメイン環境では `%APPDATA%` が別の PC に移る。`seen`、`hidden`、`physical` はインスタンス ID なので、別の PC では何にも一致せず無害。テーマと言語は利用者の好みなので移ってよい。
11. **`mklm-client` はビルド ID の対象外**（A.1）: helper と関係しないため。client の変更で CLI と GUI の挙動がずれることはない（両方が同じ client でビルドされる）。
12. **ジャーナルの schema 2**（WP-E1）: `Cleanup` のエントリだけが 2 になる。M3 より前のビルドに戻すと、そのエントリは `NewerSchema` になり、書き込みが止まる（`mklm-cli` と GUI が「MKLM を更新してください」と出す）。ダウングレードは想定しない（m2 C.10）。
13. **GUI と CLI の同時使用**: 書き込みはロックで直列化され、後から来た方は `Busy`（UAC の前に `gate` で止まる場合もある）。表示は `Read` のたびに最新になる。
14. **Run と RunOnce の競合**（F.1）: サインイン時に 2 つの `mklm.exe` が同時に起動する。ミューテックスで 1 つになり、判断はジャーナルで行い、`activate` はページを変えないので、どちらが残っても再起動後の確認は出る。
15. **切り替わった言語の Slint の文言の更新**: `select_bundled_translation` の後、Slint は `@tr` を再評価する。トレイのメニューが作り直されるかは T-LANG-1 で確かめる。作り直されなければ、言語の変更時にトレイを作り直す。
16. **アプリ内のアンインストール**（計画 3.13）: M5 で扱う。M3 は設定の保存（WP-E2）まで。
17. **ドリフトの検知と通知、ヘルスチェック**（計画 3.11）: M4。M3 の `Read` と `startup::summarize` は、その土台になる。
18. **セッション終了時の戻し**（F.5、WP-E3）: helper がセッション終了で先に終了させられた場合、確定していない配列が次のサインイン画面で有効になりうる。WP-E3 で helper の終了順と自前の戻しを足すが、OS の終了の順序に頼るので完全ではない。T-END-1 で確かめ、結果を記録する。
19. **Slint のキー イベントの伝わり方**（E.1）: 打鍵テストの欄が受け取らない Esc が、祖先の `FocusScope`（`DialogFrame`）に届くことを前提にしている。T-APPLY-7 で確かめ、届かなければダイアログのボタンにフォーカスがあるときの Esc と、欄の中の Esc を個別に扱う。
20. **ポインターを「別の入力手段」に数えること**（B.5）: マウスの利用者はスクリーン キーボードで続けられるという m2 C9 の考え方による。タッチ操作は Raw Input に出ないので数えない（何も見ていないときは「再起動」が既定）。

---

## K. 計画・M2 設計からの変更点

| # | 計画・M2 | この設計 | 理由 |
|---|---|---|---|
| 1 | OS のテーマの補助に `AppsUseLightTheme` を `RegNotifyChangeKeyValue` で監視する（計画 3.7） | 隠しシェル ウィンドウの `WM_SETTINGCHANGE("ImmersiveColorSet")` を補助にする | M0 #8 で動作を確かめた方式。レジストリの監視スレッドが要らない |
| 2 | 入力方式の変化を `WM_INPUTLANGCHANGE` で拾う（計画 3.2） | 見えている間だけ 500 ms ごとに `GetKeyboardLayout(0)` を読む | winit がウィンドウを持ち、そのメッセージを渡さないため（J.7） |
| 3 | 初めてトレイに入ったら通知を出す（計画 3.9） | 最初の × でウィンドウ内の通知を出し、「OK」で隠す。バルーンは使わない | Slint に通知の API がない。Windows 11 は通知を抑えても API が成功し、新しいアイコンを「^」に隠すので、バルーンでは伝わらないことがある（U13） |
| 4 | 既存の値を「取り込む / 削除する / そのまま」で選び、結果を baseline にする（計画 3.1） | 問題のある値だけを出し、「そのまま（おすすめ）/ 削除する（ドライバーが読まない値のときだけ）」から選ぶ。問題のない値は選ばせない。キーボード以外のコレクションは案内だけ | baseline の仕組み（m2 C.6）で目的を果たせる。書かない選択肢が 2 つあると区別できない（U3）。計画 1.5 の許可リストを守る |
| 5 | 適用範囲「このデバイスのみ / 同じ機種ならどのポートでも」（計画 3.4） | M3 は「このデバイスのみ」だけを示す | 後者はサービスの判断とともに M4（m2 0.1） |
| 6 | 回復が必要なら、呼び出し元が自動で helper を起動する（1 回の起動につき 1 回まで、m2 D.7。helper を失った直後も、m2 E.7） | GUI は、利用者がボタンを押したときにだけ UAC を出す。回復の画面は 1 回の起動につき 1 回まで自動で出す。helper を失った直後は、オーバーレイで理由を示して尋ね（`Frontend::confirm_recovery`）、「今すぐ元に戻す」のときだけ起動する。終了を選んだ後は尋ねない。CLI は m2 のまま | 理由の分からない UAC を出さない（R12 の教訓、原則 6。A1） |
| 7 | CLI から操作したら CLI を登録する。M3 では GUI がインストールされていれば GUI（m2 F.4） | GUI は GUI を登録する。CLI は隣の `mklm.exe` のビルド ID が同じなら GUI を登録する（WP-C1） | 版の違う GUI を起動しないため |
| 8 | 呼び出し元のコードは CLI にある（M2） | `crates/mklm-client` に抽出し、CLI と GUI で共有する | 安全網（C8、C17、S6）を 2 か所に書かないため |
| 9 | ワークスペースの構成に client がない（計画 2.4） | `crates/mklm-client` を追加。GUI はライブラリ（`mklm_gui`）と薄いバイナリ | 同上。ライブラリにすると、ウィンドウなしでテストでき、未配線の公開 API が警告にならない |
| 10 | `tracing` / `tracing-appender`（計画 2.5） | M3 は依存のない小さなログ | 依存を増やさない。M5 で検討する |
| 11 | HKLM への書き込みは `regwrite` と `journal_store` だけ（m2 K） | `machine_settings` を加える | 「アンインストール時に元に戻す」の保存（計画 3.13）のため。helper だけが呼ぶ |
| 12 | `PROTOCOL_VERSION` 1 | 2（`SetMachineSettings`、`CleanupValues`、`ApplyOptions::countdown_seconds`） | 要求と選択肢の追加 |
| 13 | ジャーナルの版 1（m2 C.10） | `Cleanup` のエントリだけ `schema_version = 2` | 古いビルドが形式の誤りではなく「新しい版」として扱えるように |
| 14 | 多重起動のパイプの DACL に利用者の SID（計画 2.2） | 計画どおり。加えて、1 接続 1 コマンド、16 バイトまで、1 秒の期限、データを運ばない。2 つ目はサーバーのセッション、利用者、実行ファイルを確かめてから送る。名前を取られても起動を続ける | 名前は秘密ではなく、先に作られうるため（A6） |
| 15 | ウィザードは「入力方式 → 既存の設定の取り込み → モードの説明と移行」（計画 3.1） | 「ようこそ → 入力方式 → キーボードの配列 → まとめ」の 4 手順。標準配列は固定モードの移行のときだけ選ばせる。移行は選んだ割り当てを必ず含む | 移行に割り当てがないと、再起動しても何も変わらない（U3）。計画 1.3 の「割り当てを同じトランザクションで書く」に合わせた |
| 16 | カウントダウンは 15〜20 秒（計画 3.5） | 既定 20 秒。設定で 60 秒も選べる（helper は 20 と 60 だけを受け付ける。WP-E3） | 読み上げを使う利用者が、聞いて、打鍵し、Tab で移って押すには 20 秒では足りないことがある（U10）。既定は計画どおり |
| 17 | 適用内容の確認画面 → UAC の事前説明の画面 → UAC（計画 3.5 の 1、2） | 割り当て、確認、切り替え方を 1 ページにまとめ、UAC の説明は初回だけ独立した画面、以後はボタンの横の 1〜2 行 | 同じ説明を毎回通らせると読まずに進む癖がつく（R12 の教訓の逆効果。U14）。説明が UAC より前にある点は変わらない |
| 18 | 「別の入力手段」の申告（m2 C9 のチェック） | 「すぐに切り替えて試す / PC の再起動で切り替える」の 2 択。既定値にポインターの使用も数える | チェックでは結果が分からず、マウスの利用者が再起動の経路に入り、以後の変更がすべて止まっていた（U1） |
| 19 | バッジ「未確認」（計画 3.2） | 「キー入力なし」。内蔵キーボードには付けない。「動作未検証」はバッジにしない。「要確認」は「設定に問題」 | 似た語が別の意味で並び、自分の PC の内蔵キーボードが「未確認」になっていた（U17） |
| 20 | 取り消しは「`Planned` の前なら中継をやめる」（この設計の初版） | `Planned` を送る要求（`plans_first`）だけ。ほかは結果を待つ | `Revert`、`Undo` などは `Planned` なしで書き込みとリセットをするため（A4） |
| 21 | セッション終了では、パイプが切れて helper が戻す（この設計の初版、R16 の解釈） | `WM_QUERYENDSESSION` で取り消しと RunOnce の登録、`WM_ENDSESSION` で最大 3 秒待つ。helper にもセッション終了への備えを足す（WP-E3）。最後の砦は次回起動時の回復と明記 | セッション終了では helper も終了させられうる（A5） |

---

## レビュー対応

M3 設計の初版と骨組みに対する 2 つのレビュー（U: 日本語の利用者の UX、アクセシビリティ、多言語。A: アーキテクチャ、スレッド、セキュリティ）への対応。「採用」は指摘どおり、「一部採用」は目的を採り方法を変えたもの。

| # | 指摘（要約） | 対応 | 変更箇所・理由 |
|---|---|---|---|
| U1（重大） | 「別の入力手段」の既定値がキーだけを見るので、マウスの利用者は再起動の経路に入り、以後の変更がすべて止まる。根拠のない「唯一のキーボード」の警告も出る | 採用 | チェックを結果の分かる 2 択（`ApplyMethod`）にした。既定は `vm::change::default_apply_method`（ほかのキーボードかポインター → すぐに、対象だけ → 再起動と警告、何もなし → 再起動で警告なし。テスト済み）。`input_capture` がポインターを 5 秒に 1 回まで知らせる。`takes_effect(RestartPc)` に「再起動するまで、MKLM でほかの変更はできません」。回復、元に戻す、undo でも同じ 2 択（B.5、K.18） |
| U2 | 打鍵テストの判定が、期待する配列、入力元、入力方式を見ずに緑の成功を出す | 採用 | `vm::keytest::key_pressed` に `KeyContext`（入力元、HKL、期待する配列と対象）を足し、一致は成功、不一致は危険と「元に戻す」の推奨、対象外のキーボードは案内、日本語以外の入力方式は判定しない（テスト済み）。再起動後の確認に「打鍵」の列と、⚠ のときの警告（B.6、B.9） |
| U3 | 固定モードの移行が割り当てを運ばず、再起動しても何も変わらない。キーボードごとモードの標準配列の選択は無意味。既存の値の 5 択が区別できない | 採用 | ウィザードを 4 手順にし、手順 3 でキーボードごとに JIS / US を選ぶ。`vm::wizard::wizard_plan` が割り当てを含む `Migrate` を作る（テスト済み）。標準配列は固定モードの移行のときだけ。既存の値は問題のあるものだけ「そのまま / 削除する」（B.1、K.4、K.15） |
| U4 | 「✓」が「保存値と Raw Input が合う」の意味で、固定モードの US キーボードも緑の ✓ になる | 採用 | ✓ は明示的に割り当てた配列が効いているときだけ（「✓ 設定どおり」）。標準に従う / 固定モードは中立。実物の配列を判定と JIS 専用のキーから学び（`settings.keyboards.physical`）、違えば「⚠ 実物は US 配列ですが JIS として動いています」。固定モードの説明を状態行に（B.2、テスト済み） |
| U5 | 決める画面に「Raw Input」、16 進、値の名前、KLID、「helper」、「中断?」が出る。衝突で残した結果が「変更は残っていません」になる | 採用 | 「Windows の認識: … は JIS 配列です」、言語名、「MKLM の管理用プログラム（mklm-helper.exe）」、`ConcurrentChange` の新しい文、`entry_state`（「途中で止まりました（回復が必要）」「MKLM 以外の値を残しました」）。数値は技術的な詳細へ。日本語の出力のラテン文字を許可リストで検査するテスト（`vm::unexpected_latin`）を足した（D.5、H.1） |
| U6 | 衝突の画面が値ごとの数値と 4 択で、組を壊せ、おすすめもない | 採用 | キーボードごと、配列名の選択肢（重複を除く）、`recommend`（4/0・7/2 の外なら「変更前に戻す」）、組は同じ選択、数値と値ごとの上書きは詳細へ。用語をそろえた（B.10、`vm::conflict::choices` はテスト済み） |
| U7 | 多くのエラーに次の手順がない。隔離が「インストールし直し」で繰り返す。無応答の案内が標準ユーザーにできない。今の状態が分からない | 一部採用 | 結果の先頭に「今の状態」、コード別の次の手順、ウイルス対策の案内、`StartFailed(Option<u32>)`（225 / 226 / 1260）、E.8 の分類、無応答は「1 分待って再起動」、「詳細をコピー」。ただし「変更は元に戻されています」は、エラー コードからは断定できない（`Registry` でも `Conflict` に残りうる）ので、ジャーナルがそう言うときだけ書く（B.17、`i18n` 実装済み） |
| U8 | 「反映待ち」に操作がなく、利用者はどうすればよいか分からない | 一部採用 | バナーと行に「今すぐ反映…」（`Recover` と 2 択。m2 D.7 の 4 で `apply_pending` が消える）。文は用語「保存済み（反映待ち: …が必要）」を保ち、次の行に「抜き差しするか、［今すぐ反映…］を押してください」などを添えた（用語の表を崩さないため）。ボタンはジャーナルに `apply_pending` があるキーボードだけ（ジャーナルにない反映待ちは `Recover` で消えないため）（B.2、B.12） |
| U9 | 回復の画面で結果が予想できず、2 つのボタンの違いが分からない | 採用 | `decide_recovery` を非昇格で使い、項目ごとに回復の結果を文で示す（`vm::recovery`）。主ボタンは「回復する（おすすめ）」だけ。undo は確認待ちがあるときだけ、「確認待ちの変更をすべて元に戻す」に名前をそろえた。「後で」に代償を添えた（B.12） |
| U10 | カウントダウンの時間の制限が読み上げられず、Esc が打鍵テストに吸われ、20 秒は短く、ウィンドウが後ろだと打鍵できない | 採用 | 開いたときの 1 文（時間と残し方）を assertive で読み、欄の説明にも入れた。10 秒と 5 秒だけ polite。欄は Esc を受け取らず `DialogFrame::escape` で戻す。質問が開いたら前面に出して点滅。60 秒の設定（WP-E3、K.16）（B.6、E.3） |
| U11 | ダイアログのボタンが枠の外に出る。最小サイズが作業領域を超える。テキストのサイズに従わない | 採用 | 題 / スクロールする本文 / 常に見えるボタンの 3 部構成（`DialogBody`、`DialogButtons`）。最小 560 × 400、ナビゲーション 160px まで。`TextScaleFactor` を `Theme.font-scale` にし、文字と打鍵テストの欄をその倍数に（`read_text_scale_factor`、`TextScaleWatcher` 実装済み）。T-A11Y-4 を足した（E.4、J.3） |
| U12 | 特定の強調が色だけで、読み上げられず、ハイコントラストで読めず、Space でボタンが押される | 採用 | `CaptureArea` がフォーカスを取り、Tab と Esc 以外を吸収、Esc で終わる。「◀ いま押したキーボード」のバッジ、polite の読み上げ、ハイコントラストでは塗らずに 3px の枠（B.3、C.2） |
| U13 | バルーンは応答不可で見えないのに「出した」ことになる。アイコンは「^」に隠れる。「通知領域」「見守ります」が不正確 | 採用 | 最初の × は必ずウィンドウ内の通知、OK で隠す。`tray_notice`（バルーン）を削除。文と［タスク バーの設定を開く］（`SettingsPage::Taskbar`）。自動起動の文も「タスク バー（「^」の中）」（B.16、K.3、J.1） |
| U14 | 14 ページ、1 回の変更に 9 操作。毎回の UAC 説明は読まれなくなる。判定の画面が別で、最初のキーで固定される | 採用 | 割り当て、確認、UAC の説明を変更ページにまとめ、UAC の画面は初回だけ。判定は変更ページとウィザードの中で、対象のキーボードだけを数え、答えのキーでだけ固定、「やり直す」（`detect` 実装済み）。12 ページ、6 操作（B.0、B.3〜B.5、K.17） |
| U15 | 危険な操作の結果の説明が足りない（ファイルの保存、「後で」の代償、元に戻すのに再起動、パスワードの記号、リセットの意味） | 採用 | 再起動画面にファイルの保存と、パスワードの記号の警告と［サインイン オプションを開く］（`SettingsPage::SignInOptions`）。各「後で」に「決めるまで、ほかのキーボードの配列も変更できません」。「元に戻す（もう一度 PC の再起動が必要）」。`takes_effect(ResetKeyboard)` にキーボード本体の設定は変わらないこと（B.5、B.8、B.9） |
| U16 | 選択とフォーカスが同じ見た目で、矢印キーが効かない。ボタンの読み上げ名がない。無効の理由が読まれない。ページが変わるとフォーカスが消える。手順の位置が色だけ | 採用 | `RadioList`（◉ / ○、塗り、外側のフォーカス枠、矢印キー）。「Keychron Receiver の配列を変更」などの読み上げ名、コンボボックスの名前。行に変更できない理由と読み上げ。`PageHeader` がフォーカスを取る。「手順 3 / 4」（E.1、E.2） |
| U17 | 「未確認」「動作未検証」「要確認」が紛らわしく、内蔵キーボードも「未確認」 | 採用 | 「キー入力なし」（内蔵には付けない）、「動作未検証」は削除（標準に従うの説明にだけ）、「設定に問題」（B.2、K.19。テスト済み） |
| U18 | UAC の説明が Windows 11 の画面と合わず、標準ユーザーのパスワードの要求に触れていない | 採用 | 実際の表示（題、ファイル名、確認済みの発行元、ファイルの入手先）に合わせた文と画面の例、管理者のパスワードか PIN の説明。T-APPLY-1 で実物と照らす（B.5） |
| U19 | IME の案内が適用の前にしか出ず、設定の場所が書かれていない | 採用 | US を「このままにする」で終えた結果に案内とボタン。案内のページに Microsoft IME の設定の手順（B.13、B.17） |
| A1 | helper を失うと GUI が説明なしに 2 回目の UAC を出す。終了を選んだ後にも出る | 採用 | `Frontend::confirm_recovery`（既定 true で CLI は不変）。`Orchestrator::run` は `cancel_requested` なら起動せず、`confirm_recovery` が false なら起動しない（`RecoverySkip`）。GUI は `RecoveryConfirmDialog` で尋ねる。テスト 2 件（A.2.3、0.2 の 6、K.6） |
| A2 | UAC 待ちの間、状態が Idle のままで、2 つ目のセッションが 1 つ目を置き換え、終了が素通りする | 採用 | `SessionPhase::Launching { id }` を `StartSession` と同じ `update` で設定、Idle 以外は開始しない、メッセージにセッション ID、古い ID は捨てる、`start_session` は生きているワーカーを拒む、起動の失敗は `SessionEnded(NotLaunched)`。F.5 に「UAC 待ち」の列。テスト済み（A.4、B.18） |
| A3 | RunOnce の規則が I/O ワーカーのキューに積まれるだけで、終了に追い越される | 採用 | `Orchestrator::run_then` と `run_once::run_request` で、要求の直後に同じスレッドで適用し、結果を `SessionEnded` に入れる。CLI も WP-C1 で同じ関数を使う。終了前に I/O の保存を 1 秒まで待つ（A.2.3、F.5） |
| A4 | `Planned` を送らない要求を取り消すと、その場で中継をやめて結果が失われる | 採用 | `cancel_action(view, request)` と `plans_first`。`Planned` の前に抜けるのは記録を先にする要求だけ。`Revert` / `Undo` のテストを足した（A.2.2） |
| A5 | セッション終了時の安全網が過大に書かれている（helper も終了させられる）。取り消しを立てるのが遅く、`RefCell` の中にある | 採用 | `WM_QUERYENDSESSION` で `SessionShared::cancel`（静的な `Mutex` の中、`RefCell` の外）を直接立て、記録済みなら RunOnce を無条件に登録。`WM_ENDSESSION` で `Condvar` を最大 3 秒待つ（実装済み）。helper 側の追補を WP-E3 に。F.5 と 0.1 の文を直した（K.21、J.18） |
| A6 | 多重起動のパイプとミューテックスは名前を先に作られうる。「SID を含むので乗っ取れない」は誤り。読み取りの期限がない | 採用 | 2 つ目はサーバーのセッション、トークンの SID、実行ファイルのパスを確かめてから送る（`ServerIdentity`、`is_our_instance`。テスト済み）。名前を取られても 1 つ目として起動を続ける。1 秒の読み取り期限。M5 の送り手にも必須。`instance.rs` の説明を直した（F.1、K.14） |
| A7 | 打鍵のたびにモデルを作り直し、行のフォーカスが消える | 採用 | 一覧の `VecModel` を保持し、`vm::list_ops` の差分で更新（テスト済み）。`DeviceKey` は見えるものが変わったときだけ描画（テスト済み）。T-MAIN-4 を足した（A.6） |
| A8 | `activate` で `Navigate(Main)` すると、再起動後の確認や作業中のページを置き換える | 採用 | `Activate` は表示、前面化、`Read` だけ。ページは次の `SystemRead` で決める。`single_instance.rs` の説明を直した（F.1） |
| A9 | `control.exe` をパスなしで開くと、作業フォルダーの同名のファイルが実行されうる | 採用 | `GetSystemDirectoryW` から作った絶対パスと、作業フォルダー System32 で開く（`open_settings_page` 実装済み。A.5） |
| A10 | 入力方式のタイマーがトレイでも動き、非表示でも描画する | 採用 | `ShowWindow` / `HideWindow` でタイマーを開始・停止、非表示の間は描画しない（実装済み）。T-LONG-2 に CPU 時間（A.4） |
| A11 | ワーカーがパニックすると `SessionEnded` が届かず、終了できない | 採用 | ワーカーに終了を必ず送るガード（`EndGuard`）。パニックでも `Lost { "the session worker panicked" }` を送り、終わりの合図も立てる（実装済み。A.4） |
| A12 | M5 の更新で helper が GUI に `quit` を送ると、互いに待って止まる | 採用 | 更新の helper は元のセッションに専用の最後のメッセージを送って閉じ、GUI はそれで終了し、helper は GUI の PID の終了を待つ（計画 4.2 の 6 に合わせた）。`quit` は `--quit` 専用と明記（F.5） |

### 実装レビュー（段階 1 の統合後、b739746）への対応

エンジン、GUI のアーキテクチャ、利用者向けの品質、unsafe と Win32、書き込みなしの通し確認の 5 つの観点のレビュー（指摘 36 件、重複を含む）への対応。重複した指摘はまとめた。

| 指摘（要約） | 対応 | 変更箇所・理由 |
|---|---|---|
| 読まれない値だけを戻す「導入前に戻す」が `apply_pending` を残す（D.11 に反する） | 採用 | エンジンの `close_pending` が、`apply_pending_on_close` に渡す前に読まれない値の記録を除く（種類によらない。回復で閉じる場合も）。m2 D.11 に追記。テスト（未接続のキーボード、クラッシュの網羅） |
| helper のセッション終了が、カウントダウン前の書き込み・リセット中を待たない | 採用 | `SessionEnd` に `InFlight`（`Planned` か書き込み中の状態から、`CountdownStarted`、再接続待ち、終わりの状態まで）。m2 A.6 の文を「新しい操作の書き込みも、変更を反映するためのリセットも始めない（戻すときの反映し直しは行う）」に直した |
| 届いている「このままにする」より、セッション終了の `RevertNow` が先になる | 採用 | 戻す前に内側のシンクを 1 ms だけ読み、届いている答えを先に使う（`answer_or_revert`）。テスト |
| schema 1 の固定データが 9 件中 2 件だけ | 採用 | 開発機の 9 件をすべて値のまま写し、全件の往復、回復で閉じた記録（`recover:roll-back`）、`CallerDisconnected` を確かめる |
| 昇格した GUI がセッション終了時に RunOnce を登録する | 採用 | 起動時の昇格の状態を静的な値に置き、昇格していれば登録しない（F.2、F.5） |
| 多重起動のパイプを作るのが遅い | 採用 | Slint のバックエンドを選んだ直後に受け付けを始め、返事の待ちを 4 秒にした（F.1） |
| 前のワーカーが残っていると `Launching` のまま戻らない | 採用 | その場合も `SessionEnded(NotLaunched)` を送る（A2） |
| 「復旧用ファイルのフォルダーを開く」が所有者と DACL を確かめない | 採用 | `protected_dir::verify_protected_dir`（読み取りだけの検証）、ハンドルで固定、`ShellExecuteExW` の `Folder` クラス。レビューの規則に 3 つ目として追記（A.5、I、B.11） |
| `list_ops` が重複したキーでパニックする | 採用 | 未配置の行だけから移す。衝突の行は対象（インスタンス ID）をキーにする。網羅テスト |
| I/O ワーカーのパニックで要求元が待ち続ける | 採用 | タスクごとに `catch_unwind`、要求元に失敗を返す、書き込みの数え上げはガードで戻す |
| ラジオの一覧が描画のたびに新しいモデルになる | 採用 | `models::KeptModel` で保持して行ごとに更新（変更、ウィザード、衝突、回復の切り替え方も）。Slint 側の配列リテラルもやめた |
| 画面の本文が縦に引き伸ばされる | 採用 | `DialogBody` を `alignment: start` に。ウィザードのコンボボックスの幅をそろえた |
| 状態行が折り返さず、最小サイズで崩れる | 採用 | 状態行を 4 列（項目名の下に値、値は折り返す）、チェックボックスを別の行、キーボードの行の中央の列を縮められるようにした（B.2） |
| 結果の文が区切りなしでつながる | 採用 | 部品ごとに 1 行（B.12） |
| カウントダウンの Tab の案内と実際の順が逆 | 採用 | 「このままにする」をフォーカスの順で先に、見た目は `layout-order` で保つ（B.6） |
| ウィザードの手順が変わってもフォーカスが見出しに移らない | 採用 | 手順の変化で `PageHeader::focus-heading`。変更ページが導入前に戻す画面に変わるときも同じ |
| 「今すぐ反映…」の結果が「回復しました。」 | 採用 | `AppState::applying_now` から「反映しました。」 |
| 「確定」が画面に残る、括弧の混在、「。」の欠け | 採用 | 「このままにしました」「［このままにする］が選ばれる前に…」。ボタン名は［］、引用は「」、『』は使わない。D.5 に表記の規則。`i18n::tests::japanese_notation` |
| ラジオの一覧の見出しが画面に出ない | 採用 | `label` を一覧の上に表示 |
| 準備の失敗の次の手順が、そのページにないボタンを指す | 採用 | 変更ページとウィザードで別の手順（`PreparePlace`） |
| 英語の単複、reports の重複、残り時間の知らせ | 採用 | 1 件は単数、`arrival` は "{name}: {kind}"、"Reverting in 10 seconds" |
| 箇条書きの記号が英語でも「・」 | 採用 | `@tr("• {}")`、日本語は「・{}」 |
| 最後の手順にも「後でセットアップする」 | 採用 | 「完了」の画面では出さない（B.1） |
| 同じ名前のボタンの読み上げ名 | 採用 | 回復の項目の「このままにする」「元に戻す…」、行の「今すぐ反映…」に操作を含む名前 |
| ジャーナルのページからの初回の UAC に説明の画面がない | 一部採用 | 初回は独立した説明の画面を通す（B.5 と同じ）。ボタンの文に「（次に Windows の確認が出ます）」は足さない。ボタンの横の 1 行（`uac_line`）が既にあり、再起動後の確認のように要求のボタンが 2 つ並ぶページでは、最小幅に収まらなくなるため |
| 自動起動の失敗に次の手順がない | 採用 | 「サインイン時の起動」に語をそろえ、次の手順を添えた |
| 許可リストの「helper」で、単独の helper を検出できない | 採用 | `mklm-helper.exe` をファイル名として除いてから検査。「OK」（Windows のダイアログのボタン）を許可 |
| 昇格した GUI に `TaskbarCreated` が届かない（UIPI） | 採用 | `ChangeWindowMessageFilterEx` で通す。T-TRAY-2 に昇格時を追加 |
| `WM_ENDSESSION` の待ちが 3 秒 + 1 秒 | 採用 | 合わせて 3 秒（残りの時間で I/O を待つ。A.4、F.5） |
| マニフェストの注釈が TaskDialogIndirect | 採用 | MessageBoxW に直した |
