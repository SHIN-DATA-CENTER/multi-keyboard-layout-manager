# Multi Keyboard Layout Manager (MKLM)

日本語 | [English](#english)

JIS 配列（日本語 106/109）と US 配列（英語 101/104）の物理キーボードを、1 台の Windows PC で併用するためのツール。キーボードごとに配列を割り当て、変更を安全に適用し、うまくいかなければ元に戻す。

今は、Windows のレジストリ（デバイスごとの `Device Parameters` と、PC 全体の `Services\i8042prt\Parameters`）を手で編集するしかない。この編集は誤りやすく、失敗したときの戻し方もない。MKLM はこれを 1 本のソフトにまとめる。

## 警告（使う前に必ず読むこと）

> **MKLM は開発中で、まだ一般の利用に向けて公開していない。**

- **PC 全体の設定を書き換える。** MKLM の書き込みのコマンドは、`HKLM\SYSTEM` のキーボードの設定を変える。設定は PC のすべてのユーザーとサインイン画面に効く。誤った設定になると、**サインイン画面でパスワードの記号が打てなくなる**ことがある。
- **書き込みのコマンド（M2）は、実機での検証がまだ済んでいない。** 検証用の PC 以外では使わないこと。
- **使う前に準備すること**（詳しくは [docs/recovery.md](docs/recovery.md) の 7 章）
  - [docs/recovery.md](docs/recovery.md) を読み、印刷するか別の端末で読めるようにしておく。
  - 数字だけの PIN でサインインできること、サインイン画面でスクリーンキーボードを出せることを確かめる。
  - BitLocker（デバイスの暗号化）の回復キーの場所を確かめる（<https://aka.ms/myrecoverykey>）。
  - `reg export` で `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` を書き出しておく。
- **困ったら**: コマンドプロンプトで `mklm-cli undo` を実行し、PC を**再起動**する（シャットダウンではなく再起動）。それで直らなければ [docs/recovery.md](docs/recovery.md)（セーフモード、回復環境 WinRE での手順）。
- **コード署名をしていない。** UAC の確認画面では発行元が「不明」と表示され、SmartScreen の警告が出ることがある。スマートアプリコントロールが有効な PC では使えない。
- **対応 OS**: Windows 11 24H2（build 26100）以降。x64 を優先する（ARM64 は実機では試していない）。
- **無保証**: Apache License 2.0 のとおり、どのような保証もない。

## できること・できないこと

| | 内容 |
|---|---|
| できる | キーボードごとに JIS か US を割り当てる。ただし Windows の入力方式が**日本語 IME** のときだけ効く |
| できる | 外付けの USB キーボードは、キーボードのリセットでその場で反映する（20 秒のカウントダウンで「このままにする / 元に戻す」を選ぶ）。Bluetooth は再接続で反映する |
| 再起動が必要 | 内蔵の PS/2 キーボードの変更と、PC 全体の設定の変更（Windows の仕様） |
| できない | 英語（US）の入力方式に切り替えている間のキーボードごとの配列。すべてのキーボードが US になる |
| できない | Logicool の Unifying / Bolt レシーバーや KVM につないだ複数のキーボードの区別。1 台として扱われる |
| できない | リモートデスクトップの接続先で、キーボードごとに配列を変えること |

## 今の状態

| マイルストーン | 内容 | 状態 |
|---|---|---|
| M0 | 実機での検証（[docs/research/m0-results.md](docs/research/m0-results.md)） | 完了 |
| M1 | 読み取りの土台（`mklm-cli list`、`status`、`global status`） | 完了 |
| M2 | 書き込みと安全網（ジャーナル、回復、helper、書き込みのコマンド） | 実装中。実機での検証の前（設計: [docs/design/m2-engine.md](docs/design/m2-engine.md)） |
| M3 | GUI（Slint） | 予定 |
| M4 | 常駐機能（ずれの検知と通知） | 予定 |
| M5 | 配布（MSI インストーラー、自動更新） | 予定 |

GUI とインストーラーはまだない。今使えるのはコマンドラインの `mklm-cli` だけ。

## ビルド

Rust のツールチェーン（`rust-toolchain.toml` で版を固定している）と、Visual Studio Build Tools（C++ と Windows SDK）が必要。

```bat
cargo build --release
```

`target\release\` に `mklm-cli.exe` と `mklm-helper.exe` ができる。**2 つは同じフォルダーに置いて使う**（`mklm-cli` は、同じフォルダーの `mklm-helper.exe` を UAC で昇格して起動し、書き込みはすべて helper が行う）。

## 使い方

### 読み取り（M1。管理者権限は不要で、何も書き換えない）

| コマンド | 内容 |
|---|---|
| `mklm-cli list` | 接続中のキーボードの一覧。保存されている種類、Raw Input が報告する種類、実際に使われる配列を示す |
| `mklm-cli list --all` | 接続していないキーボード（phantom）も含める |
| `mklm-cli status [--json] [--all]` | 読み取ったすべての情報と、MKLM の判定 |
| `mklm-cli global status [--json]` | PC 全体の設定（モード、標準配列）と、入力方式の警告 |
| `mklm-cli journal [--json]` | MKLM の操作の記録（M2） |

### 書き込み（M2。実装中、検証用の PC だけで使う）

書き込みのコマンドは、実行すると UAC の確認画面を出して `mklm-helper.exe` を起動する。書く前に、変わる値と必要な操作（キーボードのリセット、再接続、PC の再起動）を表示して確認を求める。

| コマンド | 内容 |
|---|---|
| `mklm-cli set <キーボード> --layout jis\|us\|standard` | キーボードに配列を割り当てる（`standard` は PC の標準配列に従う。外付けのキーボードだけ） |
| `mklm-cli migrate [--standard jis\|us] [--also <キーボード>=<配列>]...` | 固定モード（すべてのキーボードが同じ配列）から、キーボードごとモードへ移行する。PC の再起動が 1 回必要 |
| `mklm-cli undo` | 確認待ち、再起動待ち、衝突の変更をまとめて取り消す。**困ったときに最初に使うコマンド** |
| `mklm-cli revert <操作 ID>` | 操作を 1 つ取り消す |
| `mklm-cli restore --baseline (--all \| <キーボード>)` | MKLM を使う前の値に戻す |
| `mklm-cli recover` | 途中で止まった操作（強制終了、電源断など）を回復する |
| `mklm-cli resolve <操作 ID>` | 衝突（MKLM の書いた値がほかから変えられた）をどうするか決める |
| `mklm-cli keep <操作 ID>` | 確認待ちの操作を確定する（別名 `confirm`） |
| `mklm-cli reboot` | 変更を反映するために PC を再起動する |

- `<キーボード>` はインスタンス ID か、`mklm-cli list` の行番号 `#n`。インスタンス ID には `&` が入るので、引用符で囲む。PowerShell では `#` が注釈の始まりになるので、`"#2"` のように囲む。
- キーボードをリセットし得るコマンドには、`--other-input`（別のキーボードかマウスで操作できる。リセットしてよい）か `--no-reset`（リセットしない。再接続か再起動で反映する）を付けられる。どちらもなければ、必要になった時点で尋ねる。`set` と `restore` で `--yes`（確認を省く）を使うときは、どちらかが必須。
- USB キーボードをその場でリセットした場合は、リセットの後に 20 秒のカウントダウンが始まる。Shift+2 を打って確かめ（JIS なら `"`、US なら `@`）、`y` で確定する。答えなければ自動で元に戻す。
- 終了コード: 0 完了、1 失敗、2 使い方の誤り、3 取り消した、4 自動で元に戻した、5 衝突、6 止められた、10 再接続を待っている、3010 PC の再起動が必要。

例: 外付けの US キーボードに US を割り当てる（すでにキーボードごとモードの PC）。

```bat
mklm-cli list
mklm-cli set "#2" --layout us --other-input
```

例: 固定モード（JIS）の PC で、内蔵キーボードを JIS のまま、外付けキーボードを US にする。

```bat
mklm-cli migrate --standard jis --also "#2=us"
mklm-cli reboot
```

再起動してサインインすると確認画面（`mklm-cli post-reboot`）が開くので、Shift+2 で確かめてから答える。

## 困ったとき

1. `mklm-cli undo` を実行し、PC を**再起動**する。
2. それでも直らなければ、[docs/recovery.md](docs/recovery.md) を上から順に試す（サインイン画面での入力、セーフモード、回復環境 WinRE、`restore-offline.cmd`）。
3. 復旧用のファイルは、最初の書き込みの前に `C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery\` に作られる。

## ドキュメント

| 文書 | 内容 |
|---|---|
| [docs/recovery.md](docs/recovery.md) | 復旧ガイド（日本語）。サインインできない、配列がおかしい、などのときの戻し方 |
| [docs/design/m2-engine.md](docs/design/m2-engine.md) | M2 の設計（書き込み、ジャーナル、回復、helper との通信、CLI） |
| [docs/research/m0-results.md](docs/research/m0-results.md) | M0 の実機での検証結果 |

## ライセンス

[Apache License 2.0](LICENSE)。Copyright SHIN DATA CENTER.

---

## English

Multi Keyboard Layout Manager (MKLM) lets you use JIS (Japanese 106/109) and US (English 101/104) physical keyboards side by side on one Windows PC. It assigns a layout to each keyboard, applies the change safely, and puts it back if something goes wrong.

Today this means hand-editing the registry (each device's `Device Parameters` and the PC-wide `Services\i8042prt\Parameters`), which is easy to get wrong and has no way back. MKLM wraps it in one tool.

### Warning (read before use)

> **MKLM is under development and not yet released for general use.**

- **It changes PC-wide settings.** MKLM's write commands change keyboard settings under `HKLM\SYSTEM`. They apply to every user on the PC and to the sign-in screen. A wrong setting can leave you **unable to type the symbols in your password on the sign-in screen**.
- **The write commands (M2) have not been verified on real hardware yet.** Use them only on a test PC.
- **Before you use it** (details in chapter 7 of [docs/recovery.md](docs/recovery.md), in Japanese)
  - Read [docs/recovery.md](docs/recovery.md), and print it or keep it readable on another device.
  - Make sure you can sign in with a digits-only PIN, and that you can open the on-screen keyboard on the sign-in screen.
  - Know where your BitLocker (device encryption) recovery key is (<https://aka.ms/myrecoverykey>).
  - Back up `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` with `reg export`.
- **If something goes wrong**: run `mklm-cli undo` in a command prompt, then **restart** the PC (a restart, not a shutdown). If that does not help, follow [docs/recovery.md](docs/recovery.md) (Safe Mode, the recovery environment WinRE).
- **Not code-signed.** The UAC prompt shows the publisher as "Unknown", and SmartScreen may warn. It cannot run on a PC with Smart App Control turned on.
- **Supported OS**: Windows 11 24H2 (build 26100) or later. x64 first (ARM64 is untested on hardware).
- **No warranty**: as stated in the Apache License 2.0.

### What it can and cannot do

| | |
|---|---|
| Can | Assign JIS or US to each keyboard. This works only while the Windows input method is the **Japanese IME** |
| Can | Apply a change to an external USB keyboard at once by resetting the keyboard (then choose keep or revert within a 20-second countdown). Bluetooth keyboards apply on reconnect |
| Needs a restart | Changes to a built-in PS/2 keyboard and to PC-wide settings (a Windows limitation) |
| Cannot | Per-keyboard layouts while the English (US) input method is active; every keyboard is US then |
| Cannot | Tell apart several keyboards behind one Logitech Unifying / Bolt receiver or a KVM; they count as one |
| Cannot | Per-keyboard layouts inside a Remote Desktop session |

### Status

| Milestone | Scope | Status |
|---|---|---|
| M0 | Experiments on real hardware ([docs/research/m0-results.md](docs/research/m0-results.md), Japanese) | Done |
| M1 | Read-only foundation (`mklm-cli list`, `status`, `global status`) | Done |
| M2 | Writes and safety net (journal, recovery, helper, write commands) | In progress; not yet verified on hardware (design: [docs/design/m2-engine.md](docs/design/m2-engine.md), Japanese) |
| M3 | GUI (Slint) | Planned |
| M4 | Background features (drift detection and notification) | Planned |
| M5 | Distribution (MSI installer, auto-update) | Planned |

There is no GUI and no installer yet; only the command-line tool `mklm-cli`.

### Build

Needs the Rust toolchain (the version is pinned in `rust-toolchain.toml`) and Visual Studio Build Tools (C++ and the Windows SDK).

```bat
cargo build --release
```

This produces `mklm-cli.exe` and `mklm-helper.exe` in `target\release\`. **Keep the two in the same folder**: `mklm-cli` starts the `mklm-helper.exe` next to it, elevated through UAC, and the helper performs every write.

### Usage

#### Reading (M1; no administrator rights needed, nothing is changed)

| Command | What it does |
|---|---|
| `mklm-cli list` | Connected keyboards with their stored type, the type Raw Input reports, and the layout actually in effect |
| `mklm-cli list --all` | Also keyboards that are not connected (phantoms) |
| `mklm-cli status [--json] [--all]` | Everything read from the system and how MKLM evaluates it |
| `mklm-cli global status [--json]` | PC-wide settings (mode, standard layout) and input-method warnings |
| `mklm-cli journal [--json]` | MKLM's record of operations (M2) |

#### Writing (M2; in progress, use on a test PC only)

A write command shows a UAC prompt and starts `mklm-helper.exe`. Before writing it shows the values that will change and what applying them takes (a keyboard reset, a reconnect, or a PC restart), and asks for confirmation.

| Command | What it does |
|---|---|
| `mklm-cli set <keyboard> --layout jis\|us\|standard` | Assign a layout to a keyboard (`standard` follows the PC's standard layout; external keyboards only) |
| `mklm-cli migrate [--standard jis\|us] [--also <keyboard>=<layout>]...` | Switch from fixed mode (every keyboard has the same layout) to per-keyboard mode; needs one PC restart |
| `mklm-cli undo` | Undo every change that waits for a confirmation, a restart, or a conflict decision. **The first command to try when something is wrong** |
| `mklm-cli revert <op>` | Undo one operation |
| `mklm-cli restore --baseline (--all \| <keyboard>)` | Put back the values from before MKLM |
| `mklm-cli recover` | Finish or undo operations that were interrupted (killed process, power loss) |
| `mklm-cli resolve <op>` | Decide what to do when a value MKLM wrote was changed by someone else (a conflict) |
| `mklm-cli keep <op>` | Keep an operation that waits for confirmation (alias `confirm`) |
| `mklm-cli reboot` | Restart the PC to apply pending changes |

- `<keyboard>` is an instance ID or `#n`, the row number in `mklm-cli list`. Instance IDs contain `&`, so quote them. In PowerShell `#` starts a comment, so write `"#2"`.
- Commands that may reset a keyboard accept `--other-input` (you have another keyboard or a mouse; a reset is fine) or `--no-reset` (never reset; apply on reconnect or restart). With neither, the CLI asks when a reset becomes necessary. `set` and `restore` require one of them with `--yes` (which skips the confirmation).
- When a USB keyboard is reset in place, a 20-second countdown starts after the reset. Type Shift+2 to check (`"` means JIS, `@` means US) and answer `y` to keep it. Without an answer it is reverted automatically.
- Exit codes: 0 done, 1 failed, 2 usage error, 3 cancelled, 4 reverted automatically, 5 conflict, 6 blocked, 10 waiting for a reconnect, 3010 a PC restart is needed.

Example: assign US to an external US keyboard (on a PC already in per-keyboard mode).

```bat
mklm-cli list
mklm-cli set "#2" --layout us --other-input
```

Example: on a PC in fixed mode (JIS), keep the built-in keyboard JIS and make an external keyboard US.

```bat
mklm-cli migrate --standard jis --also "#2=us"
mklm-cli reboot
```

After the restart and sign-in, a confirmation (`mklm-cli post-reboot`) opens; check with Shift+2 before answering.

### When something goes wrong

1. Run `mklm-cli undo`, then **restart** the PC.
2. If that does not help, work through [docs/recovery.md](docs/recovery.md) (Japanese) from the top: typing on the sign-in screen, Safe Mode, the recovery environment (WinRE), `restore-offline.cmd`.
3. Recovery files are created in `C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery\` before the first write. Its `README.txt` summarizes the steps in Japanese and English.

### Documents

| Document | Contents |
|---|---|
| [docs/recovery.md](docs/recovery.md) | Recovery guide (Japanese): what to do when you cannot sign in or a layout is wrong |
| [docs/design/m2-engine.md](docs/design/m2-engine.md) | M2 design (Japanese): writes, journal, recovery, helper protocol, CLI |
| [docs/research/m0-results.md](docs/research/m0-results.md) | M0 results on real hardware (Japanese) |

### License

[Apache License 2.0](LICENSE). Copyright SHIN DATA CENTER.
