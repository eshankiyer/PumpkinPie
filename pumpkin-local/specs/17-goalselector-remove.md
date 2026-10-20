WORKTREE: /home/eshanki/pumpkin-wt/goalremove
FILES: pumpkin/src/entity/ai/goal/goal_selector.rs:15-70
CRATES: pumpkin
FUNC: pub async fn remove_goal<G: Goal + 'static>(&mut self, mob: &dyn Mob) {
ASSERT_ABSENT: for goal_idx in goals_to_remove {
---
`GoalSelector::remove_goal` collects indices, then removes them with
`swap_remove` while fixing up `goals_by_control` as though `Vec::remove` had been
used. The two disagree, and the loop is also wrong for more than one removal.

    for goal_idx in goals_to_remove {
        self.goals.swap_remove(goal_idx);
        for slot in &mut self.goals_by_control {
            ...
            } else if *slot > goal_idx {

`Vec::remove` shifts every later element down by one, which is what the
`*slot > goal_idx` decrement assumes. `swap_remove` does something completely
different: it moves the LAST element into `goal_idx` and shortens the vector. So
after a `swap_remove` the only index that moved is `len - 1`, which is now at
`goal_idx`, and every other index is unchanged. The decrement corrupts them.

Second bug: the collected indices are stale after the first removal. Removing
index 2 and then index 5 from the original numbering removes the wrong goal,
because 5 no longer refers to what it did when the list was built.

This function has zero call sites today. That makes it safe to change and makes
it a landmine for whoever calls it first.

Fix it: iterate the indices in descending order so earlier indices stay valid,
and make the `goals_by_control` fixup match `swap_remove` exactly. A slot equal
to the removed index becomes `usize::MAX`; a slot equal to the old last index
becomes the removed index; every other slot is untouched. Do not decrement
anything.

If a correct fixup turns out to be more code than the alternative, the acceptable
alternative is to switch the removal to `Vec::remove`, which makes the existing
decrement correct, at the cost of being O(n). Either is fine. Deleting the
function is also acceptable, since nothing calls it. Pick one, and say which in
a comment.

Keep the `stop(mob).await` on running goals. That part is right.

Clippy denies all, pedantic, nursery and cargo. Identifiers in doc comments need
backticks. No emoji, no em-dashes.
