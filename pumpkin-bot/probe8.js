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
  // does a blockstate-qualified clone filter parse?
  await run('setblock 20 100 20 minecraft:wheat');
  console.log('setblock wheat plain :', await run('setblock 20 100 20 minecraft:wheat'));
  console.log('setblock wheat age=0 :', await run('setblock 21 100 20 minecraft:wheat[age=0]'));
  console.log('clone filt wheat     :', await run('clone 20 100 20 21 100 20 40 121 40 filtered minecraft:wheat'));
  console.log('clone filt wheat[a=0]:', await run('clone 20 100 20 21 100 20 40 122 40 filtered minecraft:wheat[age=0]'));

  // ice melt needs BLOCK light in vanilla, not sky light
  await run('gamerule random_tick_speed 300');
  await run('setblock 24 100 24 minecraft:ice');
  await run('setblock 25 100 24 minecraft:glowstone');
  await run('setblock 23 100 24 minecraft:glowstone');
  await run('setblock 24 101 24 minecraft:glowstone');
  await sleep(15000);
  console.log('ice next to glowstone:', await run('clone 24 100 24 24 100 24 40 123 40 filtered minecraft:ice'));
  console.log('water there          :', await run('clone 24 100 24 24 100 24 40 124 40 filtered minecraft:water'));
  r.end();
})();
