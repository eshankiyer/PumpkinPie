// Live verification over RCON.
//
// Primitives that actually work on this build:
//   write : setblock <pos> <block>
//   count : clone <a> <b> <dest> filtered <block>  ->  "N blocks cloned"
// `execute if block` ignores its predicate and `fill` needs the console-world
// fix, so neither is used here.
const { Rcon } = require('rcon-client');

const HOST = '192.168.1.93';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let rcon;
const run = async (c) => {
  try {
    return String(await rcon.send(c)).replace(/\n/g, ' | ').trim();
  } catch (e) {
    return 'ERR ' + e.message;
  }
};

// Count blocks of `block` in the inclusive region a..b by cloning it to a
// scratch area with a filter and reading the reported count.
const countBlocks = async (a, b, block) => {
  const out = await run(`clone ${a} ${b} 40 120 40 filtered ${block}`);
  if (/^No blocks cloned/i.test(out)) return 0;
  const m = out.match(/^(\d+) blocks? cloned/);
  return m ? Number(m[1]) : `UNPARSED(${out})`;
};

const results = [];
const record = (name, pass, detail) => {
  results.push({ name, pass, detail });
  console.log((pass === true ? 'PASS ' : pass === false ? 'FAIL ' : 'INFO ') + name + '  ::  ' + detail);
};

async function clearArena() {
  for (let x = 0; x <= 12; x++) {
    for (let z = 0; z <= 12; z++) {
      for (let y = 100; y <= 104; y++) {
        await run(`setblock ${x} ${y} ${z} minecraft:air`);
      }
    }
  }
}

async function main() {
  rcon = await Rcon.connect({ host: HOST, port: 25575, password: 'pumpkintest' });

  await run('gamerule advance_weather false');
  await run('weather clear');
  await run('time set day');

  // The arena sits at world spawn, which is terrain, so clear it and open the
  // sky above it. Everything below depends on these blocks being sky lit.
  console.log('clearing arena...');
  await clearArena();
  const skyCheck = await countBlocks('0 105 0', '12 130 12', 'minecraft:air');
  console.log('open air volume above arena: ' + skyCheck + ' of ' + 13 * 26 * 13);

  // ---- 1. fire coral bleaches when dry (PR #2571) ----
  await run('setblock 1 100 1 minecraft:fire_coral_block');
  await run('gamerule random_tick_speed 200');
  await sleep(9000);
  {
    const dead = await countBlocks('1 100 1', '1 100 1', 'minecraft:dead_fire_coral_block');
    const alive = await countBlocks('1 100 1', '1 100 1', 'minecraft:fire_coral_block');
    record(
      '#2571 fire coral bleaches to DEAD_FIRE (not itself)',
      dead === 1 && alive === 0,
      `dead=${dead} alive=${alive}`,
    );
  }

  // ---- 2. ice melts in bright light (PR #2579) ----
  await run('setblock 3 100 3 minecraft:ice');
  await sleep(9000);
  {
    const ice = await countBlocks('3 100 3', '3 100 3', 'minecraft:ice');
    const water = await countBlocks('3 100 3', '3 100 3', 'minecraft:water');
    record('#2579 ice melts to water in light', ice === 0 && water === 1, `ice=${ice} water=${water}`);
  }

  // ---- 3. crops need light (PR #2567) ----
  // lit wheat should advance, sealed wheat should not
  await run('setblock 5 99 5 minecraft:farmland[moisture=7]');
  await run('setblock 5 100 5 minecraft:wheat[age=0]');
  await run('setblock 7 99 7 minecraft:farmland[moisture=7]');
  await run('setblock 7 100 7 minecraft:wheat[age=0]');
  // seal the second one from light
  for (const [dx, dy, dz] of [[0, 1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]) {
    await run(`setblock ${7 + dx} ${100 + dy} ${7 + dz} minecraft:stone`);
  }
  await sleep(14000);
  {
    const litStill0 = await countBlocks('5 100 5', '5 100 5', 'minecraft:wheat');
    const darkStill0 = await countBlocks('7 100 7', '7 100 7', 'minecraft:wheat');
    record(
      '#2567 wheat present lit / sealed (state read is coarse)',
      null,
      `lit_wheat=${litStill0} sealed_wheat=${darkStill0}`,
    );
  }

  // ---- 4. blockDrops gamerule gates BOTH loot and experience (PR #2575) ----
  {
    const before = await run('gamerule block_drops');
    await run('gamerule block_drops false');
    const off = await run('gamerule block_drops');
    await run('gamerule block_drops true');
    record(
      '#2575 block_drops gamerule is readable and settable',
      off.includes('false'),
      `${before} -> ${off}`,
    );
  }

  // ---- 5. random_tick_speed gamerule is honoured (fix/tickspeed, not yet PR'd) ----
  {
    await run('gamerule random_tick_speed 0');
    await run('setblock 9 100 9 minecraft:ice');
    await sleep(9000);
    const iceAt0 = await countBlocks('9 100 9', '9 100 9', 'minecraft:ice');
    await run('gamerule random_tick_speed 200');
    await sleep(9000);
    const iceAt200 = await countBlocks('9 100 9', '9 100 9', 'minecraft:ice');
    record(
      'random_tick_speed 0 stops random ticks, 200 resumes',
      iceAt0 === 1 && iceAt200 === 0,
      `ice@speed0=${iceAt0} (want 1) ice@speed200=${iceAt200} (want 0)`,
    );
  }

  // ---- 6. /fill works from RCON (PR #2583) ----
  {
    const out = await run('fill 10 100 10 12 100 12 minecraft:stone');
    const n = await countBlocks('10 100 10', '12 100 12', 'minecraft:stone');
    record('#2583 /fill works from RCON', !out.includes('Internal error') && n === 9, `"${out}" placed=${n}`);
  }

  // ---- 7. grass spreads to adjacent dirt (PR #2580) ----
  await run('setblock 2 100 6 minecraft:grass_block');
  await run('setblock 3 100 6 minecraft:dirt');
  await sleep(14000);
  {
    const grass = await countBlocks('3 100 6', '3 100 6', 'minecraft:grass_block');
    record('#2580 grass spreads onto lit adjacent dirt', grass === 1, `dirt_became_grass=${grass}`);
  }

  console.log('\n===== SUMMARY =====');
  for (const r of results) {
    console.log((r.pass === true ? 'PASS' : r.pass === false ? 'FAIL' : 'INFO').padEnd(5) + r.name);
  }
  rcon.end();
}

main().catch((e) => {
  console.log('SUITE ERROR ' + e.message);
  process.exit(1);
});
