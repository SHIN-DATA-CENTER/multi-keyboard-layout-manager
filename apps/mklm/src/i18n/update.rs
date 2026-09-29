//! The texts of updates (design m5b E.2 to E.7, D.13, B.4 5): the messages of design m5b E.6 by
//! their IDs ([`Msg`]), the update page, the banners, the settings section, the hand-off and the
//! result. Which typed value says which message is `vm::update`'s exhaustive `match`es; the words
//! are here, in both languages (design m3 D.4).

use super::{Lang, pick};

/// A message of design m5b E.6, by its ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Msg {
    Net,
    NetDl,
    ProxyAuth,
    Tls,
    Cancelled,
    NotFound,
    NotFoundLong,
    GhTemp,
    GhChanged,
    Format,
    DownloadMismatch,
    Cache,
    Sig,
    Race,
    Rollback,
    NotNewer,
    Manual,
    NotConfigured,
    NotInstalledCopy,
    EnvUnknown,
    OpOpen,
    OpReboot,
    Busy,
    DiskFull,
    StageMismatch,
    PrepareFailed,
    Handoff,
    Installed,
    InstalledOther,
    InstanceBusy,
    ProgramsRunning,
    FilesInUse,
    SessionEnding,
    AvBlocked,
    InstallerNotStarted,
    VersionChanged,
    InstallerEnv,
    FileWrite,
    NotInstalled,
    Inconsistent,
    Timeout,
    /// Interrupted before anything was replaced (D.13 table, row 1).
    InterruptedNothing,
    /// Interrupted while installing, the new version in place (row 2).
    InterruptedInstalled,
    /// Interrupted while installing, the old version still in place (row 3).
    InterruptedKept,
    Expired,
    Stale,
    StaleStructural,
}

impl Msg {
    /// The ID of design m5b E.6.
    pub fn id(self) -> &'static str {
        match self {
            Msg::Net => "upd-net",
            Msg::NetDl => "upd-net-dl",
            Msg::ProxyAuth => "upd-proxy-auth",
            Msg::Tls => "upd-tls",
            Msg::Cancelled => "upd-cancelled",
            Msg::NotFound => "upd-not-found",
            Msg::NotFoundLong => "upd-not-found-long",
            Msg::GhTemp => "upd-gh-temp",
            Msg::GhChanged => "upd-gh-changed",
            Msg::Format => "upd-format",
            Msg::DownloadMismatch => "upd-download-mismatch",
            Msg::Cache => "upd-cache",
            Msg::Sig => "upd-sig",
            Msg::Race => "upd-race",
            Msg::Rollback => "upd-rollback",
            Msg::NotNewer => "upd-not-newer",
            Msg::Manual => "upd-manual",
            Msg::NotConfigured => "upd-not-configured",
            Msg::NotInstalledCopy => "upd-not-installed-copy",
            Msg::EnvUnknown => "upd-env-unknown",
            Msg::OpOpen => "upd-op-open",
            Msg::OpReboot => "upd-op-reboot",
            Msg::Busy => "upd-busy",
            Msg::DiskFull => "upd-disk-full",
            Msg::StageMismatch => "upd-stage-mismatch",
            Msg::PrepareFailed => "upd-prepare-failed",
            Msg::Handoff => "upd-handoff",
            Msg::Installed => "upd-installed",
            Msg::InstalledOther => "upd-installed-other",
            Msg::InstanceBusy => "upd-instance-busy",
            Msg::ProgramsRunning => "upd-programs-running",
            Msg::FilesInUse => "upd-files-in-use",
            Msg::SessionEnding => "upd-session-ending",
            Msg::AvBlocked => "upd-av-blocked",
            Msg::InstallerNotStarted => "upd-installer-not-started",
            Msg::VersionChanged => "upd-version-changed",
            Msg::InstallerEnv => "upd-installer-env",
            Msg::FileWrite => "upd-file-write",
            Msg::NotInstalled => "upd-not-installed",
            Msg::Inconsistent => "upd-inconsistent",
            Msg::Timeout => "upd-timeout",
            Msg::InterruptedNothing => "upd-interrupted-nothing",
            Msg::InterruptedInstalled => "upd-interrupted-installed",
            Msg::InterruptedKept => "upd-interrupted-kept",
            Msg::Expired => "upd-expired",
            Msg::Stale => "upd-stale",
            Msg::StaleStructural => "upd-stale-structural",
        }
    }

    /// Every message (the snapshot of the table, and the tests that every one has both texts).
    pub const ALL: [Msg; 47] = [
        Msg::Net,
        Msg::NetDl,
        Msg::ProxyAuth,
        Msg::Tls,
        Msg::Cancelled,
        Msg::NotFound,
        Msg::NotFoundLong,
        Msg::GhTemp,
        Msg::GhChanged,
        Msg::Format,
        Msg::DownloadMismatch,
        Msg::Cache,
        Msg::Sig,
        Msg::Race,
        Msg::Rollback,
        Msg::NotNewer,
        Msg::Manual,
        Msg::NotConfigured,
        Msg::NotInstalledCopy,
        Msg::EnvUnknown,
        Msg::OpOpen,
        Msg::OpReboot,
        Msg::Busy,
        Msg::DiskFull,
        Msg::StageMismatch,
        Msg::PrepareFailed,
        Msg::Handoff,
        Msg::Installed,
        Msg::InstalledOther,
        Msg::InstanceBusy,
        Msg::ProgramsRunning,
        Msg::FilesInUse,
        Msg::SessionEnding,
        Msg::AvBlocked,
        Msg::InstallerNotStarted,
        Msg::VersionChanged,
        Msg::InstallerEnv,
        Msg::FileWrite,
        Msg::NotInstalled,
        Msg::Inconsistent,
        Msg::Timeout,
        Msg::InterruptedNothing,
        Msg::InterruptedInstalled,
        Msg::InterruptedKept,
        Msg::Expired,
        Msg::Stale,
        Msg::StaleStructural,
    ];
}

