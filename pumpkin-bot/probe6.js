const { Rcon } = require('rcon-client');
(async () => {
  const r = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' });
  const run = async (c) => {
    try {
      return String(await r.send(c)).replace(/\n/g, ' | ');
    } catch (e) {
      return 'ERR ' + e.message;
    }
  };
  await run('setblock 8 101 8 minecraft:stone');
  console.log('POSITIVE (is stone):', await run('execute if block 8 101 8 minecraft:stone run seed'));
  console.log('NEGATIVE (is air?) :', await run('execute if block 8 101 8 minecraft:air run seed'));
  console.log('UNLESS  (not air)  :', await run('execute unless block 8 101 8 minecraft:air run seed'));
  // blockstate-qualified match, needed for things like farmland moisture
  await run('setblock 7 101 8 minecraft:farmland[moisture=0]');
  console.log('farmland moisture=0:', await run('execute if block 7 101 8 minecraft:farmland[moisture=0] run seed'));
  console.log('farmland moisture=7:', await run('execute if block 7 101 8 minecraft:farmland[moisture=7] run seed'));
  r.end();
})();
