# Slint 試作（M0 #8）

プランの M0 #8 のための、使い捨ての Slint 1.18 試作です。M3 の GUI を作る前に、次の 5 点が Slint と winit で実現できるかを確かめます。

1. テーマ（ライト / ダーク / システム）。OS のテーマの判定はアプリ側で行い、タイトルバーも合わせる
2. 日本語 IME での入力（`LineEdit`）
3. IME を通さない打鍵テスト欄（`FocusScope`）。Shift+2 で `@` なら US、`"` なら JIS
4. 「どのキーボードで打ったか」の特定（winit の `device_event` と `persistent_identifier()`）
5. トレイ（`SystemTrayIcon`）。閉じるとトレイに入り、「終了」で終わる

システムには何も書き込みません。レジストリは `AppsUseLightTheme` を読むだけです。デバイスの再起動もしません。

このクレートはルートのワークスペースに含めていません（ルートの `Cargo.toml` は `exclude = ["prototypes"]`、このクレートは空の `[workspace]` を持ちます）。ビルド結果は `prototypes/slint-proto/target/` に出ます。

## 実行方法

```powershell
cd "E:\GitHub Repository\multi-keyboard-layout-manager\prototypes\slint-proto"
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path

cargo run            # デバッグ版。コンソールが付く
cargo run --release  # リリース版。コンソールなし（初回のビルドは 10 分ほどかかる）
```

ビルド済みの exe を直接起動してもかまいません: `target\release\slint-proto.exe`

| 引数・環境変数 | 用途 |
|---|---|
| `--theme=light` / `dark` / `system` | 起動時のテーマ。既定は `system` |
| `--no-with-theme` | 検証用。ウィンドウを `with_theme` なしで作り、winit の不具合を再現する（後述） |
| `--list-keyboards` | GUI を出さずに、Raw Input のキーボード一覧（パス、インスタンス ID、type/subtype）を表示して終わる。デバッグ版で使う |
| `--exit-after=秒` | 指定した秒数で通常の終了処理を行い、イベントログを標準出力に出す（動作確認用） |
| `SLINT_BACKEND=winit-software` | ソフトウェア レンダラーで起動する（既定は femtovg / OpenGL） |

## 手動テストのチェックリスト

画面の「6. イベントログ」に、テーマの適用、`WM_SETTINGCHANGE`、`TaskbarCreated`、表示と非表示の記録が出ます。結果を報告するときは、このログも添えてください。

### A. 日本語 IME

- [ ] 「2. 日本語 IME 入力」の欄をクリックし、IME をオンにして「にほんご」と打って変換・確定できる
- [ ] 変換中の文字（下線付き）と候補ウィンドウが、入力欄の近くに表示される
- [ ] `半角/全角` キー（JIS）と `` Alt+` ``（US）で IME のオン・オフを切り替えられる
- [ ] Backspace、矢印キー、Ctrl+A / C / V が普通に動く

### B. 打鍵テストとキーボードの特定

「3. 打鍵テスト」の枠をクリックしてから（枠が太くなります）、各キーボードで Shift+2 を押します。

- [ ] 内蔵キーボードで Shift+2 → 大きく `"` と表示され、「JIS 配列として動作しています」と出る
- [ ] Keychron で Shift+2 → `@` と表示され、「US 配列として動作しています」と出る
- [ ] 「このキーを送ったキーボード」が、押したキーボードのインスタンス ID になっている
  - 内蔵: `ACPI\FUJ0309\4&320DB4C2&0`
  - Keychron: `HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000`
- [ ] 「4. どのキーボード？」のパスが `\\?\HID#VID_3434&PID_D027&MI_00&Col01#...` の形になり、Raw Input の type/subtype が内蔵 = 0x7/0x2、Keychron = 0x4/0x0 になっている
- [ ] キーボードの一覧に 2 台が並び、それぞれの「最後の文字」が `"` と `@` になっている
- [ ] 「離したときの文字」が、押したときの文字と同じになっている（違う場合は記録してください。後述の 18 番）
- [ ] 打鍵テスト欄では、IME がオンの状態でも変換されずに文字がそのまま出る
- [ ] ほかのアプリを前面にしてキーを打っても、この画面の表示は変わらない（前面のときだけ受け取る）

### C. テーマ

- [ ] 「ライト」「ダーク」「システム」を切り替えると、画面の色とタイトルバーの色が両方とも変わる
- [ ] 「現在のテーマ」が、選んだモードと OS の設定から正しく決まっている
- [ ] 「システム」を選んだまま、Windows の設定（個人用設定 → 色 → アプリ モード）でライト / ダークを切り替えると、画面とタイトルバーが追従する。ログに `ColorValuesChanged` または `WM_SETTINGCHANGE(ImmersiveColorSet)` が出る
- [ ] 「ライト」固定のまま OS をダークにしても（逆も）、画面もタイトルバーも変わらない

