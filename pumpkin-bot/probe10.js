const { Rcon } = require('rcon-client');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const r = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' });
  const run = async (c) => {
    try {
      return String(await r.send(c)).replace(/\n/g, ' | ').trim();
    } catch (e) {
      return 'ERR ' + e.message;
    }
  };
  let dy = 140;
  const count = async (a, b, blk) => {
    const o = await run(`clone ${a} ${b} 60 ${dy++} 60 filtered ${blk}`);
    if (/^No blocks cloned/i.test(o)) return 0;
    const m = o.match(/^(\d+) blocks? cloned/);
    return m ? Number(m[1]) : 'UNPARSED:' + o;
  };

  await run('gamerule random_tick_speed 500');
  await run('time set day');
  await run('weather clear');

  // Clear a lane and open the sky above it.
  for (let x = 44; x <= 52; x++) {
    for (let y = 101; y <= 112; y++) await run(`setblock ${x} ${y} 40 minecraft:air`);
  }

  // grass source next to bare dirt, both sky exposed
  await run('setblock 45 100 40 minecraft:grass_block');
  await run('setblock 46 100 40 minecraft:dirt');
  await run('setblock 47 100 40 minecraft:dirt');

  // a second dirt sealed under stone as the negative control
  await run('setblock 50 100 40 minecraft:grass_block');
  await run('setblock 51 100 40 minecraft:dirt');
  await run('setblock 51 101 40 minecraft:stone');

  console.log('waiting 60s at random_tick_speed 500 ...');
  await sleep(60000);

  const lit1 = await count('46 100 40', '46 100 40', 'minecraft:grass_block');
  const lit2 = await count('47 100 40', '47 100 40', 'minecraft:grass_block');
  const sealed = await count('51 100 40', '51 100 40', 'minecraft:grass_block');
  console.log('adjacent lit dirt  -> grass:', lit1, '(want 1)');
  console.log('two-away lit dirt  -> grass:', lit2, '(want 1 eventually)');
  console.log('sealed dirt        -> grass:', sealed, '(want 0)');

  // sanity: did the source grass survive, i.e. is it not being killed
  const src = await count('45 100 40', '45 100 40', 'minecraft:grass_block');
  console.log('source grass still grass  :', src, '(want 1)');
  r.end();
})();
