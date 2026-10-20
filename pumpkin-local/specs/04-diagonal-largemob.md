WORKTREE: /home/eshanki/pumpkin-wt/crithit
FILES: pumpkin/src/entity/ai/pathfinder/walk_node_evaluator.rs:55-95
CRATES: pumpkin
FUNC: fn is_diagonal_valid(
---
# Bug: diagonal pathfinding is missing vanilla's large-mob early-out

You are shown `is_diagonal_valid`. Vanilla's version begins with a guard that
this port does not have: a mob wider than one block may NOT cut a diagonal when
either of the two adjacent orthogonal nodes carries a positive cost malus.

VERIFIED VANILLA `WalkNodeEvaluator.isDiagonalValid` opens with, in effect:

    if (mob.getBbWidth() > 1.0F && (northSouthNode.costMalus > 0.0F
                                 || eastWestNode.costMalus > 0.0F)) {
        return false;
    }

Without it, wide mobs such as ravagers and horses cut diagonals past
penalised blocks that vanilla makes them path around.

# Change

Add that early-out at the START of `is_diagonal_valid`, before the existing
checks. Use whatever the function already has in scope:
- the mob's bounding-box width. The struct stores it as something like
  `self.base.entity_width`; use the existing field rather than adding a
  parameter.
- the two adjacent orthogonal nodes the function already takes or looks up.
  Read their `cost_malus` field.

Add a comment naming the vanilla behaviour:
    // Vanilla refuses diagonals for mobs wider than one block when either
    // adjacent node is penalised.

# Do not

- Do not change any other function in this file.
- Do not alter the existing diagonal checks that follow; only prepend.
- If the two adjacent nodes are not already available in this function, do NOT
  invent a lookup. Stop and report that the signature does not expose them.
