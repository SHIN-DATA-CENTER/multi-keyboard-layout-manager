# リモート デスクトップと 英数キー（調査メモ）

- 調査日: 2026-09-29（E7 / E7b / E8 は未実施。結果が出たらこのメモに追記する）
- 対象: リモート デスクトップ（RDP）の接続先で、英数キーだけでは日本語入力が切り替わらず、Shift+英数 で切り替わる、という報告
- 読み取りだけで調べた。レジストリ、デバイス、IME の設定には何も書いていない。

## 1. 概要と結論

**原因**: 接続先の PC は、2026-09-29 02:03 まで Windows の「英語キーボード (101/102 キー)」の固定モードだった（全体の `OverrideKeyboardType=7`、`OverrideKeyboardSubtype=0`、`LayerDriver JPN=kbd101.dll`、`OverrideKeyboardIdentifier=PCAT_101KEY`）。利用者は 02:03 に MKLM でキーボードごとモード（標準配列 JIS）へ移行した（ジャーナルの操作 `c10d2d38`）。この移行は **PC の再起動待ち**で、まだ効いていない。PC は 2026-09-28 13:29 に起動し、セッション 1 はその直後にコンソールでサインインして作られ、13:58 以降は RDP で再接続して使われている。そのため、このセッションは **101 の表（kbd101）でキーを解釈している**。

101 の表では、スキャンコード 0x3A は単独で Caps Lock（`VK_CAPITAL`）、Shift 付きで 英数（`VK_DBE_ALPHANUMERIC`）になる。106/109（JIS）の表では逆で、単独が 英数、Shift 付きが Caps Lock。報告された症状は、101 の表の動作そのものだった。記号キーも US の位置になっている（Shift+2 で `@`、P の右のキーで `[`）。

**判定**: 仮説 H1 を採る（7 節）。Windows App と mstsc の両方で、セッション内の低レベル フックが同じ結果を示した（6.4 節）。

**直し方（見込み。E7 で確かめる）**: 保留中の再起動（「シャットダウン」ではなく「再起動」）をして、**新しくサインインする**。キー配列はセッションが始まったときに決まるので、再起動を待つ変更がリモート デスクトップに反映されるのは、早くても再起動して新しいセッションにサインインした後になる。H2（7 節。未判定）が正しければ、再起動しても 101 の表のまま。それまでの代わりのキーは 9 節。

**まだ分からないこと**: 再起動後の新しい RDP セッションがどの表で打つか。接続元の報告する種類（7/2 → `KeyboardType Mapping\JPN\00000002` = kbd106.dll）に従うのか、PC の標準配列に従うのか、kbdjpn.dll の既定の表（kbd101。H2）になるのか。E7 は JIS の接続元（7/2）から標準配列 JIS の PC に接続するので、「接続元に従う」と「標準配列に従う」はどちらも JIS になり、分かるのは JIS か 101 か（H1 か H2 か）だけ。接続元と標準配列のどちらに従うかは、7/0 か 4/0 を報告する接続元から接続する E7b で分ける（8 節）。案内の文言は、これら 3 つのどの結果でも正しい書き方にしてある。

## 2. 環境

| | 内容 |
|---|---|
| 接続先の PC | `DESKTOP-3TCSIET`。Windows 11 Pro 25H2（build 26200.9457、x64）、ja-JP。M0 の開発機（Fujitsu のノート PC、Home）とは**別の PC** |
| 接続先のキーボード | 標準 PS/2 キーボード（`ACPI\PNP0303\0`。移行で 7/2 に固定済み）、USB キーボード 04D9:1818、2.4G Wireless Device 1D57:FA60（2 コレクション）、VXE Mouse 1K Dongle 3554:F58E |
| 接続元の PC | `DESKTOP-6FDOQLK`。build 26200、JIS キーボード。Windows App 2.0.1375（MSRDC のコア `rdclientax.dll`）と mstsc の両方で試した |
| セッション | セッション 1。2026-09-28 13:29 にコンソールでサインイン（LocalSessionManager のイベント 21 / 22、送信元「ローカル」）。13:58 から RDP で接続（RemoteConnectionManager 1149、以後は再接続 25） |
| PC の起動 | 2026-09-28 13:29:32 |
| MKLM | インストール版 0.1.0。操作 `c10d2d38`（2026-09-29 02:03:51）: 固定モード（英語）→ キーボードごと（標準 JIS）。baseline は 7/0、`kbd101.dll`、`PCAT_101KEY`。状態は「PC の再起動待ち」 |
| 入力方式 | 日本語（Microsoft IME、既定の設定。`keystyle=NATURAL`、キーの割り当てなし）。リマッパー（PowerToys など）なし、`Scancode Map` なし |

