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
  const count = async (a, b, blk, dy) => {
    const o = await run(`clone ${a} ${b} 60 ${dy} 60 filtered ${blk}`);
    if (/^No blocks cloned/i.test(o)) return 0;
    const m = o.match(/^(\d+) blocks? cloned/);
    return m ? Number(m[1]) : 'UNPARSED:' + o;
  };

  await run('gamerule random_tick_speed 400');
  await run('time set day');
  await run('weather clear');

  // control A: snow layer in open daylight. Pumpkin already ships snow.rs and
  // vanilla melts snow on getMaxLocalRawBrightness > 11, which includes SKY light.
  await run('setblock 30 100 30 minecraft:stone');
  await run('setblock 30 101 30 minecraft:snow');

  // control B: ice ringed with glowstone. Vanilla melts ice on BLOCK light only.
  await run('setblock 34 101 30 minecraft:ice');
  for (const p of ['35 101 30', '33 101 30', '34 102 30', '34 101 31', '34 101 29']) {
    await run(`setblock ${p} minecraft:glowstone`);
  }

  await sleep(20000);

  const snow = await count('30 101 30', '30 101 30', 'minecraft:snow', 130);
  const ice = await count('34 101 30', '34 101 30', 'minecraft:ice', 131);
  const water = await count('34 101 30', '34 101 30', 'minecraft:water', 132);
  console.log('snow layer in daylight still present :', snow, '(0 means it melted)');
  console.log('ice ringed by glowstone still present:', ice, '(0 means it melted)');
  console.log('water where the ice was              :', water);
  r.end();
})();
