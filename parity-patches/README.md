# Pumpkin 26.2 parity patch series

These patches contain production and regression-test changes only. They exclude the
local JDK, decompiler, decompiled Minecraft sources, audit artifacts, roadmap, quirk
backlog, agent guidelines, and local handoff.

Apply them in the exact order listed in `series`, from the campaign base containing
commit `cc5d51e`:

```bash
while IFS= read -r patch; do git am "parity-patches/$patch"; done < parity-patches/series
```

The crafting patches are ordered: the matcher/remainder implementation precedes its
generated-data feature and the later transmute component-preservation patch.

Several early fixes already have open upstream pull requests. Before applying to a
newer upstream branch, skip commits already present and prefer their reviewed PR form.