/// What a message names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params {
    /// The running version ("0.2.0").
    pub installed: String,
    /// The version offered or updated to ("0.2.1").
    pub offered: String,
    /// The folder this MKLM runs from (`NotInstalledCopy`).
    pub path: String,
    /// The installer's file name ("MKLM-Setup-0.2.1-x64.exe").
    pub installer: String,
    /// Megabytes still needed (`DiskFull`).
    pub needed_mb: u64,
    /// The MKLM programs that were running, as a list.
    pub programs: String,
    /// Why the installer refused (`InstallerEnv`).
    pub reason: String,
    /// A date, local ("2027/04/13").
    pub date: String,
    /// What the installation is now, for `Inconsistent` / `Timeout` (a sentence, or empty).
    pub state_now: String,
}

/// The text of `msg`.
pub fn text(msg: Msg, p: &Params, lang: Lang) -> String {
    let ja = lang == Lang::Ja;
    let (installed, offered) = (p.installed.as_str(), p.offered.as_str());
    match msg {
        Msg::Net | Msg::NetDl => {
            let (what_ja, what_en) = if msg == Msg::Net {
                ("確認", "check for updates")
            } else {
                ("ダウンロード", "download the update")
            };
            if ja {
                format!(
                    "更新を{what_ja}できませんでした。インターネットにつながっているか確かめてください。会社や学校のネットワークでは、プロキシの設定が必要なことがあります。"
                )
            } else {
                format!(
                    "MKLM could not {what_en}. Check that this PC is connected to the internet. On a company or school network, a proxy may have to be set up."
                )
            }
        }
        Msg::ProxyAuth => pick(
            lang,
            "このネットワークのプロキシは認証を求めるため、MKLM は自動で更新を確認できません（安全のため、Windows の資格情報を自動では送りません）。GitHub のリリース ページで新しい版を確かめてください。",
            "The proxy of this network asks for authentication, so MKLM cannot check for updates by itself (for safety it never sends your Windows credentials automatically). Look for a new version on the GitHub release page.",
        ),
        Msg::Tls => pick(
            lang,
            "GitHub と安全に接続できませんでした。PC の日付と時刻が正しいか確かめてください。",
            "MKLM could not connect to GitHub securely. Check that the PC's date and time are right.",
        ),
        Msg::Cancelled => pick(
            lang,
            "取り消しました（何も変更していません）。",
            "Cancelled (nothing was changed).",
        ),
        Msg::NotFound => pick(
            lang,
            "更新情報が見つかりませんでした。しばらくしてからもう一度確かめてください。",
            "The update information was not found. Check again later.",
        ),
        Msg::NotFoundLong => pick(
            lang,
            "更新情報が 7 日以上見つかりません。自動更新が使えなくなっている可能性があります。GitHub のリリース ページで新しい版を確かめてください。",
            "The update information has not been found for 7 days or more. Automatic updates may no longer work. Look for a new version on the GitHub release page.",
        ),
        Msg::GhTemp => pick(
            lang,
            "GitHub が一時的に応答しませんでした。しばらくしてからもう一度確かめてください。",
            "GitHub did not answer for the moment. Check again later.",
        ),
        Msg::GhChanged => pick(
            lang,
            "GitHub の配布の仕組みが変わったため、この版の MKLM は自動で更新できない可能性があります。GitHub のリリース ページから新しい版を入れてください。",
            "GitHub changed how it hands out files, so this version of MKLM may not be able to update itself. Install a new version from the GitHub release page.",
        ),
        Msg::Format => pick(
            lang,
            "更新情報の形式が、この版の MKLM と合いません。GitHub のリリース ページから新しい版を入れてください。",
            "The update information is in a form this version of MKLM does not understand. Install a new version from the GitHub release page.",
        ),
        Msg::DownloadMismatch => pick(
            lang,
            "ダウンロードしたファイルが更新情報と一致しませんでした。もう一度ダウンロードしてください。",
            "The downloaded file does not match the update information. Download it again.",
        ),
        Msg::Cache => pick(
            lang,
            "ダウンロードしたファイルを保存できませんでした。ディスクの空きを確かめてください。",
            "The downloaded file could not be saved. Check the free disk space.",
        ),
        Msg::Sig => pick(
            lang,
            "更新情報の署名を確かめられませんでした。安全のため、この更新は使いません。GitHub のリリース ページで最新の案内を確かめてください。",
            "The signature of the update information could not be verified. For safety, this update is not used. Check the GitHub release page for the latest news.",
        ),
        Msg::Race => pick(
            lang,
            "更新情報を取得している間に、新しい版が公開されたようです。しばらくしてからもう一度確かめてください。",
            "A new version seems to have been published while the update information was fetched. Check again later.",
        ),
        Msg::Rollback => pick(
            lang,
            "以前に確かめたものより古い更新情報が届いたため、使いませんでした。新しい版が取り下げられたか、途中で古い情報に差し替えられた可能性があります。GitHub のリリース ページで最新の版を確かめてください。",
            "Update information older than the one checked before arrived, so it was not used. A new version may have been withdrawn, or older information swapped in on the way. Check the newest version on the GitHub release page.",
        ),
        Msg::NotNewer => pick(
            lang,
            "この更新は、今の MKLM より新しくありません（すでに入っているようです）。",
            "This update is not newer than the MKLM you have (it seems to be installed already).",
        ),
        Msg::Manual => pick(
            lang,
            "この版からは自動で更新できません。リリース ページからインストーラーをダウンロードして実行してください。",
            "This version cannot be updated automatically. Download the installer from the release page and run it.",
        ),
        Msg::NotConfigured => pick(
            lang,
            "この MKLM には更新を確かめるための鍵が入っていないため、自動更新は使えません。",
            "This MKLM has no keys to verify updates with, so automatic updates are not available.",
        ),
        Msg::NotInstalledCopy => {
            if ja {
                format!(
                    "この MKLM は {} から動いているため、自動更新は使えません（インストールした MKLM だけが更新できます）。",
                    p.path
                )
            } else {
                format!(
                    "This MKLM runs from {}, so automatic updates are not available (only an installed MKLM updates itself).",
                    p.path
                )
            }
        }
        Msg::EnvUnknown => pick(
            lang,
            "更新を使えるか確かめられませんでした。MKLM を開き直してください。",
            "MKLM could not tell whether updates can be used. Open MKLM again.",
        ),
        Msg::OpOpen => pick(
            lang,
            "確認待ちの変更があるため、今は更新できません。［確認…］で変更を決めてから更新してください。",
            "A change is waiting for you, so MKLM cannot update now. Decide about it with \"Review…\", then update.",
        ),
        Msg::OpReboot => pick(
            lang,
            "PC の再起動を待っている変更があるため、今は更新できません。PC を再起動し（シャットダウンではなく再起動）、確認を終えてから更新してください。",
            "A change is waiting for a PC restart, so MKLM cannot update now. Restart the PC (Restart, not Shut down), finish the check, then update.",
        ),
        Msg::Busy => pick(
            lang,
            "別の MKLM が処理中です。しばらくしてからもう一度試してください。",
            "Another MKLM is busy. Try again in a moment.",
        ),
        Msg::DiskFull => {
            if ja {
                format!(
                    "ディスクの空きが足りないため、更新できませんでした（あと約 {} MB 必要です）。MKLM は {installed} のままで、キーボードの設定も変わっていません。",
                    p.needed_mb
                )
            } else {
                format!(
                    "There is not enough free disk space, so MKLM was not updated (about {} MB more is needed). MKLM stays at {installed}, and the keyboard settings did not change.",
                    p.needed_mb
                )
            }
        }
        Msg::StageMismatch => pick(
            lang,
            "更新の準備の途中で、受け渡したファイルが一致しませんでした。何も変更していません。もう一度ダウンロードしてから試してください。",
            "While the update was prepared, the file handed over did not match. Nothing was changed. Download it again, then try again.",
        ),
        Msg::PrepareFailed => pick(
            lang,
            "更新の準備に失敗しました。何も変更していません。PC を再起動してからもう一度試してください。直らない場合は［詳細をコピー］を押して、その内容を添えて報告してください。",
            "Preparing the update failed. Nothing was changed. Restart the PC and try again. If it keeps failing, choose \"Copy details\" and include them in a report.",
        ),
        Msg::Handoff => handoff_text(lang),
        Msg::Installed => {
            if ja {
                format!("MKLM を {offered} に更新しました。")
            } else {
                format!("MKLM was updated to {offered}.")
            }
        }
        Msg::InstalledOther => {
            if ja {
                format!("MKLM は {offered} に更新されました（別のユーザーが更新しました）。")
            } else {
                format!("MKLM was updated to {offered} (another user updated it).")
            }
        }
        Msg::InstanceBusy => {
            if ja {
                format!(
                    "ほかのユーザーの MKLM が、キーボードの変更の途中か確認を待っていたため、更新しませんでした（MKLM は {installed} のままです）。その変更が終わってから、もう一度［今すぐ更新］を押してください。"
                )
            } else {
                format!(
                    "Another user's MKLM was in the middle of a keyboard change or waiting for an answer, so MKLM was not updated (it stays at {installed}). When that change is done, choose \"Update now\" again."
                )
            }
        }
        Msg::ProgramsRunning => {
            if ja {
                format!(
                    "ほかの MKLM（{}）が動いていたため、更新しませんでした（MKLM は {installed} のままです）。それを終了してから、もう一度［今すぐ更新］を押してください。",
                    p.programs
                )
            } else {
                format!(
                    "Another MKLM ({}) was running, so MKLM was not updated (it stays at {installed}). Close it, then choose \"Update now\" again.",
                    p.programs
                )
            }
        }
        Msg::FilesInUse => {
            if ja {
                format!(
                    "別のプログラムが MKLM のファイルを開いていたため、更新しませんでした（MKLM は {installed} のままです）。ほかのユーザーのプログラムや、ウイルス対策ソフトのことがあります。しばらくしてから、もう一度［今すぐ更新］を押してください。何度試しても同じなら、［詳細をコピー］の内容（ファイルを開いているプログラムとセッションの番号）を管理者に伝えてください。"
                )
            } else {
                format!(
                    "Another program had MKLM's files open, so MKLM was not updated (it stays at {installed}). It may be another user's program or an antivirus. Wait a moment, then choose \"Update now\" again. If it happens every time, give your administrator what \"Copy details\" copies (the programs holding the files and their session numbers)."
                )
            }
        }
        Msg::SessionEnding => {
            if ja {
                format!(
                    "サインアウトかシャットダウンが始まったため、更新しませんでした（MKLM は {installed} のままです）。"
                )
            } else {
                format!(
                    "Signing out or shutting down had begun, so MKLM was not updated (it stays at {installed})."
                )
            }
        }
        Msg::AvBlocked => {
            if ja {
                format!(
                    "インストーラーがウイルス対策ソフトに止められました。Windows セキュリティ → ウイルスと脅威の防止 → 保護の履歴 で確かめてください。MKLM は {installed} のままです。"
                )
            } else {
                format!(
                    "An antivirus stopped the installer. Look under Windows Security → Virus & threat protection → Protection history. MKLM stays at {installed}."
                )
            }
        }
        Msg::InstallerNotStarted => {
            if ja {
                format!(
                    "インストーラーを起動できませんでした。MKLM は {installed} のままで、キーボードの設定も変わっていません。"
                )
            } else {
                format!(
                    "The installer could not be started. MKLM stays at {installed}, and the keyboard settings did not change."
                )
            }
        }
        Msg::VersionChanged => pick(
            lang,
            "更新の準備の間に、別の方法で MKLM がインストールされました。今の版で問題なければ、何もする必要はありません。",
            "MKLM was installed another way while the update was prepared. If the version you have now is fine, there is nothing to do.",
        ),
        Msg::InstallerEnv => {
            if ja {
                format!(
                    "インストーラーが、この PC では動かないと判断しました（{}）。MKLM は {installed} のままです。",
                    p.reason
                )
            } else {
                format!(
                    "The installer found that it cannot run on this PC ({}). MKLM stays at {installed}.",
                    p.reason
                )
            }
        }
        Msg::FileWrite => {
            if ja {
                format!(
                    "インストーラーがファイルを書けませんでした。ディスクの空きとウイルス対策ソフトを確かめてください。MKLM は {installed} のままです。"
                )
            } else {
                format!(
                    "The installer could not write its files. Check the free disk space and your antivirus. MKLM stays at {installed}."
                )
            }
        }
        Msg::NotInstalled => {
            if ja {
                format!(
                    "更新できませんでした。MKLM は {installed} のままで、キーボードの設定も変わっていません。"
                )
            } else {
                format!(
                    "MKLM could not be updated. It stays at {installed}, and the keyboard settings did not change."
                )
            }
        }
        Msg::Inconsistent => {
            if ja {
                format!(
                    "更新が途中で止まったため、MKLM のファイルの版がそろっていません。キーボードの設定はそのままです。［インストーラーを実行］を押すか、GitHub のリリース ページから {} をダウンロードして実行してください。",
                    p.installer
                )
            } else {
                format!(
                    "An update stopped half way, so MKLM's files are of different versions. The keyboard settings are as they were. Choose \"Run the installer\", or download {} from the GitHub release page and run it.",
                    p.installer
                )
            }
        }
        Msg::Timeout => {
            let head = pick(
                lang,
                "インストーラーが 60 分たっても終わりませんでした。",
                "The installer had not finished after 60 minutes.",
            );
            join_sentences(&head, &p.state_now, lang)
        }
        Msg::InterruptedNothing => {
            if ja {
                format!(
                    "更新は中断されました。何も変更されていません（MKLM は {installed} のままです）。"
                )
            } else {
                format!(
                    "The update was interrupted. Nothing was changed (MKLM stays at {installed})."
                )
            }
        }
        Msg::InterruptedInstalled => {
            if ja {
                format!(
                    "MKLM は {offered} に更新されました（終わる直前に PC が再起動したか、サインアウトしました）。"
                )
            } else {
                format!(
                    "MKLM was updated to {offered} (the PC restarted, or you signed out, just before the update finished)."
                )
            }
        }
        Msg::InterruptedKept => {
            if ja {
                format!("更新は中断されました。MKLM は {installed} のままです。")
            } else {
                format!("The update was interrupted. MKLM stays at {installed}.")
            }
        }
        Msg::Expired => {
            if ja {
                format!(
                    "更新情報の有効期限（{}）を過ぎています。新しい版が長く公開されていないか、古い情報が届いています。GitHub のリリース ページで確かめてください。",
                    p.date
                )
            } else {
                format!(
                    "The update information expired on {}. Either no new version has been published for a long time, or older information is arriving. Check the GitHub release page.",
                    p.date
                )
            }
        }
        Msg::Stale => pick(
            lang,
            "30 日以上、MKLM の更新を確認できていません。インターネットやプロキシの設定を確かめてください。",
            "MKLM has not been able to check for updates for 30 days or more. Check the internet and proxy settings.",
        ),
        Msg::StaleStructural => pick(
            lang,
            "MKLM の自動更新が使えなくなっている可能性があります。GitHub のリリース ページで新しい版を確かめてください。",
            "MKLM's automatic updates may no longer work. Look for a new version on the GitHub release page.",
        ),
    }
}

