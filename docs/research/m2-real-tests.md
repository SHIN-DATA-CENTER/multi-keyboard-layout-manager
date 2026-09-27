# M2 実機テストの結果

`docs/design/m2-engine.md` の H.2 に定めた実機テストの記録。開発機は Windows 11 Home 25H2（build 26200）。M0 の後なので、すでにキーボードごとモードになっている（内蔵 PS/2 は 7/2、Keychron は 4/0）。ビルドは `m2/write-path` の `945277c`（release）。各テストは、ユーザーの同意を得てから実行した。

## R1: 昇格と通信だけ（2026-09-27 22:14、合格）

- 実行: `mklm-cli recover --yes --no-reset`
- UAC → helper の起動 → パイプでの通信とハンドシェイク（ビルド ID の照合を含む）→ 回復の処理、が一通り動いた。「Nothing needed recovery.」と表示され、終了コードは 0。
- 作られたもの:
  - `C:\ProgramData\SHIN DATA CENTER` と `...\MKLM`
    - 所有者は `BUILTIN\Administrators`。上位フォルダーからの継承は切られている（`AreAccessRulesProtected=True`）。
    - SY と BA がフルコントロール、BU は RX。ProgramData から継承する BU の作成権と CREATOR OWNER は付かない。
  - `MKLM\mklm.lock`: 一般ユーザーでは `icacls` で読むことすらできない（アクセス拒否）。設計どおり。
- 作られなかったもの:
  - `HKLM\SOFTWARE\SHIN DATA CENTER`。回復する対象がないので、ジャーナルのキーは作らない。
  - HKCU の RunOnce への登録。
- キーボードの値は変わっていない。

## R3: 時間切れで自動的に元に戻る（22:19、合格）

- 実行: `mklm-cli set <Keychron の ID> --layout jis --other-input --yes`。標準入力は EOF で、答えない。
- 流れ:
  1. 復旧用ファイルを作成する。
  2. ジャーナルに記録する（`bfaca7cd`）。
  3. 書き込み、キーボードをリセットする。Raw Input は 0x7/0x2 になった。
  4. 20 秒のカウントダウンが終わる。
  5. 元の値に戻して、もう一度リセットする。Raw Input は 0x4/0x0 に戻った。
- 結果: 状態は `Reverted`（理由: no answer before the countdown ran out）。終了コードは 4（設計どおり）。所要時間は 29 秒。

## R2: 切り替えて「このままにする」（22:19、合格）

- 実行: `mklm-cli set <Keychron の ID> --layout jis --other-input --yes --answer keep`
- 流れ: ジャーナルに記録する（`1c2cc975`）→ 書き込み → リセット → Raw Input が 0x7/0x2 になったことを確かめて「このままにする」を選ぶ。
- 結果: 状態は `Confirmed`。終了コードは 0。所要時間は 5 秒。
- ユーザーの確認: Keychron の Shift+2 で `"` が出た。MKLM の操作で、実際に入力される文字まで JIS に切り替わっている。
- 観察: 同じレシーバーのマウス用コレクション（Col03）について、目立った影響の報告はない。このレシーバーにマウスはつないでいない。

## R4: 取り消しと「導入前に戻す」（22:22、合格）

- 実行: `mklm-cli revert 1c2cc975 --other-input --yes`
  - CAS で 7/2 から 4/0 に戻し、リセット後の Raw Input は 0x4/0x0 だった。
  - 状態は `Reverted`。終了コードは 0。
- 実行: `mklm-cli restore --baseline <Keychron の ID> --other-input --yes`
  - 「Nothing to restore: 2 value(s) already have their value from before MKLM.」と表示され、UAC は出なかった。終了コードは 0。
- ジャーナル: 2 件の操作と、baseline（4/0。`bfaca7cd` が記録した値）が正しく残っている。
- 気付いた点: ジャーナルの表示時刻が UTC になっている。M3 の GUI では現地時刻で表示したい。

## R7: 復旧用ファイルと ACL（22:24、合格）

- `MKLM\Recovery` にあるもの:
  - `README.txt`
  - `mklm-baseline.reg`: UTF-16LE の BOM 付き。Keychron の値を 4/0 に書き戻す内容。
  - `restore-offline.cmd`
- `restore-offline.cmd` の中身:
  - `Select\Default` から ControlSet を決め、`Select\Current` と違えば何も変えずに止まる。
  - Keychron の値を `reg add ... /d 4`、`/d 0` で書き戻す。
- `Recovery` の ACL: SY と BA がフルコントロール、BU は RX。ユーザーが USB メモリにコピーできる。

## R6: 外部の変更との衝突（22:25〜22:28、合格）