### D. 無関係な設定変更の後もタイトルバーが保たれるか

OS のテーマと逆のテーマに固定したときが、最も崩れやすい条件です。

1. OS がダークなら「ライト」、OS がライトなら「ダーク」を選ぶ。
2. `Win+R` → `SystemPropertiesAdvanced` → 「環境変数」→ 何も変えずに「OK」を押す（`WM_SETTINGCHANGE "Environment"` が全ウィンドウに送られる）。
3. 次を確認する。
   - [ ] ログに `WM_SETTINGCHANGE "Environment"` が出る
   - [ ] タイトルバーの色が変わらない
4. 参考: `--no-with-theme` を付けて起動し、同じ手順を行うと、タイトルバーが OS のテーマに戻ってしまう（winit の不具合の再現）。画面上のチェックボックス「WM_SETTINGCHANGE を受けるたびに DWM 属性を再適用する」をオンにすると、その場で直ることも確かめられる。

### E. トレイ

- [ ] 起動するとトレイ（通知領域。隠れている場合は `^` の中）にキーボードのアイコンが出る。ツールチップは「MKLM 試作」
- [ ] ウィンドウの × で閉じると、ウィンドウが消えてトレイにアイコンが残る
- [ ] トレイのアイコンを左クリックすると、ウィンドウが前面に戻る
- [ ] 右クリックのメニューに「表示」「終了」があり、どちらも動く。「終了」でトレイのアイコンも消える（画面の「終了」ボタンでも同じ）
- [ ] OS がダークのとき、右クリックのメニューの色を記録する（ライトのままになる見込み。後述の 7 番）

### F. explorer.exe の再起動

1. タスク マネージャー → 「エクスプローラー」を右クリック → 「再起動」。
2. 次を確認する。
   - [ ] ログに「TaskbarCreated 受信 → トレイ アイコンを作り直しました」が出る
   - [ ] トレイのアイコンが戻っている。メニューの「表示」「終了」も動く
3. 「explorer の再起動でトレイ アイコンを作り直す（回避策）」のチェックを外して、もう一度 explorer を再起動する。
   - [ ] アイコンが消えたままになるかを記録する（Slint 標準の動作。後述の 6 番のとおりなら消える）
   - アイコンが消えると、隠したウィンドウを戻せなくなります。ウィンドウを表示したまま試し、最後は画面の「終了」ボタンで終わってください。隠してしまった場合は、タスク マネージャーで `slint-proto.exe` を終了してください。

### G. 長時間の非表示

1. 「トレイに隠す」または × で隠し、数時間そのままにする（可能ならスリープからの復帰も挟む）。
2. トレイから表示し、次を確認する。
   - [ ] ログに「非表示だった時間 ○時間○分○秒」が出る
   - [ ] 画面が正しく描画される（真っ黒、真っ白、古い内容のまま、文字化けがない）
   - [ ] タイトルバーのテーマが保たれている
   - [ ] IME 入力と打鍵テストがそのまま使える
3. 描画が崩れた場合は、`SLINT_BACKEND=winit-software` で同じ手順を試し、違いを記録する。

## 自動で確認したこと（2026-09-27、この PC）

- デバッグ版・リリース版とも起動し、10 秒以上動き続けて、`--exit-after` で正常に終了した（終了コード 0）。標準エラーへの出力はなかった。
- 起動からウィンドウ表示まで: デバッグ版で約 3 秒、リリース版で約 1 秒。
- メモリ（ワーキング セット）: femtovg（既定）で約 127 MB、`winit-software` で約 28 MB（リリース版）。
- `--list-keyboards` の結果は M0 の記録と一致した。
  - Keychron: `HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000`、type=0x4 / subtype=0x0
  - VXE R1SE+ の COL02: type=0x51 / subtype=0x0
  - 内蔵: `ACPI\FUJ0309\4&320DB4C2&0`、type=0x7 / subtype=0x2
- タイトルバー: 試作のウィンドウに直接 `WM_SETTINGCHANGE "Environment"` を送り、`DwmGetWindowAttribute(20)` で確かめた（OS はダーク）。
  - `--theme=light`（`with_theme` あり）: 0 → 0。保たれる。
  - `--theme=light --no-with-theme`: 0 → **1**。OS のテーマに戻ってしまう。
- Slint のトレイ用ウィンドウ（クラス `SlintSystemTrayWindow`）の親は `Message` クラスで、メッセージ専用ウィンドウであることを確かめた。`EnumWindows` にも出てこない。
- `TaskbarCreated` を監視ウィンドウに直接送ると、トレイのアイコンを作り直す処理が動いた。実際の explorer の再起動は手動で確認する（F）。
- キー入力（B）は、実機のキーボードで押す必要があるため未確認。