/// Two sentences (Japanese sentences end with "。", English ones are joined with a space).
pub fn join_sentences(first: &str, second: &str, lang: Lang) -> String {
    match (second.is_empty(), lang) {
        (true, _) => first.to_string(),
        (false, Lang::Ja) => format!("{first}{second}"),
        (false, Lang::En) => format!("{first} {second}"),
    }
}

/// "キーボードの設定は変わっていません。": every "not installed" and "failed" text says it (design m5b
/// E.6: an update never touches the keyboards).
pub fn keyboards_unchanged(lang: Lang) -> String {
    pick(
        lang,
        "キーボードの設定は変わっていません。",
        "The keyboard settings did not change.",
    )
}

/// The mention [`keyboards_unchanged`] adds, unless `text` already says it.
pub fn with_keyboards_unchanged(text: String, lang: Lang) -> String {
    let said = match lang {
        Lang::Ja => text.contains("キーボードの設定"),
        Lang::En => text.contains("keyboard settings"),
    };
    if said {
        text
    } else {
        join_sentences(&text, &keyboards_unchanged(lang), lang)
    }
}

/// "MKLM は 0.2.0 のままで、キーボードの設定も変わっていません。": added to the text of an update
/// the runner refused (`NotInstalled(Refused(…))`, design m5b D.7 steps 8–10, E.6), which is
/// the refusal's own text and names neither — unless `text` already says it (`DiskFull`).
pub fn with_version_kept(text: String, version: &str, lang: Lang) -> String {
    let said = match lang {
        Lang::Ja => text.contains("キーボードの設定"),
        Lang::En => text.contains("keyboard settings"),
    };
    if said {
        return text;
    }
    let kept = match lang {
        Lang::Ja => format!("MKLM は {version} のままで、キーボードの設定も変わっていません。"),
        Lang::En => format!("MKLM stays at {version}, and the keyboard settings did not change."),
    };
    join_sentences(&text, &kept, lang)
}