「英語キーボード」の固定モードに誰がいつ切り替えたのかは分かっていない（12 節）。

## 3. キーの届き方

- RDP のキー入力はスキャンコードで届く（[MS-RDPBCGR] `TS_KEYBOARD_EVENT` / `TS_FP_KEYBOARD_EVENT`。仮想キーは送らない）。英数 と Caps Lock は同じイベント（keyCode 0x3A、拡張なし）で、Shift+英数 は Shift 0x2A、0x3A、0x3A 離す、0x2A 離す、の順になる。**0x3A が 英数 か Caps Lock かは、接続先のセッションのキーの表と修飾キーで決まる。**
- `TS_SYNC_EVENT` はトグルの状態（Scroll / Num / Caps / Kana）だけを運び、サーバーのキーを「すべて離した」状態にする。キー操作そのものは届けない。クライアントは、フォーカスが戻ったときなどに送る。
- 接続元は Client Core Data で `keyboardType`（7 = 日本語キーボード）、`keyboardSubType`、`keyboardFunctionKey` を報告する。セッション内の `GetKeyboardType` は、この値（7/2/12）を返す。
- 接続元の Windows のキーボード配列と IME は、キーを送る経路には入らない（Windows のクライアントには Unicode モードの切り替えがない）。
- 接続先では、キー入力は「リモート デスクトップ キーボード デバイス」（`TERMINPUT_BUS\UMB\…&SESSION1KEYBOARD0`、ハードウェア ID `TS_INPT\TS_KBD`、サービス terminpt）から入る。

## 4. kbd106 と kbd101 の表

この PC の DLL を読み込んで表を読んだ結果（Microsoft の公開サンプル `Windows-driver-samples` の `fe_kbds/jpn` と同じ内容）。

| スキャンコード | 表 | 仮想キー | 単独 | Shift | Ctrl | Alt |
|---|---|---|---|---|---|---|
| 0x3A | kbd106（JIS、7/2） | `VK_DBE_ALPHANUMERIC` (0xF0) | 英数 | Caps Lock (`VK_CAPITAL`) | 英数 | 英数 |
| 0x3A | kbd101（US、4/0。kbdjpn.dll の既定の表） | `VK_CAPITAL` (0x14) | Caps Lock | 英数 (`VK_DBE_ALPHANUMERIC`) | ひらがな | カタカナ |
| 0x29 | kbd106 | `VK_DBE_SBCSCHAR` / `DBCSCHAR` (0xF3 / 0xF4) | 半角/全角 | | | 漢字 (`VK_KANJI`) = IME オン/オフ |
| 0x29 | kbd101 | `VK_OEM_3` (0xC0) | `` ` `` | | | 漢字 (`VK_KANJI`) = IME オン/オフ |

Microsoft の IME の説明（キーボード ショートカット）も同じ: 106/109 キーでは「英数 (Caps Lock)」でひらがな/英数を切り替え、101/102 キーでは Shift+Caps Lock が英数キー、Ctrl+Caps Lock がひらがなキー、Alt+Caps Lock が Shift+ひらがなキー、Alt+`` ` `` が IME のオン/オフ。**Alt+半角/全角（スキャンコード 0x29 + Alt）は、どちらの表でも `VK_KANJI`** なので、どちらの表でも IME のオン/オフになる。

kbdjpn.dll は 3 つの表を持つ（`KbdLayerMultiDescriptor`）: kbd101.dll（4/0）、kbd106.dll（7/2）、kbdnec.dll（7/0xD02）。先頭の kbd101 が既定の表。

