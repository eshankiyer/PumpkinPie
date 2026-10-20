# Sweep proposals (WORKQUEUE stubs, STATE suspected)

Auto-filed by oracle_sweep.py. These are candidate WORKQUEUE items in the
queue's own ID/STATE/WHERE/WHAT/VANILLA/DO/CARE shape, but they are
NEVER promoted here: every one is STATE suspected. A human or Claude reads
the evidence in DISCOVERED.md, confirms the code site, and only then
copies a hardened item into WORKQUEUE.md. This file is a staging area, not
the queue. The vanilla line is oracle-OBSERVED (live RCON), which is why
these are worth a look, but 'suspected' still holds until the WHERE is
pinned to real code.

---

### SWEEP-selector-type_namespaced  @e[type=minecraft:pig] namespaced entity type
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this selector probe (filed 2026-07-30 08:34).
SEQ `kill @e[type=minecraft:pig,limit=0]`
VANILLA (oracle-observed) obs=`REJECTED` reply=`Limit must be at least 1...pig,limit=0]<--[HERE]`
PUMPKIN obs=`UNKNOWN_ENTITY` reply=`Invalid or unknown entity type 'minecraft'...l @e[type=minecraft:pig,limit=0]<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-selector-type_negated  @e[type=!minecraft:player] negated type
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this selector probe (filed 2026-07-30 08:34).
SEQ `kill @e[type=!minecraft:player,limit=0]`
VANILLA (oracle-observed) obs=`REJECTED` reply=`Limit must be at least 1...yer,limit=0]<--[HERE]`
PUMPKIN obs=`UNKNOWN_ENTITY` reply=`Invalid or unknown entity type 'minecraft'...l @e[type=!minecraft:player,limit=0]<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-selector-type_tag  @e[type=#minecraft:skeletons] entity type tag
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this selector probe (filed 2026-07-30 08:34).
SEQ `kill @e[type=#minecraft:skeletons,limit=0]`
VANILLA (oracle-observed) obs=`REJECTED` reply=`Limit must be at least 1...ons,limit=0]<--[HERE]`
PUMPKIN obs=`UNKNOWN_ENTITY` reply=`Invalid or unknown entity type ''...l @e[type=#minecraft:skeletons,limit=0]<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-argument-summon_rel_norun  summon at relative coords via rcon (no origin)
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this argument probe (filed 2026-07-30 08:34).
SEQ `summon minecraft:pig ~ ~ ~`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`Summoned new Pig`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...:pig ~ ~ ~<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-argument-give_component  give with a data-component argument
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this argument probe (filed 2026-07-30 08:34).
SEQ `give @a[limit=0] minecraft:stone[minecraft:custom_name='x'] 1`
VANILLA (oracle-observed) obs=`REJECTED` reply=`Limit must be at least 1... @a[limit=0] minecraft:stone[minecraft:custom_name='x'] 1<--[HERE]`
PUMPKIN obs=`SUCCESS` reply=`Gave 1 * [Stone] to 0`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-argument-particle_named  particle by name with coords
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this argument probe (filed 2026-07-30 08:34).
SEQ `particle minecraft:flame 0 200 0 0 0 0 0 1 force`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`The particle was not visible for anybody`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...0 0 0 0 1 force<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-summon_nbt-nogravity  summon armor_stand with {NoGravity:1b}
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this summon_nbt probe (filed 2026-07-30 08:34).
SEQ `summon minecraft:armor_stand 2 200 2 {NoGravity:1b}`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`Summoned new Armor Stand`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...d 2 200 2 {NoGravity:1b}<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-summon_nbt-customname  summon with a CustomName component
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this summon_nbt probe (filed 2026-07-30 08:34).
SEQ `summon minecraft:armor_stand 2 200 2 {CustomName:'{"text":"x"}'}`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`Summoned new {"text":"x"}`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...d 2 200 2 {CustomName:'{"text":"x"}'}<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-summon_nbt-tags  summon carrying scoreboard Tags
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this summon_nbt probe (filed 2026-07-30 08:34).
SEQ `summon minecraft:armor_stand 2 200 2 {Tags:["sweep"]}`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`Summoned new Armor Stand`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...d 2 200 2 {Tags:["sweep"]}<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.

### SWEEP-summon_nbt-passengers  summon with a Passengers stack
STATE suspected
WHERE `TBD - localise from the command sequence below`
WHAT vanilla and Pumpkin give different observable outcomes for this summon_nbt probe (filed 2026-07-30 08:34).
SEQ `summon minecraft:armor_stand 2 200 2 {Passengers:[{id:"minecraft:pig"}]}`
VANILLA (oracle-observed) obs=`SUCCESS` reply=`Summoned new Armor Stand`
PUMPKIN obs=`REJECTED` reply=`Incorrect argument for command...d 2 200 2 {Passengers:[{id:"minecraft:pig"}]}<--[HERE]`
DO Verify the vanilla side against 26.2, pin WHERE to code, then promote to WORKQUEUE.md as confirmed.
CARE Outcome-class diff only; wording differences are already normalised out, so a divergence here is a real class change.