/// "今の MKLM は 0.2.0 です。" (a failed update whose installation is consistent now).
pub fn version_now(version: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("今の MKLM は {version} です。"),
        Lang::En => format!("MKLM is at {version} now."),
    }
}

/// "別のユーザーの更新のために、MKLM はいったん終了していました。" (design m5b E.5).
pub fn closed_for_another_update(lang: Lang) -> String {
    pick(
        lang,
        "別のユーザーの更新のために、MKLM はいったん終了していました。",
        "MKLM had been closed for a while because another user updated it.",
    )
}

/// Why the installer refused to run here (`InstallerExit::{OsTooOld, WrongArch}`).
pub fn installer_env_reason(too_old: bool, lang: Lang) -> String {
    if too_old {
        pick(
            lang,
            "Windows の版が古い",
            "this version of Windows is too old",
        )
    } else {
        pick(
            lang,
            "PC の種類（アーキテクチャ）が違う",
            "the PC's architecture does not match",
        )
    }
}

/// `ProgramKind`: "MKLM", "mklm-cli", "mklm-helper".
pub fn program(kind: mklm_update::run::ProgramKind) -> &'static str {
    match kind {
        mklm_update::run::ProgramKind::Gui => "MKLM",
        mklm_update::run::ProgramKind::Cli => "mklm-cli",
        mklm_update::run::ProgramKind::Helper => "mklm-helper",
    }
}