- **1 回目（テストの設計の誤り）**: JIS を確定したあと（操作 `75c67e35`）、外部から `KeyboardTypeOverride=4` を書いて `revert` した。
  - 外部で書いた値が、取り消しの戻し先（before = 4）と同じだった。そのため、MKLM はその値を「すでに目的の値」として書かなかった。
  - もう一方の Subtype は、MKLM が最後に書いた値（2）のままだったので、CAS で 0 に戻した。
  - 結果は `Reverted`。意図に反する上書きはないので、妥当な振る舞いと判断した。
- **2 回目**: JIS を確定したあと（操作 `371b1633`）、外部から `KeyboardTypeOverride=8` を書いた。戻し先とも、MKLM が書いた値とも違う値。
  - `mklm-cli revert 371b1633 --other-input --yes`: 衝突を検出し、何も書かずに止まった。表示は「now 8; MKLM last wrote 7; before 4, intended 7, baseline 4」。終了コードは 5。
    - 衝突していない Subtype も書き換えず、一部だけ戻すことはしない。
    - `resolve` と `undo` を案内する。
  - `mklm-cli resolve 371b1633 --all keep-current --other-input --yes`: 状態は `Failed`（the values changed outside MKLM were kept）。`last_written` は現在の値（8）になった。反映が済んでいないので、「再接続が必要」と表示する。
  - 後片付け: `mklm-cli set … --layout us` で、8/2 から 4/0 に書き換えてリセットした（操作 `31f7f7bc`、確定）。

## R11: パイプの防御（22:28〜22:30、合格）

- 昇格していない同じユーザーのプロセスから、helper 用のパイプ（`\\.\pipe\SHINDATACENTER.MKLM.<uuid>`）を開こうとした結果:
  - .NET の `NamedPipeClientStream`（InOut / In / Out / ReadData）: いずれも `UnauthorizedAccessException`。
  - `CreateFileW` の `WRITE_DAC` だけ / `WRITE_OWNER` だけ: いずれも拒否（Win32 エラー 5）。
- 不正な引数（`--pipe \\.\pipe\evil --nonce 00 --caller-pid 4`）で helper を昇格して起動すると、パイプに接続せず終了コード 2 で終わった。
- 補足: 1 回目の試行ではパイプを見つける前に処理が終わった。ユーザーが UAC ですぐ「はい」を押したためで、操作 `bc7cc73f` として JIS が確定した。これは `revert` で戻した。

## R12: UAC を断る（22:29、合格）

- 実行: `mklm-cli set <Keychron の ID> --layout jis …`。UAC で「いいえ」を選んだ。
- 結果: 「Cancelled: the administrator permission was declined; nothing was changed.」と表示され、終了コードは 3。ジャーナルの行数（12）もキーボードの値（4/0）も変わらなかった。
- 進め方の教訓: ユーザーの手順の説明と UAC の画面が同時に出ると、説明を読む前に「はい」を押してしまいやすい。「いいえ」を押してもらうテストは、先に AskUserQuestion で準備を確かめてから始める。

## R5: 書き込み途中の強制終了からの回復（22:31〜22:33、合格）

- 方法:
  - 昇格した PowerShell でデバッグビルドの CLI を動かし、`MKLM_DEBUG_PAUSE=after-first-write` を helper に引き継がせた。UAC 経由の起動ではユーザーの既定の環境変数しか渡らないので、この方法をとった。
  - 1 段目を書いた直後の一時停止中に、`taskkill /F /IM mklm-helper.exe` で helper を止めた。
- 一時停止中のレジストリ: 7/2（書き込み済みで、キーボードはまだリセットしていない）。
- CLI の動き:
  1. 「the helper stopped before it answered」を検出する。
  2. 「Recovering what the helper left unfinished now.」と表示し、新しい helper を起動する（昇格したコンソールからなので UAC は出ない）。
  3. ロールバックする。
  4. 「8e9a9970: planned (interrupted?) -> reverted (roll-back)」と表示する。終了コードは 4。
- 結果:
  - 値は 4/0 に戻った。Raw Input は 4/0 のまま（リセット前に止めたため）。
  - ジャーナルの状態は `Reverted`（the change was never kept after the keyboard reset; recovery put it back）。
  - 回復用の helper にも一時停止の指定が引き継がれたので、回復に 2 分かかった（想定どおり）。
- 気付いた点（M3 で直す）: 回復の理由の文言が「after the keyboard reset」になっているが、実際にはリセットの前に止めている。

## 未実施

| テスト | 内容 | 必要なもの |
|---|---|---|
| R8 | 移行の往復 | 再起動 3〜4 回 |
| R9 / R10 | 起動 ID の安定性 | シャットダウン、スリープ、休止、時刻の再同期 |
| R13 | BLE | 本物の BLE キーボード |
| R14 / R15 | 別ユーザーでの確認 | VM か別アカウント |
| R16 | 昇格したコンソールでの Ctrl+C | ユーザーの操作 |
