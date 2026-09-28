# MKLM 復旧ガイド

MKLM（Multi Keyboard Layout Manager）でキーボードの設定を変えた後に、困ったことが起きたときの戻し方をまとめる。上の節ほど簡単で安全な方法なので、上から順に試す。

| 項目 | 内容 |
|---|---|
| 対象 | MKLM 0.1（マイルストーン M2: 書き込みと安全網）。Windows 11 24H2（build 26100）以降 |
| 読み手 | MKLM を使う人。コマンドプロンプトを開いて、書いてあるとおりに入力できれば足りる |
| 根拠 | `docs/design/m2-engine.md`（とくに C、D、G 章）、`docs/research/m0-results.md` |

> **この文書のコマンドの一部は、PC 全体のレジストリを書き換える。** 書いてあるコマンドを、書いてあるとおりに実行すること。とくに、レジストリの**キー**を消すコマンド（`/v` のない `reg delete`）は絶対に実行しない。

---

## 1. この文書を使う場面

- **サインインできない。** パスワードに記号が入っていて、サインイン画面でその記号が打てない。
- **キーの配列がおかしい。** 例えば Shift+2 で `"` が出るはずなのに `@` が出る（またはその逆）。内蔵キーボードも外付けキーボードも同じ配列になってしまった。
- **MKLM が起動しない、またはエラーで止まる。** 「回復が必要です」「別の MKLM が処理中です」「衝突」などと表示される。
- **「まだ反映されていません」と表示され続ける。**

どの方法を使うかは、次の表で決める。