/// A list of names ("MKLM、mklm-cli" / "MKLM, mklm-cli").
pub fn list(names: &[&str], lang: Lang) -> String {
    names.join(match lang {
        Lang::Ja => "、",
        Lang::En => ", ",
    })
}

/// `RunPhase` in the technical details: "受け取り中" … (never in the user's sentences, E.6).
pub fn phase(phase: mklm_update::run::RunPhase, lang: Lang) -> String {
    use mklm_update::run::RunPhase;
    let (ja, en) = match phase {
        RunPhase::Staging => ("受け取り中", "receiving"),
        RunPhase::Staged => ("受け取り済み", "received"),
        RunPhase::Ready => ("準備完了", "ready"),
        RunPhase::Waiting => ("ほかの MKLM の終了待ち", "waiting for other MKLMs to end"),
        RunPhase::Installing => ("インストール中", "installing"),
        RunPhase::Finishing => ("確認中", "checking"),
        RunPhase::Done => ("完了", "done"),
    };
    pick(lang, ja, en)
}

// --- The update page (design m5b E.2) --------------------------------------------------------

pub fn page_title(lang: Lang) -> String {
    pick(lang, "MKLM の更新", "MKLM updates")
}

pub fn checking(lang: Lang) -> String {
    pick(lang, "確認しています…", "Checking…")
}

pub fn not_checked(lang: Lang) -> String {
    pick(
        lang,
        "まだ更新を確認していません。",
        "MKLM has not checked for updates yet.",
    )
}

/// "MKLM は最新です（0.2.0。2026/10/15 の更新情報）。" — the date of the manifest lets a frozen
/// feed be noticed (RED-TEAM-1).
pub fn up_to_date(installed: &str, issued: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("MKLM は最新です（{installed}。{issued} の更新情報）。"),
        Lang::En => {
            format!("MKLM is up to date ({installed}; update information of {issued}).")
        }
    }
}

/// "最後の確認: 2026/10/16 09:12".
pub fn last_checked(when: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("最後の確認: {when}"),
        Lang::En => format!("Last check: {when}"),
    }
}

/// "新しい版があります: 0.2.1（今の版: 0.2.0）".
pub fn new_version(offered: &str, installed: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("新しい版があります: {offered}（今の版: {installed}）"),
        Lang::En => format!("A new version is available: {offered} (you have {installed})"),
    }
}

/// "公開: 2026/10/15".
pub fn published(date: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("公開: {date}"),
        Lang::En => format!("Published: {date}"),
    }
}

pub fn skipped(version: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{version} はスキップしました。"),
        Lang::En => format!("You skipped {version}."),
    }
}

pub fn not_downloaded(lang: Lang) -> String {
    pick(
        lang,
        "まだダウンロードしていません。",
        "Not downloaded yet.",
    )
}