## 5. セッションが表を選ぶ仕組みと、分かっていないこと

分かっていること:

- コンソールでは、`i8042prt\Parameters` の `LayerDriver JPN`（標準配列）と、固定モードなら全体の `OverrideKeyboardType/Subtype` が表を決める。キーボードごとモードでは、キーボードの種類（7/2 → kbd106、4/0 → kbd101）が表を選び、種類が表を選ばないキーボード（0x51/0）は標準配列に従う（M0 #4。0x51 の打鍵確認はまだ）。
- RDP のセッションでは、接続元が報告した種類から `HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\KeyboardType Mapping\JPN` で DLL を選ぶ経路がある（この PC の値: `00000000`=kbd101.dll、`00000002`=kbd106.dll、`00010002`=kbd106n.dll など）。このキーは Microsoft の正式な文書にない。
- [MS-RDPBCGR] の注 <6>: 接続元の入力ロケールは**新しく作るセッションにだけ**使い、既存のセッションへの接続では無視する。
- 2018 年の Microsoft の Ask CORE ブログ（Windows 10 1803）: RDP のセッションが `Keyboard Layouts\00000411` の `Layout File`（KBDJPN.DLL、英語配列が基本）を使い、コンソールの `LayerDriver JPN`（kbd106）を使わなかった。KB4458469 で修正。2025 年の Microsoft Japan のブログ（AVD / Windows 365、Mac の接続元）も同じ種類のずれを「調査中」としている。

分かっていないこと:

- キーボードごとモードで、RDP のキーボード（terminpt。種類は分からない。セッションの Raw Input に並ぶ名前のない 0x51/0 の項目がこれかどうかも分からない。6.3 節）がどの表になるか: 接続元の 7/2 → kbd106 か、PC の標準配列か、kbdjpn の既定（kbd101）か。**JIS か 101 かは E7 で、接続元と標準配列のどちらに従うかは E7b で確かめる。**
- セッション 1 が 101 の表のままなのは、(a) コンソールで作られたセッションだから（作られたときの表が残る）か、(b) 起動時の固定モード（英語）が再起動まで全セッションに効いているからか。どちらも「再起動して新しくサインインする」で直る。E6 で切り分けられるが、セッション 1 を終わらせる必要がある。
- `KeyboardType Mapping` が 24H2 以降でも使われるか（Windows 11 では効かないという未確認の書き込みがある）。
- win32k が 0x3A の NLS のトグル（`KBDNLS_TYPE_TOGGLE`、`NLSFEProcSwitch`）をどう扱うかは公開されていない。

## 6. 観察

### 6.1 レジストリ（読み取りのみ）

- `i8042prt\Parameters`: `LayerDriver JPN=kbd106.dll`、`OverrideKeyboardIdentifier=PCAT_106KEY`、全体の `OverrideKeyboardType/Subtype` はない（移行で書いた値。再起動まで効かない）。
- `Keyboard Layouts\00000411` の `Layout File` = `KBDJPN.DLL`（既定のまま）。
- `KeyboardType Mapping\JPN`: 5 節のとおり。`Scancode Map` はない。

### 6.2 セッション内の API

- `GetKeyboardType(0/1/2)` = 7/2/12（接続中のセッション 1 の中で、サンドボックスの外から実行したプローブで確認）。**接続元の報告であって、セッションが使う表ではない。** Claude Code のシェル（サンドボックス）と、切断中のセッションでは 0/0/0 を返した。
- API から見た表（調査用のプローブ。`tools/m0/Get-KbdState.ps1` の `SessionTable0x3A` でも見られる）: `MapVirtualKeyEx(0x3A)` = 0x14（`VK_CAPITAL`）、0x29 → 0xC0、`VkKeyScanEx('@')` = Shift+2、`VkKeyScanEx('"')` = Shift+`VK_OEM_7`。どれも US の位置。キーボードごとモードでは、API の表とデバイスごとの表が違うことがありうるので、決め手は 6.4 節のフック。

### 6.3 デバイスと Raw Input

