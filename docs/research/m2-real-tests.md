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

## 未実施

| テスト | 内容 | 必要なもの |
|---|---|---|
| R5 | 書き込み途中の強制終了からの回復 | デバッグビルドの一時停止機能と、昇格した taskkill |
| R6 | 外部の変更との衝突 | 昇格した `reg add` |
| R8 | 移行の往復 | 再起動 3〜4 回 |
| R9 / R10 | 起動 ID の安定性 | シャットダウン、スリープ、休止、時刻の再同期 |
| R11 | パイプの防御 | — |
| R12 | UAC を断る | — |
| R13 | BLE | 本物の BLE キーボード |
| R14 / R15 | 別ユーザーでの確認 | VM か別アカウント |
| R16 | 昇格したコンソールでの Ctrl+C | — |
