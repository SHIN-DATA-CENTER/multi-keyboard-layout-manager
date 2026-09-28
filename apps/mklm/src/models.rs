//! Kept Slint models (design m3 A.6, review A7): a render updates a list's model in place rather
//! than handing the property a new model, for which a Repeater would rebuild every row — and the
//! focus, the radio buttons' accessible nodes and what a screen reader was reading with them.
//! Lists of things with an identity (keyboards, history rows) use `vm::list_ops`; these helpers
//! are for short positional lists such as the options of a radio group.

use std::rc::Rc;

use slint::{Model, ModelRc, VecModel};

/// Makes `model` hold `rows`, position by position: a changed row is set, missing rows are added
/// and extra rows removed at the end; unchanged rows are left alone.
pub fn sync_rows<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rows: impl IntoIterator<Item = T>,
) {
    let mut count = 0;
    for (index, row) in rows.into_iter().enumerate() {
        count = index + 1;
        if index < model.row_count() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        } else {
            model.push(row);
        }
    }
    while model.row_count() > count {
        model.remove(model.row_count() - 1);
    }
}

/// One model kept for a property's lifetime. [`KeptModel::show`] always returns the same model,
/// which Slint does not treat as a new one (a Repeater compares the model it had).
pub struct KeptModel<T>(Rc<VecModel<T>>);

impl<T> std::fmt::Debug for KeptModel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptModel").finish_non_exhaustive()
    }
}

impl<T: Clone + PartialEq + 'static> Default for KeptModel<T> {
    fn default() -> Self {
        Self(Rc::new(VecModel::default()))
    }
}

impl<T: Clone + PartialEq + 'static> KeptModel<T> {
    /// Updates the kept model to `rows` ([`sync_rows`]) and returns it for the property.
    pub fn show(&self, rows: impl IntoIterator<Item = T>) -> ModelRc<T> {
        sync_rows(&self.0, rows);
        ModelRc::from(self.0.clone())
    }
}

/// `new` if its rows differ from `old`'s, else `old` itself: a row set into a kept list keeps the
/// model of its own sub-list (a conflict row's options) when that sub-list did not change.
pub fn same_or_new<T: PartialEq + Clone + 'static>(
    old: &ModelRc<T>,
    new: ModelRc<T>,
) -> ModelRc<T> {
    let same = old.row_count() == new.row_count() && old.iter().eq(new.iter());
    if same { old.clone() } else { new }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(model: &ModelRc<u32>) -> Vec<u32> {
        model.iter().collect()
    }

    #[test]
    fn a_kept_model_is_updated_in_place() {
        let kept = KeptModel::default();
        let first = kept.show([1, 2, 3]);
        assert_eq!(rows(&first), [1, 2, 3]);
        for next in [
            vec![1, 5, 3],
            vec![1],
            vec![],
            vec![4, 4, 4, 4],
            vec![4, 4, 4, 4],
        ] {
            let shown = kept.show(next.clone());
            assert_eq!(rows(&shown), next);
            assert!(shown == first, "the same model every time");
        }
    }

    #[test]
    fn an_unchanged_sub_list_keeps_its_model() {
        let old = ModelRc::new(VecModel::from(vec![1_u32, 2]));
        let same = same_or_new(&old, ModelRc::new(VecModel::from(vec![1, 2])));
        assert!(same == old);
        let changed = same_or_new(&old, ModelRc::new(VecModel::from(vec![1, 3])));
        assert!(changed != old);
        assert_eq!(rows(&changed), [1, 3]);
    }
}