- RDP のキーボード: `TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION1KEYBOARD0`、「リモート デスクトップ キーボード デバイス」、ハードウェア ID `TS_INPT\TS_KBD`、互換 ID `TI_COMPAT_DEVICE`、サービス terminpt（スタック kbdclass / terminpt / umbus）、親 `UMB\UMB\1&841921D&0&TERMINPUT_BUS` → `ROOT\UMBUS\0000` → `HTREE\ROOT\0`、コンテナは内蔵（`{00000000-0000-0000-FFFF-FFFFFFFFFFFF}`）、状態 0x0180200A（開始済み）。セッションを切断すると devnode は非接続になる（06:12 の切断で `IsPresent=False`）。
- Raw Input には、名前のないキーボードが 5〜8 個並ぶ（数は観察ごとに違う）。`RIDI_DEVICENAME` の長さは終端の 1 文字だけで、名前を読むと失敗する（`GetLastError` は 6 か 0）。種類はどれも 0x51/0。Microsoft は、RDP の入力デバイスは Raw Input の一覧に出ないと書いているので、これらの項目が何かは分からない。RDP のキーボードの devnode とは対応づけられない。
- MKLM 0.1.0（この調査の前）: RDP のキーボードを接続方式「不明」と表示し、名前のない項目 1 つごとに「could not read Raw Input device …: GetRawInputDeviceInfoW failed with Win32 error 6」の警告を出し、「the remote client's keyboard layout applies to the whole session」という誤った注意を出していた。

### 6.4 セッション内の低レベル フック（Windows App と mstsc）

リードの記録用スクリプト（切り替えと IME のキーだけを記録し、ほかのキーは「other key」とだけ書く。`tools/rdp/Watch-Keys.ps1` はその移植）をセッション 1 で動かし、メモ帳と Chrome で押した。**Windows App と mstsc で結果は同じだった。** 下の表は記録に残ったキーだけ。Ctrl+英数、Alt+英数、Alt+半角/全角 は、どちらの記録にもない（`vk=0xF1` / `0xF2` / `0x19` も、`SYSDOWN` / `SYSUP` もない。スクリプトはスキャンコード 0x3A / 0x29 のイベントをすべて書くので、押していれば残る）。

| 押したキー | 届いたもの（両方のクライアント） | 101 の表の予想 | 106 の表の予想 |
|---|---|---|---|
| 英数 だけ | `vk=0x14 sc=0x3A` の DOWN。**UP は届かない**。Caps Lock は最初の 1 回だけ切り替わる（2 回目以降は押しっぱなしの繰り返しと同じ扱い） | 0x14 | 0xF0 |
| Shift+英数 | Shift（0xA0）のあと `vk=0xF0 sc=0x3A`（`VK_DBE_ALPHANUMERIC`）。IME が切り替わる。0x3A の UP は、次に押したときの DOWN の直前に届く | 0xF0 | 0x14 |
| 半角/全角 | `vk=0xC0 sc=0x29`（UP と DOWN が同時に届く）。Chrome では、両方のクライアントで押すたびに IME のオン/オフが切り替わった（記録の `imeOpen` が 1 と 0 の間で変わる） | 0xC0 | 0xF3 / 0xF4 |