/// Megabytes with one decimal ("6.0").
pub fn megabytes(bytes: u64) -> String {
    let tenths = (bytes * 10 + 524_288) / 1_048_576;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// "ダウンロード中: 2.9 MB / 6.0 MB（45 %）".
pub fn downloading(received: u64, total: u64, percent: u32, lang: Lang) -> String {
    let (received, total) = (megabytes(received), megabytes(total));
    match lang {
        Lang::Ja => format!("ダウンロード中: {received} MB / {total} MB（{percent} %）"),
        Lang::En => format!("Downloading: {received} MB of {total} MB ({percent} %)"),
    }
}

/// The progress bar's accessible label: "ダウンロード: 45 %".
pub fn download_progress_label(percent: u32, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("ダウンロード: {percent} %"),
        Lang::En => format!("Download: {percent} %"),
    }
}

/// "ダウンロード: 完了（6.0 MB。内容を確かめました）".
pub fn downloaded(total: u64, lang: Lang) -> String {
    let total = megabytes(total);
    match lang {
        Lang::Ja => format!("ダウンロード: 完了（{total} MB。内容を確かめました）"),
        Lang::En => format!("Download: done ({total} MB; the content was checked)"),
    }
}

/// The explanation before "今すぐ更新" (design m5b E.2; SECURITY-14): what Windows shows, and the
/// program's real location to check with "Show more details". Without a prompt (an elevated
/// MKLM), only what happens.
pub fn update_explanation(helper: &str, prompt: bool, lang: Lang) -> String {
    let after = pick(
        lang,
        "許可すると MKLM はいったん終了し、更新が終わると自動で開きます。キーボードの設定は変わりません。",
        "Once you allow it, MKLM closes, and opens again by itself when the update is done. The keyboard settings do not change.",
    );
    if !prompt {
        return pick(
            lang,
            "［今すぐ更新］を押すと MKLM はいったん終了し、更新が終わると自動で開きます。キーボードの設定は変わりません。",
            "When you choose \"Update now\", MKLM closes, and opens again by itself when the update is done. The keyboard settings do not change.",
        );
    }
    let before = match lang {
        Lang::Ja => format!(
            "［今すぐ更新］を押すと、Windows が管理者の許可を求めます。発行元は「不明」と表示されます。{}",
            location_check(helper, lang)
        ),
        Lang::En => format!(
            "When you choose \"Update now\", Windows asks for administrator permission. The publisher shows as \"Unknown\". {}",
            location_check(helper, lang)
        ),
    };
    join_sentences(&before, &after, lang)
}

/// How to check the prompt's program before allowing it (design m5b E.2, E.4; SECURITY-14): also
/// on the standalone UAC explanation of an update.
pub fn location_check(helper: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "許可する前に「詳細を表示」を押し、プログラムの場所が {helper} であることを確かめてください。違う場所なら「いいえ」を押してください。MKLM が許可を求めるのは、あなたが［今すぐ更新］を押した直後だけです。"
        ),
        Lang::En => format!(
            "Before you allow it, choose \"Show more details\" and check that the program's location is {helper}. If it is anywhere else, choose No. MKLM asks for permission only right after you choose \"Update now\"."
        ),
    }
}

/// The update session's progress (design m5b E.2).
pub fn waiting_for_permission(lang: Lang) -> String {
    pick(
        lang,
        "管理者の確認を待っています…",
        "Waiting for administrator permission…",
    )
}

pub fn preparing(percent: u32, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("更新を準備しています…（送信 {percent} %）"),
        Lang::En => format!("Preparing the update… ({percent} % handed over)"),
    }
}

pub fn windows_checking(lang: Lang) -> String {
    pick(
        lang,
        "Windows がファイルを確認しています…（最大 2 分ほど）",
        "Windows is checking the files… (up to about 2 minutes)",
    )
}

pub fn handing_over(lang: Lang) -> String {
    pick(
        lang,
        "MKLM はまもなく終了し、更新が終わると自動で開きます。",
        "MKLM closes in a moment and opens again by itself when the update is done.",
    )
}

/// "最後の確認: 古い更新情報を無視しました（受け取ったもの 2026/10/20、記録 2026/11/02）" (E.3).
pub fn rollback_ignored(received: &str, recorded: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "最後の確認: 古い更新情報を無視しました（受け取ったもの {received}、記録 {recorded}）"
        ),
        Lang::En => format!(
            "Last check: older update information was ignored (received {received}, recorded {recorded})"
        ),
    }
}

/// The last check failed while an earlier result is shown.
pub fn last_check_failed(message: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("最後の確認に失敗しました: {message}"),
        Lang::En => format!("The last check failed: {message}"),
    }
}

/// "この PC では ARM64 版も使えます。" (design m5b C.7: shown only; J-1).
pub fn arm64_available(lang: Lang) -> String {
    pick(
        lang,
        "この PC では ARM64 版も使えます（今の MKLM は x64 版です。切り替えるには、リリース ページから ARM64 版を入れてください）。",
        "This PC can also run the ARM64 version (this MKLM is the x64 one; to switch, install the ARM64 version from the release page).",
    )
}

// --- Buttons and their accessible names (design m5b E.2, E.7) --------------------------------

/// What a button of the update page, a banner or the result does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    CheckNow,
    CheckAgain,
    Cancel,
    Download,
    DownloadAgain,
    Skip,
    Later,
    UpdateNow,
    UpdateNowPrompt,
    ReleaseNotes,
    ReleasePage,
    CopyDetails,
    RunInstaller,
    Review,
    Restart,
    Details,
    Close,
}

