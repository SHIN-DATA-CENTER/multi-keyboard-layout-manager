//! Physical keyboard rows shared by the CLI and GUI standard-layout preview.
use mklm_core::{
    EffectiveLayout, KeyboardDriver, LayoutTable, StandardPlan, SystemSnapshot, assess,
    group_keyboards, live_reset_bans,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandardRole {
    Assigned { ps2: bool },
    Pinned,
    Follows,
    Mixed,
    RemoteDesktop,
    NotAssignable { follows: Option<bool> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardRow {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub present: bool,
    pub before: Option<EffectiveLayout>,
    pub after: Option<LayoutTable>,
    pub role: StandardRole,
    pub can_reset: bool,
}

pub fn standard_rows(snapshot: &SystemSnapshot, plan: &StandardPlan) -> Vec<StandardRow> {
    let before = assess(snapshot);
    let after = assess(&SystemSnapshot {
        keyboards: plan.plan.checked.keyboards.clone(),
        global: plan.plan.checked.global.clone(),
        ..snapshot.clone()
    });
    group_keyboards(&snapshot.keyboards)
        .into_iter()
        .map(|group| {
            let members: Vec<_> = snapshot
                .keyboards
                .iter()
                .filter(|kb| group.keyboards.contains(&kb.instance_id))
                .collect();
            let has = |ids: &[String]| members.iter().any(|kb| ids.contains(&kb.instance_id));
            let pinned = has(&plan.change.pinned);
            let follows = has(&plan.change.following);
            let role = if members.iter().any(|kb| kb.is_remote_desktop()) {
                StandardRole::RemoteDesktop
            } else if let Some((_, follows)) = plan
                .change
                .not_assignable
                .iter()
                .find(|(id, _)| group.keyboards.contains(id))
            {
                StandardRole::NotAssignable { follows: *follows }
            } else if follows {
                StandardRole::Follows
            } else if pinned
                && members
                    .iter()
                    .any(|kb| !plan.change.pinned.contains(&kb.instance_id))
            {
                StandardRole::Mixed
            } else if pinned {
                StandardRole::Pinned
            } else {
                StandardRole::Assigned {
                    ps2: members
                        .iter()
                        .any(|kb| kb.driver == KeyboardDriver::I8042prt),
                }
            };
            let uniform = |assessment: &mklm_core::Assessment| {
                let mut layouts = members.iter().map(|kb| {
                    assessment
                        .keyboards
                        .iter()
                        .find(|a| a.instance_id == kb.instance_id)
                        .and_then(|a| a.after_restart.clone())
                });
                let first = layouts.next().flatten();
                if layouts.all(|next| next == first) {
                    first
                } else {
                    None
                }
            };
            let can_reset = members.iter().any(|kb| {
                plan.change.pinned.contains(&kb.instance_id)
                    && kb.present
                    && live_reset_bans(kb, false).is_empty()
            });
            StandardRow {
                id: if group.is_internal {
                    None
                } else {
                    group.container_id.clone()
                }
                .unwrap_or_else(|| group.keyboards[0].clone()),
                name: group.display_name,
                present: members.iter().any(|kb| kb.present),
                before: uniform(&before),
                after: uniform(&after).map(|l| l.table),
                members: group.keyboards,
                role,
                can_reset,
            }
        })
        .collect()
}