## 分かったこと・プランとの違い

### API の名前と使い方（slint 1.18.1 / winit 0.30.13）

1. **フックとハンドラーは、プランの名前どおりにある。** `slint::BackendSelector::with_winit_window_attributes_hook()` と `with_winit_custom_application_handler()` を使う。後者は `Box` ではなく値を受け取る。トレイトなどは `slint::winit_030::{CustomApplicationHandler, EventResult, WinitWindowAccessor, winit}` にある。`device_event(&mut self, &ActiveEventLoop, DeviceId, DeviceEvent) -> EventResult` はウィンドウより前に呼ばれる。
2. **`persistent_identifier()` もある。** `winit::platform::windows::DeviceIdExtWindows::persistent_identifier()` が、`GetRawInputDeviceInfoW(RIDI_DEVICENAME)` の値をそのまま返す（例: `\\?\HID#VID_3434&PID_D027&MI_00&Col01#8&148ad7e3&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}`）。呼ぶたびに OS に問い合わせるので、`DeviceId` ごとにキャッシュし、`DeviceEvent::Added` / `Removed` で破棄する。
3. **`WindowEvent::KeyboardInput` の `device_id` は Windows では常にダミー（0）で、`persistent_identifier()` は `None` になる。** キーボードの特定には、`DeviceEvent::Key`（Raw Input）を使うしかない。Slint のキー イベントとの対応は「直前の `DeviceEvent::Key`」で取る（`WM_INPUT` は `WM_KEYDOWN` より先に処理される見込み）。手動テスト B で、対応が正しいかを確かめる。
4. **Raw Input は前面にあるときだけ届く。** winit の既定は `DeviceEvents::WhenFocused`（`RIDEV_DEVNOTIFY` あり、`RIDEV_INPUTSINK` なし）。プランの「前面にあるときだけ受け取る」と一致するので、MKLM が独自に登録する必要はない。変える場合は `ActiveEventLoop::listen_device_events()` を使う。
5. **インスタンス ID は正規化が必要。** インターフェイス パスから `CM_Get_Device_Interface_PropertyW(DEVPKEY_Device_InstanceId)` で取ると、パスの大文字・小文字のまま（`Col01`、`8&148ad7e3...`）返る。`CM_Locate_DevNodeW` と `CM_Get_Device_IDW` を通すと、PnP の正規の形（`COL01`、`8&148AD7E3...`）になる。M1 以降、インスタンス ID は正規化するか、大文字・小文字を区別せずに比べる。

### トレイ（`SystemTrayIcon`）

6. **explorer の再起動でアイコンが消える見込み（Slint 1.18.1 の不具合）。** Slint の Windows 実装は `TaskbarCreated` を受けてアイコンを追加し直すコードを持っている。しかし、受け手のウィンドウをメッセージ専用ウィンドウ（`HWND_MESSAGE`）として作っており、メッセージ専用ウィンドウにはブロードキャストが届かない。**回避策**: アプリが自前で、表示しないトップレベル ウィンドウを作って `TaskbarCreated` を受け、`TrayIcon` のコンポーネントを作り直す（`src/watcher.rs`）。手動テスト F で確定させる。
7. **トレイのメニューはダークにならない見込み。** `TrackPopupMenu` による Win32 のメニューで、winit も Slint も `SetPreferredAppMode` を呼ばないため。気になる場合は、非公開 API を使うか、tray-icon / muda クレートなどに替える。
8. **トレイの通知（バルーン）を出す API がない。** プラン 3.9 の「初めてトレイに入ったときに一度だけ通知」は、Slint だけでは実現できない。自前の `Shell_NotifyIcon(NIF_INFO)` かトースト通知が必要になる。
9. **使い方の注意。**
   - `export component X inherits SystemTrayIcon` と書く、トップレベルのコンポーネントにする。子は `Menu` 1 つだけ。メニュー項目のショートカットは無視される。
   - 組み込みの `clicked` コールバックは Rust から見えない。公式ドキュメントの `tray.on_clicked(...)` はコンパイルできない。自分で `callback icon-clicked(); clicked => { root.icon-clicked(); }` のように中継する。
   - 組み込みの `icon` プロパティも Rust から直接は設定できない。`in property <image> tray-image; icon: tray-image;` のように中継する。
   - 表示中のトレイはイベント ループを生かし続けるが、ウィンドウを隠した状態でトレイを破棄すると、その場でループが終了する。作り直しのときに困るので、`slint::run_event_loop_until_quit()` を使い、「終了」で明示的に `quit_event_loop()` を呼ぶ。
   - Slint 1.18 では `system-tray` が既定の feature に含まれている。追加の feature 指定は要らない。