/// A button's text.
pub fn button(button: Button, lang: Lang) -> String {
    let (ja, en) = match button {
        Button::CheckNow => ("今すぐ確認", "Check now"),
        Button::CheckAgain => ("もう一度確認", "Check again"),
        Button::Cancel => ("キャンセル", "Cancel"),
        Button::Download => ("ダウンロード", "Download"),
        Button::DownloadAgain => ("もう一度ダウンロード", "Download again"),
        Button::Skip => ("この版をスキップ", "Skip this version"),
        Button::Later => ("後で", "Later"),
        Button::UpdateNow => ("今すぐ更新", "Update now"),
        Button::UpdateNowPrompt => (
            "今すぐ更新（次に Windows の確認が出ます）",
            "Update now (Windows asks next)",
        ),
        Button::ReleaseNotes => ("リリースノートを開く", "Open the release notes"),
        Button::ReleasePage => ("リリース ページを開く", "Open the release page"),
        Button::CopyDetails => ("詳細をコピー", "Copy details"),
        Button::RunInstaller => (
            "インストーラーを実行（管理者の許可が要ります）",
            "Run the installer (needs administrator permission)",
        ),
        Button::Review => ("確認…", "Review…"),
        Button::Restart => ("再起動…", "Restart…"),
        Button::Details => ("詳細…", "Details…"),
        Button::Close => ("閉じる", "Close"),
    };
    pick(lang, ja, en)
}

/// A button's accessible name when it says more than its text (design m5b E.7): "0.2.1 をスキップ",
/// "今すぐ 0.2.1 に更新", "0.2.1 をダウンロード", "MKLM の更新の詳細を開く".
pub fn button_label(button: Button, version: &str, lang: Lang) -> String {
    match (button, lang) {
        (Button::Skip, Lang::Ja) => format!("{version} をスキップ"),
        (Button::Skip, Lang::En) => format!("Skip {version}"),
        (Button::UpdateNow | Button::UpdateNowPrompt, Lang::Ja) => {
            format!("今すぐ {version} に更新")
        }
        (Button::UpdateNow | Button::UpdateNowPrompt, Lang::En) => {
            format!("Update to {version} now")
        }
        (Button::Download | Button::DownloadAgain, Lang::Ja) => {
            format!("{version} をダウンロード")
        }
        (Button::Download | Button::DownloadAgain, Lang::En) => format!("Download {version}"),
        (Button::Details, _) => pick(
            lang,
            "MKLM の更新の詳細を開く",
            "Open the details of the MKLM update",
        ),
        (other, _) => self::button(other, lang),
    }
}

// --- Banners and the tray (design m5b E.3) ---------------------------------------------------

/// "MKLM の新しい版（0.2.1）を使えます。".
pub fn banner_available(version: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("MKLM の新しい版（{version}）を使えます。"),
        Lang::En => format!("A new version of MKLM ({version}) is ready."),
    }
}

/// "以前に確かめたものより古い更新情報が届いたため、使いませんでした。".
pub fn banner_rollback(lang: Lang) -> String {
    pick(
        lang,
        "以前に確かめたものより古い更新情報が届いたため、使いませんでした。",
        "Update information older than the one checked before arrived, so it was not used.",
    )
}

/// The tray's tooltip while an update is ready: "MKLM — 新しい版（0.2.1）があります".
pub fn tray_tooltip(version: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("MKLM — 新しい版（{version}）があります"),
        Lang::En => format!("MKLM — a new version ({version}) is ready"),
    }
}

// --- Settings (design m5b E.2) ---------------------------------------------------------------

/// The line under "更新を自動で確認する": the last check and what it found.
pub fn settings_last_check(when: Option<&str>, result: Option<&str>, lang: Lang) -> String {
    match (when, result) {
        (None, _) => pick(
            lang,
            "最後の確認: まだ確認していません",
            "Last check: not yet",
        ),
        (Some(when), None) => last_checked(when, lang),
        (Some(when), Some(result)) => match lang {
            Lang::Ja => format!("最後の確認: {when}（{result}）"),
            Lang::En => format!("Last check: {when} ({result})"),
        },
    }
}

/// What the last check found, in a few words.
pub fn settings_check_result(found: SettingsFound<'_>, lang: Lang) -> String {
    match found {
        SettingsFound::UpToDate => pick(lang, "最新です", "up to date"),
        SettingsFound::Available(version) => match lang {
            Lang::Ja => format!("新しい版 {version} があります"),
            Lang::En => format!("{version} is available"),
        },
        SettingsFound::Failed => pick(lang, "確認できませんでした", "the check failed"),
    }
}

/// What the last check found (for [`settings_check_result`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFound<'a> {
    UpToDate,
    Available(&'a str),
    Failed,
}

/// "最後に成功した確認: …" (OPS-UX-TEST-5).
pub fn settings_last_success(when: Option<&str>, lang: Lang) -> String {
    match (when, lang) {
        (Some(when), Lang::Ja) => format!("最後に成功した確認: {when}"),
        (Some(when), Lang::En) => format!("Last successful check: {when}"),
        (None, _) => pick(
            lang,
            "最後に成功した確認: まだありません",
            "Last successful check: none yet",
        ),
    }
}

/// "30 日以上、更新を確認できていません。" with the kind of the last failure (OPS-UX-TEST-5).
pub fn settings_stale(structural: bool, lang: Lang) -> String {
    let kind = if structural {
        pick(
            lang,
            "最後の失敗: この版の MKLM では直らない見込みの問題",
            "last failure: a problem this version of MKLM is unlikely to get past",
        )
    } else {
        pick(
            lang,
            "最後の失敗: 時間をおけば直りうる問題",
            "last failure: a problem that may go away by itself",
        )
    };
    match lang {
        Lang::Ja => format!("30 日以上、更新を確認できていません（{kind}）。"),
        Lang::En => {
            format!("MKLM has not been able to check for updates for 30 days or more ({kind}).")
        }
    }
}