- 調べたキー（0x3A と 0x29、それと組み合わせた Shift）には `INJECTED` の印がなく、`VK_PACKET` もない。スキャンコードの経路で届いている。ただし mstsc の記録には、フォーカスが移ったとき（`fg=` が空になる直前）に、`INJECTED` の印が付いたキーの UP（Shift の `vk=0xA0 sc=0xAA` と `vk=0xA1 sc=0xB6`、ほかのキー 4 つ）がまとめて届いたことが 2 回ある。クライアントが修飾キーを離した状態に戻すためのものと考えられ、判定には使っていない。
- 半角/全角だけで IME が切り替わったことは、4 節の表（101 の表では 0x29 は `VK_OEM_3` = `` ` ``）と合わない。表ではなく IME の側がスキャンコード 0x29 を半角/全角として扱っている、と考えられるが確かめていない。記録はどれも Chrome のもので、メモ帳では記録用スクリプトが IME の状態を読めていない可能性がある（メモ帳の中では、Shift+英数 の後も `imeOpen` が変わらなかった）。9 節の案内は半角/全角だけの動作には頼らず、どちらの表でも `VK_KANJI` になる Alt+半角/全角 を案内する。
- 0x3A の UP が遅れる、または届かないのは、xrdp の issue #2158 が mstsc で記録したもの（英数を単独で押すと押した信号だけが届く）と同じで、クライアント側の性質と考えられる。106 の表のセッションで影響があるかは、E7 で見る。

### 6.5 記号の確認（E1）

IME をオフにしてメモ帳で打った: Shift+2 → `@`、P の右のキー → `[`。どちらも US の位置。「文字は正しい」という最初の報告は、英字か、IME がオンの状態で確かめたものと考えられる（2018 年のブログのとおり、IME がオンだと気付きにくい）。

## 7. 仮説と判定

| | 仮説 | 判定 |
|---|---|---|
| H1 | セッション 1 は、再起動前の固定モード（英語）の名残で 101 の表を使っている。(a) コンソールで作られたセッションの表が残る、(b) 起動時の固定モードが再起動まで効く、のどちらか | **採用**。フック（両クライアント）、記号、API のすべてが 101 の表を示す。(a) と (b) は切り分けていない（E6 未実施）。どちらも再起動と新しいサインインで直る |
| H2 | 移行が効いた後も、キーボードごとモードの RDP のセッションは 101 の表になる（たとえば、RDP のキーボードの種類が表を選ばず、kbdjpn の既定の表になる。RDP のキーボードの種類は分かっていない。6.3 節） | **未判定**。E7 / E8 で確かめる |
| H3 | Windows App がキーを変えている（英数を押した信号だけ、など） | 主な原因としては**棄却**。mstsc でも同じ。0x3A の UP の遅れは両方のクライアントの性質として残る |
| H4 | 表は kbd106 だが、IME が `PCAT_101KEY` を読んで 101 のように振る舞う | **棄却**。単独の 0x3A が 0x14 で届いており、表そのものが 101 |
| H5 | 表は kbd106 だが、`TS_SYNC_EVENT` で 0x3A のトグルがずれる | **棄却**。表が kbd106 ではない |
| H6 | Windows App が Unicode の経路でキーを送り、接続元の IME が処理している | **棄却**。スキャンコード（sc=0x3A）が届き、`VK_PACKET` がない |

## 8. 実験

| | 内容 | 予想 | 結果（2026-09-29） |
|---|---|---|---|
| E1 | 記号の確認（IME オフ、メモ帳）: Shift+2、P の右、L の右の右、¥ の左、¥、ろ、半角/全角 | 101 の表なら US の記号、kbd106 なら JIS の記号 | Shift+2 → `@`、P の右 → `[`。**101 の表** |
| E2 | セッション内のフック（Windows App）: 英数 ×3、Shift+英数 ×2、Ctrl+英数、Alt+英数、半角/全角、Alt+半角/全角 | 101: 英数 = 0x14、Shift+英数 = 0xF0、半角/全角 = 0xC0 | 記録に残ったのは 英数、Shift+英数、半角/全角 で、どれも予想どおり（6.4 節）。**101 の表**。Ctrl+英数、Alt+英数、Alt+半角/全角 は記録にない。これらが効くことは 4 節の表と Microsoft の説明によるもので、RDP ではまだ確かめていない |
| E3 | E2 を mstsc で | 表は接続先で決まるので同じ | 英数、Shift+英数、半角/全角 は**同じ**。Ctrl+英数、Alt+英数、Alt+半角/全角 は、こちらの記録にもない |
| E4 | 接続元の PC だけでの確認と、クライアントがキーを消費しないかの確認 | 接続元は JIS の動作 | 未実施（E2 / E3 が同じだったので優先度は低い） |
| E5 | 接続先の IME とリマッパーの確認（読み取り） | 既定の Microsoft IME、リマッパーなし | 確認済み: 既定の設定、`Scancode Map` なし、PowerToys などなし |
| E6 | 再起動前に、サインアウトしてから RDP で新しいセッションを作る | H1(a) なら JIS、H1(b) なら 101 のまま | 未実施（セッション 1 のアプリがすべて終わるため、利用者の判断が必要） |
| E7 | 再起動の後、コンソールでサインインせずに、同じ JIS の接続元（7/2）から RDP で接続し、E1 と E2 | H1 なら JIS（英数 = 0xF0、Shift+2 → `"`）。101 のままなら H2。接続元（7/2 → kbd106）に従っても標準配列（JIS）に従っても JIS なので、JIS でもどちらに従ったかは分からない（E7b） | **保留**（再起動待ち） |
| E7b | E7 で JIS なら: 7/0 か 4/0 を報告する接続元（英語キーボードの PC、「英語キーボード (101/102 キー)」の設定の Windows、ほかの OS の Windows App など）から新しいセッションで接続し、E1 と E2。接続先には何も書き込まない | 101 なら接続元の種類に従う（7/0 は `KeyboardType Mapping\JPN\00000000` = kbd101.dll）。JIS なら PC の標準配列に従う | **保留**（そのような接続元が必要） |
| E8 | 再起動の後、コンソールでサインインしてから RDP で再接続し、E1 と E2 | JIS なら再接続の注意は不要。101 ならコンソールで作ったセッションの表が残る | **保留**（コンソールでの操作が必要） |
| E9 | 再起動前に、接続先の PC の前でその PC のキーボードで 英数 と Shift+2 | H1 なら 101 の動作（固定モード（英語）が再起動まで効くため） | 未実施 |
| E10 | Caps Lock の再同期（英数のあと、窓の外をクリックして戻る） | 接続元の Caps オフが同期で戻る | 一部: フックでは最初の 英数 で Caps が 1 になり、その後の 英数 では変わらない（UP が届かないため）。同期の影響は未確認 |

`tools/rdp/Watch-Keys.ps1`（E2 / E3）と `tools/m0/Get-KbdState.ps1`（API の表 `SessionTable0x3A`、`GetKeyboardType`、名前のない Raw Input の数）で、同じ確認ができる。どちらも読み取りだけで、セッションの中で利用者が開いた PowerShell の窓から実行する。

## 9. 利用者への案内

- **再起動をする**（「シャットダウン」ではなく「再起動」）。MKLM の操作 `c10d2d38` が待っている。キー配列はセッションが始まったときに決まるので、再起動を待つ変更は、再起動して新しくサインインするまでリモート デスクトップに反映されない。再起動の後は、切断したセッションに再接続するのではなく、新しくサインインしてから使う（再接続したセッションは、始まったときのキー配列のまま）。サインアウトだけで足りるか（再起動の前に新しいセッションを作れば直るか）は E6 まで分からないので、案内しない。
- それまでの代わりのキー（101 の表で効く。4 節の表と Microsoft の説明による。6.4 節のフックの記録にあるのは Shift+英数 だけ）: **Shift+英数 = 英数**（ひらがな/英数の切り替え）、**Ctrl+英数 = ひらがな**、**Alt+英数 = カタカナ**、**Alt+半角/全角 = IME のオン/オフ**（どちらの表でも効く）。Microsoft IME の設定 →「キーとタッチのカスタマイズ」で Ctrl+Space に IME-オン/オフを割り当てると、表に関係なく使える。
- それまでは記号キーが US の位置で打たれる（Shift+2 で `@`）。
- mstsc と Windows App で違いは見つかっていない。

## 10. MKLM がすること・しないこと

する（どれも書き込みなし）:

- RDP のキーボード（`TERMINPUT_BUS\…` か、ハードウェア ID `TS_INPT\TS_KBD`）を仮想のキーボードとして扱い、読み取り専用にする（計画 3.2）。GUI では「リモート デスクトップ」と表示し、配列の名前は出さない。
- 名前のない Raw Input のキーボードで警告を出さない。
- 接続元が報告した種類は「接続元の報告」としてだけ示し、使われている配列としては示さない（このセッションは 7/2 を報告しながら 101 の表で打っていた）。種類の名前は `KeyboardType Mapping\JPN`（6.1 節）に合わせる: 7/2 は「日本語キーボード (JIS)」、7/1・7/3・7/0xD01〜0xD04 は「日本語キーボード」、7/0（Windows の「英語キーボード (101/102 キー)」の設定。`00000000` = kbd101.dll）と 4/x は「英語キーボード (101/102 キー)」、それ以外は示さない。
- IME の案内画面（計画 3.10）で 9 節の代わりのキーを、CLI の注意でキー配列が決まる時点（セッションが始まったとき。再起動を待つ変更は再起動と新しいサインインの後）を案内する。どちらも、新しいセッションが接続元と PC の標準配列のどちらに従うかは書かない（E7b まで分からない）。

しない（採らない回避策と理由）:

| 回避策 | 採らない理由 |
|---|---|
| `Keyboard Layouts\00000411` の `Layout File` を kbd106.dll にする（ブログでよく見る） | キーボードごとの配列には kbdjpn.dll が必要で、これを変えると MKLM のキーボードごとの配列が効かなくなる |
| `Terminal Server\KeyboardType Mapping` を書き換える | 文書にない値で、PC 全体と、すべての接続元に効く。24H2 以降で効くかも確認できていない |
| `Scancode Map` を足す | PC 全体に効き、キーボードごとに変えられない。Microsoft はターミナル サービスでは正しく動かないことがあると書いている |
| `IgnoreRemoteKeyboardLayout` | 入力言語（HKL）の設定で、キーの表とは関係がない |
| RDP のキーボードの devnode に override の値を書く | terminpt は値を読まない。devnode はセッションごと。計画 3.2 で読み取り専用 |
| Microsoft IME のレジストリ値を書く | 計画 3.10 で禁止。案内と設定画面へのリンクだけにする |
| PC を固定モード（JIS）に戻す | キーボードごとの配列をあきらめることになる。H2 が確かめられたときに、利用者が決めることとして説明するだけにする |

## 11. 実装メモ

- 接続方式: `TERMINPUT_BUS` の列挙子を `Transport::Virtual` にした（新しい種類は足していない）。`mklm_core::is_remote_desktop_keyboard` / `KeyboardDevice::is_remote_desktop` が、列挙子かハードウェア ID で見分ける。`mklm-win` の読み取りは `mklm_core::classify_keyboard_transport` で、このキーボードを列挙子、ドライバー、バスのサービスに関係なく仮想にする。許可リスト（`check_device_writes`、`check_cleanup`、`cleanup_candidates`）は、接続方式にかかわらず `is_remote_desktop` のキーボードを拒むので、GUI の読み取り専用の行と、書き込めるものが食い違わない。リセットの禁止と「ほかに使えるキーボード」に数えないことは、仮想の規則で効く（i8042prt のキーボードでも、PS/2 のリセットの禁止と INV-PS2 はドライバーで決まるので残る）。
- 変わる動作: RDP のキーボードは「ほかに使えるキーボード」に数えなくなった（設計 m2 D.2 のとおり）。RDP で接続中に PC の唯一の物理キーボードを変えると、その場でのリセットではなく再起動の計画になる。
- RDP のキーボードは内蔵コンテナにあるが、行は別で、「内蔵」のバッジは付けない（CLI の `Internal` も no）。行の ID はインスタンス ID（`SESSION1KEYBOARD0` を含む）なので、非表示の設定はセッションが変わると引き継がれない。
- Raw Input: `RIDI_DEVICENAME` の長さが 1 文字以下か、名前が空の項目は、issue にせず読み飛ばす。ほかの失敗はこれまでどおり `RawInput` の警告。
- `OsInfo::client_keyboard_type`: リモート セッションのときだけ `GetKeyboardType` の種類とサブタイプを入れる。`KeyboardDevice::reported_type` には入れず、配列にも対応づけない。
- `fixtures::rdp_keyboard()` がこの PC で読んだ値を持つ（`dev_machine()` には入れていない）。

## 12. 未解決の問い

- E7 / E8 の結果: キーボードごとモードの新しい RDP セッションは JIS になるか。101 のままなら、README の「できない」とバッジの説明に、確かめた制限として書く。
- E7b の結果: 新しいセッションは接続元の種類と PC の標準配列のどちらに従うか。標準配列に従うなら、PC 全体の設定（標準配列や固定モード）が新しいセッションの表を決めることを、案内に書ける。
- H1 の (a) と (b) のどちらか（E6）。
- 誰がいつ「英語キーボード」の固定モードにしたのか。意図したものなら、再起動の後、値を持たない HID キーボードは新しい標準（JIS）に従う。再起動の後に接続先のキーボードを確かめる。
- 0x3A の UP が遅れる、または届かないクライアントの性質は、106 の表のセッションで困ることがあるか。
- 計画 3.11 の健全性の確認で、`KeyboardType Mapping\JPN` を（読み取りだけで）示すべきか。
- RDP の行の非表示を、ハードウェア ID で覚えるべきか。
- 名前のない Raw Input の項目は何か。再接続のたびに増えるのか（5〜8 個と観察ごとに違った）。
- サンドボックスのシェルと切断中のセッションで `GetKeyboardType` が 0 を返す理由。

## 13. 出典

- [MS-RDPBCGR] Client Core Data（`TS_UD_CS_CORE`）: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/00f1da4a-ee9c-421a-852f-c19f92343d73>
- [MS-RDPBCGR] 製品の注（<5>、<6>、<35>）: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/cbe1ed0a-d320-4ea5-be5a-f2eb6e032853>
- [MS-RDPBCGR] Input Capability Set: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/b3bc76ae-9ee5-454f-b197-ede845ca69cc>
- [MS-RDPBCGR] Keyboard Event: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/7acaec9f-c8a6-4ee9-87d6-d9b89cf56489> 、Fast-Path Keyboard Event: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/089d362b-31eb-4a1a-b6fa-92fe61bb5dbf>
- [MS-RDPBCGR] Synchronize Event: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/6c5d0ef9-4653-4d69-9ba9-09ba3acd660f>
- `GetKeyboardType`: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardtype>
- `GetRawInputDeviceList`（RDP のデバイスは一覧に出ない）: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getrawinputdevicelist>
- Microsoft IME（キーボード ショートカット）: <https://support.microsoft.com/en-us/windows/microsoft-japanese-ime-da40471d-6b91-4042-ae8b-713a96476916> 、<https://learn.microsoft.com/en-us/globalization/input/japanese-ime>
- 表のサンプル: <https://github.com/microsoft/Windows-driver-samples/blob/main/input/layout/fe_kbds/jpn/106/kbd106.c> 、<https://github.com/microsoft/Windows-driver-samples/blob/main/input/layout/fe_kbds/jpn/101/kbd101.c> 、Windows SDK 10.0.26100.0 `kbd.h`
- Ask CORE（2018、RS4 の RDP の配列、KB4458469）: <https://learn.microsoft.com/en-us/archive/blogs/askcorejp/rs4-rdp-keyboardlayout>
- Microsoft Japan（2025、Windows 365 のキーボード）: <https://jpwinsup.github.io/blog/2025/05/19/Windows365/W365KeyboardRedirection/>
- KB3120433（Caps Lock の状態が接続元に同期されない）: <https://learn.microsoft.com/en-us/troubleshoot/windows-server/remote/caps-lock-key-status-not-synced-to-client>
- 地域と言語の設定（`IgnoreRemoteKeyboardLayout`）: <https://learn.microsoft.com/en-us/troubleshoot/windows-server/application-management/regional-and-language-options-settings>
- Windows App の入力: <https://learn.microsoft.com/en-us/windows-app/input-keyboard-mouse-touch-pen> 、リモート PC への接続（プレビュー、MSTSC を推奨）: <https://learn.microsoft.com/en-us/windows-app/get-started-connect-devices-desktops-apps?pivots=remote-pc>
- xrdp #2158（mstsc の 英数 は押した信号だけ）: <https://github.com/neutrinolabs/xrdp/issues/2158>
- Citrix CTX575043（接続元の申告する種類で 英数 / Caps Lock が変わる）: <https://support.citrix.com/external/article/CTX575043/capslock-and-eisu-keys-on-japanese-106-k.html>
- Scancode Map: <https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/keyboard-and-mouse-class-drivers>
- Windows の設定の URI（`ms-settings:regionlanguage-jpnime`）: <https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-settings-app>