10. **閉じるとトレイに入る動作は簡単。** `window().on_close_requested()` で `CloseRequestResponse::HideWindow` を返す（既定値も同じ）。Windows では、隠しても winit のウィンドウ（HWND）は破棄されず `set_visible(false)` になるだけなので、DWM の属性も残る。

### テーマ

11. **winit 0.30 の `set_theme` は設定を保持しない（プランの記述どおり、実機で再現した）。** `set_theme` は `preferred_theme` を更新しないため、`with_theme` なしで作ったウィンドウは、無関係な `WM_SETTINGCHANGE` を受けると OS のテーマに戻る。`with_theme(Some(..))` で作れば、winit はそれ以降 `WM_SETTINGCHANGE` でテーマを変えない。
12. **winit の `WCA_USEDARKMODECOLORS` と `DWMWA_USE_IMMERSIVE_DARK_MODE` は、同じ状態を操作している。** winit が内部で呼ぶ処理（`SetWindowTheme` と `SetWindowCompositionAttribute`）によって、`DwmGetWindowAttribute(20)` の読み取り値も変わった。そのため、テーマを変えるときは、winit の `set_theme()`（`SetWindowTheme` と WCA を更新）と `DwmSetWindowAttribute(20)` の両方を呼び、食い違いを作らないようにした。
13. **属性フックが呼ばれるのは、`show()` ではなく `AppWindow::new()` のとき。** そのため、OS のテーマはコンポーネントを作る前に判定しておく必要がある。フックは、その後に作られるすべての Slint ウィンドウに適用される。
14. **winit のウィンドウは、イベント ループが始まってから作られる。** 初回の DWM 設定は、`slint::spawn_local` の中で `window().winit_window().await` を待ってから行う。得た `Arc<winit::window::Window>` を持ち続けると、隠したウィンドウが画面に残るので、すぐに手放す。
15. **`Palette.color-scheme` は Rust から直接は設定できない。** `.slint` 側に `public function apply-color-scheme(dark: bool)` を置き、その中で `Palette.color-scheme` に Dark / Light を代入する。一度代入すれば、Slint が内部で持つ（winit から得た、古いかもしれない）テーマの値の影響を受けない。
16. `UISettings` は `BackendSelector` より前に、明示的な COM の初期化なしで作れる（windows-rs が必要に応じて `CoIncrementMTAUsage` を呼ぶ）。`ColorValuesChanged` はスレッド プールで呼ばれるので、`slint::invoke_from_event_loop` で UI スレッドに戻す。

### 打鍵テスト

17. **押したときの文字は、キーボードごとの配列を正しく反映するはず。** `FocusScope` の `key-pressed` の `event.text` は、winit が `WM_CHAR` から得た文字。
18. **離したときの文字は、キーボードごとの配列を反映しないかもしれない。** 離したときは `WM_CHAR` がないため、winit は自前のレイアウト キャッシュ（`ToUnicodeEx`）で文字を求める。判定には `key-pressed` だけを使う。手動テスト B の「離したときの文字」で、実際に違いが出るかを確かめる。
19. `FocusScope` にフォーカスがあると、Slint は IME を無効にする。そのため、IME を通さないテスト欄として使える。

### その他

20. femtovg（OpenGL）では、ワーキング セットがソフトウェア レンダラーの約 4.5 倍になる（127 MB と 28 MB）。常駐するアプリなので、M3 では `renderer-software` を既定にすることも検討する価値がある。長時間の非表示の後やスリープからの復帰で、GL コンテキストの問題が出ないかも、G の結果で判断する。
21. `slint::include_modules!()` が生成するコードは、ワークスペースの `missing_debug_implementations` の警告に引っかかる。`#[allow(missing_debug_implementations)]` を付けたモジュールの中で展開する。
22. Slint が依存する `windows` クレートは 0.62.2 で、ワークスペースと同じ版なので、二重にならない。

## ファイル

| ファイル | 内容 |
|---|---|
| `ui/app.slint` | 画面（`AppWindow`）とトレイ（`TrayIcon`） |
| `src/main.rs` | 起動、テーマの解決と適用、打鍵テスト、`CustomApplicationHandler`、トレイ、ログ |
| `src/theme.rs` | `UISettings` による OS のテーマの判定、`ColorValuesChanged`、`DwmSetWindowAttribute` |
| `src/devices.rs` | インターフェイス パスからインスタンス ID と `RIDI_DEVICEINFO` を取得（読み取りのみ） |
| `src/watcher.rs` | `TaskbarCreated` と `WM_SETTINGCHANGE` を受ける、表示しないトップレベル ウィンドウ |
| `src/icon.rs` | 画像ファイルを使わずに描いた、ウィンドウとトレイのアイコン |