// --- The hand-off and the result (design m5b E.4, E.5) ---------------------------------------

pub fn handoff_title(lang: Lang) -> String {
    pick(lang, "MKLM を更新しています", "Updating MKLM")
}

/// The hand-off overlay (design m5b E.4 step 3; about 110 characters, read out assertively).
pub fn handoff_text(lang: Lang) -> String {
    pick(
        lang,
        "MKLM を更新しています。MKLM はまもなく終了し、更新が終わると 1 分ほどで自動で開きます。開き直すまで、サインアウトや再起動はしないでください。2 分たっても開かない場合は、スタート メニューから MKLM を開いてください。",
        "MKLM is being updated. MKLM closes in a moment and opens again by itself about a minute after the update is done. Until then, do not sign out or restart. If it has not opened after 2 minutes, open MKLM from the Start menu.",
    )
}

pub fn result_title(lang: Lang) -> String {
    pick(lang, "MKLM の更新", "MKLM update")
}

/// "技術的な詳細" lines: the installer's exit code.
pub fn installer_exit_code(code: u32, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("インストーラーの終了コード {code}"),
        Lang::En => format!("installer exit code {code}"),
    }
}

// --- The first-run wizard (design m5b E.2) ---------------------------------------------------

/// The welcome step's line about the daily check.
pub fn wizard_daily_check(lang: Lang) -> String {
    pick(
        lang,
        "MKLM は 1 日に 1 回、GitHub で新しい版を確かめます（設定でオフにできます）。",
        "MKLM checks GitHub for a new version once a day (you can turn this off in Settings).",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params {
            installed: "0.2.0".into(),
            offered: "0.2.1".into(),
            path: r"D:\dev\target\debug".into(),
            installer: "MKLM-Setup-0.2.1-x64.exe".into(),
            needed_mb: 12,
            programs: "MKLM、mklm-cli".into(),
            reason: installer_env_reason(true, Lang::Ja),
            date: "2027/04/13".into(),
            state_now: version_now("0.2.0", Lang::Ja),
        }
    }

    #[test]
    fn every_message_has_both_texts_and_its_own_id() {
        let mut ids = std::collections::BTreeSet::new();
        for msg in Msg::ALL {
            assert!(ids.insert(msg.id()), "{msg:?}");
            assert!(msg.id().starts_with("upd-"));
            for lang in [Lang::Ja, Lang::En] {
                assert!(!text(msg, &params(), lang).is_empty(), "{msg:?}");
            }
        }
        assert_eq!(
            text(Msg::Installed, &params(), Lang::Ja),
            "MKLM を 0.2.1 に更新しました。"
        );
        assert_eq!(
            text(Msg::NetDl, &params(), Lang::Ja),
            "更新をダウンロードできませんでした。インターネットにつながっているか確かめてください。会社や学校のネットワークでは、プロキシの設定が必要なことがあります。"
        );
        assert!(text(Msg::DiskFull, &params(), Lang::Ja).contains("あと約 12 MB"));
        assert_eq!(
            text(Msg::Timeout, &params(), Lang::Ja),
            "インストーラーが 60 分たっても終わりませんでした。今の MKLM は 0.2.0 です。"
        );
    }

    /// "Not installed" and "failed" always say the keyboards did not change (design m5b E.6).
    #[test]
    fn keyboards_are_always_mentioned_once() {
        let with = with_keyboards_unchanged(text(Msg::InstanceBusy, &params(), Lang::Ja), Lang::Ja);
        assert!(
            with.ends_with("キーボードの設定は変わっていません。"),
            "{with}"
        );
        let once = with_keyboards_unchanged(text(Msg::NotInstalled, &params(), Lang::Ja), Lang::Ja);
        assert_eq!(once.matches("キーボードの設定").count(), 1);
        let en = with_keyboards_unchanged(text(Msg::FileWrite, &params(), Lang::En), Lang::En);
        assert!(
            en.ends_with(" The keyboard settings did not change."),
            "{en}"
        );
    }

    #[test]
    fn sizes_and_labels() {
        assert_eq!(megabytes(6_291_456), "6.0");
        assert_eq!(megabytes(3_040_000), "2.9");
        assert_eq!(megabytes(0), "0.0");
        assert_eq!(
            downloading(3_040_000, 6_291_456, 48, Lang::Ja),
            "ダウンロード中: 2.9 MB / 6.0 MB（48 %）"
        );
        assert_eq!(
            button_label(Button::Skip, "0.2.1", Lang::Ja),
            "0.2.1 をスキップ"
        );
        assert_eq!(
            button_label(Button::UpdateNowPrompt, "0.2.1", Lang::Ja),
            "今すぐ 0.2.1 に更新"
        );
        assert_eq!(
            button_label(Button::Details, "0.2.1", Lang::Ja),
            "MKLM の更新の詳細を開く"
        );
        assert_eq!(
            button_label(Button::RunInstaller, "0.2.1", Lang::Ja),
            "インストーラーを実行（管理者の許可が要ります）"
        );
        // The run-installer path is never called verified (RED-TEAM-2).
        for lang in [Lang::Ja, Lang::En] {
            let label = button(Button::RunInstaller, lang);
            assert!(!label.contains("検証") && !label.contains("確かめ"));
            assert!(!label.to_lowercase().contains("verified"));
        }
    }
}