| 今の状況 | 使う方法 | 節 |
|---|---|---|
| Windows にサインインできる | `mklm-cli undo`。だめなら `recover`、`restore --baseline --all` | [2](#2-まず試すこと)、[3](#3-windows-にサインインできる場合) |
| サインイン画面でパスワードが打てない | PIN、指紋、顔、スクリーンキーボード、別のキーボード | [4](#4-サインイン画面で入力できない場合) |
| サインインはできるが、通常の Windows では MKLM が動かない | セーフモードで `mklm-cli undo` か `restore-offline.cmd online` | [5](#5-セーフモードでの復旧) |
| Windows にサインインする方法がない、Windows が起動しない | 回復環境（WinRE）で `restore-offline.cmd offline D:`、または手作業 | [6](#6-回復環境winreでの手動復旧) |
| MKLM の自動更新が途中で止まった、更新の後に MKLM が起動しない、更新がいつも止まる | インストーラーを実行し直す。止めているプログラムを終わらせる | [9](#9-自動更新で困ったとき) |

---

## 2. まず試すこと

**コマンドプロンプト（管理者でなくてよい）で次の 1 行を実行し、終わったら PC を「再起動」する。**

```bat
mklm-cli undo
```

- 操作 ID を知らなくても、**確認待ち、再起動待ち、衝突**の変更をまとめて取り消す。途中で止まった操作（強制終了や電源断）があれば、先にそれを回復する。
- UAC の確認画面が出る。MKLM はコード署名をしていないので、発行元は「不明」と表示される。プログラム名が **mklm-helper.exe** であることを確かめて「はい」を押す。
- 取り消した値は、次の PC の**再起動**で確実に効く。「シャットダウン」→ 電源オンでは効かないことがある（高速スタートアップのため。[8 章](#8-よくある質問)）。スタート → 電源 → **再起動** を選ぶか、`mklm-cli reboot` を使う。
- 別のキーボードかマウスで操作できるなら、`mklm-cli undo --other-input` にすると、USB キーボードをその場でリセットして元の配列をすぐに反映する。付けなければリセットはせず、「まだ反映されていません: 〜してください」と案内が出る（抜き差しか再起動で反映される）。

`mklm-cli` は、`mklm-cli.exe` と `mklm-helper.exe` を置いたフォルダーで実行する（2 つは同じフォルダーに置く）。M2 の時点ではインストーラーがないので、ビルドした場合は `target\release` になる。以下の例も同じ前提で書く。

```bat
cd /d "<mklm-cli.exe のあるフォルダー>"
mklm-cli undo
```

---

## 3. Windows にサインインできる場合

### 3.1 GUI の「元に戻す」

GUI（M3 で提供予定）では、「元に戻す」ボタンが `mklm-cli undo` と同じ働きをする。M2 の時点では GUI はまだないので、次の CLI を使う。

### 3.2 CLI で戻す

上から順に試す。どれもコマンドプロンプト（管理者でなくてよい）で実行でき、書き込むときは UAC の確認画面が出る。

| 順 | コマンド | 何をするか |
|---|---|---|
| 1 | `mklm-cli undo` | 確認待ち、再起動待ち、衝突の変更をまとめて取り消す（2 章） |
| 2 | `mklm-cli recover` | 強制終了、電源断、ウィンドウを閉じた、などで途中で止まった操作を回復する。何も戻さなかったのに確認待ちの操作が残っていれば、そう表示して `undo` を案内する |
| 3 | `mklm-cli journal` | 操作の一覧と状態を見る（読み取りだけ）。操作 ID もここで分かる |
| 4 | `mklm-cli revert <操作 ID>` | 操作を 1 つ指定して取り消す。「このままにする」で確定した操作も対象になる（`undo` は確定した操作を取り消さない）。その値を最後に変えた操作だけを取り消せる。操作 ID は 8 桁以上の先頭部分でもよい |
| 5 | `mklm-cli restore --baseline --all` | MKLM が変えたすべての値を、**MKLM が初めて変える前の値**（導入前の値）に戻す |
| 6 | `mklm-cli resolve <操作 ID>` | 「衝突」の操作を、値ごとにどうするか決める（3.4） |

- `undo`、`recover`、`revert`、`resolve` は、`--other-input` を付けない限りキーボードをリセットしない。値は戻り、反映は抜き差しか再起動になる。
- `restore --baseline` は、書く前に変わる値を表示して確認を求める。USB キーボードの値を戻すときは「別のキーボードかマウスとスクリーンキーボードで入力できますか? [y/N]」とも尋ねる。分からなければ `N`（リセットしない）でよい。`--yes` で確認を省く場合は、`--other-input` か `--no-reset` のどちらかも付ける。
- `restore --baseline` は、MKLM 以外（Windows の設定アプリ、ほかの管理者、Windows Update など）が値を変えていると、**何も書かずに**止まって値の一覧を表示する（`--on-conflict report`、既定）。一覧を見たうえで、その値を飛ばすなら `--on-conflict skip`、導入前の値で上書きするなら `--on-conflict overwrite` を付けて実行し直す。
- 確定した「導入前に戻す」自体は取り消せない。元の配列にしたいときは、もう一度 `mklm-cli set` で割り当てる。

**再起動後の確認画面**: PC の再起動が必要な変更（移行、内蔵キーボードの変更など）の後は、サインインすると `mklm-cli post-reboot` が自動で開き、「このままにしますか? [y/N]」と尋ねる。配列がおかしければ `n` を入力すると取り消す。Enter だけなら何もせず、次のサインインでまた尋ねる。**自動では元に戻さない。**

### 3.3 最後に PC を再起動する

内蔵キーボード（PS/2）と PC 全体の値は、**PC の再起動でしか読まれない**。戻した後は、スタート → 電源 → **再起動** を選ぶ（または `mklm-cli reboot`）。「シャットダウン」して電源を入れ直すのは再起動の代わりにならない。高速スタートアップが有効だと、ドライバーが値を読み直さないため。

### 3.4 「衝突（Conflict）」と表示された場合

MKLM は、自分が書いた値がその後で**ほかの誰かに変えられている**と、上書きせずに止まる。これが「衝突」。原因の例は、Windows の設定アプリでキーボードの種類を変えた、ほかの管理者が変えた、[6 章](#6-回復環境winreでの手動復旧)の手作業や `restore-offline.cmd` で戻した、など。

```bat
mklm-cli resolve <操作 ID>
```

値の一覧が表示され、値ごとに次から選ぶ。`--all <選択>` ですべての値に同じ選択を、`--value <番号>=<選択>` で値ごとの選択を指定することもできる。

| 選択 | 値をどうするか |
|---|---|
| `keep-current` | 今の値をそのまま残す（外部の変更を受け入れる） |
| `before` | その操作の前の値に戻す |
| `intended` | その操作が書こうとした値にする |
| `baseline` | MKLM を使う前の値に戻す |

`undo` は衝突している値を書き換えない（ほかの値は戻す）。衝突が残った場合は `resolve` で決める。内蔵キーボードの固定値がないまま PC 全体の固定が外れる組み合わせ（内蔵キーボードが US になる）は、MKLM が拒否する。

### 3.5 MKLM のコマンドが使えない場合

`mklm-cli` がエラーで止まる、`mklm-helper.exe` が起動しない、などの場合は、[5 章](#5-セーフモードでの復旧)と同じく `restore-offline.cmd online` を管理者のコマンドプロンプトで実行する（通常の Windows でもセーフモードでも使える）。

```bat
cd /d "C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery"
restore-offline.cmd online
```

同じフォルダーの `mklm-baseline.reg` をダブルクリックして取り込んでもよい（UAC の確認画面が出る）。どちらも MKLM の記録（ジャーナル）を見ずに導入前の値を書くので、次に MKLM を使うと「衝突」が表示される。そのときは [6.8](#68-オフラインで戻した後) のとおり `keep-current` を選ぶ。

### 3.6 終了コードと次の手順

書き込みのコマンドの終了コード（`echo %ERRORLEVEL%` で見られる）と、その後にすること。

| コード | 意味 | 次の手順 |
|---|---|---|
| 0 | 完了（確定した、取り消した、変更なし、回復した） | 必要なら再起動 |
| 1 | 失敗。何も変わっていないか、エラーのためにすべて戻した | 表示されたメッセージを見る |
| 2 | 使い方の誤り | コマンドを見直す |
| 3 | 取り消した（UAC で「いいえ」、確認で「いいえ」など） | 何も書かれていない |
| 4 | 自動で元に戻した（カウントダウン切れ、キーボードが戻らない、など） | そのままでよい |
| 5 | 衝突 | 3.4 |
| 6 | 止められた（ほかの MKLM が処理中、確認待ちの操作がある、回復が必要） | しばらく待つか、`mklm-cli undo` |
| 10 | 書いた。再接続して確定か取り消しを待っている | キーボードを抜き差しか再接続し、`mklm-cli keep <操作 ID>` か `revert <操作 ID>` |
| 3010 | PC の再起動が必要 | **再起動**する（シャットダウンではない） |

---

## 4. サインイン画面で入力できない場合

サインイン画面のキーボードの配列も、PC 全体の設定に従う。MKLM で配列を変えたキーボードでは、パスワードの記号が思っていたのと違う文字になることがある。次の順に試す。

1. **PIN、指紋、顔**（Windows Hello）でサインインする。**数字だけの PIN** は、JIS と US で数字キーの位置が同じなので、配列の影響を受けない。
2. **スクリーンキーボード**を使う。サインイン画面の右下の「アクセシビリティ」ボタン → 「スクリーン キーボード」をオンにして、マウスかタッチで入力する。
3. **別のキーボード**を USB でつなぐ。MKLM で配列を割り当てていないキーボードは、PC の標準配列で動く。
4. **記号の位置を読み替えて入力する**。4.1 の表を使う。パスワード欄の右端の目のアイコン（「パスワードの表示」）を押している間は、入力した文字が見えるので、合っているか確かめられる。

それでもサインインできなければ、[6 章](#6-回復環境winreでの手動復旧)（WinRE）に進む。[5 章](#5-セーフモードでの復旧)のセーフモードもサインインが必要で、PIN や指紋が使えずパスワードを求められることがあるので、パスワードが打てないときの助けにはならない。

サインイン画面の入力方式が英語（US）になっている PC では、MKLM の設定に関係なく、サインイン画面ではすべてのキーボードが US として動く。`mklm-cli global status` で確かめられる。

### 4.1 JIS と US の記号の対応

数字、英字、`1 !`、`3 #`、`4 $`、`5 %`、`,`、`.`、`/` は、どちらでも同じ位置にある。

キーは、数字キーと英字キーからの位置で示す。「JIS として動くとき」は日本語キーボード（106/109）として扱われているとき、「US として動くとき」は英語キーボード（101/104）として扱われているとき。左が Shift なし、右が Shift あり。

| キーの位置 | JIS として動くとき | US として動くとき |
|---|---|---|
| 2 | `2` / `"` | `2` / `@` |
| 6 | `6` / `&` | `6` / `^` |
| 7 | `7` / `'` | `7` / `&` |
| 8 | `8` / `(` | `8` / `*` |
| 9 | `9` / `)` | `9` / `(` |
| 0 | `0` /（何も出ない） | `0` / `)` |
| 0 の右隣 | `-` / `=` | `-` / `_` |
| 0 の右の 2 つ目 | `^` / `~` | `=` / `+` |
| P の右隣 | `@` / `` ` `` | `[` / `{` |
| P の右の 2 つ目 | `[` / `{` | `]` / `}` |
| L の右隣 | `;` / `+` | `;` / `:` |
| L の右の 2 つ目 | `:` / `*` | `'` / `"` |
| JIS キーボードの `]` キー（L の右の 3 つ目）、US キーボードの `\` キー（P の右の 3 つ目、Enter の上） | `]` / `}` | `\` / `\|` |
| 1 の左 | 半角/全角（IME の切り替え。文字は出ない） | `` ` `` / `~` |
| JIS キーボードだけにある `￥` キー（BackSpace の左） | `\` / `\|` | 何も出ないことがある |
| JIS キーボードだけにある `ろ` キー（右 Shift の左） | `\` / `_` | 何も出ないことがある |

例: US 配列のキーボードが JIS として動いていると、`@` を打つには「P の右隣」のキー（刻印は `[`）を押す。JIS 配列のキーボードが US として動いていると、`"` を打つには Shift を押しながら「L の右の 2 つ目」のキー（刻印は `:`）を押す。

---

## 5. セーフモードでの復旧

通常の Windows で MKLM のコマンドが動かない場合に使う。セーフモードでもキーボードの配列の設定は同じなので、サインインするにはパスワードを入力できる必要がある（PIN や指紋が使えないことがある）。

1. **セーフモードで起動する。** サインイン画面の右下の電源ボタン → **Shift を押しながら「再起動」** → 「トラブルシューティング」→「詳細オプション」→「スタートアップ設定」→「再起動」→ 再起動後の一覧で **4**（または F4）を押す。サインインできる場合は、設定 → システム → 回復 → 「PC の起動をカスタマイズする」の「今すぐ再起動」からも同じ画面に進める。BitLocker の回復キーを求められたら入力する（[6.2](#62-bitlockerデバイスの暗号化)）。
2. 管理者のアカウントでサインインする。
3. **管理者のコマンドプロンプトを開く。** スタートで「cmd」と入力し、「管理者として実行」を選ぶ。
4. MKLM のコマンドを、上から 1 つずつ、直るまで実行する。昇格済みのコンソールなので UAC の確認画面は出ない。この場合も `mklm-helper.exe` は別のプロセスとして動くので、ウィンドウを閉じたり Ctrl+C を押したりしても、変更が途中で放置されることはない。

   ```bat
   cd /d "<mklm-cli.exe のあるフォルダー>"
   mklm-cli undo
   mklm-cli recover
   mklm-cli restore --baseline --all --no-reset
   ```

   セーフモードではキーボードをその場でリセットしない（`--no-reset`）。どのみち最後に再起動するので、それで反映される。

5. MKLM のコマンドが動かなければ、MKLM の記録を見ずに導入前の値に戻すスクリプトを使う。`Done. Restart Windows (a restart, not a shutdown).` と表示されれば完了。このスクリプトを使った場合は、後で [6.8](#68-オフラインで戻した後) の後始末をする。

   ```bat
   cd /d "C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery"
   restore-offline.cmd online
   ```

6. 通常どおり**再起動**する。

---

## 6. 回復環境（WinRE）での手動復旧

Windows にサインインする方法がないとき、または Windows が起動しないときに使う。WinRE は Windows 本体のレジストリを使わずに動くので、MKLM の設定の影響を受けない。ここでは、Windows 本体の SYSTEM ハイブ（キーボードの設定が入っているファイル）を読み込んで、値を直接書き換える。

最初に次の 3 点を知っておく。

- **いちばん簡単で確実なのは `restore-offline.cmd`（6.5）。** MKLM が変えた値だけを、この PC で記録した導入前の値に、安全な順序で戻す。
- **サインインできるようにするための最小限の手順は、PC 全体の設定を「固定モード」（日本語キーボード 106/109）に戻すことだけ**（6.6 の手順 4）。固定モードでは、内蔵の PS/2 キーボードも外付けのキーボードも JIS として動く。ただし、内蔵キーボードに MKLM で US を割り当てていた場合は、内蔵キーボードの値も JIS に書き換える（6.6 の手順 5）。
- Windows のドライブに `ProgramData\SHIN DATA CENTER\MKLM\Recovery` がなければ、MKLM は一度も値を書いていない。MKLM が変えたものはない。

### 6.1 WinRE を起動する

1. サインイン画面の右下の電源ボタン → **Shift を押しながら「再起動」**。Windows が起動しない場合は、起動に続けて失敗すると自動で「自動修復」の画面になるので、「詳細オプション」を選ぶ。
2. 「キーボード レイアウトの選択」が表示されたら、**これから使うキーボードに合う配列**を選ぶ（JIS の内蔵キーボードなら日本語、US のキーボードなら US）。
3. 「オプションの選択」→「トラブルシューティング」→「詳細オプション」→「コマンド プロンプト」。
4. 管理者のアカウントとパスワードを求められたら入力する。

WinRE で入力される記号は、手順 2 で選んだ配列で決まる。コマンドを打つ前に **Shift+2** を押して、`"` が出れば JIS、`@` が出れば US として動いている。コマンドに出てくる記号の位置は次のとおり（`-`、`/`、`.`、数字は同じ位置）。

| 記号 | JIS として動くとき | US として動くとき |
|---|---|---|
| `"` | Shift+2 | Shift+`'`（L の右の 2 つ目。Enter の左） |
| `\` | `￥` キー（BackSpace の左）か `ろ` キー（右 Shift の左） | `\` キー（Enter の上） |
| `:` | `:` キー（L の右の 2 つ目） | Shift+`;`（L の右隣） |
| `_` | Shift+`ろ` キー（右 Shift の左） | Shift+`-`（0 の右隣） |
| `&` | Shift+6 | Shift+7 |

US 配列のキーボードが JIS として動いていると、`\` と `_` を打つキーがない（US キーボードには `￥` キーと `ろ` キーがない）。その場合は手順 2 で US を選び直すか、JIS のキーボード（内蔵キーボードなど）を使う。

### 6.2 BitLocker（デバイスの暗号化）

Windows のドライブが暗号化されていると、WinRE では**回復キー**（6 桁 × 8 組、48 桁の数字）を求められる。

- 回復キーの場所: Microsoft アカウントの「デバイス」→「回復キーの表示」。別の PC かスマートフォンで <https://aka.ms/myrecoverykey> を開いてサインインすると表示される。印刷して保管した紙、職場や学校の PC なら管理者にも問い合わせられる。
- 回復キーは数字とハイフンだけなので、JIS と US の違いの影響を受けない。
- 回復キーの入力を求められなかった（「このドライブをスキップする」を選んだ）のにドライブが読めない場合は、コマンド プロンプトでロックを解除する。`manage-bde -status` で「ロック状態: ロック」（Lock Status: Locked）になっているドライブの文字を `C:` の代わりに、自分の回復キーを例の数字の代わりに入れる。

  ```bat
  manage-bde -status
  manage-bde -unlock C: -RecoveryPassword 111111-222222-333333-444444-555555-666666-777777-888888
  ```

### 6.3 Windows のドライブ文字を確かめる

WinRE では、Windows のドライブが `C:` でないことがある（`D:` になることが多い。`X:` は WinRE 自身）。次のように確かめる。

```bat
dir C:\Windows\System32\config\SYSTEM
dir D:\Windows\System32\config\SYSTEM
```

ファイルが表示された方が Windows のドライブ。**以下の例はすべて `D:` で書くので、違う場合は読み替える。**

### 6.4 MKLM の復旧用ファイルの場所

MKLM は、値を書く前に復旧用ファイルを次のフォルダーに作り、導入前の値が変わるたびに書き直す。

```text
<Windows のドライブ>:\ProgramData\SHIN DATA CENTER\MKLM\Recovery\
```

| ファイル | 内容 |
|---|---|
| `restore-offline.cmd` | MKLM が変えたキーボードの値を、導入前の値に戻すスクリプト。`online`（Windows かセーフモード）と `offline D:`（WinRE）の 2 通りで使う |
| `mklm-baseline.reg` | 同じ値のレジストリファイル。Windows 上でダブルクリックして取り込む（WinRE では使わない） |
| `README.txt` | 手順の要約と、記録した値の一覧（インスタンス ID を含む）。日本語と英語 |
| `*.prev` | それぞれの 1 つ前の版（控え） |

WinRE では `notepad` で中身を読める（例: `notepad "D:\ProgramData\SHIN DATA CENTER\MKLM\Recovery\README.txt"`）。

**USB メモリなどにコピーしたファイルは実行しない。** コピーは、インスタンス ID と手順を読むための控えとして持っておく。USB メモリ上のファイルは誰でも書き換えられるうえ、WinRE では SYSTEM 権限で実行されるため。実行するのは、Windows のドライブ上の `...\ProgramData\SHIN DATA CENTER\MKLM\Recovery\restore-offline.cmd` だけにする。

### 6.5 スクリプトを使う場合

パスに空白が入っているので、フォルダーに移ってから実行する。

```bat
cd /d "D:\ProgramData\SHIN DATA CENTER\MKLM\Recovery"
restore-offline.cmd offline D:
```

- 最初の行の `"` は、JIS 配列では **Shift+2**、US 配列では **Shift+'**（Enter の左）で打つ（6.1 の表）。
- 2 行目の `D:` は、スクリプトのあるフォルダーではなく、**Windows のドライブ**を指す。

スクリプトは、Windows の SYSTEM ハイブを `HKLM\MKLM_OFFLINE` に読み込み、`Select\Default`（次の起動で使われる制御セット）から `ControlSet00N` を求め、記録した値を安全な順序（PS/2 の値の設定 → 全体の値の設定 → HID キーボード → 全体の値の削除 → PS/2 の値の削除）で戻してから、ハイブを外す。

| 表示 | 終了コード | 意味と次の手順 |
|---|---|---|
| `Done. Restart Windows (a restart, not a shutdown).` | 0 | 完了。`exit` と入力し、「続行」で Windows を起動する |
| `Some values could not be restored.` | 1 | 戻せなかった値がある。`notepad restore-offline.cmd` で `Record #` を含む行を探す（安全に書けない文字を含む値や、cmd では扱えない型の値は、スクリプトに入れずにそう書いてある）。サインインできるようにするには、6.6 の手順 1、2、4、5、7 を行う |
| `usage: restore-offline.cmd online \| offline D:` | 2 | 引数が足りない。`offline D:` まで入力する |
| `SYSTEM hive not found under D:\Windows` | 2 | ドライブ文字が違うか、BitLocker でロックされている（6.2、6.3） |
| `reg load failed` | 3 | ハイブを読み込めない。前回の実行で読み込んだままなら `reg unload HKLM\MKLM_OFFLINE` を実行してからやり直す |
| `Select\Default not found` | 4 | SYSTEM ハイブの中身が想定と違う。何も書いていない。6.7 |
| `Select\Default is 1 but Select\Current is 2. Nothing was changed; see docs\recovery.md.` | 5 | 次の起動で使う制御セットと、前回の起動で使った制御セットが違う。何も書いていない。6.7 |

### 6.6 手作業の場合

スクリプトがない、またはスクリプトが使えない場合の手順。**書き込む前に必ず手順 2 で制御セットを確かめる。** オフラインでは `CurrentControlSet` は存在しないので、`ControlSet001` などの実際の名前を使う。

- パスは必ず `"` で囲む（インスタンス ID に `&` が、値の名前 `LayerDriver JPN` に空白が入っているため）。
- `reg delete` には必ず `/v <値の名前>` を付ける。`/v` のない `reg delete` はキーそのものを消してしまう。
- ここに書いた値以外は変えない。とくに `Services\i8042prt` の `Start` を変えると、内蔵キーボードが動かなくなる。

#### 手順 1: SYSTEM ハイブを読み込む

```bat
reg load HKLM\MKLM_OFFLINE D:\Windows\System32\config\SYSTEM
```

`この操作を正しく終了しました。`（The operation completed successfully.）と表示されれば成功。

#### 手順 2: 制御セットの番号を確かめる

```bat
reg query HKLM\MKLM_OFFLINE\Select
```

```text
HKEY_LOCAL_MACHINE\MKLM_OFFLINE\Select
    Current    REG_DWORD    0x1
    Default    REG_DWORD    0x1
    Failed    REG_DWORD    0x0
    LastKnownGood    REG_DWORD    0x1
```

`Default` の値が使う番号（`0x1` なら `ControlSet001`、`0x2` なら `ControlSet002`）。**`Default` と `Current` が違う場合は、ここで作業をやめる**（`reg unload HKLM\MKLM_OFFLINE` を実行してから 6.7）。以下の例は `ControlSet001` で書く。

#### 手順 3: 今の値を見る（読むだけ）

```bat
reg query "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters"
reg query "HKLM\MKLM_OFFLINE\ControlSet001\Enum" /s /v OverrideKeyboardType
reg query "HKLM\MKLM_OFFLINE\ControlSet001\Enum" /s /v KeyboardTypeOverride
```

- 1 行目: `OverrideKeyboardType` と `OverrideKeyboardSubtype` がなければ「キーボードごとモード」、あれば「固定モード」（[付録 A](#付録-a-mklm-が書き換える値)）。
- 2 行目: 内蔵の PS/2 キーボードの値（`ACPI\...` のキー）。`0x4` なら US、`0x7` なら JIS が割り当てられている。
- 3 行目: 外付けの HID キーボードの値（`HID\...` のキー）。
- 見つからなければ `End of search: 0 match(es) found.`（または日本語の同じ意味のメッセージ）と表示される。

#### 手順 4: PC 全体を固定モード（日本語キーボード 106/109）に戻す

サインインできるようにするための最小限の手順。

```bat
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters" /v "LayerDriver JPN" /t REG_SZ /d kbd106.dll /f
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters" /v OverrideKeyboardIdentifier /t REG_SZ /d PCAT_106KEY /f
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters" /v OverrideKeyboardType /t REG_DWORD /d 7 /f
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Services\i8042prt\Parameters" /v OverrideKeyboardSubtype /t REG_DWORD /d 2 /f
```

- これで PC の標準配列が JIS になり、外付けのキーボードはすべて JIS で動く。固定モードでは、外付けキーボードのキーボードごとの値は使われない（MKLM の M0 の検証で確認済み）。
- 内蔵の PS/2 キーボードは、キーボードごとの値がないか 7 / 2 なら JIS で動く。手順 3 で 4 / 0（US）が割り当てられていたら、手順 5 も行う。
- 内蔵キーボードが US 配列の PC で、もともと英語キーボード（101/102）の固定モードだった場合は、代わりに `kbd101.dll`、`PCAT_101KEY`、`7`、`0` を書く（Windows の設定で英語キーボードを選んだときに使われる値として知られている組み合わせ。MKLM の開発機では確かめていない）。

#### 手順 5: 内蔵キーボードに US を割り当てていた場合は JIS にする

手順 3 の 2 行目で、内蔵の PS/2 キーボードの `OverrideKeyboardType` が `0x4`（US）だった場合だけ行う。値を 7 / 2（JIS）に書き換える。固定値を**書く**のは、どのモードでも安全。

```bat
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v OverrideKeyboardType /t REG_DWORD /d 7 /f
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v OverrideKeyboardSubtype /t REG_DWORD /d 2 /f
```

`<インスタンス ID>` は、手順 3 で表示されたキーのうち `...\Enum\` と `\Device Parameters` の間の部分（例: `ACPI\FUJ0309\4&320DB4C2&0`）。

#### 手順 6（必要なときだけ）: キーボードごとの値を導入前に戻す

固定モードに戻せばサインインには足りるので、ふつうは不要。キーボードごとの値も MKLM の導入前の状態に戻したいときだけ行う。値の一覧（キーのパスと、導入前の値）は Recovery フォルダーの `README.txt` にある。`(値なし: 削除 / absent: delete)` と書いてある値は消し、`REG_DWORD 4 (0x4)` などと書いてある値はその値を書く。

外付けの HID キーボード（USB、Bluetooth など）の値を消す:

```bat
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v KeyboardTypeOverride /f
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v KeyboardSubtypeOverride /f
```

外付けの HID キーボードに導入前の値（例: 4 / 0）を書く:

```bat
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v KeyboardTypeOverride /t REG_DWORD /d 4 /f
reg add "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v KeyboardSubtypeOverride /t REG_DWORD /d 0 /f
```

内蔵の PS/2 キーボードの値を消す（**手順 4 で固定モードに戻した後だけ**。キーボードごとモードのまま消すと、内蔵キーボードが US になる）:

```bat
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v OverrideKeyboardType /f
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\<インスタンス ID>\Device Parameters" /v OverrideKeyboardSubtype /f
```

例（MKLM の開発機の Keychron レシーバーと、Fujitsu の内蔵キーボード）:

```bat
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters" /v KeyboardTypeOverride /f
reg delete "HKLM\MKLM_OFFLINE\ControlSet001\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" /v OverrideKeyboardType /f
```

値が最初からなければ「指定されたレジストリ キーまたは値が見つかりませんでした」と表示される。それで問題ない。

#### 手順 7: ハイブを外して Windows を起動する

```bat
reg unload HKLM\MKLM_OFFLINE
exit
```

`reg unload` が失敗する場合は、`notepad` や `regedit` などのウィンドウを閉じてからもう一度実行する。**ハイブを外すまで、書いた値はファイルに保存されない。** `exit` の後、「続行」を選んで Windows を起動する（これは完全な起動なので、書いた値が読まれる）。

### 6.7 `Select\Default` と `Select\Current` が違う場合

次の起動で使われる制御セット（`Default`）と、前回の起動で使われた制御セット（`Current`）が違う。ふつうは起きない状態で、どちらに書けば効くかを確実には決められない。**WinRE では何も書かない**（`restore-offline.cmd` もこの場合は何も書かずに止まる）。

- ハイブを読み込んだままなら、`reg unload HKLM\MKLM_OFFLINE` で外す。
- パスワードでサインインできるなら、セーフモード（[5 章](#5-セーフモードでの復旧)）で `restore-offline.cmd online` を使う。起動中の Windows が実際に使っている制御セット（`CurrentControlSet`）に書くので、この問題は起きない。
- それもできなければ、作業をやめて相談する（<https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/issues>。`reg query HKLM\MKLM_OFFLINE\Select` の結果を添える）。

### 6.8 オフラインで戻した後

`restore-offline.cmd`、`mklm-baseline.reg`、手作業のどれかで戻した場合、MKLM の記録（ジャーナル）には、確認待ちや再起動待ちの操作が残っていることがある。次に MKLM を使うと、MKLM が書いた値が変わっているので「衝突」と表示される。そのときは**今の値を残す**を選ぶ。

```bat
mklm-cli journal
mklm-cli resolve <操作 ID> --all keep-current
```

操作 ID は `mklm-cli journal` で分かる（8 桁以上の先頭部分でもよい）。

---

## 7. 事前の準備

MKLM で書き込む前に、次を済ませておく。

- **サインインの手段を 2 つ以上**: 数字だけの PIN を設定する。サインイン画面でスクリーンキーボードを出せることを確かめる。可能なら予備の USB キーボードを用意する。
- **BitLocker の回復キー**: <https://aka.ms/myrecoverykey> で表示できることを確かめ、紙に控えるか別の端末で見られるようにしておく。管理者のコマンドプロンプトで `manage-bde -protectors -get C:` を実行しても表示される。
- **管理者アカウントのパスワード**: セーフモードと WinRE で必要になることがある。記号を含むなら、[4.1](#41-jis-と-us-の記号の対応) の表でその記号の位置を確かめておく。
- **Recovery フォルダーの控え**: 最初の書き込みの後、`C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery` を USB メモリなどにコピーしておく（読むための控え。実行はしない）。
- **自分でのバックアップ**: 念のため、MKLM が変える値を書き出しておく。書き出すだけなので、管理者でなくても実行できる。キーボードごとの値は、`mklm-cli list --all` でインスタンス ID を確かめてから、`HKLM\SYSTEM\CurrentControlSet\Enum\<インスタンス ID>\Device Parameters` を同じように書き出す。

  ```bat
  reg export "HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters" i8042prt-Parameters.reg
  ```

- **WinRE の出し方**: この文書の [6.1](#61-winre-を起動する) を印刷するか、別の端末で読めるようにしておく。

---

## 8. よくある質問

### 再起動したのに配列が変わらない

「シャットダウン」→ 電源オンは再起動の代わりにならない。高速スタートアップが有効だと、シャットダウンは休止に近い動作になり、ドライバーが値を読み直さない。スタート → 電源 → **再起動** を選ぶ。`mklm-cli post-reboot` が「まだ反映されていません」と表示するのもこの場合。

### 「まだ反映されていません: 〜してください」と表示される

保存した値と、キーボードが今使っている配列が食い違っているかもしれない、という意味。書き込みは止めない。USB や Bluetooth のキーボードは抜き差しか再接続（Bluetooth は電源のオフとオン）、内蔵キーボードや PC 全体の値は PC の再起動で反映される。反映されると表示は消える。

### 「衝突（Conflict）」と表示された

MKLM が書いた値を、ほかの誰かが変えている（[3.4](#34-衝突conflictと表示された場合)）。`mklm-cli resolve <操作 ID>` で値ごとに決める。オフラインで戻した後なら `keep-current`（[6.8](#68-オフラインで戻した後)）。

### 「別の MKLM が処理中です」と表示される

ほかの MKLM（別のウィンドウの CLI など）が書き込み中。カウントダウン中なら最大 25 秒ほど待つ。`mklm-helper.exe` が固まっている場合は、タスク マネージャーで終了してから `mklm-cli recover` を実行する。

### 確認待ちや再起動待ちの操作があって、`set` ができない

確認待ちや再起動待ちの操作がある間は、新しい変更を始めない（安全のため）。PC を再起動して確認画面で決めるか、`mklm-cli undo` で取り消す。

### helper を起動できない、「helper の版が違います」と表示される

`mklm-cli.exe` と `mklm-helper.exe` は、同じフォルダーに置いた同じビルドのものを使う（開発中は両方をビルドし直す）。管理者のコマンドプロンプトでは、最後の手段として `mklm-cli recover --in-process`（`undo` と `restore` にも使える）で、helper を使わずに回復できる。この方法では、カウントダウン中にウィンドウを閉じたときの安全網が弱くなるので、ふだんは使わない。

### UAC の確認画面で発行元が「不明」になっている

MKLM はコード署名をしていないため。プログラム名が `mklm-helper.exe` であることを確かめてから「はい」を押す。

### 「導入前に戻す」を取り消したい

確定する前（確認待ち、再起動待ち）なら `mklm-cli undo` か `revert` で取り消せる。確定した後は取り消せないので、もう一度 `mklm-cli set` で配列を割り当てる。

### 「導入前の値」はいつの値か。MKLM を使う前に手で変えた値はどうなるか

MKLM が**初めて**その値を変える直前の値。MKLM を使う前に手で変えた値（MKLM の開発機では M0 の実験で書いた値）は、「導入前」に含まれる。それより前の状態に戻すには、そのとき自分で取ったバックアップを使う（開発機ではデスクトップの `MKLM-backup-20260927\`）。

### Windows の設定アプリでキーボードの種類を変えてもよいか

MKLM が値を変えた後に設定アプリで変えると、次の MKLM の操作で「衝突」になる。とくに「接続済みキーボード レイアウトを使用する」は、MKLM の移行（`mklm-cli migrate`）が終わる前に選ぶと、内蔵の PS/2 キーボードが US になる。移行の後なら選んでかまわない。

### Recovery フォルダーがない

MKLM がまだ一度も値を書いていない。MKLM による変更はないので、戻すものもない。

---

## 9. 自動更新で困ったとき

MKLM 0.2.0 以降は、新しい版を自動で確認してダウンロードし、［今すぐ更新］を押したときだけ管理者の許可（UAC）を求めてインストールする。**更新はキーボードの設定に一切触れない**（読むだけで、書かない）。この章のどの場面でも、キーボードの値は更新の前のままである。

更新の状態と最後の結果は、コマンドプロンプト（管理者でなくてよい）で確かめられる。

```bat
mklm-cli update --status
```

- 最後の結果（インストールした、しなかった、途中で止まった）、進行中の更新、インストールの状態（3 つの exe の版がそろっているか）が表示される。`--json` を付けると機械で読める形になる。
- 更新の記録は `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update`（`Trust`、`Run`、`LastResult`）と、`%ProgramData%\SHIN DATA CENTER\MKLM\logs\update.log`（管理者だけが読める。英語）にある。**手で書き換えたり消したりしない。**

### 9.1 更新が途中で止まったとき

PC の電源が切れた、更新の途中でサインアウトやシャットダウンを強制した、などで更新が途中で止まると、次に MKLM を開いたときに次のどれかが 1 回だけ表示される。

| 表示 | 意味 | すること |
|---|---|---|
| 「更新は中断されました。何も変更されていません（MKLM は 0.2.0 のままです）。」 | インストーラーが動く前に止まった | 何もしなくてよい。もう一度［今すぐ更新］を押せば更新できる |
| 「MKLM は 0.2.1 に更新されました（終わる直前に PC が再起動したか、サインアウトしました）。」 | インストールは終わっていた | 何もしなくてよい |
| 「更新は中断されました。MKLM は 0.2.0 のままです。」 | インストーラーが途中で止まったが、元の版のまま | もう一度［今すぐ更新］を押す |
| 「更新が途中で止まったため、MKLM のファイルの版がそろっていません。…」 | 3 つの exe の版がそろっていない（ファイルの入れ替えの数ミリ秒の間に電源が切れた、など） | 9.2 |

- 途中で止まった更新の後片付け（記録と `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\` の古いフォルダー）は、次に管理者の許可で MKLM が動いたとき（次の更新、またはキーボードの変更）に自動で行われる。フォルダーを手で消す必要はない。
- インストーラーが 15 分たっても終わらないときは「インストーラーが時間内に終わりませんでした」と記録されるが、MKLM はインストーラーを止めない（ファイルの入れ替えの途中で止めると、かえって半端になるため）。インストーラーが後で終われば、結果は本当の結果で書き直される。

### 9.2 更新の後に MKLM が起動しないとき（ファイルの版がそろっていない）

`mklm.exe`、`mklm-cli.exe`、`mklm-helper.exe` の版がそろっていないと、MKLM は helper を起動できず、自動更新でも直せない。**インストーラーを実行し直せば直る**（インストーラーは 3 つのファイルを全部書き直す。キーボードの設定には触れない）。

1. MKLM の画面に［インストーラーを実行（管理者の許可が要ります）］が出ていれば、それを押す。UAC の確認画面で「詳細を表示」を押し、プログラムの場所が自分の `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\MKLM-Setup-<版>-<x64 か arm64>.exe` であることを確かめてから「はい」を押す。
2. MKLM 自体が起動しない場合は、エクスプローラーで `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\` を開き、そこにある `MKLM-Setup-<版>-<x64 か arm64>.exe` をダブルクリックする。
3. そのフォルダーにインストーラーがなければ、GitHub のリリース ページ（<https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases>）から、この PC に合う `MKLM-Setup-<版>-x64.exe`（ARM64 の PC で ARM64 版を使っていた場合は `-arm64.exe`）をダウンロードして実行する。ダウンロードしたファイルは、同じリリースの `SHA256SUMS` と照らして確かめる（PowerShell で `Get-FileHash <ファイル>`）。

インストーラーを手で実行したときの終了コード（`/S` でサイレントに実行した場合、`echo %ERRORLEVEL%`）:

| コード | 意味 | 何か置き換えたか |
|---|---|---|
| 0 | 完了 | はい |
| 1 | 利用者が取り消した | いいえ |
| 2 | インストーラーが途中で中止した（文書のファイルが書けない、など） | いいえ |
| 20 | Windows 11 24H2（build 26100）より古い | いいえ |
| 21 | ARM64 用のインストーラーを ARM64 以外で実行した、または 32 ビットの Windows | いいえ |
| 22 | `mklm-helper.exe` が動いている | いいえ |
| 23 | `mklm-cli.exe` が動いている | いいえ |
| 24 | `mklm.exe` が終了しない | いいえ |
| 26 | 別のプログラムが MKLM のファイルを開いていて、置き換えられなかった（入れ替えた分は元に戻した） | いいえ |
| 27 | 新しいファイルを書き込めなかった（ディスクの空き、ウイルス対策ソフト） | いいえ |

22〜24 と 26 のときは 9.3 のとおり、動いている MKLM やファイルを開いているプログラムを終わらせてから、もう一度実行する。

### 9.3 更新がいつも「ファイルを開いていた」「ほかの MKLM が動いていた」で止まるとき

更新は、この PC で動いているすべての MKLM（ほかのユーザーのものも含む）が終わり、MKLM のファイルをどのプログラムも開いていない状態でしか行わない。途中まで入れて半端にしないためである。そのため、**この PC のほかのユーザーが MKLM を何かの途中で止めたままにしている、`mklm-cli` を入力待ちのまま置いている、MKLM のファイルを開いたままにしている、といった状態が続く限り、更新は何度試しても止まる**（自動では抜けない。半端に入ることはない）。

画面の文は次のどれか（版は例）。

| 画面の文 | 止めたもの |
|---|---|
| 「ほかのユーザーの MKLM が、キーボードの変更の途中か確認を待っていたため、更新しませんでした（MKLM は 0.2.0 のままです）。…」 | 別のセッションの MKLM が、キーボードの変更の途中か確認を待っていた |
| 「ほかの MKLM（<プログラム>）が動いていたため、更新しませんでした（MKLM は 0.2.0 のままです）。…」 | 終了しない MKLM（ほかのユーザーのセッションの `mklm-cli` のこともある） |
| 「別のプログラムが MKLM のファイルを開いていたため、更新しませんでした（MKLM は 0.2.0 のままです）。…」 | MKLM 以外のプログラム（ほかのユーザーのプログラムや、ウイルス対策ソフトのこともある）が MKLM のファイルを開いていた |

止めた相手は、画面の「技術的な詳細」（［詳細をコピー］）と `mklm-cli update --status` に出る。`LastResult` の記録では次の形になる。

```json
"holders":[{"pid":7120,"session_id":2,"name":"powershell.exe"}]
```

- **pid**: 相手のプロセスの ID。**session_id**: そのプロセスが動いているサインインのセッション（ユーザーの切り替えで複数のユーザーがサインインしていると、それぞれ番号が違う）。**name**: 実行ファイルの名前。ユーザーの名前は、ほかのユーザーにも読める記録なので載せていない。
- 「ほかのユーザーの MKLM が、キーボードの変更の途中か確認を待っていた」ときは、プロセスの代わりに、その MKLM のセッションの番号が出る（`"sessions":[2]`）。
- ファイルを開いているプログラムを Windows が教えない場合、`holders` は空になる。

管理者ができること:

1. タスク マネージャーを開き、「ユーザー」タブでセッションとユーザーの対応を確かめる。「詳細」タブで「PID」の列を表示し、表示された pid のプロセスを探す。
2. そのユーザーに、MKLM の操作を終えて MKLM を終了してもらう。できなければ、そのプロセスを選んで「タスクの終了」を押す（MKLM の途中の変更は、次に MKLM を使ったときに回復する。[3.2](#32-cli-で戻す) の `recover`）。
3. もう一度［今すぐ更新］を押す。
4. それでも止まる場合は、PC を**再起動**した直後、ほかのユーザーがサインインする前に、自分だけがサインインして更新する。

### 9.4 ほかのユーザーの PC で、管理者として更新を承認するとき

標準ユーザーの PC で、管理者が UAC の画面に自分の資格情報を入れて更新を承認する場面の注意。

- ［今すぐ更新］で出る UAC の画面は、プログラムが `C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe`（インストール先。管理者しか書き換えられない）であることを「詳細を表示」で確かめてから承認する。更新の中身は helper が署名で確かめる。
- 「更新が途中で止まったため、MKLM のファイルの版がそろっていません」の画面の［インストーラーを実行］は、**そのユーザーのフォルダー（`%LOCALAPPDATA%\…\update\`）にあるファイル**を実行する。MKLM は実行の前に大きさと SHA-256 を照らすが、これは壊れたファイルを見つけるためだけのもので、安全の保証ではない（そのユーザー、またはそのユーザーの権限で動くプログラムは、照合の後にファイルを差し替えられる）。**ほかのユーザーの PC で管理者として承認するときは、この経路を使わず**、GitHub のリリース ページから自分でダウンロードし、`SHA256SUMS` と署名で確かめたインストーラーを実行する。
- 管理者として承認した後に MKLM が開き直さないことがある（別の管理者の資格情報で昇格した場合の、画面の起動し直しの制約）。そのユーザーがスタート メニューから MKLM を開けば、結果が表示される。次にサインインしたときにも表示される。

---

## 付録 A: MKLM が書き換える値

MKLM が書き換えるのは次の値だけ。ほかの値（`LayerDriver KOR`、`PollingIterations` など）と、`Services\i8042prt` の `Start` には触れない。**`Start` を変えると内蔵キーボードが動かなくなるので、手作業でも絶対に変えない。**

| 対象 | キー（Windows 上の場所） | 値の名前 | JIS | US | 標準に従う |
|---|---|---|---|---|---|
| 外付けの HID キーボード（USB、Bluetooth、BLE など） | `HKLM\SYSTEM\CurrentControlSet\Enum\<インスタンス ID>\Device Parameters` | `KeyboardTypeOverride` / `KeyboardSubtypeOverride`（DWORD） | 7 / 2 | 4 / 0 | 両方の値なし |
| 内蔵の PS/2 キーボード（i8042prt） | 同上（`ACPI\...` のインスタンス） | `OverrideKeyboardType` / `OverrideKeyboardSubtype`（DWORD。語順が HID と逆） | 7 / 2 | 4 / 0 | 使わない |
| PC 全体 | `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` | `LayerDriver JPN`（SZ）/ `OverrideKeyboardIdentifier`（SZ）: PC の標準配列 | `kbd106.dll` / `PCAT_106KEY` | `kbd101.dll` / `PCAT_101KEY` | — |
| PC 全体（固定モードのときだけ） | 同上 | `OverrideKeyboardType` / `OverrideKeyboardSubtype`（DWORD） | 7 / 2 | 7 / 0 | — |

- **固定モード**: PC 全体の `OverrideKeyboardType/Subtype` がある状態。すべてのキーボードが同じ配列になり、キーボードごとの値は効かない。Windows の設定で「日本語キーボード」「英語キーボード」を選んだ状態。
- **キーボードごとモード**: PC 全体の `OverrideKeyboardType/Subtype` がない状態。キーボードごとの値が効く。Windows の設定の「接続済みキーボード レイアウトを使用する」に当たる。
- **守らなければならない規則**: キーボードごとモードでは、すべての内蔵 PS/2 キーボード（今つながっていないものも含む）が、キーボードごとの `OverrideKeyboardType/Subtype` を持っていなければならない。持っていない内蔵キーボードは US として動く。そのため、手作業で戻すときは、**PC 全体の固定値を先に書き、内蔵キーボードの値を消すのはその後**にする。PC 全体の固定値を、内蔵キーボードの値より先に消してはいけない。
- キーボードごとの配列は、Windows の入力方式が**日本語 IME** のときだけ効く。英語（US）の入力方式に切り替えている間は、すべてのキーボードが US になる。

### Windows 上の場所と WinRE での場所の対応

| Windows（オンライン） | WinRE（オフライン。`reg load HKLM\MKLM_OFFLINE ...` の後） |
|---|---|
| `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` | `HKLM\MKLM_OFFLINE\ControlSet00N\Services\i8042prt\Parameters` |
| `HKLM\SYSTEM\CurrentControlSet\Enum\<インスタンス ID>\Device Parameters` | `HKLM\MKLM_OFFLINE\ControlSet00N\Enum\<インスタンス ID>\Device Parameters` |

`N` は `Select\Default` の値（6.6 の手順 2）。

### MKLM 自身の記録（手で書き換えない）

| もの | 場所 |
|---|---|
| ジャーナル（操作の記録と導入前の値） | `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal`。`mklm-cli journal` で読める |
| 復旧用ファイル | `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery\`（通常は `C:\ProgramData\...`） |
| ロックファイル | `%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock` |
| 再起動後の確認の登録 | `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` の `SHINDATACENTER.MKLM.PostReboot` |
